#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
packer="$script_dir/dat-packer"
manifest="$script_dir/dat-manifest.toml"
output="$script_dir/../data/assets.dat"
base_dir="$script_dir"
chunk_size=0
max_file_size=0
number_single=0

usage() {
    cat <<'EOF'
Usage: pack-dat.sh [--manifest PATH] [--output PATH] [--base-dir DIR]
                   [--chunk-size BYTES] [--max-file-size BYTES] [--number-single]

Manifest source paths are resolved relative to --base-dir. By default, the
manifest and base directory are beside this script, and output is ../data/assets.dat.
--number-single also names a one-file result assets-000.dat (using the output stem).
EOF
}

fail() {
    printf 'DAT packing error: %s\n' "$*" >&2
    exit 1
}

while (($#)); do
    case "$1" in
        --help|-h)
            usage
            exit 0
            ;;
        --manifest|--output|--base-dir|--chunk-size|--max-file-size)
            (($# >= 2)) || fail "missing value for $1"
            option=$1
            value=$2
            [[ -n $value ]] || fail "empty value for $option"
            case "$option" in
                --manifest) manifest=$value ;;
                --output) output=$value ;;
                --base-dir) base_dir=$value ;;
                --chunk-size) chunk_size=$value ;;
                --max-file-size) max_file_size=$value ;;
            esac
            shift 2
            ;;
        --number-single)
            number_single=1
            shift
            ;;
        *)
            fail "unknown argument: $1"
            ;;
    esac
done

[[ -f $packer && -x $packer ]] || fail "missing or non-executable DAT packer: $packer"
[[ -f $manifest && -r $manifest ]] || fail "missing or unreadable DAT manifest: $manifest"
[[ -d $base_dir ]] || fail "missing base directory: $base_dir"
[[ $chunk_size =~ ^[0-9]+$ ]] || fail "invalid --chunk-size: $chunk_size"
[[ $max_file_size =~ ^[0-9]+$ ]] || fail "invalid --max-file-size: $max_file_size"

output_name=$(basename -- "$output")
output_dir=$(dirname -- "$output")
[[ $output_name == *.[dD][aA][tT] ]] || fail "output must have a .dat extension: $output"
stem=${output_name%.*}
extension=${output_name##*.}
[[ -n $stem && ! $stem =~ -[0-9]{3}$ ]] ||
    fail "output must name a DAT family, not a numbered part: $output"
mkdir -p -- "$output_dir"
output_dir=$(cd -- "$output_dir" && pwd -P)
output_path="$output_dir/$output_name"

shopt -s nullglob
export LC_ALL=C
rm -f -- "$output_path"
for path in "$output_dir/$stem-"*".$extension"; do
    part=${path##*/}
    part=${part#"$stem-"}
    part=${part%".$extension"}
    [[ $part =~ ^[0-9]+$ ]] || continue
    rm -f -- "$path"
done

pack_args=(--pack --input "$manifest" --output "$output_path" --base-dir "$base_dir")
[[ $chunk_size =~ ^0+$ ]] || pack_args+=(--chunk-size "$chunk_size")
[[ $max_file_size =~ ^0+$ ]] || pack_args+=(--max-file-size "$max_file_size")
"$packer" "${pack_args[@]}"

generated=()
for path in "$output_dir/$stem-"*".$extension"; do
    part=${path##*/}
    part=${part#"$stem-"}
    part=${part%".$extension"}
    [[ $part =~ ^[0-9]+$ ]] || continue
    [[ $part =~ ^[0-9]{3}$ ]] || fail "unsupported generated DAT part: $path"
    printf -v expected '%03d' "${#generated[@]}"
    [[ $part == "$expected" ]] || fail "DAT parts are not contiguous from 000: $path"
    generated+=("$path")
done
if [[ -e $output_path || -L $output_path ]]; then
    [[ -f $output_path && ! -L $output_path && ${#generated[@]} -eq 0 ]] ||
        fail "ambiguous generated DAT family: $output_path"
    if (( number_single )); then
        numbered="$output_dir/$stem-000.$extension"
        mv -- "$output_path" "$numbered"
        generated+=("$numbered")
    else
        generated+=("$output_path")
    fi
fi
((${#generated[@]} > 0)) || fail "packer produced no DAT files"
for path in "${generated[@]}"; do
    [[ -s $path ]] || fail "packer produced an empty DAT file: $path"
done
"$packer" --inspect --input "${generated[0]}"
printf 'Validated DAT family written from %s to %s (%s file(s))\n' \
    "$manifest" "$output_path" "${#generated[@]}"
