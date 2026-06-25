use std::{env, fs, path::PathBuf};

use flutz_app::playlist::{
    persistence::{
        inject_legacy_version_field, inject_unknown_playlist_fields, load_playlist,
        parse_playlist_json_str, render_playlist_json_string, save_playlist,
    },
    PlaylistOrderMode, PlaylistRepeatMode, PlaylistState,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from("_local/runtime-tests");
    fs::create_dir_all(&out_dir)?;

    let mixed_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| out_dir.join("phase6-mixed-format.fplist"));
    let legacy_path = out_dir.join("phase6-legacy-format.fplist");
    let unknown_path = out_dir.join("phase6-unknown-fields.fplist");

    let mut playlist = PlaylistState::default();
    playlist.shuffle_seed = 0xC0FFEE_u64;
    playlist.loop_enabled = false;
    playlist.repeat_mode = PlaylistRepeatMode::Playlist;
    playlist.order_mode = PlaylistOrderMode::Shuffle;
    for file in [
        "MIDI Files/rendering-parity-midi/smb-fx-test.fmid",
        "encoded-audio/mp3-44100-stereo.mp3",
        "encoded-audio/flac-44100-stereo.flac",
        "encoded-audio/opus-44100-stereo.opus",
        "encoded-audio/aiff-16bit-44100-stereo.aiff",
        "encoded-audio/opus-44100-mono.opus",
    ] {
        playlist.add_entry(PathBuf::from(file));
    }
    playlist.file_path = Some(mixed_path.clone());
    playlist.dirty = false;

    save_playlist(&mixed_path, &playlist)?;
    let persisted_json = fs::read_to_string(&mixed_path)?;
    let saved_has_version = persisted_json.contains("\"version\"");
    let reloaded = load_playlist(&mixed_path)?;
    println!(
        "scenario=roundtrip path={} entry_count={} repeat_mode={:?} order_mode={:?} has_version_field={} status=ok",
        mixed_path.display(),
        reloaded.entries.len(),
        reloaded.repeat_mode,
        reloaded.order_mode,
        saved_has_version,
    );

    let legacy_json = inject_legacy_version_field(&render_playlist_json_string(&playlist)?, 99)?;
    fs::write(&legacy_path, &legacy_json)?;
    let legacy = load_playlist(&legacy_path)?;
    println!(
        "scenario=legacy-version path={} legacy_version=99 accepted_entries={} status=ok",
        legacy_path.display(),
        legacy.entries.len(),
    );

    let unknown_json = inject_unknown_playlist_fields(&render_playlist_json_string(&playlist)?)?;
    fs::write(&unknown_path, &unknown_json)?;
    let reparsed = parse_playlist_json_str(&unknown_json, &unknown_path)?;
    println!(
        "scenario=unknown-fields path={} parsed_entries={} unknown_field_tolerated=true status=ok",
        unknown_path.display(),
        reparsed.entries.len(),
    );

    for entry in &reloaded.entries {
        let extension = entry
            .file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        println!(
            "entry path={} extension={} exists={} status=ok",
            entry.file_path.display(),
            extension,
            entry.file_exists,
        );
    }

    println!("status=ok");
    Ok(())
}
