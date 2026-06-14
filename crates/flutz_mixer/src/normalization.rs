#[derive(Debug, Copy, Clone, PartialEq)]
pub struct AutoNormalization {
    pub enabled: bool,
    pub amount: f64,
}

impl Default for AutoNormalization {
    fn default() -> Self {
        Self {
            enabled: false,
            amount: 0.25,
        }
    }
}

impl AutoNormalization {
    pub fn gain(self, source_peak: f32, target_peak: f32) -> f32 {
        if !self.enabled || source_peak <= f32::EPSILON {
            return 1.0;
        }
        let raw_gain = target_peak / source_peak;
        let amount = self.amount.clamp(0.0, 1.0) as f32;
        1.0 + (raw_gain - 1.0) * amount
    }
}
