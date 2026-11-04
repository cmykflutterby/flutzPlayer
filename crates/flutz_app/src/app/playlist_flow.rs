use super::content_loading::prepare_decoded_audio_load_job;
use super::*;

pub(super) struct PendingPlaylistPreload {
    pub(super) token: u64,
    pub(super) index: usize,
    pub(super) path: PathBuf,
    pub(super) receiver: mpsc::Receiver<DecodedAudioLoadJobResult>,
}

pub(super) struct ReadyPlaylistPreload {
    pub(super) token: u64,
    pub(super) index: usize,
    pub(super) path: PathBuf,
    pub(super) result: DecodedAudioLoadJobResult,
}

impl FlutzDesktopApp {
    pub(super) fn regenerate_playlist_with_single_entry(
        &mut self,
        path: PathBuf,
        align_repeat_to_loop: bool,
    ) {
        if !Self::is_supported_playlist_entry_path(&path) {
            return;
        }

        let mut playlist = PlaylistState::default();
        playlist.shuffle_seed = 0xC0FFEE_u64;
        playlist.add_entry(path);

        if align_repeat_to_loop {
            let repeat_mode = if self.loop_enabled {
                PlaylistRepeatMode::Track
            } else {
                PlaylistRepeatMode::Off
            };
            playlist.repeat_mode = repeat_mode;
            playlist.loop_enabled = repeat_mode == PlaylistRepeatMode::Playlist;
        }

        playlist.dirty = false;
        self.playlist = Some(playlist);
        self.playlist_selected_indices.clear();
        self.playlist_drag_from = None;
    }

    pub(super) fn prepare_playlist_for_open_path(&mut self, path: &Path) -> bool {
        if !Self::is_supported_playlist_entry_path(path) {
            return true;
        }

        let (entry_count, playlist_dirty) = self
            .playlist
            .as_ref()
            .map(|playlist| (playlist.entries.len(), playlist.dirty))
            .unwrap_or((0, false));

        if entry_count > 1 && playlist_dirty {
            let choice = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Unsaved Playlist")
                .set_description(
                    "The current playlist has unsaved changes. Save before opening a new file?",
                )
                .set_buttons(rfd::MessageButtons::YesNoCancel)
                .show();

            match choice {
                rfd::MessageDialogResult::Yes => {
                    self.save_playlist();
                    if self
                        .playlist
                        .as_ref()
                        .is_some_and(|playlist| playlist.dirty)
                    {
                        self.set_status("Open canceled: playlist was not saved");
                        return false;
                    }
                }
                rfd::MessageDialogResult::No => {}
                _ => {
                    self.set_status("Open canceled");
                    return false;
                }
            }
        }

        self.regenerate_playlist_with_single_entry(path.to_path_buf(), false);
        true
    }

    pub(super) fn sync_loop_button_from_playlist_repeat_mode(&mut self, mark_project_dirty: bool) {
        let Some(repeat_mode) = self.playlist.as_ref().map(|playlist| playlist.repeat_mode) else {
            return;
        };

        let enabled = repeat_mode == PlaylistRepeatMode::Track;
        let previous = self.loop_state_tuple();
        self.loop_enabled = enabled;
        if enabled && matches!(self.loop_mode, LoopMode::None) {
            self.loop_mode = LoopMode::Infinite;
        }
        if enabled && self.loop_end_tick == 0 {
            self.loop_end_tick = self.transport_tick_length();
        }
        self.normalize_loop_state(false);
        if mark_project_dirty {
            let _ = self.commit_loop_state_change(previous);
        } else if let Err(error) = self
            .playback
            .set_loop_settings(self.current_loop_settings())
        {
            self.set_status(format!("{error}"));
        }
    }

    pub fn get_next_track_file_path(&mut self) -> Option<PathBuf> {
        let mut preview = self.playlist.clone()?;
        let next_index = preview.next_track_for_mode_by(|entry| {
            Self::is_supported_playlist_entry_path(&entry.file_path)
        })?;
        preview
            .entries
            .get(next_index)
            .map(|entry| entry.file_path.clone())
    }

    pub fn next_track(&mut self) {
        let previous_state = self.run_state;
        let autoplay = previous_state == AppRunState::Playing;
        let loaded = self.navigate_playlist(true, autoplay);
        if loaded && previous_state == AppRunState::Paused {
            self.run_state = AppRunState::Paused;
            self.set_status("Paused");
        }
    }

    pub fn prev_track(&mut self) {
        let previous_state = self.run_state;
        let autoplay = previous_state == AppRunState::Playing;
        let loaded = self.navigate_playlist(false, autoplay);
        if loaded && previous_state == AppRunState::Paused {
            self.run_state = AppRunState::Paused;
            self.set_status("Paused");
        }
    }

