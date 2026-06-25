//! Streaming parametric EQ primitives for decoded-audio mastering.
//!
//! This crate currently requires `std` for preset file I/O and owned runtime state.
//! It does not install a custom global allocator and relies on the process allocator.
//!
//! `PeqProcessor` is a single-owner processing object with no internal locking.
//! Prepare replacement configs off the realtime path with `PreparedConfig::from_config`
//! and hand them to `set_prepared_config`; the swap becomes active on the next
//! `process_*` call, which is the intended chunk boundary for live playback updates.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, error::Error, f32::consts::PI, fmt, fs, mem, path::Path};

pub type Result<T> = std::result::Result<T, PeqError>;

const MIN_FREQUENCY_HZ: f32 = 10.0;
const MIN_ATTACK_RELEASE_MS: f32 = 0.0;
const DEFAULT_TOLERANCE: f32 = 0.000_001;

fn default_sample_rate_hz() -> u32 {
    48_000
}

fn default_channel_count() -> u16 {
    2
}

#[derive(Debug)]
pub enum PeqError {
    InvalidConfig(String),
    InvalidState(String),
    BufferMismatch(String),
    Io(String),
    Parse(String),
}

impl fmt::Display for PeqError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => write!(f, "invalid PEQ config: {message}"),
            Self::InvalidState(message) => write!(f, "invalid PEQ state: {message}"),
            Self::BufferMismatch(message) => write!(f, "buffer mismatch: {message}"),
            Self::Io(message) => write!(f, "PEQ I/O error: {message}"),
            Self::Parse(message) => write!(f, "PEQ parse error: {message}"),
        }
    }
}

impl Error for PeqError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelLayout {
    Interleaved,
    Planar,
}

