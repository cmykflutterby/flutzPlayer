#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || -z $1 ]]; then
    echo 'Usage: build-dat.sh OUTPUT_DIRECTORY' >&2
    exit 2
fi

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
source_root=$(cd -- "$script_dir/../../.." && pwd -P)
output_dir=$1
manifest="$source_root/assets/dat-manifest.toml"
[[ -f $manifest && -r $manifest ]] || {
    printf 'Missing or unreadable DAT manifest: %s\n' "$manifest" >&2
    exit 1
}

cd -- "$source_root"
cargo build --release -p flutz_soundfont_tools
target_dir=$(cargo metadata --no-deps --format-version 1 | jq -er .target_directory)
packer="$target_dir/release/flutz_soundfont_tools"
[[ -f $packer && -x $packer ]] || {
    printf 'Missing or non-executable release DAT packer: %s\n' "$packer" >&2
    exit 1
}

mkdir -p -- "$output_dir"
tool_dir=$(mktemp -d -- "$output_dir/.dat-packer-tool-XXXXXX")
cleanup() {
    rm -f -- "$tool_dir/dat-packer" "$tool_dir/pack-dat.sh"
    rmdir -- "$tool_dir"
}
trap cleanup EXIT
ln -s -- "$packer" "$tool_dir/dat-packer"
cp -- "$script_dir/pack-dat.sh" "$tool_dir/pack-dat.sh"
bash "$tool_dir/pack-dat.sh" \
    --manifest "$manifest" \
    --base-dir "$source_root" \
    --output "$output_dir/assets.dat" \
    --number-single
[[ -s $output_dir/assets-000.dat ]] || {
    printf 'Linux DAT build produced no numbered DAT family in %s\n' "$output_dir" >&2
    exit 1
}
printf 'Linux DAT build and inspection complete: %s\n' "$output_dir"
