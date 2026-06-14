use std::{fs, path::Path};

use flutz_core::{FlutzError, Result};
use serde::{Deserialize, Serialize};

use super::{
    PlaylistEntry, PlaylistOrderMode, PlaylistRepeatMode, PlaylistState,
};

const PLAYLIST_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlaylistFileEntry {
    file_path: String,
    display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlaylistFile {
    version: u32,
    loop_enabled: bool,
    entries: Vec<PlaylistFileEntry>,
    repeat_mode: Option<String>,
    order_mode: Option<String>,
    shuffle_seed: Option<u64>,
}

pub fn save_playlist(path: &Path, state: &PlaylistState) -> Result<()> {
    let serializable = PlaylistFile {
        version: PLAYLIST_VERSION,
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
    fs::write(path, json)
        .map_err(|error| FlutzError::Runtime(format!("failed to write {}: {error}", path.display())))
}

pub fn load_playlist(path: &Path) -> Result<PlaylistState> {
    let json = fs::read_to_string(path)
        .map_err(|error| FlutzError::Runtime(format!("failed to read {}: {error}", path.display())))?;
    let parsed: PlaylistFile = serde_json::from_str(&json)
        .map_err(|error| FlutzError::Runtime(format!("failed to parse {}: {error}", path.display())))?;

    if parsed.version == 0 || parsed.version > PLAYLIST_VERSION {
        return Err(FlutzError::Runtime(format!(
            "unsupported playlist version {} in {}",
            parsed.version,
            path.display()
        )));
    }

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
