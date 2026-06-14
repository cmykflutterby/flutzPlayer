#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum DesiredSoundFontFormat {
    Sf2,
    SfArk,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RuntimeSupport {
    DirectlyUsable,
    RequiresConversion,
    Unsupported,
    UnknownPendingSourceReview,
}

impl DesiredSoundFontFormat {
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "sf2" => Some(Self::Sf2),
            "sfark" => Some(Self::SfArk),
            _ => None,
        }
    }

    pub fn canonical_extension(self) -> &'static str {
        match self {
            Self::Sf2 => ".sf2",
            Self::SfArk => ".sfArk",
        }
    }

    pub fn rustysynth_runtime_support(self) -> RuntimeSupport {
        match self {
            Self::Sf2 => RuntimeSupport::DirectlyUsable,
            Self::SfArk => RuntimeSupport::RequiresConversion,
        }
    }

    pub fn rustysynth_review_note(self) -> &'static str {
        match self {
            Self::Sf2 => "Directly supported when the file is a RIFF sfbk SoundFont with raw smpl sample data and valid pdta metadata.",
            Self::SfArk => "Not directly supported. rustysynth has no sfArk decompressor or archive reader; convert before normal DAT packing.",
        }
    }
}
