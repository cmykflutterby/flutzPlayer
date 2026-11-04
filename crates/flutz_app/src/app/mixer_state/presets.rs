use super::*;

impl FlutzDesktopApp {
    pub(in crate::app) fn apply_preset(&mut self, preset: &'static Preset, mark_dirty: bool) {
        let requested_soundfonts = preset
            .font_ids
            .iter()
            .map(|font_id| (*font_id).to_owned())
            .collect::<Vec<_>>();

        let midi_bytes = self
            .playback
            .loaded_midi_bytes()
            .map(|bytes| bytes.to_vec());
        let source_name = self
            .playback
            .loaded_midi()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "embedded.mid".to_owned());
        let resume_state = self.capture_playback_reload_resume_state();

        if let Some(midi_bytes) = midi_bytes {
            match self
                .playback
                .load_midi_bytes(midi_bytes, source_name, &requested_soundfonts)
            {
                Ok(message) => {
                    self.active_preset_id = preset.id.to_owned();
                    self.selected_preset_id = preset.id.to_owned();
                    self.mixer_assignment_mode = MixerAssignmentMode::Manual;
                    let loaded_ids = self.playback.loaded_soundfont_ids();
                    self.soundfonts = self.soundfont_rows_for_ids(&loaded_ids);
                    self.sync_coverage_cache_from_playback();
                    self.reset_strip_layout();
                    self.sync_playback_controls();
                    self.sync_playback_loop_settings();
                    self.restore_playback_reload_resume_state(resume_state);
                    if mark_dirty {
                        self.mark_dirty();
                    }
                    self.set_status(format!("Preset {} applied: {message}", preset.display_name));
                }
                Err(error) => self.set_status(format!("{error}")),
            }
            return;
        }

