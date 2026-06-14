use std::f32::consts;

use flutz_core::{SoundFontId, StripId};

use crate::effects::LimiterControls;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MixerStripIdentity {
    pub strip_id: StripId,
    pub soundfont_id: SoundFontId,
    pub midi_channel: u8,
    pub midi_program: u8,
    pub is_percussion: bool,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MixerStripControls {
    pub volume: f64,
    pub mute: bool,
    pub solo: bool,
    pub pan: f64,
    pub gain_db: f64,
    pub limiter: LimiterControls,
    pub reverb: f64,
    pub chorus: f64,
}

impl Default for MixerStripControls {
    fn default() -> Self {
        Self {
            volume: 1.0,
            mute: false,
            solo: false,
            pan: 0.0,
            gain_db: 0.0,
            limiter: LimiterControls::default(),
            reverb: 0.0,
            chorus: 0.0,
        }
    }
}

impl MixerStripControls {
    pub fn base_gain(self) -> f32 {
        (self.volume.clamp(0.0, 2.5) * db_to_gain(self.gain_db)) as f32
    }

    pub fn session_excluded(self, solo_active: bool) -> bool {
        if solo_active {
            !self.solo
        } else {
            self.mute
        }
    }

    pub fn session_gain(self, solo_active: bool) -> f32 {
        if self.session_excluded(solo_active) {
            0.0
        } else {
            self.base_gain()
        }
    }

    pub fn pan_gains(self) -> (f32, f32) {
        let pan = self.pan.clamp(-1.0, 1.0) as f32;
        let angle = consts::FRAC_PI_4 * (pan + 1.0);
        let center_gain = consts::FRAC_1_SQRT_2;
        (angle.cos() / center_gain, angle.sin() / center_gain)
    }
}

pub fn db_to_gain(db: f64) -> f64 {
    10.0f64.powf(db / 20.0)
}
