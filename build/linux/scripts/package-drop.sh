#!/usr/bin/env bash
set -euo pipefail

fail() {
    printf 'Release packaging error: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 2 ]] || {
    echo 'Usage: package-drop.sh APPIMAGE_OUTPUT_DIRECTORY DAT_PACKER' >&2
    exit 2
}
output_directory=$1
packer=$2
[[ $output_directory == /* && $output_directory != / && $packer == /* ]] ||
    fail 'the output directory and DAT packer paths must be absolute'

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
source_root=$(cd -- "$script_dir/../../.." && pwd -P)
wrapper="$source_root/build/linux/scripts/pack-dat.sh"
manifest="$source_root/assets/dat-manifest.toml"
image="$output_directory/flutzPlayer-x86_64.AppImage"
metadata="$output_directory/metadata.json"
data="$output_directory/data"
archive_name=flutzPlayer-linux-x86_64.tar.gz
checksum_name=$archive_name.sha256.txt
export LC_ALL=C
unset LD_PRELOAD LD_LIBRARY_PATH

for path in "$wrapper" "$manifest" "$metadata"; do
    [[ -f $path && ! -L $path && -r $path ]] || fail "missing packaging input: $path"
done
for path in "$image" "$packer"; do
    [[ -f $path && ! -L $path && -x $path ]] || fail "missing or non-executable packaging input: $path"
done
[[ -d $data && ! -L $data ]] || fail "missing validated DAT directory: $data"
[[ $(head -c 64 -- "$wrapper") != *$'\r'* ]] || fail 'pack-dat.sh has CRLF line endings'

shopt -s nullglob
dat_files=("$data"/*)
shopt -u nullglob
((${#dat_files[@]} > 0)) || fail 'no DAT parts to package'
for index in "${!dat_files[@]}"; do
    printf -v expected 'assets-%03d.dat' "$index"
    path=${dat_files[index]}
    [[ ${path##*/} == "$expected" && -f $path && ! -L $path && -s $path ]] ||
        fail "unexpected or noncontiguous DAT directory entry: $path"
done

# One build timestamp for every entry: the AppImage build time from its metadata.
created=$(jq -er '.createdAtUtc' "$metadata")
epoch=$(date -u -d "$created" +%s) || fail "invalid metadata timestamp: $created"

package="$output_directory/package"
release="$output_directory/release"
rm -rf -- "$package" "$release"
mkdir -p -- "$package" "$release"
trap 'rm -rf -- "$package"' EXIT
stage="$package/flutzPlayer"
install -d -m 0755 -- "$stage" "$stage/data" "$stage/dat-packer"
ln -- "$image" "$stage/flutzPlayer-x86_64.AppImage"
chmod 0755 -- "$stage/flutzPlayer-x86_64.AppImage"
for path in "${dat_files[@]}"; do
    ln -- "$path" "$stage/data/${path##*/}"
    chmod 0644 -- "$stage/data/${path##*/}"
done
install -m 0755 -- "$packer" "$stage/dat-packer/dat-packer"
install -m 0755 -- "$wrapper" "$stage/dat-packer/pack-dat.sh"
install -m 0644 -- "$manifest" "$stage/dat-packer/dat-manifest.toml"

expected_tree="$package/expected-tree.txt"
{
    echo 'd 755 flutzPlayer'
    echo 'f 755 flutzPlayer/flutzPlayer-x86_64.AppImage'
    echo 'd 755 flutzPlayer/data'
    for path in "${dat_files[@]}"; do
        echo "f 644 flutzPlayer/data/${path##*/}"
    done
    echo 'd 755 flutzPlayer/dat-packer'
    echo 'f 755 flutzPlayer/dat-packer/dat-packer'
    echo 'f 755 flutzPlayer/dat-packer/pack-dat.sh'
    echo 'f 644 flutzPlayer/dat-packer/dat-manifest.toml'
} | sort > "$expected_tree"
tree_of() {
    (cd -- "$1" && find flutzPlayer -printf '%y %m %p\n' | sort)
}
diff -u -- "$expected_tree" <(tree_of "$package") ||
    fail 'staging tree does not match the distribution contract'
[[ -z $(find "$stage" -iname '*.ini' -print -quit) ]] || fail 'staging tree contains an INI file'

archive="$release/$archive_name"
tar --create --format=gnu --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@$epoch" --hard-dereference --no-acls --no-xattrs --no-selinux \
    --directory="$package" --file=- flutzPlayer | gzip -n -6 > "$archive"
