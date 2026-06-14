use std::{
    collections::VecDeque,
    env,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    allocation_trace::{
        snapshot as allocation_trace_snapshot, AllocationScope, AllocationScopeGuard,
        AllocationTraceSnapshot,
    },
    memory_runtime::{self, MemoryRuntimeSnapshot},
    playback::PlaybackDebugMetrics,
};
use flutz_synth::{
    PlaybackMemoryDebug, SoundFontRuntimeCacheDebug, SoundFontRuntimeCacheEntryDebug,
    StemRenderAllocationDebug, SynthInstanceMemoryDebug,
};

const MAX_RECORDS: usize = 96;
const MAX_ISSUES: usize = 64;
const SAMPLE_INTERVAL_MS: u128 = 2_000;
const LATENCY_SAMPLE_INTERVAL_MS: u128 = 1_000;
const FRAME_STALL_MS: u128 = 120;

pub const PERF_TRACE_COMPILED: bool = cfg!(debug_assertions);

#[derive(Debug)]
pub struct LatencyTrace {
    enabled: bool,
    session_id: String,
    sequence: u64,
    started_at: Instant,
    last_sample_at: Instant,
    last_callback_count: u64,
    last_underrun_count: u64,
    last_queue_error_count: u64,
    last_target_frames: u32,
    last_reason: &'static str,
    log_path: Option<PathBuf>,
    log_error: Option<String>,
}

impl LatencyTrace {
    pub fn new(debug_latency: bool) -> Self {
        let now = Instant::now();
        #[cfg(not(debug_assertions))]
        {
            let _ = debug_latency;
            return Self {
                enabled: false,
                session_id: String::new(),
                sequence: 0,
                started_at: now,
                last_sample_at: now,
                last_callback_count: 0,
                last_underrun_count: 0,
                last_queue_error_count: 0,
                last_target_frames: 0,
                last_reason: "disabled",
                log_path: None,
                log_error: None,
            };
        }

        #[cfg(debug_assertions)]
        {
            let session_id = wall_time_ms().to_string();
            let enabled = debug_latency && env::var("FLUTZ_LATENCY_TRACE").as_deref() != Ok("0");
            let log_path = if enabled {
                env::var_os("FLUTZ_LATENCY_LOG")
                    .map(PathBuf::from)
                    .or_else(|| Some(default_latency_log_path(&session_id)))
            } else {
                None
            };
            let mut trace = Self {
                enabled,
                session_id,
                sequence: 0,
                started_at: now,
                last_sample_at: now,
                last_callback_count: 0,
                last_underrun_count: 0,
                last_queue_error_count: 0,
                last_target_frames: 0,
                last_reason: "launch",
                log_path,
                log_error: None,
            };
            if trace.enabled {
                trace.write_marker("launch");
            }
            trace
        }
    }

    pub fn status_line(&self) -> String {
        if !PERF_TRACE_COMPILED {
            return "latency trace inert for this build".to_owned();
        }
        let logging = self
            .log_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not logging".to_owned());
        let error = self
            .log_error
            .as_ref()
            .map(|error| format!(", log error: {error}"))
            .unwrap_or_default();
        format!(
            "latency trace {} ({logging}{error})",
            if self.enabled { "on" } else { "off" }
        )
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_periodic_sample_due(&self) -> bool {
        self.enabled
            && Instant::now()
                .duration_since(self.last_sample_at)
                .as_millis()
                >= LATENCY_SAMPLE_INTERVAL_MS
    }

    pub fn sample_frame(&mut self, metrics: &PlaybackDebugMetrics) {
        if !self.enabled {
            return;
        }

        let now = Instant::now();
        let diagnostics = metrics.audio_diagnostics.as_ref();
        let stats = diagnostics.map(|diagnostics| diagnostics.stats);
        let callback_count = stats.map(|stats| stats.callback_count).unwrap_or_default();
        let underruns = stats.map(|stats| stats.underrun_count).unwrap_or_default();
        let queue_errors = stats
            .map(|stats| stats.queue_error_count)
            .unwrap_or_default();
        let target_frames = metrics.flux_guard.decision.target.target_frames;
        let reason = metrics.flux_guard.decision.reason.as_str();

        let event = if underruns > self.last_underrun_count {
            Some("underrun")
        } else if queue_errors > self.last_queue_error_count {
            Some("queue-error")
        } else if target_frames != self.last_target_frames {
            Some("target-change")
        } else if reason != self.last_reason {
            Some("reason-change")
        } else if callback_count != self.last_callback_count
            && now.duration_since(self.last_sample_at).as_millis() >= LATENCY_SAMPLE_INTERVAL_MS
        {
            Some("sample")
        } else {
            None
        };

        self.last_callback_count = callback_count;
        self.last_underrun_count = underruns;
        self.last_queue_error_count = queue_errors;
        self.last_target_frames = target_frames;
        self.last_reason = reason;

        if let Some(event) = event {
            self.write_metrics(event, metrics);
            self.last_sample_at = now;
        }
    }

    #[cfg(debug_assertions)]
    fn write_marker(&mut self, event: &str) {
        let Some(path) = self.log_path.clone() else {
            return;
        };
        self.sequence = self.sequence.saturating_add(1);
        let json = format!(
            "{{\"schema\":\"flutz_latency_trace.v1\",\"session\":\"{}\",\"seq\":{},\"event\":\"{}\",\"wall_ms\":{},\"app_ms\":{}}}",
            escape_json(&self.session_id),
            self.sequence,
            escape_json(event),
            wall_time_ms(),
            Instant::now().duration_since(self.started_at).as_millis()
        );
        if let Err(error) = append_json_line(&path, &json) {
            self.log_error = Some(error.to_string());
        }
    }

    fn write_metrics(&mut self, event: &str, metrics: &PlaybackDebugMetrics) {
        let Some(path) = self.log_path.clone() else {
            return;
        };
        let diagnostics = metrics.audio_diagnostics.as_ref();
        let stats = diagnostics.map(|diagnostics| diagnostics.stats);
        let decision = metrics.flux_guard.decision;
        self.sequence = self.sequence.saturating_add(1);
        let mut json = format!(
            "{{\"schema\":\"flutz_latency_trace.v1\",\"session\":\"{}\",\"seq\":{},\"event\":\"{}\",\"wall_ms\":{},\"app_ms\":{},\"backend\":\"{}\",\"sample_rate\":{},\"latency_frames\":{},\"latency_ms\":{:.3},\"wrapper_queue_frames\":{},\"device_queue_frames\":{},\"target_frames\":{},\"low_water_frames\":{},\"high_water_frames\":{},\"buffered_frames\":{},\"available_frames\":{},\"largest_callback_frames\":{},\"reason\":\"{}\",\"callbacks\":{},\"frames_requested\":{},\"frames_delivered\":{},\"underruns\":{},\"queue_errors\":{}",
            escape_json(&self.session_id),
            self.sequence,
            escape_json(event),
            wall_time_ms(),
            Instant::now().duration_since(self.started_at).as_millis(),
            metrics.audio_backend,
            metrics.audio_config.sample_rate,
            metrics.meter_latency_frames,
            metrics.meter_latency_ms,
            metrics.meter_wrapper_queue_frames,
            metrics.meter_device_queue_frames,
            decision.target.target_frames,
            decision.target.low_water_frames,
            decision.target.high_water_frames,
            decision.state.current_buffered_frames,
            decision.state.available_frames,
            decision.state.largest_callback_frames,
            decision.reason.as_str(),
            stats.map(|stats| stats.callback_count).unwrap_or_default(),
            stats.map(|stats| stats.frames_requested).unwrap_or_default(),
            stats.map(|stats| stats.frames_delivered).unwrap_or_default(),
            stats.map(|stats| stats.underrun_count).unwrap_or_default(),
            stats.map(|stats| stats.queue_error_count).unwrap_or_default()
        );
        if let Some(diagnostics) = diagnostics {
            json.push_str(&format!(
                ",\"ring_available_frames\":{},\"ring_capacity_frames\":{}",
                diagnostics.ring_available_frames, diagnostics.ring_capacity_frames
            ));
        }
        json.push('}');
        if let Err(error) = append_json_line(&path, &json) {
            self.log_error = Some(error.to_string());
        }
    }
}

#[derive(Debug)]
pub struct PerfTrace {
    enabled: bool,
    session_id: String,
    sequence: u64,
    operation_sequence: u64,
    started_at: Instant,
    last_frame_at: Instant,
    last_sample_at: Instant,
    max_frame_gap_ms: u128,
    last_underrun_count: u64,
    last_queue_error_count: u64,
    records: VecDeque<PerfTraceRecord>,
    issues: VecDeque<PerfTraceRecord>,
    log_path: Option<PathBuf>,
    log_error: Option<String>,
    process_metrics: ProcessMetricSampler,
}

impl PerfTrace {
    pub fn new(debug_memory: bool) -> Self {
        let now = Instant::now();
        #[cfg(not(debug_assertions))]
        {
            let _ = debug_memory;
            return Self {
                enabled: false,
                session_id: String::new(),
                sequence: 0,
                operation_sequence: 0,
                started_at: now,
                last_frame_at: now,
                last_sample_at: now,
                max_frame_gap_ms: 0,
                last_underrun_count: 0,
                last_queue_error_count: 0,
                records: VecDeque::new(),
                issues: VecDeque::new(),
                log_path: None,
                log_error: None,
                process_metrics: ProcessMetricSampler::new(now),
            };
        }

        #[cfg(debug_assertions)]
        {
            let session_id = wall_time_ms().to_string();
            let env_log_path = env::var_os("FLUTZ_PERF_TRACE_LOG").map(PathBuf::from);
            let enabled = debug_memory && env::var("FLUTZ_PERF_TRACE").as_deref() != Ok("0");
            let log_path = if enabled {
                env_log_path.or_else(|| Some(default_log_path(&session_id)))
            } else {
                None
            };
            let mut trace = Self {
                enabled,
                session_id,
                sequence: 0,
                operation_sequence: 0,
                started_at: now,
                last_frame_at: now,
                last_sample_at: now,
                max_frame_gap_ms: 0,
                last_underrun_count: 0,
                last_queue_error_count: 0,
                records: VecDeque::with_capacity(MAX_RECORDS),
                issues: VecDeque::with_capacity(MAX_ISSUES),
                log_path,
                log_error: None,
                process_metrics: ProcessMetricSampler::new(now),
            };
            if trace.enabled {
                trace.record_marker("app.session", "launch");
            }
            trace
        }
    }

