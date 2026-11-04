#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: write-source-snapshot.sh SOURCE_ROOT MANIFEST_PATH" >&2
    exit 2
fi

source_root=$1
manifest_path=$2
manifest_directory=$(dirname "$manifest_path")
entries_file=$(mktemp)
json_entries=$(mktemp)
cleanup() {
    rm -f -- "$entries_file" "$json_entries"
}
trap cleanup EXIT

cd "$source_root"
{
    [[ ! -f Cargo.toml ]] || printf '%s\0' Cargo.toml
    [[ ! -f Cargo.lock ]] || printf '%s\0' Cargo.lock
    find crates assets tools build -type f -print0
    find . -maxdepth 1 -type f \( -name '*.ps1' -o -name '*.toml' -o -name '*.json' \) -print0
} | sort -zu | while IFS= read -r -d '' path; do
    path=${path#./}
    hash=$(sha256sum -- "$path" | awk '{print $1}')
    printf '%s\t%s\n' "$hash" "$path"
done > "$entries_file"

[[ -s "$entries_file" ]] || {
    echo "No source inputs were found for the snapshot manifest." >&2
    exit 1
}

jq -Rn '[inputs | capture("^(?<sha256>[0-9a-f]{64})\\t(?<path>.*)$")]' \
    < "$entries_file" > "$json_entries"
aggregate_sha256=$(sha256sum "$entries_file" | awk '{print $1}')
entry_count=$(wc -l < "$entries_file" | tr -d ' ')

mkdir -p "$manifest_directory"
temporary_manifest=$(mktemp "$manifest_directory/source-snapshot.XXXXXX")
jq -n \
    --arg schemaId "https://flutzplayer.local/schemas/source-snapshot/v1" \
    --arg sourceRoot "$source_root" \
    --arg createdAtUtc "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg aggregateSha256 "$aggregate_sha256" \
    --argjson entryCount "$entry_count" \
    --slurpfile entries "$json_entries" \
    '{
        schemaId: $schemaId,
        sourceRoot: $sourceRoot,
        createdAtUtc: $createdAtUtc,
        aggregateSha256: $aggregateSha256,
        entryCount: $entryCount,
        entries: $entries[0]
    }' > "$temporary_manifest"
chmod 0644 "$temporary_manifest"
mv -f -- "$temporary_manifest" "$manifest_path"

jq -e \
    --arg aggregate "$aggregate_sha256" \
    --argjson count "$entry_count" \
    '.schemaId == "https://flutzplayer.local/schemas/source-snapshot/v1" and
     .aggregateSha256 == $aggregate and
     .entryCount == $count and
     (.entries | length) == $count' \
    "$manifest_path" >/dev/null
echo "SOURCE_SNAPSHOT_OK entries=$entry_count sha256=$aggregate_sha256 manifest=$manifest_path"
