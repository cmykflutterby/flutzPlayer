use super::mixer_state::non_empty_media_metadata;
use super::*;

impl FlutzDesktopApp {
    pub(super) fn process_media_control_commands(&mut self) {
        loop {
            let Some(command) = self.media_controls.poll_command() else {
                break;
            };
            self.handle_media_control_command(command);
        }
    }

    pub(super) fn handle_media_control_command(&mut self, command: MediaControlCommand) {
        match command {
            MediaControlCommand::Play => self.play(),
            MediaControlCommand::Pause => self.pause(),
            MediaControlCommand::TogglePlayPause => {
                if self.run_state == AppRunState::Playing {
                    self.pause();
                } else {
                    self.play();
                }
            }
            MediaControlCommand::Stop => self.stop(),
            MediaControlCommand::NextTrack => self.next_track(),
            MediaControlCommand::PreviousTrack => self.prev_track(),
            MediaControlCommand::SeekToFraction(fraction) => self.seek_transport_fraction(fraction),
            MediaControlCommand::SeekBySeconds(seconds) => {
                let duration = self.transport_duration_seconds();
                if duration > 0.0 {
                    let target = (self.transport_seconds() + seconds).clamp(0.0, duration);
                    self.seek_transport_fraction((target / duration) as f32);
                }
            }
        }
    }

    pub(super) fn publish_media_controls_snapshot(&mut self) {
        let snapshot = self.media_track_snapshot();
        let timeline_elapsed = self
            .last_media_controls_timeline_publish
            .map(|published_at| published_at.elapsed());
        if !should_publish_snapshot(
            self.last_media_controls_snapshot.as_ref(),
            &snapshot,
            timeline_elapsed,
        ) {
            return;
        }
        self.media_controls.publish_snapshot(&snapshot);
        self.last_media_controls_snapshot = Some(snapshot);
        self.last_media_controls_timeline_publish = Some(Instant::now());
    }

    pub(super) fn publish_media_controls_shutdown_snapshot(&mut self) {
        let mut snapshot = self.media_track_snapshot();
        snapshot.status = MediaPlaybackStatus::Stopped;
        snapshot.position_seconds = 0.0;
        snapshot.can_play = false;
        snapshot.can_pause = false;
        snapshot.can_stop = false;
        snapshot.can_seek = false;
        snapshot.can_next = false;
        snapshot.can_previous = false;
        self.media_controls.publish_snapshot(&snapshot);
        self.last_media_controls_snapshot = Some(snapshot);
        self.last_media_controls_timeline_publish = Some(Instant::now());
    }

    pub(super) fn media_track_snapshot(&mut self) -> MediaTrackSnapshot {
        let has_project = self.has_loaded_project();
        let duration_seconds = self.media_controls_duration_seconds();
        let position_seconds = self.transport_seconds();
        let can_previous = self.playlist_has_prev_track();
        let can_next = self.playlist_has_next_track();
        let status = match self.run_state {
            AppRunState::Idle => MediaPlaybackStatus::Stopped,
            AppRunState::Playing => MediaPlaybackStatus::Playing,
            AppRunState::Paused => MediaPlaybackStatus::Paused,
        };

        MediaTrackSnapshot {
            title: self.project_title().to_owned(),
            artist: non_empty_media_metadata(&self.metadata_edit_state.artist),
            album: non_empty_media_metadata(&self.metadata_edit_state.album),
            source_path: self.current_path().map(PathBuf::from),
            duration_seconds,
            position_seconds,
            can_play: has_project,
            can_pause: has_project,
            can_stop: has_project,
            can_seek: has_project && duration_seconds > 0.0,
            can_next,
            can_previous,
            status,
        }
    }

    pub(super) fn media_controls_duration_seconds(&self) -> f64 {
        self.playback
            .decoded_transport_metadata()
            .map(|metadata| metadata.duration_seconds)
            .unwrap_or_else(|| self.playback.midi_transport_metadata().duration_seconds)
    }
}