    pub fn status_line(&self) -> String {
        if !PERF_TRACE_COMPILED {
            return "perf trace inert for this build".to_owned();
        }
        let logging = self
            .log_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not logging".to_owned());
        let error = self
            .log_error
            .as_ref()
            .map(|error| format!(", log error: {error}"))
            .unwrap_or_default();
        format!(
            "{}; {} record(s), {} issue(s); {logging}{error}",
            if self.enabled { "enabled" } else { "disabled" },
            self.records.len(),
            self.issues.len()
        )
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_periodic_sample_due(&self) -> bool {
        self.enabled
            && Instant::now()
                .duration_since(self.last_sample_at)
                .as_millis()
                >= SAMPLE_INTERVAL_MS
    }

    pub fn observe_frame(&mut self) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let frame_gap_ms = now.duration_since(self.last_frame_at).as_millis();
        self.last_frame_at = now;
        self.max_frame_gap_ms = self.max_frame_gap_ms.max(frame_gap_ms);
    }

    pub fn records(&self) -> Vec<PerfTraceRecord> {
        self.records.iter().cloned().collect()
    }

    pub fn issues(&self) -> Vec<PerfTraceRecord> {
        self.issues.iter().cloned().collect()
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.issues.clear();
        self.log_error = None;
        if self.enabled {
            self.record_marker("perf_trace.clear", "cleared");
        }
    }

    pub fn begin_user_event(&mut self, label: &str, metrics: &PlaybackDebugMetrics) -> u64 {
        if !self.enabled {
            return 0;
        }
        self.operation_sequence = self.operation_sequence.saturating_add(1);
        let operation_id = self.operation_sequence;
        let sequence = self.next_sequence();
        let process = self.process_metrics.sample();
        self.push_record(PerfTraceRecord::from_metrics(
            sequence,
            self.session_id.clone(),
            operation_id,
            "event",
            "before",
            label,
            None,
            self.started_at,
            0,
            metrics,
            process,
        ));
        operation_id
    }

    pub fn finish_user_event(
        &mut self,
        operation_id: u64,
        label: &str,
        outcome: &str,
        metrics: &PlaybackDebugMetrics,
    ) {
        if !self.enabled || operation_id == 0 {
            return;
        }
        let sequence = self.next_sequence();
        let process = self.process_metrics.sample();
        self.push_record(PerfTraceRecord::from_metrics(
            sequence,
            self.session_id.clone(),
            operation_id,
            "event",
            "after",
            label,
            Some(outcome.to_owned()),
            self.started_at,
            0,
            metrics,
            process,
        ));
    }

    pub fn record_user_event(
        &mut self,
        label: &str,
        outcome: &str,
        metrics: &PlaybackDebugMetrics,
    ) {
        if !self.enabled {
            return;
        }
        self.operation_sequence = self.operation_sequence.saturating_add(1);
        let operation_id = self.operation_sequence;
        let sequence = self.next_sequence();
        let process = self.process_metrics.sample();
        self.push_record(PerfTraceRecord::from_metrics(
            sequence,
            self.session_id.clone(),
            operation_id,
            "event",
            "point",
            label,
            Some(outcome.to_owned()),
            self.started_at,
            0,
            metrics,
            process,
        ));
    }

    pub fn record_diagnostic(&mut self, label: &str, outcome: &str, details: &[(&str, String)]) {
        if !self.enabled {
            return;
        }
        let sequence = self.next_sequence();
        let process = self.process_metrics.sample();
        self.push_record(PerfTraceRecord::marker(
            sequence,
            self.session_id.clone(),
            "diagnostic",
            "point",
            label,
            outcome,
            details,
            self.started_at,
            process,
        ));
    }

    pub fn sample_frame(&mut self, metrics: &PlaybackDebugMetrics) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.last_sample_at).as_millis() < SAMPLE_INTERVAL_MS {
            return;
        }
        let frame_gap_ms = self.max_frame_gap_ms;
        self.max_frame_gap_ms = 0;

        let underruns = metrics
            .audio_diagnostics
            .as_ref()
            .map(|diagnostics| diagnostics.stats.underrun_count)
            .unwrap_or_default();
        let queue_errors = metrics
            .audio_diagnostics
            .as_ref()
            .map(|diagnostics| diagnostics.stats.queue_error_count)
            .unwrap_or_default();

        let mut issue = None::<String>;
        if frame_gap_ms >= FRAME_STALL_MS {
            issue = Some(format!("ui frame gap {frame_gap_ms}ms"));
        } else if underruns > self.last_underrun_count {
            issue = Some(format!(
                "audio underrun count advanced {} -> {}",
                self.last_underrun_count, underruns
            ));
        } else if queue_errors > self.last_queue_error_count {
            issue = Some(format!(
                "audio queue error count advanced {} -> {}",
                self.last_queue_error_count, queue_errors
            ));
        }

        self.last_underrun_count = underruns;
        self.last_queue_error_count = queue_errors;

        if let Some(issue) = issue {
            let sequence = self.next_sequence();
            let process = self.process_metrics.sample();
            let record = PerfTraceRecord::from_metrics(
                sequence,
                self.session_id.clone(),
                0,
                "issue",
                "detected",
                "performance.quality",
                Some(issue),
                self.started_at,
                frame_gap_ms,
                metrics,
                process,
            );
            self.push_issue(record.clone());
            self.push_record(record);
            self.last_sample_at = now;
            return;
        }

        let sequence = self.next_sequence();
        let process = self.process_metrics.sample();
        self.push_record(PerfTraceRecord::from_metrics(
            sequence,
            self.session_id.clone(),
            0,
            "sample",
            "periodic",
            "performance.sample",
            None,
            self.started_at,
            frame_gap_ms,
            metrics,
            process,
        ));
        self.last_sample_at = now;
    }

    pub fn start_logging_default(&mut self) -> Result<PathBuf, String> {
        if !PERF_TRACE_COMPILED {
            return Err("perf trace is inert for this build".to_owned());
        }
        self.enabled = true;
        let path = default_log_path(&self.session_id);
        self.log_path = Some(path.clone());
        self.log_error = None;
        self.write_all_records(&path)
            .map_err(|error| format!("failed to initialize perf trace log: {error}"))?;
        self.record_marker("perf_trace.log", "started");
        Ok(path)
    }

    pub fn stop_logging(&mut self) {
        if !self.enabled {
            return;
        }
        self.record_marker("perf_trace.log", "stopped");
        self.log_path = None;
    }

    pub fn export_jsonl_default(&mut self) -> Result<PathBuf, String> {
        if !PERF_TRACE_COMPILED {
            return Err("perf trace is inert for this build".to_owned());
        }
        let path = default_log_path(&self.session_id);
        self.write_all_records(&path)
            .map_err(|error| format!("failed to export perf trace log: {error}"))?;
        Ok(path)
    }

    fn record_marker(&mut self, label: &str, outcome: &str) {
        if !self.enabled {
            return;
        }
        let sequence = self.next_sequence();
        let process = self.process_metrics.sample();
        self.push_record(PerfTraceRecord::marker(
            sequence,
            self.session_id.clone(),
            "event",
            "marker",
            label,
            outcome,
            &[],
            self.started_at,
            process,
        ));
    }

    fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.sequence
    }

    fn push_issue(&mut self, record: PerfTraceRecord) {
        push_bounded(&mut self.issues, record, MAX_ISSUES);
    }

    fn push_record(&mut self, record: PerfTraceRecord) {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::PerfTrace);
        if let Some(path) = self.log_path.clone() {
            if let Err(error) = append_record(&path, &record) {
                self.log_error = Some(error.to_string());
            }
        }
        push_bounded(&mut self.records, record, MAX_RECORDS);
    }

    fn write_all_records(&self, path: &Path) -> io::Result<()> {
        let mut file = File::create(path)?;
        for record in &self.records {
            writeln!(file, "{}", record.to_json_line())?;
        }
        Ok(())
    }
}

