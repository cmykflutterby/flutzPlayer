# File Format Design

This document captures the fixed FMID project format and DAT asset archives.

## FMID Project Container

FMID is a fixed-format chunked binary project container. It stores the complete source MIDI bytes byte-for-byte plus binary project records for soundfont selections, mixer settings, loop settings, smart-mix settings, and project metadata.

FMID uses ASCII magic `FMID`, little-endian integer fields, and `u64` for offsets, sizes, counts, flags, MIDI ticks, MIDI channel/program values, loop counts, and record lengths. The format has no version field and should be treated as fixed once implemented.

The file layout is:

```text
Header
Chunk table
Chunk payload area
```

The header lives at file start and contains magic, header size, flags, file size, chunk table offset, chunk table length, and chunk count. The chunk table is required and uses ASCII magic `FIDX`. Chunk IDs are 4-byte ASCII IDs. Writer output should prefer canonical chunk order, but readers must locate chunks through the chunk table and accept any physical order.

Required chunks are `MIDI`, `PROJ`, `FONT`, `MIXR`, `LOOP`, and `SMIX`. All required chunks are present in every FMID file, even when a chunk only represents default or no-custom settings. `NOTE` and `MSRC` are known optional chunks. `NOTE` stores user notes or metadata. `MSRC` stores the mixer source mode used to distinguish explicit custom mixer state from preset-default mixer state.

Readers fail on missing required chunks, duplicate known singleton chunks, and unknown enum-like values inside known chunks. Unknown chunks are ignored for loading but preserved as raw extents for future save operations. Known chunks use explicit record lengths so readers can skip unknown trailing fields or records when safe.

Primitive encoding rules:

- Strings are length-prefixed UTF-8.
- Persisted continuous control values are `f64`.
- Booleans are `u8`, with `0` false and `1` true.
- MIDI channels and programs are stored zero-based using MIDI-native values.
- Compression and integrity flags may be reserved, but currently emits no compressed chunks and no CRC/hash fields.

Required FMID records:

```text
FMID header
   magic:                4 bytes ASCII "FMID"
   header_size:          u64
   flags:                u64
   file_size:            u64
   chunk_table_offset:   u64
   chunk_table_length:   u64
   chunk_count:          u64

Chunk table
   magic:                4 bytes ASCII "FIDX"
   table_length:         u64
   flags:                u64
   chunk_count:          u64
   chunks:               FmidChunkRecord[chunk_count]

FmidChunkRecord
   chunk_id:             4 bytes ASCII
   offset:               u64
   length:               u64
   flags:                u64
   ordinal:              u64

Utf8String
   byte_length:          u64
   bytes:                [u8; byte_length]
```

`PROJ` stores project name, source MIDI filename, project flags, and notes. `FONT` stores explicitly ordered soundfont slots by internal soundfont ID. `MIXR` stores master mixer controls, soundfont-row mute states, and per-strip controls. `LOOP` stores loop enabled state, loop mode, start tick, end tick, and count. `SMIX` stores smart-mix and auto-normalization settings. `MSRC` stores whether `MIXR` should be applied as custom explicit state or ignored in favor of recomputed preset defaults.

Per-strip mixer identity is soundfont ID, MIDI channel, MIDI program, and percussion flag. Each strip persists volume, mute, pan, gain, limiter enabled, limiter amount, limiter release, reverb, and chorus. `MIXR` also persists per-soundfont-row mute state so row mute behavior restores exactly. Master mixer state persists master volume, master limiter enabled, master limiter amount, master limiter release, master reverb, master chorus, and master EQ low/mid/high.

Loop mode values represent `none`, `inf`, and `n`. The numeric enum values must be fixed in the `flutz_fmid` implementation before writing production FMID files.

Known FMID chunk IDs:

| ID | Required | Purpose |
| --- | --- | --- |
| `MIDI` | yes | Embedded source MIDI bytes. |
| `PROJ` | yes | Project metadata: project name, source MIDI filename, flags, and notes. |
| `FONT` | yes | Explicit ordered soundfont slot IDs. |
| `MIXR` | yes | Master mixer controls, soundfont row mutes, and per-strip controls. |
| `LOOP` | yes | Loop enabled state, mode, start/end ticks, and count. |
| `SMIX` | yes | Smart-mix and auto-normalization controls. |
| `NOTE` | no | Optional project note string. |
| `MSRC` | no | Optional mixer source mode for preset-default vs custom loading. |

### FMID `MSRC` Mixer Source Chunk

`MSRC` is optional. When absent, readers must treat the FMID as `custom` for backward compatibility with older files.

Payload layout:

