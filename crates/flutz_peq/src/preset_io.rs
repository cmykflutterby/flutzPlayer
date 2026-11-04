use super::*;

pub fn serialize_preset_toml(preset: &PeqPresetFile) -> Result<String> {
    preset.validate()?;
    toml::to_string_pretty(preset)
        .map_err(|error| PeqError::Parse(format!("failed to serialize PEQ preset: {error}")))
}

pub fn deserialize_preset_toml(text: &str) -> Result<PeqPresetFile> {
    let preset: PeqPresetFile = toml::from_str(text)
        .map_err(|error| PeqError::Parse(format!("failed to parse PEQ preset TOML: {error}")))?;
    preset.validate()?;
    Ok(preset)
}

pub fn save_preset_file(path: &Path, preset: &PeqPresetFile) -> Result<()> {
    let text = serialize_preset_toml(preset)?;
    fs::write(path, text)
        .map_err(|error| PeqError::Io(format!("failed to write {}: {error}", path.display())))
}

pub fn load_preset_file(path: &Path) -> Result<PeqPresetFile> {
    let text = fs::read_to_string(path)
        .map_err(|error| PeqError::Io(format!("failed to read {}: {error}", path.display())))?;
    deserialize_preset_toml(&text)
}
