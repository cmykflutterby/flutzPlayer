use super::mixer_state::soundfont_rows_from_catalog;
use super::*;

impl FlutzDesktopApp {
    pub(super) fn new(
        creation_context: &eframe::CreationContext<'_>,
        config: AppStartupConfig,
    ) -> Self {
        crate::ui::apply_theme(&creation_context.egui_ctx);
        let catalog_soundfonts = match &config.dat_summary {
            DatStartupSummary::Available { soundfonts, .. } => soundfonts.clone(),
            DatStartupSummary::Unavailable(_) => Vec::new(),
        };
        let data_summary = match &config.dat_summary {
            DatStartupSummary::Available {
                dat_file_count,
                dat_byte_count,
                soundfonts,
            } => format!(
                "{} DAT file(s), {} byte(s), {} soundfont(s)",
                dat_file_count,
                dat_byte_count,
                soundfonts.len()
            ),
            DatStartupSummary::Unavailable(message) => message.clone(),
        };
        let preset_set = default_preset_set();
        let loaded_preferences =
            Self::load_or_create_preferences_from_data_dir(&config.data_dir, preset_set);
        if let Some(warning) = &loaded_preferences.warning {
            eprintln!("{}", warning.detail);
        }
        let startup_preferences_warning = loaded_preferences.warning.clone();
        let preferences = loaded_preferences.preferences;
        let default_decoded_peq_preset = preferences.decoded_audio_peq.default_preset.clone();
        let default_preset = preset_set
            .find_preset(&preferences.default_preset_id)
            .unwrap_or_else(|| preset_set.default_preset());
        let soundfonts = soundfont_rows_from_catalog(&catalog_soundfonts, default_preset.font_ids);
        let status = match &config.dat_summary {
            DatStartupSummary::Available { soundfonts, .. } if soundfonts.is_empty() => {
                "No bundled soundfonts found in data folder".to_owned()
            }
            DatStartupSummary::Available { .. } => "Ready".to_owned(),
            DatStartupSummary::Unavailable(message) => message.clone(),
        };
        let mut app = Self {
            run_state: AppRunState::Idle,
            release_edit_mode: false,
            release_editor_panels_collapsed: false,
            speaker_volume_popup_open: false,
            final_output_volume_percent: 0.0,
            release_loop_popup_open: false,
            loaded_content: None,
            project_title: "Untitled MIDI".to_owned(),
            current_path: None,
            dirty: false,
            transport_position: 0.0,
            loop_enabled: false,
            loop_mode: LoopMode::None,
            loop_start_tick: 0,
            loop_end_tick: 0,
            loop_count: 1,
            master: MasterControls::default(),
            smart_mix: SmartMixControls::default(),
            mixer_fx_expanded: false,
            soundfonts,
            catalog_soundfonts: catalog_soundfonts.clone(),
            preset_set,
            active_preset_id: default_preset.id.to_owned(),
            selected_preset_id: default_preset.id.to_owned(),
            default_preset_id: default_preset.id.to_owned(),
            default_decoded_peq_preset,
            decoded_peq_bypass_enabled: preferences.decoded_audio_peq.bypass,
            coverage_cache: BTreeMap::new(),
            selected_soundfont: 0,
            mixer_assignment_mode: MixerAssignmentMode::Manual,
            data_dir: config.data_dir.clone(),
            data_summary,
            playback: PlaybackController::new(
                config.data_dir,
                catalog_soundfonts,
                config.audio_backend,
                config.debug_analyzer,
                config.debug_render_errors,
            ),
            media_controls: create_media_controls_backend_for_creation_context(creation_context),
            last_media_controls_snapshot: None,
            last_media_controls_timeline_publish: None,
            perf_trace: PerfTrace::new(config.debug_memory),
            latency_trace: LatencyTrace::new(config.debug_latency),
            logged_unmatched_snapshot_strips: BTreeSet::new(),
            _memory_maintenance: crate::memory_runtime::MemoryMaintenanceHandle::spawn(
                crate::memory_runtime::MEMORY_MAINTENANCE_INTERVAL,
            )
            .ok(),
            realtime_snapshot_buffer: RealtimeMixerSnapshot::default(),
            mixer_edit_buffer: MixerEditSnapshot::default(),
            published_mixer_controls: MixerControlState::default(),
            status,
            missing_preset_warning: None,
            preferences_status_active: false,
            last_preferences_warning: loaded_preferences.warning,
            preferences_save_blocked: loaded_preferences.save_blocked,
            playlist: None,
            window_view_prefs: WindowViewPrefs {
                playlist_open: preferences.window.playlist_open,
                metadata_open: preferences.window.metadata_open,
                playlist_position: preferences.window.playlist_position,
                playlist_size: preferences.window.playlist_size,
                metadata_position: preferences.window.metadata_position,
                metadata_size: preferences.window.metadata_size,
            },
            playlist_window_open: preferences.window.playlist_open,
            playlist_file_picker_pending: false,
            playlist_selected_indices: BTreeSet::new(),
            playlist_drag_from: None,
            metadata_popup_open: preferences.window.metadata_open,
            metadata_edit_dirty: false,
            metadata_edit_state: MetadataEditState::default(),
            peq_edit_state: PeqEditState::default(),
            pending_decoded_audio_load: None,
            pending_playlist_preload: None,
            ready_playlist_preload: None,
            last_playback_active: false,
        };

        let repaint_context = creation_context.egui_ctx.clone();
        let ui_waker: UiWaker = Arc::new(move || repaint_context.request_repaint());
        app.playback.set_ui_waker(Arc::clone(&ui_waker));
        crate::memory_runtime::set_idle_release_waker(Arc::clone(&ui_waker));
        app.media_controls.set_command_waker(ui_waker);

        if let Some(path) = config.launch_open_path {
            let startup_path = path.clone();
            let startup_extension = startup_path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_ascii_lowercase());
            app.open_project_path_with_resume(path, true);
            if app.has_loaded_project() && app.run_state != AppRunState::Playing {
                if startup_extension.as_deref() != Some("fplist") {
                    app.regenerate_playlist_with_single_entry(startup_path, true);
                }
            }
        }