    pub(super) fn navigate_playlist(&mut self, forward: bool, autoplay: bool) -> bool {
        let Some(playlist) = self.playlist.as_mut() else {
            self.set_status("No playlist loaded");
            return false;
        };

        if playlist.is_empty() {
            self.set_status("Playlist is empty");
            return false;
        }

        let target_index = if forward {
            playlist.next_track_for_mode_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
        } else {
            playlist.prev_track_for_mode_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
        };

        let Some(index) = target_index else {
            self.log_playlist_skip_reason();
            return false;
        };

        self.load_playlist_track_by_index(index, true, autoplay)
    }

    pub(super) fn load_playlist_track_by_index(
        &mut self,
        index: usize,
        push_history: bool,
        autoplay: bool,
    ) -> bool {
        let Some(playlist) = self.playlist.as_mut() else {
            return false;
        };
        if index >= playlist.entries.len() {
            return false;
        }
        let path = playlist.entries[index].file_path.clone();
        playlist.set_current_with_history(index, push_history);
        let loaded = if let Some(preloaded) = self.take_ready_playlist_preload(index, &path) {
            self.commit_decoded_audio_load(preloaded, autoplay, true)
        } else {
            self.clear_playlist_preload();
            self.load_playlist_entry_path(path, autoplay)
        };
        if loaded {
            // Playlist repeat mode is authoritative for track looping when a track
            // is selected from a playlist (.fplist load, next/prev, double-click).
            self.sync_loop_button_from_playlist_repeat_mode(false);
        }
        loaded
    }

    pub(super) fn playlist_has_prev_track(&mut self) -> bool {
        let Some(mut preview) = self.playlist.clone() else {
            return false;
        };
        preview
            .prev_track_for_mode_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
            .is_some()
    }

    pub(super) fn playlist_has_next_track(&mut self) -> bool {
        let Some(mut preview) = self.playlist.clone() else {
            return false;
        };
        preview
            .next_track_for_mode_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
            .is_some()
    }

    pub(super) fn log_playlist_skip_reason(&mut self) {
        let Some(playlist) = self.playlist.as_mut() else {
            self.set_status("No valid playlist entries remain");
            return;
        };
        let mut first_missing = None;
        let mut first_unsupported = None;
        for entry in &mut playlist.entries {
            entry.refresh_status();
            if !entry.file_exists && first_missing.is_none() {
                first_missing = Some(entry.display_name.clone());
            } else if entry.file_exists
                && !Self::is_supported_playlist_entry_path(&entry.file_path)
                && first_unsupported.is_none()
            {
                first_unsupported = Some(entry.display_name.clone());
            }
        }

        if let Some(name) = first_missing {
            self.set_status(format!("Skipped missing file: {name}"));
        } else if let Some(name) = first_unsupported {
            self.set_status(format!("Skipped unsupported file: {name}"));
        } else {
            self.set_status("No valid playlist entries remain");
        }
    }

    pub(super) fn load_playlist_entry_path(&mut self, path: PathBuf, autoplay: bool) -> bool {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());

