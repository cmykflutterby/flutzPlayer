# rustystem

`rustystem` is flutzPlayer's local RustySynth-derived SoundFont renderer crate.

It started as a RustySynth 1.3.6 fork, but it now includes substantial behavior and API additions for flutzPlayer's stem-based runtime: grouped stem rendering, MIDI SysEx interpretation for channel-role changes, coverage extraction for SF2 assets, allocation/memory diagnostics, and sequencer seek/introspection features.

## Status at a Glance

- Production-focused direct rendering path remains the audio-quality reference.
- Stem rendering is implemented and usable now, including grouped identities.
- Sequenced stem rendering supports residual reconciliation to preserve parity with direct output.
- Runtime includes diagnostics for memory/allocation behavior during stem rendering.

## Upstream and License

- Derived from RustySynth 1.3.6 (original author: Nobuaki Tanaka).
- This crate is distributed under the MIT license, matching upstream metadata.

## What Is New vs. RustySynth 1.3.6

The following capabilities are present in `rustystem` and are not part of the original RustySynth 1.3.6 public surface:

### 1. Stem Rendering API and Runtime

- `StemRenderMode` supports:
  - `WholeSoundFont`
  - `MidiChannel`
  - `MidiProgram`
  - `Percussion`
  - `ChannelProgram`
- `StemRenderRequest` can request one or multiple stem grouping modes for a given soundfont ID.
- `StemIdentity` includes:
  - soundfont ID
  - optional MIDI channel
  - optional MIDI bank
  - optional MIDI program
  - percussion flag
- `StemRenderBlock` provides:
  - validated stereo `Vec<f32>` buffers
  - optional display name
  - active note tracking
  - interleaving helpers (`copy_to_interleaved`, `to_interleaved`)
- `StemRenderSet` validates frame consistency across blocks.

### 2. Grouped Stem Rendering in Synthesizer and Sequencer

- `Synthesizer::render_stems` and `Synthesizer::render_stems_into` generate grouped stems directly from active voices.
- `MidiFileSequencer::render_stems` and `MidiFileSequencer::render_stems_into` perform time-accurate event processing plus stem extraction.
- Grouped stems can include wet effects contributions (chorus/reverb) per stem identity.
- A reconciliation pass computes residual difference between direct mix and summed grouped stems:
  - if non-zero, residual is folded into a global whole-soundfont stem (display name may be `Global effects`), preserving direct-render parity.

### 3. MIDI SysEx Interpretation and Channel Role Changes

- `MidiInterpretation` tracks:
  - system modes (`GeneralMidi`, `GeneralMidi2`, `RolandGs`, `YamahaXg`)
  - percussion channel set (default includes channel 10 / index 9)
  - warnings
  - detailed SysEx event summaries
- Recognized SysEx handling includes:
  - GM/GM2 system on
  - Yamaha XG system mode and part role changes
  - Roland GS mode and channel role changes (with checksum validation and warning on invalid checksum)
- `Synthesizer::process_sysex_message` and `set_channel_role`/`set_percussion_channels` update channel percussion state at runtime.

### 4. MIDI Introspection Beyond Base Playback

- `MidiFile::get_interpretation` exposes interpreted SysEx/system behavior.
- `MidiFile::get_channel_program_roles` infers channel/bank/program/percussion role usage from stream events.
- `MidiFile::get_loop_start_ticks` and `get_loop_end_ticks` expose parsed loop markers.
- `MidiFile::get_time_at_tick` and `get_tick_at_time` provide tick/time mapping.

### 5. Extended Loop Marker Support

- `MidiFileLoopType` integration supports style-specific loop marker interpretation in CC streams:
  - RPG Maker
  - Incredible Machine
  - Final Fantasy
  - explicit `LoopPoint`

### 6. SoundFont Coverage Extraction

- `extract_coverage_from_sf2` builds `SoundFontCoverage` from loaded SF2 data:
  - melodic bank/program set
  - percussion presence and key ranges structure
  - metadata (preset names, sample count)
- Useful for preflight routing, compatibility checks, and UI/runtime hinting.

### 7. Runtime Memory and Allocation Diagnostics

- `MidiFile::memory_debug` reports event/tick/time/SysEx memory footprint estimates.
- `Synthesizer::memory_debug` reports:
  - soundfont wave/meta counts
  - voice/buffer usage
  - retained stem/effects cache metrics
  - last stem render allocation snapshot
  - estimated total bytes
- `StemRenderAllocationDebug` reports output/internal/residual/effect-input buffer footprints and growth.
- Sequencer exposes `last_stem_render_allocations` and forwards diagnostics through `memory_debug`.

### 8. Sequencer Control Additions

- `MidiFileSequencer::seek_to_seconds` and `seek_to_tick` rebuild synth state by replaying events up to target.
- `set_play_loop` toggles loop behavior on loaded MIDI.
- `get_tick_position` and `get_position` allow dual-domain transport inspection.

## Behavior Notes and Compatibility Differences

These are important when migrating code written for upstream RustySynth:

- Direct stereo rendering behavior remains the compatibility baseline.
- SysEx is not only parsed but interpreted for channel-role/percussion behavior, which affects routing and stem identity.
- Grouped stem output is identity-based and may include a global residual/effects stem to preserve parity.
- Stem APIs are additive and do not replace direct render APIs.
- Sequencer seek operations are deterministic replays, not random-access decode of audio state.
- Memory diagnostics are first-class APIs intended for runtime validation and churn tracking.

## Public API Highlights

Primary types you will use most often:

