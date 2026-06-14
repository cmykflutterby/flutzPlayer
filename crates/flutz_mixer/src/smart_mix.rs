#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SmartMixSettings {
    pub enabled: bool,
    pub target_headroom: f64,
    pub attack: f64,
    pub release: f64,
    pub lookahead: f64,
}

impl Default for SmartMixSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            target_headroom: 0.75,
            attack: 0.124_562,
            release: 0.120_603,
            lookahead: 0.210_526,
        }
    }
}

impl SmartMixSettings {
    pub fn target_peak(self) -> f32 {
        (1.0 - self.target_headroom.clamp(0.0, 0.95) as f32).max(0.05)
    }

    pub fn reduction_gain(self, peak: f32, rms: f32) -> f32 {
        self.reduction_gain_balanced(peak, rms, 1.0, 1.0)
    }

    pub fn reduction_gain_balanced(
        self,
        peak: f32,
        rms: f32,
        desired_share: f32,
        actual_share: f32,
    ) -> f32 {
        if !self.enabled {
            return 1.0;
        }

        let target_peak = self.target_peak();
        let peak_gain = if peak > target_peak && peak > f32::EPSILON {
            target_peak / peak
        } else {
            1.0
        };

        if desired_share <= f32::EPSILON || actual_share <= f32::EPSILON {
            return peak_gain;
        }

        let dominance = (actual_share / desired_share).max(1.0);
        let energy_bias = (rms / peak).clamp(0.0, 1.0);
        let strength = (0.5 + energy_bias * 0.5) * self.attack.clamp(0.0, 1.0) as f32;
        let balance_gain = dominance.powf(-strength);

        balance_gain.min(peak_gain).clamp(0.05, 1.0)
    }

    pub fn lookahead_blocks(self) -> usize {
        (self.lookahead.clamp(0.0, 1.0) * 16.0).round() as usize
    }

    pub fn smooth_gain(self, current: f32, target: f32) -> f32 {
        let coefficient = if target < current {
            self.attack
        } else {
            self.release
        }
        .clamp(0.0, 1.0) as f32;
        current + (target - current) * coefficient
    }
}
