#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct FmidChunkRecord {
    pub chunk_id: [u8; 4],
    pub offset: u64,
    pub length: u64,
    pub flags: u64,
    pub ordinal: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FmidFile {
    pub header_flags: u64,
    pub chunk_table_flags: u64,
    pub midi_bytes: Vec<u8>,
    pub project: ProjectRecord,
    pub soundfonts: Vec<SoundFontSlot>,
    pub mixer: MixerRecord,
    pub mixer_source_mode: MixerSourceMode,
    pub looping: LoopRecord,
    pub smart_mix: SmartMixRecord,
    pub note: Option<String>,
    pub unknown_chunks: Vec<UnknownChunk>,
}

impl Default for FmidFile {
    fn default() -> Self {
        Self {
            header_flags: 0,
            chunk_table_flags: 0,
            midi_bytes: Vec::new(),
            project: ProjectRecord::default(),
            soundfonts: Vec::new(),
            mixer: MixerRecord::default(),
            mixer_source_mode: MixerSourceMode::default(),
            looping: LoopRecord::default(),
            smart_mix: SmartMixRecord::default(),
            note: None,
            unknown_chunks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MixerSourceMode {
    #[default]
    Custom,
    PresetDefault(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectRecord {
    pub project_name: String,
    pub source_midi_filename: String,
    pub project_flags: u64,
    pub notes: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontSlot {
    pub internal_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixerRecord {
    pub master: MasterMixerRecord,
    pub row_mutes: Vec<SoundFontRowMuteRecord>,
    pub strips: Vec<MixerStripRecord>,
}

impl Default for MixerRecord {
    fn default() -> Self {
        Self {
            master: MasterMixerRecord::default(),
            row_mutes: Vec::new(),
            strips: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontRowMuteRecord {
    pub soundfont_id: String,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MasterMixerRecord {
    pub volume_db: f64,
    pub limiter_enabled: bool,
    pub limiter_amount: f64,
    pub limiter_release: f64,
    pub reverb: f64,
    pub chorus: f64,
    pub eq_low_db: f64,
    pub eq_mid_db: f64,
    pub eq_high_db: f64,
}

impl Default for MasterMixerRecord {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            limiter_enabled: false,
            limiter_amount: 0.25,
            limiter_release: 0.5,
            reverb: 0.0,
            chorus: 0.0,
            eq_low_db: 0.0,
            eq_mid_db: 0.0,
            eq_high_db: 0.0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MixerStripIdentity {
    pub soundfont_id: String,
    pub midi_channel: u64,
    pub midi_program: u64,
    pub is_percussion: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixerStripControls {
    pub volume: f64,
    pub mute: bool,
    pub pan: f64,
    pub gain_db: f64,
    pub limiter_enabled: bool,
    pub limiter_amount: f64,
    pub limiter_release: f64,
    pub reverb: f64,
    pub chorus: f64,
}

impl Default for MixerStripControls {
    fn default() -> Self {
        Self {
            volume: 1.0,
            mute: false,
            pan: 0.0,
            gain_db: 0.0,
            limiter_enabled: false,
            limiter_amount: 0.25,
            limiter_release: 0.5,
            reverb: 0.0,
            chorus: 0.0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MixerStripRecord {
    pub identity: MixerStripIdentity,
    pub controls: MixerStripControls,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum LoopMode {
    #[default]
    None,
    Infinite,
    Counted,
}

impl LoopMode {
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::None),
            1 => Some(Self::Infinite),
            2 => Some(Self::Counted),
            _ => None,
        }
    }

    pub fn to_u64(self) -> u64 {
        match self {
            Self::None => 0,
            Self::Infinite => 1,
            Self::Counted => 2,
        }
    }
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct LoopRecord {
    pub enabled: bool,
    pub mode: LoopMode,
    pub start_tick: u64,
    pub end_tick: u64,
    pub loop_count: u64,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SmartMixRecord {
    pub enabled: bool,
    pub target_headroom: f64,
    pub attack: f64,
    pub release: f64,
    pub lookahead: f64,
    pub auto_normalization_enabled: bool,
    pub auto_normalization_amount: f64,
}

impl Default for SmartMixRecord {
    fn default() -> Self {
        Self {
            enabled: false,
            target_headroom: 0.0,
            attack: 0.0,
            release: 0.0,
            lookahead: 0.0,
            auto_normalization_enabled: false,
            auto_normalization_amount: 0.0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnknownChunk {
    pub chunk_id: [u8; 4],
    pub flags: u64,
    pub ordinal: u64,
    pub payload: Vec<u8>,
}
