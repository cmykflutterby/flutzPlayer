use flutz_synth::DesiredSoundFontFormat;

pub fn detect_by_extension(extension: &str) -> Option<DesiredSoundFontFormat> {
    DesiredSoundFontFormat::from_extension(extension)
}
