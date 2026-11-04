use super::biquad::octave_bandwidth_to_q;
use super::*;

pub type Result<T> = std::result::Result<T, PeqError>;

const MIN_FREQUENCY_HZ: f32 = 10.0;
const MIN_ATTACK_RELEASE_MS: f32 = 0.0;
pub(super) const DEFAULT_TOLERANCE: f32 = 0.000_001;

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

    pub(super) fn q_value(&self) -> f32 {
        match self {
            Self::Q { value } => value.max(0.05),
            Self::Octaves { value } => octave_bandwidth_to_q(*value),
        }
    }

    pub(super) fn shelf_slope(&self) -> f32 {
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
pub(super) fn validate_positive_finite(value: f32, field_name: &str) -> Result<()> {
    validate_finite(value, field_name)?;
    if value <= 0.0 {
        return Err(PeqError::InvalidConfig(format!(
            "{field_name} must be greater than zero"
        )));
    }
    Ok(())
}

pub(super) fn validate_finite(value: f32, field_name: &str) -> Result<()> {
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