        self.active_preset_id = preset.id.to_owned();
        self.selected_preset_id = preset.id.to_owned();
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        self.soundfonts = self.soundfont_rows_for_ids(&requested_soundfonts);
        self.sync_coverage_cache_from_playback();
        self.reset_strip_layout();
        self.sync_playback_controls();
        if mark_dirty {
            self.mark_dirty();
        }
        self.set_status(format!("Preset {} applied", preset.display_name));
    }

    pub(in crate::app) fn convert_to_custom_mode(&mut self) {
        self.active_preset_id = "custom".to_owned();
    }

    pub(in crate::app) fn convert_manual_mixer_edit_to_custom_mode(&mut self) {
        self.convert_to_custom_mode();
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        self.mark_dirty();
    }

    pub(in crate::app) fn apply_mixer_assignment_mode(&mut self, mode: MixerAssignmentMode) {
        self.sync_coverage_cache_from_playback();
        self.convert_to_custom_mode();
        self.mixer_assignment_mode = mode;

        let mut top_provider_by_strip = BTreeMap::new();
        for (font_index, font) in self.soundfonts.iter().enumerate() {
            for strip in &font.strips {
                if coverage_supports_strip(self.coverage_cache.get(&font.internal_id), strip) {
                    top_provider_by_strip.insert(
                        (
                            strip.channel,
                            strip.bank,
                            strip.program,
                            strip.is_percussion,
                        ),
                        font_index,
                    );
                }
            }
        }

        for (font_index, font) in self.soundfonts.iter_mut().enumerate() {
            font.muted = false;
            font.soloed = false;
            for strip in &mut font.strips {
                strip.volume = 1.0;
                strip.volume_policy_default = false;
                strip.mute_policy_default = false;
                strip.soloed = false;
                strip.muted = match mode {
                    MixerAssignmentMode::Manual => strip.muted,
                    MixerAssignmentMode::Balance => false,
                    MixerAssignmentMode::Layer => {
                        let key = (
                            strip.channel,
                            strip.bank,
                            strip.program,
                            strip.is_percussion,
                        );
                        match top_provider_by_strip.get(&key) {
                            Some(provider_index) => *provider_index != font_index,
                            None => true,
                        }
                    }
                };
            }
        }

        self.sync_playback_controls();
        self.mark_dirty();
        self.set_status(match mode {
            MixerAssignmentMode::Manual => "Mixer -> Custom".to_owned(),
            MixerAssignmentMode::Balance => "Mixer -> Balanced".to_owned(),
            MixerAssignmentMode::Layer => "Mixer -> Layered".to_owned(),
        });
    }

    pub(in crate::app) fn capture_playback_reload_resume_state(&self) -> PlaybackReloadResumeState {
        PlaybackReloadResumeState {
            run_state: self.run_state,
            transport_seconds: self.playback.audible_transport_seconds(),
        }
    }

    pub(in crate::app) fn restore_playback_reload_resume_state(
        &mut self,
        resume_state: PlaybackReloadResumeState,
    ) {
        if resume_state.transport_seconds > 0.0 {
            if let Err(error) = self
                .playback
                .seek_transport_seconds(resume_state.transport_seconds)
            {
                self.set_status(format!("{error}"));
            }
        }
        self.transport_position = self.playback.transport_fraction();
        match resume_state.run_state {
            AppRunState::Playing => self.play(),
            AppRunState::Paused => {
                self.run_state = AppRunState::Paused;
                let _ = self.playback.pause();
            }
            AppRunState::Idle => self.run_state = AppRunState::Idle,
        }
    }

    /// Refills `snapshot` in place so its row/strip buffers are reused across frames.
    pub(in crate::app) fn fill_mixer_edit_snapshot(&self, snapshot: &mut MixerEditSnapshot) {
        snapshot.master.clone_from(&self.master);
        snapshot.smart_mix.clone_from(&self.smart_mix);
        snapshot.rows.truncate(self.soundfonts.len());
        for (index, row) in self.soundfonts.iter().enumerate() {
            if index == snapshot.rows.len() {
                snapshot.rows.push(RowEditSnapshot {
                    internal_id: String::new(),
                    muted: false,
                    soloed: false,
                    strips: Vec::new(),
                });
            }
            let target = &mut snapshot.rows[index];
            target.internal_id.clone_from(&row.internal_id);
            target.muted = row.muted;
            target.soloed = row.soloed;
            target.strips.clear();
            target
                .strips
                .extend(row.strips.iter().map(|strip| StripEditSnapshot {
                    channel: strip.channel,
                    bank: strip.bank,
                    program: strip.program,
                    is_percussion: strip.is_percussion,
                    volume: strip.volume,
                    muted: strip.muted,
                    soloed: strip.soloed,
                    pan: strip.pan,
                    gain_db: strip.gain_db,
                    limiter_enabled: strip.limiter_enabled,
                    limiter_amount: strip.limiter_amount,
                    reverb: strip.reverb,
                    chorus: strip.chorus,
                }));
        }
    }

    pub(in crate::app) fn apply_mixer_edit_snapshot(&mut self, snapshot: &MixerEditSnapshot) {
        let mut mixer_changed =
            self.master != snapshot.master || self.smart_mix != snapshot.smart_mix;

        for (row_index, row) in self.soundfonts.iter_mut().enumerate() {
            let Some(previous_row) = find_with_index_hint(&snapshot.rows, row_index, |previous| {
                previous.internal_id == row.internal_id
            }) else {
                continue;
            };
            if row.muted != previous_row.muted {
                mixer_changed = true;
            }
            if row.soloed != previous_row.soloed {
                mixer_changed = true;
            }

            for (strip_index, strip) in row.strips.iter_mut().enumerate() {
                let Some(previous_strip) =
                    find_with_index_hint(&previous_row.strips, strip_index, |previous| {
                        previous.channel == strip.channel
                            && previous.bank == strip.bank
                            && previous.program == strip.program
                            && previous.is_percussion == strip.is_percussion
                    })
                else {
                    continue;
                };
                if strip.volume != previous_strip.volume {
                    strip.volume_policy_default = false;
                    mixer_changed = true;
                }
                if strip.muted != previous_strip.muted {
                    strip.mute_policy_default = false;
                    mixer_changed = true;
                }
                if strip.soloed != previous_strip.soloed {
                    mixer_changed = true;
                }
                if strip.pan != previous_strip.pan
                    || strip.gain_db != previous_strip.gain_db
                    || strip.limiter_enabled != previous_strip.limiter_enabled
                    || strip.limiter_amount != previous_strip.limiter_amount
                    || strip.reverb != previous_strip.reverb
                    || strip.chorus != previous_strip.chorus
                {
                    strip.mute_policy_default = false;
                    strip.volume_policy_default = false;
                    mixer_changed = true;
                }
            }
        }

        if mixer_changed {
            self.convert_manual_mixer_edit_to_custom_mode();
            self.record_user_trace_point("mixer.edit", "changed");
        }
    }

    pub(in crate::app) fn apply_soundfont_set_change(
        &mut self,
        requested_soundfonts: Vec<String>,
        action: &str,
    ) {
        let previous_rows = self.soundfonts.clone();
        let resume_state = self.capture_playback_reload_resume_state();
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;

        let midi_bytes = self
            .playback
            .loaded_midi_bytes()
            .map(|bytes| bytes.to_vec());
        let source_name = self
            .playback
            .loaded_midi()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "embedded.mid".to_owned());

        if let Some(midi_bytes) = midi_bytes {
            match self
                .playback
                .load_midi_bytes(midi_bytes, source_name, &requested_soundfonts)
            {
                Ok(message) => {
                    let loaded_ids = self.playback.loaded_soundfont_ids();
                    self.soundfonts = self.soundfont_rows_for_ids(&loaded_ids);
                    self.sync_coverage_cache_from_playback();
                    self.selected_soundfont = self
                        .catalog_soundfonts
                        .iter()
                        .position(|entry| {
                            self.soundfonts
                                .first()
                                .map(|font| font.internal_id.as_str())
                                .is_some_and(|id| id == entry.internal_id)
                        })
                        .unwrap_or(0);
                    self.reset_strip_layout();
                    self.restore_soundfont_row_controls(&previous_rows);
                    self.sync_playback_controls();
                    self.sync_playback_loop_settings();
                    self.restore_playback_reload_resume_state(resume_state);
                    self.mark_dirty();
                    self.set_status(format!("{action}: {message}"));
                }
                Err(error) => self.set_status(format!("{error}")),
            }
            return;
        }

        self.soundfonts = self.soundfont_rows_for_ids(&requested_soundfonts);
        self.sync_coverage_cache_from_playback();
        self.selected_soundfont = self
            .catalog_soundfonts
            .iter()
            .position(|entry| {
                self.soundfonts
                    .first()
                    .map(|font| font.internal_id.as_str())
                    .is_some_and(|id| id == entry.internal_id)
            })
            .unwrap_or(0);
        self.reset_strip_layout();
        self.restore_soundfont_row_controls(&previous_rows);
        self.sync_playback_controls();
        self.mark_dirty();
        self.set_status(action);
    }

    pub(in crate::app) fn restore_soundfont_row_controls(
        &mut self,
        previous_rows: &[SoundFontUiRow],
    ) {
        let previous_by_id = previous_rows
            .iter()
            .map(|row| (row.internal_id.clone(), row))
            .collect::<BTreeMap<_, _>>();

        for row in &mut self.soundfonts {
            let Some(previous_row) = previous_by_id.get(&row.internal_id) else {
                continue;
            };

            row.collapsed = previous_row.collapsed;
            row.muted = previous_row.muted;

            let previous_strip_map = previous_row
                .strips
                .iter()
                .map(|strip| {
                    (
                        (
                            strip.channel,
                            strip.bank,
                            strip.program,
                            strip.is_percussion,
                        ),
                        (
                            strip.volume,
                            strip.muted,
                            strip.pan,
                            strip.gain_db,
                            strip.limiter_enabled,
                            strip.limiter_amount,
                            strip.reverb,
                            strip.chorus,
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>();

            for strip in &mut row.strips {
                let Some(saved) = previous_strip_map.get(&(
                    strip.channel,
                    strip.bank,
                    strip.program,
                    strip.is_percussion,
                )) else {
                    continue;
                };

                strip.volume = saved.0;
                strip.muted = saved.1;
                strip.unsupported = false;
                strip.mute_policy_default = false;
                strip.volume_policy_default = false;
                strip.pan = saved.2;
                strip.gain_db = saved.3;
                strip.limiter_enabled = saved.4;
                strip.limiter_amount = saved.5;
                strip.reverb = saved.6;
                strip.chorus = saved.7;
            }
        }
    }

    pub(in crate::app) fn soundfont_rows_for_ids(&self, ids: &[String]) -> Vec<SoundFontUiRow> {
        if ids.is_empty() {
            return vec![self
                .catalog_soundfonts
                .iter()
                .find(|font| font.is_default)
                .or_else(|| self.catalog_soundfonts.first())
                .map(SoundFontUiRow::from_catalog_entry)
                .unwrap_or_else(SoundFontUiRow::default_retro)];
        }

        ids.iter()
            .map(|id| {
                self.catalog_soundfonts
                    .iter()
                    .find(|entry| entry.internal_id == *id)
                    .map(SoundFontUiRow::from_catalog_entry)
                    .unwrap_or_else(|| SoundFontUiRow {
                        internal_id: id.clone(),
                        display_name: id.clone(),
                        is_default: false,
                        collapsed: true,
                        muted: false,
                        soloed: false,
                        strips: Vec::new(),
                    })
            })
            .collect()
    }

    pub(in crate::app) fn apply_fmid_mixer_settings(&mut self, fmid: &FmidFile) {
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        let row_mutes = fmid
            .mixer
            .row_mutes
            .iter()
            .map(|row| (row.soundfont_id.as_str(), row.muted))
            .collect::<std::collections::BTreeMap<_, _>>();

        for font in &mut self.soundfonts {
            font.muted = row_mutes
                .get(font.internal_id.as_str())
                .copied()
                .unwrap_or(false);
            font.soloed = false;
            for strip in &mut font.strips {
                strip.soloed = false;
                strip.muted = false;
            }
        }

        for saved_strip in &fmid.mixer.strips {
            let Some(font) = self
                .soundfonts
                .iter_mut()
                .find(|font| font.internal_id == saved_strip.identity.soundfont_id)
            else {
                continue;
            };

            let Some(strip) = font.strips.iter_mut().find(|strip| {
                strip.channel as u64 == saved_strip.identity.midi_channel
                    && strip.program as u64 == saved_strip.identity.midi_program
                    && strip.is_percussion == saved_strip.identity.is_percussion
            }) else {
                continue;
            };

            strip.volume = saved_strip.controls.volume as f32;
            strip.muted = saved_strip.controls.mute;
            strip.pan = saved_strip.controls.pan as f32;
            strip.gain_db = saved_strip.controls.gain_db as f32;
            strip.limiter_enabled = saved_strip.controls.limiter_enabled;
            strip.limiter_amount = saved_strip.controls.limiter_amount as f32;
            strip.reverb = saved_strip.controls.reverb as f32;
            strip.chorus = saved_strip.controls.chorus as f32;
        }
    }

    pub(in crate::app) fn apply_fmid_mixer_source_mode(&mut self, fmid: &FmidFile) {
        match &fmid.mixer_source_mode {
            MixerSourceMode::Custom => {
                self.active_preset_id = "custom".to_owned();
                self.apply_fmid_mixer_settings(fmid);
            }
            MixerSourceMode::PresetDefault(preset_id) => {
                let preset = self
                    .preset_set
                    .find_preset(preset_id)
                    .unwrap_or_else(|| self.preset_set.default_preset());
                self.active_preset_id = preset.id.to_owned();
                self.selected_preset_id = preset.id.to_owned();
            }
        }
    }
}

/// Rows and strips keep the same order between the edit snapshot and the live state unless
/// the UI reorders them, so the same index is tried first. Keys are unique per list; the
/// fallback scans from the end to match the previous last-wins map lookup.
fn find_with_index_hint<T>(items: &[T], hint: usize, matches: impl Fn(&T) -> bool) -> Option<&T> {
    items
        .get(hint)
        .filter(|item| matches(item))
        .or_else(|| items.iter().rev().find(|item| matches(item)))
}
