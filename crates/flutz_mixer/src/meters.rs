#[derive(Debug, Copy, Clone, Default, PartialEq)]
pub struct MeterReading {
    pub peak: f32,
    pub rms: f32,
    pub headroom: f32,
}

impl MeterReading {
    pub fn from_interleaved(samples: &[f32]) -> Self {
        if samples.is_empty() {
            return Self {
                peak: 0.0,
                rms: 0.0,
                headroom: 1.0,
            };
        }

        let mut peak = 0.0f32;
        let mut sum_square = 0.0f64;
        for sample in samples {
            let magnitude = sample.abs();
            peak = peak.max(magnitude);
            sum_square += (*sample as f64) * (*sample as f64);
        }

        let rms = (sum_square / samples.len() as f64).sqrt() as f32;
        Self {
            peak,
            rms,
            headroom: (1.0 - peak).max(0.0),
        }
    }
}
