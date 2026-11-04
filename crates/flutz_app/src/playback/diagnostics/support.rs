use super::super::*;

pub(in crate::playback) struct AnalyzerTrace {
    writer: BufWriter<fs::File>,
    sequence: u64,
}

pub(in crate::playback) struct RenderErrorTrace {
    writer: BufWriter<fs::File>,
    sequence: u64,
    recent: VecDeque<RenderErrorTraceEvent>,
}

impl RenderErrorTrace {
    pub(in crate::playback) fn open() -> Option<Self> {
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

    pub(in crate::playback) fn record_midi_scan(
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

    pub(in crate::playback) fn take_recent(&mut self) -> Vec<RenderErrorTraceEvent> {
        self.recent.drain(..).collect()
    }
}

impl AnalyzerTrace {
    pub(in crate::playback) fn open() -> Option<Self> {
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

    pub(in crate::playback) fn record(&mut self, frame: &VisualizerFrame) {
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

pub(in crate::playback) fn record_analyzer_trace_arc(
    trace: Option<&Arc<Mutex<AnalyzerTrace>>>,
    frame: &VisualizerFrame,
) {
    if let Some(trace) = trace {
        if let Ok(mut trace) = trace.lock() {
            trace.record(frame);
        }
    }
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
    pub(in crate::playback) fn from_frame(frame: &VisualizerFrame) -> Self {
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
    pub(in crate::playback) fn from_arc<T>(arc: &Arc<T>, expected_strong_roots: usize) -> Self {
        let strong_count = Arc::strong_count(arc);
        Self {
            strong_count,
            weak_count: Arc::weak_count(arc),
            expected_strong_roots,
            excess_strong_count: strong_count.saturating_sub(expected_strong_roots),
        }
    }
}

pub(in crate::playback) fn expected_audio_closure_roots(audio_open: bool) -> usize {
    if audio_open {
        2
    } else {
        1
    }
}

pub(in crate::playback) fn expected_engine_roots(audio_open: bool) -> usize {
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
    pub(in crate::playback) fn from_memory_debug(
        memory_debug: Option<&PlaybackMemoryDebug>,
    ) -> Self {
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
    pub(in crate::playback) fn from_audio_diagnostics(
        audio: Option<&AudioDeviceDiagnostics>,
    ) -> Self {
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
    pub(in crate::playback) fn record(&mut self, snapshot: RenderChurnSnapshot) {
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

pub(in crate::playback) fn stem_blocks_audio_bytes(blocks: &[StemRenderBlock]) -> usize {
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

pub(in crate::playback) fn stem_blocks_total_bytes(blocks: &Vec<StemRenderBlock>) -> usize {
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

pub(in crate::playback) fn soundfont_catalog_entry_string_bytes(
    entry: &SoundFontCatalogEntry,
) -> usize {
    entry
        .internal_id
        .capacity()
        .saturating_add(entry.display_name.capacity())
        .saturating_add(entry.source_format.capacity())
        .saturating_add(entry.storage_format.capacity())
        .saturating_add(entry.runtime_format.capacity())
}

pub(in crate::playback) fn coverage_estimated_bytes(coverage: &SoundFontCoverage) -> usize {
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

pub(in crate::playback) struct RenderFailureContext<'a> {
    pub(in crate::playback) source: &'static str,
    pub(in crate::playback) frames_requested: usize,
    pub(in crate::playback) midi_source: Option<&'a Path>,
    pub(in crate::playback) midi_scan: &'a PlaybackMidiScanDiagnostics,
}

pub(in crate::playback) fn render_mixed_audio_guarded(
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

pub(in crate::playback) fn clone_mixer_controls_with_recovery(
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

pub(in crate::playback) fn record_render_error_event(
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
