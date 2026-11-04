use super::*;

#[derive(Debug, Clone)]
pub struct PerfTraceRecord {
    pub sequence: u64,
    pub session_id: String,
    pub operation_id: u64,
    pub record_type: &'static str,
    pub phase: &'static str,
    pub label: String,
    pub outcome: Option<String>,
    pub details: Vec<(String, String)>,
    pub wall_time_ms: u128,
    pub app_time_ms: u128,
    pub frame_gap_ms: u128,
    pub metrics: Option<PerfMetricSnapshot>,
    pub process: Option<PerfProcessSnapshot>,
}

impl PerfTraceRecord {
    pub(super) fn marker(
        sequence: u64,
        session_id: String,
        record_type: &'static str,
        phase: &'static str,
        label: &str,
        outcome: &str,
        details: &[(&str, String)],
        started_at: Instant,
        process: Option<PerfProcessSnapshot>,
    ) -> Self {
        Self {
            sequence,
            session_id,
            operation_id: 0,
            record_type,
            phase,
            label: label.to_owned(),
            outcome: Some(outcome.to_owned()),
            details: details
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
            wall_time_ms: wall_time_ms(),
            app_time_ms: Instant::now().duration_since(started_at).as_millis(),
            frame_gap_ms: 0,
            metrics: None,
            process,
        }
    }

    pub(super) fn from_metrics(
        sequence: u64,
        session_id: String,
        operation_id: u64,
        record_type: &'static str,
        phase: &'static str,
        label: &str,
        outcome: Option<String>,
        started_at: Instant,
        frame_gap_ms: u128,
        metrics: &PlaybackDebugMetrics,
        process: Option<PerfProcessSnapshot>,
    ) -> Self {
        Self {
            sequence,
            session_id,
            operation_id,
            record_type,
            phase,
            label: label.to_owned(),
            outcome,
            details: Vec::new(),
            wall_time_ms: wall_time_ms(),
            app_time_ms: Instant::now().duration_since(started_at).as_millis(),
            frame_gap_ms,
            metrics: Some(PerfMetricSnapshot::from(metrics)),
            process,
        }
    }

    pub fn summary(&self) -> String {
        let outcome = self
            .outcome
            .as_ref()
            .map(|outcome| format!(" {outcome}"))
            .unwrap_or_default();
        let metrics = self
            .metrics
            .as_ref()
            .map(|metrics| {
                format!(
                    " tick={} peak={:.3} ring={} underruns={} qerr={}",
                    metrics.transport_tick,
                    metrics.output_peak,
                    metrics
                        .ring_available_frames
                        .map(|available| format!(
                            "{}/{}",
                            available,
                            metrics.ring_capacity_frames.unwrap_or_default()
                        ))
                        .unwrap_or_else(|| "closed".to_owned()),
                    metrics.underrun_count.unwrap_or_default(),
                    metrics.queue_error_count.unwrap_or_default()
                )
            })
            .unwrap_or_default();
        format!(
            "#{:04} {} {} {}{}{}",
            self.sequence, self.record_type, self.phase, self.label, outcome, metrics
        )
    }

