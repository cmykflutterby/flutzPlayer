use std::{
    backtrace::Backtrace,
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Write as _,
    fs,
    io::{BufWriter, Cursor, Write},
    mem,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use flutz_audio_sdl3::{
    AudioDeviceConfig, AudioDeviceDiagnostics, AudioMemoryHooks, RenderAheadState,
    RenderAheadTarget, SdlAudioOutput,
};
#[cfg(windows)]
use flutz_audio_wasapi::WasapiAudioOutput;
use flutz_core::{FlutzError, Result, SoundFontId, StripId};
use flutz_dat::{
    assets::DatEntryRecord,
    read::{
        coalesce_entry_ranges, extract_entry_from_file, parse_dat_index_file,
        read_soundfont_json_resources_from_file, read_soundfont_sample_ranges_from_files,
        read_soundfont_thin_metadata_from_files, DatArchiveIndex, DatEntryRange,
        SoundFontJsonResources, SplitDatEntryPart,
    },
    soundfont_json::SoundFontIndexJson,
};
use flutz_formats::{
    ContentKind, DecodedAudioBuffer, DecodedAudioStreamSession, DecodedAudioStreamSource,
    MasteringCapability,
};
use flutz_mixer::{
    AudioBlockView, MeterReading, MixerEngine, MixerSettings, MixerStripControls,
    MixerStripIdentity, MixerStripInputView, StripMixReport,
};
use flutz_peq::{ChannelLayout, PeqConfig, PeqPresetFile, PeqProcessor, PreparedConfig};
use flutz_synth::{
    LoadedMidi, LoadedSoundFont, MidiFileLoopType, MultiSoundFontPlayback, PlaybackConfig,
    PlaybackLoopMode, PlaybackLoopSettings, PlaybackMemoryDebug, PlaybackState, SoundFontBytes,
    SoundFontCoverage, SoundFontRuntimeCache, SoundFontRuntimeCacheDebug, SoundFontSubsetBytes,
    SoundFontSubsetSampleRange, StemIdentity, StemRenderAllocationDebug, StemRenderBlock,
};
use flutz_visualizer_core::{VisualizerAnalyzer, VisualizerAnalyzerConfig, VisualizerFrame};
use rustystem::{MidiFile, MidiInterpretation, MidiSystemMode, SoundFontMetadataClosure};

use crate::{
    allocation_trace::{AllocationScope, AllocationScopeGuard},
    app::{SoundFontCatalogEntry, FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER},
    memory_runtime::{self, MemoryDomain},
    repaint_policy::UiWaker,
};

mod audio_output;
mod controller;
mod decoded_audio;
mod diagnostics;
use diagnostics::{
    clone_mixer_controls_with_recovery, record_analyzer_trace_arc, render_mixed_audio_guarded,
    stem_blocks_audio_bytes, stem_blocks_total_bytes, AnalyzerTrace, RenderErrorTrace,
    RenderFailureContext,
};
pub use diagnostics::{
    ArcReferenceDiagnostics, AudioRuntimeMemoryDiagnostics, MidiRuntimeMemoryDiagnostics,
    PlaybackComponentMemoryDiagnostics, PlaybackDebugMetrics, PlaybackLifecycleDiagnostics,
    PlaybackReferenceDiagnostics, RenderChurnDiagnostics, RenderChurnSnapshot,
    RenderErrorTraceEvent, RustystemRuntimeDiagnostics, SoundFontMetadataDiagnostics,
    VisualizerDebugMetrics,
};
mod midi;
mod idle_memory;
use idle_memory::touch_pages;
pub use idle_memory::PlaybackPrewarmReport;
mod render;
use render::{render_mixed_audio, MixedAudioRenderReport, PlaybackRenderScratch, SnapshotHistory};
pub use render::{MixerControlState, RealtimeMixerSnapshot, RealtimeStripSnapshot};
mod soundfonts;
use audio_output::{output_latency_breakdown, AudioOutput, FluxGuard, RenderLayerTransition};
pub use audio_output::{
    AudioBackend, FluxGuardDecision, FluxGuardDecisionReason, FluxGuardSnapshot,
    OutputLatencyBreakdown, OutputViewKind, OutputViewSnapshot,
};
use decoded_audio::{render_decoded_audio_stereo, DecodedAudioPlaybackState};
pub use decoded_audio::{
    AudioPlaybackStatus, DecodedAudioTransportMetadata, DecodedRenderProbeReport,
    DecodedStreamCacheDebug,
};
use midi::{
    json_option_string, json_string, json_string_array, midi_scan_json, midi_scan_trace_json,
    MidiDemandProfile,
};
pub use midi::{
    BankProgramDemand, MidiStripDescriptor, MidiTransportMetadata, PlaybackMidiChannelRoleChange,
    PlaybackMidiScanDiagnostics, PlaybackMidiSysexEvent, RenderProbeReport,
    RenderProbeStripMixDiagnostics, RenderStemProbeReport, SoundFontDemandDiagnostics,
    SoundFontDemandSignature,
};
pub use soundfonts::{
    LoadedSubsetState, PlaybackSoundFontLoadDiagnostics, PlaybackSubsetTransitionDiagnostics,
    SoundFontSubsetPlanAggregateDebug, SoundFontSubsetPlanDebug,
};

pub struct PlaybackController {
    data_dir: PathBuf,
    catalog: Vec<SoundFontCatalogEntry>,
    engine: Option<Arc<Mutex<MultiSoundFontPlayback>>>,
    mixer: Arc<Mutex<MixerEngine>>,
    mixer_controls: Arc<Mutex<MixerControlState>>,
    session_output_gain: Arc<Mutex<f32>>,
    latest_snapshot: Arc<Mutex<SnapshotHistory>>,
    visualizer_analyzer: Arc<Mutex<VisualizerAnalyzer>>,
    analyzer_trace: Option<Arc<Mutex<AnalyzerTrace>>>,
    render_error_trace: Option<Arc<Mutex<RenderErrorTrace>>>,
    rendered_frame_clock: Arc<AtomicU64>,
    audio_backend: AudioBackend,
    audio: Option<AudioOutput>,
    audio_config: AudioDeviceConfig,
    flux_guard: Mutex<FluxGuard>,
    audio_error: Option<String>,
    decoded_audio: Option<Arc<Mutex<DecodedAudioPlaybackState>>>,
    loaded_midi: Option<PathBuf>,
    loaded_midi_bytes: Option<Vec<u8>>,
    loaded_soundfonts: Vec<String>,
    loaded_coverages: BTreeMap<String, SoundFontCoverage>,
    soundfont_cache: SoundFontRuntimeCache,
    midi_transport: MidiTransportMetadata,
    midi_strips: Arc<Mutex<Vec<MidiStripDescriptor>>>,
    render_scratch: Arc<Mutex<PlaybackRenderScratch>>,
    render_churn: Arc<Mutex<RenderChurnDiagnostics>>,
    lifecycle: PlaybackLifecycleDiagnostics,
    last_demand_profile: MidiDemandProfile,
    last_midi_scan: PlaybackMidiScanDiagnostics,
    last_soundfont_demand: SoundFontDemandDiagnostics,
    loaded_subset_state: BTreeMap<String, LoadedSubsetState>,
    subset_transition: PlaybackSubsetTransitionDiagnostics,
    last_subset_plans: BTreeMap<String, SoundFontSubsetPlanDebug>,
    last_soundfont_load: PlaybackSoundFontLoadDiagnostics,
    ui_waker: Option<UiWaker>,
    last_prewarm: Option<PlaybackPrewarmReport>,
}
