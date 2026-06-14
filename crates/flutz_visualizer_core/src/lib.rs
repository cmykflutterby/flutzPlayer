use std::sync::Arc;

use rustfft::{num_complex::Complex, Fft, FftPlanner};

pub const VIS_BAND_COUNT_TARGET: usize = 30;
pub const VIS_FREQ_MIN_HZ: f32 = 20.0;
pub const VIS_FREQ_MAX_HZ: f32 = 20_000.0;
pub const VIS_FFT_SIZE: usize = 2048;
pub const VIS_DB_FLOOR: f32 = -72.0;
pub const VIS_DB_CEILING: f32 = -6.0;
pub const VIS_NOISE_GATE_DB: f32 = -78.0;
pub const VIS_COLUMN_ATTACK_PER_SEC: f32 = 18.0;
pub const VIS_COLUMN_FALL_PER_SEC: f32 = 7.0;
pub const VIS_PEAK_FALL_PER_SEC: f32 = 0.65;
pub const VIS_ANALYSIS_UPDATE_HZ: f32 = 60.0;
pub const VIS_UI_INTERPOLATION_ENABLED: bool = true;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VisualizerBandLayout {
    ThirdOctave,
}

impl Default for VisualizerBandLayout {
    fn default() -> Self {
        Self::ThirdOctave
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VisualizerAnalysisMode {
    Spectrum,
}

impl Default for VisualizerAnalysisMode {
    fn default() -> Self {
        Self::Spectrum
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VisualizerWindowFunction {
    Hann,
}

impl Default for VisualizerWindowFunction {
    fn default() -> Self {
        Self::Hann
    }
}

#[derive(Debug, Clone)]
pub struct VisualizerAnalyzerConfig {
    pub sample_rate_hz: u32,
    pub analysis_mode: VisualizerAnalysisMode,
    pub band_layout: VisualizerBandLayout,
    pub fft_size: usize,
    pub window_function: VisualizerWindowFunction,
    pub db_floor: f32,
    pub db_ceiling: f32,
    pub noise_gate_db: f32,
    pub column_attack_per_sec: f32,
    pub column_fall_per_sec: f32,
    pub peak_fall_per_sec: f32,
    pub analysis_update_hz: f32,
    pub ui_interpolation_enabled: bool,
}

impl Default for VisualizerAnalyzerConfig {
    fn default() -> Self {
        Self {
            sample_rate_hz: 44_100,
            analysis_mode: VisualizerAnalysisMode::Spectrum,
            band_layout: VisualizerBandLayout::ThirdOctave,
            fft_size: VIS_FFT_SIZE,
            window_function: VisualizerWindowFunction::Hann,
            db_floor: VIS_DB_FLOOR,
            db_ceiling: VIS_DB_CEILING,
            noise_gate_db: VIS_NOISE_GATE_DB,
            column_attack_per_sec: VIS_COLUMN_ATTACK_PER_SEC,
            column_fall_per_sec: VIS_COLUMN_FALL_PER_SEC,
            peak_fall_per_sec: VIS_PEAK_FALL_PER_SEC,
            analysis_update_hz: VIS_ANALYSIS_UPDATE_HZ,
            ui_interpolation_enabled: VIS_UI_INTERPOLATION_ENABLED,
        }
    }
}

impl VisualizerAnalyzerConfig {
    pub fn with_sample_rate(sample_rate_hz: u32) -> Self {
        Self {
            sample_rate_hz,
            ..Self::default()
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct VisualizerBandDefinition {
    pub band_index: usize,
    pub lower_hz: f32,
    pub center_hz: f32,
    pub upper_hz: f32,
}

#[derive(Debug, Copy, Clone, Default)]
pub struct VisualizerBandState {
    pub live_level_norm: f32,
    pub column_level_norm: f32,
    pub peak_square_level_norm: f32,
    pub peak_hold_level_norm: f32,
    pub peak_hold_remaining_sec: f32,
}

#[derive(Debug, Copy, Clone, Default)]
pub struct VisualizerBandFrame {
    pub definition: VisualizerBandDefinition,
    pub state: VisualizerBandState,
}

#[derive(Debug, Clone, Default)]
pub struct VisualizerFrame {
    pub sequence: u64,
    pub timestamp_seconds: f64,
    pub sample_rate_hz: u32,
    pub fft_size: usize,
    pub bands: Vec<VisualizerBandFrame>,
    pub aggregate_peak: f32,
    pub aggregate_rms: f32,
}

impl VisualizerFrame {
    pub fn band_count(&self) -> usize {
        self.bands.len()
    }

    pub fn dominant_band_index(&self) -> Option<usize> {
        self.bands
            .iter()
            .max_by(|left, right| {
                left.state
                    .live_level_norm
                    .total_cmp(&right.state.live_level_norm)
            })
            .map(|band| band.definition.band_index)
    }
}

pub struct VisualizerAnalyzer {
    config: VisualizerAnalyzerConfig,
    bands: Vec<VisualizerBandDefinition>,
    states: Vec<VisualizerBandState>,
    sample_buffer: Vec<f32>,
    window: Vec<f32>,
    fft_buffer: Vec<Complex<f32>>,
    fft: Arc<dyn Fft<f32>>,
    write_pos: usize,
    samples_seen: usize,
    elapsed_seconds: f64,
    sequence: u64,
    latest_frame: VisualizerFrame,
}

impl VisualizerAnalyzer {
    pub fn new(config: VisualizerAnalyzerConfig) -> Self {
        let fft_size = config.fft_size.max(64).next_power_of_two();
        let mut config = config;
        config.fft_size = fft_size;
        let bands = generate_band_definitions(config.band_layout);
        let states = vec![VisualizerBandState::default(); bands.len()];
        let window = build_window(config.window_function, fft_size);
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let mut analyzer = Self {
            config,
            bands,
            states,
            sample_buffer: vec![0.0; fft_size],
            window,
            fft_buffer: vec![Complex::default(); fft_size],
            fft,
            write_pos: 0,
            samples_seen: 0,
            elapsed_seconds: 0.0,
            sequence: 0,
            latest_frame: VisualizerFrame::default(),
        };
        analyzer.publish_frame(0.0, 0.0);
        analyzer
    }

    pub fn config(&self) -> &VisualizerAnalyzerConfig {
        &self.config
    }

    pub fn band_definitions(&self) -> &[VisualizerBandDefinition] {
        &self.bands
    }

    pub fn states(&self) -> &[VisualizerBandState] {
        &self.states
    }

    pub fn latest_frame(&self) -> VisualizerFrame {
        self.latest_frame.clone()
    }

    pub fn ingest_mono(&mut self, samples: &[f32], delta_time_seconds: f32) -> VisualizerFrame {
        let mut peak = 0.0f32;
        let mut sum_squares = 0.0f64;
        for &sample in samples {
            let sample = sample.clamp(-1.0, 1.0);
            peak = peak.max(sample.abs());
            sum_squares += f64::from(sample * sample);
            self.push_sample(sample);
        }
        let rms = if samples.is_empty() {
            0.0
        } else {
            (sum_squares / samples.len() as f64).sqrt() as f32
        };
        self.update(delta_time_seconds, peak, rms)
    }

    pub fn ingest_interleaved_stereo(
        &mut self,
        samples: &[f32],
        delta_time_seconds: f32,
    ) -> VisualizerFrame {
        self.ingest_interleaved(samples, 2, delta_time_seconds)
    }

    pub fn ingest_interleaved(
        &mut self,
        samples: &[f32],
        channels: usize,
        delta_time_seconds: f32,
    ) -> VisualizerFrame {
        let channels = channels.max(1);
        let mut peak = 0.0f32;
        let mut sum_squares = 0.0f64;
        for frame in samples.chunks_exact(channels) {
            let mut mono = 0.0f32;
            for &sample in frame {
                let sample = sample.clamp(-1.0, 1.0);
                peak = peak.max(sample.abs());
                sum_squares += f64::from(sample * sample);
                mono += sample;
            }
            self.push_sample(mono / channels as f32);
        }
        let rms = if samples.is_empty() {
            0.0
        } else {
            (sum_squares / samples.len() as f64).sqrt() as f32
        };
        self.update(delta_time_seconds, peak, rms)
    }

    pub fn advance_silence(
        &mut self,
        frame_count: usize,
        delta_time_seconds: f32,
    ) -> VisualizerFrame {
        for _ in 0..frame_count {
            self.push_sample(0.0);
        }
        self.update(delta_time_seconds, 0.0, 0.0)
    }

    pub fn reset_to_silence(&mut self) -> VisualizerFrame {
        self.sample_buffer.fill(0.0);
        self.write_pos = 0;
        self.samples_seen = 0;
        for state in &mut self.states {
            *state = VisualizerBandState::default();
        }
        self.publish_frame(0.0, 0.0);
        self.latest_frame()
    }

    fn push_sample(&mut self, sample: f32) {
        self.sample_buffer[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) % self.sample_buffer.len();
        self.samples_seen = self.samples_seen.saturating_add(1);
    }

    fn update(
        &mut self,
        delta_time_seconds: f32,
        aggregate_peak: f32,
        aggregate_rms: f32,
    ) -> VisualizerFrame {
        let delta_time_seconds = delta_time_seconds.max(0.0);
        self.elapsed_seconds += f64::from(delta_time_seconds);
        let live_levels = self.measure_band_levels();
        for (state, live_level) in self.states.iter_mut().zip(live_levels) {
            state.live_level_norm = live_level;
            let column_rate = if live_level >= state.column_level_norm {
                self.config.column_attack_per_sec
            } else {
                self.config.column_fall_per_sec
            };
            state.column_level_norm = move_towards(
                state.column_level_norm,
                live_level,
                column_rate * delta_time_seconds,
            );
            if live_level > state.peak_square_level_norm {
                state.peak_square_level_norm = live_level;
            } else {
                state.peak_square_level_norm = (state.peak_square_level_norm
                    - self.config.peak_fall_per_sec * delta_time_seconds)
                    .max(live_level);
            }
            state.peak_hold_level_norm = state.peak_hold_level_norm.max(live_level).clamp(0.0, 1.0);
            state.peak_hold_remaining_sec = state.peak_hold_remaining_sec.max(0.0);
            state.live_level_norm = state.live_level_norm.clamp(0.0, 1.0);
            state.column_level_norm = state.column_level_norm.clamp(0.0, 1.0);
            state.peak_square_level_norm = state.peak_square_level_norm.clamp(0.0, 1.0);
        }
        self.publish_frame(aggregate_peak, aggregate_rms);
        self.latest_frame()
    }

    fn measure_band_levels(&mut self) -> Vec<f32> {
        let fft_size = self.config.fft_size;
        let valid_samples = self.samples_seen.min(fft_size);
        let zero_prefix = fft_size.saturating_sub(valid_samples);
        for index in 0..fft_size {
            let sample = if index < zero_prefix {
                0.0
            } else {
                let recent_index = index - zero_prefix;
                let source_index = if self.samples_seen < fft_size {
                    recent_index
                } else {
                    (self.write_pos + recent_index) % fft_size
                };
                self.sample_buffer[source_index]
            };
            self.fft_buffer[index] = Complex::new(sample * self.window[index], 0.0);
        }
        self.fft.process(&mut self.fft_buffer);

        let nyquist_bin = fft_size / 2;
        let bin_hz = self.config.sample_rate_hz as f32 / fft_size as f32;
        self.bands
            .iter()
            .map(|band| {
                let first_bin = (band.lower_hz / bin_hz).floor().max(1.0) as usize;
                let last_bin = (band.upper_hz / bin_hz).ceil().min(nyquist_bin as f32) as usize;
                if first_bin > last_bin || first_bin > nyquist_bin {
                    return 0.0;
                }
                let mut sum = 0.0f32;
                let mut count = 0usize;
                for bin in first_bin..=last_bin {
                    let magnitude = self.fft_buffer[bin].norm() / (fft_size as f32 * 0.5);
                    sum += magnitude * magnitude;
                    count += 1;
                }
                if count == 0 {
                    return 0.0;
                }
                let rms = (sum / count as f32).sqrt();
                let db = 20.0 * rms.max(1.0e-9).log10();
                if db <= self.config.noise_gate_db {
                    0.0
                } else {
                    ((db - self.config.db_floor) / (self.config.db_ceiling - self.config.db_floor))
                        .clamp(0.0, 1.0)
                }
            })
            .collect()
    }

    fn publish_frame(&mut self, aggregate_peak: f32, aggregate_rms: f32) {
        self.sequence = self.sequence.saturating_add(1);
        let bands = self
            .bands
            .iter()
            .copied()
            .zip(self.states.iter().copied())
            .map(|(definition, state)| VisualizerBandFrame { definition, state })
            .collect();
        self.latest_frame = VisualizerFrame {
            sequence: self.sequence,
            timestamp_seconds: self.elapsed_seconds,
            sample_rate_hz: self.config.sample_rate_hz,
            fft_size: self.config.fft_size,
            bands,
            aggregate_peak,
            aggregate_rms,
        };
    }
}

impl Default for VisualizerAnalyzer {
    fn default() -> Self {
        Self::new(VisualizerAnalyzerConfig::default())
    }
}

pub fn generate_band_definitions(layout: VisualizerBandLayout) -> Vec<VisualizerBandDefinition> {
    match layout {
        VisualizerBandLayout::ThirdOctave => third_octave_bands(),
    }
}

fn third_octave_bands() -> Vec<VisualizerBandDefinition> {
    let centers = [
        25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0,
        630.0, 800.0, 1_000.0, 1_250.0, 1_600.0, 2_000.0, 2_500.0, 3_150.0, 4_000.0, 5_000.0,
        6_300.0, 8_000.0, 10_000.0, 12_500.0, 16_000.0, 20_000.0,
    ];
    let half_step = 2.0f32.powf(1.0 / 6.0);
    centers
        .iter()
        .enumerate()
        .map(|(band_index, &center_hz)| VisualizerBandDefinition {
            band_index,
            lower_hz: (center_hz / half_step).max(VIS_FREQ_MIN_HZ),
            center_hz,
            upper_hz: (center_hz * half_step).min(VIS_FREQ_MAX_HZ),
        })
        .collect()
}

fn build_window(window_function: VisualizerWindowFunction, fft_size: usize) -> Vec<f32> {
    match window_function {
        VisualizerWindowFunction::Hann => (0..fft_size)
            .map(|index| {
                let phase = 2.0 * std::f32::consts::PI * index as f32 / fft_size as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect(),
    }
}

fn move_towards(current: f32, target: f32, max_delta: f32) -> f32 {
    if (target - current).abs() <= max_delta {
        target
    } else if target > current {
        current + max_delta
    } else {
        current - max_delta
    }
}