- Loading and synthesis:
  - `SoundFont`
  - `SynthesizerSettings`
  - `Synthesizer`
- MIDI playback and analysis:
  - `MidiFile`
  - `MidiFileSequencer`
  - `MidiInterpretation`
  - `MidiChannelProgramRole`
- Stem rendering:
  - `StemRenderRequest`
  - `StemRenderMode`
  - `StemIdentity`
  - `StemRenderBlock`
  - `StemRenderSet`
- Coverage and diagnostics:
  - `extract_coverage_from_sf2`
  - `SoundFontCoverage`
  - `MidiFileMemoryDebug`
  - `SynthesizerMemoryDebug`
  - `StemRenderAllocationDebug`

## Quick Start

### 1. Create Synthesizer and Render Stereo

```rust
use std::fs;
use std::io::Cursor;
use std::sync::Arc;

use rustystem::{SoundFont, Synthesizer, SynthesizerSettings};

fn main() {
    let sf2_bytes = fs::read("soundfonts/FluidR3_GM2-2.SF2").unwrap();
    let mut cursor = Cursor::new(sf2_bytes);
    let soundfont = Arc::new(SoundFont::new(&mut cursor).unwrap());

    let mut settings = SynthesizerSettings::new(48_000);
    settings.block_size = 512;
    settings.maximum_polyphony = 128;
    settings.enable_reverb_and_chorus = true;

    let mut synth = Synthesizer::new(&soundfont, &settings).unwrap();

    let frames = 48_000;
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    synth.render(&mut left, &mut right);
}
```

### 2. Render Grouped Stems from a MIDI File

```rust
use std::fs;
use std::io::Cursor;
use std::sync::Arc;

use rustystem::{
    MidiFile, MidiFileSequencer, SoundFont, StemRenderRequest, Synthesizer, SynthesizerSettings,
};

fn main() {
    let sf2 = fs::read("soundfonts/FluidR3_GM2-2.SF2").unwrap();
    let midi = fs::read("MIDI Files/rendering-parity-midi/example.mid").unwrap();

    let mut sf2_cursor = Cursor::new(sf2);
    let soundfont = Arc::new(SoundFont::new(&mut sf2_cursor).unwrap());

    let mut midi_cursor = Cursor::new(midi);
    let midi_file = Arc::new(MidiFile::new(&mut midi_cursor).unwrap());

    let settings = SynthesizerSettings::new(48_000);
    let synth = Synthesizer::new(&soundfont, &settings).unwrap();
    let mut seq = MidiFileSequencer::new(synth);
    seq.play(&midi_file, false);

    let stems = seq
        .render_stems(&StemRenderRequest::channel_program("default_sf"), 48_000)
        .blocks;

    println!("stem_count={}", stems.len());
}
```

## Examples and Validation Probes

### Stem API contract probe

```text
cargo run -p rustystem --example stem_api_probe
```

Checks identities, mode mapping, validation errors, and basic interleaving behavior.

### Direct-vs-stem parity probe (DAT-aware)

```text
cargo run -p rustystem --example stem_parity_probe -- --dat-dir _local/generated-assets/dat --frames 48000
```

Optional arguments:

- `--soundfont-id <id>`: select specific DAT soundfont internal ID.
- `--midi-dir <path>` or `--midi <path>`: choose probe input MIDI source.
- `--frames <n>`: choose frame count.

Probe output includes:

- direct/mixed peak and RMS
- raw sum and mixed diffs
- per-stem meters and identity summary
- mute/solo leak checks against expected behavior
- global residual stem metrics

Reports are written under `_local/runtime-tests/measurements`.

## Developer Guidance

### Choosing Stem Modes

- Use `WholeSoundFont` when you want exact direct output in one block.
- Use `ChannelProgram` for app-facing strip routing (most common in flutzPlayer).
- Use `MidiProgram` for instrument-family grouping across channels.
- Use `MidiChannel` for channel-oriented workflows.
- Use `Percussion` to isolate drum/percussion channels.

### Handling Global Residual Stems

- Treat whole-soundfont residual/global-effect stems as non-user strips in most UIs.
- Include residual behavior in routing/mute/solo policy so summed output remains parity-safe.
- In flutzPlayer integration, keep residual controls derived from visible-strip state.

### Performance and Memory Inspection

- Capture `Synthesizer::memory_debug()` around render cycles.
- Capture `last_stem_render_allocations()` for per-render churn snapshots.
- Prefer `render_stems_into` reuse paths in hot loops to reduce transient allocations.

### SysEx and Channel Roles

- If playback source uses GS/XG role switching, ensure SysEx events are fed through sequencer or synthesizer message path.
- Do not assume only channel 10 is percussion after interpretation begins.

## Common Pitfalls

- Left/right buffers passed to render methods must match in length.
- Synth settings must stay within validated ranges:
  - sample rate: 16,000..=192,000
  - block size: 8..=1024
  - polyphony: 8..=256
- Stem blocks must keep stereo frame counts aligned; constructors and `validate` enforce this.

## Project Context

This crate is intentionally local to the flutzPlayer workspace and collaborates with sibling crates (notably `flutz_dat`, `flutz_mixer`, and `flutz_core`) for DAT-based assets, neutral mixing parity checks, and runtime routing identity.

## Development Workflow

From workspace root:

```text
cargo fmt --check
cargo check -p rustystem
cargo run -p rustystem --example stem_api_probe
cargo run -p rustystem --example stem_parity_probe -- --dat-dir _local/generated-assets/dat --frames 48000
```
