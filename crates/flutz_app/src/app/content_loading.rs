use super::mixer_state::soundfont_rows_from_catalog;
use super::persistence::{default_fmid_file_name, default_wrapped_audio_file_name};
use super::*;
mod jobs;
pub(super) use jobs::{
    prepare_decoded_audio_load_job, DecodedAudioLoadJobResult, LoadedContentState,
    PendingDecodedAudioLoad,
};

impl FlutzDesktopApp {
    pub fn open_midi_dialog(&mut self) {
        let trace = self.begin_user_trace("file.open_dialog");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Song files", &Self::open_dialog_extensions())
            .pick_file()
        else {
            self.set_status("Open canceled");
            self.finish_user_trace(trace, "file.open_dialog", "canceled");
            return;
        };

        if !self.prepare_playlist_for_open_path(&path) {
            self.finish_user_trace(trace, "file.open_dialog", "canceled");
            return;
        }

        let path_display = path.display().to_string();
        self.finish_user_trace(trace, "file.open_dialog", "selected");
        self.open_project_path(path.clone());

        if self
            .current_path
            .as_deref()
            .is_some_and(|current| current == path_display)
            && Self::is_supported_playlist_entry_path(&path)
        {
            self.regenerate_playlist_with_single_entry(path, true);
        }
    }

    pub fn open_project_path(&mut self, path: PathBuf) {
        self.open_project_path_with_resume(path, false);
    }

