use super::*;

impl FlutzDesktopApp {
    pub fn master(&mut self) -> &mut MasterControls {
        &mut self.master
    }

    pub fn smart_mix(&mut self) -> &mut SmartMixControls {
        &mut self.smart_mix
    }

    pub fn soundfont_rows(&self) -> &[SoundFontUiRow] {
        &self.soundfonts
    }

    pub fn soundfonts(&mut self) -> &mut Vec<SoundFontUiRow> {
        &mut self.soundfonts
    }

    pub fn mixer_fx_expanded(&self) -> bool {
        self.mixer_fx_expanded
    }

    pub fn set_mixer_fx_expanded(&mut self, expanded: bool) {
        self.mixer_fx_expanded = expanded;
    }

    pub fn selected_soundfont(&mut self) -> &mut usize {
        &mut self.selected_soundfont
    }

    pub fn selected_soundfont_index(&self) -> usize {
        self.selected_soundfont
    }

    pub fn set_selected_soundfont_index(&mut self, index: usize) {
        self.selected_soundfont = index.min(self.catalog_soundfonts.len().saturating_sub(1));
    }

    pub fn mixer_assignment_mode(&self) -> MixerAssignmentMode {
        self.mixer_assignment_mode
    }

    pub fn all_mixer_rows_collapsed(&self) -> bool {
        !self.soundfonts.is_empty() && self.soundfonts.iter().all(|font| font.collapsed)
    }

    pub fn toggle_all_mixer_rows_collapsed(&mut self) {
        let collapsed = !self.all_mixer_rows_collapsed();
        for font in &mut self.soundfonts {
            font.collapsed = collapsed;
        }
    }

    pub fn apply_balanced_mixer_assignment(&mut self) {
        self.apply_mixer_assignment_mode(MixerAssignmentMode::Balance);
    }

    pub fn apply_layered_mixer_assignment(&mut self) {
        self.apply_mixer_assignment_mode(MixerAssignmentMode::Layer);
    }

    pub fn add_selected_soundfont(&mut self) {
        let trace = self.begin_user_trace("soundfont.add");
        let Some(selected) = self
            .catalog_soundfonts
            .get(self.selected_soundfont)
            .map(|entry| entry.internal_id.clone())
        else {
            self.set_status("No catalog soundfont selected");
            self.finish_user_trace(trace, "soundfont.add", "none_selected");
            return;
        };

        if self
            .soundfonts
            .iter()
            .any(|font| font.internal_id == selected)
        {
            self.set_status("Soundfont is already loaded");
            self.finish_user_trace(trace, "soundfont.add", "already_loaded");
            return;
        }

        let mut requested = self
            .soundfonts
            .iter()
            .map(|font| font.internal_id.clone())
            .collect::<Vec<_>>();
        requested.push(selected);
        self.convert_to_custom_mode();
        self.apply_soundfont_set_change(requested, "Added soundfont");
        self.finish_user_trace(trace, "soundfont.add", "completed");
    }

    pub fn remove_selected_soundfont(&mut self) {
        let trace = self.begin_user_trace("soundfont.remove");
        let Some(selected) = self
            .catalog_soundfonts
            .get(self.selected_soundfont)
            .map(|entry| entry.internal_id.clone())
        else {
            self.set_status("No catalog soundfont selected");
            self.finish_user_trace(trace, "soundfont.remove", "none_selected");
            return;
        };

        if !self
            .soundfonts
            .iter()
            .any(|font| font.internal_id == selected)
        {
            self.set_status("Selected catalog soundfont is not loaded");
            self.finish_user_trace(trace, "soundfont.remove", "not_loaded");
            return;
        }

        let requested = self
            .soundfonts
            .iter()
            .map(|font| font.internal_id.clone())
            .filter(|id| id != &selected)
            .collect::<Vec<_>>();

        if requested.is_empty() {
            self.set_status("Cannot remove the last loaded soundfont");
            self.finish_user_trace(trace, "soundfont.remove", "last_soundfont");
            return;
        }

        self.convert_to_custom_mode();
        self.apply_soundfont_set_change(requested, "Removed soundfont");
        self.finish_user_trace(trace, "soundfont.remove", "completed");
    }

    pub(in crate::app) fn reset_strip_layout(&mut self) {
        let layout = self.playback.midi_strip_layout();
        self.logged_unmatched_snapshot_strips.clear();
        let active_preset = self.preset_set.find_preset(&self.active_preset_id);
        let font_ids = self
            .soundfonts
            .iter()
            .map(|font| font.internal_id.clone())
            .collect::<Vec<_>>();
        for (font_index, font) in self.soundfonts.iter_mut().enumerate() {
            font.muted = false;
            font.soloed = false;
            font.strips = layout
                .iter()
                .map(|&(channel, bank, program, is_percussion)| {
                    let routing_decision = active_preset.and_then(|preset| {
                        compute_strip_routing(
                            preset,
                            &font_ids,
                            &self.coverage_cache,
                            program,
                            bank,
                            is_percussion,
                        )
                        .into_iter()
                        .find(|decision| decision.font_index == font_index)
                    });
                    let strip_id = strip_id_for(font_index, channel, bank, program, is_percussion);
                    MixerStripUiState {
                        strip_id,
                        channel,
                        bank,
                        program,
                        is_percussion,
                        program_name: if is_percussion {
                            "Percussion".to_owned()
                        } else {
                            format!("Program {}", program)
                        },
                        volume: routing_decision
                            .map(|decision| decision.volume)
                            .unwrap_or(1.0),
                        muted: routing_decision
                            .map(|decision| decision.muted)
                            .unwrap_or(false),
                        unsupported: routing_decision
                            .map(|decision| decision.unsupported)
                            .unwrap_or(false),
                        mute_policy_default: routing_decision.is_some(),
                        volume_policy_default: routing_decision.is_some(),
                        soloed: false,
                        pan: 0.0,
                        gain_db: 0.0,
                        limiter_enabled: false,
                        limiter_amount: 0.25,
                        reverb: 0.0,
                        chorus: 0.0,
                        meter: 0.0,
                        active: false,
                        current_note: "--".to_owned(),
                    }
                })
                .collect();
        }
    }
}
