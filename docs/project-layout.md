# Project Layout Design

This project uses source-code-first implementation tracking with complete items and completion contracts rather than temporary milestone terminology.

Initial implementation should match the final target described by the specification and design docs. Work marked complete should not require later rewrites to support later implementation items; later work integrates through stable ownership boundaries and public contracts.

Implementation tracking lives in `docs/implementation-tracking.md`.

## Workspace Layout

```text
flutzPlayer/
   Cargo.toml
   build.ps1
   .cargo/
      config.toml

   crates/
      flutz_app/
      flutz_core/
      flutz_mixer/
      flutz_audio_sdl3/
      flutz_synth/
      flutz_fmid/
      flutz_dat/
      flutz_soundfont_tools/

   docs/

   assets/
      dat-manifest.toml      

   vendor/
      SDL3/

   _local/
      target/
      logs/
      runtime-tests/
      generated-assets/
      scratch/

   drops/
      flutzplayer/
         flutzplayer executable
         data/
            required DAT files
```

## Artifact Policy

Source code and tracked documentation stay separate from builds, drops, runtime test output, generated assets, logs, and local scratch files.

`_local/` is the ignored local workspace folder for compiler artifacts, runtime-test scripts, captured measurements, generated DAT files, converted soundfonts, logs, and scratch material.

`drops/` is the ignored top-level release/drop folder. A prepared app drop uses `drops/flutzplayer/` with complete runtime `.dat` files under `drops/flutzplayer/data/`.

Generated DAT files and converted soundfonts are not source-controlled. They live under `_local/` during generation and under `drops/flutzplayer/data/` only as part of a local packaged drop.

## Testing Policy

Do not rely on or use built-in Rust test functions as the validation strategy. Runtime validation runs compiled binaries or helper executables, measures real outputs, and inspects debug/log output produced by the running binary.

Runtime-test scripts live only under ignored `_local/runtime-tests/scripts/`. Runtime-test results, captured output, measurements, and reports also live under `_local/runtime-tests/`.

## Crate Ownership

`flutz_mixer` is a reusable mixer crate. It does not depend on egui, eframe, SDL3, FMID, DAT, or rustysynth. It owns buffer-level mixing, per-strip controls, master controls, meters, smart-mix, auto-normalization, and mixer DSP contracts.

`flutz_audio_sdl3` is a reusable SDL3 audio wrapper crate. It is generic enough for non-MIDI audio projects. It owns SDL3 device setup, callback bridging, lock-free final-audio ring buffer integration, underrun diagnostics, and audio-output lifecycle.

`flutz_synth` owns MIDI timeline handling, seek/loop state rebuild support, rustysynth renderer integration, runtime soundfont loading behavior, and soundfont runtime capability checks.

`flutz_fmid` owns FMID read/write, chunk tables, records, known/unknown chunk preservation, and primitive encoding.

`flutz_dat` owns DAT read/write, manifest consumption, asset metadata, chunk records, entry records, and DAT packing/reading behavior.

`flutz_soundfont_tools` owns rustysynth compatibility review helpers, source-format detection, and any later conversion tooling if required.

`flutz_core` owns shared IDs, error types, project model types, and small cross-crate domain types that do not belong to a format, UI, audio, or mixer crate.

`flutz_app` composes the reusable crates into the egui/eframe application and keeps app-specific UI behavior out of reusable library crates.

Developer automation scripts remain local under ignored `_local/runtime-tests/scripts/` when needed. Source-controlled build/package entry points stay at the repository root (`build.ps1`, `build-dat.ps1`).
