use std::collections::HashSet;

use flutz_core::{FlutzError, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatRegistry {
    pub default_soundfont_id: Option<String>,
    pub soundfonts: Vec<DatRegistrySoundFont>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatRegistrySoundFont {
    pub internal_id: String,
    pub display_name: String,
    pub runtime_format: String,
}

#[derive(Debug, Deserialize)]
struct RawDatRegistry {
    default_soundfont_id: Option<String>,
    #[serde(default)]
    soundfonts: Vec<RawDatRegistrySoundFont>,
}

#[derive(Debug, Deserialize)]
struct RawDatRegistrySoundFont {
    internal_id: String,
    display_name: String,
    runtime_format: String,
}

pub fn parse_dat_registry(input: &str) -> Result<DatRegistry> {
    let raw: RawDatRegistry = toml::from_str(input)
        .map_err(|error| FlutzError::InvalidInput(format!("invalid DAT registry TOML: {error}")))?;

    let mut seen_ids = HashSet::new();
    for soundfont in &raw.soundfonts {
        if !seen_ids.insert(soundfont.internal_id.clone()) {
            return Err(FlutzError::InvalidInput(format!(
                "duplicate registry soundfont ID: {}",
                soundfont.internal_id
            )));
        }
    }

    Ok(DatRegistry {
        default_soundfont_id: raw.default_soundfont_id,
        soundfonts: raw
            .soundfonts
            .into_iter()
            .map(|soundfont| DatRegistrySoundFont {
                internal_id: soundfont.internal_id,
                display_name: soundfont.display_name,
                runtime_format: soundfont.runtime_format,
            })
            .collect(),
    })
}
