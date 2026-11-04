use super::config::DEFAULT_TOLERANCE;
use super::*;

#[derive(Debug, Clone)]
pub(super) struct RuntimeBand {
    config: PeqBandConfig,
    q_value: f32,
    shelf_slope: f32,
    sample_rate_hz: u32,
    pub(super) current_gain_db: f32,
    target_gain_db: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
    pub(super) current_coefficients: BiquadCoefficients,
    channel_states: Vec<BiquadState>,
}

impl RuntimeBand {
    pub(super) fn new(
        sample_rate_hz: u32,
        channel_count: usize,
        config: PeqBandConfig,
    ) -> Result<Self> {
        let q_value = config.bandwidth.q_value();
        let shelf_slope = config.bandwidth.shelf_slope();
        let current_coefficients = design_coefficients(
            sample_rate_hz,
            config.filter_type,
            config.frequency_hz,
            config.gain_db,
            q_value,
            shelf_slope,
            config.enabled,
        );
        Ok(Self {
            q_value,
            shelf_slope,
            sample_rate_hz,
            current_gain_db: config.gain_db,
            target_gain_db: config.gain_db,
            attack_coefficient: smoothing_coefficient(config.attack_ms, sample_rate_hz),
            release_coefficient: smoothing_coefficient(config.release_ms, sample_rate_hz),
            current_coefficients,
            channel_states: vec![BiquadState::default(); channel_count],
            config,
        })
    }

    pub(super) fn reset_state(&mut self) {
        for state in &mut self.channel_states {
            *state = BiquadState::default();
        }
        self.current_gain_db = self.target_gain_db;
        self.current_coefficients = self.design_coefficients(self.current_gain_db);
    }

    pub(super) fn advance_for_next_sample(&mut self) {
        if !self.config.enabled {
            return;
        }
        let difference = self.target_gain_db - self.current_gain_db;
        if difference.abs() <= DEFAULT_TOLERANCE {
            self.current_gain_db = self.target_gain_db;
            return;
        }
        let coefficient = if self.target_gain_db.abs() >= self.current_gain_db.abs() {
            self.attack_coefficient
        } else {
            self.release_coefficient
        };
        self.current_gain_db =
            self.target_gain_db + ((self.current_gain_db - self.target_gain_db) * coefficient);
        self.current_coefficients = self.design_coefficients(self.current_gain_db);
    }

    pub(super) fn process_sample(&mut self, channel_index: usize, input_sample: f32) -> f32 {
        if !self.config.enabled {
            return input_sample;
        }
        let state = &mut self.channel_states[channel_index];
        self.current_coefficients.process(input_sample, state)
    }

    pub(super) fn design_coefficients(&self, gain_db: f32) -> BiquadCoefficients {
        design_coefficients(
            self.sample_rate_hz,
            self.config.filter_type,
            self.config.frequency_hz,
            gain_db,
            self.q_value,
            self.shelf_slope,
            self.config.enabled,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BiquadCoefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoefficients {
    const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn process(&self, input: f32, state: &mut BiquadState) -> f32 {
        let output = (self.b0 * input) + (self.b1 * state.x1) + (self.b2 * state.x2)
            - (self.a1 * state.y1)
            - (self.a2 * state.y2);
        state.x2 = state.x1;
        state.x1 = input;
        state.y2 = state.y1;
        state.y1 = output;
        output
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

fn design_coefficients(
    sample_rate_hz: u32,
    filter_type: PeqFilterType,
    frequency_hz: f32,
    gain_db: f32,
    q_value: f32,
    shelf_slope: f32,
    enabled: bool,
) -> BiquadCoefficients {
    if !enabled || gain_db.abs() <= DEFAULT_TOLERANCE {
        return BiquadCoefficients::IDENTITY;
    }

    let omega = 2.0 * PI * (frequency_hz / sample_rate_hz as f32);
    let sin_omega = omega.sin();
    let cos_omega = omega.cos();
    let amplitude = 10.0_f32.powf(gain_db / 40.0);

    match filter_type {
        PeqFilterType::Bell => {
            let alpha = match q_value {
                value if value.is_finite() && value > 0.0 => sin_omega / (2.0 * value),
                _ => sin_omega / 2.0,
            };
            normalize_coefficients(
                1.0 + (alpha * amplitude),
                -2.0 * cos_omega,
                1.0 - (alpha * amplitude),
                1.0 + (alpha / amplitude),
                -2.0 * cos_omega,
                1.0 - (alpha / amplitude),
            )
        }
        PeqFilterType::LowShelf => {
            let alpha = shelf_alpha(sin_omega, amplitude, shelf_slope);
            let double_root = 2.0 * amplitude.sqrt() * alpha;
            normalize_coefficients(
                amplitude * ((amplitude + 1.0) - ((amplitude - 1.0) * cos_omega) + double_root),
                2.0 * amplitude * ((amplitude - 1.0) - ((amplitude + 1.0) * cos_omega)),
                amplitude * ((amplitude + 1.0) - ((amplitude - 1.0) * cos_omega) - double_root),
                (amplitude + 1.0) + ((amplitude - 1.0) * cos_omega) + double_root,
                -2.0 * ((amplitude - 1.0) + ((amplitude + 1.0) * cos_omega)),
                (amplitude + 1.0) + ((amplitude - 1.0) * cos_omega) - double_root,
            )
        }
        PeqFilterType::HighShelf => {
            let alpha = shelf_alpha(sin_omega, amplitude, shelf_slope);
            let double_root = 2.0 * amplitude.sqrt() * alpha;
            normalize_coefficients(
                amplitude * ((amplitude + 1.0) + ((amplitude - 1.0) * cos_omega) + double_root),
                -2.0 * amplitude * ((amplitude - 1.0) + ((amplitude + 1.0) * cos_omega)),
                amplitude * ((amplitude + 1.0) + ((amplitude - 1.0) * cos_omega) - double_root),
                (amplitude + 1.0) - ((amplitude - 1.0) * cos_omega) + double_root,
                2.0 * ((amplitude - 1.0) - ((amplitude + 1.0) * cos_omega)),
                (amplitude + 1.0) - ((amplitude - 1.0) * cos_omega) - double_root,
            )
        }
    }
}

fn normalize_coefficients(
    b0: f32,
    b1: f32,
    b2: f32,
    a0: f32,
    a1: f32,
    a2: f32,
) -> BiquadCoefficients {
    if a0.abs() <= DEFAULT_TOLERANCE {
        return BiquadCoefficients::IDENTITY;
    }
    BiquadCoefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

fn shelf_alpha(sin_omega: f32, amplitude: f32, shelf_slope: f32) -> f32 {
    let slope = shelf_slope.max(0.0001);
    let inner = ((amplitude + (1.0 / amplitude)) * ((1.0 / slope) - 1.0) + 2.0).max(0.0);
    (sin_omega * 0.5) * inner.sqrt()
}

fn smoothing_coefficient(duration_ms: f32, sample_rate_hz: u32) -> f32 {
    if duration_ms <= DEFAULT_TOLERANCE {
        return 0.0;
    }
    let duration_seconds = duration_ms / 1_000.0;
    (-1.0 / (duration_seconds * sample_rate_hz as f32)).exp()
}

pub(super) fn octave_bandwidth_to_q(octaves: f32) -> f32 {
    let clamped = octaves.max(0.01);
    let exponent = 2.0_f32.powf(clamped * 0.5);
    let ratio = (exponent - (1.0 / exponent)).max(DEFAULT_TOLERANCE);
    1.0 / ratio
}

pub(super) fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
