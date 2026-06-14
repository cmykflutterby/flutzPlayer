use crate::SoundFontId;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FmidProject {
    pub project_name: String,
    pub source_midi_filename: String,
    pub loaded_soundfonts: Vec<SoundFontId>,
}