        if let Some(warning) = app
            .last_preferences_warning
            .as_ref()
            .or(startup_preferences_warning.as_ref())
        {
            app.set_preferences_status(warning.status);
        }

        app
    }

    pub(super) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(super) fn schedule_next_repaint(&self, context: &egui::Context) {
        let minimized = context.input(|input| input.viewport().minimized.unwrap_or(false));
        let inputs = RepaintInputs {
            playing: self.run_state == AppRunState::Playing,
            seconds_until_track_end: self.seconds_until_track_end(),
            background_work_pending: self.pending_decoded_audio_load.is_some()
                || self.pending_playlist_preload.is_some()
                || self.playlist_file_picker_pending,
            minimized,
            frame_tracing_enabled: self.perf_trace.is_enabled() || self.latency_trace.is_enabled(),
        };
        context.request_repaint_after(repaint_interval(inputs));
    }

    /// Finishes a deep idle release requested by the memory maintenance thread: drops the
    /// playback-held buffers and audio stream while keeping loaded content and position.
    fn service_idle_memory_release(&mut self) {
        if !crate::memory_runtime::take_ui_idle_release_request() {
            return;
        }
        if let Err(error) = self.playback.release_idle_memory() {
            self.set_status(format!("{error}"));
        }
    }

    /// Uses the per-frame cached transport fraction to avoid `debug_metrics()`.
    fn seconds_until_track_end(&self) -> Option<f64> {
        if self.loop_enabled {
            return None;
        }
        let duration = self.media_controls_duration_seconds();
        (duration > 0.0).then(|| duration * (1.0 - f64::from(self.transport_position)))
    }
}

impl eframe::App for FlutzDesktopApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::AppUpdate);
        self.process_playlist_file_picker_pending();
        self.process_pending_decoded_audio_load();
        self.process_pending_playlist_preload();
        let frame_seconds = context.input(|input| input.unstable_dt);
        self.refresh_realtime_feedback(frame_seconds);
        self.maybe_start_playlist_preload();
        self.process_media_control_commands();
        let mut mixer_snapshot = std::mem::take(&mut self.mixer_edit_buffer);
        self.fill_mixer_edit_snapshot(&mut mixer_snapshot);
        {
            let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::Ui);
            crate::ui::draw_app(context, self);
        }
        self.apply_mixer_edit_snapshot(&mixer_snapshot);
        self.mixer_edit_buffer = mixer_snapshot;
        self.sync_playback_controls();
        self.handle_playback_end_transition();
        self.service_idle_memory_release();
        self.publish_media_controls_snapshot();
        self.schedule_next_repaint(context);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

impl Drop for FlutzDesktopApp {
    fn drop(&mut self) {
        self.publish_media_controls_shutdown_snapshot();
        self.media_controls.shutdown();
    }
}

pub(super) fn load_app_icon() -> egui::IconData {
    let png_bytes = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/flutzplayer-icon.png"
    ));
    let image = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
        .expect("flutzPlayer icon PNG must decode")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}
