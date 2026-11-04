use super::*;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LoopMode {
    None,
    Infinite,
    Counted,
}

impl LoopMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Infinite => "inf",
            Self::Counted => "n",
        }
    }

    fn playback_mode(self) -> PlaybackLoopMode {
        match self {
            Self::None => PlaybackLoopMode::None,
            Self::Infinite => PlaybackLoopMode::Infinite,
            Self::Counted => PlaybackLoopMode::Counted,
        }
    }
}

impl FlutzDesktopApp {
    pub fn play(&mut self) {
        let trace = self.begin_user_trace("transport.play");
        let mut outcome = "error";
        match self.playback.play() {
            Ok(AudioPlaybackStatus::Audible) => {
                self.run_state = AppRunState::Playing;
                self.set_status("Playing");
                outcome = "audible";
            }
            Ok(AudioPlaybackStatus::AudioUnavailable(message)) => {
                self.run_state = AppRunState::Paused;
                self.set_status(format!(
                    "{message}. Connect an output device and use Retry Audio."
                ));
                outcome = "audio_unavailable";
            }
            Err(error) => self.set_status(format!("{error}")),
        }
        self.finish_user_trace(trace, "transport.play", outcome);
    }

    pub fn retry_audio(&mut self) {
        let trace = self.begin_user_trace("transport.retry_audio");
        let mut outcome = "error";
        match self.playback.retry_audio() {
            Ok(()) => {
                self.set_status("Audio output ready");
                outcome = "ok";
            }
            Err(error) => self.set_status(format!(
                "Audio still unavailable: {error}. Connect an output device and retry."
            )),
        }
        self.finish_user_trace(trace, "transport.retry_audio", outcome);
    }

    pub fn pause(&mut self) {
        let trace = self.begin_user_trace("transport.pause");
        let mut outcome = "error";
        match self.playback.pause() {
            Ok(()) => {
                self.run_state = AppRunState::Paused;
                self.set_status("Paused");
                outcome = "ok";
            }
            Err(error) => self.set_status(format!("{error}")),
        }
        self.finish_user_trace(trace, "transport.pause", outcome);
    }

    pub fn stop(&mut self) {
        let trace = self.begin_user_trace("transport.stop");
        let mut outcome = "error";
        match self.playback.stop() {
            Ok(()) => {
                self.run_state = AppRunState::Idle;
                self.transport_position = 0.0;
                self.set_status("Stopped");
                self.publish_media_controls_snapshot();
                outcome = "ok";
            }
            Err(error) => self.set_status(format!("{error}")),
        }
        self.finish_user_trace(trace, "transport.stop", outcome);
    }

    pub fn transport_position(&mut self) -> &mut f32 {
        &mut self.transport_position
    }

    pub fn seek_transport_fraction(&mut self, fraction: f32) {
        let trace = self.begin_user_trace("transport.seek");
        let clamped = fraction.clamp(0.0, 1.0);
        self.transport_position = clamped;
        let mut outcome = "ok";
        if let Err(error) = self.playback.seek_transport_fraction(clamped) {
            self.set_status(format!("{error}"));
            outcome = "error";
        } else {
            let target_tick = (self.transport_tick_length() as f32 * clamped).round() as u64;
            self.handle_counted_loop_seek(target_tick);
        }
        self.finish_user_trace(trace, "transport.seek", outcome);
    }

    pub fn seek_transport_tick(&mut self, tick: u64) {
        let trace = self.begin_user_trace("transport.seek_tick");
        let clamped = tick.min(self.transport_tick_length());
        let mut outcome = "ok";
        if let Err(error) = self.playback.seek_transport_tick(clamped) {
            self.set_status(format!("{error}"));
            outcome = "error";
        } else {
            self.handle_counted_loop_seek(clamped);
        }
        self.playback.update_latency_control();
        self.transport_position = self.playback.transport_fraction();
        self.finish_user_trace(trace, "transport.seek_tick", outcome);
    }

    pub fn loop_enabled_value(&self) -> bool {
        self.loop_enabled
    }

