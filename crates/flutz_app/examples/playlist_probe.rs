use std::{collections::BTreeMap, path::Path};

use flutz_app::playlist::{PlaylistEntry, PlaylistRepeatMode, PlaylistState};
use flutz_formats::builtin_registry;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut playlist = PlaylistState::default();
    playlist.shuffle_seed = 0xC0FFEE_u64;
    playlist.repeat_mode = PlaylistRepeatMode::Playlist;

    for path in [
        "MIDI Files/rendering-parity-midi/smb-fx-test.fmid",
        "encoded-audio/mp3-44100-stereo.mp3",
        "_local/runtime-tests/missing-phase6-track.flac",
        "encoded-audio/aiff-16bit-44100-stereo.aiff",
        "encoded-audio/opus-44100-stereo.opus",
    ] {
        playlist.add_entry(path.into());
    }

    let mut counts = BTreeMap::<String, usize>::new();
    for entry in &playlist.entries {
        let key = classify_entry(entry);
        *counts.entry(key).or_default() += 1;
    }

    let counts_summary = counts
        .into_iter()
        .map(|(format, count)| format!("{format}:{count}"))
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "scenario=mixed-format-counts entries={} by_format={} status=ok",
        playlist.entries.len(),
        counts_summary,
    );

    let filter = |entry: &PlaylistEntry| is_supported_playlist_entry(&entry.file_path);
    let first = playlist
        .first_valid_track_by(filter)
        .ok_or("expected at least one valid track")?;
    playlist.set_current_index(Some(first));
    println!(
        "transition=initial current_index={} path={} status=ok",
        first,
        playlist.entries[first].file_path.display(),
    );

    let next_one = playlist
        .next_track_for_mode_by(filter)
        .ok_or("expected a next valid track")?;
    playlist.set_current_with_history(next_one, true);
    println!(
        "transition=next current_index={} path={} status=ok",
        next_one,
        playlist.entries[next_one].file_path.display(),
    );

    let next_two = playlist
        .next_track_for_mode_by(filter)
        .ok_or("expected another next valid track")?;
    playlist.set_current_with_history(next_two, true);
    println!(
        "transition=next current_index={} path={} skipped_missing=true status=ok",
        next_two,
        playlist.entries[next_two].file_path.display(),
    );

    let previous = playlist
        .prev_track_for_mode_by(filter)
        .ok_or("expected a previous valid track")?;
    println!(
        "transition=prev current_index={} path={} history_depth={} status=ok",
        previous,
        playlist.entries[previous].file_path.display(),
        playlist.history.len(),
    );

    println!("status=ok");
    Ok(())
}

fn classify_entry(entry: &PlaylistEntry) -> String {
    let registry = builtin_registry();
    entry
        .file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| registry.find_by_extension(ext))
        .map(|descriptor| descriptor.id.to_owned())
        .unwrap_or_else(|| "unsupported".to_owned())
}

fn is_supported_playlist_entry(path: &Path) -> bool {
    let registry = builtin_registry();
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| registry.find_by_extension(ext).is_some())
}