archive_sha=$(sha256sum < "$archive" | cut -d ' ' -f 1)
printf '%s  %s\n' "$archive_sha" "$archive_name" > "$release/$checksum_name"
(cd -- "$release" && sha256sum --check --strict --quiet -- "$checksum_name") ||
    fail 'archive checksum verification failed'
checksum_sha=$(sha256sum < "$release/$checksum_name" | cut -d ' ' -f 1)

listing=$(TZ=UTC tar --list --verbose --numeric-owner --gzip --file="$archive")
awk -v epoch="$(date -u -d "@$epoch" '+%Y-%m-%d %H:%M')" '
    $2 != "0/0" { printf "archive entry is not owned by 0/0: %s\n", $0 > "/dev/stderr"; bad = 1 }
    ($4 " " $5) != epoch { printf "archive entry has an unexpected timestamp: %s\n", $0 > "/dev/stderr"; bad = 1 }
    $6 ~ /^\// || $6 ~ /(^|\/)\.\.(\/|$)/ || NF != 6 {
        printf "unsafe or unexpected archive entry: %s\n", $0 > "/dev/stderr"; bad = 1
    }
    END { exit bad }
' <<<"$listing" || fail 'archive ownership, timestamps, or entry names are invalid'

fresh="$package/fresh extraction"
mkdir -- "$fresh"
tar --extract --gzip --preserve-permissions --file="$archive" --directory="$fresh"
diff -u -- "$expected_tree" <(tree_of "$fresh") ||
    fail 'fresh extraction does not match the distribution contract'
[[ -z $(find "$fresh" -iname '*.ini' -print -quit) ]] || fail 'the archive ships an INI file'
while IFS= read -r -d '' path; do
    relative=${path#"$fresh/"}
    cmp -s -- "$path" "$package/$relative" || fail "extracted file differs from staging: $relative"
done < <(find "$fresh" -type f -print0)

extracted="$fresh/flutzPlayer"
run_dir="$package/elsewhere"
mkdir -- "$run_dir" "$run_dir/tmp" "$run_dir/xdg-data" "$run_dir/xdg-config"
inspect=$(cd -- "$run_dir" && env -u APPIMAGE -u APPDIR -u APPIMAGE_EXTRACT_AND_RUN \
    TMPDIR="$run_dir/tmp" XDG_DATA_HOME="$run_dir/xdg-data" XDG_CONFIG_HOME="$run_dir/xdg-config" \
    "$extracted/flutzPlayer-x86_64.AppImage" --appimage-extract-and-run --inspect-dat 2>&1) ||
    fail "extracted AppImage failed to inspect its data: $inspect"
grep -Fxq "DAT files: ${#dat_files[@]}" <<<"$inspect" ||
    fail "extracted AppImage did not discover ${#dat_files[@]} adjacent DAT parts: $inspect"
grep -Eq '^SoundFonts: [1-9][0-9]*$' <<<"$inspect" ||
    fail "extracted AppImage did not discover soundfonts: $inspect"

dependencies=$(ldd "$extracted/dat-packer/dat-packer" 2>&1) || [[ $dependencies == *'not a dynamic executable'* ]] ||
    fail "cannot inspect extracted DAT packer dependencies: $dependencies"
[[ $dependencies != *'not found'* ]] || fail "extracted DAT packer has missing libraries: $dependencies"
(cd -- "$run_dir" && "$extracted/dat-packer/dat-packer" --inspect \
    --input "$extracted/data/assets-000.dat" --format json) > "$run_dir/inspection.json"
jq -e --argjson count "${#dat_files[@]}" \
    '.dat_file_count == $count and .dat_byte_count > 0 and (.soundfonts | length > 0)' \
    "$run_dir/inspection.json" >/dev/null || fail 'extracted DAT packer inspection failed'
help=$(cd -- "$run_dir" && "$extracted/dat-packer/pack-dat.sh" --help)
[[ $help == 'Usage: pack-dat.sh'* ]] || fail 'extracted pack-dat.sh did not run'

while IFS= read -r -d '' path; do
    relative=${path#"$stage/"}
    printf 'RELEASE_FILE %s %s\n' "$(sha256sum < "$path" | cut -d ' ' -f 1)" "$relative"
done < <(find "$stage" -type f -print0 | sort -z)
printf 'RELEASE_ARCHIVE %s %s\n' "$archive_sha" "$checksum_sha"
