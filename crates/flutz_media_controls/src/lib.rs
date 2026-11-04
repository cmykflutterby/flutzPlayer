#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod noop;
#[cfg(windows)]
mod windows;

use std::{path::PathBuf, sync::Arc, time::Duration};

pub use noop::NoopMediaControlsBackend;

pub const MEDIA_CONTROLS_TIMELINE_PUBLISH_INTERVAL_MS: u64 = 1_000;
pub const MEDIA_CONTROLS_TIMELINE_PUBLISH_INTERVAL: Duration =
    Duration::from_millis(MEDIA_CONTROLS_TIMELINE_PUBLISH_INTERVAL_MS);

/// Called from the thread that queued a command so the host can poll promptly.
pub type MediaCommandWaker = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum MediaControlCommand {
    Play,
    Pause,
    TogglePlayPause,
    Stop,
    NextTrack,
    PreviousTrack,
    SeekToFraction(f32),
    SeekBySeconds(f64),
}

pub const MEDIA_COMMAND_ARGUMENT_NAME: &str = "--media-command";

pub fn media_command_argument(command: &MediaControlCommand) -> Option<&'static str> {
    match command {
        MediaControlCommand::Play => Some("play"),
        MediaControlCommand::Pause => Some("pause"),
        MediaControlCommand::TogglePlayPause => Some("play-pause"),
        MediaControlCommand::Stop => Some("stop"),
        MediaControlCommand::NextTrack => Some("next"),
        MediaControlCommand::PreviousTrack => Some("previous"),
        MediaControlCommand::SeekToFraction(_) | MediaControlCommand::SeekBySeconds(_) => None,
    }
}

pub fn media_command_from_argument(value: &str) -> Option<MediaControlCommand> {
    match value.trim().to_ascii_lowercase().as_str() {
        "play" => Some(MediaControlCommand::Play),
        "pause" => Some(MediaControlCommand::Pause),
        "play-pause" | "toggle" | "toggle-play-pause" => Some(MediaControlCommand::TogglePlayPause),
        "stop" => Some(MediaControlCommand::Stop),
        "next" | "next-track" => Some(MediaControlCommand::NextTrack),
        "previous" | "prev" | "previous-track" | "prev-track" => {
            Some(MediaControlCommand::PreviousTrack)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaPlaybackStatus {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaTrackSnapshot {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub source_path: Option<PathBuf>,
    pub duration_seconds: f64,
    pub position_seconds: f64,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_stop: bool,
    pub can_seek: bool,
    pub can_next: bool,
    pub can_previous: bool,
    pub status: MediaPlaybackStatus,
}

impl MediaTrackSnapshot {
    pub fn has_immediate_publish_change(&self, previous: &Self) -> bool {
        self.title != previous.title
            || self.artist != previous.artist
            || self.album != previous.album
            || self.source_path != previous.source_path
            || self.can_play != previous.can_play
            || self.can_pause != previous.can_pause
            || self.can_stop != previous.can_stop
            || self.can_seek != previous.can_seek
            || self.can_next != previous.can_next
            || self.can_previous != previous.can_previous
            || self.status != previous.status
    }

    pub fn has_timeline_change(&self, previous: &Self) -> bool {
        self.duration_seconds != previous.duration_seconds
            || self.position_seconds != previous.position_seconds
    }
}

pub fn should_publish_snapshot(
    previous: Option<&MediaTrackSnapshot>,
    next: &MediaTrackSnapshot,
    timeline_elapsed: Option<Duration>,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    if next.has_immediate_publish_change(previous) {
        return true;
    }
    next.has_timeline_change(previous)
        && timeline_elapsed
            .is_some_and(|elapsed| elapsed >= MEDIA_CONTROLS_TIMELINE_PUBLISH_INTERVAL)
}

pub trait MediaControlsBackend {
    fn poll_command(&mut self) -> Option<MediaControlCommand>;
    fn publish_snapshot(&mut self, snapshot: &MediaTrackSnapshot);
    fn shutdown(&mut self);
    /// Backends that receive commands asynchronously call `waker` after queueing one.
    fn set_command_waker(&mut self, _waker: MediaCommandWaker) {}
}

pub fn create_media_controls_backend() -> Box<dyn MediaControlsBackend> {
    create_platform_media_controls_backend()
}

#[cfg(target_os = "linux")]
fn create_platform_media_controls_backend() -> Box<dyn MediaControlsBackend> {
    linux::create_media_controls_backend()
}

#[cfg(target_os = "macos")]
fn create_platform_media_controls_backend() -> Box<dyn MediaControlsBackend> {
    macos::create_media_controls_backend()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn create_platform_media_controls_backend() -> Box<dyn MediaControlsBackend> {
    Box::new(NoopMediaControlsBackend::default())
}

#[cfg(windows)]
pub fn create_media_controls_backend_for_window(hwnd: isize) -> Box<dyn MediaControlsBackend> {
    windows::WindowsMediaControlsBackend::new(hwnd)
        .map(|backend| Box::new(backend) as Box<dyn MediaControlsBackend>)
        .unwrap_or_else(|_| create_media_controls_backend())
}

#[cfg(not(windows))]
pub fn create_media_controls_backend_for_window(_hwnd: isize) -> Box<dyn MediaControlsBackend> {
    create_media_controls_backend()
}

#[cfg(windows)]
pub fn register_media_control_jump_list_tasks() -> std::result::Result<(), String> {
    windows::register_jump_list_tasks().map_err(|error| error.to_string())
}

#[cfg(not(windows))]
pub fn register_media_control_jump_list_tasks() -> std::result::Result<(), String> {
    Ok(())
}

#[cfg(windows)]
pub fn send_media_command_to_running_instance(command: MediaControlCommand) -> bool {
    windows::send_media_command_to_running_instance(command)
}

#[cfg(not(windows))]
pub fn send_media_command_to_running_instance(_command: MediaControlCommand) -> bool {
    false
}
