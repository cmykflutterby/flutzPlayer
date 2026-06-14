use flutz_synth::DesiredSoundFontFormat;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ConversionStatus {
    NotRequired,
    RequiredNotImplemented,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ConversionStage {
    PassThroughSf2,
    DecompressSfArk,
    WriteSf2RiffSfbk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionPlan {
    pub source_format: DesiredSoundFontFormat,
    pub target_format: DesiredSoundFontFormat,
    pub status: ConversionStatus,
    pub stages: Vec<ConversionStage>,
}

impl ConversionPlan {
    pub fn for_rustysynth(source_format: DesiredSoundFontFormat) -> Self {
        match source_format {
            DesiredSoundFontFormat::Sf2 => Self {
                source_format,
                target_format: DesiredSoundFontFormat::Sf2,
                status: ConversionStatus::NotRequired,
                stages: vec![ConversionStage::PassThroughSf2],
            },
            DesiredSoundFontFormat::SfArk => Self {
                source_format,
                target_format: DesiredSoundFontFormat::Sf2,
                status: ConversionStatus::RequiredNotImplemented,
                stages: vec![
                    ConversionStage::DecompressSfArk,
                    ConversionStage::WriteSf2RiffSfbk,
                ],
            },
        }
    }
}
