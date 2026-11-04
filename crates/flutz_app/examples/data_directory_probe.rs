use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().collect();
    let binary = fs::canonicalize(args.get(1).ok_or("pass the flutzplayer binary")?)?;
    let base = PathBuf::from(args.get(2).ok_or("pass an isolated probe directory")?);
    fs::create_dir_all(&base)?;
    let work = fs::canonicalize(base)?.join(format!("data-directory-probe-{}", std::process::id()));
    fs::create_dir(&work)?;
    let unrelated = work.join("unrelated working directory");
    let mount = work.join("temporary mount");
    fs::create_dir(&unrelated)?;
    fs::create_dir(&mount)?;

    let fallback = binary.parent().ok_or("binary has no parent")?.join("data");
    let output = debug_env(&binary, &unrelated, None, &mount, None)?;
    require_data_dir(&output, &fallback)?;
    println!("PASS executable-relative fallback from an unrelated directory");

    let override_dir = work.join("explicit data directory");
    let output = debug_env(
        &binary,
        &unrelated,
        Some(Path::new("relative.AppImage")),
        &mount,
        Some(&override_dir),
    )?;
    require_data_dir(&output, &override_dir)?;
    println!("PASS --data-dir overrides the default");

    #[cfg(target_os = "linux")]
    {
        let app_dir = work.join("App Directory with spaces");
        fs::create_dir(&app_dir)?;
        let image = app_dir.join("flutzPlayer test.AppImage");
        fs::write(&image, b"")?;
        let output = debug_env(&binary, &unrelated, Some(&image), &mount, None)?;
        require_data_dir(&output, &app_dir.join("data"))?;
        require(!app_dir.join("data").exists(), "resolver created data")?;
        println!("PASS APPIMAGE with spaces uses adjacent data without creating it");

        use std::os::unix::fs::symlink;
        let directory_link = work.join("app-directory-link");
        symlink(&app_dir, &directory_link)?;
        let output = debug_env(
            &binary,
            &unrelated,
            Some(&directory_link.join("flutzPlayer test.AppImage")),
            &mount,
            None,
        )?;
        require_data_dir(&output, &app_dir.join("data"))?;
        println!("PASS APPIMAGE parent normalization");

        let alias_dir = work.join("alias");
        fs::create_dir(&alias_dir)?;
        let alias = alias_dir.join("flutzPlayer.AppImage");
        symlink(&image, &alias)?;
        let output = debug_env(&binary, &unrelated, Some(&alias), &mount, None)?;
        require_data_dir(&output, &alias_dir.join("data"))?;
        println!("PASS the original AppImage symlink directory owns its data");

        for invalid in [Path::new("relative.AppImage"), Path::new("")] {
            let output = debug_env(&binary, &unrelated, Some(invalid), &mount, None)?;
            require(!output.status.success(), "invalid APPIMAGE was accepted")?;
            require(
                String::from_utf8_lossy(&output.stderr)
                    .contains("APPIMAGE must name an absolute AppImage path"),
                "invalid APPIMAGE error was not explicit",
            )?;
        }
        require(!mount.join("data").exists(), "data appeared in the mount")?;
        println!("PASS invalid APPIMAGE does not fall back to the mounted executable");

        if let Some(fixture) = args.get(3) {
            check_read_only_dat(&binary, &unrelated, &mount, &work, Path::new(fixture))?;
        } else {
            println!("SKIP read-only DAT: pass a real assets-000.dat as the third argument");
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let output = debug_env(
            &binary,
            &unrelated,
            Some(Path::new("relative.AppImage")),
            &mount,
            None,
        )?;
        require_data_dir(&output, &fallback)?;
        println!("PASS non-Linux executable fallback ignores APPIMAGE");
    }

    println!("data_directory_probe: PASS");
    Ok(())
}

fn debug_env(
    binary: &Path,
    cwd: &Path,
    image: Option<&Path>,
    mount: &Path,
    override_dir: Option<&Path>,
) -> io::Result<Output> {
    let mut command = Command::new(binary);
    command
        .current_dir(cwd)
        .env_remove("APPIMAGE")
        .env("APPDIR", mount)
        .arg("--debug-env");
    if let Some(image) = image {
        command.env("APPIMAGE", image);
    }
    if let Some(override_dir) = override_dir {
        command.arg("--data-dir").arg(override_dir);
    }
    command.output()
}

fn require_data_dir(output: &Output, expected: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let expected_line = format!("data_dir: {}", expected.display());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let actual = stdout
        .lines()
        .find_map(|line| line.strip_prefix("data_dir: "));
    #[cfg(windows)]
    let matches = actual
        .is_some_and(|path| windows_path(path) == windows_path(&expected.display().to_string()));
    #[cfg(not(windows))]
    let matches = actual.is_some_and(|path| Path::new(path) == expected);
    if output.status.success() && matches {
        Ok(())
    } else {
        Err(format!(
            "expected {expected_line}; status: {}; stdout: {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

#[cfg(windows)]
fn windows_path(path: &str) -> PathBuf {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(path.strip_prefix(r"\\?\").unwrap_or(path))
    }
}

#[cfg(target_os = "linux")]
fn require(condition: bool, failure: &str) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(format!("FAIL {failure}").into())
    }
}

#[cfg(target_os = "linux")]
fn check_read_only_dat(
    binary: &Path,
    cwd: &Path,
    mount: &Path,
    work: &Path,
    fixture: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::{fs::OpenOptions, os::unix::fs::PermissionsExt};

    struct RestorePermissions {
        path: PathBuf,
        permissions: fs::Permissions,
    }

    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            if let Err(error) = fs::set_permissions(&self.path, self.permissions.clone()) {
                eprintln!(
                    "failed to restore permissions on {}: {error}",
                    self.path.display()
                );
            }
        }
    }

    let app_dir = work.join("read-only App");
    let data_dir = app_dir.join("data");
    fs::create_dir_all(&data_dir)?;
    let target = data_dir.join("assets-000.dat");
    if let Err(error) = fs::hard_link(fixture, &target) {
        eprintln!("could not hard-link DAT fixture ({error}); copying it instead");
        fs::copy(fixture, &target)?;
    }
    let original = fs::metadata(&data_dir)?.permissions();
    let _restore = RestorePermissions {
        path: data_dir.clone(),
        permissions: original,
    };
    fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o555))?;
    let writable_check = data_dir.join("unexpected-writable-file");
    if OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&writable_check)
        .is_ok()
    {
        fs::remove_file(writable_check)?;
        return Err("read-only check requires an unprivileged user".into());
    }

    let output = Command::new(binary)
        .current_dir(cwd)
        .env("APPIMAGE", app_dir.join("flutzPlayer.AppImage"))
        .env("APPDIR", mount)
        .arg("--inspect-dat")
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    require(output.status.success(), "read-only DAT inspection failed")?;
    require(
        stdout.lines().any(|line| line == "DAT files: 1"),
        "read-only DAT was not discovered",
    )?;
    require(
        stdout.lines().any(|line| {
            line.strip_prefix("SoundFonts: ")
                .and_then(|value| value.parse::<usize>().ok())
                .is_some_and(|count| count > 0)
        }),
        "read-only DAT contains no discoverable soundfonts",
    )?;
    require(
        !data_dir.join("preferences.ini").exists(),
        "read-only DAT inspection created preferences",
    )?;
    println!("PASS real DAT remains readable in a read-only data directory");
    Ok(())
}
