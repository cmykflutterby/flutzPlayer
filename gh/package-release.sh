#!/usr/bin/env bash
# Assembles and verifies the eight release archives from prebuilt inputs:
#   WINDOWS_DROP: flutzplayer.exe and dat-packer/{dat-packer.exe,pack-dat.ps1,dat-manifest.toml}
#   LINUX_BUILD:  flutzplayer, dat-packer, flutzPlayer-x86_64.AppImage, metadata.json
#   DAT_DIR:      assets-NNN.dat parts and their SHA256SUMS
# Binaries are packaged once per platform; full and nodat variants differ only by data/assets-*.dat.
set -euo pipefail

fail() {
    printf 'Release assembly error: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 7 ]] || {
    echo 'Usage: package-release.sh STAMP REVISION WINDOWS_DROP LINUX_BUILD DAT_DIR OUTPUT_DIR CHECKSUM_FILE' >&2
    exit 2
}
stamp=$1
revision=$2
windows_drop=$(realpath -e -- "$3")
linux_build=$(realpath -e -- "$4")
dat_dir=$(realpath -e -- "$5")
output_dir=$6
checksum_file=$7

[[ $stamp =~ ^[0-9]{12}$ ]] || fail "invalid UTC yyMMddHHmmss stamp: $stamp"
[[ $revision =~ ^[0-9a-f]{40}$ ]] || fail "invalid source revision: $revision"
epoch=$(date -u -d "20${stamp:0:2}-${stamp:2:2}-${stamp:4:2} ${stamp:6:2}:${stamp:8:2}:${stamp:10:2}" +%s) ||
    fail "stamp is not a valid UTC time: $stamp"

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
source_root=$(cd -- "$script_dir/.." && pwd -P)
[[ $(git -C "$source_root" rev-parse HEAD) == "$revision" ]] || fail 'checkout does not match the source revision'
export LC_ALL=C
gz=gzip
command -v pigz >/dev/null && gz=pigz

windows_archive="flutzPlayer-Windows-x86_64-$stamp.zip"
windows_nodat_archive="flutzPlayer-Windows-x86_64-nodat-$stamp.zip"
appimage_archive="flutzPlayer-Linux-AppImage-x86_64-$stamp.tar.gz"
appimage_nodat_archive="flutzPlayer-Linux-AppImage-x86_64-nodat-$stamp.tar.gz"
linux_archive="flutzPlayer-Linux-x86_64-$stamp.tar.gz"
linux_nodat_archive="flutzPlayer-Linux-x86_64-nodat-$stamp.tar.gz"
source_tar_archive="flutzPlayer-Source-$stamp.tar.gz"
source_zip_archive="flutzPlayer-Source-$stamp.zip"
expected_archives=(
    "$windows_archive" "$windows_nodat_archive"
    "$appimage_archive" "$appimage_nodat_archive"
    "$linux_archive" "$linux_nodat_archive"
    "$source_tar_archive" "$source_zip_archive"
)

# Input files must match their contracts exactly.
file_list() {
    (cd -- "$1" && find . -mindepth 1 \( -type f -o -type l \) -printf '%P\n' | sort)
}
diff -u <(printf '%s\n' dat-packer/dat-manifest.toml dat-packer/dat-packer.exe dat-packer/pack-dat.ps1 flutzplayer.exe) \
    <(file_list "$windows_drop") || fail 'unexpected Windows drop contents'
diff -u <(printf '%s\n' dat-packer flutzPlayer-x86_64.AppImage flutzplayer metadata.json) \
    <(file_list "$linux_build") || fail 'unexpected Linux build contents'
cmp -s -- "$source_root/assets/dat-manifest.toml" "$windows_drop/dat-packer/dat-manifest.toml" ||
    fail 'Windows DAT manifest differs from the source'
cmp -s -- "$source_root/tools/dat-packer/pack-dat.ps1" "$windows_drop/dat-packer/pack-dat.ps1" ||
    fail 'Windows pack-dat.ps1 differs from the source'

metadata="$linux_build/metadata.json"
sha_of() { sha256sum < "$1" | cut -d ' ' -f 1; }
[[ $(jq -er '.appimage.sha256' "$metadata") == "$(sha_of "$linux_build/flutzPlayer-x86_64.AppImage")" ]] ||
    fail 'AppImage checksum does not match its build metadata'
[[ $(jq -er '.build.binarySha256' "$metadata") == "$(sha_of "$linux_build/flutzplayer")" ]] ||
    fail 'the plain Linux binary differs from the binary packaged in the AppImage'