    pub(super) fn open_project_path_with_resume(&mut self, path: PathBuf, resume_after_load: bool) {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());
        match extension.as_deref() {
            Some("fplist") => self.load_fplist_path(path),
            Some(ext) => {
                let registry = builtin_registry();
                match registry.find_by_extension(ext).copied() {
                    Some(descriptor) if descriptor.content_kind == ContentKind::Fmid => {
                        self.load_fmid_path(path);
                        if resume_after_load && self.has_loaded_project() {
                            self.play();
                        }
                    }
                    Some(descriptor) if descriptor.content_kind == ContentKind::Midi => {
                        self.load_midi_path(path);
                        if resume_after_load && self.has_loaded_project() {
                            self.play();
                        }
                    }
                    Some(descriptor)
                        if descriptor.mastering == MasteringCapability::DecodedAudioPeq =>
                    {
                        self.load_decoded_audio_path_with_resume(
                            path,
                            descriptor,
                            resume_after_load,
                            false,
                        )
                    }
                    _ => self.set_status("Unsupported file type"),
                }
            }
            None => {
                self.load_midi_path(path);
                if resume_after_load && self.has_loaded_project() {
                    self.play();
                }
            }
        }
    }

    pub fn save_project(&mut self) {
        let trace = self.begin_user_trace("file.save");
        let path = self
            .current_path
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| self.can_save_project_to_existing_path(path));

        if let Some(path) = path {
            self.save_project_to_path(path);
        } else {
            self.save_project_as();
        }
        self.finish_user_trace(trace, "file.save", "completed");
    }

    pub fn save_project_as(&mut self) {
        let trace = self.begin_user_trace("file.save_as");
        let (filter_name, extensions, default_name) = self.save_as_dialog_spec();
        let Some(path) = rfd::FileDialog::new()
            .add_filter(&filter_name, &extensions)
            .set_file_name(&default_name)
            .save_file()
        else {
            self.set_status("Save As canceled");
            self.finish_user_trace(trace, "file.save_as", "canceled");
            return;
        };
        self.save_project_to_path(path);
        self.finish_user_trace(trace, "file.save_as", "completed");
    }

    pub fn load_midi_path(&mut self, path: PathBuf) {
        let trace = self.begin_user_trace("file.load_midi");
        let path_display = path.display().to_string();
        self.log_midi_mapping_scan_start("file.load_midi", &path_display);
        let preset = self
            .preset_set
            .find_preset(&self.default_preset_id)
            .unwrap_or_else(|| self.preset_set.default_preset());
        let selected_soundfonts = preset
            .font_ids
            .iter()
            .map(|font_id| (*font_id).to_owned())
            .collect::<Vec<_>>();
        match self.playback.load_midi_file(&path, &selected_soundfonts) {
            Ok(message) => {
                self.project_title = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| "Loaded MIDI".to_owned());
                self.loaded_content = Some(LoadedContentState {
                    kind: ContentKind::Midi,
                    format_id: "midi".to_owned(),
                    friendly_name: "MIDI sequence".to_owned(),
                    mastering: MasteringCapability::MidiMastering,
                    wrapped_extension: Some("fmid".to_owned()),
                    decoded_wrapper: None,
                    decoded_source_path: None,
                    decoded_source_bytes: None,
                });
                self.current_path = Some(path.display().to_string());
                self.transport_position = 0.0;
                self.run_state = AppRunState::Idle;
                self.active_preset_id = preset.id.to_owned();
                self.selected_preset_id = preset.id.to_owned();
                self.soundfonts =
                    soundfont_rows_from_catalog(&self.catalog_soundfonts, preset.font_ids);
                self.apply_metadata_for_plain_midi(&path);
                self.reset_plain_midi_defaults();
                let transport_metadata = self.playback.midi_transport_metadata();
                let has_non_default_loop =
                    Self::midi_has_non_default_loop_config(&transport_metadata);
                self.apply_midi_transport_metadata(transport_metadata);
                self.apply_loop_defaults_after_load(has_non_default_loop);
                self.sync_coverage_cache_from_playback();
                self.reset_strip_layout();
                self.sync_playback_controls();
                self.sync_playback_loop_settings();
                self.log_midi_mapping_scan_result("file.load_midi");
                self.peq_edit_state = PeqEditState::default();
                self.set_status(message);
                self.finish_user_trace(trace, "file.load_midi", "ok");
            }
            Err(error) => {
                self.set_status(format!("{error}"));
                self.finish_user_trace(trace, "file.load_midi", "error");
            }
        }
    }

    pub fn load_fmid_path(&mut self, path: PathBuf) {
        let trace = self.begin_user_trace("file.load_fmid");
        let path_display = path.display().to_string();
        self.log_midi_mapping_scan_start("file.load_fmid", &path_display);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.set_status(format!("failed to read {}: {error}", path.display()));
                self.finish_user_trace(trace, "file.load_fmid", "read_error");
                return;
            }
        };

        let fmid = match read_fmid(&bytes) {
            Ok(file) => file,
            Err(error) => {
                self.set_status(format!("{error}"));
                self.finish_user_trace(trace, "file.load_fmid", "parse_error");
                return;
            }
        };

        let (requested_soundfonts, preset_mode_message) = match &fmid.mixer_source_mode {
            MixerSourceMode::Custom => (
                fmid.soundfonts
                    .iter()
                    .map(|slot| slot.internal_id.clone())
                    .collect::<Vec<_>>(),
                None,
            ),
            MixerSourceMode::PresetDefault(preset_id) => {
                let preset = self
                    .preset_set
                    .find_preset(preset_id)
                    .unwrap_or_else(|| self.preset_set.default_preset());
                let message = if preset.id == preset_id {
                    None
                } else {
                    Some(format!(
                        "Preset {preset_id} was unavailable; loaded default preset {}",
                        preset.display_name
                    ))
                };
                self.missing_preset_warning = message.clone();
                (
                    preset
                        .font_ids
                        .iter()
                        .map(|font_id| (*font_id).to_owned())
                        .collect::<Vec<_>>(),
                    message,
                )
            }
        };
        self.soundfonts = self.soundfont_rows_for_ids(&requested_soundfonts);

        match self.playback.load_midi_bytes(
            fmid.midi_bytes.clone(),
            fmid.project.source_midi_filename.clone(),
            &requested_soundfonts,
        ) {
            Ok(message) => {
                self.project_title = if fmid.project.project_name.is_empty() {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| "Loaded FMID".to_owned())
                } else {
                    fmid.project.project_name.clone()
                };
                self.loaded_content = Some(LoadedContentState {
                    kind: ContentKind::Fmid,
                    format_id: "fmid".to_owned(),
                    friendly_name: "Flutz MIDI project".to_owned(),
                    mastering: MasteringCapability::MidiMastering,
                    wrapped_extension: Some("fmid".to_owned()),
                    decoded_wrapper: None,
                    decoded_source_path: None,
                    decoded_source_bytes: None,
                });
                self.current_path = Some(path.display().to_string());
                self.transport_position = 0.0;
                self.run_state = AppRunState::Idle;

                self.apply_fmid_global_settings(&fmid);
                self.apply_metadata_for_fmid(&fmid, &path);
                let has_non_default_loop = Self::fmid_has_non_default_loop_config(&fmid);
                self.apply_loop_defaults_after_load(has_non_default_loop);
                self.sync_coverage_cache_from_playback();
                self.reset_strip_layout();
                self.apply_fmid_mixer_source_mode(&fmid);
                self.sync_playback_controls();
                self.sync_playback_loop_settings();

                let loaded_soundfonts = self.playback.loaded_soundfont_ids();
                if !loaded_soundfonts.is_empty() {
                    self.soundfonts = self.soundfont_rows_for_ids(&loaded_soundfonts);
                    self.sync_coverage_cache_from_playback();
                    self.reset_strip_layout();
                    self.apply_fmid_mixer_source_mode(&fmid);
                    self.sync_playback_controls();
                    self.sync_playback_loop_settings();
                }

                self.dirty = false;
                self.log_midi_mapping_scan_result("file.load_fmid");
                self.peq_edit_state = PeqEditState::default();
                self.set_status(
                    preset_mode_message.unwrap_or_else(|| format!("Loaded FMID: {message}")),
                );
                self.finish_user_trace(trace, "file.load_fmid", "ok");
            }
            Err(error) => {
                self.set_status(format!("{error}"));
                self.finish_user_trace(trace, "file.load_fmid", "error");
            }
        }
    }

    pub fn load_decoded_audio_path(&mut self, path: PathBuf, descriptor: FormatDescriptor) {
        self.load_decoded_audio_path_with_resume(path, descriptor, false, false);
    }

    pub(super) fn load_decoded_audio_path_with_resume(
        &mut self,
        path: PathBuf,
        descriptor: FormatDescriptor,
        resume_after_load: bool,
        playlist_entry_load: bool,
    ) {
        let trace = self.begin_user_trace("file.load_decoded_audio");
        let token = DECODED_AUDIO_LOAD_TOKEN.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = mpsc::channel();
        self.pending_decoded_audio_load = Some(PendingDecodedAudioLoad {
            token,
            receiver,
            resume_after_load,
            playlist_entry_load,
        });
        let path_for_status = path.display().to_string();
        let spawn_result = thread::Builder::new()
            .name("flutz-decoded-load".to_owned())
            .spawn(move || {
                let result = prepare_decoded_audio_load_job(token, path, descriptor);
                let _ = sender.send(result);
            });
        if let Err(error) = spawn_result {
            self.pending_decoded_audio_load = None;
            self.set_status(format!("failed to start decoded audio load: {error}"));
            self.finish_user_trace(trace, "file.load_decoded_audio", "spawn_error");
            return;
        }
        self.set_status(format!("Loading decoded audio {}", path_for_status));
        self.finish_user_trace(trace, "file.load_decoded_audio", "scheduled");
    }

    pub(super) fn process_pending_decoded_audio_load(&mut self) {
        let Some(pending) = &self.pending_decoded_audio_load else {
            return;
        };
        let token = pending.token;
        let resume_after_load = pending.resume_after_load;
        let playlist_entry_load = pending.playlist_entry_load;
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_decoded_audio_load = None;
                self.set_status("decoded audio load worker exited before reporting a result");
                return;
            }
        };
        self.pending_decoded_audio_load = None;
        if result.token != token || result.token != DECODED_AUDIO_LOAD_TOKEN.load(Ordering::Relaxed)
        {
            return;
        }
        let _ = self.commit_decoded_audio_load(result, resume_after_load, playlist_entry_load);
    }

    pub(super) fn commit_decoded_audio_load(
        &mut self,
        result: DecodedAudioLoadJobResult,
        resume_after_load: bool,
        playlist_entry_load: bool,
    ) -> bool {
        let DecodedAudioLoadJobResult {
            path,
            path_display,
            descriptor,
            is_wrapped,
            wrapper,
            stream_source,
            source_bytes,
            error,
            error_stage,
            ..
        } = result;
        if let Some(error) = error {
            self.set_status(format!("decoded audio {error_stage} failed: {error}"));
            return false;
        }
        let Some(wrapper) = wrapper else {
            self.set_status("decoded audio load did not produce wrapper metadata");
            return false;
        };
        let Some(stream_source) = stream_source else {
            self.set_status("decoded audio load did not produce a stream source");
            return false;
        };

        match self.playback.load_decoded_audio_stream(
            path.clone(),
            descriptor.id,
            descriptor.friendly_name,
            stream_source,
            wrapper.peq.clone(),
        ) {
            Ok(message) => {
                self.loaded_content = Some(LoadedContentState {
                    kind: if is_wrapped {
                        ContentKind::DecodedAudioWrapper
                    } else {
                        ContentKind::DecodedAudio
                    },
                    format_id: descriptor.id.to_owned(),
                    friendly_name: descriptor.friendly_name.to_owned(),
                    mastering: descriptor.mastering,
                    wrapped_extension: descriptor
                        .wrapped_extensions
                        .first()
                        .map(|extension| (*extension).to_owned()),
                    decoded_wrapper: Some(wrapper.clone()),
                    decoded_source_path: (!is_wrapped).then(|| path.clone()),
                    decoded_source_bytes: source_bytes,
                });
                self.project_title = decoded_project_title(
                    &wrapper.metadata,
                    &wrapper.native_metadata,
                    path.file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Loaded Audio"),
                );
                self.current_path = Some(path_display);
                self.transport_position = 0.0;
                self.run_state = AppRunState::Idle;
                self.apply_metadata_for_decoded_audio(&wrapper, &path);
                self.apply_decoded_audio_defaults(wrapper.loop_region);
                self.sync_playback_output_volume();
                self.reset_decoded_peq_edit_state(wrapper.peq.clone());
                self.sync_playback_loop_settings();
                if playlist_entry_load {
                    self.sync_loop_button_from_playlist_repeat_mode(false);
                }
                self.reset_strip_layout();
                self.sync_playback_controls();
                self.dirty = !is_wrapped;
                self.set_status(message);
                if resume_after_load {
                    self.play();
                }
                true
            }
            Err(error) => {
                self.set_status(format!("{error}"));
                false
            }
        }
    }

    pub(super) fn apply_fmid_global_settings(&mut self, fmid: &FmidFile) {
        self.loop_enabled = fmid.looping.enabled;
        self.loop_mode = match fmid.looping.mode {
            FmidLoopMode::None => LoopMode::None,
            FmidLoopMode::Infinite => LoopMode::Infinite,
            FmidLoopMode::Counted => LoopMode::Counted,
        };
        self.loop_start_tick = fmid.looping.start_tick;
        self.loop_end_tick = fmid.looping.end_tick;
        self.loop_count = fmid.looping.loop_count as u32;
        self.normalize_loop_state(false);

        self.master = MasterControls {
            volume_db: fmid.mixer.master.volume_db as f32,
            limiter_enabled: fmid.mixer.master.limiter_enabled,
            limiter_amount: fmid.mixer.master.limiter_amount as f32,
            reverb: fmid.mixer.master.reverb as f32,
            chorus: fmid.mixer.master.chorus as f32,
            eq_low: fmid.mixer.master.eq_low_db as f32,
            eq_mid: fmid.mixer.master.eq_mid_db as f32,
            eq_high: fmid.mixer.master.eq_high_db as f32,
        };

        self.smart_mix = SmartMixControls {
            enabled: fmid.smart_mix.enabled,
            auto_normalize: fmid.smart_mix.auto_normalization_enabled,
            target_headroom_db: fmid.smart_mix.target_headroom as f32,
            attack_ms: fmid.smart_mix.attack as f32,
            release_ms: fmid.smart_mix.release as f32,
            lookahead_ms: fmid.smart_mix.lookahead as f32,
            normalization_amount: fmid.smart_mix.auto_normalization_amount as f32,
        };
    }

    pub(super) fn apply_metadata_for_plain_midi(&mut self, path: &Path) {
        let project_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Loaded MIDI")
            .to_owned();
        let source_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("embedded.mid")
            .to_owned();
        self.metadata_edit_state = MetadataEditState {
            project_name: project_name.clone(),
            source_filename: source_name,
            ..MetadataEditState::default()
        };
        self.project_title = project_name;
        self.metadata_edit_dirty = false;
    }

    pub(super) fn apply_metadata_for_fmid(&mut self, fmid: &FmidFile, path: &Path) {
        self.metadata_edit_state = MetadataEditState::from_fmid_project(
            &fmid.project,
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Loaded FMID")
                .to_owned(),
            "embedded.mid".to_owned(),
        );
        self.project_title = self.metadata_edit_state.project_name.clone();
        self.metadata_edit_dirty = false;
    }

    pub(super) fn apply_metadata_for_decoded_audio(
        &mut self,
        wrapper: &FlutzAudioWrapper,
        path: &Path,
    ) {
        let project_name = decoded_project_title(
            &wrapper.metadata,
            &wrapper.native_metadata,
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Loaded Audio"),
        );
        let source_filename = if wrapper.metadata.source_filename.trim().is_empty() {
            wrapper.source.original_filename.clone()
        } else {
            wrapper.metadata.source_filename.clone()
        };
        self.metadata_edit_state = MetadataEditState::from_track_metadata(
            &wrapper.metadata,
            project_name.clone(),
            source_filename,
        )
        .with_native_metadata(wrapper.native_metadata.clone());
        self.project_title = project_name;
        self.metadata_edit_dirty = false;
    }

    pub(super) fn reset_plain_midi_defaults(&mut self) {
        self.dirty = false;
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        self.loop_enabled = false;
        self.loop_mode = LoopMode::None;
        self.loop_start_tick = 0;
        self.loop_end_tick = 0;
        self.loop_count = 1;
        self.master = MasterControls::default();
        self.smart_mix = SmartMixControls::default();
        for font in &mut self.soundfonts {
            font.muted = false;
            font.soloed = false;
            for strip in &mut font.strips {
                strip.volume = 1.0;
                strip.muted = false;
                strip.unsupported = false;
                strip.mute_policy_default = false;
                strip.volume_policy_default = false;
                strip.soloed = false;
                strip.pan = 0.0;
                strip.gain_db = 0.0;
                strip.limiter_enabled = false;
                strip.limiter_amount = 0.25;
                strip.reverb = 0.0;
                strip.chorus = 0.0;
                strip.meter = 0.0;
                strip.active = false;
                strip.current_note = "--".to_owned();
            }
        }
        self.sync_playback_output_volume();
    }

    pub(super) fn open_dialog_extensions() -> Vec<String> {
        let mut extensions = Self::playlist_dialog_extensions();
        extensions.push("fplist".to_owned());
        extensions.sort();
        extensions.dedup();
        extensions
    }

    pub(super) fn playlist_dialog_extensions() -> Vec<String> {
        let mut extensions = Vec::new();
        for descriptor in builtin_registry().descriptors() {
            extensions.extend(
                descriptor
                    .extensions
                    .iter()
                    .map(|extension| (*extension).to_owned()),
            );
            extensions.extend(
                descriptor
                    .wrapped_extensions
                    .iter()
                    .map(|extension| (*extension).to_owned()),
            );
        }
        extensions.sort();
        extensions.dedup();
        extensions
    }

    pub(super) fn save_as_dialog_spec(&self) -> (String, Vec<String>, String) {
        if let Some(content) = self
            .loaded_content
            .as_ref()
            .filter(|content| content.mastering == MasteringCapability::DecodedAudioPeq)
        {
            let extension = content
                .wrapped_extension
                .as_deref()
                .unwrap_or("faudio")
                .trim_start_matches('.');
            let default_name = default_wrapped_audio_file_name(&self.project_title, extension);
            return (
                format!("Flutz {} wrapper", content.friendly_name),
                vec![extension.to_owned()],
                default_name,
            );
        }

        (
            "FMID files".to_owned(),
            vec!["fmid".to_owned()],
            default_fmid_file_name(&self.project_title),
        )
    }
}
