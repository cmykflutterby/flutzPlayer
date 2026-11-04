use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    mem,
    sync::Arc,
    thread,
};

use flutz_core::{FlutzError, Result};
use rustystem::{
    extract_coverage_from_sf2, MidiChannelProgramRole, MidiFile, MidiFileLoopType,
    MidiFileMemoryDebug, MidiFileSequencer, MidiInterpretation, SequencerLoopMode,
    SequencerLoopSettings, SoundFont, SoundFontCoverage, SoundFontMetadata,
    SoundFontMetadataClosure, StemRenderAllocationDebug, StemRenderBlock, StemRenderRequest,
    Synthesizer, SynthesizerMemoryDebug, SynthesizerSettings,
};

mod config;
mod engine;
mod midi;
mod soundfont_cache;
pub use config::{PlaybackConfig, PlaybackLoopMode, PlaybackLoopSettings};
pub use engine::{
    MultiSoundFontPlayback, PlaybackMemoryDebug, PlaybackState, SynthInstanceMemoryDebug,
};
pub use midi::{validate_midi_bytes, validate_midi_bytes_with_loop_type, LoadedMidi};
pub use soundfont_cache::{
    LoadedSoundFont, SoundFontBytes, SoundFontDataSource, SoundFontRuntimeCache,
    SoundFontRuntimeCacheDebug, SoundFontRuntimeCacheEntryDebug, SoundFontSubsetBytes,
    SoundFontSubsetSampleRange,
};
