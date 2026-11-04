#!/usr/bin/env bash
set -euo pipefail

fail() {
    printf 'AppImage packaging error: %s\n' "$*" >&2
    exit 1
}

usage() {
    echo 'Usage: build-appimage.sh record BINARY SNAPSHOT RECORD ALLOCATOR' >&2
    echo '       build-appimage.sh build BINARY SNAPSHOT RECORD OUTPUT_DIRECTORY ALLOCATOR' >&2
    echo '       build-appimage.sh ci BINARY OUTPUT_DIRECTORY ALLOCATOR SOURCE_REVISION' >&2
    exit 2
}

[[ $# -ge 1 ]] || usage
mode=$1
shift
case "$mode" in
    record) [[ $# -eq 4 ]] || usage ;;
    build) [[ $# -eq 5 ]] || usage ;;
    ci) [[ $# -eq 4 ]] || usage ;;
    *) usage ;;
esac
binary=$1
snapshot=
record=
source_revision=
if [[ $mode == ci ]]; then
    # GitHub Actions: an ubuntu:26.04 container builds a git checkout, so the
    # commit replaces the VM's synchronized-snapshot provenance.
    output_directory=$2
    allocator=$3
    source_revision=$4
    [[ $source_revision =~ ^[0-9a-f]{40}$ ]] || fail "invalid source revision: $source_revision"
else
    snapshot=$2
    record=$3
    if [[ $mode == build ]]; then
        output_directory=$4
        allocator=$5
    else
        allocator=$4
    fi
fi
[[ $allocator == jemalloc || $allocator == system ]] || fail "invalid allocator: $allocator"
if [[ $mode == ci ]]; then
    [[ $binary == /* ]] || fail 'binary path must be absolute'
else
    [[ $binary == /* && $snapshot == /* && $record == /* ]] ||
        fail 'binary, snapshot, and record paths must be absolute'
fi
[[ -f $binary && -x $binary ]] || fail "missing or non-executable release binary: $binary"
[[ $mode == ci || -f $snapshot ]] || fail "missing source snapshot: $snapshot; synchronize the source first"

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
source_root=$(cd -- "$script_dir/../../.." && pwd -P)
versions="$source_root/build/linux/tool-versions.json"
desktop="$source_root/build/linux/appimage/flutzplayer.desktop"
icon="$source_root/build/linux/appimage/flutzplayer.png"
apprun="$source_root/build/linux/appimage/AppRun"
library_policy="$source_root/build/linux/appimage/library-exclusions.txt"
export PATH="$HOME/.cargo/bin:$PATH"
unset LD_PRELOAD LD_LIBRARY_PATH

. /etc/os-release
if [[ $mode == ci ]]; then
    # Container images have no installer media record; the release matches
    # the VM's Ubuntu 26.04 baseline.
    [[ $ID == ubuntu && $VERSION_ID == 26.04 && $(uname -m) == x86_64 ]] ||
        fail 'Ubuntu 26.04 x86_64 is required'
    media=container
    snapshot_sha=
else
    [[ $ID == ubuntu && $VERSION_ID == 26.04 && $(uname -m) == x86_64 ]] &&
        grep -Fq '26.04.1' /var/log/installer/media-info ||
        fail 'Ubuntu 26.04.1 x86_64 is required'
    media=26.04.1
    snapshot_root=$(jq -er '.sourceRoot' "$snapshot")
    [[ $(realpath -e -- "$snapshot_root") == "$source_root" ]] ||
        fail 'source snapshot belongs to a different source tree'
    snapshot_sha=$(jq -er '
        select(.schemaId == "https://flutzplayer.local/schemas/source-snapshot/v1") |
        .aggregateSha256
    ' "$snapshot")
    [[ $snapshot_sha =~ ^[0-9a-f]{64}$ ]] || fail 'invalid source snapshot hash'
fi
binary_sha=$(sha256sum < "$binary" | cut -d ' ' -f 1)
lock_sha=$(sha256sum < "$source_root/Cargo.lock" | cut -d ' ' -f 1)
rustc_version=$(rustc --version)
cargo_version=$(cargo --version)

if [[ $mode == record ]]; then
    mkdir -p -- "$(dirname -- "$record")"
    jq -n \
        --arg snapshot "$snapshot_sha" \
        --arg binary "$binary_sha" \
        --arg lock "$lock_sha" \
        --arg rustc "$rustc_version" \
        --arg allocator "$allocator" \
        '{
            schemaId: "https://flutzplayer.local/schemas/appimage-build-record/v1",
            sourceSnapshotSha256: $snapshot,
            binarySha256: $binary,
            cargoLockSha256: $lock,
            rustc: $rustc,
            allocator: $allocator
        }' > "$record"
    echo "Recorded release binary provenance: $record"
    exit 0
fi

[[ $output_directory == /* && $output_directory != / ]] || fail 'output directory must be absolute'
if [[ $mode == build ]]; then
    [[ -f $record ]] || fail 'release build provenance is missing; build without -SkipBuild first'
    jq -e \
        --arg snapshot "$snapshot_sha" \
        --arg binary "$binary_sha" \
        --arg lock "$lock_sha" \
        --arg rustc "$rustc_version" \
        --arg allocator "$allocator" \
        '.schemaId == "https://flutzplayer.local/schemas/appimage-build-record/v1" and
         .sourceSnapshotSha256 == $snapshot and .binarySha256 == $binary and
         .cargoLockSha256 == $lock and .rustc == $rustc and .allocator == $allocator' \
        "$record" >/dev/null ||
        fail 'release binary provenance does not match the synchronized source, toolchain, or allocator; build without -SkipBuild'
fi

for path in "$versions" "$desktop" "$icon" "$apprun" "$library_policy"; do
    [[ -f $path && -r $path ]] || fail "missing packaging resource: $path"
done
desktop-file-validate "$desktop"
[[ $(file -b -- "$icon") == 'PNG image data, 256 x 256,'* ]] ||
    fail 'the AppImage icon must be a 256x256 PNG'
if dpkg-query -W -f='${db:Status-Status}\n' libfuse2 2>/dev/null | grep -Fxq installed ||
   dpkg-query -W -f='${db:Status-Status}\n' libfuse2t64 2>/dev/null | grep -Fxq installed; then
    fail 'libfuse2 is installed; the build must use FUSE3 and extraction fallback'
fi

rm -rf -- "$output_directory"
mkdir -p -- "$output_directory"
work="$output_directory/work"
mkdir -- "$work"
trap 'rm -rf -- "$work"' EXIT
export TMPDIR="$work/tmp"
mkdir -- "$TMPDIR"
if [[ $mode == build ]]; then
    bash "$source_root/build/linux/sync/write-source-snapshot.sh" \
        "$source_root" "$work/current-snapshot.json" > "$work/source-snapshot.log"
    current_snapshot_sha=$(jq -er '.aggregateSha256' "$work/current-snapshot.json")
    [[ $current_snapshot_sha == "$snapshot_sha" ]] ||
        fail 'guest source changed after synchronization; synchronize and rebuild'
fi

for tool in linuxdeploy appimagetool; do
    asset=$(jq -er ".$tool.asset" "$versions")
    expected=$(jq -er ".$tool.sha256" "$versions")
    [[ $asset =~ ^[A-Za-z0-9._+-]+$ && $expected =~ ^[0-9a-f]{64}$ ]] ||
        fail "invalid $tool pin"
    path="/opt/flutzplayer-tools/$asset"
    [[ -f $path && -x $path ]] || fail "missing pinned $tool: $path; run build-init.ps1 -OS Linux"
    actual=$(sha256sum < "$path" | cut -d ' ' -f 1)
    [[ $actual == "$expected" ]] || fail "$tool checksum mismatch; run build-init.ps1 -OS Linux"
    if [[ $tool == linuxdeploy ]]; then
        linuxdeploy=$path
    else
        appimagetool=$path
    fi
done

runtime_asset=$(jq -er '.appimageRuntime.sourceAsset' "$versions")
runtime_release=$(jq -er '.appimageRuntime.sourceRelease' "$versions")
runtime_commit=$(jq -er '.appimageRuntime.commit' "$versions")
runtime_size=$(jq -er '.appimageRuntime.size' "$versions")
digest_offset=$(jq -er '.appimageRuntime.digestOffset' "$versions")
digest_size=$(jq -er '.appimageRuntime.digestSize' "$versions")
runtime_sha=$(jq -er '.appimageRuntime.sha256' "$versions")
[[ $runtime_asset == "$(jq -er '.appimagetool.asset' "$versions")" &&
   $runtime_release == "$(jq -er '.appimagetool.release' "$versions")" &&
   $runtime_commit =~ ^[0-9a-f]{40}$ && $runtime_size =~ ^[0-9]+$ &&
   $digest_offset =~ ^[0-9]+$ && $digest_size =~ ^[0-9]+$ &&
   $runtime_sha =~ ^[0-9a-f]{64}$ ]] || fail 'invalid embedded Type 2 runtime pin'
(( digest_offset > 0 && digest_size == 16 && digest_offset + digest_size < runtime_size )) ||
    fail 'invalid embedded runtime digest section bounds'
[[ $("$appimagetool" --appimage-offset) == "$runtime_size" ]] ||
    fail 'appimagetool runtime size does not match its pin'
runtime="$work/runtime-x86_64"
head -c "$runtime_size" -- "$appimagetool" > "$runtime"
chmod 0755 "$runtime"
[[ $(sha256sum < "$runtime" | cut -d ' ' -f 1) == "$runtime_sha" ]] ||
    fail 'embedded Type 2 runtime checksum mismatch'
[[ $(od -An -tx1 -j8 -N3 "$runtime" | tr -d ' \n') == 414902 ]] ||
    fail 'embedded runtime does not have the Type 2 header'
runtime_description=$(file -b -- "$runtime")
[[ $runtime_description == *'static-pie linked'* || $runtime_description == *'statically linked'* ]] ||
    fail 'embedded Type 2 runtime is not static'
runtime_version=$("$runtime" --appimage-version 2>&1)
[[ $runtime_version == *"${runtime_commit:0:7}"* ]] ||
    fail 'embedded Type 2 runtime commit does not match its pin'
digest_section=$(objdump -h "$runtime" | awk '$2 == ".digest_md5" { print $6, $3 }')
[[ $digest_section =~ ^([0-9a-fA-F]+)\ ([0-9a-fA-F]+)$ ]] ||
    fail 'embedded runtime has no unique MD5 digest section'
(( 16#${BASH_REMATCH[1]} == digest_offset && 16#${BASH_REMATCH[2]} == digest_size )) ||
    fail 'embedded runtime digest section does not match its pin'

binary_ldd=$(ldd "$binary" 2>&1) || fail "cannot inspect release binary dependencies: $binary_ldd"
[[ $binary_ldd != *'not found'* && $binary_ldd != *'libSDL3.so'* &&
   $binary_ldd != *'libxkbcommon.so'* ]] ||
    fail "release binary has missing dependencies or dynamic SDL3/xkbcommon: $binary_ldd"
nm --defined-only --format=posix "$binary" | awk '
    $1 == "SDL_Init" && $2 ~ /^[Tt]$/ { found = 1 }
    END { exit !found }
' || fail 'release binary does not define the source-static SDL_Init symbol'

exclusions=()
patterns=()
while IFS= read -r pattern || [[ -n $pattern ]]; do
    [[ -n $pattern && $pattern != \#* ]] || continue
    [[ $pattern =~ ^[A-Za-z0-9._*?+-]+$ ]] || fail "invalid library exclusion: $pattern"
    patterns+=("$pattern")
    exclusions+=("--exclude-library=$pattern")
done < "$library_policy"
((${#patterns[@]} > 0)) || fail 'library exclusion policy is empty'

appdir="$work/AppDir"
APPIMAGE_EXTRACT_AND_RUN=1 "$linuxdeploy" \
    --appdir="$appdir" \
    --executable="$binary" \
    --desktop-file="$desktop" \
    --icon-file="$icon" \
    --custom-apprun="$apprun" \
    "${exclusions[@]}"

image="$work/flutzPlayer-x86_64.AppImage"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$appimagetool" \
    --no-appstream --runtime-file "$runtime" "$appdir" "$image"
chmod 0755 "$image"
[[ $(od -An -tx1 -j8 -N3 "$image" | tr -d ' \n') == 414902 ]] ||
    fail 'generated AppImage does not report a Type 2 header'
[[ $("$image" --appimage-offset) == "$runtime_size" ]] ||
    fail 'generated AppImage has an unexpected runtime size'
cmp -s -n "$digest_offset" -- "$runtime" "$image" &&
    cmp -s -i "$((digest_offset + digest_size))" \
        -n "$((runtime_size - digest_offset - digest_size))" -- "$runtime" "$image" ||
    fail 'generated AppImage differs from the pinned runtime outside its MD5 digest section'

mkdir -- "$work/extracted"
(cd -- "$work/extracted" && "$image" --appimage-extract > "$work/extraction.log")
extracted="$work/extracted/squashfs-root"
[[ -x $extracted/AppRun && -x $extracted/usr/bin/flutzplayer ]] ||
    fail 'extracted AppDir is missing AppRun or the executable'
cmp -s -- "$apprun" "$extracted/AppRun" || fail 'extracted AppRun differs from the source'
cmp -s -- "$desktop" "$extracted/flutzplayer.desktop" ||
    fail 'extracted desktop entry differs from the source'
cmp -s -- "$icon" "$extracted/usr/share/icons/hicolor/256x256/apps/flutzplayer.png" ||
    fail 'extracted icon differs from the source'
desktop-file-validate "$extracted/flutzplayer.desktop"
unexpected=$(find "$extracted" \( -iname '*.dat' -o -iname 'preferences.ini' \) -print -quit)
[[ -z $unexpected ]] || fail "external data or preferences were included: $unexpected"
for pattern in "${patterns[@]}"; do
    unexpected=$(find "$extracted" \( -type f -o -type l \) -name "$pattern" -print -quit)
    [[ -z $unexpected ]] || fail "host or forbidden library was bundled: $unexpected"
done
while IFS= read -r -d '' candidate; do
    description=$(file -b -- "$candidate")
    [[ $description == ELF* ]] || continue
    dependencies=$(LD_LIBRARY_PATH="$extracted/usr/lib" ldd "$candidate" 2>&1) ||
        fail "cannot inspect extracted ELF dependencies for $candidate: $dependencies"
    [[ $dependencies != *'not found'* ]] ||
        fail "extracted ELF has missing dependencies for $candidate: $dependencies"
done < <(find "$extracted/usr" -type f -print0)

"$image" --appimage-extract-and-run --help > "$work/extract-and-run-help.txt"
grep -Fq 'Data files default to a data folder beside the executable or AppImage' \
    "$work/extract-and-run-help.txt" || fail 'extraction fallback did not launch flutzPlayer'

image_sha=$(sha256sum < "$image" | cut -d ' ' -f 1)
metadata="$work/metadata.json"
jq -n \
    --arg createdAtUtc "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg osId "$ID" \
    --arg osVersion "$VERSION_ID" \
    --arg osName "$PRETTY_NAME" \
    --arg media "$media" \
    --arg arch "$(uname -m)" \
    --arg kernel "$(uname -r)" \
    --arg rustc "$rustc_version" \
    --arg cargo "$cargo_version" \
    --arg lock "$lock_sha" \
    --arg snapshot "$snapshot_sha" \
    --arg revision "$source_revision" \
    --arg binary "$binary_sha" \
    --arg allocator "$allocator" \
    --arg image "$image_sha" \
    --argjson linuxdeploy "$(jq -c '.linuxdeploy' "$versions")" \
    --argjson appimagetool "$(jq -c '.appimagetool' "$versions")" \
    --argjson runtime "$(jq -c '.appimageRuntime' "$versions")" \
    '{
        schemaId: "https://flutzplayer.local/schemas/appimage-build-metadata/v1",
        createdAtUtc: $createdAtUtc,
        guest: {id: $osId, version: $osVersion, name: $osName, media: $media, architecture: $arch, kernel: $kernel},
        tools: {linuxdeploy: $linuxdeploy, appimagetool: $appimagetool, runtime: $runtime},
        build: ({rustc: $rustc, cargo: $cargo, cargoLockSha256: $lock,
                sourceSnapshotSha256: (if $snapshot == "" then null else $snapshot end),
                binarySha256: $binary, allocator: $allocator} +
                (if $revision == "" then {} else {sourceRevision: $revision} end)),
        appimage: {name: "flutzPlayer-x86_64.AppImage", sha256: $image}
    }' > "$metadata"
metadata_sha=$(sha256sum < "$metadata" | cut -d ' ' -f 1)
mv -- "$image" "$output_directory/flutzPlayer-x86_64.AppImage"
mv -- "$metadata" "$output_directory/metadata.json"
printf 'APPIMAGE_RESULT %s %s\n' "$image_sha" "$metadata_sha"