impl Default for PerfTrace {
    fn default() -> Self {
        Self::new(false)
    }
}

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
    fn marker(
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

    fn from_metrics(
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

    fn to_json_line(&self) -> String {
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

impl Drop for PerfTrace {
    fn drop(&mut self) {
        if self.enabled {
            self.record_marker("app.session", "exit");
        }
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

#[derive(Debug, Clone, Default)]
pub struct PerfAudioOutputSnapshot {
    pub ring_retained_bytes: usize,
    pub callback_scratch_bytes: usize,
    pub producer_render_block_bytes: u64,
    pub write_calls: u64,
    pub write_bytes: u64,
    pub last_callback_additional_bytes: u64,
    pub largest_callback_additional_bytes: u64,
    pub last_callback_total_bytes: u64,
    pub largest_callback_total_bytes: u64,
    pub init_calls: u64,
    pub quit_calls: u64,
    pub stream_opens: u64,
    pub stream_destroys: u64,
    pub stream_resumes: u64,
    pub stream_pauses: u64,
    pub callback_states_created: u64,
    pub callback_states_dropped: u64,
    pub producer_threads_started: u64,
    pub producer_threads_finished: u64,
    pub stream_id: u64,
    pub shared_strong_count: usize,
    pub shared_weak_count: usize,
    pub callback_shared_strong_count: usize,
    pub callback_shared_weak_count: usize,
}

impl From<&flutz_audio_sdl3::AudioDeviceDiagnostics> for PerfAudioOutputSnapshot {
    fn from(diagnostics: &flutz_audio_sdl3::AudioDeviceDiagnostics) -> Self {
        Self {
            ring_retained_bytes: diagnostics.ring_retained_bytes,
            callback_scratch_bytes: diagnostics.callback_scratch_bytes,
            producer_render_block_bytes: diagnostics.producer_render_block_bytes,
            write_calls: diagnostics.stats.write_calls,
            write_bytes: diagnostics.stats.write_bytes,
            last_callback_additional_bytes: diagnostics.stats.last_callback_additional_bytes,
            largest_callback_additional_bytes: diagnostics.stats.largest_callback_additional_bytes,
            last_callback_total_bytes: diagnostics.stats.last_callback_total_bytes,
            largest_callback_total_bytes: diagnostics.stats.largest_callback_total_bytes,
            init_calls: diagnostics.sdl_lifecycle.init_calls,
            quit_calls: diagnostics.sdl_lifecycle.quit_calls,
            stream_opens: diagnostics.sdl_lifecycle.stream_opens,
            stream_destroys: diagnostics.sdl_lifecycle.stream_destroys,
            stream_resumes: diagnostics.sdl_lifecycle.stream_resumes,
            stream_pauses: diagnostics.sdl_lifecycle.stream_pauses,
            callback_states_created: diagnostics.sdl_lifecycle.callback_states_created,
            callback_states_dropped: diagnostics.sdl_lifecycle.callback_states_dropped,
            producer_threads_started: diagnostics.sdl_lifecycle.producer_threads_started,
            producer_threads_finished: diagnostics.sdl_lifecycle.producer_threads_finished,
            stream_id: diagnostics.references.stream_id,
            shared_strong_count: diagnostics.references.shared_strong_count,
            shared_weak_count: diagnostics.references.shared_weak_count,
            callback_shared_strong_count: diagnostics.references.callback_shared_strong_count,
            callback_shared_weak_count: diagnostics.references.callback_shared_weak_count,
        }
    }
}

impl PerfAudioOutputSnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"ring_retained_bytes\":{},\"callback_scratch_bytes\":{},\"producer_render_block_bytes\":{},\"write_calls\":{},\"write_bytes\":{},\"last_callback_additional_bytes\":{},\"largest_callback_additional_bytes\":{},\"last_callback_total_bytes\":{},\"largest_callback_total_bytes\":{},\"init_calls\":{},\"quit_calls\":{},\"stream_opens\":{},\"stream_destroys\":{},\"stream_resumes\":{},\"stream_pauses\":{},\"callback_states_created\":{},\"callback_states_dropped\":{},\"producer_threads_started\":{},\"producer_threads_finished\":{},\"stream_id\":{},\"shared_strong_count\":{},\"shared_weak_count\":{},\"callback_shared_strong_count\":{},\"callback_shared_weak_count\":{}}}",
            self.ring_retained_bytes,
            self.callback_scratch_bytes,
            self.producer_render_block_bytes,
            self.write_calls,
            self.write_bytes,
            self.last_callback_additional_bytes,
            self.largest_callback_additional_bytes,
            self.last_callback_total_bytes,
            self.largest_callback_total_bytes,
            self.init_calls,
            self.quit_calls,
            self.stream_opens,
            self.stream_destroys,
            self.stream_resumes,
            self.stream_pauses,
            self.callback_states_created,
            self.callback_states_dropped,
            self.producer_threads_started,
            self.producer_threads_finished,
            self.stream_id,
            self.shared_strong_count,
            self.shared_weak_count,
            self.callback_shared_strong_count,
            self.callback_shared_weak_count
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfReferenceSnapshot {
    pub engine: Option<PerfArcReferenceSnapshot>,
    pub mixer: PerfArcReferenceSnapshot,
    pub mixer_controls: PerfArcReferenceSnapshot,
    pub latest_snapshot: PerfArcReferenceSnapshot,
    pub midi_strips: PerfArcReferenceSnapshot,
    pub render_scratch: PerfArcReferenceSnapshot,
    pub render_churn: PerfArcReferenceSnapshot,
    pub audio_stream_open: bool,
}

impl From<&PlaybackDebugMetrics> for PerfReferenceSnapshot {
    fn from(metrics: &PlaybackDebugMetrics) -> Self {
        let references = &metrics.references;
        Self {
            engine: references.engine.map(PerfArcReferenceSnapshot::from),
            mixer: PerfArcReferenceSnapshot::from(references.mixer),
            mixer_controls: PerfArcReferenceSnapshot::from(references.mixer_controls),
            latest_snapshot: PerfArcReferenceSnapshot::from(references.latest_snapshot),
            midi_strips: PerfArcReferenceSnapshot::from(references.midi_strips),
            render_scratch: PerfArcReferenceSnapshot::from(references.render_scratch),
            render_churn: PerfArcReferenceSnapshot::from(references.render_churn),
            audio_stream_open: references.audio_stream_open,
        }
    }
}

impl PerfReferenceSnapshot {
    fn to_json(&self) -> String {
        let mut json = format!(
            "{{\"audio_stream_open\":{},\"mixer\":{},\"mixer_controls\":{},\"latest_snapshot\":{},\"midi_strips\":{},\"render_scratch\":{},\"render_churn\":{}",
            self.audio_stream_open,
            self.mixer.to_json(),
            self.mixer_controls.to_json(),
            self.latest_snapshot.to_json(),
            self.midi_strips.to_json(),
            self.render_scratch.to_json(),
            self.render_churn.to_json()
        );
        if let Some(engine) = self.engine {
            json.push_str(",\"engine\":");
            json.push_str(&engine.to_json());
        }
        json.push('}');
        json
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfArcReferenceSnapshot {
    pub strong_count: usize,
    pub weak_count: usize,
    pub expected_strong_roots: usize,
    pub excess_strong_count: usize,
}

impl From<crate::playback::ArcReferenceDiagnostics> for PerfArcReferenceSnapshot {
    fn from(value: crate::playback::ArcReferenceDiagnostics) -> Self {
        Self {
            strong_count: value.strong_count,
            weak_count: value.weak_count,
            expected_strong_roots: value.expected_strong_roots,
            excess_strong_count: value.excess_strong_count,
        }
    }
}

impl PerfArcReferenceSnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"strong\":{},\"weak\":{},\"expected_strong\":{},\"excess_strong\":{}}}",
            self.strong_count,
            self.weak_count,
            self.expected_strong_roots,
            self.excess_strong_count
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfComponentMemorySnapshot {
    pub tracked_total_bytes: usize,
    pub soundfont_metadata: PerfSoundFontMetadataMemorySnapshot,
    pub midi: PerfMidiMemorySnapshot,
    pub rustystem: PerfRustystemMemorySnapshot,
    pub audio: PerfAudioMemorySnapshot,
}

impl From<&PlaybackDebugMetrics> for PerfComponentMemorySnapshot {
    fn from(metrics: &PlaybackDebugMetrics) -> Self {
        let component = &metrics.component_memory;
        Self {
            tracked_total_bytes: component.tracked_total_bytes,
            soundfont_metadata: PerfSoundFontMetadataMemorySnapshot::from(
                component.soundfont_metadata,
            ),
            midi: PerfMidiMemorySnapshot::from(component.midi),
            rustystem: PerfRustystemMemorySnapshot::from(component.rustystem),
            audio: PerfAudioMemorySnapshot::from(component.audio),
        }
    }
}

impl PerfComponentMemorySnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"tracked_total_bytes\":{},\"soundfont_metadata\":{},\"midi\":{},\"rustystem\":{},\"audio\":{}}}",
            self.tracked_total_bytes,
            self.soundfont_metadata.to_json(),
            self.midi.to_json(),
            self.rustystem.to_json(),
            self.audio.to_json()
        )
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfSoundFontMetadataMemorySnapshot {
    pub catalog_entries: usize,
    pub catalog_estimated_bytes: usize,
    pub loaded_soundfont_ids: usize,
    pub loaded_soundfont_id_bytes: usize,
    pub loaded_coverage_entries: usize,
    pub loaded_coverage_estimated_bytes: usize,
    pub estimated_bytes: usize,
}

impl From<crate::playback::SoundFontMetadataDiagnostics> for PerfSoundFontMetadataMemorySnapshot {
    fn from(value: crate::playback::SoundFontMetadataDiagnostics) -> Self {
        Self {
            catalog_entries: value.catalog_entries,
            catalog_estimated_bytes: value.catalog_estimated_bytes,
            loaded_soundfont_ids: value.loaded_soundfont_ids,
            loaded_soundfont_id_bytes: value.loaded_soundfont_id_bytes,
            loaded_coverage_entries: value.loaded_coverage_entries,
            loaded_coverage_estimated_bytes: value.loaded_coverage_estimated_bytes,
            estimated_bytes: value.estimated_bytes,
        }
    }
}

impl PerfSoundFontMetadataMemorySnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"catalog_entries\":{},\"catalog_estimated_bytes\":{},\"loaded_soundfont_ids\":{},\"loaded_soundfont_id_bytes\":{},\"loaded_coverage_entries\":{},\"loaded_coverage_estimated_bytes\":{},\"estimated_bytes\":{}}}",
            self.catalog_entries,
            self.catalog_estimated_bytes,
            self.loaded_soundfont_ids,
            self.loaded_soundfont_id_bytes,
            self.loaded_coverage_entries,
            self.loaded_coverage_estimated_bytes,
            self.estimated_bytes
        )
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfMidiMemorySnapshot {
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

impl From<crate::playback::MidiRuntimeMemoryDiagnostics> for PerfMidiMemorySnapshot {
    fn from(value: crate::playback::MidiRuntimeMemoryDiagnostics) -> Self {
        Self {
            raw_bytes: value.raw_bytes,
            raw_capacity_bytes: value.raw_capacity_bytes,
            strip_count: value.strip_count,
            strip_capacity: value.strip_capacity,
            strip_bytes: value.strip_bytes,
            jump_point_count: value.jump_point_count,
            jump_point_bytes: value.jump_point_bytes,
            parsed_message_count: value.parsed_message_count,
            parsed_sysex_events: value.parsed_sysex_events,
            parsed_sysex_bytes: value.parsed_sysex_bytes,
            parsed_estimated_bytes: value.parsed_estimated_bytes,
            estimated_bytes: value.estimated_bytes,
        }
    }
}

impl PerfMidiMemorySnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"raw_bytes\":{},\"raw_capacity_bytes\":{},\"strip_count\":{},\"strip_capacity\":{},\"strip_bytes\":{},\"jump_point_count\":{},\"jump_point_bytes\":{},\"parsed_message_count\":{},\"parsed_sysex_events\":{},\"parsed_sysex_bytes\":{},\"parsed_estimated_bytes\":{},\"estimated_bytes\":{}}}",
            self.raw_bytes,
            self.raw_capacity_bytes,
            self.strip_count,
            self.strip_capacity,
            self.strip_bytes,
            self.jump_point_count,
            self.jump_point_bytes,
            self.parsed_message_count,
            self.parsed_sysex_events,
            self.parsed_sysex_bytes,
            self.parsed_estimated_bytes,
            self.estimated_bytes
        )
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfRustystemMemorySnapshot {
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

impl From<crate::playback::RustystemRuntimeDiagnostics> for PerfRustystemMemorySnapshot {
    fn from(value: crate::playback::RustystemRuntimeDiagnostics) -> Self {
        Self {
            instance_count: value.instance_count,
            soundfont_wave_bytes: value.soundfont_wave_bytes,
            voice_buffer_bytes: value.voice_buffer_bytes,
            block_buffer_bytes: value.block_buffer_bytes,
            effects_bytes: value.effects_bytes,
            preset_lookup_bytes: value.preset_lookup_bytes,
            channel_bytes: value.channel_bytes,
            stem_effect_map_bytes: value.stem_effect_map_bytes,
            block_stem_vec_bytes: value.block_stem_vec_bytes,
            estimated_bytes: value.estimated_bytes,
        }
    }
}

impl PerfRustystemMemorySnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"instance_count\":{},\"soundfont_wave_bytes\":{},\"voice_buffer_bytes\":{},\"block_buffer_bytes\":{},\"effects_bytes\":{},\"preset_lookup_bytes\":{},\"channel_bytes\":{},\"stem_effect_map_bytes\":{},\"block_stem_vec_bytes\":{},\"estimated_bytes\":{}}}",
            self.instance_count,
            self.soundfont_wave_bytes,
            self.voice_buffer_bytes,
            self.block_buffer_bytes,
            self.effects_bytes,
            self.preset_lookup_bytes,
            self.channel_bytes,
            self.stem_effect_map_bytes,
            self.block_stem_vec_bytes,
            self.estimated_bytes
        )
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfAudioMemorySnapshot {
    pub stream_open: bool,
    pub ring_retained_bytes: usize,
    pub callback_scratch_bytes: usize,
    pub producer_render_block_bytes: u64,
    pub estimated_bytes: usize,
}

impl From<crate::playback::AudioRuntimeMemoryDiagnostics> for PerfAudioMemorySnapshot {
    fn from(value: crate::playback::AudioRuntimeMemoryDiagnostics) -> Self {
        Self {
            stream_open: value.stream_open,
            ring_retained_bytes: value.ring_retained_bytes,
            callback_scratch_bytes: value.callback_scratch_bytes,
            producer_render_block_bytes: value.producer_render_block_bytes,
            estimated_bytes: value.estimated_bytes,
        }
    }
}

impl PerfAudioMemorySnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"stream_open\":{},\"ring_retained_bytes\":{},\"callback_scratch_bytes\":{},\"producer_render_block_bytes\":{},\"estimated_bytes\":{}}}",
            self.stream_open,
            self.ring_retained_bytes,
            self.callback_scratch_bytes,
            self.producer_render_block_bytes,
            self.estimated_bytes
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfSoundFontCacheSnapshot {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub decoded_bytes: u64,
    pub resident_bytes: usize,
    pub evictions: u64,
    pub total_strong_count: usize,
    pub max_strong_count: usize,
    pub entry_refs: Vec<PerfSoundFontCacheEntrySnapshot>,
}

impl From<&SoundFontRuntimeCacheDebug> for PerfSoundFontCacheSnapshot {
    fn from(debug: &SoundFontRuntimeCacheDebug) -> Self {
        Self {
            entries: debug.entries,
            hits: debug.hits,
            misses: debug.misses,
            decoded_bytes: debug.decoded_bytes,
            resident_bytes: debug.resident_bytes,
            evictions: debug.evictions,
            total_strong_count: debug.total_strong_count,
            max_strong_count: debug.max_strong_count,
            entry_refs: debug
                .entry_refs
                .iter()
                .map(PerfSoundFontCacheEntrySnapshot::from)
                .collect(),
        }
    }
}

impl PerfSoundFontCacheSnapshot {
    fn to_json(&self) -> String {
        let entry_refs = self
            .entry_refs
            .iter()
            .map(PerfSoundFontCacheEntrySnapshot::to_json)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"entries\":{},\"hits\":{},\"misses\":{},\"decoded_bytes\":{},\"resident_bytes\":{},\"evictions\":{},\"total_strong_count\":{},\"max_strong_count\":{},\"entry_refs\":[{}]}}",
            self.entries,
            self.hits,
            self.misses,
            self.decoded_bytes,
            self.resident_bytes,
            self.evictions,
            self.total_strong_count,
            self.max_strong_count,
            entry_refs
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfSoundFontCacheEntrySnapshot {
    pub key: String,
    pub strong_count: usize,
    pub weak_count: usize,
    pub resident_bytes: usize,
}

impl From<&SoundFontRuntimeCacheEntryDebug> for PerfSoundFontCacheEntrySnapshot {
    fn from(debug: &SoundFontRuntimeCacheEntryDebug) -> Self {
        Self {
            key: debug.key.clone(),
            strong_count: debug.strong_count,
            weak_count: debug.weak_count,
            resident_bytes: debug.resident_bytes,
        }
    }
}

impl PerfSoundFontCacheEntrySnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"key\":\"{}\",\"strong\":{},\"weak\":{},\"resident_bytes\":{}}}",
            escape_json(&self.key),
            self.strong_count,
            self.weak_count,
            self.resident_bytes
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfLifecycleSnapshot {
    pub engine_replacements: u64,
    pub cumulative_replaced_engine_estimated_bytes: u64,
    pub last_replaced_engine_estimated_bytes: usize,
    pub last_replaced_engine_instances: usize,
    pub last_replaced_stem_effects: usize,
    pub last_replaced_stem_effect_bytes: usize,
}

impl From<&PlaybackDebugMetrics> for PerfLifecycleSnapshot {
    fn from(metrics: &PlaybackDebugMetrics) -> Self {
        Self {
            engine_replacements: metrics.lifecycle.engine_replacements,
            cumulative_replaced_engine_estimated_bytes: metrics
                .lifecycle
                .cumulative_replaced_engine_estimated_bytes,
            last_replaced_engine_estimated_bytes: metrics
                .lifecycle
                .last_replaced_engine_estimated_bytes,
            last_replaced_engine_instances: metrics.lifecycle.last_replaced_engine_instances,
            last_replaced_stem_effects: metrics.lifecycle.last_replaced_stem_effects,
            last_replaced_stem_effect_bytes: metrics.lifecycle.last_replaced_stem_effect_bytes,
        }
    }
}

impl PerfLifecycleSnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"engine_replacements\":{},\"cumulative_replaced_engine_estimated_bytes\":{},\"last_replaced_engine_estimated_bytes\":{},\"last_replaced_engine_instances\":{},\"last_replaced_stem_effects\":{},\"last_replaced_stem_effect_bytes\":{}}}",
            self.engine_replacements,
            self.cumulative_replaced_engine_estimated_bytes,
            self.last_replaced_engine_estimated_bytes,
            self.last_replaced_engine_instances,
            self.last_replaced_stem_effects,
            self.last_replaced_stem_effect_bytes
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfRenderChurnSnapshot {
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
    pub last_allocation_events: usize,
    pub last_deallocation_events: usize,
    pub last_frames: usize,
    pub last_rendered_blocks: usize,
    pub last_routed_blocks: usize,
    pub last_mixer_inputs: usize,
    pub last_zero_fill_blocks: usize,
    pub last_visual_strips: usize,
    pub last_stem_audio_bytes: usize,
    pub last_stem_output_blocks: usize,
    pub last_stem_output_audio_bytes: usize,
    pub last_stem_output_active_note_bytes: usize,
    pub last_stem_output_vec_bytes: usize,
    pub last_stem_internal_blocks: usize,
    pub last_stem_internal_audio_bytes: usize,
    pub last_stem_internal_active_note_bytes: usize,
    pub last_stem_internal_vec_bytes: usize,
    pub last_stem_residual_buffer_bytes: usize,
    pub last_stem_effect_input_bytes: usize,
    pub last_stem_effect_input_vec_bytes: usize,
    pub last_stem_output_growth_bytes: usize,
    pub last_stem_internal_growth_bytes: usize,
    pub last_stem_residual_growth_bytes: usize,
    pub last_stem_effect_input_growth_bytes: usize,
    pub last_stem_playback_growth_bytes: usize,
    pub last_stem_total_growth_bytes: usize,
    pub last_stem_tracked_alloc_bytes: usize,
    pub last_stem_untracked_alloc_bytes: usize,
    pub last_zero_fill_audio_bytes: usize,
    pub last_mixer_frame_bytes: usize,
    pub last_mixer_internal_alloc_bytes: usize,
    pub last_mixer_internal_dealloc_bytes: usize,
    pub last_mixer_scratch_growth_bytes: usize,
    pub last_mixer_output_bytes: usize,
    pub last_mixer_prepared_frame_bytes: usize,
    pub last_mixer_report_bytes: usize,
    pub last_vec_overhead_bytes: usize,
    pub last_retained_container_capacity_bytes: usize,
    pub last_temp_container_alloc_bytes: usize,
    pub last_retained_capacity_growth_bytes: usize,
    pub last_transient_alloc_bytes: usize,
    pub last_transient_dealloc_bytes: usize,
    pub last_transient_growth_bytes: usize,
}

impl From<&PlaybackDebugMetrics> for PerfRenderChurnSnapshot {
    fn from(metrics: &PlaybackDebugMetrics) -> Self {
        let churn = &metrics.render_churn;
        Self {
            render_calls: churn.render_calls,
            cumulative_allocation_events: churn.cumulative_allocation_events,
            cumulative_deallocation_events: churn.cumulative_deallocation_events,
            cumulative_transient_alloc_bytes: churn.cumulative_transient_alloc_bytes,
            cumulative_transient_dealloc_bytes: churn.cumulative_transient_dealloc_bytes,
            cumulative_transient_growth_bytes: churn.cumulative_transient_growth_bytes,
            cumulative_retained_capacity_growth_bytes: churn
                .cumulative_retained_capacity_growth_bytes,
            cumulative_temp_container_alloc_bytes: churn.cumulative_temp_container_alloc_bytes,
            max_transient_alloc_bytes: churn.max_transient_alloc_bytes,
            max_transient_growth_bytes: churn.max_transient_growth_bytes,
            max_retained_capacity_growth_bytes: churn.max_retained_capacity_growth_bytes,
            max_temp_container_alloc_bytes: churn.max_temp_container_alloc_bytes,
            last_allocation_events: churn.last.allocation_events,
            last_deallocation_events: churn.last.deallocation_events,
            last_frames: churn.last.frames,
            last_rendered_blocks: churn.last.rendered_blocks,
            last_routed_blocks: churn.last.routed_blocks,
            last_mixer_inputs: churn.last.mixer_inputs,
            last_zero_fill_blocks: churn.last.zero_fill_blocks,
            last_visual_strips: churn.last.visual_strips,
            last_stem_audio_bytes: churn.last.stem_audio_bytes,
            last_stem_output_blocks: churn.last.stem_output_blocks,
            last_stem_output_audio_bytes: churn.last.stem_output_audio_bytes,
            last_stem_output_active_note_bytes: churn.last.stem_output_active_note_bytes,
            last_stem_output_vec_bytes: churn.last.stem_output_vec_bytes,
            last_stem_internal_blocks: churn.last.stem_internal_blocks,
            last_stem_internal_audio_bytes: churn.last.stem_internal_audio_bytes,
            last_stem_internal_active_note_bytes: churn.last.stem_internal_active_note_bytes,
            last_stem_internal_vec_bytes: churn.last.stem_internal_vec_bytes,
            last_stem_residual_buffer_bytes: churn.last.stem_residual_buffer_bytes,
            last_stem_effect_input_bytes: churn.last.stem_effect_input_bytes,
            last_stem_effect_input_vec_bytes: churn.last.stem_effect_input_vec_bytes,
            last_stem_output_growth_bytes: churn.last.stem_output_growth_bytes,
            last_stem_internal_growth_bytes: churn.last.stem_internal_growth_bytes,
            last_stem_residual_growth_bytes: churn.last.stem_residual_growth_bytes,
            last_stem_effect_input_growth_bytes: churn.last.stem_effect_input_growth_bytes,
            last_stem_playback_growth_bytes: churn.last.stem_playback_growth_bytes,
            last_stem_total_growth_bytes: churn.last.stem_total_growth_bytes,
            last_stem_tracked_alloc_bytes: churn.last.stem_tracked_alloc_bytes,
            last_stem_untracked_alloc_bytes: churn.last.stem_untracked_alloc_bytes,
            last_zero_fill_audio_bytes: churn.last.zero_fill_audio_bytes,
            last_mixer_frame_bytes: churn.last.mixer_frame_bytes,
            last_mixer_internal_alloc_bytes: churn.last.mixer_internal_alloc_bytes,
            last_mixer_internal_dealloc_bytes: churn.last.mixer_internal_dealloc_bytes,
            last_mixer_scratch_growth_bytes: churn.last.mixer_scratch_growth_bytes,
            last_mixer_output_bytes: churn.last.mixer_output_bytes,
            last_mixer_prepared_frame_bytes: churn.last.mixer_prepared_frame_bytes,
            last_mixer_report_bytes: churn.last.mixer_report_bytes,
            last_vec_overhead_bytes: churn.last.vec_overhead_bytes,
            last_retained_container_capacity_bytes: churn.last.retained_container_capacity_bytes,
            last_temp_container_alloc_bytes: churn.last.temp_container_alloc_bytes,
            last_retained_capacity_growth_bytes: churn.last.retained_capacity_growth_bytes,
            last_transient_alloc_bytes: churn.last.transient_alloc_bytes,
            last_transient_dealloc_bytes: churn.last.transient_dealloc_bytes,
            last_transient_growth_bytes: churn.last.transient_growth_bytes,
        }
    }
}

impl PerfRenderChurnSnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"render_calls\":{},\"cumulative_allocation_events\":{},\"cumulative_deallocation_events\":{},\"cumulative_transient_alloc_bytes\":{},\"cumulative_transient_dealloc_bytes\":{},\"cumulative_transient_growth_bytes\":{},\"cumulative_retained_capacity_growth_bytes\":{},\"cumulative_temp_container_alloc_bytes\":{},\"max_transient_alloc_bytes\":{},\"max_transient_growth_bytes\":{},\"max_retained_capacity_growth_bytes\":{},\"max_temp_container_alloc_bytes\":{},\"last_allocation_events\":{},\"last_deallocation_events\":{},\"last_frames\":{},\"last_rendered_blocks\":{},\"last_routed_blocks\":{},\"last_mixer_inputs\":{},\"last_zero_fill_blocks\":{},\"last_visual_strips\":{},\"last_stem_audio_bytes\":{},\"last_stem_output_blocks\":{},\"last_stem_output_audio_bytes\":{},\"last_stem_output_active_note_bytes\":{},\"last_stem_output_vec_bytes\":{},\"last_stem_internal_blocks\":{},\"last_stem_internal_audio_bytes\":{},\"last_stem_internal_active_note_bytes\":{},\"last_stem_internal_vec_bytes\":{},\"last_stem_residual_buffer_bytes\":{},\"last_stem_effect_input_bytes\":{},\"last_stem_effect_input_vec_bytes\":{},\"last_stem_output_growth_bytes\":{},\"last_stem_internal_growth_bytes\":{},\"last_stem_residual_growth_bytes\":{},\"last_stem_effect_input_growth_bytes\":{},\"last_stem_playback_growth_bytes\":{},\"last_stem_total_growth_bytes\":{},\"last_stem_tracked_alloc_bytes\":{},\"last_stem_untracked_alloc_bytes\":{},\"last_zero_fill_audio_bytes\":{},\"last_mixer_frame_bytes\":{},\"last_mixer_internal_alloc_bytes\":{},\"last_mixer_internal_dealloc_bytes\":{},\"last_mixer_scratch_growth_bytes\":{},\"last_mixer_output_bytes\":{},\"last_mixer_prepared_frame_bytes\":{},\"last_mixer_report_bytes\":{},\"last_vec_overhead_bytes\":{},\"last_retained_container_capacity_bytes\":{},\"last_temp_container_alloc_bytes\":{},\"last_retained_capacity_growth_bytes\":{},\"last_transient_alloc_bytes\":{},\"last_transient_dealloc_bytes\":{},\"last_transient_growth_bytes\":{}}}",
            self.render_calls,
            self.cumulative_allocation_events,
            self.cumulative_deallocation_events,
            self.cumulative_transient_alloc_bytes,
            self.cumulative_transient_dealloc_bytes,
            self.cumulative_transient_growth_bytes,
            self.cumulative_retained_capacity_growth_bytes,
            self.cumulative_temp_container_alloc_bytes,
            self.max_transient_alloc_bytes,
            self.max_transient_growth_bytes,
            self.max_retained_capacity_growth_bytes,
            self.max_temp_container_alloc_bytes,
            self.last_allocation_events,
            self.last_deallocation_events,
            self.last_frames,
            self.last_rendered_blocks,
            self.last_routed_blocks,
            self.last_mixer_inputs,
            self.last_zero_fill_blocks,
            self.last_visual_strips,
            self.last_stem_audio_bytes,
            self.last_stem_output_blocks,
            self.last_stem_output_audio_bytes,
            self.last_stem_output_active_note_bytes,
            self.last_stem_output_vec_bytes,
            self.last_stem_internal_blocks,
            self.last_stem_internal_audio_bytes,
            self.last_stem_internal_active_note_bytes,
            self.last_stem_internal_vec_bytes,
            self.last_stem_residual_buffer_bytes,
            self.last_stem_effect_input_bytes,
            self.last_stem_effect_input_vec_bytes,
            self.last_stem_output_growth_bytes,
            self.last_stem_internal_growth_bytes,
            self.last_stem_residual_growth_bytes,
            self.last_stem_effect_input_growth_bytes,
            self.last_stem_playback_growth_bytes,
            self.last_stem_total_growth_bytes,
            self.last_stem_tracked_alloc_bytes,
            self.last_stem_untracked_alloc_bytes,
            self.last_zero_fill_audio_bytes,
            self.last_mixer_frame_bytes,
            self.last_mixer_internal_alloc_bytes,
            self.last_mixer_internal_dealloc_bytes,
            self.last_mixer_scratch_growth_bytes,
            self.last_mixer_output_bytes,
            self.last_mixer_prepared_frame_bytes,
            self.last_mixer_report_bytes,
            self.last_vec_overhead_bytes,
            self.last_retained_container_capacity_bytes,
            self.last_temp_container_alloc_bytes,
            self.last_retained_capacity_growth_bytes,
            self.last_transient_alloc_bytes,
            self.last_transient_dealloc_bytes,
            self.last_transient_growth_bytes
        )
    }
}

#[derive(Debug, Clone)]
pub struct PerfMemorySnapshot {
    pub midi_bytes: usize,
    pub midi_capacity_bytes: usize,
    pub instance_count: usize,
    pub soundfont_wave_bytes: usize,
    pub soundfont_metadata_items: usize,
    pub total_voices: usize,
    pub active_voices: usize,
    pub voice_buffer_bytes: usize,
    pub block_buffer_bytes: usize,
    pub retained_stem_blocks: usize,
    pub retained_stem_block_bytes: usize,
    pub stem_effects: usize,
    pub stem_effect_bytes: usize,
    pub stem_effect_allocations: u64,
    pub stem_effect_deallocations: u64,
    pub stem_effect_allocated_bytes: u64,
    pub stem_effect_deallocated_bytes: u64,
    pub stem_effect_cache_clears: u64,
    pub stem_effect_cache_released_bytes: u64,
    pub last_stem_render_allocations: StemRenderAllocationDebug,
    pub effects_bytes: usize,
    pub preset_lookup_bytes: usize,
    pub channel_bytes: usize,
    pub stem_effect_map_bytes: usize,
    pub block_stem_vec_bytes: usize,
    pub estimated_bytes: usize,
    pub instances: Vec<PerfMemoryInstanceSnapshot>,
}

impl PerfMemorySnapshot {
    fn from_playback(
        memory: &PlaybackMemoryDebug,
        midi_bytes: usize,
        midi_capacity_bytes: usize,
    ) -> Self {
        Self {
            midi_bytes,
            midi_capacity_bytes,
            instance_count: memory.instance_count,
            soundfont_wave_bytes: memory.soundfont_wave_bytes,
            soundfont_metadata_items: memory.soundfont_metadata_items,
            total_voices: memory.total_voices,
            active_voices: memory.active_voices,
            voice_buffer_bytes: memory.voice_buffer_bytes,
            block_buffer_bytes: memory.block_buffer_bytes,
            retained_stem_blocks: memory.retained_stem_blocks,
            retained_stem_block_bytes: memory.retained_stem_block_bytes,
            stem_effects: memory.stem_effects,
            stem_effect_bytes: memory.stem_effect_bytes,
            stem_effect_allocations: memory.stem_effect_allocations,
            stem_effect_deallocations: memory.stem_effect_deallocations,
            stem_effect_allocated_bytes: memory.stem_effect_allocated_bytes,
            stem_effect_deallocated_bytes: memory.stem_effect_deallocated_bytes,
            stem_effect_cache_clears: memory.stem_effect_cache_clears,
            stem_effect_cache_released_bytes: memory.stem_effect_cache_released_bytes,
            last_stem_render_allocations: memory.last_stem_render_allocations,
            effects_bytes: memory.effects_bytes,
            preset_lookup_bytes: memory.preset_lookup_bytes,
            channel_bytes: memory.channel_bytes,
            stem_effect_map_bytes: memory.stem_effect_map_bytes,
            block_stem_vec_bytes: memory.block_stem_vec_bytes,
            estimated_bytes: memory.estimated_bytes,
            instances: memory
                .instances
                .iter()
                .map(PerfMemoryInstanceSnapshot::from)
                .collect(),
        }
    }

    fn to_json(&self) -> String {
        let mut json = format!(
            "{{\"midi_bytes\":{},\"midi_capacity_bytes\":{},\"instances\":{},\"soundfont_wave_bytes\":{},\"soundfont_metadata_items\":{},\"total_voices\":{},\"active_voices\":{},\"voice_buffer_bytes\":{},\"block_buffer_bytes\":{},\"retained_stem_blocks\":{},\"retained_stem_block_bytes\":{},\"stem_effects\":{},\"stem_effect_bytes\":{},\"stem_effect_allocations\":{},\"stem_effect_deallocations\":{},\"stem_effect_allocated_bytes\":{},\"stem_effect_deallocated_bytes\":{},\"stem_effect_cache_clears\":{},\"stem_effect_cache_released_bytes\":{},\"last_stem_render_allocations\":{},\"effects_bytes\":{},\"preset_lookup_bytes\":{},\"channel_bytes\":{},\"stem_effect_map_bytes\":{},\"block_stem_vec_bytes\":{},\"estimated_bytes\":{}",
            self.midi_bytes,
            self.midi_capacity_bytes,
            self.instance_count,
            self.soundfont_wave_bytes,
            self.soundfont_metadata_items,
            self.total_voices,
            self.active_voices,
            self.voice_buffer_bytes,
            self.block_buffer_bytes,
            self.retained_stem_blocks,
            self.retained_stem_block_bytes,
            self.stem_effects,
            self.stem_effect_bytes,
            self.stem_effect_allocations,
            self.stem_effect_deallocations,
            self.stem_effect_allocated_bytes,
            self.stem_effect_deallocated_bytes,
            self.stem_effect_cache_clears,
            self.stem_effect_cache_released_bytes,
            stem_render_allocations_json(self.last_stem_render_allocations),
            self.effects_bytes,
            self.preset_lookup_bytes,
            self.channel_bytes,
            self.stem_effect_map_bytes,
            self.block_stem_vec_bytes,
            self.estimated_bytes
        );
        if !self.instances.is_empty() {
            json.push_str(",\"instance_details\":[");
            for (index, instance) in self.instances.iter().enumerate() {
                if index > 0 {
                    json.push(',');
                }
                json.push_str(&instance.to_json());
            }
            json.push(']');
        }
        json.push('}');
        json
    }
}

#[derive(Debug, Clone)]
pub struct PerfMemoryInstanceSnapshot {
    pub internal_id: String,
    pub display_name: String,
    pub soundfont_wave_bytes: usize,
    pub soundfont_sample_headers: usize,
    pub soundfont_presets: usize,
    pub soundfont_instruments: usize,
    pub total_voices: usize,
    pub active_voices: usize,
    pub voice_buffer_bytes: usize,
    pub retained_stem_blocks: usize,
    pub retained_stem_block_bytes: usize,
    pub stem_effects: usize,
    pub stem_effect_bytes: usize,
    pub stem_effect_allocations: u64,
    pub stem_effect_deallocations: u64,
    pub stem_effect_allocated_bytes: u64,
    pub stem_effect_deallocated_bytes: u64,
    pub stem_effect_cache_clears: u64,
    pub stem_effect_cache_released_bytes: u64,
    pub last_stem_render_allocations: StemRenderAllocationDebug,
    pub effects_bytes: usize,
    pub preset_lookup_bytes: usize,
    pub channel_bytes: usize,
    pub stem_effect_map_bytes: usize,
    pub block_stem_vec_bytes: usize,
    pub estimated_bytes: usize,
}

impl From<&SynthInstanceMemoryDebug> for PerfMemoryInstanceSnapshot {
    fn from(instance: &SynthInstanceMemoryDebug) -> Self {
        Self {
            internal_id: instance.internal_id.clone(),
            display_name: instance.display_name.clone(),
            soundfont_wave_bytes: instance.synth.soundfont_wave_bytes,
            soundfont_sample_headers: instance.synth.soundfont_sample_headers,
            soundfont_presets: instance.synth.soundfont_presets,
            soundfont_instruments: instance.synth.soundfont_instruments,
            total_voices: instance.synth.total_voices,
            active_voices: instance.synth.active_voices,
            voice_buffer_bytes: instance.synth.voice_buffer_bytes,
            retained_stem_blocks: instance.synth.retained_stem_blocks,
            retained_stem_block_bytes: instance.synth.retained_stem_block_bytes,
            stem_effects: instance.synth.stem_effects,
            stem_effect_bytes: instance.synth.stem_effect_bytes,
            stem_effect_allocations: instance.synth.stem_effect_allocations,
            stem_effect_deallocations: instance.synth.stem_effect_deallocations,
            stem_effect_allocated_bytes: instance.synth.stem_effect_allocated_bytes,
            stem_effect_deallocated_bytes: instance.synth.stem_effect_deallocated_bytes,
            stem_effect_cache_clears: instance.synth.stem_effect_cache_clears,
            stem_effect_cache_released_bytes: instance.synth.stem_effect_cache_released_bytes,
            last_stem_render_allocations: instance.synth.last_stem_render_allocations,
            effects_bytes: instance.synth.effects_bytes,
            preset_lookup_bytes: instance.synth.preset_lookup_bytes,
            channel_bytes: instance.synth.channel_bytes,
            stem_effect_map_bytes: instance.synth.stem_effect_map_bytes,
            block_stem_vec_bytes: instance.synth.block_stem_vec_bytes,
            estimated_bytes: instance.synth.estimated_bytes,
        }
    }
}

impl PerfMemoryInstanceSnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"id\":\"{}\",\"display\":\"{}\",\"soundfont_wave_bytes\":{},\"soundfont_sample_headers\":{},\"soundfont_presets\":{},\"soundfont_instruments\":{},\"total_voices\":{},\"active_voices\":{},\"voice_buffer_bytes\":{},\"retained_stem_blocks\":{},\"retained_stem_block_bytes\":{},\"stem_effects\":{},\"stem_effect_bytes\":{},\"stem_effect_allocations\":{},\"stem_effect_deallocations\":{},\"stem_effect_allocated_bytes\":{},\"stem_effect_deallocated_bytes\":{},\"stem_effect_cache_clears\":{},\"stem_effect_cache_released_bytes\":{},\"last_stem_render_allocations\":{},\"effects_bytes\":{},\"preset_lookup_bytes\":{},\"channel_bytes\":{},\"stem_effect_map_bytes\":{},\"block_stem_vec_bytes\":{},\"estimated_bytes\":{}}}",
            escape_json(&self.internal_id),
            escape_json(&self.display_name),
            self.soundfont_wave_bytes,
            self.soundfont_sample_headers,
            self.soundfont_presets,
            self.soundfont_instruments,
            self.total_voices,
            self.active_voices,
            self.voice_buffer_bytes,
            self.retained_stem_blocks,
            self.retained_stem_block_bytes,
            self.stem_effects,
            self.stem_effect_bytes,
            self.stem_effect_allocations,
            self.stem_effect_deallocations,
            self.stem_effect_allocated_bytes,
            self.stem_effect_deallocated_bytes,
            self.stem_effect_cache_clears,
            self.stem_effect_cache_released_bytes,
            stem_render_allocations_json(self.last_stem_render_allocations),
            self.effects_bytes,
            self.preset_lookup_bytes,
            self.channel_bytes,
            self.stem_effect_map_bytes,
            self.block_stem_vec_bytes,
            self.estimated_bytes
        )
    }
}

fn stem_render_allocations_json(allocations: StemRenderAllocationDebug) -> String {
    format!(
        "{{\"output_block_count\":{},\"output_audio_bytes\":{},\"output_active_note_bytes\":{},\"output_vec_bytes\":{},\"internal_block_count\":{},\"internal_audio_bytes\":{},\"internal_active_note_bytes\":{},\"internal_vec_bytes\":{},\"residual_buffer_bytes\":{},\"effect_input_bytes\":{},\"effect_input_vec_bytes\":{},\"output_growth_bytes\":{},\"internal_growth_bytes\":{},\"residual_growth_bytes\":{},\"effect_input_growth_bytes\":{},\"total_bytes\":{},\"total_growth_bytes\":{}}}",
        allocations.output_block_count,
        allocations.output_audio_bytes,
        allocations.output_active_note_bytes,
        allocations.output_vec_bytes,
        allocations.internal_block_count,
        allocations.internal_audio_bytes,
        allocations.internal_active_note_bytes,
        allocations.internal_vec_bytes,
        allocations.residual_buffer_bytes,
        allocations.effect_input_bytes,
        allocations.effect_input_vec_bytes,
        allocations.output_growth_bytes,
        allocations.internal_growth_bytes,
        allocations.residual_growth_bytes,
        allocations.effect_input_growth_bytes,
        allocations.total_bytes(),
        allocations.total_growth_bytes()
    )
}

#[derive(Debug, Clone)]
struct ProcessMetricSampler {
    #[cfg(debug_assertions)]
    last_sample_at: Instant,
    #[cfg(debug_assertions)]
    last_cpu_time_ms: Option<u128>,
    #[cfg(debug_assertions)]
    last_read_bytes: Option<u64>,
    #[cfg(debug_assertions)]
    last_write_bytes: Option<u64>,
    #[cfg(debug_assertions)]
    last_page_fault_count: Option<u32>,
}

impl ProcessMetricSampler {
    fn new(#[cfg_attr(not(debug_assertions), allow(unused_variables))] now: Instant) -> Self {
        #[cfg(not(debug_assertions))]
        {
            Self {}
        }
        #[cfg(debug_assertions)]
        {
            Self {
                last_sample_at: now,
                last_cpu_time_ms: None,
                last_read_bytes: None,
                last_write_bytes: None,
                last_page_fault_count: None,
            }
        }
    }

    fn sample(&mut self) -> Option<PerfProcessSnapshot> {
        #[cfg(not(debug_assertions))]
        {
            None
        }
        #[cfg(debug_assertions)]
        {
            let now = Instant::now();
            let raw = process_resource_snapshot()?;
            let elapsed_ms = now.duration_since(self.last_sample_at).as_millis();
            let cpu_percent = self
                .last_cpu_time_ms
                .and_then(|previous| raw.cpu_time_ms.checked_sub(previous))
                .and_then(|delta| process_cpu_percent(delta, elapsed_ms));

            let read_delta_bytes = self
                .last_read_bytes
                .map(|previous| raw.read_bytes.saturating_sub(previous));
            let write_delta_bytes = self
                .last_write_bytes
                .map(|previous| raw.write_bytes.saturating_sub(previous));
            let page_fault_delta = self
                .last_page_fault_count
                .map(|previous| raw.page_fault_count.saturating_sub(previous));
            let read_bytes_per_sec = bytes_per_second(read_delta_bytes, elapsed_ms);
            let write_bytes_per_sec = bytes_per_second(write_delta_bytes, elapsed_ms);
            let page_faults_per_sec = count_per_second(page_fault_delta, elapsed_ms);

            self.last_sample_at = now;
            self.last_cpu_time_ms = Some(raw.cpu_time_ms);
            self.last_read_bytes = Some(raw.read_bytes);
            self.last_write_bytes = Some(raw.write_bytes);
            self.last_page_fault_count = Some(raw.page_fault_count);

            Some(PerfProcessSnapshot {
                cpu_percent,
                cpu_time_ms: raw.cpu_time_ms,
                working_set_bytes: raw.working_set_bytes,
                peak_working_set_bytes: raw.peak_working_set_bytes,
                pagefile_bytes: raw.pagefile_bytes,
                peak_pagefile_bytes: raw.peak_pagefile_bytes,
                page_fault_count: raw.page_fault_count,
                page_fault_delta,
                page_faults_per_sec,
                thread_count: raw.thread_count,
                handle_count: raw.handle_count,
                read_bytes: raw.read_bytes,
                write_bytes: raw.write_bytes,
                read_delta_bytes,
                write_delta_bytes,
                read_bytes_per_sec,
                write_bytes_per_sec,
                native_heaps: raw.native_heaps,
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct PerfProcessSnapshot {
    pub cpu_percent: Option<f64>,
    pub cpu_time_ms: u128,
    pub working_set_bytes: u64,
    pub peak_working_set_bytes: u64,
    pub pagefile_bytes: u64,
    pub peak_pagefile_bytes: u64,
    pub page_fault_count: u32,
    pub page_fault_delta: Option<u32>,
    pub page_faults_per_sec: Option<f64>,
    pub thread_count: Option<u32>,
    pub handle_count: Option<u32>,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_delta_bytes: Option<u64>,
    pub write_delta_bytes: Option<u64>,
    pub read_bytes_per_sec: Option<f64>,
    pub write_bytes_per_sec: Option<f64>,
    pub native_heaps: Option<NativeHeapSnapshot>,
}

impl PerfProcessSnapshot {
    fn to_json(&self) -> String {
        let mut json = format!(
            "{{\"cpu_time_ms\":{},\"working_set_bytes\":{},\"peak_working_set_bytes\":{},\"pagefile_bytes\":{},\"peak_pagefile_bytes\":{},\"page_fault_count\":{},\"read_bytes\":{},\"write_bytes\":{}",
            self.cpu_time_ms,
            self.working_set_bytes,
            self.peak_working_set_bytes,
            self.pagefile_bytes,
            self.peak_pagefile_bytes,
            self.page_fault_count,
            self.read_bytes,
            self.write_bytes
        );
        push_json_f64_opt(&mut json, "cpu_percent", self.cpu_percent);
        push_json_u64_opt(&mut json, "read_delta_bytes", self.read_delta_bytes);
        push_json_u64_opt(&mut json, "write_delta_bytes", self.write_delta_bytes);
        push_json_u32_opt(&mut json, "page_fault_delta", self.page_fault_delta);
        push_json_f64_opt(&mut json, "read_bytes_per_sec", self.read_bytes_per_sec);
        push_json_f64_opt(&mut json, "write_bytes_per_sec", self.write_bytes_per_sec);
        push_json_f64_opt(&mut json, "page_faults_per_sec", self.page_faults_per_sec);
        push_json_u32_opt(&mut json, "thread_count", self.thread_count);
        push_json_u32_opt(&mut json, "handle_count", self.handle_count);
        if let Some(native_heaps) = &self.native_heaps {
            json.push_str(",\"native_heaps\":");
            json.push_str(&native_heaps.to_json());
        }
        json.push('}');
        json
    }
}

#[derive(Debug, Clone, Default)]
pub struct NativeHeapSnapshot {
    pub heap_count: u32,
    pub walked_heaps: u32,
    pub failed_heaps: u32,
    pub busy_blocks: u64,
    pub busy_bytes: u64,
    pub free_blocks: u64,
    pub free_bytes: u64,
    pub region_count: u64,
    pub committed_region_bytes: u64,
    pub uncommitted_region_bytes: u64,
    pub uncommitted_ranges: u64,
    pub uncommitted_range_bytes: u64,
    pub block_overhead_bytes: u64,
}

impl NativeHeapSnapshot {
    fn to_json(&self) -> String {
        format!(
            "{{\"heap_count\":{},\"walked_heaps\":{},\"failed_heaps\":{},\"busy_blocks\":{},\"busy_bytes\":{},\"free_blocks\":{},\"free_bytes\":{},\"region_count\":{},\"committed_region_bytes\":{},\"uncommitted_region_bytes\":{},\"uncommitted_ranges\":{},\"uncommitted_range_bytes\":{},\"block_overhead_bytes\":{}}}",
            self.heap_count,
            self.walked_heaps,
            self.failed_heaps,
            self.busy_blocks,
            self.busy_bytes,
            self.free_blocks,
            self.free_bytes,
            self.region_count,
            self.committed_region_bytes,
            self.uncommitted_region_bytes,
            self.uncommitted_ranges,
            self.uncommitted_range_bytes,
            self.block_overhead_bytes
        )
    }
}

#[derive(Debug, Clone)]
#[cfg(debug_assertions)]
struct RawProcessResourceSnapshot {
    cpu_time_ms: u128,
    working_set_bytes: u64,
    peak_working_set_bytes: u64,
    pagefile_bytes: u64,
    peak_pagefile_bytes: u64,
    page_fault_count: u32,
    thread_count: Option<u32>,
    handle_count: Option<u32>,
    read_bytes: u64,
    write_bytes: u64,
    native_heaps: Option<NativeHeapSnapshot>,
}

#[cfg(debug_assertions)]
fn process_cpu_percent(delta_cpu_ms: u128, elapsed_ms: u128) -> Option<f64> {
    if elapsed_ms == 0 {
        return None;
    }
    let parallelism = std::thread::available_parallelism()
        .map(|value| value.get() as f64)
        .unwrap_or(1.0);
    Some((delta_cpu_ms as f64 / (elapsed_ms as f64 * parallelism)) * 100.0)
}

#[cfg(debug_assertions)]
fn bytes_per_second(delta_bytes: Option<u64>, elapsed_ms: u128) -> Option<f64> {
    if elapsed_ms == 0 {
        return None;
    }
    delta_bytes.map(|bytes| bytes as f64 * 1_000.0 / elapsed_ms as f64)
}

#[cfg(debug_assertions)]
fn count_per_second(delta: Option<u32>, elapsed_ms: u128) -> Option<f64> {
    if elapsed_ms == 0 {
        return None;
    }
    delta.map(|count| count as f64 * 1_000.0 / elapsed_ms as f64)
}

#[cfg(all(debug_assertions, windows))]
fn process_resource_snapshot() -> Option<RawProcessResourceSnapshot> {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS,
    };

    let process = unsafe { GetCurrentProcess() };

    let mut creation_time: FILETIME = unsafe { zeroed() };
    let mut exit_time: FILETIME = unsafe { zeroed() };
    let mut kernel_time: FILETIME = unsafe { zeroed() };
    let mut user_time: FILETIME = unsafe { zeroed() };
    if unsafe {
        GetProcessTimes(
            process,
            &mut creation_time,
            &mut exit_time,
            &mut kernel_time,
            &mut user_time,
        )
    } == 0
    {
        return None;
    }

    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    memory.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    if unsafe { GetProcessMemoryInfo(process, &mut memory, memory.cb) } == 0 {
        return None;
    }

    let mut io_counters: IO_COUNTERS = unsafe { zeroed() };
    if unsafe { GetProcessIoCounters(process, &mut io_counters) } == 0 {
        return None;
    }

    Some(RawProcessResourceSnapshot {
        cpu_time_ms: (filetime_100ns(&kernel_time) + filetime_100ns(&user_time)) / 10_000,
        working_set_bytes: memory.WorkingSetSize as u64,
        peak_working_set_bytes: memory.PeakWorkingSetSize as u64,
        pagefile_bytes: memory.PagefileUsage as u64,
        peak_pagefile_bytes: memory.PeakPagefileUsage as u64,
        page_fault_count: memory.PageFaultCount,
        thread_count: current_process_thread_count(),
        handle_count: current_process_handle_count(process),
        read_bytes: io_counters.ReadTransferCount,
        write_bytes: io_counters.WriteTransferCount,
        native_heaps: current_process_heap_snapshot(),
    })
}

#[cfg(all(debug_assertions, windows))]
fn current_process_heap_snapshot() -> Option<NativeHeapSnapshot> {
    use std::{mem::zeroed, ptr};

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Memory::{
        GetProcessHeaps, HeapLock, HeapUnlock, HeapWalk, PROCESS_HEAP_ENTRY,
    };

    const PROCESS_HEAP_REGION: u32 = 1;
    const PROCESS_HEAP_UNCOMMITTED_RANGE: u32 = 2;
    const PROCESS_HEAP_ENTRY_BUSY: u32 = 4;

    let heap_count = unsafe { GetProcessHeaps(0, ptr::null_mut()) };
    if heap_count == 0 {
        return None;
    }
    let mut heaps = vec![0 as HANDLE; heap_count as usize];
    let copied = unsafe { GetProcessHeaps(heap_count, heaps.as_mut_ptr()) };
    if copied == 0 {
        return None;
    }
    heaps.truncate(copied as usize);

    let mut snapshot = NativeHeapSnapshot {
        heap_count: copied,
        ..NativeHeapSnapshot::default()
    };

    for heap in heaps {
        if unsafe { HeapLock(heap) } == 0 {
            snapshot.failed_heaps = snapshot.failed_heaps.saturating_add(1);
            continue;
        }
        snapshot.walked_heaps = snapshot.walked_heaps.saturating_add(1);
        let mut entry: PROCESS_HEAP_ENTRY = unsafe { zeroed() };
        loop {
            if unsafe { HeapWalk(heap, &mut entry) } == 0 {
                break;
            }
            let flags = entry.wFlags as u32;
            if flags & PROCESS_HEAP_ENTRY_BUSY != 0 {
                snapshot.busy_blocks = snapshot.busy_blocks.saturating_add(1);
                snapshot.busy_bytes = snapshot.busy_bytes.saturating_add(entry.cbData as u64);
                snapshot.block_overhead_bytes = snapshot
                    .block_overhead_bytes
                    .saturating_add(entry.cbOverhead as u64);
            } else if flags & PROCESS_HEAP_REGION != 0 {
                snapshot.region_count = snapshot.region_count.saturating_add(1);
                let region = unsafe { entry.Anonymous.Region };
                snapshot.committed_region_bytes = snapshot
                    .committed_region_bytes
                    .saturating_add(region.dwCommittedSize as u64);
                snapshot.uncommitted_region_bytes = snapshot
                    .uncommitted_region_bytes
                    .saturating_add(region.dwUnCommittedSize as u64);
            } else if flags & PROCESS_HEAP_UNCOMMITTED_RANGE != 0 {
                snapshot.uncommitted_ranges = snapshot.uncommitted_ranges.saturating_add(1);
                snapshot.uncommitted_range_bytes = snapshot
                    .uncommitted_range_bytes
                    .saturating_add(entry.cbData as u64);
            } else {
                snapshot.free_blocks = snapshot.free_blocks.saturating_add(1);
                snapshot.free_bytes = snapshot.free_bytes.saturating_add(entry.cbData as u64);
            }
        }
        unsafe {
            HeapUnlock(heap);
        }
    }

    Some(snapshot)
}

#[cfg(all(debug_assertions, windows))]
fn current_process_handle_count(process: windows_sys::Win32::Foundation::HANDLE) -> Option<u32> {
    use windows_sys::Win32::System::Threading::GetProcessHandleCount;

    let mut count = 0u32;
    if unsafe { GetProcessHandleCount(process, &mut count) } == 0 {
        None
    } else {
        Some(count)
    }
}

#[cfg(all(debug_assertions, windows))]
fn current_process_thread_count() -> Option<u32> {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }

    let current_pid = unsafe { GetCurrentProcessId() };
    let mut entry: THREADENTRY32 = unsafe { zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    let mut count = 0u32;
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == current_pid {
            count = count.saturating_add(1);
        }
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe {
        CloseHandle(snapshot);
    }
    Some(count)
}

#[cfg(all(debug_assertions, not(windows)))]
fn process_resource_snapshot() -> Option<RawProcessResourceSnapshot> {
    None
}

#[cfg(all(debug_assertions, windows))]
fn filetime_100ns(filetime: &windows_sys::Win32::Foundation::FILETIME) -> u128 {
    ((filetime.dwHighDateTime as u128) << 32) | filetime.dwLowDateTime as u128
}

fn append_record(path: &Path, record: &PerfTraceRecord) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", record.to_json_line())
}

fn append_json_line(path: &Path, json: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{json}")
}

fn push_bounded(records: &mut VecDeque<PerfTraceRecord>, record: PerfTraceRecord, max_len: usize) {
    while records.len() >= max_len {
        records.pop_front();
    }
    records.push_back(record);
}

fn default_log_path(session_id: &str) -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_owned))
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(format!("flutz-session-{session_id}.jsonl"))
}

#[cfg(debug_assertions)]
fn default_latency_log_path(session_id: &str) -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_owned))
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(format!("flutz-latency-{session_id}.jsonl"))
}