    pub(super) fn to_json_line(&self) -> String {
        let mut json = format!(
            "{{\"schema\":\"flutz_perf_trace.v1\",\"session\":\"{}\",\"seq\":{},\"op\":{},\"type\":\"{}\",\"phase\":\"{}\",\"label\":\"{}\",\"wall_ms\":{},\"app_ms\":{},\"frame_gap_ms\":{}",
            escape_json(&self.session_id),
            self.sequence,
            self.operation_id,
            self.record_type,
            self.phase,
            escape_json(&self.label),
            self.wall_time_ms,
            self.app_time_ms,
            self.frame_gap_ms
        );
        if let Some(outcome) = &self.outcome {
            json.push_str(&format!(",\"outcome\":\"{}\"", escape_json(outcome)));
        }
        if !self.details.is_empty() {
            json.push_str(",\"details\":{");
            for (index, (key, value)) in self.details.iter().enumerate() {
                if index > 0 {
                    json.push(',');
                }
                json.push_str(&format!(
                    "\"{}\":\"{}\"",
                    escape_json(key),
                    escape_json(value)
                ));
            }
            json.push('}');
        }
        if let Some(metrics) = &self.metrics {
            json.push_str(",\"metrics\":");
            json.push_str(&metrics.to_json());
        }
        if let Some(process) = &self.process {
            json.push_str(",\"process\":");
            json.push_str(&process.to_json());
        }
        if let (Some(metrics), Some(process)) = (&self.metrics, &self.process) {
            json.push_str(",\"memory_gap\":");
            json.push_str(&memory_gap_json(
                metrics.component_memory.tracked_total_bytes,
                &metrics.memory_runtime,
                process,
            ));
        }
        json.push('}');
        json
    }
}

#[derive(Debug, Clone)]
pub struct PerfMetricSnapshot {
    pub engine_state: String,
    pub transport_seconds: f64,
    pub transport_duration_seconds: f64,
    pub transport_tick: u64,
    pub loaded_soundfont_count: usize,
    pub requested_soundfont_count: usize,
    pub loaded_provider_count: usize,
    pub pruned_soundfont_count: usize,
    pub midi_strip_count: usize,
    pub midi_demand_role_count: usize,
    pub subset_plan_count: usize,
    pub subset_preset_count: usize,
    pub subset_instrument_count: usize,
    pub subset_sample_count: usize,
    pub subset_logical_wave_range_count: usize,
    pub subset_planned_range_count: usize,
    pub subset_planned_byte_count: u64,
    pub subset_index_json_plan_count: usize,
    pub subset_transition_exact_hits: usize,
    pub subset_transition_contained_hits: usize,
    pub subset_transition_superset_growths: usize,
    pub subset_transition_font_set_changes: usize,
    pub subset_transition_missing_samples: usize,
    pub subset_transition_missing_bytes: u64,
    pub subset_compact_loaded_count: usize,
    pub subset_full_fallback_count: usize,
    pub output_peak: f32,
    pub output_rms: f32,
    pub active_strip_count: usize,
    pub audio_status: String,
    pub audio_error: Option<String>,
    pub audio_backend: &'static str,
    pub sample_rate: u32,
    pub channels: u16,
    pub block_frames: u16,
    pub ring_available_frames: Option<usize>,
    pub ring_capacity_frames: Option<usize>,
    pub callback_count: Option<u64>,
    pub frames_requested: Option<u64>,
    pub frames_delivered: Option<u64>,
    pub underrun_count: Option<u64>,
    pub queue_error_count: Option<u64>,
    pub producer_rendered_frames: Option<u64>,
    pub audio_output: Option<PerfAudioOutputSnapshot>,
    pub memory: Option<PerfMemorySnapshot>,
    pub render_churn: PerfRenderChurnSnapshot,
    pub lifecycle: PerfLifecycleSnapshot,
    pub soundfont_cache: PerfSoundFontCacheSnapshot,
    pub component_memory: PerfComponentMemorySnapshot,
    pub references: PerfReferenceSnapshot,
    pub allocator: AllocationTraceSnapshot,
    pub memory_runtime: MemoryRuntimeSnapshot,
}

