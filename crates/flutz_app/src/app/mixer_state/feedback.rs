use super::*;

impl FlutzDesktopApp {
    pub(in crate::app) fn sync_coverage_cache_from_playback(&mut self) {
        self.coverage_cache = self
            .catalog_soundfonts
            .iter()
            .filter_map(|font| {
                font.coverage
                    .clone()
                    .map(|coverage| (font.internal_id.clone(), coverage))
            })
            .collect();
        self.coverage_cache
            .extend(self.playback.loaded_soundfont_coverages());
    }

    pub(in crate::app) fn apply_realtime_snapshot(&mut self, snapshot: &RealtimeMixerSnapshot) {
        for visual in snapshot.strips.values() {
            let Some(font_index) = self
                .soundfonts
                .iter()
                .position(|font| font.internal_id == visual.soundfont_id)
            else {
                continue;
            };
            let strip_id = strip_id_for(
                font_index,
                visual.midi_channel,
                visual.midi_bank,
                visual.midi_program,
                visual.is_percussion,
            );
            let Some(strip_index) = self.soundfonts[font_index]
                .strips
                .iter()
                .position(|strip| strip.strip_id == strip_id)
            else {
                if visual.display_name.as_deref() != Some("Global effects") {
                    let key = RealtimeStripKey::from_snapshot(visual);
                    if self.logged_unmatched_snapshot_strips.insert(key) {
                        self.record_diagnostic_trace(
                            "midi.mapping.unmatched_realtime_strip",
                            "detected",
                            &[
                                ("soundfont", visual.soundfont_id.clone()),
                                ("channel", visual.midi_channel.to_string()),
                                ("bank", visual.midi_bank.to_string()),
                                ("program", visual.midi_program.to_string()),
                                ("is_percussion", visual.is_percussion.to_string()),
                                ("expected_strip_id", strip_id.0.to_string()),
                            ],
                        );
                    }
                }
                continue;
            };
            let strip = &mut self.soundfonts[font_index].strips[strip_index];
            strip.meter = visual.meter.peak.clamp(0.0, 1.0);
            strip.active = visual.audible || !visual.active_notes.is_empty();
            if let Some(display_name) = &visual.display_name {
                set_text_in_place(&mut strip.program_name, display_name);
            }
            match visual.active_notes.last() {
                Some(note) => write_note_name(&mut strip.current_note, *note),
                None => set_text_in_place(&mut strip.current_note, "--"),
            }
        }
    }

    pub(in crate::app) fn current_mixer_settings(&self) -> MixerSettings {
        let target_peak = 10.0_f32.powf(self.smart_mix.target_headroom_db / 20.0);
        MixerSettings {
            master: MixerMasterControls {
                volume_db: self.master.volume_db as f64,
                limiter: LimiterControls {
                    enabled: self.master.limiter_enabled,
                    amount: self.master.limiter_amount as f64,
                    release: LimiterControls::default().release,
                },
                reverb: self.master.reverb as f64,
                chorus: self.master.chorus as f64,
                eq_low_db: self.master.eq_low as f64,
                eq_mid_db: self.master.eq_mid as f64,
                eq_high_db: self.master.eq_high as f64,
            },
            smart_mix: SmartMixSettings {
                enabled: self.smart_mix.enabled,
                target_headroom: (1.0 - target_peak.clamp(0.05, 1.0)) as f64,
                attack: (self.smart_mix.attack_ms / 2_000.0).clamp(0.0, 1.0) as f64,
                release: (self.smart_mix.release_ms / 2_000.0).clamp(0.0, 1.0) as f64,
                lookahead: (self.smart_mix.lookahead_ms / 2_000.0).clamp(0.0, 1.0) as f64,
            },
            auto_normalization: AutoNormalization {
                enabled: self.smart_mix.auto_normalize,
                amount: (self.smart_mix.normalization_amount / 100.0).clamp(0.0, 1.0) as f64,
            },
        }
    }
}
