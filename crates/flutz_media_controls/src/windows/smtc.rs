#![allow(unsafe_code)]

use std::sync::{Arc, Mutex};

use windows::{
    core::{factory, Ref, Result, HSTRING},
    Foundation::{TimeSpan, TypedEventHandler},
    Media::{
        MediaPlaybackStatus as WindowsPlaybackStatus, MediaPlaybackType, MusicDisplayProperties,
        PlaybackPositionChangeRequestedEventArgs, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
        SystemMediaTransportControlsTimelineProperties,
    },
    Win32::{Foundation::HWND, System::WinRT::ISystemMediaTransportControlsInterop},
};

use crate::{
    MediaCommandWaker, MediaControlCommand, MediaControlsBackend, MediaPlaybackStatus,
    MediaTrackSnapshot,
};

use super::{
    relay::{self, WindowsMediaCommandRelayServer},
    CommandQueue,
};

const WINDOWS_TIMESPAN_TICKS_PER_SECOND: f64 = 10_000_000.0;

pub struct WindowsMediaControlsBackend {
    controls: SystemMediaTransportControls,
    button_pressed_token: Option<i64>,
    position_requested_token: Option<i64>,
    commands: CommandQueue,
    relay_server: Option<WindowsMediaCommandRelayServer>,
    duration_seconds: Arc<Mutex<Option<f64>>>,
    last_snapshot: Option<MediaTrackSnapshot>,
}

impl WindowsMediaControlsBackend {
    pub fn new(hwnd: isize) -> Result<Self> {
        let interop: ISystemMediaTransportControlsInterop =
            factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        let controls: SystemMediaTransportControls =
            unsafe { interop.GetForWindow(HWND(hwnd as _))? };
        controls.SetIsEnabled(true)?;

        let commands = CommandQueue::default();
        let duration_seconds: Arc<Mutex<Option<f64>>> = Arc::new(Mutex::new(None));
        let button_commands = commands.clone();
        let button_pressed_token = controls.ButtonPressed(&TypedEventHandler::new(
            move |_sender: Ref<SystemMediaTransportControls>,
                  args: Ref<SystemMediaTransportControlsButtonPressedEventArgs>| {
                if let Some(command) = args
                    .as_ref()
                    .and_then(|args| args.Button().ok())
                    .and_then(command_from_button)
                {
                    button_commands.push(command);
                }
                Ok(())
            },
        ))?;

        let position_commands = commands.clone();
        let position_duration_seconds = Arc::clone(&duration_seconds);
        let position_requested_token =
            controls.PlaybackPositionChangeRequested(&TypedEventHandler::new(
                move |_sender: Ref<SystemMediaTransportControls>,
                      args: Ref<PlaybackPositionChangeRequestedEventArgs>| {
                    if let Some(position) = args
                        .as_ref()
                        .and_then(|args| args.RequestedPlaybackPosition().ok())
                    {
                        let duration_seconds = position_duration_seconds
                            .lock()
                            .ok()
                            .and_then(|duration| *duration);
                        if let Some(duration_seconds) =
                            duration_seconds.filter(|duration| *duration > 0.0)
                        {
                            position_commands.push(MediaControlCommand::SeekToFraction(
                                (seconds_from_timespan(position) / duration_seconds)
                                    .clamp(0.0, 1.0) as f32,
                            ));
                        }
                    }
                    Ok(())
                },
            ))?;

        let relay_server = relay::start_command_server(commands.clone());

        Ok(Self {
            controls,
            button_pressed_token: Some(button_pressed_token),
            position_requested_token: Some(position_requested_token),
            commands,
            relay_server,
            duration_seconds,
            last_snapshot: None,
        })
    }

    fn publish_snapshot_inner(&mut self, snapshot: &MediaTrackSnapshot) -> Result<()> {
        self.controls
            .SetIsEnabled(snapshot.can_play || snapshot.can_pause || snapshot.can_stop)?;
        self.controls.SetIsPlayEnabled(snapshot.can_play)?;
        self.controls.SetIsPauseEnabled(snapshot.can_pause)?;
        self.controls.SetIsStopEnabled(snapshot.can_stop)?;
        self.controls.SetIsNextEnabled(snapshot.can_next)?;
        self.controls.SetIsPreviousEnabled(snapshot.can_previous)?;
        self.controls
            .SetPlaybackStatus(windows_playback_status(snapshot.status))?;
        self.publish_display(snapshot)?;
        self.publish_timeline(snapshot)?;
        if let Ok(mut duration) = self.duration_seconds.lock() {
            *duration = (snapshot.duration_seconds > 0.0).then_some(snapshot.duration_seconds);
        }
        self.last_snapshot = Some(snapshot.clone());
        Ok(())
    }

