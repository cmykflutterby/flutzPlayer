#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BlendMode {
    ReplaceMute,
    BlendEven,
    BlendWeight,
}

impl BlendMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReplaceMute => "replace-mute",
            Self::BlendEven => "blend-even",
            Self::BlendWeight => "blend-weight",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PresetWeight {
    pub font_id: &'static str,
    pub weight: u32,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub font_ids: &'static [&'static str],
    pub blend_mode: BlendMode,
    pub weights: &'static [PresetWeight],
}

impl Preset {
    pub fn weight_for_font(self, font_id: &str) -> Option<u32> {
        self.weights
            .iter()
            .find(|weight| weight.font_id == font_id)
            .map(|weight| weight.weight)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CoverageSettings {
    pub mode: &'static str,
    pub strict_bank_program_match: bool,
    pub percussion_requires_bank_128: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PresetSet {
    pub default_preset_id: &'static str,
    pub coverage: CoverageSettings,
    pub presets: &'static [Preset],
}

impl PresetSet {
    pub fn default_preset(self) -> &'static Preset {
        self.find_preset(self.default_preset_id)
            .expect("generated preset set must contain its default_preset_id")
    }

    pub fn find_preset(self, preset_id: &str) -> Option<&'static Preset> {
        self.presets.iter().find(|preset| preset.id == preset_id)
    }
}

mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated_presets.rs"));
}

pub fn default_preset_set() -> &'static PresetSet {
    &generated::PRESET_SET
}