impl Default for ChannelLayout {
    fn default() -> Self {
        Self::Interleaved
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PeqFilterType {
    Bell,
    LowShelf,
    HighShelf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum Bandwidth {
    Q { value: f32 },
    Octaves { value: f32 },
}

impl Default for Bandwidth {
    fn default() -> Self {
        Self::Q { value: 1.0 }
    }
}

impl Bandwidth {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Q { value } => validate_positive_finite(*value, "bandwidth.q")?,
            Self::Octaves { value } => validate_positive_finite(*value, "bandwidth.octaves")?,
        }
        Ok(())
    }

    fn q_value(&self) -> f32 {
        match self {
            Self::Q { value } => value.max(0.05),
            Self::Octaves { value } => octave_bandwidth_to_q(*value),
        }
    }

    fn shelf_slope(&self) -> f32 {
        match self {
            Self::Q { value } => (1.0 / value.max(0.1)).clamp(0.1, 4.0),
            Self::Octaves { value } => (1.0 / value.max(0.25)).clamp(0.1, 4.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeqBandConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub filter_type: PeqFilterType,
    pub frequency_hz: f32,
    pub gain_db: f32,
    #[serde(default)]
    pub bandwidth: Bandwidth,
    #[serde(default)]
    pub attack_ms: f32,
    #[serde(default)]
    pub release_ms: f32,
    #[serde(flatten)]
    pub extra_fields: BTreeMap<String, toml::Value>,
}

impl Default for PeqBandConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            filter_type: PeqFilterType::Bell,
            frequency_hz: 1_000.0,
            gain_db: 0.0,
            bandwidth: Bandwidth::default(),
            attack_ms: 0.0,
            release_ms: 0.0,
            extra_fields: BTreeMap::new(),
        }
    }
}

impl PeqBandConfig {
    pub fn validate(&self, sample_rate_hz: u32) -> Result<()> {
        validate_positive_finite(self.frequency_hz, "frequency_hz")?;
        if self.frequency_hz < MIN_FREQUENCY_HZ {
            return Err(PeqError::InvalidConfig(format!(
                "band frequency {} Hz is below minimum {} Hz",
                self.frequency_hz, MIN_FREQUENCY_HZ
            )));
        }
        let nyquist = sample_rate_hz as f32 * 0.5;
        if self.frequency_hz >= nyquist {
            return Err(PeqError::InvalidConfig(format!(
                "band frequency {} Hz must be below Nyquist {} Hz",
                self.frequency_hz, nyquist
            )));
        }
        validate_finite(self.gain_db, "gain_db")?;
        if self.attack_ms < MIN_ATTACK_RELEASE_MS || !self.attack_ms.is_finite() {
            return Err(PeqError::InvalidConfig(format!(
                "attack_ms must be finite and >= {}",
                MIN_ATTACK_RELEASE_MS
            )));
        }
        if self.release_ms < MIN_ATTACK_RELEASE_MS || !self.release_ms.is_finite() {
            return Err(PeqError::InvalidConfig(format!(
                "release_ms must be finite and >= {}",
                MIN_ATTACK_RELEASE_MS
            )));
        }
        self.bandwidth.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeqConfig {
    #[serde(default = "default_sample_rate_hz", skip_serializing)]
    pub sample_rate_hz: u32,
    #[serde(default = "default_channel_count", skip_serializing)]
    pub channel_count: u16,
    #[serde(default, skip_serializing)]
    pub channel_layout: ChannelLayout,
    #[serde(default)]
    pub output_gain_db: f32,
    #[serde(default = "default_wet_mix")]
    pub wet_mix: f32,
    #[serde(default)]
    pub bands: Vec<PeqBandConfig>,
    #[serde(flatten)]
    pub extra_fields: BTreeMap<String, toml::Value>,
}

impl Default for PeqConfig {
    fn default() -> Self {
        Self {
            sample_rate_hz: 48_000,
            channel_count: 2,
            channel_layout: ChannelLayout::Interleaved,
            output_gain_db: 0.0,
            wet_mix: 1.0,
            bands: Vec::new(),
            extra_fields: BTreeMap::new(),
        }
    }
}

impl PeqConfig {
    pub fn validate(&self) -> Result<()> {
        if self.sample_rate_hz == 0 {
            return Err(PeqError::InvalidConfig(
                "sample_rate_hz must be greater than zero".to_owned(),
            ));
        }
        if self.channel_count == 0 {
            return Err(PeqError::InvalidConfig(
                "channel_count must be greater than zero".to_owned(),
            ));
        }
        validate_finite(self.output_gain_db, "output_gain_db")?;
        if !self.wet_mix.is_finite() || !(0.0..=1.0).contains(&self.wet_mix) {
            return Err(PeqError::InvalidConfig(
                "wet_mix must be finite and between 0.0 and 1.0".to_owned(),
            ));
        }
        for band in &self.bands {
            band.validate(self.sample_rate_hz)?;
        }
        Ok(())
    }

    pub fn with_band(mut self, band: PeqBandConfig) -> Self {
        self.bands.push(band);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetMetadata {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default, skip_serializing)]
    pub tags: Vec<String>,
    #[serde(flatten)]
    pub extra_fields: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeqPresetFile {
    #[serde(default)]
    pub metadata: PresetMetadata,
    pub config: PeqConfig,
    #[serde(flatten)]
    pub extra_fields: BTreeMap<String, toml::Value>,
}

impl PeqPresetFile {
    pub fn validate(&self) -> Result<()> {
        self.config.validate()
    }
}

#[derive(Debug, Clone)]
pub struct PreparedConfig {
    config: PeqConfig,
    runtime_bands: Vec<RuntimeBand>,
    estimated_state_bytes: usize,
}

impl PreparedConfig {
    /// Builds a runtime-ready config outside the audio callback or steady-state chunk path.
    pub fn from_config(config: PeqConfig) -> Result<Self> {
        config.validate()?;
        let channel_count = usize::from(config.channel_count);
        let runtime_bands = config
            .bands
            .iter()
            .cloned()
            .map(|band| RuntimeBand::new(config.sample_rate_hz, channel_count, band))
            .collect::<Result<Vec<_>>>()?;
        let estimated_state_bytes =
            runtime_bands.len() * channel_count * mem::size_of::<BiquadState>();
        Ok(Self {
            config,
            runtime_bands,
            estimated_state_bytes,
        })
    }

    pub fn config(&self) -> &PeqConfig {
        &self.config
    }

    pub fn estimated_state_bytes(&self) -> usize {
        self.estimated_state_bytes
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProcessorMetrics {
    pub active_config_generation: u64,
    pub pending_config_generation: Option<u64>,
    pub config_swaps: u64,
    pub process_calls: u64,
    pub processed_frames: u64,
    pub retained_state_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDisposition {
    Retained,
    Released,
}

#[derive(Debug, Clone)]
pub struct ProcessSummary {
    pub frames: usize,
    pub channels: usize,
    pub layout: ChannelLayout,
    pub finite_sample_count: usize,
}

#[derive(Debug, Clone)]
pub struct PeqProcessor {
    active_config: Option<PeqConfig>,
    bands: Vec<RuntimeBand>,
    pending: Option<PreparedConfig>,
    metrics: ProcessorMetrics,
    output_gain_linear: f32,
    wet_mix: f32,
}

impl Default for PeqProcessor {
    fn default() -> Self {
        Self {
            active_config: None,
            bands: Vec::new(),
            pending: None,
            metrics: ProcessorMetrics::default(),
            output_gain_linear: 1.0,
            wet_mix: 1.0,
        }
    }
}

impl PeqProcessor {
    pub fn new(prepared: PreparedConfig) -> Self {
        let mut processor = Self::default();
        processor.activate_prepared(prepared);
        processor
    }

    pub fn from_config(config: PeqConfig) -> Result<Self> {
        let prepared = PreparedConfig::from_config(config)?;
        Ok(Self::new(prepared))
    }

    pub fn prepare_config(config: PeqConfig) -> Result<PreparedConfig> {
        PreparedConfig::from_config(config)
    }

    /// Queues a config prepared off-thread/off-callback.
    ///
    /// The new runtime state is swapped in at the next `process_interleaved` or
    /// `process_planar` call so live updates take effect on the next processed chunk.
    pub fn set_prepared_config(&mut self, prepared: PreparedConfig) -> Result<()> {
        if prepared.config.channel_count == 0 {
            return Err(PeqError::InvalidConfig(
                "channel_count must be greater than zero".to_owned(),
            ));
        }
        let next_generation =
            self.metrics.active_config_generation + u64::from(self.pending.is_some()) + 1;
        self.metrics.pending_config_generation = Some(next_generation);
        self.pending = Some(prepared);
        Ok(())
    }

    /// Convenience helper that validates and prepares a config before queueing it.
    ///
    /// This method may allocate while building runtime state, so it should be called
    /// outside the audio callback or other steady-state realtime path.
    pub fn set_config(&mut self, config: PeqConfig) -> Result<()> {
        let prepared = Self::prepare_config(config)?;
        self.set_prepared_config(prepared)
    }

    pub fn active_config(&self) -> Option<&PeqConfig> {
        self.active_config.as_ref()
    }

    pub fn metrics(&self) -> &ProcessorMetrics {
        &self.metrics
    }

    pub fn reset_state(&mut self) -> Result<()> {
        if self.active_config.is_none() {
            return Err(PeqError::InvalidState(
                "cannot reset an uninitialized PEQ processor".to_owned(),
            ));
        }
        for band in &mut self.bands {
            band.reset_state();
        }
        Ok(())
    }

    pub fn release_state(&mut self) -> ReleaseDisposition {
        if self.bands.is_empty() {
            return ReleaseDisposition::Retained;
        }
        self.bands.clear();
        self.active_config = None;
        self.pending = None;
        self.output_gain_linear = 1.0;
        self.wet_mix = 1.0;
        self.metrics.retained_state_bytes = 0;
        self.metrics.pending_config_generation = None;
        ReleaseDisposition::Released
    }

    pub fn process_interleaved(
        &mut self,
        input: &[f32],
        output: &mut [f32],
    ) -> Result<ProcessSummary> {
        self.apply_pending_config();
        let config = self.ensure_active_config()?;
        if config.channel_layout != ChannelLayout::Interleaved {
            return Err(PeqError::InvalidState(
                "active config expects planar processing, not interleaved".to_owned(),
            ));
        }
        if input.len() != output.len() {
            return Err(PeqError::BufferMismatch(format!(
                "input/output sample count mismatch: {} != {}",
                input.len(),
                output.len()
            )));
        }
        let channels = usize::from(config.channel_count);
        if input.len() % channels != 0 {
            return Err(PeqError::BufferMismatch(format!(
                "interleaved input length {} is not divisible by channel count {}",
                input.len(),
                channels
            )));
        }
        let frames = input.len() / channels;
        let mut finite_sample_count = 0;
        for frame_index in 0..frames {
            self.advance_chunk_frame();
            let base = frame_index * channels;
            for channel in 0..channels {
                let sample = input[base + channel];
                let processed = self.process_sample(channel, sample);
                if processed.is_finite() {
                    finite_sample_count += 1;
                }
                output[base + channel] = processed;
            }
        }
        self.metrics.process_calls += 1;
        self.metrics.processed_frames += frames as u64;
        Ok(ProcessSummary {
            frames,
            channels,
            layout: ChannelLayout::Interleaved,
            finite_sample_count,
        })
    }

    pub fn process_planar(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
    ) -> Result<ProcessSummary> {
        self.apply_pending_config();
        let config = self.ensure_active_config()?;
        if config.channel_layout != ChannelLayout::Planar {
            return Err(PeqError::InvalidState(
                "active config expects interleaved processing, not planar".to_owned(),
            ));
        }
        let channels = usize::from(config.channel_count);
        if input.len() != channels || output.len() != channels {
            return Err(PeqError::BufferMismatch(format!(
                "planar channel slice count mismatch: input {}, output {}, channels {}",
                input.len(),
                output.len(),
                channels
            )));
        }
        let frames = input.first().map_or(0, |channel| channel.len());
        for channel in input {
            if channel.len() != frames {
                return Err(PeqError::BufferMismatch(
                    "planar input channel lengths do not match".to_owned(),
                ));
            }
        }
        for channel in output.iter() {
            if channel.len() != frames {
                return Err(PeqError::BufferMismatch(
                    "planar output channel lengths do not match input frame count".to_owned(),
                ));
            }
        }
        let mut finite_sample_count = 0;
        for frame_index in 0..frames {
            self.advance_chunk_frame();
            for channel_index in 0..channels {
                let processed =
                    self.process_sample(channel_index, input[channel_index][frame_index]);
                if processed.is_finite() {
                    finite_sample_count += 1;
                }
                output[channel_index][frame_index] = processed;
            }
        }
        self.metrics.process_calls += 1;
        self.metrics.processed_frames += frames as u64;
        Ok(ProcessSummary {
            frames,
            channels,
            layout: ChannelLayout::Planar,
            finite_sample_count,
        })
    }

    fn ensure_active_config(&self) -> Result<&PeqConfig> {
        self.active_config
            .as_ref()
            .ok_or_else(|| PeqError::InvalidState("PEQ processor has no active config".to_owned()))
    }

    fn apply_pending_config(&mut self) {
        if let Some(mut prepared) = self.pending.take() {
            for (old_band, new_band) in self.bands.iter().zip(prepared.runtime_bands.iter_mut()) {
                new_band.current_gain_db = old_band.current_gain_db;
                new_band.current_coefficients =
                    new_band.design_coefficients(new_band.current_gain_db);
            }
            self.activate_prepared(prepared);
            self.metrics.config_swaps += 1;
        }
    }

    fn activate_prepared(&mut self, prepared: PreparedConfig) {
        self.output_gain_linear = db_to_linear(prepared.config.output_gain_db);
        self.wet_mix = prepared.config.wet_mix;
        self.metrics.active_config_generation += 1;
        self.metrics.pending_config_generation = None;
        self.metrics.retained_state_bytes = prepared.estimated_state_bytes;
        self.active_config = Some(prepared.config);
        self.bands = prepared.runtime_bands;
    }

    fn advance_chunk_frame(&mut self) {
        for band in &mut self.bands {
            band.advance_for_next_sample();
        }
    }

    fn process_sample(&mut self, channel_index: usize, input_sample: f32) -> f32 {
        let mut wet = input_sample;
        for band in &mut self.bands {
            wet = band.process_sample(channel_index, wet);
        }
        let mixed = input_sample + ((wet - input_sample) * self.wet_mix);
        mixed * self.output_gain_linear
    }
}

pub fn serialize_preset_toml(preset: &PeqPresetFile) -> Result<String> {
    preset.validate()?;
    toml::to_string_pretty(preset)
        .map_err(|error| PeqError::Parse(format!("failed to serialize PEQ preset: {error}")))
}

pub fn deserialize_preset_toml(text: &str) -> Result<PeqPresetFile> {
    let preset: PeqPresetFile = toml::from_str(text)
        .map_err(|error| PeqError::Parse(format!("failed to parse PEQ preset TOML: {error}")))?;
    preset.validate()?;
    Ok(preset)
}

pub fn save_preset_file(path: &Path, preset: &PeqPresetFile) -> Result<()> {
    let text = serialize_preset_toml(preset)?;
    fs::write(path, text)
        .map_err(|error| PeqError::Io(format!("failed to write {}: {error}", path.display())))
}

pub fn load_preset_file(path: &Path) -> Result<PeqPresetFile> {
    let text = fs::read_to_string(path)
        .map_err(|error| PeqError::Io(format!("failed to read {}: {error}", path.display())))?;
    deserialize_preset_toml(&text)
}

#[derive(Debug, Clone)]
struct RuntimeBand {
    config: PeqBandConfig,
    q_value: f32,
    shelf_slope: f32,
    sample_rate_hz: u32,
    current_gain_db: f32,
    target_gain_db: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
    current_coefficients: BiquadCoefficients,
    channel_states: Vec<BiquadState>,
}

impl RuntimeBand {
    fn new(sample_rate_hz: u32, channel_count: usize, config: PeqBandConfig) -> Result<Self> {
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

    fn reset_state(&mut self) {
        for state in &mut self.channel_states {
            *state = BiquadState::default();
        }
        self.current_gain_db = self.target_gain_db;
        self.current_coefficients = self.design_coefficients(self.current_gain_db);
    }

    fn advance_for_next_sample(&mut self) {
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

    fn process_sample(&mut self, channel_index: usize, input_sample: f32) -> f32 {
        if !self.config.enabled {
            return input_sample;
        }
        let state = &mut self.channel_states[channel_index];
        self.current_coefficients.process(input_sample, state)
    }

    fn design_coefficients(&self, gain_db: f32) -> BiquadCoefficients {
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
struct BiquadCoefficients {
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
struct BiquadState {
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

fn octave_bandwidth_to_q(octaves: f32) -> f32 {
    let clamped = octaves.max(0.01);
    let exponent = 2.0_f32.powf(clamped * 0.5);
    let ratio = (exponent - (1.0 / exponent)).max(DEFAULT_TOLERANCE);
    1.0 / ratio
}

fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

fn validate_positive_finite(value: f32, field_name: &str) -> Result<()> {
    validate_finite(value, field_name)?;
    if value <= 0.0 {
        return Err(PeqError::InvalidConfig(format!(
            "{field_name} must be greater than zero"
        )));
    }
    Ok(())
}

fn validate_finite(value: f32, field_name: &str) -> Result<()> {
    if !value.is_finite() {
        return Err(PeqError::InvalidConfig(format!(
            "{field_name} must be finite"
        )));
    }
    Ok(())
}

fn default_enabled() -> bool {
    true
}

fn default_wet_mix() -> f32 {
    1.0
}
