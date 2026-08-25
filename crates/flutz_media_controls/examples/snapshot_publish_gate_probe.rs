use std::{path::PathBuf, time::Duration};

use flutz_media_controls::{
    should_publish_snapshot, MediaControlsBackend, MediaPlaybackStatus, MediaTrackSnapshot,
    NoopMediaControlsBackend, MEDIA_CONTROLS_TIMELINE_PUBLISH_INTERVAL_MS,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = snapshot("Track", 0.0, MediaPlaybackStatus::Stopped);
    if !should_publish_snapshot(None, &base, None) {
        return Err("initial media snapshot was not publishable".into());
    }

    let mut backend = NoopMediaControlsBackend::default();
    backend.publish_snapshot(&base);
    if backend.last_snapshot() != Some(&base) {
        return Err("no-op backend did not retain the last published snapshot".into());
    }

    let same = base.clone();
    if should_publish_snapshot(Some(&base), &same, Some(Duration::from_millis(10_000))) {
        return Err("unchanged media snapshot should not publish".into());
    }

    let renamed = snapshot("Renamed", 0.0, MediaPlaybackStatus::Stopped);
    if !should_publish_snapshot(Some(&base), &renamed, Some(Duration::ZERO)) {
        return Err("metadata changes should publish immediately".into());
    }

    let playing = snapshot("Track", 0.0, MediaPlaybackStatus::Playing);
    if !should_publish_snapshot(Some(&base), &playing, Some(Duration::ZERO)) {
        return Err("playback-state changes should publish immediately".into());
    }

    let moved_early = snapshot("Track", 0.5, MediaPlaybackStatus::Stopped);
    if should_publish_snapshot(
        Some(&base),
        &moved_early,
        Some(Duration::from_millis(
            MEDIA_CONTROLS_TIMELINE_PUBLISH_INTERVAL_MS - 1,
        )),
    ) {
        return Err("timeline-only changes published before the minimum interval".into());
    }

    if !should_publish_snapshot(
        Some(&base),
        &moved_early,
        Some(Duration::from_millis(
            MEDIA_CONTROLS_TIMELINE_PUBLISH_INTERVAL_MS,
        )),
    ) {
        return Err("timeline-only changes did not publish at the minimum interval".into());
    }

    Ok(())
}

fn snapshot(title: &str, position_seconds: f64, status: MediaPlaybackStatus) -> MediaTrackSnapshot {
    MediaTrackSnapshot {
        title: title.to_owned(),
        artist: Some("Artist".to_owned()),
        album: Some("Album".to_owned()),
        source_path: Some(PathBuf::from("song.fmid")),
        duration_seconds: 120.0,
        position_seconds,
        can_play: true,
        can_pause: true,
        can_stop: true,
        can_seek: true,
        can_next: true,
        can_previous: true,
        status,
    }
}
