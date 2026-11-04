use super::*;

const MAX_RECORDS: usize = 96;

const MAX_ISSUES: usize = 64;

const SAMPLE_INTERVAL_MS: u128 = 2_000;

const FRAME_STALL_MS: u128 = 120;

pub const PERF_TRACE_COMPILED: bool = cfg!(debug_assertions);

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

impl Drop for PerfTrace {
    fn drop(&mut self) {
        if self.enabled {
            self.record_marker("app.session", "exit");
        }
    }
}
