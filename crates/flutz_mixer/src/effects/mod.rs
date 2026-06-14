#[derive(Debug, Copy, Clone, PartialEq)]
pub struct LimiterControls {
    pub enabled: bool,
    pub amount: f64,
    pub release: f64,
}

impl Default for LimiterControls {
    fn default() -> Self {
        Self {
            enabled: false,
            amount: 0.25,
            release: 0.191_919,
        }
    }
}

impl LimiterControls {
    pub fn threshold(self) -> f32 {
        (1.0 - self.amount.clamp(0.0, 0.95) as f32).max(0.05)
    }

    pub fn apply(self, sample: f32) -> f32 {
        if !self.enabled {
            return sample;
        }
        let threshold = self.threshold();
        if sample.abs() <= threshold {
            return sample;
        }
        sample.signum() * threshold
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThreeBandEqState {
    left: EqChannelState,
    right: EqChannelState,
}

impl Default for ThreeBandEqState {
    fn default() -> Self {
        Self {
            left: EqChannelState::default(),
            right: EqChannelState::default(),
        }
    }
}

impl ThreeBandEqState {
    pub(crate) fn process(
        &mut self,
        left: f32,
        right: f32,
        low_gain: f32,
        mid_gain: f32,
        high_gain: f32,
    ) -> (f32, f32) {
        (
            self.left.process(left, low_gain, mid_gain, high_gain),
            self.right.process(right, low_gain, mid_gain, high_gain),
        )
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
struct EqChannelState {
    low: f32,
    high: f32,
    previous_input: f32,
}

impl Default for EqChannelState {
    fn default() -> Self {
        Self {
            low: 0.0,
            high: 0.0,
            previous_input: 0.0,
        }
    }
}

impl EqChannelState {
    fn process(&mut self, input: f32, low_gain: f32, mid_gain: f32, high_gain: f32) -> f32 {
        self.low += 0.04 * (input - self.low);
        self.high = 0.38 * (self.high + input - self.previous_input);
        self.previous_input = input;
        let mid = input - self.low - self.high;
        self.low * low_gain + mid * mid_gain + self.high * high_gain
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DelayEffectState {
    left: Vec<f32>,
    right: Vec<f32>,
    write_index: usize,
    chorus_phase: f32,
}

impl DelayEffectState {
    pub(crate) fn new(delay_frames: usize) -> Self {
        let frame_count = delay_frames.max(1);
        Self {
            left: vec![0.0; frame_count],
            right: vec![0.0; frame_count],
            write_index: 0,
            chorus_phase: 0.0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.write_index = 0;
        self.chorus_phase = 0.0;
    }

    pub(crate) fn has_signal(&self) -> bool {
        self.left
            .iter()
            .chain(self.right.iter())
            .any(|sample| sample.abs() > f32::EPSILON)
    }

    pub(crate) fn process_reverb(&mut self, left: f32, right: f32, amount: f64) -> (f32, f32) {
        let wet = (amount.clamp(0.0, 100.0) / 100.0) as f32 * 1.35;
        if wet <= f32::EPSILON {
            return (left, right);
        }

        let len = self.left.len();
        if len < 8 {
            let delayed_left = self.left[self.write_index];
            let delayed_right = self.right[self.write_index];
            self.left[self.write_index] = left + delayed_left * 0.42;
            self.right[self.write_index] = right + delayed_right * 0.42;
            self.advance();
            return (left + delayed_left * wet, right + delayed_right * wet);
        }

        let tap1 = (len * 7 / 53).clamp(1, len - 1);
        let tap2 = (len * 13 / 53).clamp(1, len - 1);
        let tap3 = (len * 23 / 53).clamp(1, len - 1);
        let tap4 = (len * 37 / 53).clamp(1, len - 1);

        let read_tap = |delay: usize| (self.write_index + len - delay) % len;
        let t1 = read_tap(tap1);
        let t2 = read_tap(tap2);
        let t3 = read_tap(tap3);
        let t4 = read_tap(tap4);

        let l1 = self.left[t1];
        let l2 = self.left[t2];
        let l3 = self.left[t3];
        let l4 = self.left[t4];
        let r1 = self.right[t1];
        let r2 = self.right[t2];
        let r3 = self.right[t3];
        let r4 = self.right[t4];

        let diffuse_left = l1 * 0.42 + l2 * 0.27 + l3 * 0.19 + l4 * 0.12;
        let diffuse_right = r1 * 0.42 + r2 * 0.27 + r3 * 0.19 + r4 * 0.12;

        self.left[self.write_index] = left + diffuse_left * 0.55 + r2 * 0.12;
        self.right[self.write_index] = right + diffuse_right * 0.55 + l2 * 0.12;
        self.advance();

        (
            left * (1.0 - wet * 0.72).max(0.0) + diffuse_left * wet,
            right * (1.0 - wet * 0.72).max(0.0) + diffuse_right * wet,
        )
    }

    pub(crate) fn process_chorus(&mut self, left: f32, right: f32, amount: f64) -> (f32, f32) {
        let wet = (amount.clamp(0.0, 100.0) / 100.0) as f32 * 1.0;
        if wet <= f32::EPSILON {
            return (left, right);
        }

        let len = self.left.len().max(3);
        let max_delay = (len - 2) as f32;
        let base_delay = (len as f32 * 0.06).clamp(96.0, 640.0);
        let depth = (len as f32 * 0.025).clamp(24.0, 280.0);

        let delay_left = (base_delay + self.chorus_phase.sin() * depth).clamp(2.0, max_delay);
        let delay_right = (base_delay
            + (self.chorus_phase + std::f32::consts::FRAC_PI_2).sin() * depth)
            .clamp(2.0, max_delay);
        let delayed_left = Self::read_interpolated(&self.left, self.write_index, delay_left);
        let delayed_right = Self::read_interpolated(&self.right, self.write_index, delay_right);
        let feedback = 0.08;

        self.left[self.write_index] = left + delayed_left * feedback;
        self.right[self.write_index] = right + delayed_right * feedback;
        self.chorus_phase = (self.chorus_phase + 0.000_05).rem_euclid(std::f32::consts::TAU);
        self.advance();

        let dry_mix = (1.0 - wet * 0.78).max(0.0);
        (
            left * dry_mix + delayed_left * wet + delayed_right * wet * 0.12,
            right * dry_mix + delayed_right * wet + delayed_left * wet * 0.12,
        )
    }

    fn read_interpolated(buffer: &[f32], write_index: usize, delay_samples: f32) -> f32 {
        let len = buffer.len();
        if len < 2 {
            return buffer.first().copied().unwrap_or_default();
        }

        let position = (write_index as f32 - delay_samples).rem_euclid(len as f32);
        let index_1 = position.floor() as usize % len;
        let index_2 = (index_1 + 1) % len;
        let fraction = position - index_1 as f32;

        buffer[index_1] + (buffer[index_2] - buffer[index_1]) * fraction
    }

    fn advance(&mut self) {
        self.write_index = (self.write_index + 1) % self.left.len();
    }
}
