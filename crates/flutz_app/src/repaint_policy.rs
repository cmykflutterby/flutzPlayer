//! Repaint cadence for the egui update loop.
//!
//! `FlutzDesktopApp::update` doubles as the app's event loop (playlist
//! advance, background job collection, media commands), so the cadence only
//! drops where those duties are either woken explicitly or tolerate the delay.

use std::{sync::Arc, time::Duration};

/// Callback that asks the UI thread to run another update as soon as possible.
pub type UiWaker = Arc<dyn Fn() + Send + Sync>;

pub const ACTIVE_REPAINT_INTERVAL: Duration = Duration::from_millis(16);
pub const PLAYING_REPAINT_INTERVAL: Duration = Duration::from_millis(33);
pub const BACKGROUND_WORK_REPAINT_INTERVAL: Duration = Duration::from_millis(100);
pub const IDLE_REPAINT_INTERVAL: Duration = Duration::from_millis(250);
pub const TRACK_END_FAST_REPAINT_WINDOW_SECONDS: f64 = 1.0;

const METER_DECAY_PER_60HZ_FRAME: f32 = 0.82;
const MAX_METER_DECAY_STEP_SECONDS: f32 = 1.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RepaintInputs {
    pub playing: bool,
    /// `None` when the track loops or its duration is unknown.
    pub seconds_until_track_end: Option<f64>,
    pub background_work_pending: bool,
    pub minimized: bool,
    pub frame_tracing_enabled: bool,
}

pub fn repaint_interval(inputs: RepaintInputs) -> Duration {
    if inputs.frame_tracing_enabled {
        return ACTIVE_REPAINT_INTERVAL;
    }
    if inputs.playing
        && inputs
            .seconds_until_track_end
            .is_some_and(|seconds| seconds <= TRACK_END_FAST_REPAINT_WINDOW_SECONDS)
    {
        return ACTIVE_REPAINT_INTERVAL;
    }
    if inputs.playing && !inputs.minimized {
        return PLAYING_REPAINT_INTERVAL;
    }
    if inputs.background_work_pending {
        return BACKGROUND_WORK_REPAINT_INTERVAL;
    }
    IDLE_REPAINT_INTERVAL
}

/// Meter falloff multiplier for `elapsed_seconds`; equals the historical
/// per-frame 0.82 at 60 fps.
pub fn meter_decay_factor(elapsed_seconds: f32) -> f32 {
    let elapsed = if elapsed_seconds.is_finite() {
        elapsed_seconds.clamp(0.0, MAX_METER_DECAY_STEP_SECONDS)
    } else {
        0.0
    };
    METER_DECAY_PER_60HZ_FRAME.powf(elapsed * 60.0)
}
