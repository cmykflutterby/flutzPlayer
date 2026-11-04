use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct StripRuntimeState {
    pub(super) smoothed_smart_mix_gain: f32,
    pub(super) previous_left_gain: f32,
    pub(super) previous_right_gain: f32,
    pub(super) gain_initialized: bool,
    pub(super) lookahead: VecDeque<f32>,
    pub(super) reverb: DelayEffectState,
    pub(super) chorus: DelayEffectState,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PreparedStrip {
    pub(super) rendered_frame_range: Range<usize>,
    pub(super) combined_meter: MeterReading,
    pub(super) session_gain: f32,
    pub(super) session_excluded: bool,
    pub(super) processed: bool,
}

impl Default for StripRuntimeState {
    fn default() -> Self {
        Self {
            smoothed_smart_mix_gain: 1.0,
            previous_left_gain: 1.0,
            previous_right_gain: 1.0,
            gain_initialized: false,
            lookahead: VecDeque::new(),
            reverb: DelayEffectState::new(12_000),
            chorus: DelayEffectState::new(4_800),
        }
    }
}

impl StripRuntimeState {
    pub(super) fn previous_or_initialize_gains(
        &mut self,
        target_left_gain: f32,
        target_right_gain: f32,
    ) -> (f32, f32) {
        if !self.gain_initialized {
            self.previous_left_gain = target_left_gain;
            self.previous_right_gain = target_right_gain;
            self.gain_initialized = true;
        }
        (self.previous_left_gain, self.previous_right_gain)
    }

    pub(super) fn lookahead_gain(&mut self, gain: f32, lookahead_blocks: usize) -> f32 {
        if lookahead_blocks == 0 {
            self.lookahead.clear();
            return gain;
        }

        self.lookahead.push_back(gain);
        while self.lookahead.len() > lookahead_blocks + 1 {
            self.lookahead.pop_front();
        }
        self.lookahead.iter().copied().fold(1.0, f32::min)
    }

    pub(super) fn has_effect_tail(&self, controls: MixerStripControls) -> bool {
        (controls.reverb > f64::EPSILON && self.reverb.has_signal())
            || (controls.chorus > f64::EPSILON && self.chorus.has_signal())
    }
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MasterRuntimeState {
    pub(super) eq: ThreeBandEqState,
    pub(super) reverb: DelayEffectState,
    pub(super) chorus: DelayEffectState,
}

impl Default for MasterRuntimeState {
    fn default() -> Self {
        Self {
            eq: ThreeBandEqState::default(),
            reverb: DelayEffectState::new(24_000),
            chorus: DelayEffectState::new(7_200),
        }
    }
}