impl From<&PlaybackDebugMetrics> for PerfMetricSnapshot {
    fn from(metrics: &PlaybackDebugMetrics) -> Self {
        let diagnostics = metrics.audio_diagnostics.as_ref();
        let stats = diagnostics.map(|diagnostics| diagnostics.stats);
        Self {
            engine_state: metrics.engine_state.clone(),
            transport_seconds: metrics.transport_seconds,
            transport_duration_seconds: metrics.transport_duration_seconds,
            transport_tick: metrics.transport_tick,
            loaded_soundfont_count: metrics.loaded_soundfont_count,
            requested_soundfont_count: metrics.requested_soundfont_count,
            loaded_provider_count: metrics.loaded_provider_count,
            pruned_soundfont_count: metrics.pruned_soundfont_count,
            midi_strip_count: metrics.midi_strip_count,
            midi_demand_role_count: metrics.midi_demand.role_count(),
            subset_plan_count: metrics.subset_plans.plan_count,
            subset_preset_count: metrics.subset_plans.preset_count,
            subset_instrument_count: metrics.subset_plans.instrument_count,
            subset_sample_count: metrics.subset_plans.sample_count,
            subset_logical_wave_range_count: metrics.subset_plans.logical_wave_range_count,
            subset_planned_range_count: metrics.subset_plans.planned_range_count,
            subset_planned_byte_count: metrics.subset_plans.planned_byte_count,
            subset_index_json_plan_count: metrics.subset_plans.index_json_plan_count,
            subset_transition_exact_hits: metrics.subset_transition.exact_subset_hits,
            subset_transition_contained_hits: metrics.subset_transition.subset_contained_hits,
            subset_transition_superset_growths: metrics.subset_transition.superset_growth_events,
            subset_transition_font_set_changes: metrics.subset_transition.font_set_changes,
            subset_transition_missing_samples: metrics.subset_transition.missing_sample_count,
            subset_transition_missing_bytes: metrics.subset_transition.missing_planned_bytes,
            subset_compact_loaded_count: metrics.subset_transition.compact_loaded_count,
            subset_full_fallback_count: metrics.subset_transition.full_fallback_count,
            output_peak: metrics.output_peak,
            output_rms: metrics.output_rms,
            active_strip_count: metrics.active_strip_count,
            audio_status: metrics.audio_status.clone(),
            audio_error: metrics.audio_error.clone(),
            audio_backend: metrics.audio_backend,
            sample_rate: metrics.audio_config.sample_rate,
            channels: metrics.audio_config.channels,
            block_frames: metrics.audio_config.internal_block_frames,
            ring_available_frames: diagnostics.map(|diagnostics| diagnostics.ring_available_frames),
            ring_capacity_frames: diagnostics.map(|diagnostics| diagnostics.ring_capacity_frames),
            callback_count: stats.map(|stats| stats.callback_count),
            frames_requested: stats.map(|stats| stats.frames_requested),
            frames_delivered: stats.map(|stats| stats.frames_delivered),
            underrun_count: stats.map(|stats| stats.underrun_count),
            queue_error_count: stats.map(|stats| stats.queue_error_count),
            producer_rendered_frames: stats.map(|stats| stats.producer_rendered_frames),
            audio_output: diagnostics.map(PerfAudioOutputSnapshot::from),
            memory: metrics.memory_debug.as_ref().map(|memory| {
                PerfMemorySnapshot::from_playback(
                    memory,
                    metrics.loaded_midi_bytes,
                    metrics.loaded_midi_capacity_bytes,
                )
            }),
            render_churn: PerfRenderChurnSnapshot::from(metrics),
            lifecycle: PerfLifecycleSnapshot::from(metrics),
            soundfont_cache: PerfSoundFontCacheSnapshot::from(&metrics.soundfont_cache),
            component_memory: PerfComponentMemorySnapshot::from(metrics),
            references: PerfReferenceSnapshot::from(metrics),
            allocator: allocation_trace_snapshot(),
            memory_runtime: memory_runtime::snapshot(),
        }
    }
}