    pub fn set_loop_enabled(&mut self, enabled: bool) {
        let trace = self.begin_user_trace("transport.loop_enabled");
        if let Some(playlist) = self.playlist.as_mut() {
            let desired_repeat_mode = if enabled {
                PlaylistRepeatMode::Track
            } else {
                PlaylistRepeatMode::Off
            };

            if playlist.repeat_mode != desired_repeat_mode {
                playlist.set_repeat_mode(desired_repeat_mode);
                if desired_repeat_mode != PlaylistRepeatMode::Playlist {
                    playlist.loop_enabled = false;
                }
                playlist.dirty = true;
            }
        }

        let previous = self.loop_state_tuple();
        self.loop_enabled = enabled;
        if enabled && matches!(self.loop_mode, LoopMode::None) {
            self.loop_mode = LoopMode::Infinite;
        }
        if enabled && self.loop_end_tick == 0 {
            self.loop_end_tick = self.transport_tick_length();
        }
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_enabled", outcome);
    }

    pub fn loop_mode_value(&self) -> LoopMode {
        self.loop_mode
    }

    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        let trace = self.begin_user_trace("transport.loop_mode");
        let previous = self.loop_state_tuple();
        self.loop_mode = mode;
        self.loop_enabled = !matches!(mode, LoopMode::None);
        if self.loop_enabled && self.loop_end_tick == 0 {
            self.loop_end_tick = self.transport_tick_length();
        }
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_mode", outcome);
    }

    pub fn loop_start_tick_value(&self) -> u64 {
        self.loop_start_tick
    }

    pub fn set_loop_start_tick(&mut self, tick: u64) {
        let trace = self.begin_user_trace("transport.loop_start_tick");
        let previous = self.loop_state_tuple();
        self.loop_start_tick = tick;
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_start_tick", outcome);
    }

    pub fn loop_end_tick_value(&self) -> u64 {
        self.loop_end_tick
    }

    pub fn set_loop_end_tick(&mut self, tick: u64) {
        let trace = self.begin_user_trace("transport.loop_end_tick");
        let previous = self.loop_state_tuple();
        self.loop_end_tick = tick;
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_end_tick", outcome);
    }

    pub fn loop_count_value(&self) -> u32 {
        self.loop_count
    }

    pub fn set_loop_count(&mut self, count: u32) {
        let trace = self.begin_user_trace("transport.loop_count");
        let previous = self.loop_state_tuple();
        self.loop_count = count.max(1);
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_count", outcome);
    }

    pub fn transport_seconds(&self) -> f64 {
        self.debug_metrics().transport_seconds
    }

    pub fn transport_duration_seconds(&self) -> f64 {
        self.debug_metrics().transport_duration_seconds
    }

    pub fn transport_tick(&self) -> u64 {
        self.playback.transport_tick()
    }

    pub fn transport_tick_length(&self) -> u64 {
        self.playback
            .decoded_transport_metadata()
            .map(|metadata| metadata.frame_length)
            .unwrap_or_else(|| self.playback.midi_transport_metadata().tick_length)
    }

    pub(super) fn apply_loop_defaults_after_load(&mut self, has_non_default_loop_config: bool) {
        let enable_by_default = self.should_enable_loop_by_default(has_non_default_loop_config);
        if has_non_default_loop_config {
            self.normalize_loop_state(false);
            self.sync_playback_loop_settings();
            return;
        }

        if enable_by_default {
            let tick_length = self.transport_tick_length();
            self.loop_mode = LoopMode::Infinite;
            self.loop_enabled = tick_length > 0;
            self.loop_start_tick = 0;
            self.loop_end_tick = tick_length;
            self.loop_count = 1;
        } else {
            self.loop_mode = LoopMode::None;
            self.loop_enabled = false;
            self.loop_start_tick = 0;
            self.loop_end_tick = 0;
            self.loop_count = 1;
        }

        self.normalize_loop_state(false);
        self.sync_playback_loop_settings();
    }

    pub(super) fn current_loop_settings(&self) -> PlaybackLoopSettings {
        PlaybackLoopSettings {
            enabled: self.loop_enabled,
            mode: self.loop_mode.playback_mode(),
            start_tick: self.loop_start_tick,
            end_tick: self.loop_end_tick,
            loop_count: self.loop_count.max(1),
        }
    }

    pub(super) fn loop_state_tuple(&self) -> (bool, LoopMode, u64, u64, u32) {
        (
            self.loop_enabled,
            self.loop_mode,
            self.loop_start_tick,
            self.loop_end_tick,
            self.loop_count,
        )
    }

    pub(super) fn normalize_loop_state(&mut self, emit_status: bool) {
        let tick_length = self.transport_tick_length();
        self.loop_start_tick = self.loop_start_tick.min(tick_length);
        self.loop_end_tick = self.loop_end_tick.min(tick_length);
        self.loop_count = self.loop_count.max(1);

        if matches!(self.loop_mode, LoopMode::None) {
            self.loop_enabled = false;
            return;
        }

        if self.loop_enabled && self.loop_end_tick <= self.loop_start_tick {
            self.loop_enabled = false;
            if emit_status {
                self.set_status("Loop disabled: start tick must be less than end tick");
            }
        }
    }

    pub(super) fn sync_playback_loop_settings(&mut self) {
        if let Err(error) = self
            .playback
            .set_loop_settings(self.current_loop_settings())
        {
            self.set_status(format!("{error}"));
        }
    }

    pub(super) fn commit_loop_state_change(
        &mut self,
        previous: (bool, LoopMode, u64, u64, u32),
    ) -> &'static str {
        let changed = previous != self.loop_state_tuple();
        let mut outcome = "ok";
        if let Err(error) = self
            .playback
            .set_loop_settings(self.current_loop_settings())
        {
            self.set_status(format!("{error}"));
            outcome = "error";
        }
        if changed {
            self.mark_dirty();
        }
        outcome
    }

    pub(super) fn handle_counted_loop_seek(&mut self, target_tick: u64) {
        if !self.loop_enabled || !matches!(self.loop_mode, LoopMode::Counted) {
            return;
        }
        if target_tick >= self.loop_start_tick && target_tick < self.loop_end_tick {
            return;
        }

        self.loop_enabled = false;
        self.sync_playback_loop_settings();
        self.mark_dirty();
        self.set_status("Loop disabled after seeking outside counted loop range");
    }

    /// Only `sync_playback_controls` writes the shared mixer controls, so the last published
    /// state stays authoritative and an unchanged frame skips the rebuild and the publish.
    pub(super) fn sync_playback_controls(&mut self) {
        let settings = self.current_mixer_settings();
        if !self.mixer_controls_changed_since_publish(&settings) {
            return;
        }
        let mut state = MixerControlState {
            settings,
            strip_controls: std::collections::BTreeMap::new(),
        };
        for font in &self.soundfonts {
            for strip in &font.strips {
                state
                    .strip_controls
                    .insert(strip.strip_id, Self::strip_mixer_controls(font, strip));
            }
        }
        self.published_mixer_controls.clone_from(&state);
        if let Err(error) = self.playback.set_mixer_controls(state) {
            self.set_status(format!("{error}"));
        }
    }

    fn mixer_controls_changed_since_publish(&self, settings: &MixerSettings) -> bool {
        let published = &self.published_mixer_controls;
        if published.settings != *settings {
            return true;
        }
        let mut strip_count = 0;
        for font in &self.soundfonts {
            for strip in &font.strips {
                strip_count += 1;
                if published.strip_controls.get(&strip.strip_id)
                    != Some(&Self::strip_mixer_controls(font, strip))
                {
                    return true;
                }
            }
        }
        strip_count != published.strip_controls.len()
    }

    fn strip_mixer_controls(
        font: &SoundFontUiRow,
        strip: &MixerStripUiState,
    ) -> MixerStripControls {
        MixerStripControls {
            volume: strip.volume as f64,
            mute: font.muted || strip.muted,
            solo: font.soloed || strip.soloed,
            pan: strip.pan as f64,
            gain_db: strip.gain_db as f64,
            limiter: LimiterControls {
                enabled: strip.limiter_enabled,
                amount: strip.limiter_amount as f64,
                release: LimiterControls::default().release,
            },
            reverb: strip.reverb as f64,
            chorus: strip.chorus as f64,
        }
    }
}
