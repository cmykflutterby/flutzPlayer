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
};

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum AudioBackend {
    #[default]
    Sdl3,
    Wasapi,
}

impl AudioBackend {
    fn name(self) -> &'static str {
        match self {
            Self::Sdl3 => "sdl3",
            Self::Wasapi => "wasapi",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sdl3" => Ok(Self::Sdl3),
            "wasapi" => Ok(Self::Wasapi),
            _ => Err(FlutzError::InvalidInput(
                "--audio-backend requires sdl3 or wasapi".to_owned(),
            )),
        }
    }
}

enum AudioOutput {
    Sdl3(SdlAudioOutput),
    Wasapi(WasapiAudioOutput),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FluxGuardConfig {
    min_latency_ms: u32,
    initial_latency_ms: u32,
    max_latency_ms: u32,
    underrun_penalty_ms: u32,
    queue_error_penalty_ms: u32,
    low_water_headroom_ms: u32,
    stable_step_down_ms: u32,
    stable_observations_required: u32,
    cooldown_observations: u32,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum FluxGuardDecisionReason {
    #[default]
    NoAudio,
    Initialized,
    Stable,
    QueueErrorPressure,
    UnderrunPressure,
    LowWater,
    Cooldown,
}

impl FluxGuardDecisionReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoAudio => "no-audio",
            Self::Initialized => "initialized",
            Self::Stable => "stable",
            Self::QueueErrorPressure => "queue-error-pressure",
            Self::UnderrunPressure => "underrun-pressure",
            Self::LowWater => "low-water",
            Self::Cooldown => "cooldown",
        }
    }
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct FluxGuardDecision {
    pub target: RenderAheadTarget,
    pub state: RenderAheadState,
    pub reason: FluxGuardDecisionReason,
    pub changed: bool,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct FluxGuardSnapshot {
    pub decision: FluxGuardDecision,
    pub initialized: bool,
    pub stable_observations: u32,
    pub cooldown_observations: u32,
}

impl Default for FluxGuardConfig {
    fn default() -> Self {
        Self {
            min_latency_ms: 12,
            initial_latency_ms: 80,
            max_latency_ms: 250,
            underrun_penalty_ms: 40,
            queue_error_penalty_ms: 60,
            low_water_headroom_ms: 20,
            stable_step_down_ms: 5,
            stable_observations_required: 30,
            cooldown_observations: 8,
        }
    }
}

#[derive(Debug, Clone)]
struct FluxGuard {
    config: FluxGuardConfig,
    target_frames: u32,
    last_underrun_count: u64,
    last_queue_error_count: u64,
    stable_observations: u32,
    cooldown_observations: u32,
    initialized: bool,
    last_decision: FluxGuardDecision,
}

impl Default for FluxGuard {
    fn default() -> Self {
        Self {
            config: FluxGuardConfig::default(),
            target_frames: 0,
            last_underrun_count: 0,
            last_queue_error_count: 0,
            stable_observations: 0,
            cooldown_observations: 0,
            initialized: false,
            last_decision: FluxGuardDecision::default(),
        }
    }
}

impl FluxGuard {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn observe(&mut self, state: Option<RenderAheadState>, sample_rate: u32) -> FluxGuardSnapshot {
        let Some(state) = state else {
            self.last_decision = FluxGuardDecision::default();
            return self.snapshot();
        };

        let sample_rate = sample_rate.max(1);
        let min_frames = frames_from_ms(sample_rate, self.config.min_latency_ms);
        let initial_frames = frames_from_ms(sample_rate, self.config.initial_latency_ms);
        let max_frames = frames_from_ms(sample_rate, self.config.max_latency_ms).max(min_frames);
        if !self.initialized {
            self.initialized = true;
            self.target_frames = initial_frames.clamp(min_frames, max_frames);
        }

        let underrun_delta = state
            .underrun_count
            .saturating_sub(self.last_underrun_count);
        let queue_error_delta = state
            .queue_error_count
            .saturating_sub(self.last_queue_error_count);
        self.last_underrun_count = state.underrun_count;
        self.last_queue_error_count = state.queue_error_count;

        let callback_headroom = state
            .largest_callback_frames
            .saturating_mul(4)
            .max(min_frames);
        let mut proposed_target = self.target_frames.max(callback_headroom);
        let mut reason = FluxGuardDecisionReason::Stable;

        if queue_error_delta > 0 {
            reason = FluxGuardDecisionReason::QueueErrorPressure;
            proposed_target = proposed_target
                .max(state.current_buffered_frames)
                .saturating_add(frames_from_ms(
                    sample_rate,
                    self.config.queue_error_penalty_ms,
                ));
            self.cooldown_observations = self.config.cooldown_observations;
            self.stable_observations = 0;
        } else if underrun_delta > 0 {
            reason = FluxGuardDecisionReason::UnderrunPressure;
            proposed_target = proposed_target
                .max(state.current_buffered_frames)
                .saturating_add(frames_from_ms(sample_rate, self.config.underrun_penalty_ms));
            self.cooldown_observations = self.config.cooldown_observations;
            self.stable_observations = 0;
        } else if state.current_buffered_frames <= self.low_water_frames() {
            reason = FluxGuardDecisionReason::LowWater;
            proposed_target = proposed_target.saturating_add(frames_from_ms(
                sample_rate,
                self.config.low_water_headroom_ms,
            ));
            self.cooldown_observations = self.config.cooldown_observations;
            self.stable_observations = 0;
        } else if self.cooldown_observations > 0 {
            reason = FluxGuardDecisionReason::Cooldown;
            self.cooldown_observations = self.cooldown_observations.saturating_sub(1);
        } else if state.current_buffered_frames >= self.high_water_frames() {
            self.stable_observations = self.stable_observations.saturating_add(1);
            if self.stable_observations >= self.config.stable_observations_required {
                proposed_target = proposed_target
                    .saturating_sub(frames_from_ms(sample_rate, self.config.stable_step_down_ms));
                self.stable_observations = 0;
            }
        } else {
            self.stable_observations = 0;
        }

        let proposed_target = proposed_target.clamp(min_frames, max_frames);
        let material_delta = proposed_target.abs_diff(self.target_frames)
            >= frames_from_ms(sample_rate, self.config.stable_step_down_ms).max(1);
        let changed = material_delta || self.last_decision.reason != reason;
        if material_delta {
            self.target_frames = proposed_target;
        }

        self.last_decision = FluxGuardDecision {
            target: RenderAheadTarget {
                target_frames: self.target_frames,
                low_water_frames: self.low_water_frames(),
                high_water_frames: self.high_water_frames(),
            },
            state: RenderAheadState {
                effective_target_frames: self.target_frames,
                ..state
            },
            reason: if self.last_decision.reason == FluxGuardDecisionReason::NoAudio {
                FluxGuardDecisionReason::Initialized
            } else {
                reason
            },
            changed,
        };
        self.snapshot()
    }

    fn snapshot(&self) -> FluxGuardSnapshot {
        FluxGuardSnapshot {
            decision: self.last_decision,
            initialized: self.initialized,
            stable_observations: self.stable_observations,
            cooldown_observations: self.cooldown_observations,
        }
    }

    fn low_water_frames(&self) -> u32 {
        self.target_frames / 2
    }

    fn high_water_frames(&self) -> u32 {
        self.target_frames.saturating_add(self.target_frames / 2)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum OutputViewKind {
    Live,
    Audible,
}

#[derive(Debug, Clone)]
pub struct OutputViewSnapshot {
    pub kind: OutputViewKind,
    pub rendered_frame_clock: u64,
    pub effective_latency_frames: u64,
    pub effective_latency_ms: f64,
    pub wrapper_queue_frames: u64,
    pub device_queue_frames: u64,
    pub snapshot: RealtimeMixerSnapshot,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct OutputLatencyBreakdown {
    pub total_frames: u64,
    pub wrapper_queue_frames: u64,
    pub device_queue_frames: u64,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
struct RenderLayerTransition {
    warmup_tick: Option<u64>,
}

impl AudioOutput {
    fn open_f32_stream(
        backend: AudioBackend,
        config: AudioDeviceConfig,
        renderer: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> std::result::Result<Self, String> {
        let _memory_domain = memory_runtime::bind_current_thread(MemoryDomain::AudioBackend);
        let memory_hooks = audio_memory_hooks();
        match backend {
            AudioBackend::Sdl3 => {
                SdlAudioOutput::open_f32_stream_with_memory_hooks(config, memory_hooks, renderer)
                    .map(Self::Sdl3)
            }
            AudioBackend::Wasapi => {
                WasapiAudioOutput::open_f32_stream_with_memory_hooks(config, memory_hooks, renderer)
                    .map(Self::Wasapi)
            }
        }
    }

    fn resume(&mut self) -> std::result::Result<(), String> {
        match self {
            Self::Sdl3(audio) => audio.resume(),
            Self::Wasapi(audio) => audio.resume(),
        }
    }

    fn pause(&mut self) -> std::result::Result<(), String> {
        match self {
            Self::Sdl3(audio) => audio.pause(),
            Self::Wasapi(audio) => audio.pause(),
        }
    }

    fn diagnostics(&self) -> AudioDeviceDiagnostics {
        match self {
            Self::Sdl3(audio) => audio.diagnostics(),
            Self::Wasapi(audio) => audio.diagnostics(),
        }
    }

    fn render_ahead_state(&self) -> RenderAheadState {
        match self {
            Self::Sdl3(audio) => audio.render_ahead_state(),
            Self::Wasapi(audio) => audio.render_ahead_state(),
        }
    }

    fn apply_render_ahead_target(
        &mut self,
        target: RenderAheadTarget,
    ) -> std::result::Result<(), String> {
        match self {
            Self::Sdl3(audio) => audio.apply_render_ahead_target(target),
            Self::Wasapi(audio) => audio.apply_render_ahead_target(target),
        }
    }

    fn backend_name(&self) -> &'static str {
        match self {
            Self::Sdl3(_) => "sdl3",
            Self::Wasapi(_) => "wasapi",
        }
    }
}

fn audio_memory_hooks() -> AudioMemoryHooks {
    AudioMemoryHooks {
        producer: Some(Arc::new(|| {
            Box::new(memory_runtime::bind_current_thread(
                MemoryDomain::AudioBackend,
            ))
        })),
        callback: Some(Arc::new(|| {
            Box::new(memory_runtime::bind_current_thread(
                MemoryDomain::AudioCallback,
            ))
        })),
    }
}

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
}

impl PlaybackController {
    pub fn new(
        data_dir: PathBuf,
        catalog: Vec<SoundFontCatalogEntry>,
        audio_backend: AudioBackend,
        debug_analyzer: bool,
        debug_render_errors: bool,
    ) -> Self {
        Self {
            data_dir,
            catalog,
            engine: None,
            mixer: Arc::new(Mutex::new(MixerEngine::new(MixerSettings::default()))),
            mixer_controls: Arc::new(Mutex::new(MixerControlState::default())),
            session_output_gain: Arc::new(Mutex::new(0.5)),
            latest_snapshot: Arc::new(Mutex::new(SnapshotHistory::default())),
            visualizer_analyzer: Arc::new(Mutex::new(VisualizerAnalyzer::new(
                VisualizerAnalyzerConfig::with_sample_rate(
                    AudioDeviceConfig::default().sample_rate,
                ),
            ))),
            analyzer_trace: debug_analyzer
                .then(AnalyzerTrace::open)
                .flatten()
                .map(|trace| Arc::new(Mutex::new(trace))),
            render_error_trace: debug_render_errors
                .then(RenderErrorTrace::open)
                .flatten()
                .map(|trace| Arc::new(Mutex::new(trace))),
            rendered_frame_clock: Arc::new(AtomicU64::new(0)),
            audio_backend,
            audio: None,
            audio_config: AudioDeviceConfig::default(),
            flux_guard: Mutex::new(FluxGuard::default()),
            audio_error: None,
            decoded_audio: None,
            loaded_midi: None,
            loaded_midi_bytes: None,
            loaded_soundfonts: Vec::new(),
            loaded_coverages: BTreeMap::new(),
            soundfont_cache: SoundFontRuntimeCache::default(),
            midi_transport: MidiTransportMetadata::default(),
            midi_strips: Arc::new(Mutex::new(Vec::new())),
            render_scratch: Arc::new(Mutex::new(PlaybackRenderScratch::default())),
            render_churn: Arc::new(Mutex::new(RenderChurnDiagnostics::default())),
            lifecycle: PlaybackLifecycleDiagnostics::default(),
            last_demand_profile: MidiDemandProfile::default(),
            last_midi_scan: PlaybackMidiScanDiagnostics::default(),
            last_soundfont_demand: SoundFontDemandDiagnostics::default(),
            loaded_subset_state: BTreeMap::new(),
            subset_transition: PlaybackSubsetTransitionDiagnostics::default(),
            last_subset_plans: BTreeMap::new(),
            last_soundfont_load: PlaybackSoundFontLoadDiagnostics::default(),
        }
    }

    pub fn set_mixer_controls(&mut self, controls: MixerControlState) -> Result<()> {
        let mut shared = match self.mixer_controls.lock() {
            Ok(shared) => shared,
            Err(poisoned) => {
                self.record_render_lock_recovery("set_mixer_controls", "mixer_controls");
                poisoned.into_inner()
            }
        };
        *shared = controls;
        Ok(())
    }

    pub fn set_session_output_gain(&mut self, gain: f32) -> Result<()> {
        let mut shared = self
            .session_output_gain
            .lock()
            .map_err(|_| FlutzError::Runtime("session output gain lock is poisoned".to_owned()))?;
        *shared = gain.clamp(0.0, FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER);
        Ok(())
    }

    fn reset_visualizer_to_silence(&self) {
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            let frame = analyzer.reset_to_silence();
            self.record_analyzer_trace(&frame);
        }
    }

    fn record_analyzer_trace(&self, frame: &VisualizerFrame) {
        record_analyzer_trace_arc(self.analyzer_trace.as_ref(), frame);
    }

    pub fn latest_snapshot(&self) -> RealtimeMixerSnapshot {
        self.audible_output_view().snapshot
    }

    pub fn visualizer_frame(&self) -> VisualizerFrame {
        self.visualizer_analyzer
            .lock()
            .map(|analyzer| analyzer.latest_frame())
            .unwrap_or_default()
    }

    pub fn midi_scan_diagnostics(&self) -> PlaybackMidiScanDiagnostics {
        self.last_midi_scan.clone()
    }

    pub fn take_render_error_events(&self) -> Vec<RenderErrorTraceEvent> {
        self.render_error_trace
            .as_ref()
            .and_then(|trace| trace.lock().ok().map(|mut trace| trace.take_recent()))
            .unwrap_or_default()
    }

    fn record_render_lock_recovery(&self, source: &'static str, lock_name: &'static str) {
        record_render_error_event(
            self.render_error_trace.as_ref(),
            RenderErrorTraceEvent {
                source: source.to_owned(),
                failure_kind: "lock-recovery".to_owned(),
                detail: format!("recovered poisoned {lock_name} lock"),
                backtrace: None,
                frames_requested: 0,
                midi_source: self
                    .loaded_midi
                    .as_ref()
                    .map(|path| path.display().to_string()),
                soundfont_ids: self.loaded_soundfonts.clone(),
                midi_scan: self.last_midi_scan.clone(),
            },
        );
    }

    fn clone_mixer_controls_for_render(&self, source: &'static str) -> MixerControlState {
        clone_mixer_controls_with_recovery(
            &self.mixer_controls,
            self.render_error_trace.as_ref(),
            source,
            self.loaded_midi.as_deref(),
        )
    }

    fn record_midi_scan_trace(&self) {
        if let Some(trace) = self.render_error_trace.as_ref() {
            if let Ok(mut trace) = trace.lock() {
                trace.record_midi_scan(self.loaded_midi.as_deref(), &self.last_midi_scan);
            }
        }
    }

    pub fn live_output_view(&self) -> OutputViewSnapshot {
        let rendered_frames = self.rendered_frame_clock.load(Ordering::Relaxed);
        let snapshot = self
            .latest_snapshot
            .lock()
            .map(|snapshot| snapshot.live_snapshot())
            .unwrap_or_default();
        OutputViewSnapshot {
            kind: OutputViewKind::Live,
            rendered_frame_clock: rendered_frames,
            effective_latency_frames: 0,
            effective_latency_ms: 0.0,
            wrapper_queue_frames: 0,
            device_queue_frames: 0,
            snapshot,
        }
    }

    pub fn playback_memory_debug(&self) -> Option<PlaybackMemoryDebug> {
        self.engine
            .as_ref()
            .and_then(|engine| engine.lock().ok().map(|engine| engine.memory_debug()))
    }

    pub fn audible_output_view(&self) -> OutputViewSnapshot {
        let rendered_frames = self.rendered_frame_clock.load(Ordering::Relaxed);
        let latency = self.output_latency_breakdown();
        let sample_rate = self.audio_config.sample_rate.max(1) as f64;
        let snapshot = self
            .latest_snapshot
            .lock()
            .map(|snapshot| snapshot.delayed_snapshot(rendered_frames, latency.total_frames))
            .unwrap_or_default();
        OutputViewSnapshot {
            kind: OutputViewKind::Audible,
            rendered_frame_clock: rendered_frames,
            effective_latency_frames: latency.total_frames,
            effective_latency_ms: latency.total_frames as f64 * 1000.0 / sample_rate,
            wrapper_queue_frames: latency.wrapper_queue_frames,
            device_queue_frames: latency.device_queue_frames,
            snapshot,
        }
    }

    pub fn output_latency_breakdown(&self) -> OutputLatencyBreakdown {
        let diagnostics = self.audio.as_ref().map(AudioOutput::diagnostics);
        output_latency_breakdown(diagnostics.as_ref())
    }

    pub fn estimated_output_latency_frames(&self) -> u64 {
        self.output_latency_breakdown().total_frames
    }

    pub fn estimated_output_latency_seconds(&self) -> f64 {
        let sample_rate = self.audio_config.sample_rate.max(1) as f64;
        self.estimated_output_latency_frames() as f64 / sample_rate
    }

    pub fn audible_transport_seconds(&self) -> f64 {
        if let Some(decoded) = &self.decoded_audio {
            return decoded
                .lock()
                .map(|decoded| decoded.position_seconds())
                .unwrap_or(0.0);
        }
        let Some(engine) = &self.engine else {
            return 0.0;
        };
        match engine.lock() {
            Ok(engine) => (engine.position_seconds() - self.estimated_output_latency_seconds())
                .clamp(0.0, self.midi_transport.duration_seconds.max(0.0)),
            Err(_) => 0.0,
        }
    }

    pub fn load_midi_file(
        &mut self,
        path: &Path,
        requested_soundfonts: &[String],
    ) -> Result<String> {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::FileLoad);
        let midi_bytes = fs::read(path).map_err(|error| {
            FlutzError::Runtime(format!(
                "failed to read MIDI file {}: {error}",
                path.display()
            ))
        })?;
        self.load_midi_data(midi_bytes, path.to_owned(), requested_soundfonts)
    }

    pub fn load_midi_bytes(
        &mut self,
        midi_bytes: Vec<u8>,
        source_name: impl Into<String>,
        requested_soundfonts: &[String],
    ) -> Result<String> {
        self.load_midi_data(
            midi_bytes,
            PathBuf::from(source_name.into()),
            requested_soundfonts,
        )
    }

    fn load_midi_data(
        &mut self,
        midi_bytes: Vec<u8>,
        source_path: PathBuf,
        requested_soundfonts: &[String],
    ) -> Result<String> {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::PlaybackLoad);
        self.decoded_audio = None;
        let scan = analyze_midi_file(&midi_bytes)?;
        let detected_loop_type = scan.detected_loop_type;
        self.last_demand_profile = scan.demand_profile.clone();
        self.last_midi_scan = scan.diagnostics.clone();
        let soundfont_ids =
            self.resolve_soundfont_ids(requested_soundfonts, &scan.channel_roles)?;
        self.last_soundfont_demand = self.soundfont_demand_diagnostics(
            requested_soundfonts,
            &soundfont_ids,
            &scan.demand_profile,
        );
        let transition = self.capture_render_layer_transition(&midi_bytes, &soundfont_ids);
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }

        let config = PlaybackConfig::default();
        self.audio_config = AudioDeviceConfig {
            sample_rate: config.sample_rate,
            channels: 2,
            internal_block_frames: config.block_frames as u16,
            ..AudioDeviceConfig::default()
        };
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            *analyzer = VisualizerAnalyzer::new(VisualizerAnalyzerConfig::with_sample_rate(
                self.audio_config.sample_rate,
            ));
        }

        let can_reuse_loaded_engine = self.loaded_soundfonts == soundfont_ids
            && self.engine.is_some()
            && self.loaded_subset_state_satisfies(&soundfont_ids, &scan.channel_roles)?;
        let mut fallback_note = None::<String>;
        let loaded_midi = if can_reuse_loaded_engine {
            if let Some(engine) = &self.engine {
                engine
                    .lock()
                    .map_err(|_| {
                        FlutzError::Runtime("playback engine lock is poisoned".to_owned())
                    })?
                    .load_midi_bytes_with_loop_type(&midi_bytes, detected_loop_type)?
            } else {
                unreachable!("can_reuse_loaded_engine requires an existing playback engine")
            }
        } else {
            self.audio = None;
            self.audio_error = None;
            let mut attempts = vec![soundfont_ids.clone()];
            if requested_soundfonts.is_empty() {
                for candidate in self.catalog.iter().map(|font| font.internal_id.clone()) {
                    if !soundfont_ids.contains(&candidate) {
                        attempts.push(vec![candidate]);
                    }
                }
            }

            let mut load_errors = Vec::new();
            let mut loaded = None;
            for candidate_ids in attempts {
                match self.load_soundfonts_for_playback(&candidate_ids, &scan.channel_roles) {
                    Ok(soundfonts) => {
                        match MultiSoundFontPlayback::new_with_loaded_soundfonts(soundfonts, config)
                        {
                            Ok(mut engine) => {
                                let loaded_midi = engine.load_midi_bytes_with_loop_type(
                                    &midi_bytes,
                                    detected_loop_type,
                                )?;
                                self.last_soundfont_demand = self.soundfont_demand_diagnostics(
                                    requested_soundfonts,
                                    &candidate_ids,
                                    &scan.demand_profile,
                                );
                                loaded = Some((candidate_ids, loaded_midi, engine));
                                break;
                            }
                            Err(error) => {
                                load_errors.push(format!("{} => {error}", candidate_ids.join(",")));
                            }
                        }
                    }
                    Err(error) => {
                        load_errors.push(format!("{} => {error}", candidate_ids.join(",")));
                    }
                }
            }

            let Some((resolved_ids, loaded_midi, engine)) = loaded else {
                let details = if load_errors.is_empty() {
                    "no candidate soundfonts could be loaded".to_owned()
                } else {
                    load_errors.join("; ")
                };
                return Err(FlutzError::InvalidInput(format!(
                    "failed to initialize playback soundfont set: {details}"
                )));
            };

            if resolved_ids != soundfont_ids {
                fallback_note = Some(format!(
                    "Fallback soundfont selection: requested [{}], loaded [{}]",
                    soundfont_ids.join(", "),
                    resolved_ids.join(", ")
                ));
            }

            self.record_engine_replacement();
            self.loaded_soundfonts = resolved_ids;
            self.loaded_coverages = engine.soundfont_coverages().into_iter().collect();
            self.engine = Some(Arc::new(Mutex::new(engine)));
            self.soundfont_cache.release_unused();
            loaded_midi
        };

        let analyzed_strips = loaded_midi
            .channel_program_roles()
            .into_iter()
            .map(|role| MidiStripDescriptor {
                channel: role.channel,
                bank: role.bank,
                program: role.program,
                is_percussion: role.is_percussion,
            })
            .collect::<Vec<_>>();

        self.loaded_midi = Some(source_path);
        self.loaded_midi_bytes = Some(midi_bytes);
        self.midi_transport = MidiTransportMetadata::from_loaded_midi(&loaded_midi);
        if self.loaded_soundfonts.is_empty() {
            self.loaded_soundfonts = soundfont_ids;
        }
        if self.loaded_coverages.is_empty() {
            if let Some(engine) = &self.engine {
                self.loaded_coverages = engine
                    .lock()
                    .map_err(|_| {
                        FlutzError::Runtime("playback engine lock is poisoned".to_owned())
                    })?
                    .soundfont_coverages()
                    .into_iter()
                    .collect();
            }
        }
        *self.midi_strips.lock().map_err(|_| {
            FlutzError::Runtime("MIDI strip analysis lock is poisoned".to_owned())
        })? = analyzed_strips;
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        *self.render_churn.lock().map_err(|_| {
            FlutzError::Runtime("render churn diagnostics lock is poisoned".to_owned())
        })? = RenderChurnDiagnostics::default();
        self.render_scratch
            .lock()
            .map_err(|_| {
                FlutzError::Runtime("playback render scratch lock is poisoned".to_owned())
            })?
            .release_capacity();
        memory_runtime::decay_idle_reuse_preserving(false);
        *self.latest_snapshot.lock().map_err(|_| {
            FlutzError::Runtime("realtime mixer snapshot lock is poisoned".to_owned())
        })? = SnapshotHistory::default();
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        self.finish_render_layer_transition(transition)?;

        let mut summary = format!(
            "Loaded {} byte MIDI with {} RustySynth instance(s), {:.2}s, {} ticks",
            loaded_midi.byte_len,
            self.loaded_soundfonts.len(),
            self.midi_transport.duration_seconds,
            self.midi_transport.tick_length,
        );
        if !self.midi_transport.jump_points.is_empty() {
            summary.push_str("\nJump points (tick): ");
            summary.push_str(
                &self
                    .midi_transport
                    .jump_points
                    .iter()
                    .map(|tick| tick.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !matches!(detected_loop_type, MidiFileLoopType::LoopPoint(_)) {
            summary.push_str("\nAuto-detected loop style: ");
            summary.push_str(loop_type_label(detected_loop_type));
        }
        let interpretation = loaded_midi.midi_interpretation();
        if interpretation.sysex_event_count > 0 {
            summary.push_str("\nMIDI SysEx: ");
            summary.push_str(&format!(
                "{} event(s), {} recognized, percussion channels [{}]",
                interpretation.sysex_event_count,
                interpretation.recognized_sysex_event_count,
                interpretation
                    .percussion_channels
                    .iter()
                    .map(|channel| channel.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(note) = fallback_note {
            summary.push_str("\n");
            summary.push_str(&note);
        }

        self.record_midi_scan_trace();

        Ok(summary)
    }

    pub fn load_decoded_audio_buffer(
        &mut self,
        path: PathBuf,
        format_id: impl Into<String>,
        friendly_name: impl Into<String>,
        buffer: DecodedAudioBuffer,
        peq: Option<PeqPresetFile>,
    ) -> Result<String> {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::PlaybackLoad);
        if buffer.summary.channels == 0 || buffer.summary.sample_rate == 0 {
            return Err(FlutzError::UnsupportedFormat(
                "decoded audio stream has no usable sample rate or channel layout".to_owned(),
            ));
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        self.audio = None;
        self.audio_error = None;
        self.engine = None;
        self.loaded_midi = None;
        self.loaded_midi_bytes = None;
        self.loaded_soundfonts.clear();
        self.loaded_coverages.clear();
        self.loaded_subset_state.clear();
        self.last_subset_plans.clear();
        self.subset_transition = PlaybackSubsetTransitionDiagnostics::default();
        self.midi_transport = MidiTransportMetadata::default();
        if let Ok(mut midi_strips) = self.midi_strips.lock() {
            midi_strips.clear();
        }

        self.audio_config = AudioDeviceConfig {
            sample_rate: buffer.summary.sample_rate,
            channels: 2,
            internal_block_frames: PlaybackConfig::default().block_frames as u16,
            ..AudioDeviceConfig::default()
        };
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            *analyzer = VisualizerAnalyzer::new(VisualizerAnalyzerConfig::with_sample_rate(
                self.audio_config.sample_rate,
            ));
        }

        let metadata = DecodedAudioTransportMetadata {
            path,
            format_id: format_id.into(),
            friendly_name: friendly_name.into(),
            sample_rate: buffer.summary.sample_rate,
            channels: buffer.summary.channels,
            frame_length: buffer.summary.frames_decoded,
            duration_seconds: if buffer.summary.sample_rate == 0 {
                0.0
            } else {
                buffer.summary.frames_decoded as f64 / buffer.summary.sample_rate as f64
            },
            content_kind: ContentKind::DecodedAudio,
            mastering: MasteringCapability::DecodedAudioPeq,
        };
        let summary = format!(
            "Loaded {} decoded audio, {:.2}s, {} Hz, {} channel(s)",
            metadata.friendly_name,
            metadata.duration_seconds,
            metadata.sample_rate,
            metadata.channels,
        );
        self.decoded_audio = Some(Arc::new(Mutex::new(DecodedAudioPlaybackState::new(
            metadata,
            buffer.samples,
            peq,
            PlaybackConfig::default().block_frames,
        )?)));
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        memory_runtime::decay_idle_reuse_preserving(false);
        Ok(summary)
    }

    pub fn load_decoded_audio_stream(
        &mut self,
        path: PathBuf,
        format_id: impl Into<String>,
        friendly_name: impl Into<String>,
        source: DecodedAudioStreamSource,
        peq: Option<PeqPresetFile>,
    ) -> Result<String> {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::PlaybackLoad);
        let format_id = format_id.into();
        let friendly_name = friendly_name.into();
        let session = DecodedAudioStreamSession::open(source, &format_id, Default::default())?;
        let stream_metadata = session.metadata().clone();
        if stream_metadata.channels == 0 || stream_metadata.sample_rate == 0 {
            return Err(FlutzError::UnsupportedFormat(
                "decoded audio stream has no usable sample rate or channel layout".to_owned(),
            ));
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        self.audio = None;
        self.audio_error = None;
        self.engine = None;
        self.loaded_midi = None;
        self.loaded_midi_bytes = None;
        self.loaded_soundfonts.clear();
        self.loaded_coverages.clear();
        self.loaded_subset_state.clear();
        self.last_subset_plans.clear();
        self.subset_transition = PlaybackSubsetTransitionDiagnostics::default();
        self.midi_transport = MidiTransportMetadata::default();
        if let Ok(mut midi_strips) = self.midi_strips.lock() {
            midi_strips.clear();
        }

        self.audio_config = AudioDeviceConfig {
            sample_rate: stream_metadata.sample_rate,
            channels: 2,
            internal_block_frames: PlaybackConfig::default().block_frames as u16,
            ..AudioDeviceConfig::default()
        };
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            *analyzer = VisualizerAnalyzer::new(VisualizerAnalyzerConfig::with_sample_rate(
                self.audio_config.sample_rate,
            ));
        }

        let frame_length = stream_metadata.frame_length.unwrap_or(0);
        let duration_seconds = stream_metadata
            .duration_seconds
            .unwrap_or_else(|| frame_length as f64 / stream_metadata.sample_rate.max(1) as f64);
        let metadata = DecodedAudioTransportMetadata {
            path,
            format_id,
            friendly_name,
            sample_rate: stream_metadata.sample_rate,
            channels: stream_metadata.channels,
            frame_length,
            duration_seconds,
            content_kind: ContentKind::DecodedAudio,
            mastering: MasteringCapability::DecodedAudioPeq,
        };
        let summary = format!(
            "Loaded {} decoded audio stream, {:.2}s, {} Hz, {} channel(s)",
            metadata.friendly_name,
            metadata.duration_seconds,
            metadata.sample_rate,
            metadata.channels,
        );
        self.decoded_audio = Some(Arc::new(Mutex::new(
            DecodedAudioPlaybackState::new_streaming(
                metadata,
                session,
                peq,
                PlaybackConfig::default().block_frames,
            )?,
        )));
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        memory_runtime::decay_idle_reuse_preserving(false);
        Ok(summary)
    }

    fn soundfont_demand_diagnostics(
        &self,
        requested_soundfonts: &[String],
        resolved_soundfonts: &[String],
        demand_profile: &MidiDemandProfile,
    ) -> SoundFontDemandDiagnostics {
        let available = self
            .catalog
            .iter()
            .map(|font| font.internal_id.as_str())
            .collect::<BTreeSet<_>>();
        let requested_available_count = requested_soundfonts
            .iter()
            .filter(|id| available.contains(id.as_str()))
            .count();
        let requested_soundfont_count = if requested_soundfonts.is_empty() {
            resolved_soundfonts.len()
        } else {
            requested_available_count
        };
        let resolved_requested_count = resolved_soundfonts
            .iter()
            .filter(|id| requested_soundfonts.is_empty() || requested_soundfonts.contains(id))
            .count();

        SoundFontDemandDiagnostics {
            requested_soundfont_count,
            loaded_provider_count: resolved_soundfonts.len(),
            pruned_soundfont_count: requested_soundfont_count
                .saturating_sub(resolved_requested_count),
            demand_profile: demand_profile.clone(),
        }
    }

    pub fn play(&mut self) -> Result<AudioPlaybackStatus> {
        if let Some(decoded) = self.decoded_audio.as_ref().map(Arc::clone) {
            {
                let mut decoded = decoded.lock().map_err(|_| {
                    FlutzError::Runtime("decoded audio lock is poisoned".to_owned())
                })?;
                decoded.play();
            }
            if let Err(error) = self.ensure_audio_stream() {
                let message = format!("audio unavailable: {error}");
                self.audio_error = Some(message.clone());
                if let Ok(mut decoded) = decoded.lock() {
                    decoded.pause();
                }
                return Ok(AudioPlaybackStatus::AudioUnavailable(message));
            }
            if let Some(audio) = &mut self.audio {
                if let Err(error) = audio.resume() {
                    self.audio = None;
                    let message = format!("audio resume failed: {error}");
                    self.audio_error = Some(message.clone());
                    if let Ok(mut decoded) = decoded.lock() {
                        decoded.pause();
                    }
                    return Ok(AudioPlaybackStatus::AudioUnavailable(message));
                }
            }
            self.update_latency_control();
            self.audio_error = None;
            return Ok(AudioPlaybackStatus::Audible);
        }
        let engine = self.engine.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load a MIDI file before playing".to_owned())
        })?;
        {
            let mut engine = engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?;
            if engine.state() == PlaybackState::Paused {
                engine.resume();
            } else {
                engine.play()?;
            }
        }
        if let Err(error) = self.ensure_audio_stream() {
            let message = format!("audio unavailable: {error}");
            self.audio_error = Some(message.clone());
            if let Some(engine) = &self.engine {
                if let Ok(mut engine) = engine.lock() {
                    engine.pause();
                }
            }
            return Ok(AudioPlaybackStatus::AudioUnavailable(message));
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.resume() {
                self.audio = None;
                let message = format!("audio resume failed: {error}");
                self.audio_error = Some(message.clone());
                if let Some(engine) = &self.engine {
                    if let Ok(mut engine) = engine.lock() {
                        engine.pause();
                    }
                }
                return Ok(AudioPlaybackStatus::AudioUnavailable(message));
            }
        }
        self.update_latency_control();
        self.audio_error = None;
        Ok(AudioPlaybackStatus::Audible)
    }

    pub fn update_latency_control(&mut self) {
        let Some(audio) = self.audio.as_ref() else {
            let _ = self
                .flux_guard
                .lock()
                .map(|mut guard| guard.observe(None, self.audio_config.sample_rate));
            return;
        };
        let state = audio.render_ahead_state();
        let snapshot = self
            .flux_guard
            .lock()
            .map(|mut guard| guard.observe(Some(state), self.audio_config.sample_rate))
            .unwrap_or_default();
        if !snapshot.decision.changed {
            return;
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.apply_render_ahead_target(snapshot.decision.target) {
                self.audio_error = Some(format!("audio render-ahead target failed: {error}"));
            }
        }
    }

    pub fn render_probe(&mut self, frames: usize) -> Result<RenderProbeReport> {
        let reports = self.render_probe_sequence(&[frames], true)?;
        reports
            .into_iter()
            .next()
            .ok_or_else(|| FlutzError::Runtime("render probe produced no report".to_owned()))
    }

    pub fn render_probe_sequence(
        &mut self,
        frame_sequence: &[usize],
        stop_after: bool,
    ) -> Result<Vec<RenderProbeReport>> {
        let reports = self.render_probe_sequence_with_stems(frame_sequence, stop_after)?;
        Ok(reports.into_iter().map(|report| report.report).collect())
    }

    pub fn render_probe_sequence_with_stems(
        &mut self,
        frame_sequence: &[usize],
        stop_after: bool,
    ) -> Result<Vec<RenderStemProbeReport>> {
        let engine = self.engine.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load a MIDI file before rendering".to_owned())
        })?;
        let max_frames = frame_sequence.iter().copied().max().unwrap_or(0);
        let mut output = vec![0.0f32; max_frames.saturating_mul(2)];
        let mut reports = Vec::with_capacity(frame_sequence.len());
        {
            let controls = self.clone_mixer_controls_for_render("render_probe_sequence");
            let output_gain = self
                .session_output_gain
                .lock()
                .map(|gain| *gain)
                .unwrap_or(1.0);
            let mut mixer = self
                .mixer
                .lock()
                .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?;
            let mut engine = engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?;
            let midi_strips = self.midi_strips.lock().map_err(|_| {
                FlutzError::Runtime("MIDI strip analysis lock is poisoned".to_owned())
            })?;
            let mut scratch = self.render_scratch.lock().map_err(|_| {
                FlutzError::Runtime("playback render scratch lock is poisoned".to_owned())
            })?;
            engine.play()?;
            for frames in frame_sequence.iter().copied() {
                let samples = frames.saturating_mul(2);
                if output.len() < samples {
                    output.resize(samples, 0.0);
                }
                let output = &mut output[..samples];
                let report = render_mixed_audio_guarded(
                    &mut engine,
                    &mut mixer,
                    &mut scratch,
                    &controls,
                    output_gain,
                    &self.loaded_soundfonts,
                    &midi_strips,
                    output,
                    self.render_error_trace.as_ref(),
                    RenderFailureContext {
                        source: "probe-render",
                        frames_requested: frames,
                        midi_source: self.loaded_midi.as_deref(),
                        midi_scan: &self.last_midi_scan,
                    },
                );
                let recovered_error_count = usize::from(report.is_none());
                let peak = report
                    .as_ref()
                    .map(|report| report.output_meter.peak)
                    .unwrap_or(0.0);
                let stems = scratch.blocks.clone();
                let mix_diagnostics = report
                    .as_ref()
                    .map(|report| report.strip_mix_diagnostics.clone())
                    .unwrap_or_default();
                if let Some(report) = report {
                    if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
                        snapshot_history.record(
                            self.rendered_frame_clock
                                .load(Ordering::Relaxed)
                                .saturating_add(report.frames as u64),
                            report.snapshot.clone(),
                            engine.sample_rate(),
                        );
                    }
                    if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
                        let frame = analyzer.ingest_interleaved_stereo(
                            output,
                            report.frames as f32 / self.audio_config.sample_rate.max(1) as f32,
                        );
                        self.record_analyzer_trace(&frame);
                    }
                    self.rendered_frame_clock
                        .fetch_add(report.frames as u64, Ordering::Relaxed);
                } else if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
                    let frame = analyzer.advance_silence(
                        output.len() / 2,
                        (output.len() / 2) as f32 / self.audio_config.sample_rate.max(1) as f32,
                    );
                    self.record_analyzer_trace(&frame);
                }
                reports.push(RenderStemProbeReport {
                    report: RenderProbeReport {
                        frames,
                        samples: output.len(),
                        peak,
                        soundfont_count: self.loaded_soundfonts.len(),
                        midi_strip_count: midi_strips.len(),
                        recovered_error_count,
                    },
                    stems,
                    mix_diagnostics,
                });
            }
            if stop_after {
                engine.stop();
                mixer.reset_state_and_release_scratch();
                memory_runtime::decay_idle_reuse_preserving(false);
            }
        }
        Ok(reports)
    }

    pub fn pause(&mut self) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .pause();
        }
        if let Some(engine) = &self.engine {
            engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
                .pause();
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio = None;
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        memory_runtime::decay_idle_reuse_preserving(false);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .stop();
        }
        if let Some(engine) = &self.engine {
            engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
                .stop();
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio = None;
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        memory_runtime::decay_idle_reuse_preserving(false);
        Ok(())
    }

    pub fn release_idle_resources(&mut self) -> Result<()> {
        self.stop()?;
        self.audio = None;
        self.audio_error = None;
        self.engine = None;
        self.decoded_audio = None;
        self.loaded_midi = None;
        self.loaded_midi_bytes = None;
        self.loaded_soundfonts.clear();
        self.loaded_coverages.clear();
        self.loaded_subset_state.clear();
        self.last_subset_plans.clear();
        self.subset_transition = PlaybackSubsetTransitionDiagnostics::default();
        self.midi_transport = MidiTransportMetadata::default();
        if let Ok(mut midi_strips) = self.midi_strips.lock() {
            midi_strips.clear();
        }
        self.soundfont_cache.clear();
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.render_scratch
            .lock()
            .map_err(|_| {
                FlutzError::Runtime("playback render scratch lock is poisoned".to_owned())
            })?
            .release_capacity();
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        memory_runtime::aggressive_release();
        Ok(())
    }

    pub fn retry_audio(&mut self) -> Result<()> {
        self.audio = None;
        self.audio_error = None;
        self.reset_flux_guard();
        self.ensure_audio_stream()?;
        Ok(())
    }

    pub fn status_line(&self) -> String {
        if let Some(decoded) = &self.decoded_audio {
            return decoded
                .lock()
                .map(|decoded| {
                    format!(
                        "{}, {} Hz, {} channel(s), {}",
                        decoded.metadata.friendly_name,
                        decoded.metadata.sample_rate,
                        decoded.metadata.channels,
                        self.audio_status_text()
                    )
                })
                .unwrap_or_else(|_| "decoded audio unavailable".to_owned());
        }
        let Some(engine) = &self.engine else {
            return "no MIDI loaded".to_owned();
        };
        match engine.lock() {
            Ok(engine) => format!(
                "{} synth(s), {} Hz, {}",
                engine.instance_count(),
                engine.sample_rate(),
                self.audio_status_text()
            ),
            Err(_) => "playback engine unavailable".to_owned(),
        }
    }

    pub fn audio_status_text(&self) -> String {
        if let Some(error) = &self.audio_error {
            format!("audio unavailable ({error})")
        } else if let Some(audio) = &self.audio {
            let diagnostics = audio.diagnostics();
            let device = diagnostics
                .opened_device_name
                .as_deref()
                .unwrap_or("default playback");
            format!("audio ready via {} ({device})", audio.backend_name())
        } else {
            "audio closed".to_owned()
        }
    }

    pub fn debug_metrics(&self) -> PlaybackDebugMetrics {
        let decoded_transport = self.decoded_audio.as_ref().and_then(|decoded| {
            decoded.lock().ok().map(|decoded| {
                (
                    decoded.state_label().to_owned(),
                    decoded.position_seconds(),
                    decoded.position_frame,
                    decoded.metadata.duration_seconds,
                )
            })
        });
        let (engine_state, transport_seconds, transport_tick, memory_debug) = self
            .engine
            .as_ref()
            .and_then(|engine| {
                engine.lock().ok().map(|engine| {
                    (
                        format!("{:?}", engine.state()),
                        engine.position_seconds(),
                        engine.position_ticks(),
                        Some(engine.memory_debug()),
                    )
                })
            })
            .unwrap_or_else(|| {
                decoded_transport
                    .as_ref()
                    .map(|(state, seconds, frame, _duration)| {
                        (state.clone(), *seconds, *frame, None)
                    })
                    .unwrap_or_else(|| ("unloaded".to_owned(), 0.0, 0, None))
            });
        let audio_diagnostics = self.audio.as_ref().map(AudioOutput::diagnostics);
        let flux_guard = self.flux_guard_snapshot();
        let audible_output = self.audible_output_view();
        let live_output = self.live_output_view();
        let snapshot = &audible_output.snapshot;
        let component_memory =
            self.component_memory_diagnostics(memory_debug.as_ref(), audio_diagnostics.as_ref());
        PlaybackDebugMetrics {
            engine_state,
            transport_seconds,
            transport_duration_seconds: decoded_transport
                .as_ref()
                .map(|(_, _, _, duration)| *duration)
                .unwrap_or(self.midi_transport.duration_seconds),
            transport_tick,
            loaded_soundfont_count: self.loaded_soundfonts.len(),
            requested_soundfont_count: self.last_soundfont_demand.requested_soundfont_count,
            loaded_provider_count: self.last_soundfont_demand.loaded_provider_count,
            pruned_soundfont_count: self.last_soundfont_demand.pruned_soundfont_count,
            loaded_midi_bytes: self
                .loaded_midi_bytes
                .as_ref()
                .map(Vec::len)
                .unwrap_or_default(),
            loaded_midi_capacity_bytes: self
                .loaded_midi_bytes
                .as_ref()
                .map(Vec::capacity)
                .unwrap_or_default(),
            midi_strip_count: self
                .midi_strips
                .lock()
                .map(|strips| strips.len())
                .unwrap_or_default(),
            midi_demand: self.last_demand_profile.clone(),
            subset_plans: SoundFontSubsetPlanAggregateDebug::from_plans(&self.last_subset_plans),
            loaded_subset_state: self.loaded_subset_state.clone(),
            subset_transition: self.subset_transition.clone(),
            soundfont_load: self.last_soundfont_load.clone(),
            output_peak: snapshot.output_meter.peak,
            output_rms: snapshot.output_meter.rms,
            meter_latency_frames: audible_output.effective_latency_frames,
            meter_latency_ms: audible_output.effective_latency_ms,
            meter_wrapper_queue_frames: audible_output.wrapper_queue_frames,
            meter_device_queue_frames: audible_output.device_queue_frames,
            live_frame_clock: live_output.rendered_frame_clock,
            audible_frame_clock: audible_output.rendered_frame_clock,
            active_strip_count: snapshot
                .strips
                .values()
                .filter(|strip| strip.audible || !strip.active_notes.is_empty())
                .count(),
            audio_status: self.audio_status_text(),
            audio_error: self.audio_error.clone(),
            audio_backend: self.audio_backend.name(),
            audio_config: self.audio_config,
            audio_diagnostics,
            flux_guard,
            memory_debug,
            render_churn: self
                .render_churn
                .lock()
                .map(|diagnostics| diagnostics.clone())
                .unwrap_or_default(),
            lifecycle: self.lifecycle,
            soundfont_cache: self.soundfont_cache.debug(),
            component_memory,
            references: self.reference_diagnostics(),
            visualizer: VisualizerDebugMetrics::from_frame(&self.visualizer_frame()),
        }
    }

    fn reference_diagnostics(&self) -> PlaybackReferenceDiagnostics {
        PlaybackReferenceDiagnostics {
            engine: self.engine.as_ref().map(|engine| {
                ArcReferenceDiagnostics::from_arc(
                    engine,
                    expected_engine_roots(self.audio.is_some()),
                )
            }),
            mixer: ArcReferenceDiagnostics::from_arc(
                &self.mixer,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            mixer_controls: ArcReferenceDiagnostics::from_arc(
                &self.mixer_controls,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            latest_snapshot: ArcReferenceDiagnostics::from_arc(
                &self.latest_snapshot,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            midi_strips: ArcReferenceDiagnostics::from_arc(
                &self.midi_strips,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            render_scratch: ArcReferenceDiagnostics::from_arc(
                &self.render_scratch,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            render_churn: ArcReferenceDiagnostics::from_arc(
                &self.render_churn,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            audio_stream_open: self.audio.is_some(),
        }
    }

    fn flux_guard_snapshot(&self) -> FluxGuardSnapshot {
        self.flux_guard
            .lock()
            .map(|guard| guard.snapshot())
            .unwrap_or_default()
    }

    fn reset_flux_guard(&self) {
        if let Ok(mut guard) = self.flux_guard.lock() {
            guard.reset();
        }
    }

    fn engine_state(&self) -> Option<PlaybackState> {
        self.engine
            .as_ref()
            .and_then(|engine| engine.lock().ok().map(|engine| engine.state()))
    }

    pub fn playback_active(&self) -> bool {
        if self.engine_state() == Some(PlaybackState::Playing) {
            return true;
        }
        self.decoded_audio
            .as_ref()
            .and_then(|decoded| decoded.lock().ok().map(|decoded| decoded.is_playing()))
            .unwrap_or(false)
    }

    fn capture_render_layer_transition(
        &self,
        midi_bytes: &[u8],
        resolved_soundfonts: &[String],
    ) -> RenderLayerTransition {
        let same_midi = self
            .loaded_midi_bytes
            .as_deref()
            .is_some_and(|loaded| loaded == midi_bytes);
        let soundfont_set_changed = self.loaded_soundfonts != resolved_soundfonts;
        let Some(engine) = &self.engine else {
            return RenderLayerTransition::default();
        };
        if !same_midi || !soundfont_set_changed {
            return RenderLayerTransition::default();
        }
        match engine.lock() {
            Ok(engine) => RenderLayerTransition {
                warmup_tick: Some(engine.position_ticks()),
            },
            Err(_) => RenderLayerTransition::default(),
        }
    }

    fn finish_render_layer_transition(&mut self, transition: RenderLayerTransition) -> Result<()> {
        let Some(warmup_tick) = transition.warmup_tick else {
            return Ok(());
        };
        if let Some(engine) = &self.engine {
            engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
                .seek_to_tick(warmup_tick)?;
        }
        self.reset_audible_output_after_reposition(false);
        Ok(())
    }

    fn reset_audible_output_after_reposition(&mut self, resume_audio: bool) {
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio = None;
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.reset_state_and_release_scratch();
        }
        memory_runtime::decay_idle_reuse_preserving(!matches!(
            self.engine_state(),
            Some(PlaybackState::Stopped) | None
        ));
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        if resume_audio {
            if let Some(audio) = &mut self.audio {
                if let Err(error) = audio.resume() {
                    self.audio = None;
                    self.audio_error = Some(format!("audio resume failed: {error}"));
                }
            }
        }
    }

    fn component_memory_diagnostics(
        &self,
        memory_debug: Option<&PlaybackMemoryDebug>,
        audio_diagnostics: Option<&AudioDeviceDiagnostics>,
    ) -> PlaybackComponentMemoryDiagnostics {
        let soundfont_metadata = self.soundfont_metadata_diagnostics();
        let midi = self.midi_runtime_diagnostics(memory_debug);
        let rustystem = RustystemRuntimeDiagnostics::from_memory_debug(memory_debug);
        let audio = AudioRuntimeMemoryDiagnostics::from_audio_diagnostics(audio_diagnostics);
        let tracked_total_bytes = soundfont_metadata
            .estimated_bytes
            .saturating_add(midi.estimated_bytes)
            .saturating_add(rustystem.estimated_bytes)
            .saturating_add(audio.estimated_bytes);
        PlaybackComponentMemoryDiagnostics {
            soundfont_metadata,
            midi,
            rustystem,
            audio,
            tracked_total_bytes,
        }
    }

    fn soundfont_metadata_diagnostics(&self) -> SoundFontMetadataDiagnostics {
        let catalog_vec_bytes = self.catalog.capacity() * mem::size_of::<SoundFontCatalogEntry>();
        let catalog_string_bytes = self
            .catalog
            .iter()
            .map(soundfont_catalog_entry_string_bytes)
            .sum::<usize>();
        let loaded_soundfont_id_bytes = self
            .loaded_soundfonts
            .capacity()
            .saturating_mul(mem::size_of::<String>())
            .saturating_add(
                self.loaded_soundfonts
                    .iter()
                    .map(String::capacity)
                    .sum::<usize>(),
            );
        let loaded_coverage_estimated_bytes = self
            .loaded_coverages
            .iter()
            .map(|(id, coverage)| {
                id.capacity()
                    .saturating_add(coverage_estimated_bytes(coverage))
            })
            .sum::<usize>()
            .saturating_add(
                self.loaded_coverages
                    .len()
                    .saturating_mul(mem::size_of::<(String, SoundFontCoverage)>()),
            );
        let estimated_bytes = catalog_vec_bytes
            .saturating_add(catalog_string_bytes)
            .saturating_add(loaded_soundfont_id_bytes)
            .saturating_add(loaded_coverage_estimated_bytes);
        SoundFontMetadataDiagnostics {
            catalog_entries: self.catalog.len(),
            catalog_estimated_bytes: catalog_vec_bytes.saturating_add(catalog_string_bytes),
            loaded_soundfont_ids: self.loaded_soundfonts.len(),
            loaded_soundfont_id_bytes,
            loaded_coverage_entries: self.loaded_coverages.len(),
            loaded_coverage_estimated_bytes,
            estimated_bytes,
        }
    }

    fn midi_runtime_diagnostics(
        &self,
        memory_debug: Option<&PlaybackMemoryDebug>,
    ) -> MidiRuntimeMemoryDiagnostics {
        let strip_capacity = self
            .midi_strips
            .lock()
            .map(|strips| strips.capacity())
            .unwrap_or_default();
        let strip_count = self
            .midi_strips
            .lock()
            .map(|strips| strips.len())
            .unwrap_or_default();
        let strip_bytes = strip_capacity * mem::size_of::<MidiStripDescriptor>();
        let jump_point_bytes = self.midi_transport.jump_points.capacity() * mem::size_of::<u64>();
        let parsed = memory_debug.and_then(|memory| memory.midi_file);
        let parsed_estimated_bytes = parsed
            .map(|debug| debug.estimated_bytes)
            .unwrap_or_default();
        let raw_bytes = self
            .loaded_midi_bytes
            .as_ref()
            .map(Vec::len)
            .unwrap_or_default();
        let raw_capacity_bytes = self
            .loaded_midi_bytes
            .as_ref()
            .map(Vec::capacity)
            .unwrap_or_default();
        let estimated_bytes = raw_capacity_bytes
            .saturating_add(strip_bytes)
            .saturating_add(jump_point_bytes)
            .saturating_add(parsed_estimated_bytes);
        MidiRuntimeMemoryDiagnostics {
            raw_bytes,
            raw_capacity_bytes,
            strip_count,
            strip_capacity,
            strip_bytes,
            jump_point_count: self.midi_transport.jump_points.len(),
            jump_point_bytes,
            parsed_message_count: parsed.map(|debug| debug.message_count).unwrap_or_default(),
            parsed_sysex_events: parsed.map(|debug| debug.sysex_events).unwrap_or_default(),
            parsed_sysex_bytes: parsed.map(|debug| debug.sysex_bytes).unwrap_or_default(),
            parsed_estimated_bytes,
            estimated_bytes,
        }
    }

    pub fn transport_fraction(&self) -> f32 {
        if let Some(decoded) = &self.decoded_audio {
            return decoded
                .lock()
                .map(|decoded| decoded.transport_fraction())
                .unwrap_or(0.0);
        }
        let Some(engine) = &self.engine else {
            return 0.0;
        };
        if self.midi_transport.duration_seconds <= 0.0 {
            return 0.0;
        }
        match engine.lock() {
            Ok(engine) => (engine.position_seconds() / self.midi_transport.duration_seconds)
                .clamp(0.0, 1.0) as f32,
            Err(_) => 0.0,
        }
    }

    pub fn transport_tick(&self) -> u64 {
        if let Some(decoded) = &self.decoded_audio {
            return decoded
                .lock()
                .map(|decoded| decoded.position_frame)
                .unwrap_or(0);
        }
        let Some(engine) = &self.engine else {
            return 0;
        };
        match engine.lock() {
            Ok(engine) => engine.position_ticks(),
            Err(_) => 0,
        }
    }

    pub fn seek_transport_fraction(&mut self, fraction: f32) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            let was_playing = self.playback_active();
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .seek_fraction(fraction);
            self.reset_audible_output_after_reposition(was_playing);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        let was_playing = self.engine_state() == Some(PlaybackState::Playing);
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .seek_to_fraction(fraction as f64)?;
        self.reset_audible_output_after_reposition(was_playing);
        Ok(())
    }

    pub fn seek_transport_tick(&mut self, tick: u64) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            let was_playing = self.playback_active();
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .seek_frame(tick);
            self.reset_audible_output_after_reposition(was_playing);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        let was_playing = self.engine_state() == Some(PlaybackState::Playing);
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .seek_to_tick(tick)?;
        self.reset_audible_output_after_reposition(was_playing);
        Ok(())
    }

    pub fn seek_transport_seconds(&mut self, seconds: f64) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            let was_playing = self.playback_active();
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .seek_seconds(seconds);
            self.reset_audible_output_after_reposition(was_playing);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        let was_playing = self.engine_state() == Some(PlaybackState::Playing);
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .seek_to_seconds(seconds)?;
        self.reset_audible_output_after_reposition(was_playing);
        Ok(())
    }

    pub fn set_loop_enabled(&mut self, enabled: bool) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .set_loop_enabled(enabled);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .set_loop_enabled(enabled);
        Ok(())
    }

    pub fn set_loop_settings(&mut self, settings: PlaybackLoopSettings) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .set_loop_settings(settings);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .set_loop_settings(settings);
        Ok(())
    }

    pub fn midi_transport_metadata(&self) -> MidiTransportMetadata {
        self.midi_transport.clone()
    }

    pub fn decoded_transport_metadata(&self) -> Option<DecodedAudioTransportMetadata> {
        self.decoded_audio
            .as_ref()
            .and_then(|decoded| decoded.lock().ok().map(|decoded| decoded.metadata.clone()))
    }

    pub fn decoded_stream_cache_debug(&self) -> Option<DecodedStreamCacheDebug> {
        self.decoded_audio.as_ref().and_then(|decoded| {
            decoded
                .lock()
                .ok()
                .and_then(|decoded| decoded.stream_cache_debug())
        })
    }

    pub fn wait_for_decoded_stream_cache(&self, timeout: Duration) -> bool {
        self.decoded_audio
            .as_ref()
            .and_then(|decoded| {
                decoded
                    .lock()
                    .ok()
                    .map(|decoded| decoded.wait_for_stream_cache(timeout))
            })
            .unwrap_or(false)
    }

    pub fn set_decoded_peq_config(&mut self, config: PeqConfig) -> Result<u64> {
        let prepared = PeqProcessor::prepare_config(config).map_err(|error| {
            FlutzError::InvalidInput(format!("invalid decoded audio PEQ config: {error}"))
        })?;
        let decoded = self.decoded_audio.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load decoded audio before editing PEQ".to_owned())
        })?;
        decoded
            .lock()
            .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
            .set_peq_prepared_config(prepared)
    }

    pub fn decoded_render_probe(&mut self, frames: usize) -> Result<DecodedRenderProbeReport> {
        let decoded = self.decoded_audio.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load decoded audio before probing render".to_owned())
        })?;
        let mut output = vec![0.0f32; frames.saturating_mul(2)];
        let output_gain = self
            .session_output_gain
            .lock()
            .map(|gain| *gain)
            .unwrap_or(1.0);
        let mut decoded = decoded
            .lock()
            .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?;
        decoded.play();
        let retained_before = decoded.retained_scratch_bytes();
        let meter = render_decoded_audio_stereo(&mut decoded, &mut output, output_gain)?;
        let retained_after = decoded.retained_scratch_bytes();
        let peq_generation = decoded.peq_generation();
        drop(decoded);
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            let frame = analyzer.ingest_interleaved_stereo(
                &output,
                frames as f32 / self.audio_config.sample_rate.max(1) as f32,
            );
            self.record_analyzer_trace(&frame);
        }
        Ok(DecodedRenderProbeReport {
            frames,
            samples: output.len(),
            peak: meter.peak,
            rms: meter.rms,
            peq_generation,
            scratch_growth_bytes: retained_after.saturating_sub(retained_before),
        })
    }

    pub fn loaded_midi(&self) -> Option<&Path> {
        self.loaded_midi.as_deref()
    }

    pub fn loaded_midi_bytes(&self) -> Option<&[u8]> {
        self.loaded_midi_bytes.as_deref()
    }

    pub fn loaded_soundfont_ids(&self) -> Vec<String> {
        self.loaded_soundfonts.clone()
    }

    pub fn last_soundfont_subset_plans(&self) -> BTreeMap<String, SoundFontSubsetPlanDebug> {
        self.last_subset_plans.clone()
    }

    pub fn loaded_soundfont_coverages(&self) -> BTreeMap<String, SoundFontCoverage> {
        self.loaded_coverages.clone()
    }

    /// Returns the channel/bank/program/is_percussion layout discovered during MIDI analysis.
    /// This is the source of truth for how many strips each soundfont row should show.
    pub fn midi_strip_layout(&self) -> Vec<(u8, u16, u8, bool)> {
        self.midi_strips
            .lock()
            .map(|strips| {
                strips
                    .iter()
                    .map(|s| (s.channel, s.bank, s.program, s.is_percussion))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn ensure_audio_stream(&mut self) -> Result<()> {
        if self.audio.is_some() {
            return Ok(());
        }
        if let Some(decoded) = &self.decoded_audio {
            let audio_decoded = Arc::clone(decoded);
            let audio_session_output_gain = Arc::clone(&self.session_output_gain);
            let audio_snapshot = Arc::clone(&self.latest_snapshot);
            let audio_visualizer = Arc::clone(&self.visualizer_analyzer);
            let audio_analyzer_trace = self.analyzer_trace.as_ref().map(Arc::clone);
            let audio_frame_clock = Arc::clone(&self.rendered_frame_clock);
            let audio_backend = self.audio_backend;
            let audio_sample_rate = self.audio_config.sample_rate;
            let audio =
                AudioOutput::open_f32_stream(audio_backend, self.audio_config, move |output| {
                    let _allocation_scope =
                        AllocationScopeGuard::enter(AllocationScope::AudioCallback);
                    let output_gain = audio_session_output_gain
                        .lock()
                        .map(|gain| *gain)
                        .unwrap_or(1.0);
                    let meter = match audio_decoded.lock() {
                        Ok(mut decoded) => {
                            render_decoded_audio_stereo(&mut decoded, output, output_gain)
                                .unwrap_or_else(|_| {
                                    output.fill(0.0);
                                    MeterReading::default()
                                })
                        }
                        Err(_) => {
                            output.fill(0.0);
                            MeterReading::default()
                        }
                    };
                    let rendered_frames = output.len() / 2;
                    let rendered_frame_end = audio_frame_clock
                        .fetch_add(rendered_frames as u64, Ordering::Relaxed)
                        .saturating_add(rendered_frames as u64);
                    if let Ok(mut snapshot_history) = audio_snapshot.lock() {
                        snapshot_history.record(
                            rendered_frame_end,
                            RealtimeMixerSnapshot {
                                output_meter: meter,
                                strips: BTreeMap::new(),
                            },
                            audio_sample_rate,
                        );
                    }
                    if let Ok(mut analyzer) = audio_visualizer.lock() {
                        let frame = analyzer.ingest_interleaved_stereo(
                            output,
                            rendered_frames as f32 / audio_sample_rate.max(1) as f32,
                        );
                        record_analyzer_trace_arc(audio_analyzer_trace.as_ref(), &frame);
                    }
                })
                .map_err(FlutzError::Runtime)?;
            self.audio = Some(audio);
            self.audio_error = None;
            return Ok(());
        }
        let engine = self.engine.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load a MIDI file before opening audio".to_owned())
        })?;
        let audio_engine = Arc::clone(engine);
        let audio_mixer = Arc::clone(&self.mixer);
        let audio_controls = Arc::clone(&self.mixer_controls);
        let audio_session_output_gain = Arc::clone(&self.session_output_gain);
        let audio_snapshot = Arc::clone(&self.latest_snapshot);
        let audio_visualizer = Arc::clone(&self.visualizer_analyzer);
        let audio_analyzer_trace = self.analyzer_trace.as_ref().map(Arc::clone);
        let audio_render_error_trace = self.render_error_trace.as_ref().map(Arc::clone);
        let audio_frame_clock = Arc::clone(&self.rendered_frame_clock);
        let audio_midi_strips = Arc::clone(&self.midi_strips);
        let audio_render_scratch = Arc::clone(&self.render_scratch);
        let audio_soundfont_ids = self.loaded_soundfonts.clone();
        let audio_render_churn = Arc::clone(&self.render_churn);
        let audio_backend = self.audio_backend;
        let audio_sample_rate = self.audio_config.sample_rate;
        let audio_midi_source = self.loaded_midi.clone();
        let audio_midi_scan = self.last_midi_scan.clone();
        let audio = AudioOutput::open_f32_stream(audio_backend, self.audio_config, move |output| {
            let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::AudioCallback);
            let Ok(mut engine) = audio_engine.lock() else {
                output.fill(0.0);
                return;
            };
            let Ok(mut mixer) = audio_mixer.lock() else {
                output.fill(0.0);
                return;
            };
            let Ok(midi_strips) = audio_midi_strips.lock() else {
                output.fill(0.0);
                return;
            };
            let controls = clone_mixer_controls_with_recovery(
                &audio_controls,
                audio_render_error_trace.as_ref(),
                "audio-callback",
                audio_midi_source.as_deref(),
            );
            let Ok(mut scratch) = audio_render_scratch.lock() else {
                output.fill(0.0);
                return;
            };
            let output_gain = audio_session_output_gain
                .lock()
                .map(|gain| *gain)
                .unwrap_or(1.0);
            match render_mixed_audio_guarded(
                &mut engine,
                &mut mixer,
                &mut scratch,
                &controls,
                output_gain,
                &audio_soundfont_ids,
                &midi_strips,
                output,
                audio_render_error_trace.as_ref(),
                RenderFailureContext {
                    source: "audio-callback",
                    frames_requested: output.len() / 2,
                    midi_source: audio_midi_source.as_deref(),
                    midi_scan: &audio_midi_scan,
                },
            ) {
                Some(report) => {
                    let rendered_frame_end = audio_frame_clock
                        .fetch_add(report.frames as u64, Ordering::Relaxed)
                        .saturating_add(report.frames as u64);
                    if let Ok(mut snapshot_history) = audio_snapshot.lock() {
                        snapshot_history.record(
                            rendered_frame_end,
                            report.snapshot,
                            audio_sample_rate,
                        );
                    }
                    if let Ok(mut analyzer) = audio_visualizer.lock() {
                        let frame = analyzer.ingest_interleaved_stereo(
                            output,
                            report.frames as f32 / audio_sample_rate.max(1) as f32,
                        );
                        record_analyzer_trace_arc(audio_analyzer_trace.as_ref(), &frame);
                    }
                    if let Ok(mut diagnostics) = audio_render_churn.lock() {
                        diagnostics.record(report.churn);
                    }
                }
                None => {
                    output.fill(0.0);
                    if let Ok(mut analyzer) = audio_visualizer.lock() {
                        let frame = analyzer.advance_silence(
                            output.len() / 2,
                            (output.len() / 2) as f32 / audio_sample_rate.max(1) as f32,
                        );
                        record_analyzer_trace_arc(audio_analyzer_trace.as_ref(), &frame);
                    }
                }
            }
        })
        .map_err(FlutzError::Runtime)?;
        self.audio = Some(audio);
        self.audio_error = None;
        Ok(())
    }

    fn resolve_soundfont_ids(
        &self,
        requested_soundfonts: &[String],
        channel_roles: &[MidiStripDescriptor],
    ) -> Result<Vec<String>> {
        let available = self
            .catalog
            .iter()
            .map(|font| font.internal_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut ids = requested_soundfonts
            .iter()
            .filter(|id| available.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();

        if !ids.is_empty() {
            ids = self.prune_soundfont_ids_by_coverage(ids, channel_roles);
        }

        if ids.is_empty() {
            if let Some(default) = self.catalog.iter().find(|font| font.is_default) {
                ids.push(default.internal_id.clone());
            } else if let Some(first) = self.catalog.first() {
                ids.push(first.internal_id.clone());
            }
        }

        if ids.is_empty() {
            return Err(FlutzError::InvalidInput(
                "no soundfonts are available in the DAT data folder".to_owned(),
            ));
        }

        Ok(ids)
    }

    fn prune_soundfont_ids_by_coverage(
        &self,
        soundfont_ids: Vec<String>,
        channel_roles: &[MidiStripDescriptor],
    ) -> Vec<String> {
        if channel_roles.is_empty() {
            return soundfont_ids;
        }
        let coverage_by_id = self
            .catalog
            .iter()
            .map(|font| (font.internal_id.as_str(), font.coverage.as_ref()))
            .collect::<BTreeMap<_, _>>();
        let pruned = soundfont_ids
            .iter()
            .filter(|soundfont_id| {
                let Some(Some(coverage)) = coverage_by_id.get(soundfont_id.as_str()) else {
                    return true;
                };
                channel_roles
                    .iter()
                    .any(|role| coverage_supports_midi_role(coverage, role))
            })
            .cloned()
            .collect::<Vec<_>>();
        if pruned.is_empty() {
            soundfont_ids
        } else {
            pruned
        }
    }

    fn load_soundfonts_for_playback(
        &mut self,
        soundfont_ids: &[String],
        channel_roles: &[MidiStripDescriptor],
    ) -> Result<Vec<LoadedSoundFont>> {
        let load_started = Instant::now();
        let load_invocation = self.last_soundfont_load.load_invocations.saturating_add(1);
        let wanted = soundfont_ids.iter().cloned().collect::<BTreeSet<_>>();
        let catalog = self
            .catalog
            .iter()
            .map(|font| (font.internal_id.as_str(), font))
            .collect::<BTreeMap<_, _>>();
        let (read_tasks, json_resources) = self.collect_soundfont_dat_resources(&wanted)?;
        let dat_read_task_count = read_tasks.len();
        let read_tasks_by_font = read_tasks.iter().fold(
            BTreeMap::<String, Vec<&SoundFontDatReadTask>>::new(),
            |mut tasks_by_font, task| {
                tasks_by_font
                    .entry(task.soundfont_id.clone())
                    .or_default()
                    .push(task);
                tasks_by_font
            },
        );

        let json_resource_font_count = json_resources
            .values()
            .filter(|resources| soundfont_json_resources_present(resources))
            .count();
        self.last_subset_plans =
            plan_soundfont_subsets(soundfont_ids, channel_roles, &json_resources, &read_tasks)?;
        let previous_subset_state = self.loaded_subset_state.clone();
        let mut current_subset_state = BTreeMap::<String, LoadedSubsetState>::new();

        let mut full_dat_entry_reads = 0usize;
        let mut full_dat_entry_bytes = 0u64;
        let mut range_dat_reads = 0usize;
        let mut range_dat_read_bytes = 0u64;

        let mut soundfonts = Vec::with_capacity(soundfont_ids.len());
        let mut subset_request_count = 0usize;
        let mut subset_source_clone_bytes = 0u64;
        let mut full_fallback_count = 0usize;
        let mut non_subset_full_load_count = 0usize;
        for id in soundfont_ids {
            let tasks = read_tasks_by_font.get(id).map(Vec::as_slice).unwrap_or(&[]);
            if tasks.is_empty() {
                return Err(FlutzError::InvalidInput(format!(
                    "soundfont payload not found in DAT files: {id}"
                )));
            }
            let Some(entry) = catalog.get(id.as_str()) else {
                return Err(FlutzError::InvalidInput(format!(
                    "soundfont is missing from startup catalog: {id}"
                )));
            };
            if entry.runtime_format != "sf2" || entry.storage_format != "sf2" {
                return Err(FlutzError::UnsupportedFormat(format!(
                    "soundfont {id} is {} stored as {}; playback requires sf2",
                    entry.runtime_format, entry.storage_format
                )));
            }
            if let Some(plan) = self.last_subset_plans.get(id) {
                let subset_payload = json_resources
                    .get(id)
                    .and_then(|resources| resources.index.as_ref())
                    .and_then(|index| read_soundfont_subset_dat_payload(tasks, index, plan).ok());

                subset_request_count = subset_request_count.saturating_add(1);
                if let Some(subset_payload) = subset_payload {
                    range_dat_reads = range_dat_reads.saturating_add(subset_payload.range_reads);
                    range_dat_read_bytes =
                        range_dat_read_bytes.saturating_add(subset_payload.range_read_bytes as u64);
                    let request = SoundFontSubsetBytes::from_dat_entry(
                        id.clone(),
                        entry.display_name.clone(),
                        subset_payload.metadata_bytes,
                        subset_payload.sources.clone(),
                        subset_demand_signature(plan),
                        SoundFontMetadataClosure::new(
                            plan.preset_ids.iter().map(|id| *id as usize).collect(),
                            plan.instrument_ids.iter().map(|id| *id as usize).collect(),
                            plan.sample_ids.iter().map(|id| *id as usize).collect(),
                        ),
                        subset_payload.sample_ranges,
                    );
                    if let Ok(sound_font) = self.soundfont_cache.load_subset_soundfont(request) {
                        current_subset_state
                            .insert(id.clone(), LoadedSubsetState::from_plan(plan, true));
                        soundfonts.push(LoadedSoundFont {
                            internal_id: id.clone(),
                            display_name: entry.display_name.clone(),
                            sound_font,
                        });
                        continue;
                    }
                }

                full_fallback_count = full_fallback_count.saturating_add(1);
                let payload = read_soundfont_dat_payloads(tasks)?;
                full_dat_entry_reads = full_dat_entry_reads.saturating_add(payload.entry_reads);
                full_dat_entry_bytes =
                    full_dat_entry_bytes.saturating_add(payload.entry_read_bytes as u64);
                subset_source_clone_bytes =
                    subset_source_clone_bytes.saturating_add(payload.bytes.len() as u64);
                let sound_font =
                    self.soundfont_cache
                        .load_soundfont(SoundFontBytes::from_dat_entry(
                            id.clone(),
                            entry.display_name.clone(),
                            payload.bytes,
                            payload.sources,
                        ))?;
                current_subset_state.insert(id.clone(), LoadedSubsetState::from_plan(plan, false));
                soundfonts.push(LoadedSoundFont {
                    internal_id: id.clone(),
                    display_name: entry.display_name.clone(),
                    sound_font,
                });
            } else {
                non_subset_full_load_count = non_subset_full_load_count.saturating_add(1);
                let payload = read_soundfont_dat_payloads(tasks)?;
                full_dat_entry_reads = full_dat_entry_reads.saturating_add(payload.entry_reads);
                full_dat_entry_bytes =
                    full_dat_entry_bytes.saturating_add(payload.entry_read_bytes as u64);
                let sound_font =
                    self.soundfont_cache
                        .load_soundfont(SoundFontBytes::from_dat_entry(
                            id.clone(),
                            entry.display_name.clone(),
                            payload.bytes,
                            payload.sources,
                        ))?;
                soundfonts.push(LoadedSoundFont {
                    internal_id: id.clone(),
                    display_name: entry.display_name.clone(),
                    sound_font,
                });
            }
        }

        self.loaded_subset_state = current_subset_state;
        self.subset_transition =
            compare_subset_transition(&previous_subset_state, &self.loaded_subset_state);
        self.last_soundfont_load = PlaybackSoundFontLoadDiagnostics {
            load_invocations: load_invocation,
            requested_font_count: soundfont_ids.len(),
            dat_read_task_count,
            full_dat_entry_reads,
            range_dat_reads,
            full_dat_entry_bytes,
            range_dat_read_bytes,
            subset_request_count,
            subset_source_clone_bytes,
            full_fallback_count,
            non_subset_full_load_count,
            json_resource_font_count,
            index_json_plan_count: self
                .last_subset_plans
                .values()
                .filter(|plan| plan.used_index_json)
                .count(),
            load_duration_ms: load_started.elapsed().as_millis(),
        };

        Ok(soundfonts)
    }

    fn loaded_subset_state_satisfies(
        &mut self,
        soundfont_ids: &[String],
        channel_roles: &[MidiStripDescriptor],
    ) -> Result<bool> {
        let wanted = soundfont_ids.iter().cloned().collect::<BTreeSet<_>>();
        let (read_tasks, json_resources) = self.collect_soundfont_dat_resources(&wanted)?;
        let plans =
            plan_soundfont_subsets(soundfont_ids, channel_roles, &json_resources, &read_tasks)?;
        if plans.is_empty() {
            return Ok(true);
        }
        let previous_subset_state = self.loaded_subset_state.clone();
        let mut current_subset_state = BTreeMap::<String, LoadedSubsetState>::new();
        let mut satisfies = true;
        for (soundfont_id, plan) in &plans {
            let Some(previous) = previous_subset_state.get(soundfont_id) else {
                satisfies = false;
                break;
            };
            let current = LoadedSubsetState::from_plan(plan, previous.compact_loaded);
            if previous.compact_loaded && !current.sample_set().is_subset(&previous.sample_set()) {
                satisfies = false;
                break;
            }
            current_subset_state.insert(soundfont_id.clone(), current);
        }
        if satisfies {
            self.last_subset_plans = plans;
            self.subset_transition =
                compare_subset_transition(&previous_subset_state, &current_subset_state);
        }
        Ok(satisfies)
    }

    fn collect_soundfont_dat_resources(
        &self,
        wanted: &BTreeSet<String>,
    ) -> Result<(
        Vec<SoundFontDatReadTask>,
        BTreeMap<String, SoundFontJsonResources>,
    )> {
        let mut read_tasks = Vec::new();
        let mut json_resources = BTreeMap::<String, SoundFontJsonResources>::new();
        let mut next_order = 0usize;

        for dat_path in crate::collect_dat_paths(&self.data_dir)? {
            let index = parse_dat_index_file(&dat_path)?;
            for record in &index.entries {
                if record.entry.asset_type == "soundfont"
                    && wanted.contains(&record.entry.internal_id)
                {
                    read_tasks.push(SoundFontDatReadTask {
                        order: next_order,
                        soundfont_id: record.entry.internal_id.clone(),
                        dat_path: dat_path.clone(),
                        index: index.clone(),
                        record: record.clone(),
                    });
                    next_order = next_order.saturating_add(1);
                }
            }
            for soundfont_id in wanted {
                if json_resources
                    .get(soundfont_id)
                    .map(soundfont_json_resources_complete)
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Ok(resources) =
                    read_soundfont_json_resources_from_file(&dat_path, &index, soundfont_id)
                {
                    if soundfont_json_resources_present(&resources) {
                        json_resources
                            .entry(soundfont_id.clone())
                            .and_modify(|existing| {
                                merge_soundfont_json_resources(existing, &resources)
                            })
                            .or_insert(resources);
                    }
                }
            }
        }

        Ok((read_tasks, json_resources))
    }

    fn record_engine_replacement(&mut self) {
        let Some(engine) = &self.engine else {
            return;
        };
        let Ok(engine) = engine.lock() else {
            return;
        };
        let memory = engine.memory_debug();
        self.lifecycle.engine_replacements = self.lifecycle.engine_replacements.saturating_add(1);
        self.lifecycle.cumulative_replaced_engine_estimated_bytes = self
            .lifecycle
            .cumulative_replaced_engine_estimated_bytes
            .saturating_add(memory.estimated_bytes as u64);
        self.lifecycle.last_replaced_engine_estimated_bytes = memory.estimated_bytes;
        self.lifecycle.last_replaced_engine_instances = memory.instance_count;
        self.lifecycle.last_replaced_stem_effects = memory.stem_effects;
        self.lifecycle.last_replaced_stem_effect_bytes = memory.stem_effect_bytes;
    }
}

#[derive(Debug)]
struct SoundFontDatReadTask {
    order: usize,
    soundfont_id: String,
    dat_path: PathBuf,
    index: DatArchiveIndex,
    record: DatEntryRecord,
}

#[derive(Debug)]
struct SoundFontDatPayload {
    bytes: Vec<u8>,
    sources: Vec<String>,
    entry_reads: usize,
    entry_read_bytes: usize,
}

#[derive(Debug)]
struct SoundFontSubsetDatPayload {
    metadata_bytes: Vec<u8>,
    sample_ranges: Vec<SoundFontSubsetSampleRange>,
    sources: Vec<String>,
    range_reads: usize,
    range_read_bytes: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontSubsetPlanDebug {
    pub soundfont_id: String,
    pub requested_role_count: usize,
    pub preset_ids: Vec<u32>,
    pub instrument_ids: Vec<u32>,
    pub sample_ids: Vec<u32>,
    pub logical_wave_range_count: usize,
    pub planned_range_count: usize,
    pub planned_byte_count: u64,
    pub used_index_json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedSubsetState {
    pub soundfont_id: String,
    pub demand_signature: SoundFontDemandSignature,
    pub preset_ids: Vec<u32>,
    pub instrument_ids: Vec<u32>,
    pub sample_ids: Vec<u32>,
    pub planned_byte_count: u64,
    pub compact_loaded: bool,
}

impl LoadedSubsetState {
    fn from_plan(plan: &SoundFontSubsetPlanDebug, compact_loaded: bool) -> Self {
        Self {
            soundfont_id: plan.soundfont_id.clone(),
            demand_signature: SoundFontDemandSignature::from_subset_plan(plan),
            preset_ids: plan.preset_ids.clone(),
            instrument_ids: plan.instrument_ids.clone(),
            sample_ids: plan.sample_ids.clone(),
            planned_byte_count: plan.planned_byte_count,
            compact_loaded,
        }
    }

    fn sample_set(&self) -> BTreeSet<u32> {
        self.sample_ids.iter().copied().collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackSubsetTransitionDiagnostics {
    pub exact_subset_hits: usize,
    pub subset_contained_hits: usize,
    pub superset_growth_events: usize,
    pub font_set_changes: usize,
    pub missing_sample_count: usize,
    pub missing_planned_bytes: u64,
    pub compact_loaded_count: usize,
    pub full_fallback_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackSoundFontLoadDiagnostics {
    pub load_invocations: u64,
    pub requested_font_count: usize,
    pub dat_read_task_count: usize,
    pub full_dat_entry_reads: usize,
    pub range_dat_reads: usize,
    pub full_dat_entry_bytes: u64,
    pub range_dat_read_bytes: u64,
    pub subset_request_count: usize,
    pub subset_source_clone_bytes: u64,
    pub full_fallback_count: usize,
    pub non_subset_full_load_count: usize,
    pub json_resource_font_count: usize,
    pub index_json_plan_count: usize,
    pub load_duration_ms: u128,
}

fn compare_subset_transition(
    previous: &BTreeMap<String, LoadedSubsetState>,
    current: &BTreeMap<String, LoadedSubsetState>,
) -> PlaybackSubsetTransitionDiagnostics {
    let mut diagnostics = PlaybackSubsetTransitionDiagnostics::default();
    for (soundfont_id, current_state) in current {
        let Some(previous_state) = previous.get(soundfont_id) else {
            diagnostics.font_set_changes = diagnostics.font_set_changes.saturating_add(1);
            diagnostics.missing_sample_count = diagnostics
                .missing_sample_count
                .saturating_add(current_state.sample_ids.len());
            diagnostics.missing_planned_bytes = diagnostics
                .missing_planned_bytes
                .saturating_add(current_state.planned_byte_count);
            continue;
        };
        let previous_samples = previous_state.sample_set();
        let current_samples = current_state.sample_set();
        if previous_samples == current_samples {
            diagnostics.exact_subset_hits = diagnostics.exact_subset_hits.saturating_add(1);
        } else if current_samples.is_subset(&previous_samples) {
            diagnostics.subset_contained_hits = diagnostics.subset_contained_hits.saturating_add(1);
        } else if current_samples.is_superset(&previous_samples) {
            diagnostics.superset_growth_events =
                diagnostics.superset_growth_events.saturating_add(1);
            diagnostics.missing_sample_count = diagnostics
                .missing_sample_count
                .saturating_add(current_samples.difference(&previous_samples).count());
            diagnostics.missing_planned_bytes = diagnostics.missing_planned_bytes.saturating_add(
                current_state
                    .planned_byte_count
                    .saturating_sub(previous_state.planned_byte_count),
            );
        } else {
            diagnostics.font_set_changes = diagnostics.font_set_changes.saturating_add(1);
        }
    }
    diagnostics.compact_loaded_count = current
        .values()
        .filter(|state| state.compact_loaded)
        .count();
    diagnostics.full_fallback_count = current
        .values()
        .filter(|state| !state.compact_loaded)
        .count();
    diagnostics
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontSubsetPlanAggregateDebug {
    pub plan_count: usize,
    pub preset_count: usize,
    pub instrument_count: usize,
    pub sample_count: usize,
    pub logical_wave_range_count: usize,
    pub planned_range_count: usize,
    pub planned_byte_count: u64,
    pub index_json_plan_count: usize,
    pub signatures: Vec<SoundFontDemandSignature>,
}

impl SoundFontSubsetPlanAggregateDebug {
    fn from_plans(plans: &BTreeMap<String, SoundFontSubsetPlanDebug>) -> Self {
        Self {
            plan_count: plans.len(),
            preset_count: plans.values().map(|plan| plan.preset_ids.len()).sum(),
            instrument_count: plans.values().map(|plan| plan.instrument_ids.len()).sum(),
            sample_count: plans.values().map(|plan| plan.sample_ids.len()).sum(),
            logical_wave_range_count: plans
                .values()
                .map(|plan| plan.logical_wave_range_count)
                .sum(),
            planned_range_count: plans.values().map(|plan| plan.planned_range_count).sum(),
            planned_byte_count: plans.values().map(|plan| plan.planned_byte_count).sum(),
            index_json_plan_count: plans.values().filter(|plan| plan.used_index_json).count(),
            signatures: plans
                .values()
                .map(SoundFontDemandSignature::from_subset_plan)
                .collect(),
        }
    }
}

fn read_soundfont_dat_payloads(tasks: &[&SoundFontDatReadTask]) -> Result<SoundFontDatPayload> {
    if tasks.is_empty() {
        return Ok(SoundFontDatPayload {
            bytes: Vec::new(),
            sources: Vec::new(),
            entry_reads: 0,
            entry_read_bytes: 0,
        });
    }

    let mut sorted_tasks = tasks.to_vec();
    sorted_tasks.sort_by_key(|task| task.order);
    let mut bytes = Vec::new();
    let mut sources = Vec::new();
    let mut entry_read_bytes = 0usize;
    for task in sorted_tasks {
        let source = task.dat_path.display().to_string();
        let payload = extract_entry_from_file(&task.dat_path, &task.index, &task.record)?;
        entry_read_bytes = entry_read_bytes.saturating_add(payload.len());
        bytes.extend_from_slice(&payload);
        if !sources.contains(&source) {
            sources.push(source);
        }
    }
    Ok(SoundFontDatPayload {
        bytes,
        sources,
        entry_reads: tasks.len(),
        entry_read_bytes,
    })
}

fn read_soundfont_subset_dat_payload(
    tasks: &[&SoundFontDatReadTask],
    index: &SoundFontIndexJson,
    plan: &SoundFontSubsetPlanDebug,
) -> Result<SoundFontSubsetDatPayload> {
    if tasks.is_empty() {
        return Err(FlutzError::InvalidInput(
            "soundfont DAT subset has no DAT entries".to_owned(),
        ));
    }
    let mut sorted_tasks = tasks.to_vec();
    sorted_tasks.sort_by_key(|task| task.order);
    let parts = sorted_tasks
        .iter()
        .map(|task| SplitDatEntryPart {
            path: task.dat_path.as_path(),
            index: &task.index,
            entry: &task.record,
        })
        .collect::<Vec<_>>();

    let metadata = read_soundfont_thin_metadata_from_files(&parts, index)?;
    let samples = read_soundfont_sample_ranges_from_files(&parts, index, &plan.sample_ids)?;
    let mut sample_ranges = Vec::with_capacity(samples.samples.len());
    for sample in samples.samples {
        sample_ranges.push(SoundFontSubsetSampleRange {
            sample_id: sample.sample_id as usize,
            samples: decode_smpl_i16(&sample.bytes)?,
        });
    }

    Ok(SoundFontSubsetDatPayload {
        metadata_bytes: metadata.bytes,
        sample_ranges,
        sources: soundfont_dat_sources(tasks),
        range_reads: metadata.read_count.saturating_add(samples.read_count),
        range_read_bytes: metadata.byte_count.saturating_add(samples.byte_count),
    })
}

fn decode_smpl_i16(bytes: &[u8]) -> Result<Vec<i16>> {
    if bytes.len() % 2 != 0 {
        return Err(FlutzError::InvalidInput(
            "soundfont sample range has odd byte length".to_owned(),
        ));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}

fn soundfont_dat_sources(tasks: &[&SoundFontDatReadTask]) -> Vec<String> {
    let mut sources = Vec::new();
    for task in tasks {
        let source = task.dat_path.display().to_string();
        if !sources.contains(&source) {
            sources.push(source);
        }
    }
    sources
}

fn soundfont_json_resources_present(resources: &SoundFontJsonResources) -> bool {
    resources.metadata.is_some()
        || resources.coverage.is_some()
        || resources.index.is_some()
        || resources.pack_report.is_some()
}

fn soundfont_json_resources_complete(resources: &SoundFontJsonResources) -> bool {
    resources.coverage.is_some() && resources.index.is_some()
}

fn merge_soundfont_json_resources(
    existing: &mut SoundFontJsonResources,
    incoming: &SoundFontJsonResources,
) {
    if existing.metadata.is_none() {
        existing.metadata = incoming.metadata.clone();
    }
    if existing.coverage.is_none() {
        existing.coverage = incoming.coverage.clone();
    }
    if existing.index.is_none() {
        existing.index = incoming.index.clone();
    }
    if existing.pack_report.is_none() {
        existing.pack_report = incoming.pack_report.clone();
    }
}

fn coverage_supports_midi_role(coverage: &SoundFontCoverage, role: &MidiStripDescriptor) -> bool {
    if role.is_percussion {
        coverage.provides_percussion()
    } else {
        coverage.provides_melodic(role.bank, role.program)
    }
}

fn subset_demand_signature(plan: &SoundFontSubsetPlanDebug) -> String {
    format!(
        "roles={};presets={};instruments={};samples={};ranges={}",
        plan.requested_role_count,
        join_u32_ids(&plan.preset_ids),
        join_u32_ids(&plan.instrument_ids),
        join_u32_ids(&plan.sample_ids),
        plan.logical_wave_range_count
    )
}

fn join_u32_ids(ids: &[u32]) -> String {
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn plan_soundfont_subsets(
    soundfont_ids: &[String],
    channel_roles: &[MidiStripDescriptor],
    json_resources: &BTreeMap<String, SoundFontJsonResources>,
    _read_tasks: &[SoundFontDatReadTask],
) -> Result<BTreeMap<String, SoundFontSubsetPlanDebug>> {
    let mut plans = BTreeMap::new();

    for soundfont_id in soundfont_ids {
        let Some(index) = json_resources
            .get(soundfont_id)
            .and_then(|resources| resources.index.as_ref())
        else {
            continue;
        };

        let mut preset_ids = BTreeSet::<u32>::new();
        let mut instrument_ids = BTreeSet::<u32>::new();
        let mut sample_ids = BTreeSet::<u32>::new();
        let mut wave_ranges = Vec::<DatEntryRange>::new();

        for role in channel_roles {
            for preset in index.presets.iter().filter(|preset| {
                if role.is_percussion {
                    preset.bank == 128
                } else {
                    preset.bank == role.bank && preset.program == role.program
                }
            }) {
                preset_ids.insert(preset.preset_id);
                for instrument_id in &preset.instrument_ids {
                    instrument_ids.insert(*instrument_id);
                }
            }
        }

        for instrument_id in &instrument_ids {
            if let Some(load_map) = index
                .instrument_load_map
                .iter()
                .find(|load_map| load_map.instrument_id == *instrument_id)
            {
                sample_ids.extend(load_map.sample_ids.iter().copied());
                wave_ranges.extend(load_map.wave_ranges.iter().filter_map(|range| {
                    (range.byte_length > 0).then_some(DatEntryRange {
                        offset: range.smpl_start_byte,
                        length: range.byte_length,
                    })
                }));
            }
        }

        if preset_ids.is_empty() && instrument_ids.is_empty() && sample_ids.is_empty() {
            continue;
        }

        let logical_ranges = coalesce_entry_ranges(&wave_ranges)?;
        let planned_range_count = logical_ranges.len();
        let planned_byte_count = logical_ranges.iter().map(|range| range.length).sum();

        plans.insert(
            soundfont_id.clone(),
            SoundFontSubsetPlanDebug {
                soundfont_id: soundfont_id.clone(),
                requested_role_count: channel_roles.len(),
                preset_ids: preset_ids.into_iter().collect(),
                instrument_ids: instrument_ids.into_iter().collect(),
                sample_ids: sample_ids.into_iter().collect(),
                logical_wave_range_count: wave_ranges.len(),
                planned_range_count,
                planned_byte_count,
                used_index_json: true,
            },
        );
    }

    Ok(plans)
}

impl Drop for PlaybackController {
    fn drop(&mut self) {
        if let Some(audio) = &mut self.audio {
            let _ = audio.pause();
        }
        self.audio = None;
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.reset_state_and_release_scratch();
        }
        self.soundfont_cache.clear();
        memory_runtime::purge_domain(MemoryDomain::FileLoad);
        memory_runtime::purge_domain(MemoryDomain::PlaybackLoad);
        memory_runtime::purge_domain(MemoryDomain::SoundFontDecode);
        memory_runtime::decay_idle_reuse_preserving(false);
    }
}

struct AnalyzerTrace {
    writer: BufWriter<fs::File>,
    sequence: u64,
}

struct RenderErrorTrace {
    writer: BufWriter<fs::File>,
    sequence: u64,
    recent: VecDeque<RenderErrorTraceEvent>,
}

impl RenderErrorTrace {
    fn open() -> Option<Self> {
        let dir = PathBuf::from("_local").join("render-error-trace");
        fs::create_dir_all(&dir).ok()?;
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(dir.join("render-errors.jsonl"))
            .ok()?;
        Some(Self {
            writer: BufWriter::new(file),
            sequence: 0,
            recent: VecDeque::new(),
        })
    }

    fn record_midi_scan(
        &mut self,
        midi_source: Option<&Path>,
        diagnostics: &PlaybackMidiScanDiagnostics,
    ) {
        self.sequence = self.sequence.saturating_add(1);
        let line = midi_scan_trace_json(self.sequence, midi_source, diagnostics);
        let _ = writeln!(self.writer, "{line}");
        let _ = self.writer.flush();
        #[cfg(debug_assertions)]
        eprintln!("{line}");
    }

    fn record_error(&mut self, event: RenderErrorTraceEvent) {
        self.sequence = self.sequence.saturating_add(1);
        let line = render_error_trace_json(self.sequence, &event);
        let _ = writeln!(self.writer, "{line}");
        let _ = self.writer.flush();
        #[cfg(debug_assertions)]
        eprintln!("{line}");
        self.recent.push_back(event);
        while self.recent.len() > 64 {
            self.recent.pop_front();
        }
    }

    fn take_recent(&mut self) -> Vec<RenderErrorTraceEvent> {
        self.recent.drain(..).collect()
    }
}

impl AnalyzerTrace {
    fn open() -> Option<Self> {
        let dir = PathBuf::from("_local").join("analyzer-trace");
        fs::create_dir_all(&dir).ok()?;
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(dir.join("analyzer.jsonl"))
            .ok()?;
        Some(Self {
            writer: BufWriter::new(file),
            sequence: 0,
        })
    }

    fn record(&mut self, frame: &VisualizerFrame) {
        self.sequence = self.sequence.saturating_add(1);
        let _ = write!(
            self.writer,
            "{{\"schema\":\"flutz.visualizer.frame.v1\",\"sequence\":{},\"frame_sequence\":{},\"timestamp_seconds\":{:.6},\"sample_rate\":{},\"fft_size\":{},\"band_count\":{},\"aggregate_peak\":{:.6},\"aggregate_rms\":{:.6},\"bands\":[",
            self.sequence,
            frame.sequence,
            frame.timestamp_seconds,
            frame.sample_rate_hz,
            frame.fft_size,
            frame.band_count(),
            frame.aggregate_peak,
            frame.aggregate_rms
        );
        for (index, band) in frame.bands.iter().enumerate() {
            if index > 0 {
                let _ = write!(self.writer, ",");
            }
            let _ = write!(
                self.writer,
                "{{\"index\":{},\"lower_hz\":{:.3},\"center_hz\":{:.3},\"upper_hz\":{:.3},\"live_level\":{:.6},\"column_level\":{:.6},\"peak_square_level\":{:.6}}}",
                band.definition.band_index,
                band.definition.lower_hz,
                band.definition.center_hz,
                band.definition.upper_hz,
                band.state.live_level_norm,
                band.state.column_level_norm,
                band.state.peak_square_level_norm
            );
        }
        let _ = writeln!(self.writer, "]}}");
        let _ = self.writer.flush();
    }
}

fn record_analyzer_trace_arc(trace: Option<&Arc<Mutex<AnalyzerTrace>>>, frame: &VisualizerFrame) {
    if let Some(trace) = trace {
        if let Ok(mut trace) = trace.lock() {
            trace.record(frame);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioPlaybackStatus {
    Audible,
    AudioUnavailable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioTransportMetadata {
    pub path: PathBuf,
    pub format_id: String,
    pub friendly_name: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub frame_length: u64,
    pub duration_seconds: f64,
    pub content_kind: ContentKind,
    pub mastering: MasteringCapability,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedRenderProbeReport {
    pub frames: usize,
    pub samples: usize,
    pub peak: f32,
    pub rms: f32,
    pub peq_generation: u64,
    pub scratch_growth_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedStreamCacheDebug {
    pub streaming: bool,
    pub cache_start_frame: u64,
    pub cache_frames: usize,
    pub cache_capacity_frames: usize,
    pub source_channels: usize,
    pub cached_sample_capacity: usize,
    pub full_sample_len: usize,
    pub full_sample_capacity: usize,
    pub request_generation: u64,
    pub filled_generation: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
enum DecodedPlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

const DECODED_STREAM_PLAYAHEAD_SECONDS: usize = 10;

#[derive(Debug)]
struct DecodedAudioPlaybackState {
    metadata: DecodedAudioTransportMetadata,
    samples: Option<Vec<f32>>,
    stream: Option<DecodedStreamCache>,
    peq: PeqProcessor,
    peq_input: Vec<f32>,
    position_frame: u64,
    state: DecodedPlaybackState,
    loop_settings: PlaybackLoopSettings,
    counted_loops_remaining: u32,
}

#[derive(Debug)]
struct DecodedStreamCache {
    shared: Arc<(Mutex<DecodedStreamCacheShared>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Debug)]
struct DecodedStreamCacheShared {
    cache_start_frame: u64,
    cache_frames: usize,
    cache_capacity_frames: usize,
    source_channels: usize,
    samples: Vec<f32>,
    request_start_frame: u64,
    request_generation: u64,
    filled_generation: u64,
    shutdown: bool,
    last_error: Option<String>,
}

impl Drop for DecodedStreamCache {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.shared;
        if let Ok(mut shared) = lock.lock() {
            shared.shutdown = true;
            shared.request_generation = shared.request_generation.saturating_add(1);
            condvar.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl DecodedAudioPlaybackState {
    fn new(
        metadata: DecodedAudioTransportMetadata,
        samples: Vec<f32>,
        peq: Option<PeqPresetFile>,
        preallocated_frames: usize,
    ) -> Result<Self> {
        let peq_config = decoded_peq_config(peq, metadata.sample_rate, 2);
        let peq = PeqProcessor::from_config(peq_config).map_err(|error| {
            FlutzError::InvalidInput(format!("invalid decoded audio PEQ config: {error}"))
        })?;
        Ok(Self {
            metadata,
            samples: Some(samples),
            stream: None,
            peq,
            peq_input: vec![0.0; preallocated_frames.saturating_mul(2)],
            position_frame: 0,
            state: DecodedPlaybackState::Stopped,
            loop_settings: PlaybackLoopSettings::default(),
            counted_loops_remaining: 0,
        })
    }

    fn new_streaming(
        metadata: DecodedAudioTransportMetadata,
        session: DecodedAudioStreamSession,
        peq: Option<PeqPresetFile>,
        preallocated_frames: usize,
    ) -> Result<Self> {
        let peq_config = decoded_peq_config(peq, metadata.sample_rate, 2);
        let peq = PeqProcessor::from_config(peq_config).map_err(|error| {
            FlutzError::InvalidInput(format!("invalid decoded audio PEQ config: {error}"))
        })?;
        let source_channels = metadata.channels.max(1);
        let cache_capacity_frames = (metadata.sample_rate as usize)
            .saturating_mul(DECODED_STREAM_PLAYAHEAD_SECONDS)
            .max(preallocated_frames.saturating_mul(4));
        let shared = Arc::new((
            Mutex::new(DecodedStreamCacheShared {
                cache_start_frame: 0,
                cache_frames: 0,
                cache_capacity_frames,
                source_channels,
                samples: Vec::with_capacity(cache_capacity_frames.saturating_mul(source_channels)),
                request_start_frame: 0,
                request_generation: 1,
                filled_generation: 0,
                shutdown: false,
                last_error: None,
            }),
            Condvar::new(),
        ));
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("flutz-decoded-stream".to_owned())
            .spawn(move || decoded_stream_worker(session, worker_shared))
            .map_err(|error| {
                FlutzError::Runtime(format!("failed to start decoded stream worker: {error}"))
            })?;
        Ok(Self {
            metadata,
            samples: None,
            stream: Some(DecodedStreamCache {
                shared,
                worker: Some(worker),
            }),
            peq,
            peq_input: vec![0.0; preallocated_frames.saturating_mul(2)],
            position_frame: 0,
            state: DecodedPlaybackState::Stopped,
            loop_settings: PlaybackLoopSettings::default(),
            counted_loops_remaining: 0,
        })
    }

    fn play(&mut self) {
        if self.position_frame >= self.metadata.frame_length {
            self.position_frame = 0;
        }
        if matches!(self.state, DecodedPlaybackState::Stopped) {
            self.counted_loops_remaining = self.loop_settings.loop_count.max(1);
        }
        self.request_stream_cache_fill(self.position_frame);
        self.state = DecodedPlaybackState::Playing;
    }

    fn pause(&mut self) {
        if matches!(self.state, DecodedPlaybackState::Playing) {
            self.state = DecodedPlaybackState::Paused;
        }
    }

    fn stop(&mut self) {
        self.state = DecodedPlaybackState::Stopped;
        self.position_frame = 0;
        self.counted_loops_remaining = self.loop_settings.loop_count.max(1);
    }

    fn is_playing(&self) -> bool {
        matches!(self.state, DecodedPlaybackState::Playing)
    }

    fn state_label(&self) -> &'static str {
        match self.state {
            DecodedPlaybackState::Stopped => "Stopped",
            DecodedPlaybackState::Playing => "Playing",
            DecodedPlaybackState::Paused => "Paused",
        }
    }

    fn position_seconds(&self) -> f64 {
        if self.metadata.sample_rate == 0 {
            return 0.0;
        }
        self.position_frame as f64 / self.metadata.sample_rate as f64
    }

    fn transport_fraction(&self) -> f32 {
        if self.metadata.frame_length == 0 {
            return 0.0;
        }
        (self.position_frame as f64 / self.metadata.frame_length as f64).clamp(0.0, 1.0) as f32
    }

    fn seek_fraction(&mut self, fraction: f32) {
        let target = (self.metadata.frame_length as f64 * f64::from(fraction.clamp(0.0, 1.0)))
            .round() as u64;
        self.seek_frame(target);
    }

    fn seek_seconds(&mut self, seconds: f64) {
        let target = (seconds.max(0.0) * self.metadata.sample_rate.max(1) as f64).round() as u64;
        self.seek_frame(target);
    }

    fn seek_frame(&mut self, frame: u64) {
        self.position_frame = frame.min(self.metadata.frame_length);
        self.counted_loops_remaining = self.loop_settings.loop_count.max(1);
        self.request_stream_cache_fill(self.position_frame);
    }

    fn set_loop_enabled(&mut self, enabled: bool) {
        self.loop_settings.enabled = enabled;
        if enabled && matches!(self.loop_settings.mode, PlaybackLoopMode::None) {
            self.loop_settings.mode = PlaybackLoopMode::Infinite;
        }
    }

    fn set_loop_settings(&mut self, mut settings: PlaybackLoopSettings) {
        settings.start_tick = settings.start_tick.min(self.metadata.frame_length);
        settings.end_tick = settings.end_tick.min(self.metadata.frame_length);
        settings.loop_count = settings.loop_count.max(1);
        if settings.end_tick <= settings.start_tick {
            settings.enabled = false;
        }
        self.loop_settings = settings;
        self.counted_loops_remaining = self.loop_settings.loop_count;
    }

    fn next_source_frame(&mut self) -> Option<u64> {
        if !self.is_playing() || self.metadata.frame_length == 0 {
            return None;
        }
        if self.position_frame >= self.effective_play_end_frame() && !self.apply_loop_boundary() {
            self.state = DecodedPlaybackState::Stopped;
            return None;
        }
        let frame = self.position_frame;
        self.position_frame = self.position_frame.saturating_add(1);
        Some(frame)
    }

    fn effective_play_end_frame(&self) -> u64 {
        if self.loop_settings.enabled && self.loop_settings.end_tick > self.loop_settings.start_tick
        {
            self.loop_settings.end_tick.min(self.metadata.frame_length)
        } else {
            self.metadata.frame_length
        }
    }

    fn apply_loop_boundary(&mut self) -> bool {
        if !self.loop_settings.enabled
            || self.loop_settings.end_tick <= self.loop_settings.start_tick
        {
            return false;
        }
        match self.loop_settings.mode {
            PlaybackLoopMode::Infinite => {
                self.position_frame = self.loop_settings.start_tick;
                self.request_stream_cache_fill(self.position_frame);
                true
            }
            PlaybackLoopMode::Counted if self.counted_loops_remaining > 0 => {
                self.counted_loops_remaining = self.counted_loops_remaining.saturating_sub(1);
                self.position_frame = self.loop_settings.start_tick;
                self.request_stream_cache_fill(self.position_frame);
                true
            }
            _ => false,
        }
    }

    fn retained_scratch_bytes(&self) -> usize {
        let stream_bytes = self
            .stream
            .as_ref()
            .and_then(|stream| {
                stream.shared.0.lock().ok().map(|shared| {
                    shared
                        .samples
                        .capacity()
                        .saturating_mul(std::mem::size_of::<f32>())
                })
            })
            .unwrap_or_default();
        let full_sample_bytes = self
            .samples
            .as_ref()
            .map(|samples| {
                samples
                    .capacity()
                    .saturating_mul(std::mem::size_of::<f32>())
            })
            .unwrap_or_default();
        self.peq_input
            .capacity()
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_add(self.peq.metrics().retained_state_bytes)
            .saturating_add(stream_bytes)
            .saturating_add(full_sample_bytes)
    }

    fn peq_generation(&self) -> u64 {
        self.peq.metrics().active_config_generation
    }

    fn set_peq_prepared_config(&mut self, prepared: PreparedConfig) -> Result<u64> {
        self.peq.set_prepared_config(prepared).map_err(|error| {
            FlutzError::InvalidInput(format!("invalid decoded audio PEQ config: {error}"))
        })?;
        Ok(self
            .peq
            .metrics()
            .pending_config_generation
            .unwrap_or_else(|| self.peq.metrics().active_config_generation))
    }

    fn stream_cache_debug(&self) -> Option<DecodedStreamCacheDebug> {
        let Some(stream) = &self.stream else {
            return Some(DecodedStreamCacheDebug {
                streaming: false,
                cache_start_frame: 0,
                cache_frames: 0,
                cache_capacity_frames: 0,
                source_channels: self.metadata.channels,
                cached_sample_capacity: 0,
                full_sample_len: self.samples.as_ref().map(Vec::len).unwrap_or_default(),
                full_sample_capacity: self.samples.as_ref().map(Vec::capacity).unwrap_or_default(),
                request_generation: 0,
                filled_generation: 0,
                last_error: None,
            });
        };
        let shared = stream.shared.0.lock().ok()?;
        Some(DecodedStreamCacheDebug {
            streaming: true,
            cache_start_frame: shared.cache_start_frame,
            cache_frames: shared.cache_frames,
            cache_capacity_frames: shared.cache_capacity_frames,
            source_channels: shared.source_channels,
            cached_sample_capacity: shared.samples.capacity(),
            full_sample_len: 0,
            full_sample_capacity: 0,
            request_generation: shared.request_generation,
            filled_generation: shared.filled_generation,
            last_error: shared.last_error.clone(),
        })
    }

    fn wait_for_stream_cache(&self, timeout: Duration) -> bool {
        let Some(stream) = &self.stream else {
            return true;
        };
        let target_frame = self.position_frame;
        let (lock, condvar) = &*stream.shared;
        let Ok(shared) = lock.lock() else {
            return false;
        };
        if frame_in_cache(&shared, target_frame) {
            return true;
        }
        match condvar.wait_timeout_while(shared, timeout, |shared| {
            !shared.shutdown && shared.last_error.is_none() && !frame_in_cache(shared, target_frame)
        }) {
            Ok((shared, _)) => frame_in_cache(&shared, target_frame),
            Err(_) => false,
        }
    }

    fn request_stream_cache_fill(&self, frame: u64) {
        let Some(stream) = &self.stream else {
            return;
        };
        let (lock, condvar) = &*stream.shared;
        if let Ok(mut shared) = lock.lock() {
            if frame_in_cache(&shared, frame) && frame_in_cache(&shared, frame.saturating_add(1)) {
                return;
            }
            if shared.request_start_frame == frame
                && shared.request_generation != shared.filled_generation
            {
                return;
            }
            shared.request_start_frame = frame;
            shared.request_generation = shared.request_generation.saturating_add(1);
            condvar.notify_all();
        }
    }

    fn request_stream_cache_prefetch(&self, rendered_frames: usize) {
        let Some(stream) = &self.stream else {
            return;
        };
        let threshold_frames = rendered_frames
            .saturating_mul(4)
            .max((self.metadata.sample_rate as usize / 2).max(1));
        let (lock, condvar) = &*stream.shared;
        if let Ok(mut shared) = lock.lock() {
            let cache_end = shared
                .cache_start_frame
                .saturating_add(shared.cache_frames as u64);
            let position = self.position_frame.min(self.metadata.frame_length);
            let remaining = cache_end.saturating_sub(position);
            if position >= shared.cache_start_frame
                && position < cache_end
                && remaining > threshold_frames as u64
            {
                return;
            }
            if shared.request_start_frame == position
                && shared.request_generation != shared.filled_generation
            {
                return;
            }
            shared.request_start_frame = position;
            shared.request_generation = shared.request_generation.saturating_add(1);
            condvar.notify_all();
        }
    }
}

fn render_decoded_audio_stereo(
    decoded: &mut DecodedAudioPlaybackState,
    output: &mut [f32],
    output_gain: f32,
) -> Result<MeterReading> {
    if decoded.peq_input.len() < output.len() {
        decoded.peq_input.resize(output.len(), 0.0);
    }
    let source_channels = decoded.metadata.channels.max(1);
    let output_frames = output.len() / 2;
    for frame_index in 0..output_frames {
        let output_index = frame_index * 2;
        let Some(source_frame) = decoded.next_source_frame() else {
            decoded.peq_input[output_index] = 0.0;
            decoded.peq_input[output_index + 1] = 0.0;
            continue;
        };
        let (left, right) = decoded_source_stereo_frame(decoded, source_frame, source_channels);
        decoded.peq_input[output_index] = left;
        decoded.peq_input[output_index + 1] = right;
    }
    decoded
        .peq
        .process_interleaved(&decoded.peq_input[..output.len()], output)
        .map_err(|error| FlutzError::Runtime(format!("decoded audio PEQ failed: {error}")))?;
    for sample in output.iter_mut() {
        *sample *= output_gain;
    }
    decoded.request_stream_cache_prefetch(output_frames);
    Ok(MeterReading::from_interleaved(output))
}

fn decoded_source_stereo_frame(
    decoded: &DecodedAudioPlaybackState,
    source_frame: u64,
    source_channels: usize,
) -> (f32, f32) {
    if let Some(samples) = decoded.samples.as_ref() {
        let source_index = source_frame as usize * source_channels;
        if source_index >= samples.len() {
            return (0.0, 0.0);
        }
        let left = samples[source_index];
        let right = if source_channels == 1 {
            left
        } else {
            samples.get(source_index + 1).copied().unwrap_or(left)
        };
        return (left, right);
    }

    let Some(stream) = decoded.stream.as_ref() else {
        return (0.0, 0.0);
    };
    let (lock, condvar) = &*stream.shared;
    let Ok(mut shared) = lock.lock() else {
        return (0.0, 0.0);
    };
    if !frame_in_cache(&shared, source_frame) {
        if shared.request_start_frame != source_frame
            || shared.request_generation == shared.filled_generation
        {
            shared.request_start_frame = source_frame;
            shared.request_generation = shared.request_generation.saturating_add(1);
            condvar.notify_all();
        }
        return (0.0, 0.0);
    }
    let frame_offset = source_frame.saturating_sub(shared.cache_start_frame) as usize;
    let source_index = frame_offset.saturating_mul(shared.source_channels.max(1));
    let left = shared.samples.get(source_index).copied().unwrap_or(0.0);
    let right = if shared.source_channels == 1 {
        left
    } else {
        shared
            .samples
            .get(source_index + 1)
            .copied()
            .unwrap_or(left)
    };
    (left, right)
}

fn frame_in_cache(shared: &DecodedStreamCacheShared, frame: u64) -> bool {
    let cache_end = shared
        .cache_start_frame
        .saturating_add(shared.cache_frames as u64);
    frame >= shared.cache_start_frame && frame < cache_end
}

fn decoded_stream_worker(
    mut session: DecodedAudioStreamSession,
    shared: Arc<(Mutex<DecodedStreamCacheShared>, Condvar)>,
) {
    let mut decode_buffer = Vec::<f32>::new();
    loop {
        let (request_start, request_generation, cache_capacity_frames) = {
            let (lock, condvar) = &*shared;
            let mut state = match lock.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            while !state.shutdown && state.request_generation == state.filled_generation {
                state = condvar
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if state.shutdown {
                return;
            }
            (
                state.request_start_frame,
                state.request_generation,
                state.cache_capacity_frames,
            )
        };

        let result = (|| -> Result<usize> {
            session.seek_frame(request_start)?;
            let window = session.decode_next_frames(cache_capacity_frames, &mut decode_buffer)?;
            Ok(window.frames_decoded)
        })();

        let (lock, condvar) = &*shared;
        let mut state = match lock.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.shutdown {
            return;
        }
        match result {
            Ok(frames) => {
                state.samples.clear();
                state.samples.extend_from_slice(&decode_buffer);
                state.cache_start_frame = request_start;
                state.cache_frames = frames;
                state.last_error = None;
            }
            Err(error) => {
                state.samples.clear();
                state.cache_start_frame = request_start;
                state.cache_frames = 0;
                state.last_error = Some(format!("{error}"));
            }
        }
        state.filled_generation = request_generation;
        condvar.notify_all();
    }
}

fn decoded_peq_config(peq: Option<PeqPresetFile>, sample_rate: u32, channels: u16) -> PeqConfig {
    let mut config = peq.map(|preset| preset.config).unwrap_or_default();
    config.sample_rate_hz = sample_rate.max(1);
    config.channel_count = channels.max(1);
    config.channel_layout = ChannelLayout::Interleaved;
    config
}

#[derive(Debug, Clone)]
pub struct PlaybackDebugMetrics {
    pub engine_state: String,
    pub transport_seconds: f64,
    pub transport_duration_seconds: f64,
    pub transport_tick: u64,
    pub loaded_soundfont_count: usize,
    pub requested_soundfont_count: usize,
    pub loaded_provider_count: usize,
    pub pruned_soundfont_count: usize,
    pub loaded_midi_bytes: usize,
    pub loaded_midi_capacity_bytes: usize,
    pub midi_strip_count: usize,
    pub midi_demand: MidiDemandProfile,
    pub subset_plans: SoundFontSubsetPlanAggregateDebug,
    pub loaded_subset_state: BTreeMap<String, LoadedSubsetState>,
    pub subset_transition: PlaybackSubsetTransitionDiagnostics,
    pub soundfont_load: PlaybackSoundFontLoadDiagnostics,
    pub output_peak: f32,
    pub output_rms: f32,
    pub meter_latency_frames: u64,
    pub meter_latency_ms: f64,
    pub meter_wrapper_queue_frames: u64,
    pub meter_device_queue_frames: u64,
    pub live_frame_clock: u64,
    pub audible_frame_clock: u64,
    pub active_strip_count: usize,
    pub audio_status: String,
    pub audio_error: Option<String>,
    pub audio_backend: &'static str,
    pub audio_config: AudioDeviceConfig,
    pub audio_diagnostics: Option<AudioDeviceDiagnostics>,
    pub flux_guard: FluxGuardSnapshot,
    pub memory_debug: Option<PlaybackMemoryDebug>,
    pub render_churn: RenderChurnDiagnostics,
    pub lifecycle: PlaybackLifecycleDiagnostics,
    pub soundfont_cache: SoundFontRuntimeCacheDebug,
    pub component_memory: PlaybackComponentMemoryDiagnostics,
    pub references: PlaybackReferenceDiagnostics,
    pub visualizer: VisualizerDebugMetrics,
}

#[derive(Debug, Clone, Default)]
pub struct VisualizerDebugMetrics {
    pub band_count: usize,
    pub dominant_band_index: Option<usize>,
    pub dominant_center_hz: f32,
    pub dominant_live_level: f32,
    pub highest_peak_square_level: f32,
    pub aggregate_peak: f32,
    pub aggregate_rms: f32,
}

impl VisualizerDebugMetrics {
    fn from_frame(frame: &VisualizerFrame) -> Self {
        let dominant_band_index = frame.dominant_band_index();
        let dominant = dominant_band_index.and_then(|index| frame.bands.get(index));
        Self {
            band_count: frame.band_count(),
            dominant_band_index,
            dominant_center_hz: dominant
                .map(|band| band.definition.center_hz)
                .unwrap_or_default(),
            dominant_live_level: dominant
                .map(|band| band.state.live_level_norm)
                .unwrap_or_default(),
            highest_peak_square_level: frame
                .bands
                .iter()
                .map(|band| band.state.peak_square_level_norm)
                .fold(0.0, f32::max),
            aggregate_peak: frame.aggregate_peak,
            aggregate_rms: frame.aggregate_rms,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PlaybackReferenceDiagnostics {
    pub engine: Option<ArcReferenceDiagnostics>,
    pub mixer: ArcReferenceDiagnostics,
    pub mixer_controls: ArcReferenceDiagnostics,
    pub latest_snapshot: ArcReferenceDiagnostics,
    pub midi_strips: ArcReferenceDiagnostics,
    pub render_scratch: ArcReferenceDiagnostics,
    pub render_churn: ArcReferenceDiagnostics,
    pub audio_stream_open: bool,
}

#[derive(Debug, Copy, Clone, Default)]
pub struct ArcReferenceDiagnostics {
    pub strong_count: usize,
    pub weak_count: usize,
    pub expected_strong_roots: usize,
    pub excess_strong_count: usize,
}

impl ArcReferenceDiagnostics {
    fn from_arc<T>(arc: &Arc<T>, expected_strong_roots: usize) -> Self {
        let strong_count = Arc::strong_count(arc);
        Self {
            strong_count,
            weak_count: Arc::weak_count(arc),
            expected_strong_roots,
            excess_strong_count: strong_count.saturating_sub(expected_strong_roots),
        }
    }
}

fn expected_audio_closure_roots(audio_open: bool) -> usize {
    if audio_open {
        2
    } else {
        1
    }
}

fn expected_engine_roots(audio_open: bool) -> usize {
    expected_audio_closure_roots(audio_open)
}

#[derive(Debug, Clone, Default)]
pub struct PlaybackComponentMemoryDiagnostics {
    pub soundfont_metadata: SoundFontMetadataDiagnostics,
    pub midi: MidiRuntimeMemoryDiagnostics,
    pub rustystem: RustystemRuntimeDiagnostics,
    pub audio: AudioRuntimeMemoryDiagnostics,
    pub tracked_total_bytes: usize,
}

#[derive(Debug, Copy, Clone, Default)]
pub struct SoundFontMetadataDiagnostics {
    pub catalog_entries: usize,
    pub catalog_estimated_bytes: usize,
    pub loaded_soundfont_ids: usize,
    pub loaded_soundfont_id_bytes: usize,
    pub loaded_coverage_entries: usize,
    pub loaded_coverage_estimated_bytes: usize,
    pub estimated_bytes: usize,
}

#[derive(Debug, Copy, Clone, Default)]
pub struct MidiRuntimeMemoryDiagnostics {
    pub raw_bytes: usize,
    pub raw_capacity_bytes: usize,
    pub strip_count: usize,
    pub strip_capacity: usize,
    pub strip_bytes: usize,
    pub jump_point_count: usize,
    pub jump_point_bytes: usize,
    pub parsed_message_count: usize,
    pub parsed_sysex_events: usize,
    pub parsed_sysex_bytes: usize,
    pub parsed_estimated_bytes: usize,
    pub estimated_bytes: usize,
}

#[derive(Debug, Copy, Clone, Default)]
pub struct RustystemRuntimeDiagnostics {
    pub instance_count: usize,
    pub soundfont_wave_bytes: usize,
    pub voice_buffer_bytes: usize,
    pub block_buffer_bytes: usize,
    pub effects_bytes: usize,
    pub preset_lookup_bytes: usize,
    pub channel_bytes: usize,
    pub stem_effect_map_bytes: usize,
    pub block_stem_vec_bytes: usize,
    pub estimated_bytes: usize,
}

impl RustystemRuntimeDiagnostics {
    fn from_memory_debug(memory_debug: Option<&PlaybackMemoryDebug>) -> Self {
        let Some(memory) = memory_debug else {
            return Self::default();
        };
        Self {
            instance_count: memory.instance_count,
            soundfont_wave_bytes: memory.soundfont_wave_bytes,
            voice_buffer_bytes: memory.voice_buffer_bytes,
            block_buffer_bytes: memory.block_buffer_bytes,
            effects_bytes: memory.effects_bytes,
            preset_lookup_bytes: memory.preset_lookup_bytes,
            channel_bytes: memory.channel_bytes,
            stem_effect_map_bytes: memory.stem_effect_map_bytes,
            block_stem_vec_bytes: memory.block_stem_vec_bytes,
            estimated_bytes: memory.estimated_bytes.saturating_sub(
                memory
                    .midi_file
                    .map(|debug| debug.estimated_bytes)
                    .unwrap_or_default(),
            ),
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct AudioRuntimeMemoryDiagnostics {
    pub stream_open: bool,
    pub ring_retained_bytes: usize,
    pub callback_scratch_bytes: usize,
    pub producer_render_block_bytes: u64,
    pub estimated_bytes: usize,
}

impl AudioRuntimeMemoryDiagnostics {
    fn from_audio_diagnostics(audio: Option<&AudioDeviceDiagnostics>) -> Self {
        let Some(audio) = audio else {
            return Self::default();
        };
        let producer_render_block_bytes = audio.producer_render_block_bytes;
        Self {
            stream_open: true,
            ring_retained_bytes: audio.ring_retained_bytes,
            callback_scratch_bytes: audio.callback_scratch_bytes,
            producer_render_block_bytes,
            estimated_bytes: audio
                .ring_retained_bytes
                .saturating_add(audio.callback_scratch_bytes)
                .saturating_add(producer_render_block_bytes as usize),
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PlaybackLifecycleDiagnostics {
    pub engine_replacements: u64,
    pub cumulative_replaced_engine_estimated_bytes: u64,
    pub last_replaced_engine_estimated_bytes: usize,
    pub last_replaced_engine_instances: usize,
    pub last_replaced_stem_effects: usize,
    pub last_replaced_stem_effect_bytes: usize,
}

#[derive(Debug, Clone, Default)]
pub struct RenderChurnDiagnostics {
    pub render_calls: u64,
    pub cumulative_allocation_events: u64,
    pub cumulative_deallocation_events: u64,
    pub cumulative_transient_alloc_bytes: u64,
    pub cumulative_transient_dealloc_bytes: u64,
    pub cumulative_transient_growth_bytes: u64,
    pub cumulative_retained_capacity_growth_bytes: u64,
    pub cumulative_temp_container_alloc_bytes: u64,
    pub max_transient_alloc_bytes: usize,
    pub max_transient_growth_bytes: usize,
    pub max_retained_capacity_growth_bytes: usize,
    pub max_temp_container_alloc_bytes: usize,
    pub last: RenderChurnSnapshot,
}

impl RenderChurnDiagnostics {
    fn record(&mut self, snapshot: RenderChurnSnapshot) {
        self.render_calls = self.render_calls.saturating_add(1);
        self.cumulative_allocation_events = self
            .cumulative_allocation_events
            .saturating_add(snapshot.allocation_events as u64);
        self.cumulative_deallocation_events = self
            .cumulative_deallocation_events
            .saturating_add(snapshot.deallocation_events as u64);
        self.cumulative_transient_alloc_bytes = self
            .cumulative_transient_alloc_bytes
            .saturating_add(snapshot.transient_alloc_bytes as u64);
        self.cumulative_transient_dealloc_bytes = self
            .cumulative_transient_dealloc_bytes
            .saturating_add(snapshot.transient_dealloc_bytes as u64);
        self.cumulative_transient_growth_bytes = self
            .cumulative_transient_growth_bytes
            .saturating_add(snapshot.transient_growth_bytes as u64);
        self.cumulative_retained_capacity_growth_bytes = self
            .cumulative_retained_capacity_growth_bytes
            .saturating_add(snapshot.retained_capacity_growth_bytes as u64);
        self.cumulative_temp_container_alloc_bytes = self
            .cumulative_temp_container_alloc_bytes
            .saturating_add(snapshot.temp_container_alloc_bytes as u64);
        self.max_transient_alloc_bytes = self
            .max_transient_alloc_bytes
            .max(snapshot.transient_alloc_bytes);
        self.max_transient_growth_bytes = self
            .max_transient_growth_bytes
            .max(snapshot.transient_growth_bytes);
        self.max_retained_capacity_growth_bytes = self
            .max_retained_capacity_growth_bytes
            .max(snapshot.retained_capacity_growth_bytes);
        self.max_temp_container_alloc_bytes = self
            .max_temp_container_alloc_bytes
            .max(snapshot.temp_container_alloc_bytes);
        self.last = snapshot;
    }
}

#[derive(Debug, Clone, Default)]
pub struct RenderChurnSnapshot {
    pub allocation_events: usize,
    pub deallocation_events: usize,
    pub frames: usize,
    pub rendered_blocks: usize,
    pub routed_blocks: usize,
    pub mixer_inputs: usize,
    pub zero_fill_blocks: usize,
    pub visual_strips: usize,
    pub stem_audio_bytes: usize,
    pub stem_output_blocks: usize,
    pub stem_output_audio_bytes: usize,
    pub stem_output_active_note_bytes: usize,
    pub stem_output_vec_bytes: usize,
    pub stem_internal_blocks: usize,
    pub stem_internal_audio_bytes: usize,
    pub stem_internal_active_note_bytes: usize,
    pub stem_internal_vec_bytes: usize,
    pub stem_residual_buffer_bytes: usize,
    pub stem_effect_input_bytes: usize,
    pub stem_effect_input_vec_bytes: usize,
    pub stem_output_growth_bytes: usize,
    pub stem_internal_growth_bytes: usize,
    pub stem_residual_growth_bytes: usize,
    pub stem_effect_input_growth_bytes: usize,
    pub stem_playback_growth_bytes: usize,
    pub stem_total_growth_bytes: usize,
    pub stem_tracked_alloc_bytes: usize,
    pub stem_untracked_alloc_bytes: usize,
    pub zero_fill_audio_bytes: usize,
    pub mixer_frame_bytes: usize,
    pub mixer_internal_alloc_bytes: usize,
    pub mixer_internal_dealloc_bytes: usize,
    pub mixer_scratch_growth_bytes: usize,
    pub mixer_output_bytes: usize,
    pub mixer_prepared_frame_bytes: usize,
    pub mixer_report_bytes: usize,
    pub vec_overhead_bytes: usize,
    pub retained_container_capacity_bytes: usize,
    pub temp_container_alloc_bytes: usize,
    pub retained_capacity_growth_bytes: usize,
    pub transient_alloc_bytes: usize,
    pub transient_dealloc_bytes: usize,
    pub transient_growth_bytes: usize,
}

struct MixedAudioRenderReport {
    output_meter: MeterReading,
    snapshot: RealtimeMixerSnapshot,
    frames: usize,
    churn: RenderChurnSnapshot,
    strip_mix_diagnostics: Vec<RenderProbeStripMixDiagnostics>,
}

#[derive(Debug, Clone)]
pub struct MixerControlState {
    pub settings: MixerSettings,
    pub strip_controls: BTreeMap<StripId, MixerStripControls>,
}

impl Default for MixerControlState {
    fn default() -> Self {
        Self {
            settings: MixerSettings::default(),
            strip_controls: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RealtimeMixerSnapshot {
    pub output_meter: MeterReading,
    pub strips: BTreeMap<StripId, RealtimeStripSnapshot>,
}

#[derive(Debug, Clone, Default)]
struct SnapshotHistory {
    latest: RealtimeMixerSnapshot,
    frames: VecDeque<SnapshotFrame>,
}

#[derive(Debug, Clone)]
struct SnapshotFrame {
    rendered_frame_end: u64,
    snapshot: RealtimeMixerSnapshot,
}

impl SnapshotHistory {
    fn clear(&mut self) {
        self.latest = RealtimeMixerSnapshot::default();
        self.frames.clear();
    }

    fn record(
        &mut self,
        rendered_frame_end: u64,
        snapshot: RealtimeMixerSnapshot,
        sample_rate: u32,
    ) {
        self.latest = snapshot.clone();
        self.frames.push_back(SnapshotFrame {
            rendered_frame_end,
            snapshot,
        });
        let retention_frames = sample_rate.max(1) as u64 * 4;
        let cutoff = rendered_frame_end.saturating_sub(retention_frames);
        while self
            .frames
            .front()
            .is_some_and(|entry| entry.rendered_frame_end < cutoff)
        {
            self.frames.pop_front();
        }
    }

    fn live_snapshot(&self) -> RealtimeMixerSnapshot {
        self.latest.clone()
    }

    fn delayed_snapshot(&self, rendered_frames: u64, latency_frames: u64) -> RealtimeMixerSnapshot {
        let target_frame = rendered_frames.saturating_sub(latency_frames);
        for entry in self.frames.iter().rev() {
            if entry.rendered_frame_end <= target_frame {
                return entry.snapshot.clone();
            }
        }
        if self.frames.is_empty() {
            self.latest.clone()
        } else {
            RealtimeMixerSnapshot::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeStripSnapshot {
    pub soundfont_id: String,
    pub midi_channel: u8,
    pub midi_bank: u16,
    pub midi_program: u8,
    pub is_percussion: bool,
    pub display_name: Option<String>,
    pub meter: MeterReading,
    pub audible: bool,
    pub active_notes: Vec<u8>,
}

fn render_mixed_audio(
    engine: &mut MultiSoundFontPlayback,
    mixer: &mut MixerEngine,
    scratch: &mut PlaybackRenderScratch,
    controls: &MixerControlState,
    final_output_gain: f32,
    soundfont_ids: &[String],
    midi_strips: &[MidiStripDescriptor],
    output: &mut [f32],
) -> Result<MixedAudioRenderReport> {
    let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::PlaybackRender);
    mixer.set_settings(controls.settings);
    output.fill(0.0);
    let frames = output.len() / 2;
    if frames == 0 {
        return Ok(MixedAudioRenderReport {
            output_meter: MeterReading::from_interleaved(output),
            snapshot: RealtimeMixerSnapshot::default(),
            frames: 0,
            churn: RenderChurnSnapshot::default(),
            strip_mix_diagnostics: Vec::new(),
        });
    }

    let soundfont_gain = 1.0 / engine.instance_count().max(1) as f32;
    scratch.begin_render();
    let playback_stem_bytes_before = stem_blocks_total_bytes(&scratch.blocks);
    {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::RenderStems);
        engine.render_channel_program_stem_blocks_into(frames, &mut scratch.blocks);
    }
    let stem_playback_growth_bytes =
        stem_blocks_total_bytes(&scratch.blocks).saturating_sub(playback_stem_bytes_before);
    let stem_allocations = engine.last_stem_render_allocations();
    let stem_audio_bytes = stem_blocks_audio_bytes(&scratch.blocks);
    let stem_tracked_alloc_bytes = stem_allocations.total_bytes();
    let stem_total_growth_bytes = stem_allocations
        .total_growth_bytes()
        .saturating_add(stem_playback_growth_bytes);
    let stem_untracked_alloc_bytes = stem_tracked_alloc_bytes.saturating_sub(stem_audio_bytes);
    scratch.soundfont_slots.extend(
        soundfont_ids
            .iter()
            .enumerate()
            .map(|(index, soundfont_id)| (soundfont_id.clone(), index)),
    );
    for (block_index, block) in scratch.blocks.iter().enumerate() {
        let next_slot = scratch.soundfont_slots.len();
        let soundfont_slot = *scratch
            .soundfont_slots
            .entry(block.identity.soundfont_id.clone())
            .or_insert(next_slot);
        let strip_id = stem_strip_id(soundfont_slot, &block.identity);
        let is_global_residual = is_global_residual_stem(&block.identity);
        scratch.rendered_routes.push(RoutedStemRoute {
            block_index,
            soundfont_slot,
            strip_id,
            is_global_residual,
        });
    }
    let rendered_blocks = scratch.rendered_routes.len();
    scratch
        .rendered_strip_ids
        .extend(scratch.rendered_routes.iter().map(|route| route.strip_id));

    for (soundfont_slot, soundfont_id) in soundfont_ids.iter().enumerate() {
        for descriptor in midi_strips {
            let identity = StemIdentity::channel_bank_program(
                soundfont_id.clone(),
                descriptor.channel,
                Some(descriptor.bank),
                descriptor.program,
                descriptor.is_percussion,
            );
            let strip_id = stem_strip_id(soundfont_slot, &identity);
            let controls = controls
                .strip_controls
                .get(&strip_id)
                .copied()
                .unwrap_or_default();
            scratch
                .visual_controls_by_soundfont
                .entry(soundfont_slot)
                .or_default()
                .push(controls);
            scratch.visual_control_ids.insert(strip_id);
            if !scratch.rendered_strip_ids.contains(&strip_id) {
                scratch.visual_metadata.insert(
                    strip_id,
                    RealtimeStripSnapshot {
                        soundfont_id: identity.soundfont_id.clone(),
                        midi_channel: identity.midi_channel.unwrap_or(0),
                        midi_bank: identity.midi_bank.unwrap_or(0),
                        midi_program: identity.midi_program.unwrap_or(0),
                        is_percussion: identity.is_percussion,
                        display_name: None,
                        meter: MeterReading::default(),
                        audible: false,
                        active_notes: Vec::new(),
                    },
                );
                scratch.silence_candidates.push(SilenceStemRoute {
                    identity,
                    strip_id,
                    controls,
                });
            }
        }
    }

    for route in &scratch.rendered_routes {
        if !route.is_global_residual && scratch.visual_control_ids.insert(route.strip_id) {
            scratch
                .visual_controls_by_soundfont
                .entry(route.soundfont_slot)
                .or_default()
                .push(
                    controls
                        .strip_controls
                        .get(&route.strip_id)
                        .copied()
                        .unwrap_or_default(),
                );
        }

        let identity = route.identity(&scratch.blocks);
        let block_view = route.as_mixer_view(&scratch.blocks, soundfont_gain);
        let meter = block_view.meter();
        scratch.visual_metadata.insert(
            route.strip_id,
            RealtimeStripSnapshot {
                soundfont_id: identity.soundfont_id.clone(),
                midi_channel: identity.midi_channel.unwrap_or(0),
                midi_bank: identity.midi_bank.unwrap_or(0),
                midi_program: identity.midi_program.unwrap_or(0),
                is_percussion: identity.is_percussion,
                display_name: route.display_name(&scratch.blocks).map(str::to_owned),
                meter,
                audible: meter.peak > f32::EPSILON,
                active_notes: route.active_notes(&scratch.blocks).to_vec(),
            },
        );
    }

    let zero_fill_blocks = scratch.silence_candidates.len();
    let zero_fill_audio_bytes = 0;
    let mixer_frame_bytes = 0usize;
    for (route_index, route) in scratch.rendered_routes.iter().enumerate() {
        let identity = route.identity(&scratch.blocks);
        let mixer_identity = MixerStripIdentity {
            strip_id: route.strip_id,
            soundfont_id: SoundFontId::new(identity.soundfont_id.clone()),
            midi_channel: identity.midi_channel.unwrap_or(0),
            midi_program: identity.midi_program.unwrap_or(0),
            is_percussion: identity.is_percussion,
        };
        scratch.mixer_input_routes.push(MixerInputRoute {
            rendered_route_index: Some(route_index),
            identity: mixer_identity,
            controls: if route.is_global_residual {
                residual_controls_for_soundfont(
                    &scratch.visual_controls_by_soundfont,
                    route.soundfont_slot,
                )
            } else {
                controls
                    .strip_controls
                    .get(&route.strip_id)
                    .copied()
                    .unwrap_or_default()
            },
            automatic_processing: !route.is_global_residual,
        });
    }

    for candidate in &scratch.silence_candidates {
        if !candidate.controls.solo
            && !mixer.strip_has_effect_tail(candidate.strip_id, candidate.controls)
        {
            continue;
        }
        scratch.mixer_input_routes.push(MixerInputRoute {
            rendered_route_index: None,
            identity: MixerStripIdentity {
                strip_id: candidate.strip_id,
                soundfont_id: SoundFontId::new(candidate.identity.soundfont_id.clone()),
                midi_channel: candidate.identity.midi_channel.unwrap_or(0),
                midi_program: candidate.identity.midi_program.unwrap_or(0),
                is_percussion: candidate.identity.is_percussion,
            },
            controls: candidate.controls,
            automatic_processing: true,
        });
    }

    let visual_strips = scratch.visual_metadata.len();
    let routed_blocks = scratch.mixer_input_routes.len();
    let report = {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::RenderMixer);
        mixer.mix_generated_views_interleaved(routed_blocks, output, |index| {
            let route = &scratch.mixer_input_routes[index];
            let block = route
                .rendered_route_index
                .map(|route_index| {
                    scratch.rendered_routes[route_index]
                        .as_mixer_view(&scratch.blocks, soundfont_gain)
                })
                .unwrap_or(AudioBlockView::Silence {
                    frame_count: frames,
                });
            MixerStripInputView {
                identity: route.identity.clone(),
                controls: route.controls,
                automatic_processing: route.automatic_processing,
                block,
            }
        })?
    };
    let strip_mix_diagnostics = build_strip_mix_diagnostics(
        &report.strips,
        &scratch.mixer_input_routes,
        &scratch.rendered_routes,
        &scratch.blocks,
    );
    let final_output_gain = final_output_gain.clamp(0.0, FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER);
    if (final_output_gain - 1.0).abs() > f32::EPSILON {
        for sample in output.iter_mut() {
            *sample *= final_output_gain;
        }
    }
    let output_meter = MeterReading::from_interleaved(output);
    let mixer_internal_alloc_bytes = report.allocation_stats.allocated_bytes;
    let mixer_internal_dealloc_bytes = report.allocation_stats.deallocated_bytes;
    let mixer_scratch_growth_bytes = report.allocation_stats.scratch_growth_bytes;
    let mixer_output_bytes = report.allocation_stats.output_bytes;
    let mixer_prepared_frame_bytes = report.allocation_stats.prepared_rendered_frame_bytes;
    let mixer_report_bytes = report.allocation_stats.reports_bytes;
    let processed_mixer_inputs = report.allocation_stats.processed_inputs;
    let mixer_allocation_events = report.allocation_stats.allocation_events;
    let mixer_deallocation_events = report.allocation_stats.deallocation_events;
    let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::RenderSnapshot);
    let mut snapshot = RealtimeMixerSnapshot {
        output_meter,
        strips: BTreeMap::new(),
    };
    snapshot.strips.append(&mut scratch.visual_metadata);
    for strip in &report.strips {
        if let Some(visual) = snapshot.strips.get_mut(&strip.identity.strip_id) {
            visual.audible = strip.audible && visual.meter.peak > f32::EPSILON;
        }
    }
    let routed_block_overhead = scratch
        .rendered_routes
        .capacity()
        .saturating_mul(mem::size_of::<RoutedStemRoute>());
    let mixer_input_overhead = scratch
        .mixer_input_routes
        .capacity()
        .saturating_mul(mem::size_of::<MixerInputRoute>());
    let vec_overhead_bytes = routed_block_overhead.saturating_add(mixer_input_overhead);
    let retained_container_capacity_bytes = vec_overhead_bytes;
    let temp_container_alloc_bytes = 0;
    let transient_alloc_bytes = stem_audio_bytes
        .saturating_add(stem_untracked_alloc_bytes)
        .saturating_add(zero_fill_audio_bytes)
        .saturating_add(mixer_frame_bytes)
        .saturating_add(mixer_internal_alloc_bytes)
        .saturating_add(vec_overhead_bytes);
    let transient_dealloc_bytes = transient_alloc_bytes;
    let retained_capacity_growth_bytes =
        stem_total_growth_bytes.saturating_add(mixer_scratch_growth_bytes);
    let transient_growth_bytes = retained_capacity_growth_bytes;
    let allocation_events = rendered_blocks
        .saturating_mul(2)
        .saturating_add(routed_blocks)
        .saturating_add(2)
        .saturating_add(mixer_allocation_events);
    let deallocation_events = allocation_events
        .saturating_sub(mixer_allocation_events)
        .saturating_add(mixer_deallocation_events);
    Ok(MixedAudioRenderReport {
        output_meter,
        snapshot,
        frames,
        churn: RenderChurnSnapshot {
            allocation_events,
            deallocation_events,
            frames,
            rendered_blocks,
            routed_blocks,
            mixer_inputs: processed_mixer_inputs,
            zero_fill_blocks,
            visual_strips,
            stem_audio_bytes,
            stem_output_blocks: stem_allocations.output_block_count,
            stem_output_audio_bytes: stem_allocations.output_audio_bytes,
            stem_output_active_note_bytes: stem_allocations.output_active_note_bytes,
            stem_output_vec_bytes: stem_allocations.output_vec_bytes,
            stem_internal_blocks: stem_allocations.internal_block_count,
            stem_internal_audio_bytes: stem_allocations.internal_audio_bytes,
            stem_internal_active_note_bytes: stem_allocations.internal_active_note_bytes,
            stem_internal_vec_bytes: stem_allocations.internal_vec_bytes,
            stem_residual_buffer_bytes: stem_allocations.residual_buffer_bytes,
            stem_effect_input_bytes: stem_allocations.effect_input_bytes,
            stem_effect_input_vec_bytes: stem_allocations.effect_input_vec_bytes,
            stem_output_growth_bytes: stem_allocations.output_growth_bytes,
            stem_internal_growth_bytes: stem_allocations.internal_growth_bytes,
            stem_residual_growth_bytes: stem_allocations.residual_growth_bytes,
            stem_effect_input_growth_bytes: stem_allocations.effect_input_growth_bytes,
            stem_playback_growth_bytes,
            stem_total_growth_bytes,
            stem_tracked_alloc_bytes,
            stem_untracked_alloc_bytes,
            zero_fill_audio_bytes,
            mixer_frame_bytes,
            mixer_internal_alloc_bytes,
            mixer_internal_dealloc_bytes,
            mixer_scratch_growth_bytes,
            mixer_output_bytes,
            mixer_prepared_frame_bytes,
            mixer_report_bytes,
            vec_overhead_bytes,
            retained_container_capacity_bytes,
            temp_container_alloc_bytes,
            retained_capacity_growth_bytes,
            transient_alloc_bytes,
            transient_dealloc_bytes,
            transient_growth_bytes,
        },
        strip_mix_diagnostics,
    })
}

fn build_strip_mix_diagnostics(
    strip_reports: &[StripMixReport],
    mixer_input_routes: &[MixerInputRoute],
    rendered_routes: &[RoutedStemRoute],
    blocks: &[StemRenderBlock],
) -> Vec<RenderProbeStripMixDiagnostics> {
    strip_reports
        .iter()
        .zip(mixer_input_routes)
        .map(|(report, route)| {
            let (left_pan_gain, right_pan_gain) = route.controls.pan_gains();
            let session_gain = route.controls.session_gain(report.solo_active);
            let (display_name, active_notes) = route
                .rendered_route_index
                .map(|index| {
                    let rendered_route = &rendered_routes[index];
                    (
                        rendered_route.display_name(blocks).map(str::to_owned),
                        rendered_route.active_notes(blocks).to_vec(),
                    )
                })
                .unwrap_or((None, Vec::new()));
            RenderProbeStripMixDiagnostics {
                strip_id: report.identity.strip_id,
                soundfont_id: report.identity.soundfont_id.as_str().to_owned(),
                display_name,
                midi_channel: report.identity.midi_channel,
                midi_program: report.identity.midi_program,
                is_percussion: report.identity.is_percussion,
                input_meter: report.input_meter,
                estimated_post_peak: report.input_meter.peak * report.applied_gain,
                estimated_post_rms: report.input_meter.rms * report.applied_gain,
                applied_gain: report.applied_gain,
                session_gain,
                smart_mix_gain: report.smart_mix_gain,
                lookahead_gain: report.lookahead_gain,
                normalization_gain: report.normalization_gain,
                left_pan_gain,
                right_pan_gain,
                automatic_processing: route.automatic_processing,
                mute: route.controls.mute,
                solo: route.controls.solo,
                audible: report.audible,
                active_notes,
            }
        })
        .collect()
}

fn output_latency_breakdown(audio: Option<&AudioDeviceDiagnostics>) -> OutputLatencyBreakdown {
    let Some(audio) = audio else {
        return OutputLatencyBreakdown::default();
    };

    if audio.ring_retained_bytes > 0 {
        let device_queue_frames = audio
            .device_buffer_frames
            .saturating_sub(audio.device_available_frames) as u64;
        let wrapper_queue_frames = audio.ring_available_frames as u64;
        OutputLatencyBreakdown {
            total_frames: wrapper_queue_frames.saturating_add(device_queue_frames),
            wrapper_queue_frames,
            device_queue_frames,
        }
    } else {
        let total_frames = (audio
            .ring_capacity_frames
            .saturating_sub(audio.ring_available_frames)) as u64;
        OutputLatencyBreakdown {
            total_frames,
            wrapper_queue_frames: total_frames,
            device_queue_frames: 0,
        }
    }
}

fn frames_from_ms(sample_rate: u32, milliseconds: u32) -> u32 {
    ((u64::from(sample_rate.max(1)) * u64::from(milliseconds)) / 1000)
        .max(1)
        .min(u64::from(u32::MAX)) as u32
}

impl From<StemRenderAllocationDebug> for RenderChurnSnapshot {
    fn from(allocations: StemRenderAllocationDebug) -> Self {
        Self {
            stem_output_blocks: allocations.output_block_count,
            stem_output_audio_bytes: allocations.output_audio_bytes,
            stem_output_active_note_bytes: allocations.output_active_note_bytes,
            stem_output_vec_bytes: allocations.output_vec_bytes,
            stem_internal_blocks: allocations.internal_block_count,
            stem_internal_audio_bytes: allocations.internal_audio_bytes,
            stem_internal_active_note_bytes: allocations.internal_active_note_bytes,
            stem_internal_vec_bytes: allocations.internal_vec_bytes,
            stem_residual_buffer_bytes: allocations.residual_buffer_bytes,
            stem_effect_input_bytes: allocations.effect_input_bytes,
            stem_effect_input_vec_bytes: allocations.effect_input_vec_bytes,
            stem_output_growth_bytes: allocations.output_growth_bytes,
            stem_internal_growth_bytes: allocations.internal_growth_bytes,
            stem_residual_growth_bytes: allocations.residual_growth_bytes,
            stem_effect_input_growth_bytes: allocations.effect_input_growth_bytes,
            stem_total_growth_bytes: allocations.total_growth_bytes(),
            stem_tracked_alloc_bytes: allocations.total_bytes(),
            ..Self::default()
        }
    }
}

fn stem_blocks_audio_bytes(blocks: &[StemRenderBlock]) -> usize {
    blocks
        .iter()
        .map(|block| {
            block
                .left
                .capacity()
                .saturating_add(block.right.capacity())
                .saturating_mul(mem::size_of::<f32>())
        })
        .sum()
}

fn stem_blocks_total_bytes(blocks: &Vec<StemRenderBlock>) -> usize {
    blocks
        .capacity()
        .saturating_mul(mem::size_of::<StemRenderBlock>())
        .saturating_add(stem_blocks_audio_bytes(blocks))
        .saturating_add(
            blocks
                .iter()
                .map(|block| block.active_notes.capacity() * mem::size_of::<u8>())
                .sum::<usize>(),
        )
}

#[derive(Debug, Default)]
struct PlaybackRenderScratch {
    blocks: Vec<StemRenderBlock>,
    rendered_routes: Vec<RoutedStemRoute>,
    mixer_input_routes: Vec<MixerInputRoute>,
    rendered_strip_ids: BTreeSet<StripId>,
    soundfont_slots: BTreeMap<String, usize>,
    visual_metadata: BTreeMap<StripId, RealtimeStripSnapshot>,
    visual_control_ids: BTreeSet<StripId>,
    visual_controls_by_soundfont: BTreeMap<usize, Vec<MixerStripControls>>,
    silence_candidates: Vec<SilenceStemRoute>,
}

impl PlaybackRenderScratch {
    fn begin_render(&mut self) {
        self.rendered_routes.clear();
        self.mixer_input_routes.clear();
        self.rendered_strip_ids.clear();
        self.soundfont_slots.clear();
        self.visual_metadata.clear();
        self.visual_control_ids.clear();
        for controls in self.visual_controls_by_soundfont.values_mut() {
            controls.clear();
        }
        self.visual_controls_by_soundfont.clear();
        self.silence_candidates.clear();
    }

    fn release_capacity(&mut self) {
        self.blocks.clear();
        self.blocks.shrink_to_fit();
        self.rendered_routes.clear();
        self.rendered_routes.shrink_to_fit();
        self.mixer_input_routes.clear();
        self.mixer_input_routes.shrink_to_fit();
        self.rendered_strip_ids.clear();
        self.soundfont_slots.clear();
        self.visual_metadata.clear();
        self.visual_control_ids.clear();
        self.visual_controls_by_soundfont.clear();
        self.silence_candidates.clear();
        self.silence_candidates.shrink_to_fit();
    }
}

#[derive(Debug)]
struct RoutedStemRoute {
    block_index: usize,
    soundfont_slot: usize,
    strip_id: StripId,
    is_global_residual: bool,
}

#[derive(Debug)]
struct MixerInputRoute {
    rendered_route_index: Option<usize>,
    identity: MixerStripIdentity,
    controls: MixerStripControls,
    automatic_processing: bool,
}

#[derive(Debug)]
struct SilenceStemRoute {
    identity: StemIdentity,
    strip_id: StripId,
    controls: MixerStripControls,
}

impl RoutedStemRoute {
    fn block<'a>(&self, blocks: &'a [StemRenderBlock]) -> &'a StemRenderBlock {
        &blocks[self.block_index]
    }

    fn identity<'a>(&self, blocks: &'a [StemRenderBlock]) -> &'a StemIdentity {
        &self.block(blocks).identity
    }

    fn display_name<'a>(&self, blocks: &'a [StemRenderBlock]) -> Option<&'a str> {
        self.block(blocks).display_name.as_deref()
    }

    fn active_notes<'a>(&self, blocks: &'a [StemRenderBlock]) -> &'a [u8] {
        &self.block(blocks).active_notes
    }

    fn as_mixer_view<'a>(
        &self,
        blocks: &'a [StemRenderBlock],
        soundfont_gain: f32,
    ) -> AudioBlockView<'a> {
        let block = self.block(blocks);
        AudioBlockView::Split {
            left: &block.left,
            right: &block.right,
            gain: soundfont_gain,
        }
    }
}

fn residual_controls_for_soundfont(
    visual_controls_by_soundfont: &BTreeMap<usize, Vec<MixerStripControls>>,
    soundfont_slot: usize,
) -> MixerStripControls {
    let Some(visible_controls) = visual_controls_by_soundfont.get(&soundfont_slot) else {
        return MixerStripControls::default();
    };

    if visible_controls.is_empty() {
        return MixerStripControls::default();
    }

    let muted = visible_controls.iter().any(|controls| controls.mute);
    let solo_count = visible_controls
        .iter()
        .filter(|controls| controls.solo)
        .count();
    let partial_solo = solo_count > 0 && solo_count < visible_controls.len();
    let row_solo = solo_count == visible_controls.len();

    MixerStripControls {
        mute: muted || partial_solo,
        solo: row_solo,
        ..MixerStripControls::default()
    }
}

fn stem_strip_id(font_index: usize, identity: &StemIdentity) -> StripId {
    let soundfont_slot = (font_index as u64 + 1) * 100_000;
    let channel_slot = identity.midi_channel.unwrap_or(16) as u64 * 16_384;
    let bank_slot = identity.midi_bank.unwrap_or(0) as u64 * 128;
    let program_slot = identity.midi_program.unwrap_or(127) as u64;
    let percussion_slot = if identity.is_percussion { 50_000 } else { 0 };
    StripId(soundfont_slot + percussion_slot + channel_slot + bank_slot + program_slot + 1)
}

fn is_global_residual_stem(identity: &StemIdentity) -> bool {
    identity.midi_channel.is_none() && identity.midi_program.is_none() && !identity.is_percussion
}

fn soundfont_catalog_entry_string_bytes(entry: &SoundFontCatalogEntry) -> usize {
    entry
        .internal_id
        .capacity()
        .saturating_add(entry.display_name.capacity())
        .saturating_add(entry.source_format.capacity())
        .saturating_add(entry.storage_format.capacity())
        .saturating_add(entry.runtime_format.capacity())
}

fn coverage_estimated_bytes(coverage: &SoundFontCoverage) -> usize {
    let preset_name_vec_bytes =
        coverage.metadata.preset_names.capacity() * mem::size_of::<String>();
    let preset_name_bytes = coverage
        .metadata
        .preset_names
        .iter()
        .map(String::capacity)
        .sum::<usize>();
    let melodic_bytes = coverage
        .melodic
        .bank_programs
        .iter()
        .map(mem::size_of_val)
        .sum::<usize>();
    let percussion_bytes = coverage
        .percussion
        .key_ranges
        .iter()
        .map(mem::size_of_val)
        .sum::<usize>();
    mem::size_of::<SoundFontCoverage>()
        .saturating_add(preset_name_vec_bytes)
        .saturating_add(preset_name_bytes)
        .saturating_add(melodic_bytes)
        .saturating_add(percussion_bytes)
}

#[derive(Debug, Clone, Default)]
pub struct MidiTransportMetadata {
    pub duration_seconds: f64,
    pub tick_length: u64,
    pub jump_start_tick: Option<u64>,
    pub jump_end_tick: Option<u64>,
    pub jump_points: Vec<u64>,
}

impl MidiTransportMetadata {
    fn from_loaded_midi(loaded_midi: &LoadedMidi) -> Self {
        let mut jump_points = loaded_midi.loop_start_ticks();
        jump_points.extend(loaded_midi.loop_end_ticks());
        jump_points.sort_unstable();
        jump_points.dedup();

        let jump_start_tick = loaded_midi.loop_start_ticks().into_iter().next();
        let jump_end_tick = loaded_midi.loop_end_ticks().into_iter().next();

        Self {
            duration_seconds: loaded_midi.duration_seconds(),
            tick_length: loaded_midi.tick_length(),
            jump_start_tick,
            jump_end_tick,
            jump_points,
        }
    }
}

pub struct RenderProbeReport {
    pub frames: usize,
    pub samples: usize,
    pub peak: f32,
    pub soundfont_count: usize,
    pub midi_strip_count: usize,
    pub recovered_error_count: usize,
}

pub struct RenderStemProbeReport {
    pub report: RenderProbeReport,
    pub stems: Vec<StemRenderBlock>,
    pub mix_diagnostics: Vec<RenderProbeStripMixDiagnostics>,
}

#[derive(Debug, Clone)]
pub struct RenderProbeStripMixDiagnostics {
    pub strip_id: StripId,
    pub soundfont_id: String,
    pub display_name: Option<String>,
    pub midi_channel: u8,
    pub midi_program: u8,
    pub is_percussion: bool,
    pub input_meter: MeterReading,
    pub estimated_post_peak: f32,
    pub estimated_post_rms: f32,
    pub applied_gain: f32,
    pub session_gain: f32,
    pub smart_mix_gain: f32,
    pub lookahead_gain: f32,
    pub normalization_gain: f32,
    pub left_pan_gain: f32,
    pub right_pan_gain: f32,
    pub automatic_processing: bool,
    pub mute: bool,
    pub solo: bool,
    pub audible: bool,
    pub active_notes: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackMidiScanDiagnostics {
    pub loop_style: String,
    pub system_modes: Vec<String>,
    pub percussion_channels: Vec<u8>,
    pub warnings: Vec<String>,
    pub sysex_event_count: usize,
    pub recognized_sysex_event_count: usize,
    pub sysex_events: Vec<PlaybackMidiSysexEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackMidiSysexEvent {
    pub index: usize,
    pub status: u8,
    pub byte_len: usize,
    pub manufacturer_id: String,
    pub recognized: bool,
    pub system_mode: Option<String>,
    pub channel_role: Option<PlaybackMidiChannelRoleChange>,
    pub warning: Option<String>,
    pub bytes_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackMidiChannelRoleChange {
    pub channel: u8,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderErrorTraceEvent {
    pub source: String,
    pub failure_kind: String,
    pub detail: String,
    pub backtrace: Option<String>,
    pub frames_requested: usize,
    pub midi_source: Option<String>,
    pub soundfont_ids: Vec<String>,
    pub midi_scan: PlaybackMidiScanDiagnostics,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MidiStripDescriptor {
    pub channel: u8,
    pub bank: u16,
    pub program: u8,
    pub is_percussion: bool,
}

impl Default for MidiStripDescriptor {
    fn default() -> Self {
        Self {
            channel: 0,
            bank: 0,
            program: 0,
            is_percussion: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiDemandProfile {
    pub channel_roles: Vec<MidiStripDescriptor>,
    pub melodic_programs: Vec<BankProgramDemand>,
    pub percussion_channels: Vec<u8>,
}

impl MidiDemandProfile {
    fn from_channel_roles(channel_roles: Vec<MidiStripDescriptor>) -> Self {
        let melodic_programs = channel_roles
            .iter()
            .filter(|role| !role.is_percussion)
            .map(|role| BankProgramDemand {
                bank: role.bank,
                program: role.program,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let percussion_channels = channel_roles
            .iter()
            .filter(|role| role.is_percussion)
            .map(|role| role.channel)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        Self {
            channel_roles,
            melodic_programs,
            percussion_channels,
        }
    }

    pub fn role_count(&self) -> usize {
        self.channel_roles.len()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BankProgramDemand {
    pub bank: u16,
    pub program: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontDemandSignature {
    pub soundfont_id: String,
    pub preset_ids: Vec<u32>,
    pub instrument_ids: Vec<u32>,
    pub sample_ids: Vec<u32>,
    pub logical_wave_range_count: usize,
    pub full_font_fallback: bool,
}

impl SoundFontDemandSignature {
    fn from_subset_plan(plan: &SoundFontSubsetPlanDebug) -> Self {
        Self {
            soundfont_id: plan.soundfont_id.clone(),
            preset_ids: plan.preset_ids.clone(),
            instrument_ids: plan.instrument_ids.clone(),
            sample_ids: plan.sample_ids.clone(),
            logical_wave_range_count: plan.logical_wave_range_count,
            full_font_fallback: !plan.used_index_json,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontDemandDiagnostics {
    pub requested_soundfont_count: usize,
    pub loaded_provider_count: usize,
    pub pruned_soundfont_count: usize,
    pub demand_profile: MidiDemandProfile,
}

impl MidiStripDescriptor {
    #[allow(dead_code)]
    fn strip_id_for_soundfont(self, soundfont_index: usize) -> StripId {
        let soundfont_slot = (soundfont_index as u64 + 1) * 10_000;
        let channel_slot = self.channel as u64 * 16_384;
        let bank_slot = self.bank as u64 * 128;
        StripId(soundfont_slot + channel_slot + bank_slot + self.program as u64 + 1)
    }
}

#[derive(Debug, Clone)]
struct MidiFileScan {
    detected_loop_type: MidiFileLoopType,
    channel_roles: Vec<MidiStripDescriptor>,
    demand_profile: MidiDemandProfile,
    diagnostics: PlaybackMidiScanDiagnostics,
}

#[derive(Debug, Copy, Clone, Default)]
struct LoopMarkerStats {
    rpg_start_count: u32,
    im_start_count: u32,
    im_end_count: u32,
    ff_start_count: u32,
    ff_end_count: u32,
}

impl LoopMarkerStats {
    fn note_cc(&mut self, controller: u8) {
        match controller {
            110 => self.im_start_count += 1,
            111 => {
                self.rpg_start_count += 1;
                self.im_end_count += 1;
            }
            116 => self.ff_start_count += 1,
            117 => self.ff_end_count += 1,
            _ => {}
        }
    }

    fn detected_loop_type(self) -> MidiFileLoopType {
        let im_pair_count = self.im_start_count.min(self.im_end_count);
        let ff_pair_count = self.ff_start_count.min(self.ff_end_count);

        if im_pair_count > 0 && im_pair_count >= ff_pair_count {
            MidiFileLoopType::IncredibleMachine
        } else if ff_pair_count > 0 {
            MidiFileLoopType::FinalFantasy
        } else if self.rpg_start_count > 0 {
            MidiFileLoopType::RpgMaker
        } else {
            MidiFileLoopType::LoopPoint(0)
        }
    }
}

fn loop_type_label(loop_type: MidiFileLoopType) -> &'static str {
    match loop_type {
        MidiFileLoopType::LoopPoint(_) => "none",
        MidiFileLoopType::RpgMaker => "rpg-maker",
        MidiFileLoopType::IncredibleMachine => "incredible-machine",
        MidiFileLoopType::FinalFantasy => "final-fantasy",
        _ => "other",
    }
}

fn analyze_midi_file(bytes: &[u8]) -> Result<MidiFileScan> {
    if bytes.len() < 14 || !bytes.starts_with(b"MThd") {
        return Err(FlutzError::InvalidInput(
            "missing standard MIDI MThd header".to_owned(),
        ));
    }

    let header_len = read_be_u32(bytes, 4)? as usize;
    if header_len < 6 || bytes.len() < 8 + header_len {
        return Err(FlutzError::InvalidInput(
            "invalid MIDI header length".to_owned(),
        ));
    }
    let track_count = read_be_u16(bytes, 10)? as usize;
    let mut offset = 8 + header_len;
    let mut loop_markers = LoopMarkerStats::default();

    for _ in 0..track_count {
        if offset + 8 > bytes.len() {
            break;
        }
        if &bytes[offset..offset + 4] != b"MTrk" {
            return Err(FlutzError::InvalidInput(
                "expected MIDI MTrk chunk".to_owned(),
            ));
        }
        let track_len = read_be_u32(bytes, offset + 4)? as usize;
        offset += 8;
        let track_end = offset.saturating_add(track_len).min(bytes.len());
        analyze_track(&bytes[offset..track_end], &mut loop_markers)?;
        offset = track_end;
    }

    let detected_loop_type = loop_markers.detected_loop_type();
    let mut cursor = Cursor::new(bytes);
    let midi_file =
        MidiFile::new_with_loop_type(&mut cursor, detected_loop_type).map_err(|error| {
            FlutzError::InvalidInput(format!("failed to parse MIDI roles: {error:?}"))
        })?;
    let channel_roles = midi_file
        .get_channel_program_roles()
        .into_iter()
        .map(|role| MidiStripDescriptor {
            channel: role.channel,
            bank: role.bank,
            program: role.program,
            is_percussion: role.is_percussion,
        })
        .collect::<Vec<_>>();
    let demand_profile = MidiDemandProfile::from_channel_roles(channel_roles.clone());
    let diagnostics =
        PlaybackMidiScanDiagnostics::from_parts(detected_loop_type, midi_file.get_interpretation());

    Ok(MidiFileScan {
        detected_loop_type,
        channel_roles,
        demand_profile,
        diagnostics,
    })
}

impl PlaybackMidiScanDiagnostics {
    fn from_parts(loop_type: MidiFileLoopType, interpretation: &MidiInterpretation) -> Self {
        Self {
            loop_style: loop_type_label(loop_type).to_owned(),
            system_modes: interpretation
                .system_modes
                .iter()
                .map(|mode| midi_system_mode_label(*mode).to_owned())
                .collect(),
            percussion_channels: interpretation.percussion_channels.iter().copied().collect(),
            warnings: interpretation.warnings.clone(),
            sysex_event_count: interpretation.sysex_event_count,
            recognized_sysex_event_count: interpretation.recognized_sysex_event_count,
            sysex_events: interpretation
                .sysex_events
                .iter()
                .map(|event| PlaybackMidiSysexEvent {
                    index: event.index,
                    status: event.status,
                    byte_len: event.byte_len,
                    manufacturer_id: event.manufacturer_id.clone(),
                    recognized: event.recognized,
                    system_mode: event
                        .system_mode
                        .map(|mode| midi_system_mode_label(mode).to_owned()),
                    channel_role: event.channel_role.map(|(channel, role)| {
                        PlaybackMidiChannelRoleChange {
                            channel,
                            role: format!("{:?}", role).to_ascii_lowercase(),
                        }
                    }),
                    warning: event.warning.clone(),
                    bytes_hex: event.bytes_hex.clone(),
                })
                .collect(),
        }
    }
}

fn midi_system_mode_label(mode: MidiSystemMode) -> &'static str {
    match mode {
        MidiSystemMode::GeneralMidi => "general-midi",
        MidiSystemMode::GeneralMidi2 => "general-midi-2",
        MidiSystemMode::RolandGs => "roland-gs",
        MidiSystemMode::YamahaXg => "yamaha-xg",
    }
}

struct RenderFailureContext<'a> {
    source: &'static str,
    frames_requested: usize,
    midi_source: Option<&'a Path>,
    midi_scan: &'a PlaybackMidiScanDiagnostics,
}

fn render_mixed_audio_guarded(
    engine: &mut MultiSoundFontPlayback,
    mixer: &mut MixerEngine,
    scratch: &mut PlaybackRenderScratch,
    controls: &MixerControlState,
    final_output_gain: f32,
    soundfont_ids: &[String],
    midi_strips: &[MidiStripDescriptor],
    output: &mut [f32],
    trace: Option<&Arc<Mutex<RenderErrorTrace>>>,
    context: RenderFailureContext<'_>,
) -> Option<MixedAudioRenderReport> {
    match panic::catch_unwind(AssertUnwindSafe(|| {
        render_mixed_audio(
            engine,
            mixer,
            scratch,
            controls,
            final_output_gain,
            soundfont_ids,
            midi_strips,
            output,
        )
    })) {
        Ok(Ok(report)) => Some(report),
        Ok(Err(error)) => {
            handle_recovered_render_failure(
                engine,
                mixer,
                scratch,
                output,
                trace,
                RenderErrorTraceEvent {
                    source: context.source.to_owned(),
                    failure_kind: "error".to_owned(),
                    detail: error.to_string(),
                    backtrace: None,
                    frames_requested: context.frames_requested,
                    midi_source: context.midi_source.map(|path| path.display().to_string()),
                    soundfont_ids: soundfont_ids.to_vec(),
                    midi_scan: context.midi_scan.clone(),
                },
            );
            None
        }
        Err(payload) => {
            handle_recovered_render_failure(
                engine,
                mixer,
                scratch,
                output,
                trace,
                RenderErrorTraceEvent {
                    source: context.source.to_owned(),
                    failure_kind: "panic".to_owned(),
                    detail: panic_payload_to_string(payload),
                    backtrace: Some(Backtrace::force_capture().to_string()),
                    frames_requested: context.frames_requested,
                    midi_source: context.midi_source.map(|path| path.display().to_string()),
                    soundfont_ids: soundfont_ids.to_vec(),
                    midi_scan: context.midi_scan.clone(),
                },
            );
            None
        }
    }
}

fn handle_recovered_render_failure(
    engine: &mut MultiSoundFontPlayback,
    mixer: &mut MixerEngine,
    scratch: &mut PlaybackRenderScratch,
    output: &mut [f32],
    trace: Option<&Arc<Mutex<RenderErrorTrace>>>,
    event: RenderErrorTraceEvent,
) {
    output.fill(0.0);
    engine.stop();
    mixer.reset_state_and_release_scratch();
    scratch.release_capacity();
    if let Some(trace) = trace {
        if let Ok(mut trace) = trace.lock() {
            trace.record_error(event);
        }
    }
}

fn clone_mixer_controls_with_recovery(
    shared: &Arc<Mutex<MixerControlState>>,
    trace: Option<&Arc<Mutex<RenderErrorTrace>>>,
    source: &'static str,
    midi_source: Option<&Path>,
) -> MixerControlState {
    match shared.lock() {
        Ok(controls) => controls.clone(),
        Err(poisoned) => {
            record_render_error_event(
                trace,
                RenderErrorTraceEvent {
                    source: source.to_owned(),
                    failure_kind: "lock-recovery".to_owned(),
                    detail: "recovered poisoned mixer_controls lock".to_owned(),
                    backtrace: None,
                    frames_requested: 0,
                    midi_source: midi_source.map(|path| path.display().to_string()),
                    soundfont_ids: Vec::new(),
                    midi_scan: PlaybackMidiScanDiagnostics::default(),
                },
            );
            poisoned.into_inner().clone()
        }
    }
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

fn record_render_error_event(
    trace: Option<&Arc<Mutex<RenderErrorTrace>>>,
    event: RenderErrorTraceEvent,
) {
    if let Some(trace) = trace {
        if let Ok(mut trace) = trace.lock() {
            trace.record_error(event);
        }
    }
}

fn render_error_trace_json(sequence: u64, event: &RenderErrorTraceEvent) -> String {
    format!(
        "{{\"schema\":\"flutz.render_debug.error.v1\",\"sequence\":{sequence},\"source\":{},\"failure_kind\":{},\"detail\":{},\"backtrace\":{},\"frames_requested\":{},\"midi_source\":{},\"soundfont_ids\":{},\"midi_scan\":{}}}",
        json_string(&event.source),
        json_string(&event.failure_kind),
        json_string(&event.detail),
        json_option_string(event.backtrace.as_deref()),
        event.frames_requested,
        json_option_string(event.midi_source.as_deref()),
        json_string_array(&event.soundfont_ids),
        midi_scan_json(&event.midi_scan)
    )
}

fn midi_scan_trace_json(
    sequence: u64,
    midi_source: Option<&Path>,
    diagnostics: &PlaybackMidiScanDiagnostics,
) -> String {
    format!(
        "{{\"schema\":\"flutz.render_debug.midi_scan.v1\",\"sequence\":{sequence},\"midi_source\":{},\"midi_scan\":{}}}",
        json_option_string(midi_source.map(|path| path.display().to_string()).as_deref()),
        midi_scan_json(diagnostics)
    )
}

fn midi_scan_json(diagnostics: &PlaybackMidiScanDiagnostics) -> String {
    let sysex = diagnostics
        .sysex_events
        .iter()
        .map(|event| {
            format!(
                "{{\"index\":{},\"status\":{},\"byte_len\":{},\"manufacturer_id\":{},\"recognized\":{},\"system_mode\":{},\"channel_role\":{},\"warning\":{},\"bytes_hex\":{}}}",
                event.index,
                event.status,
                event.byte_len,
                json_string(&event.manufacturer_id),
                if event.recognized { "true" } else { "false" },
                json_option_string(event.system_mode.as_deref()),
                json_channel_role(event.channel_role.as_ref()),
                json_option_string(event.warning.as_deref()),
                json_string(&event.bytes_hex)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"loop_style\":{},\"system_modes\":{},\"percussion_channels\":{},\"warnings\":{},\"sysex_event_count\":{},\"recognized_sysex_event_count\":{},\"sysex_events\":[{}]}}",
        json_string(&diagnostics.loop_style),
        json_string_array(&diagnostics.system_modes),
        json_u8_array(&diagnostics.percussion_channels),
        json_string_array(&diagnostics.warnings),
        diagnostics.sysex_event_count,
        diagnostics.recognized_sysex_event_count,
        sysex
    )
}

fn json_channel_role(role: Option<&PlaybackMidiChannelRoleChange>) -> String {
    role.map(|role| {
        format!(
            "{{\"channel\":{},\"role\":{}}}",
            role.channel,
            json_string(&role.role)
        )
    })
    .unwrap_or_else(|| "null".to_owned())
}

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                let _ = write!(escaped, "\\u{:04x}", ch as u32);
            }
            ch => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

fn json_option_string(value: Option<&str>) -> String {
    value.map(json_string).unwrap_or_else(|| "null".to_owned())
}

fn json_string_array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| json_string(value))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_u8_array(values: &[u8]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn analyze_track(track: &[u8], loop_markers: &mut LoopMarkerStats) -> Result<()> {
    let mut offset = 0usize;
    let mut running_status = None::<u8>;
    while offset < track.len() {
        read_var_len(track, &mut offset)?;
        if offset >= track.len() {
            break;
        }

        let first = track[offset];
        let status = if first & 0x80 != 0 {
            offset += 1;
            if first < 0xF0 {
                running_status = Some(first);
            }
            first
        } else {
            running_status.ok_or_else(|| {
                FlutzError::InvalidInput("MIDI running status used before status byte".to_owned())
            })?
        };

        match status {
            0xFF => {
                if offset >= track.len() {
                    break;
                }
                offset += 1;
                let len = read_var_len(track, &mut offset)? as usize;
                offset = offset.saturating_add(len).min(track.len());
            }
            0xF0 | 0xF7 => {
                let len = read_var_len(track, &mut offset)? as usize;
                offset = offset.saturating_add(len).min(track.len());
            }
            0x80..=0xEF => {
                let event = status & 0xF0;
                let data_len = if event == 0xC0 || event == 0xD0 { 1 } else { 2 };
                if offset + data_len > track.len() {
                    break;
                }
                let data_1 = track[offset];
                offset += data_len;

                match event {
                    0xB0 => {
                        loop_markers.note_cc(data_1);
                    }
                    _ => {}
                }
            }
            _ => break,
        }
    }
    Ok(())
}

fn read_var_len(bytes: &[u8], offset: &mut usize) -> Result<u32> {
    let mut value = 0u32;
    for _ in 0..4 {
        if *offset >= bytes.len() {
            return Err(FlutzError::InvalidInput(
                "unterminated MIDI variable length value".to_owned(),
            ));
        }
        let byte = bytes[*offset];
        *offset += 1;
        value = (value << 7) | (byte & 0x7F) as u32;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(FlutzError::InvalidInput(
        "MIDI variable length value is too long".to_owned(),
    ))
}

fn read_be_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    if offset + 2 > bytes.len() {
        return Err(FlutzError::InvalidInput("truncated MIDI u16".to_owned()));
    }
    Ok(u16::from_be_bytes([bytes[offset], bytes[offset + 1]]))
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    if offset + 4 > bytes.len() {
        return Err(FlutzError::InvalidInput("truncated MIDI u32".to_owned()));
    }
    Ok(u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}