fn wall_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn allocation_trace_json(snapshot: &AllocationTraceSnapshot) -> String {
    let mut json = format!(
        "{{\"alloc_calls\":{},\"dealloc_calls\":{},\"realloc_calls\":{},\"alloc_bytes\":{},\"dealloc_bytes\":{},\"realloc_in_bytes\":{},\"realloc_out_bytes\":{},\"live_bytes\":{},\"peak_live_bytes\":{},\"outstanding_allocations\":{}",
        snapshot.alloc_calls,
        snapshot.dealloc_calls,
        snapshot.realloc_calls,
        snapshot.alloc_bytes,
        snapshot.dealloc_bytes,
        snapshot.realloc_in_bytes,
        snapshot.realloc_out_bytes,
        snapshot.live_bytes,
        snapshot.peak_live_bytes,
        snapshot.outstanding_allocations
    );
    json.push_str(",\"scopes\":[");
    let mut first_scope = true;
    for scope in snapshot.scopes.iter().filter(|scope| {
        scope.alloc_calls != 0
            || scope.dealloc_calls != 0
            || scope.realloc_calls != 0
            || scope.net_bytes != 0
    }) {
        if !first_scope {
            json.push(',');
        }
        first_scope = false;
        json.push_str(&format!(
            "{{\"name\":\"{}\",\"alloc_calls\":{},\"dealloc_calls\":{},\"realloc_calls\":{},\"alloc_bytes\":{},\"dealloc_bytes\":{},\"realloc_in_bytes\":{},\"realloc_out_bytes\":{},\"net_bytes\":{},\"peak_net_bytes\":{}}}",
            escape_json(scope.label),
            scope.alloc_calls,
            scope.dealloc_calls,
            scope.realloc_calls,
            scope.alloc_bytes,
            scope.dealloc_bytes,
            scope.realloc_in_bytes,
            scope.realloc_out_bytes,
            scope.net_bytes,
            scope.peak_net_bytes
        ));
    }
    json.push_str("],\"stack_buckets\":[");
    let mut first_bucket = true;
    for bucket in &snapshot.stack_buckets {
        if !first_bucket {
            json.push(',');
        }
        first_bucket = false;
        json.push_str(&format!(
            "{{\"hash\":\"{:016x}\",\"scope\":\"{}\",\"sample_calls\":{},\"sample_bytes\":{},\"max_sample_bytes\":{}}}",
            bucket.hash,
            escape_json(bucket.scope),
            bucket.sample_calls,
            bucket.sample_bytes,
            bucket.max_sample_bytes
        ));
    }
    json.push_str("]}");
    json
}

