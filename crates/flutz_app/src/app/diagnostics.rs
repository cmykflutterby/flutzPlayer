use super::*;

impl FlutzDesktopApp {
    pub fn run_state(&self) -> AppRunState {
        self.run_state
    }

    pub fn release_edit_mode(&self) -> bool {
        self.release_edit_mode
    }

    pub fn release_editor_panels_collapsed(&self) -> bool {
        self.release_editor_panels_collapsed
    }

    pub fn toggle_release_edit_mode(&mut self) {
        self.release_edit_mode = !self.release_edit_mode;
        self.speaker_volume_popup_open = false;
        self.release_loop_popup_open = false;
        self.status = if self.release_edit_mode {
            "Editor mode".to_owned()
        } else {
            "Player mode".to_owned()
        };
    }

    pub fn speaker_volume_popup_open(&self) -> bool {
        self.speaker_volume_popup_open
    }

    pub fn final_output_volume_percent(&self) -> f32 {
        self.final_output_volume_percent
    }

    pub fn final_output_volume_multiplier(&self) -> f32 {
        let normalized = (self.final_output_volume_percent / 100.0).clamp(-1.0, 1.0);
        if normalized <= 0.0 {
            1.0 + normalized
        } else {
            1.0 + (FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER - 1.0) * normalized
        }
    }

    pub fn toggle_speaker_volume_popup(&mut self) {
        self.speaker_volume_popup_open = !self.speaker_volume_popup_open;
    }

    pub fn set_final_output_volume_percent(&mut self, percent: f32) {
        let clamped = percent.clamp(-100.0, 100.0);
        if (self.final_output_volume_percent - clamped).abs() <= f32::EPSILON {
            return;
        }

        self.final_output_volume_percent = clamped;
        self.sync_playback_output_volume();
    }

    pub fn close_speaker_volume_popup(&mut self) {
        self.speaker_volume_popup_open = false;
    }

    pub fn release_loop_popup_open(&self) -> bool {
        self.release_loop_popup_open
    }

    pub fn toggle_release_loop_popup(&mut self) {
        self.release_loop_popup_open = !self.release_loop_popup_open;
    }

    pub fn close_release_loop_popup(&mut self) {
        self.release_loop_popup_open = false;
    }

    pub fn toggle_release_editor_panels_collapsed(&mut self) {
        self.release_editor_panels_collapsed = !self.release_editor_panels_collapsed;
    }

    pub fn set_run_state(&mut self, run_state: AppRunState) {
        self.run_state = run_state;
        self.status = match run_state {
            AppRunState::Idle => "Stopped".to_owned(),
            AppRunState::Playing => "Playing".to_owned(),
            AppRunState::Paused => "Paused".to_owned(),
        };
    }

    pub fn project_title(&self) -> &str {
        if self.current_path.is_some() && !self.metadata_edit_state.project_name.trim().is_empty() {
            return &self.metadata_edit_state.project_name;
        }
        &self.project_title
    }

    pub fn current_path(&self) -> Option<&str> {
        self.current_path.as_deref()
    }

    pub fn has_loaded_project(&self) -> bool {
        self.current_path.is_some()
    }

    pub fn active_mastering_capability(&self) -> MasteringCapability {
        self.loaded_content
            .as_ref()
            .map(|content| content.mastering)
            .unwrap_or(MasteringCapability::None)
    }

    pub fn active_content_kind(&self) -> Option<ContentKind> {
        self.loaded_content.as_ref().map(|content| content.kind)
    }

