use super::*;

const LATENCY_SAMPLE_INTERVAL_MS: u128 = 1_000;

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
