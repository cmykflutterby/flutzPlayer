use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

const DESKTOP_FILE: &str = "flutzplayer.desktop";
const ICON_FILE: &str = "flutzplayer.png";
const ICON_DIRECTORY: &str = "icons/hicolor/256x256/apps";
const MIME_TYPES: &str = "audio/midi;audio/x-midi;audio/mpeg;audio/ogg;audio/opus;audio/flac;audio/wav;audio/x-wav;audio/aac;audio/mp3;audio/mp4;";

pub(crate) fn integrate_best_effort() {
    if let Err(error) = integrate() {
        eprintln!("Linux desktop integration failed: {error}");
    }
}

fn integrate() -> io::Result<()> {
    let Some(appimage) = crate::launch::original_appimage_path()
        .map_err(|error| io::Error::other(error.to_string()))?
    else {
        return Ok(());
    };
    let appdir = env::var_os("APPDIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDIR is not set"))?;
    if !appdir.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("APPDIR must be absolute: {}", appdir.display()),
        ));
    }

    let data_home = xdg_data_home()?;
    let icon_path = data_home.join(ICON_DIRECTORY).join(ICON_FILE);
    let desktop_path = data_home.join("applications").join(DESKTOP_FILE);
    let fields = desktop_fields(&appimage, &icon_path)?;
    let existing_desktop = read_optional(&desktop_path)?;
    let desktop_update = merge_desktop_entry(existing_desktop.as_deref(), &fields)?;

    let source_icon = appdir
        .join("usr/share")
        .join(ICON_DIRECTORY)
        .join(ICON_FILE);
    let icon = fs::read(&source_icon).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read icon {}: {error}", source_icon.display()),
        )
    })?;
    if read_optional(&icon_path)?.as_deref() != Some(icon.as_slice()) {
        write_atomic(&icon_path, &icon)?;
    }

    if let Some(contents) = desktop_update {
        write_atomic(&desktop_path, contents.as_bytes())?;
        refresh_desktop_database(
            desktop_path
                .parent()
                .expect("desktop entry path has a parent"),
        );
    }
    Ok(())
}

fn xdg_data_home() -> io::Result<PathBuf> {
    if let Some(value) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Ok(path);
        }
        eprintln!(
            "ignoring relative XDG_DATA_HOME {}; it must be absolute",
            path.display()
        );
    }

    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    if !home.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("HOME must be absolute: {}", home.display()),
        ));
    }
    Ok(home.join(".local/share"))
}

fn desktop_fields(appimage: &Path, icon: &Path) -> io::Result<Vec<(&'static str, String)>> {
    let directory = appimage.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "AppImage has no parent directory",
        )
    })?;
    Ok(vec![
        ("Type", "Application".to_owned()),
        ("Name", "flutzPlayer".to_owned()),
        ("Comment", "Audio player".to_owned()),
        ("Exec", desktop_exec(appimage)?),
        ("Path", desktop_path_value(directory)?),
        ("Icon", desktop_path_value(icon)?),
        ("Terminal", "false".to_owned()),
        ("Categories", "AudioVideo;Audio;Player;".to_owned()),
        ("MimeType", MIME_TYPES.to_owned()),
        ("StartupNotify", "true".to_owned()),
    ])
}

fn desktop_exec(appimage: &Path) -> io::Result<String> {
    let path = utf8_path(appimage)?;
    if path.contains('%') || path.contains('=') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "direct desktop launch does not support '%' or '=' in the AppImage path {}; move the AppImage to a path without those characters",
                appimage.display()
            ),
        ));
    }
    let mut command = String::from("\"");
    for character in path.chars() {
        match character {
            '"' | '\\' | '$' | '`' => {
                command.push('\\');
                command.push(character);
            }
            _ => command.push(character),
        }
    }
    command.push_str("\" --gui %f");
    escape_value(&command, false)
}

fn desktop_path_value(path: &Path) -> io::Result<String> {
    escape_value(utf8_path(path)?, true)
}

fn utf8_path(path: &Path) -> io::Result<&str> {
    path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("desktop entry path is not UTF-8: {}", path.display()),
        )
    })
}

fn escape_value(value: &str, escape_spaces: bool) -> io::Result<String> {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            ' ' if escape_spaces => escaped.push_str("\\s"),
            character if character.is_control() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "desktop entry path contains an unsupported control character",
                ));
            }
            _ => escaped.push(character),
        }
    }
    Ok(escaped)
}

fn merge_desktop_entry(
    existing: Option<&[u8]>,
    fields: &[(&str, String)],
) -> io::Result<Option<String>> {
    let Some(existing) = existing else {
        let mut contents = String::from("[Desktop Entry]\n");
        let mut seen = vec![false; fields.len()];
        append_missing_fields(&mut contents, fields, &mut seen, "\n");
        return Ok(Some(contents));
    };
    let existing = std::str::from_utf8(existing).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("existing desktop entry is not UTF-8: {error}"),
        )
    })?;
    let newline = if existing.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut contents = String::with_capacity(existing.len());
    let mut seen = vec![false; fields.len()];
    let mut found_entry = false;
    let mut in_entry = false;
    let mut changed = false;

    for line in existing.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = body.strip_suffix('\r').unwrap_or(body);
        let trimmed = body.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_entry {
                changed |= append_missing_fields(&mut contents, fields, &mut seen, newline);
            }
            in_entry = trimmed == "[Desktop Entry]";
            if in_entry {
                if found_entry {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "existing desktop entry has duplicate Desktop Entry groups",
                    ));
                }
                found_entry = true;
            }
            contents.push_str(line);
            continue;
        }

        if in_entry && !trimmed.starts_with('#') {
            if let Some((key, value)) = body.split_once('=') {
                if let Some(index) = fields.iter().position(|(name, _)| *name == key.trim()) {
                    if seen[index] {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "existing desktop entry has duplicate {} keys",
                                fields[index].0
                            ),
                        ));
                    }
                    seen[index] = true;
                    if value.trim() != fields[index].1 {
                        changed = true;
                        contents.push_str(fields[index].0);
                        contents.push('=');
                        contents.push_str(&fields[index].1);
                        if line.ends_with("\r\n") {
                            contents.push_str("\r\n");
                        } else if line.ends_with('\n') {
                            contents.push('\n');
                        }
                        continue;
                    }
                }
            }
        }
        contents.push_str(line);
    }

    if !found_entry {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "existing desktop entry has no Desktop Entry group",
        ));
    }
    if in_entry {
        changed |= append_missing_fields(&mut contents, fields, &mut seen, newline);
    }
    Ok(changed.then_some(contents))
}

fn append_missing_fields(
    contents: &mut String,
    fields: &[(&str, String)],
    seen: &mut [bool],
    newline: &str,
) -> bool {
    let mut changed = false;
    for (index, (key, value)) in fields.iter().enumerate() {
        if !seen[index] {
            if !contents.ends_with('\n') {
                contents.push_str(newline);
            }
            contents.push_str(key);
            contents.push('=');
            contents.push_str(value);
            contents.push_str(newline);
            seen[index] = true;
            changed = true;
        }
    }
    changed
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("could not read {}: {error}", path.display()),
        )),
    }
}

fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    flutz_fs_sdl3::atomic_write(path, contents).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not update {}: {error}", path.display()),
        )
    })
}

fn refresh_desktop_database(directory: &Path) {
    match Command::new("update-desktop-database")
        .arg(directory)
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!(
            "update-desktop-database failed for {}: {status}",
            directory.display()
        ),
        Err(error) => eprintln!(
            "could not run update-desktop-database for {}: {error}",
            directory.display()
        ),
    }
}