impl PerfMetricSnapshot {
    fn to_json(&self) -> String {
        let mut json = format!(
            "{{\"engine\":\"{}\",\"transport_s\":{:.3},\"duration_s\":{:.3},\"tick\":{},\"soundfonts\":{},\"requested_soundfonts\":{},\"loaded_providers\":{},\"pruned_soundfonts\":{},\"midi_strips\":{},\"midi_demand_roles\":{},\"subset_plans\":{},\"subset_presets\":{},\"subset_instruments\":{},\"subset_samples\":{},\"subset_logical_wave_ranges\":{},\"subset_planned_ranges\":{},\"subset_planned_bytes\":{},\"subset_index_json_plans\":{},\"subset_transition_exact_hits\":{},\"subset_transition_contained_hits\":{},\"subset_transition_superset_growths\":{},\"subset_transition_font_set_changes\":{},\"subset_transition_missing_samples\":{},\"subset_transition_missing_bytes\":{},\"subset_compact_loaded\":{},\"subset_full_fallback\":{},\"output_peak\":{:.6},\"output_rms\":{:.6},\"active_strips\":{},\"audio_status\":\"{}\",\"audio_backend\":\"{}\",\"sample_rate\":{},\"channels\":{},\"block_frames\":{}",
            escape_json(&self.engine_state),
            self.transport_seconds,
            self.transport_duration_seconds,
            self.transport_tick,
            self.loaded_soundfont_count,
            self.requested_soundfont_count,
            self.loaded_provider_count,
            self.pruned_soundfont_count,
            self.midi_strip_count,
            self.midi_demand_role_count,
            self.subset_plan_count,
            self.subset_preset_count,
            self.subset_instrument_count,
            self.subset_sample_count,
            self.subset_logical_wave_range_count,
            self.subset_planned_range_count,
            self.subset_planned_byte_count,
            self.subset_index_json_plan_count,
            self.subset_transition_exact_hits,
            self.subset_transition_contained_hits,
            self.subset_transition_superset_growths,
            self.subset_transition_font_set_changes,
            self.subset_transition_missing_samples,
            self.subset_transition_missing_bytes,
            self.subset_compact_loaded_count,
            self.subset_full_fallback_count,
            self.output_peak,
            self.output_rms,
            self.active_strip_count,
            escape_json(&self.audio_status),
            self.audio_backend,
            self.sample_rate,
            self.channels,
            self.block_frames
        );
        push_json_string_opt(&mut json, "audio_error", self.audio_error.as_deref());
        push_json_usize_opt(
            &mut json,
            "ring_available_frames",
            self.ring_available_frames,
        );
        push_json_usize_opt(&mut json, "ring_capacity_frames", self.ring_capacity_frames);
        push_json_u64_opt(&mut json, "callback_count", self.callback_count);
        push_json_u64_opt(&mut json, "frames_requested", self.frames_requested);
        push_json_u64_opt(&mut json, "frames_delivered", self.frames_delivered);
        push_json_u64_opt(&mut json, "underrun_count", self.underrun_count);
        push_json_u64_opt(&mut json, "queue_error_count", self.queue_error_count);
        push_json_u64_opt(
            &mut json,
            "producer_rendered_frames",
            self.producer_rendered_frames,
        );
        if let Some(memory) = &self.memory {
            json.push_str(",\"memory\":");
            json.push_str(&memory.to_json());
        }
        if let Some(audio_output) = &self.audio_output {
            json.push_str(",\"audio_output\":");
            json.push_str(&audio_output.to_json());
        }
        json.push_str(",\"render_churn\":");
        json.push_str(&self.render_churn.to_json());
        json.push_str(",\"lifecycle\":");
        json.push_str(&self.lifecycle.to_json());
        json.push_str(",\"soundfont_cache\":");
        json.push_str(&self.soundfont_cache.to_json());
        json.push_str(",\"component_memory\":");
        json.push_str(&self.component_memory.to_json());
        json.push_str(",\"references\":");
        json.push_str(&self.references.to_json());
        json.push_str(",\"allocator\":");
        json.push_str(&allocation_trace_json(&self.allocator));
        json.push_str(",\"memory_runtime\":");
        json.push_str(&self.memory_runtime.to_json());
        json.push('}');
        json
    }
}
