#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ContentKind {
    Midi,
    Fmid,
    DecodedAudio,
    DecodedAudioWrapper,
}

impl ContentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Midi => "midi",
            Self::Fmid => "fmid",
            Self::DecodedAudio => "decoded-audio",
            Self::DecodedAudioWrapper => "decoded-audio-wrapper",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BackendKind {
    Midi,
    Fmid,
    Symphonia,
    SymphoniaLibOpus,
    Wrapper,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Midi => "midi",
            Self::Fmid => "fmid",
            Self::Symphonia => "symphonia",
            Self::SymphoniaLibOpus => "symphonia-libopus",
            Self::Wrapper => "flutz-wrapper",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MasteringCapability {
    MidiMastering,
    DecodedAudioPeq,
    None,
}

impl MasteringCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MidiMastering => "midi-mastering",
            Self::DecodedAudioPeq => "decoded-audio-peq",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LoopMode {
    None,
    Infinite,
    Counted,
}

impl Default for LoopMode {
    fn default() -> Self {
        Self::None
    }
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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Infinite => "infinite",
            Self::Counted => "counted",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum LoopUnit {
    MidiTicks { start: u64, end: u64 },
    SampleFrames { start: u64, end: u64 },
    Seconds { start: f64, end: f64 },
}

impl LoopUnit {
    pub fn unit_name(self) -> &'static str {
        match self {
            Self::MidiTicks { .. } => "ticks",
            Self::SampleFrames { .. } => "sample-frames",
            Self::Seconds { .. } => "seconds",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MediaLoop {
    pub enabled: bool,
    pub mode: LoopMode,
    pub unit: LoopUnit,
    pub loop_count: u64,
}

impl Default for MediaLoop {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: LoopMode::None,
            unit: LoopUnit::SampleFrames { start: 0, end: 0 },
            loop_count: 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackMetadata {
    pub project_name: String,
    pub source_filename: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub composer: String,
    pub conductor: String,
    pub genre: String,
    pub date: String,
    pub track_number: String,
    pub track_total: String,
    pub disc_number: String,
    pub disc_total: String,
    pub description: String,
    pub copyright: String,
    pub publisher: String,
    pub encoded_by: String,
    pub encoder: String,
    pub language: String,
    pub lyrics: String,
    pub url: String,
    pub notes: String,
    pub extra_fields: Vec<MetadataField>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataField {
    pub key: String,
    pub value: String,
}

pub type NativeMetadata = Vec<MetadataField>;
