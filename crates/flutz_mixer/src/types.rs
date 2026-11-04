use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct MixerStripInput {
    pub identity: MixerStripIdentity,
    pub controls: MixerStripControls,
    pub automatic_processing: bool,
    pub block: AudioBlock,
}

#[derive(Debug, Clone)]
pub struct MixerStripInputView<'a> {
    pub identity: MixerStripIdentity,
    pub controls: MixerStripControls,
    pub automatic_processing: bool,
    pub block: AudioBlockView<'a>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MixerSettings {
    pub master: MasterControls,
    pub smart_mix: SmartMixSettings,
    pub auto_normalization: AutoNormalization,
}

impl Default for MixerSettings {
    fn default() -> Self {
        Self {
            master: MasterControls::default(),
            smart_mix: SmartMixSettings::default(),
            auto_normalization: AutoNormalization::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StripMixReport {
    pub identity: MixerStripIdentity,
    pub input_meter: MeterReading,
    pub applied_gain: f32,
    pub smart_mix_gain: f32,
    pub lookahead_gain: f32,
    pub normalization_gain: f32,
    pub audible: bool,
    pub solo_active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixReport {
    pub output: AudioBlock,
    pub output_meter: MeterReading,
    pub strips: Vec<StripMixReport>,
    pub allocation_stats: MixerAllocationStats,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixOutputReport {
    pub output_meter: MeterReading,
    pub strips: Vec<StripMixReport>,
    pub allocation_stats: MixerAllocationStats,
}