```text
MSRC payload
   mode:                 u64
   preset_id:            Utf8String, present only when mode == 1
```

Mode values:

| Value | Mode | Meaning |
| --- | --- | --- |
| `0` | custom | Load exact `FONT` order and restore explicit `MIXR` row and strip controls. |
| `1` | preset_default | Resolve `preset_id` from compiled-in presets, load that preset font stack, and recompute routing-derived mute/volume defaults during strip initialization. |

For `preset_default`, the `FONT` and `MIXR` chunks remain present as fallback/debug context, but preset defaults are authoritative. If the referenced `preset_id` is unavailable, the app falls back to the compiled default preset and surfaces a user-facing warning. Writers should emit `MSRC` for newly saved files. Readers that do not understand `MSRC` should preserve it as an unknown chunk if they rewrite the file.

## DAT Asset Archive

DAT is a fixed chunk-addressed binary archive for bundled assets. Currently only consumes soundfonts, but DAT supports generic assets from day one.

DAT uses ASCII magic `FDAT`, little-endian integer fields, and `u64` for offsets, sizes, counts, IDs, and flags. The format has no version field and should be treated as fixed once implemented.

The archive layout is:

```text
Header
Primary index
Blob chunk area
Backup index
EOF footer
```

The primary index appears immediately after the header. The backup index appears near EOF after the blob chunk area. The EOF footer is the final data in the file and points to the backup index so the archive can recover if the header or primary index is damaged.

The default packer chunk size is 256 MiB, and the actual chunk size is stored in the header. Runtime lookup uses a chunk table plus per-entry chunk extents, not absolute entry offsets. The default packer file-size cap is also 256 MiB; when manifest asset bytes do not fit under that cap, the packer emits numbered DAT files such as `assets-000.dat`, `assets-001.dat`, and so on.

DAT stores runtime asset bytes selected by the packer. V1 does not compress assets and does not write CRC/hash fields; soundfont `.sfArk` sources are converted to runtime `.sf2` bytes before storage. DAT metadata records both the original source format and the actual stored byte format.

Required entry metadata:

- Internal ID.
- Display name.
- Asset type.
- Source format.
- Storage format.
- Original filename.
- Total size.
- Chunk extents.
- Flags.
- Runtime format metadata for soundfont entries.

The DAT packer uses a manifest plus the default registry. Only manifest-listed files are packed; missing manifest files fail the pack; extra folder files are ignored; duplicate internal IDs fail the pack. When a registry is supplied, each soundfont asset must have a matching `[[soundfonts]]` registry entry; the registry provides soundfont display/runtime metadata, and `default_soundfont_id` marks the bundled soundfont that should be auto-loaded for plain MIDI files.

Accepted v1 soundfont source extensions are `.sf2` and `.sfArk`. Recognized source extension does not imply runtime playability. Normal DAT packing must reject soundfont assets that rustysynth cannot directly load and render, unless an explicit override archives them as unsupported raw assets.

DAT records:

```text
DAT header
   magic:                4 bytes ASCII "FDAT"
   header_size:          u64
   flags:                u64
   chunk_size:           u64
   primary_index_offset: u64
   primary_index_length: u64
   entry_count:          u64
   chunk_count:          u64

Index block
   magic:                4 bytes ASCII "DIDX"
   index_length:         u64
   flags:                u64
   entry_count:          u64
   chunk_count:          u64
   chunk_records:        ChunkRecord[chunk_count]
   entry_records:        EntryRecord[entry_count]

ChunkRecord
   chunk_id:             u64
   file_offset:          u64
   stored_length:        u64

EntryRecord
   internal_id:          Utf8String
   display_name:         Utf8String
   asset_type:           Utf8String
   source_format:        Utf8String
   storage_format:       Utf8String
   runtime_format:       Utf8String
   original_filename:    Utf8String
   total_size:           u64
   flags:                u64
   extent_count:         u64
   extents:              ChunkExtent[extent_count]

EntryRecord flags
   bit 0:                default soundfont entry

ChunkExtent
   chunk_id:             u64
   offset_in_chunk:      u64
   length:               u64

EOF footer
   magic:                4 bytes ASCII "DEND"
   backup_index_offset:  u64
   backup_index_length:  u64
   footer_size:          u64
```

Packing should keep each asset contiguous when it fits within one configured chunk and one configured DAT file. Assets larger than the configured chunk size are split across multiple chunk extents; assets larger than the configured max DAT file size are split across multiple numbered DAT files as repeated partial entries with the same internal ID. If a default soundfont is split across files, each partial entry carries the default soundfont flag. Readers should try the primary index first and fall back to the backup index through the EOF footer if needed.