fn push_json_string_opt(json: &mut String, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        json.push_str(&format!(",\"{}\":\"{}\"", key, escape_json(value)));
    }
}

fn push_json_u64_opt(json: &mut String, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        json.push_str(&format!(",\"{}\":{}", key, value));
    }
}

fn push_json_u32_opt(json: &mut String, key: &str, value: Option<u32>) {
    if let Some(value) = value {
        json.push_str(&format!(",\"{}\":{}", key, value));
    }
}

fn push_json_f64_opt(json: &mut String, key: &str, value: Option<f64>) {
    if let Some(value) = value.filter(|value| value.is_finite()) {
        json.push_str(&format!(",\"{}\":{:.3}", key, value));
    }
}

fn push_json_usize_opt(json: &mut String, key: &str, value: Option<usize>) {
    if let Some(value) = value {
        json.push_str(&format!(",\"{}\":{}", key, value));
    }
}

fn memory_gap_json(
    tracked_total_bytes: usize,
    memory_runtime: &MemoryRuntimeSnapshot,
    process: &PerfProcessSnapshot,
) -> String {
    let tracked = tracked_total_bytes as u64;
    let active = memory_runtime.totals.active_bytes as u64;
    let resident = memory_runtime.totals.resident_bytes as u64;
    format!(
        "{{\"tracked_total_bytes\":{},\"jemalloc_active_bytes\":{},\"jemalloc_resident_bytes\":{},\"working_set_minus_tracked_bytes\":{},\"working_set_minus_jemalloc_resident_bytes\":{},\"pagefile_minus_tracked_bytes\":{},\"pagefile_minus_jemalloc_active_bytes\":{}}}",
        tracked_total_bytes,
        memory_runtime.totals.active_bytes,
        memory_runtime.totals.resident_bytes,
        process.working_set_bytes.saturating_sub(tracked),
        process.working_set_bytes.saturating_sub(resident),
        process.pagefile_bytes.saturating_sub(tracked),
        process.pagefile_bytes.saturating_sub(active)
    )
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}