    pub fn transport_unit_label(&self) -> &'static str {
        match self.active_content_kind() {
            Some(ContentKind::DecodedAudio | ContentKind::DecodedAudioWrapper) => "sample-frames",
            Some(ContentKind::Midi | ContentKind::Fmid) => "ticks",
            None => "ticks",
        }
    }

    pub fn transport_unit_prefix(&self) -> &'static str {
        match self.active_content_kind() {
            Some(ContentKind::DecodedAudio | ContentKind::DecodedAudioWrapper) => "f:",
            Some(ContentKind::Midi | ContentKind::Fmid) => "t:",
            None => "t:",
        }
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub fn data_summary(&self) -> &str {
        &self.data_summary
    }

    pub fn playback_summary(&self) -> String {
        self.playback.status_line()
    }

    pub fn audio_status(&self) -> String {
        self.playback.audio_status_text()
    }

    pub fn audio_backend_label(&self) -> &'static str {
        match self.debug_metrics().audio_backend {
            "wasapi" => "WASAPI",
            _ => "SDL3",
        }
    }

    pub fn measured_output_latency_ms(&self) -> f64 {
        self.playback.estimated_output_latency_seconds() * 1000.0
    }

    pub fn debug_metrics(&self) -> PlaybackDebugMetrics {
        self.playback.debug_metrics()
    }

    pub fn visualizer_frame(&self) -> VisualizerFrame {
        self.playback.visualizer_frame()
    }

    pub fn perf_trace_status(&self) -> String {
        self.perf_trace.status_line()
    }

    pub fn perf_trace_records(&self) -> Vec<PerfTraceRecord> {
        self.perf_trace.records()
    }

    pub fn perf_trace_issues(&self) -> Vec<PerfTraceRecord> {
        self.perf_trace.issues()
    }

    pub fn start_perf_trace_logging(&mut self) {
        match self.perf_trace.start_logging_default() {
            Ok(path) => self.set_status(format!("Performance trace logging to {}", path.display())),
            Err(error) => self.set_status(error),
        }
    }

    pub fn stop_perf_trace_logging(&mut self) {
        self.perf_trace.stop_logging();
        self.set_status("Performance trace logging stopped");
    }

    pub fn export_perf_trace_log(&mut self) {
        match self.perf_trace.export_jsonl_default() {
            Ok(path) => {
                self.set_status(format!("Performance trace exported to {}", path.display()))
            }
            Err(error) => self.set_status(error),
        }
    }

    pub fn clear_perf_trace(&mut self) {
        self.perf_trace.clear();
        self.set_status("Performance trace cleared");
    }

    pub fn catalog_soundfonts(&self) -> &[SoundFontCatalogEntry] {
        &self.catalog_soundfonts
    }

    pub fn loaded_preset_font_order(&self) -> String {
        self.soundfonts
            .iter()
            .map(|font| font.display_name.clone())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    pub fn preset_set(&self) -> &'static PresetSet {
        self.preset_set
    }

    pub fn active_preset(&self) -> &'static Preset {
        self.preset_set
            .find_preset(&self.active_preset_id)
            .unwrap_or_else(|| self.preset_set.default_preset())
    }

    pub fn active_preset_label(&self) -> String {
        self.preset_set
            .find_preset(&self.active_preset_id)
            .map(|preset| preset.display_name.to_owned())
            .unwrap_or_else(|| "Custom Mode".to_owned())
    }

    pub fn selected_preset_id(&self) -> &str {
        &self.selected_preset_id
    }

    pub fn set_selected_preset_id(&mut self, preset_id: impl Into<String>) {
        let preset_id = preset_id.into();
        if self.preset_set.find_preset(&preset_id).is_some() {
            self.selected_preset_id = preset_id;
        }
    }

    pub fn apply_selected_preset(&mut self) {
        let trace = self.begin_user_trace("preset.apply");
        if self.selected_preset_id == self.active_preset_id {
            self.set_status("Preset already active");
            self.finish_user_trace(trace, "preset.apply", "already_active");
            return;
        }

        let Some(preset) = self.preset_set.find_preset(&self.selected_preset_id) else {
            let fallback = self.preset_set.default_preset();
            self.selected_preset_id = fallback.id.to_owned();
            self.set_status(format!(
                "Preset unavailable; selected fallback preset {}",
                fallback.display_name
            ));
            self.finish_user_trace(trace, "preset.apply", "fallback_selected");
            return;
        };

        self.apply_preset(preset, true);
        self.finish_user_trace(trace, "preset.apply", "completed");
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub(crate) fn preferences_status_active(&self) -> bool {
        self.preferences_status_active
    }

    pub fn missing_preset_warning(&self) -> Option<&str> {
        self.missing_preset_warning.as_deref()
    }

    pub fn dismiss_missing_preset_warning(&mut self) {
        self.missing_preset_warning = None;
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.preferences_status_active = false;
    }

    pub(super) fn set_preferences_status(&mut self, status: &str) {
        self.status = status.to_owned();
        self.preferences_status_active = true;
    }

    pub fn mark_project_dirty(&mut self) {
        self.mark_dirty();
    }

    pub(super) fn begin_user_trace(&mut self, label: &str) -> u64 {
        if !self.perf_trace.is_enabled() {
            return 0;
        }
        let metrics = self.debug_metrics();
        self.perf_trace.begin_user_event(label, &metrics)
    }

    pub(super) fn finish_user_trace(&mut self, operation_id: u64, label: &str, outcome: &str) {
        if !self.perf_trace.is_enabled() || operation_id == 0 {
            return;
        }
        let metrics = self.debug_metrics();
        self.perf_trace
            .finish_user_event(operation_id, label, outcome, &metrics);
    }

    pub(super) fn record_user_trace_point(&mut self, label: &str, outcome: &str) {
        if !self.perf_trace.is_enabled() {
            return;
        }
        let metrics = self.debug_metrics();
        self.perf_trace.record_user_event(label, outcome, &metrics);
    }

    pub(super) fn record_diagnostic_trace(
        &mut self,
        label: &str,
        outcome: &str,
        details: &[(&str, String)],
    ) {
        self.perf_trace.record_diagnostic(label, outcome, details);
    }

    pub(super) fn log_midi_mapping_scan_start(&mut self, source: &str, path: &str) {
        self.record_diagnostic_trace(
            "midi.mapping.scan",
            "start",
            &[("source", source.to_owned()), ("path", path.to_owned())],
        );
    }

    pub(super) fn log_midi_mapping_scan_result(&mut self, source: &str) {
        let strips = self
            .playback
            .midi_strip_layout()
            .into_iter()
            .map(|(channel, bank, program, is_percussion)| {
                format!("ch={channel} bank={bank} prog={program} perc={is_percussion}")
            })
            .collect::<Vec<_>>();
        let loaded_soundfonts = self
            .soundfonts
            .iter()
            .map(|font| font.internal_id.clone())
            .collect::<Vec<_>>();
        let per_font_strip_counts = self
            .soundfonts
            .iter()
            .map(|font| format!("{}={}", font.internal_id, font.strips.len()))
            .collect::<Vec<_>>();

        self.record_diagnostic_trace(
            "midi.mapping.scan",
            "result",
            &[
                ("source", source.to_owned()),
                ("soundfont_count", loaded_soundfonts.len().to_string()),
                ("soundfonts", loaded_soundfonts.join(",")),
                ("strip_count", strips.len().to_string()),
                ("strips", strips.join(";")),
                ("per_font_strip_counts", per_font_strip_counts.join(",")),
            ],
        );
    }

    pub(super) fn apply_midi_transport_metadata(&mut self, metadata: MidiTransportMetadata) {
        if let Some(start_tick) = metadata.jump_start_tick {
            self.loop_start_tick = start_tick;
        }
        if let Some(end_tick) = metadata.jump_end_tick {
            self.loop_end_tick = end_tick;
        } else if metadata.tick_length > 0 {
            self.loop_end_tick = metadata.tick_length;
        }

        if self.loop_start_tick > 0 || self.loop_end_tick > 0 {
            self.loop_mode = LoopMode::Infinite;
            self.loop_enabled = true;
        }
        self.normalize_loop_state(false);

        if !metadata.jump_points.is_empty() {
            self.status = format!(
                "{} jump point(s) parsed: {}",
                metadata.jump_points.len(),
                metadata
                    .jump_points
                    .iter()
                    .map(|tick| tick.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    pub(super) fn midi_has_non_default_loop_config(metadata: &MidiTransportMetadata) -> bool {
        metadata.jump_start_tick.is_some()
            || metadata.jump_end_tick.is_some()
            || !metadata.jump_points.is_empty()
    }

    pub(super) fn fmid_has_non_default_loop_config(fmid: &FmidFile) -> bool {
        fmid.looping.enabled
            || !matches!(fmid.looping.mode, FmidLoopMode::None)
            || fmid.looping.start_tick > 0
            || fmid.looping.end_tick > 0
            || fmid.looping.loop_count > 1
    }

    pub(super) fn should_enable_loop_by_default(&self, has_non_default_loop_config: bool) -> bool {
        if has_non_default_loop_config {
            return true;
        }
        !self.playlist_has_multiple_entries()
    }

    pub(super) fn refresh_realtime_feedback(&mut self, elapsed_seconds: f32) {
        let meter_decay = crate::repaint_policy::meter_decay_factor(elapsed_seconds);
        for font in &mut self.soundfonts {
            for strip in &mut font.strips {
                strip.meter *= meter_decay;
                strip.active = false;
                set_text_in_place(&mut strip.current_note, "--");
            }
        }

        self.transport_position = self.playback.transport_fraction();

        let mut snapshot = std::mem::take(&mut self.realtime_snapshot_buffer);
        self.playback.copy_audible_snapshot_into(&mut snapshot);
        self.apply_realtime_snapshot(&snapshot);
        self.realtime_snapshot_buffer = snapshot;
        self.perf_trace.observe_frame();
        if self.perf_trace.is_periodic_sample_due() || self.latency_trace.is_periodic_sample_due() {
            let metrics = self.debug_metrics();
            if self.perf_trace.is_enabled() {
                self.perf_trace.sample_frame(&metrics);
            }
            if self.latency_trace.is_enabled() {
                self.latency_trace.sample_frame(&metrics);
            }
        }
    }
}