[[ $(jq -er '.build.sourceRevision' "$metadata") == "$revision" ]] ||
    fail 'AppImage was built from a different source revision'

mapfile -t dat_names < <(file_list "$dat_dir" | grep -vx SHA256SUMS)
((${#dat_names[@]} > 0)) || fail 'no DAT parts were provided'
for index in "${!dat_names[@]}"; do
    printf -v expected 'assets-%03d.dat' "$index"
    [[ ${dat_names[index]} == "$expected" ]] || fail "unexpected or noncontiguous DAT entry: ${dat_names[index]}"
done
diff -u <(printf '%s\n' "${dat_names[@]}") <(awk '{ print $2 }' "$dat_dir/SHA256SUMS" | sort) ||
    fail 'DAT checksum list does not match the DAT parts'
(cd -- "$dat_dir" && sha256sum --check --strict --quiet SHA256SUMS) || fail 'DAT checksum verification failed'

mkdir -p -- "$output_dir"
output_dir=$(realpath -e -- "$output_dir")
[[ -z $(find "$output_dir" -mindepth 1 -print -quit) ]] || fail "output directory is not empty: $output_dir"
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

# DAT parts are hard-linked into every full stage rather than copied.
chmod 0644 -- "$dat_dir"/assets-*.dat
touch -d "@$epoch" -- "$dat_dir"/assets-*.dat
add_dats() {
    local data=$1 name
    for name in "${dat_names[@]}"; do
        ln -- "$dat_dir/$name" "$data/$name" 2>/dev/null || cp -p -- "$dat_dir/$name" "$data/$name"
    done
}

stage_windows() {
    local root=$1 with_dat=$2
    install -d -m 0755 -- "$root" "$root/data" "$root/dat-packer"
    install -m 0755 -- "$windows_drop/flutzplayer.exe" "$root/flutzplayer.exe"
    install -m 0755 -- "$windows_drop/dat-packer/dat-packer.exe" "$root/dat-packer/dat-packer.exe"
    install -m 0644 -- "$windows_drop/dat-packer/pack-dat.ps1" "$root/dat-packer/pack-dat.ps1"
    install -m 0644 -- "$windows_drop/dat-packer/dat-manifest.toml" "$root/dat-packer/dat-manifest.toml"
    [[ $with_dat == 1 ]] && add_dats "$root/data"
    find "$root" -exec touch -h -d "@$epoch" -- {} +
}

# Layouts follow build/linux/scripts/package-drop.sh (AppImage) and LinuxBuild.ps1 (plain).
stage_linux() {
    local root=$1 kind=$2 with_dat=$3 app=$1/flutzPlayer
    install -d -m 0755 -- "$app" "$app/data" "$app/dat-packer"
    if [[ $kind == appimage ]]; then
        install -m 0755 -- "$linux_build/flutzPlayer-x86_64.AppImage" "$app/flutzPlayer-x86_64.AppImage"
    else
        install -m 0755 -- "$linux_build/flutzplayer" "$app/flutzplayer"
    fi
    install -m 0755 -- "$linux_build/dat-packer" "$app/dat-packer/dat-packer"
    install -m 0755 -- "$source_root/build/linux/scripts/pack-dat.sh" "$app/dat-packer/pack-dat.sh"
    install -m 0644 -- "$source_root/assets/dat-manifest.toml" "$app/dat-packer/dat-manifest.toml"
    [[ $with_dat == 1 ]] && add_dats "$app/data"
    find "$root" -exec touch -h -d "@$epoch" -- {} +
}

tree_of() {
    (cd -- "$1" && find . -mindepth 1 -printf '%M %P\n' | sort -k 2)
}
hashes_of() {
    (cd -- "$1" && find . -type f -printf '%P\0' | xargs -0 -r sha256sum) | sort -k 2
}

make_tar() {
    local stage=$1 archive=$2 listing
    tar --create --format=gnu --sort=name --owner=0 --group=0 --numeric-owner \
        --mtime="@$epoch" --hard-dereference --no-acls --no-xattrs \
        --directory="$stage" --file=- flutzPlayer | "$gz" -n -6 > "$archive"

    listing=$(TZ=UTC tar --list --verbose --numeric-owner --gzip --file="$archive")
    awk -v when="$(date -u -d "@$epoch" '+%Y-%m-%d %H:%M')" '
        $2 != "0/0" || ($4 " " $5) != when || NF != 6 || $6 ~ /^\// || $6 ~ /(^|\/)\.\.(\/|$)/ {
            printf "invalid archive entry: %s\n", $0 > "/dev/stderr"; bad = 1
        }
        END { exit bad }
    ' <<<"$listing" || fail "$archive has invalid ownership, timestamps, or entry names"
    diff -u <(tree_of "$stage") <(awk '{ sub(/\/$/, "", $6); print $1, $6 }' <<<"$listing" | sort -k 2) ||
        fail "$archive does not match its staged tree and modes"
    diff -u <(hashes_of "$stage") <(tar --extract --gzip --file="$archive" \
        --to-command='printf "%s  %s\n" "$(sha256sum | cut -d " " -f 1)" "$TAR_FILENAME"' | sort -k 2) ||
        fail "$archive content differs from its staged files"
}

make_zip() {
    local stage=$1 archive=$2 name
    (cd -- "$stage" && find . -mindepth 1 -printf '%P\n' | sort | zip -X -q -6 "$archive" -@)
    unzip -tqq "$archive" || fail "$archive failed zip integrity test"
    diff -u <(cd -- "$stage" && find . -mindepth 1 \( -type d -printf '%P/\n' -o -printf '%P\n' \) | sort) \
        <(unzip -Z1 "$archive" | sort) || fail "$archive does not match its staged tree"
    diff -u <(hashes_of "$stage") <(
        unzip -Z1 "$archive" | grep -v '/$' | while IFS= read -r name; do
            printf '%s  %s\n' "$(unzip -p "$archive" "$name" | sha256sum | cut -d ' ' -f 1)" "$name"
        done | sort -k 2
    ) || fail "$archive content differs from its staged files"
}

make_source() {
    local entries
    git -C "$source_root" archive --format=tar --prefix=flutzPlayer/ "$revision" |
        "$gz" -n -6 > "$output_dir/$source_tar_archive"
    git -C "$source_root" archive --format=zip --prefix=flutzPlayer/ -o "$output_dir/$source_zip_archive" "$revision"
    entries=$(git -C "$source_root" ls-tree -r --name-only "$revision" | sed 's|^|flutzPlayer/|' | sort)
    diff -u <(printf '%s\n' "$entries") <(tar --list --gzip --file="$output_dir/$source_tar_archive" | grep -v '/$' | sort) ||
        fail 'source tarball does not match the tracked files'
    diff -u <(printf '%s\n' "$entries") <(unzip -Z1 "$output_dir/$source_zip_archive" | grep -v '/$' | sort) ||
        fail 'source zip does not match the tracked files'
    ! grep -Eiq '\.(dat|sf2|exe|appimage)$' <<<"$entries" || fail 'source archives contain binary release payloads'
}

stage_windows "$work/windows-nodat" 0
stage_windows "$work/windows" 1
stage_linux "$work/appimage-nodat" appimage 0
stage_linux "$work/appimage" appimage 1
stage_linux "$work/linux-nodat" linux 0
stage_linux "$work/linux" linux 1

# The four DAT-bearing archives dominate; build all eight concurrently.
pids=()
make_zip "$work/windows-nodat" "$output_dir/$windows_nodat_archive" & pids+=($!)
make_zip "$work/windows" "$output_dir/$windows_archive" & pids+=($!)
make_tar "$work/appimage-nodat" "$output_dir/$appimage_nodat_archive" & pids+=($!)
make_tar "$work/appimage" "$output_dir/$appimage_archive" & pids+=($!)
make_tar "$work/linux-nodat" "$output_dir/$linux_nodat_archive" & pids+=($!)
make_tar "$work/linux" "$output_dir/$linux_archive" & pids+=($!)
make_source & pids+=($!)
failed=0
for pid in "${pids[@]}"; do
    wait "$pid" || failed=1
done
((failed == 0)) || fail 'one or more archives failed to build or verify'

diff -u <(printf '%s\n' "${expected_archives[@]}" | sort) <(file_list "$output_dir") ||
    fail 'release output set is incomplete or contains unexpected files'
for name in "${expected_archives[@]}"; do
    [[ -s $output_dir/$name ]] || fail "empty release archive: $name"
done
(cd -- "$output_dir" && sha256sum -- "${expected_archives[@]}") > "$checksum_file"
cat -- "$checksum_file"
printf 'Release assembly complete: %s archive(s) in %s\n' "${#expected_archives[@]}" "$output_dir"
