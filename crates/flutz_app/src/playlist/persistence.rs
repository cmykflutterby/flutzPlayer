use std::{fs, path::Path};

use flutz_core::{FlutzError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{PlaylistEntry, PlaylistOrderMode, PlaylistRepeatMode, PlaylistState};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlaylistFileEntry {
    file_path: String,
    display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlaylistFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    loop_enabled: bool,
    #[serde(default)]
    entries: Vec<PlaylistFileEntry>,
    #[serde(default)]
    repeat_mode: Option<String>,
    #[serde(default)]
    order_mode: Option<String>,
    #[serde(default)]
    shuffle_seed: Option<u64>,
}

pub fn save_playlist(path: &Path, state: &PlaylistState) -> Result<()> {
    let serializable = PlaylistFile {
        version: None,
        loop_enabled: state.loop_enabled,
        entries: state
            .entries
            .iter()
            .map(|entry| PlaylistFileEntry {
                file_path: entry.file_path.display().to_string(),
                display_name: entry.display_name.clone(),
            })
            .collect(),
        repeat_mode: Some(match state.repeat_mode {
            PlaylistRepeatMode::Off => "off".to_owned(),
            PlaylistRepeatMode::Track => "track".to_owned(),
            PlaylistRepeatMode::Playlist => "playlist".to_owned(),
        }),
        order_mode: Some(match state.order_mode {
            PlaylistOrderMode::Sequential => "sequential".to_owned(),
            PlaylistOrderMode::Shuffle => "shuffle".to_owned(),
            PlaylistOrderMode::Random => "random".to_owned(),
        }),
        shuffle_seed: Some(state.shuffle_seed),
    };

    let json = serde_json::to_string_pretty(&serializable)
        .map_err(|error| FlutzError::Runtime(format!("failed to serialize playlist: {error}")))?;
    fs::write(path, json).map_err(|error| {
        FlutzError::Runtime(format!("failed to write {}: {error}", path.display()))
    })
}

pub fn load_playlist(path: &Path) -> Result<PlaylistState> {
    let json = fs::read_to_string(path).map_err(|error| {
        FlutzError::Runtime(format!("failed to read {}: {error}", path.display()))
    })?;
    let parsed: PlaylistFile = serde_json::from_str(&json).map_err(|error| {
        FlutzError::Runtime(format!("failed to parse {}: {error}", path.display()))
    })?;

    let entries = parsed
        .entries
        .into_iter()
        .map(|entry| {
            let mut playlist_entry = PlaylistEntry::from_path(entry.file_path.into());
            if !entry.display_name.trim().is_empty() {
                playlist_entry.display_name = entry.display_name;
            }
            playlist_entry
        })
        .collect::<Vec<_>>();

    let repeat_mode = match parsed.repeat_mode.as_deref() {
        Some("track") => PlaylistRepeatMode::Track,
        Some("playlist") => PlaylistRepeatMode::Playlist,
        _ => PlaylistRepeatMode::Off,
    };
    let order_mode = match parsed.order_mode.as_deref() {
        Some("shuffle") => PlaylistOrderMode::Shuffle,
        Some("random") => PlaylistOrderMode::Random,
        _ => PlaylistOrderMode::Sequential,
    };

    let mut state = PlaylistState {
        entries,
        current_index: None,
        file_path: Some(path.to_path_buf()),
        dirty: false,
        loop_enabled: parsed.loop_enabled,
        repeat_mode,
        order_mode,
        shuffle_seed: parsed.shuffle_seed.unwrap_or(0xC0FFEE_u64),
        ..PlaylistState::default()
    };

    if !state.entries.is_empty() {
        state.set_current_index(Some(0));
    }

    Ok(state)
}

pub fn render_playlist_json_value(state: &PlaylistState) -> Result<Value> {
    let serializable = PlaylistFile {
        version: None,
        loop_enabled: state.loop_enabled,
        entries: state
            .entries
            .iter()
            .map(|entry| PlaylistFileEntry {
                file_path: entry.file_path.display().to_string(),
                display_name: entry.display_name.clone(),
            })
            .collect(),
        repeat_mode: Some(match state.repeat_mode {
            PlaylistRepeatMode::Off => "off".to_owned(),
            PlaylistRepeatMode::Track => "track".to_owned(),
            PlaylistRepeatMode::Playlist => "playlist".to_owned(),
        }),
        order_mode: Some(match state.order_mode {
            PlaylistOrderMode::Sequential => "sequential".to_owned(),
            PlaylistOrderMode::Shuffle => "shuffle".to_owned(),
            PlaylistOrderMode::Random => "random".to_owned(),
        }),
        shuffle_seed: Some(state.shuffle_seed),
    };

    serde_json::to_value(&serializable)
        .map_err(|error| FlutzError::Runtime(format!("failed to serialize playlist: {error}")))
}

pub fn render_playlist_json_string(state: &PlaylistState) -> Result<String> {
    let value = render_playlist_json_value(state)?;
    serde_json::to_string_pretty(&value)
        .map_err(|error| FlutzError::Runtime(format!("failed to serialize playlist: {error}")))
}

pub fn parse_playlist_json_str(json: &str, source: &Path) -> Result<PlaylistState> {
    let parsed: PlaylistFile = serde_json::from_str(json).map_err(|error| {
        FlutzError::Runtime(format!("failed to parse {}: {error}", source.display()))
    })?;

    let entries = parsed
        .entries
        .into_iter()
        .map(|entry| {
            let mut playlist_entry = PlaylistEntry::from_path(entry.file_path.into());
            if !entry.display_name.trim().is_empty() {
                playlist_entry.display_name = entry.display_name;
            }
            playlist_entry
        })
        .collect::<Vec<_>>();

    let repeat_mode = match parsed.repeat_mode.as_deref() {
        Some("track") => PlaylistRepeatMode::Track,
        Some("playlist") => PlaylistRepeatMode::Playlist,
        _ => PlaylistRepeatMode::Off,
    };
    let order_mode = match parsed.order_mode.as_deref() {
        Some("shuffle") => PlaylistOrderMode::Shuffle,
        Some("random") => PlaylistOrderMode::Random,
        _ => PlaylistOrderMode::Sequential,
    };

    let mut state = PlaylistState {
        entries,
        current_index: None,
        file_path: Some(source.to_path_buf()),
        dirty: false,
        loop_enabled: parsed.loop_enabled,
        repeat_mode,
        order_mode,
        shuffle_seed: parsed.shuffle_seed.unwrap_or(0xC0FFEE_u64),
        ..PlaylistState::default()
    };

    if !state.entries.is_empty() {
        state.set_current_index(Some(0));
    }

    Ok(state)
}

pub fn inject_unknown_playlist_fields(json: &str) -> Result<String> {
    let mut value: Value = serde_json::from_str(json)
        .map_err(|error| FlutzError::Runtime(format!("failed to parse playlist json: {error}")))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| FlutzError::Runtime("playlist json must be an object".to_owned()))?;
    object.insert(
        "future_capability".to_owned(),
        Value::String("decoded-audio-peq".to_owned()),
    );
    object.insert(
        "future_flags".to_owned(),
        Value::Array(vec![Value::String("survive-roundtrip".to_owned())]),
    );
    if let Some(entries) = object.get_mut("entries").and_then(Value::as_array_mut) {
        if let Some(first) = entries.first_mut().and_then(Value::as_object_mut) {
            first.insert(
                "capabilities".to_owned(),
                Value::Array(vec![Value::String("metadata".to_owned())]),
            );
        }
    }
    serde_json::to_string_pretty(&value).map_err(|error| {
        FlutzError::Runtime(format!("failed to serialize augmented playlist: {error}"))
    })
}

pub fn inject_legacy_version_field(json: &str, version: u32) -> Result<String> {
    let mut value: Value = serde_json::from_str(json)
        .map_err(|error| FlutzError::Runtime(format!("failed to parse playlist json: {error}")))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| FlutzError::Runtime("playlist json must be an object".to_owned()))?;
    object.insert("version".to_owned(), Value::from(version));
    serde_json::to_string_pretty(&value).map_err(|error| {
        FlutzError::Runtime(format!("failed to serialize legacy playlist: {error}"))
    })
}