    fn publish_display(&self, snapshot: &MediaTrackSnapshot) -> Result<()> {
        let updater = self.controls.DisplayUpdater()?;
        updater.SetType(MediaPlaybackType::Music)?;
        updater.SetAppMediaId(&HSTRING::from(
            snapshot
                .source_path
                .as_ref()
                .and_then(|path| path.to_str())
                .unwrap_or("flutzplayer"),
        ))?;
        let music = updater.MusicProperties()?;
        set_music_properties(&music, snapshot)?;
        updater.Update()
    }

    fn publish_timeline(&self, snapshot: &MediaTrackSnapshot) -> Result<()> {
        let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
        let start = TimeSpan { Duration: 0 };
        let end = timespan_from_seconds(snapshot.duration_seconds.max(0.0));
        let position = timespan_from_seconds(snapshot.position_seconds.max(0.0));
        timeline.SetStartTime(start)?;
        timeline.SetMinSeekTime(start)?;
        timeline.SetEndTime(end)?;
        timeline.SetMaxSeekTime(end)?;
        timeline.SetPosition(position)?;
        self.controls.UpdateTimelineProperties(&timeline)
    }
}

impl MediaControlsBackend for WindowsMediaControlsBackend {
    fn poll_command(&mut self) -> Option<MediaControlCommand> {
        self.commands.pop()
    }

    fn publish_snapshot(&mut self, snapshot: &MediaTrackSnapshot) {
        let _ = self.publish_snapshot_inner(snapshot);
    }

    fn set_command_waker(&mut self, waker: MediaCommandWaker) {
        self.commands.set_waker(waker);
    }

    fn shutdown(&mut self) {
        if let Some(token) = self.button_pressed_token.take() {
            let _ = self.controls.RemoveButtonPressed(token);
        }
        if let Some(token) = self.position_requested_token.take() {
            let _ = self.controls.RemovePlaybackPositionChangeRequested(token);
        }
        if let Some(relay_server) = self.relay_server.take() {
            relay_server.shutdown();
        }
        let _ = self
            .controls
            .SetPlaybackStatus(WindowsPlaybackStatus::Stopped);
        if let Ok(updater) = self.controls.DisplayUpdater() {
            let _ = updater.ClearAll();
            let _ = updater.Update();
        }
        let _ = self.controls.SetIsEnabled(false);
    }
}

impl Drop for WindowsMediaControlsBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn set_music_properties(
    music: &MusicDisplayProperties,
    snapshot: &MediaTrackSnapshot,
) -> Result<()> {
    music.SetTitle(&HSTRING::from(snapshot.title.as_str()))?;
    music.SetArtist(&HSTRING::from(snapshot.artist.as_deref().unwrap_or("")))?;
    music.SetAlbumTitle(&HSTRING::from(snapshot.album.as_deref().unwrap_or("")))?;
    music.SetAlbumArtist(&HSTRING::from(snapshot.artist.as_deref().unwrap_or("")))?;
    Ok(())
}

fn command_from_button(button: SystemMediaTransportControlsButton) -> Option<MediaControlCommand> {
    match button {
        value if value == SystemMediaTransportControlsButton::Play => {
            Some(MediaControlCommand::Play)
        }
        value if value == SystemMediaTransportControlsButton::Pause => {
            Some(MediaControlCommand::Pause)
        }
        value if value == SystemMediaTransportControlsButton::Stop => {
            Some(MediaControlCommand::Stop)
        }
        value if value == SystemMediaTransportControlsButton::Next => {
            Some(MediaControlCommand::NextTrack)
        }
        value if value == SystemMediaTransportControlsButton::Previous => {
            Some(MediaControlCommand::PreviousTrack)
        }
        _ => None,
    }
}

fn windows_playback_status(status: MediaPlaybackStatus) -> WindowsPlaybackStatus {
    match status {
        MediaPlaybackStatus::Stopped => WindowsPlaybackStatus::Stopped,
        MediaPlaybackStatus::Playing => WindowsPlaybackStatus::Playing,
        MediaPlaybackStatus::Paused => WindowsPlaybackStatus::Paused,
    }
}

fn timespan_from_seconds(seconds: f64) -> TimeSpan {
    TimeSpan {
        Duration: (seconds * WINDOWS_TIMESPAN_TICKS_PER_SECOND).round() as i64,
    }
}

fn seconds_from_timespan(value: TimeSpan) -> f64 {
    value.Duration as f64 / WINDOWS_TIMESPAN_TICKS_PER_SECOND
}