        match extension.as_deref() {
            Some(ext) => {
                let registry = builtin_registry();
                let Some(descriptor) = registry.find_by_extension(ext).copied() else {
                    self.set_status("Skipped unsupported file: unsupported playlist entry type");
                    return false;
                };
                match descriptor.content_kind {
                    ContentKind::Fmid => {
                        self.load_fmid_path(path);
                        if autoplay {
                            self.play();
                        }
                    }
                    ContentKind::Midi => {
                        self.load_midi_path(path);
                        if autoplay {
                            self.play();
                        }
                    }
                    _ if descriptor.mastering == MasteringCapability::DecodedAudioPeq => {
                        self.load_decoded_audio_path_with_resume(path, descriptor, autoplay, true);
                    }
                    _ => {
                        self.set_status(
                            "Skipped unsupported file: unsupported playlist entry type",
                        );
                        return false;
                    }
                }
                true
            }
            _ => {
                self.set_status("Skipped unsupported file: unsupported playlist entry type");
                false
            }
        }
    }

    pub(super) fn is_supported_playlist_entry_path(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| builtin_registry().find_by_extension(ext).is_some())
    }

    pub fn new_playlist(&mut self) {
        self.clear_playlist_preload();
        let mut playlist = PlaylistState::default();
        playlist.shuffle_seed = 0xC0FFEE_u64;
        self.playlist = Some(playlist);
        self.playlist_selected_indices.clear();
        self.playlist_drag_from = None;
        if !self.preferences_status_active() {
            self.set_status("New playlist");
        }
        self.set_playlist_window_open(true);
    }

    pub fn load_fplist_path(&mut self, path: PathBuf) {
        self.clear_playlist_preload();
        match playlist_persistence::load_playlist(&path) {
            Ok(mut playlist) => {
                if playlist.entries.is_empty() {
                    self.playlist = Some(playlist);
                    self.sync_loop_button_from_playlist_repeat_mode(false);
                    self.set_status("Loaded empty playlist");
                    self.set_playlist_window_open(true);
                    return;
                }

                let first_valid = playlist.first_valid_track_by(|entry| {
                    Self::is_supported_playlist_entry_path(&entry.file_path)
                });
                self.playlist = Some(playlist);
                self.sync_loop_button_from_playlist_repeat_mode(false);
                if let Some(index) = first_valid {
                    let loaded = self.load_playlist_track_by_index(index, false, true);
                    if loaded {
                        self.set_status(format!("Loaded playlist {}", path.display()));
                    } else {
                        self.set_status("Loaded playlist, but first valid track failed to load");
                    }
                } else {
                    self.set_status("Loaded playlist, but no valid tracks were found");
                }
                self.set_playlist_window_open(true);
            }
            Err(error) => self.set_status(format!("{error}")),
        }
    }

    pub fn save_playlist(&mut self) {
        let Some(playlist) = self.playlist.as_mut() else {
            self.set_status("No playlist loaded");
            return;
        };
        let Some(path) = playlist.file_path.clone() else {
            self.save_playlist_as();
            return;
        };

        if let Some(parent) = path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                self.set_status(format!("failed to create {}: {error}", parent.display()));
                return;
            }
        }

        match playlist_persistence::save_playlist(&path, playlist) {
            Ok(()) => {
                playlist.dirty = false;
                self.set_status(format!("Saved playlist {}", path.display()));
            }
            Err(error) => self.set_status(format!("{error}")),
        }
    }

    pub fn save_playlist_as(&mut self) {
        if self.playlist.is_none() {
            self.new_playlist();
        }

        let Some(path) = rfd::FileDialog::new()
            .add_filter("flutzPlayer Playlist", &["fplist"])
            .set_file_name("playlist.fplist")
            .save_file()
        else {
            self.set_status("Playlist Save As canceled");
            return;
        };

        if let Some(playlist) = self.playlist.as_mut() {
            playlist.file_path = Some(path);
        }
        self.save_playlist();
    }

    pub(super) fn remove_selected_playlist_entries(&mut self) {
        self.clear_playlist_preload();
        let Some(playlist) = self.playlist.as_mut() else {
            return;
        };
        if self.playlist_selected_indices.is_empty() {
            return;
        }

        let indices = self
            .playlist_selected_indices
            .iter()
            .copied()
            .collect::<Vec<_>>();
        playlist.remove_indices(&indices);
        self.playlist_selected_indices.clear();
    }

    pub(super) fn process_playlist_file_picker_pending(&mut self) {
        if !self.playlist_file_picker_pending {
            return;
        }
        self.playlist_file_picker_pending = false;

        let Some(paths) = rfd::FileDialog::new()
            .add_filter("Playlist Tracks", &Self::playlist_dialog_extensions())
            .pick_files()
        else {
            return;
        };

        if self.playlist.is_none() {
            self.new_playlist();
        }
        self.clear_playlist_preload();
        if let Some(playlist) = self.playlist.as_mut() {
            playlist.add_entries(paths);
        }
    }

    pub(super) fn handle_dropped_playlist_files(&mut self, context: &egui::Context) {
        let dropped_paths = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect::<Vec<_>>()
        });

        if dropped_paths.is_empty() {
            return;
        }

        let filtered = dropped_paths
            .into_iter()
            .filter(|path| Self::is_supported_playlist_entry_path(path))
            .collect::<Vec<_>>();
        if filtered.is_empty() {
            return;
        }

        if self.playlist.is_none() {
            self.new_playlist();
        }
        self.clear_playlist_preload();
        if let Some(playlist) = self.playlist.as_mut() {
            playlist.add_entries(filtered);
            self.set_playlist_window_open(true);
        }
    }

    pub(super) fn handle_playback_end_transition(&mut self) {
        let currently_active = self.playback.playback_active();
        if self.last_playback_active
            && !currently_active
            && self.run_state == AppRunState::Playing
            && !self.loop_enabled
        {
            if !self.on_track_end() {
                self.run_state = AppRunState::Idle;
                self.set_status("Stopped");
            }
        }
        self.last_playback_active = currently_active;
    }

    pub(super) fn process_pending_playlist_preload(&mut self) {
        let Some(pending) = &self.pending_playlist_preload else {
            return;
        };
        let token = pending.token;
        let index = pending.index;
        let path = pending.path.clone();
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_playlist_preload = None;
                return;
            }
        };
        self.pending_playlist_preload = None;
        if token != PLAYLIST_PRELOAD_TOKEN.load(Ordering::Relaxed) || result.error.is_some() {
            return;
        }
        self.ready_playlist_preload = Some(ReadyPlaylistPreload {
            token,
            index,
            path,
            result,
        });
    }

    pub(super) fn maybe_start_playlist_preload(&mut self) {
        if self.run_state != AppRunState::Playing || !self.playback.playback_active() {
            return;
        }
        if self.loop_enabled
            || self
                .playlist
                .as_ref()
                .is_some_and(|playlist| playlist.repeat_mode == PlaylistRepeatMode::Track)
        {
            self.clear_playlist_preload();
            return;
        }
        let duration = self.transport_duration_seconds();
        let position = self.transport_seconds();
        if duration <= 0.0 || duration - position > PLAYLIST_PRELOAD_LEAD_TIME_SECONDS {
            return;
        }
        let Some((index, path, descriptor)) = self.next_playlist_preload_candidate() else {
            self.clear_playlist_preload();
            return;
        };
        if self
            .ready_playlist_preload
            .as_ref()
            .is_some_and(|preload| preload.index == index && preload.path == path)
            || self
                .pending_playlist_preload
                .as_ref()
                .is_some_and(|preload| preload.index == index && preload.path == path)
        {
            return;
        }
        if descriptor.mastering != MasteringCapability::DecodedAudioPeq {
            self.clear_playlist_preload();
            return;
        }
        let token = PLAYLIST_PRELOAD_TOKEN.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = mpsc::channel();
        let path_for_worker = path.clone();
        let spawn_result = thread::Builder::new()
            .name("flutz-playlist-preload".to_owned())
            .spawn(move || {
                let result = prepare_decoded_audio_load_job(token, path_for_worker, descriptor);
                let _ = sender.send(result);
            });
        if spawn_result.is_ok() {
            self.pending_playlist_preload = Some(PendingPlaylistPreload {
                token,
                index,
                path,
                receiver,
            });
        }
    }

    pub(super) fn next_playlist_preload_candidate(
        &self,
    ) -> Option<(usize, PathBuf, FormatDescriptor)> {
        let mut preview = self.playlist.clone()?;
        let index = preview.next_track_for_mode_by(|entry| {
            Self::is_supported_playlist_entry_path(&entry.file_path)
        })?;
        let path = preview.entries.get(index)?.file_path.clone();
        let extension = path.extension().and_then(|ext| ext.to_str())?;
        let descriptor = builtin_registry().find_by_extension(extension).copied()?;
        Some((index, path, descriptor))
    }

    pub(super) fn clear_playlist_preload(&mut self) {
        self.pending_playlist_preload = None;
        self.ready_playlist_preload = None;
        PLAYLIST_PRELOAD_TOKEN.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn take_ready_playlist_preload(
        &mut self,
        index: usize,
        path: &Path,
    ) -> Option<DecodedAudioLoadJobResult> {
        let preload = self.ready_playlist_preload.take()?;
        if preload.index == index
            && preload.path == path
            && preload.token == PLAYLIST_PRELOAD_TOKEN.load(Ordering::Relaxed)
        {
            self.pending_playlist_preload = None;
            Some(preload.result)
        } else {
            self.ready_playlist_preload = Some(preload);
            None
        }
    }

    pub fn on_track_end(&mut self) -> bool {
        let repeat_track = self
            .playlist
            .as_ref()
            .is_some_and(|playlist| playlist.repeat_mode == PlaylistRepeatMode::Track);
        if repeat_track {
            self.play();
            return true;
        }

        if self.navigate_playlist(true, true) {
            return true;
        }

        let wrap = self.playlist.as_ref().is_some_and(|playlist| {
            playlist.loop_enabled || playlist.repeat_mode == PlaylistRepeatMode::Playlist
        });
        if !wrap {
            return false;
        }

        let first_index = self.playlist.as_mut().and_then(|playlist| {
            playlist.first_valid_track_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
        });
        if let Some(index) = first_index {
            return self.load_playlist_track_by_index(index, false, true);
        }

        false
    }

    pub(super) fn playlist_has_multiple_entries(&self) -> bool {
        self.playlist
            .as_ref()
            .is_some_and(|playlist| playlist.entries.len() > 1)
    }
}
