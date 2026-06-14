use flutz_synth::{DesiredSoundFontFormat, RuntimeSupport};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SourceReviewFinding {
    pub format: DesiredSoundFontFormat,
    pub support: RuntimeSupport,
}
