#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux_probe::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("appimage_desktop_probe requires Linux");
}

#[cfg(target_os = "linux")]
mod linux_probe {
    use std::{
        env,
        error::Error,
        fs::{self, File, OpenOptions},
        os::unix::{ffi::OsStrExt, fs::PermissionsExt},
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    type ProbeResult<T> = Result<T, Box<dyn Error>>;

    pub fn run() -> ProbeResult<()> {
        let args: Vec<_> = env::args_os().collect();
        let original_image = fs::canonicalize(args.get(1).ok_or("pass a real AppImage")?)?;
        let generic_entry = fs::canonicalize(args.get(2).ok_or("pass the generic desktop entry")?)?;
        let icon = fs::canonicalize(args.get(3).ok_or("pass the 256x256 icon")?)?;
        let base = PathBuf::from(args.get(4).ok_or("pass an isolated probe directory")?);
        fs::create_dir_all(&base)?;
        let work =
            fs::canonicalize(base)?.join(format!("appimage-desktop-probe-{}", std::process::id()));
        fs::create_dir(&work)?;
        let original_dir = work.join("original AppImage folder");
        let data = original_dir.join("data");
        let cwd = work.join("unrelated working directory");
        let xdg = work.join("isolated XDG");
        fs::create_dir_all(&data)?;
        fs::create_dir(&cwd)?;
        let image = original_dir.join("flutzPlayer-x86_64.AppImage");
        fs::copy(&original_image, &image)?;
        fs::set_permissions(&image, fs::metadata(&original_image)?.permissions())?;
        let source_data = original_image
            .parent()
            .ok_or("AppImage has no parent")?
            .join("data");
        let mut dat_count = 0;
        for item in fs::read_dir(source_data)? {
            let item = item?;
            let name = item.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("assets-") && name.ends_with(".dat") && item.file_type()?.is_file()
            {
                let destination = data.join(item.file_name());
                if let Err(error) = fs::hard_link(item.path(), &destination) {
                    eprintln!(
                        "could not hard-link {} ({error}); copying",
                        item.path().display()
                    );
                    fs::copy(item.path(), destination)?;
                }
                dat_count += 1;
            }
        }
        require(
            dat_count > 0,
            "no real DAT files were supplied next to the AppImage",
        )?;
        inspect_appimage(&image, &cwd, &xdg, dat_count)?;
        inspect_embedded_resources(&image, &generic_entry, &icon, &work)?;
        require(
            !xdg.exists(),
            "tool or extraction launch registered a desktop entry",
        )?;

        let desktop = xdg.join("applications/flutzplayer.desktop");
        let installed_icon = xdg.join("icons/hicolor/256x256/apps/flutzplayer.png");
        run_gui(&image, &cwd, &xdg, &work, "direct", false)?;
        require(
            desktop.is_file(),
            "real AppImage did not install its desktop entry",
        )?;
        require(
            fs::read(&installed_icon)? == fs::read(&icon)?,
            "real AppImage installed the wrong icon",
        )?;
        require(
            data.join("preferences.ini").is_file(),
            "direct GUI did not create adjacent preferences",
        )?;
        validate_desktop(&desktop)?;
        let desktop_inode = inode(&desktop)?;
        let icon_inode = inode(&installed_icon)?;
        println!("PASS real direct AppImage installs a validated entry and adjacent preferences");

        run_gui(&image, &cwd, &xdg, &work, "extract", true)?;
        require(
            inode(&desktop)? == desktop_inode,
            "extraction launch rewrote unchanged entry",
        )?;
        require(
            inode(&installed_icon)? == icon_inode,
            "extraction launch rewrote unchanged icon",
        )?;
        println!("PASS extraction fallback preserves unchanged desktop integration");

        let moved_dir = work.join("moved AppImage folder");
        fs::rename(&original_dir, &moved_dir)?;
        let moved_image = moved_dir.join("flutzPlayer-x86_64.AppImage");
        let old_entry = fs::read_to_string(&desktop)?;
        require(
            old_entry.contains(image.to_str().ok_or("image path is not UTF-8")?),
            "old launcher did not retain the old location",
        )?;
        run_gui(&moved_image, &cwd, &xdg, &work, "moved", false)?;
        let moved_entry = fs::read_to_string(&desktop)?;
        require(
            moved_entry.contains(moved_image.to_str().ok_or("moved path is not UTF-8")?),
            "moved AppImage did not update Exec",
        )?;
        require(
            !moved_entry.contains(image.to_str().ok_or("old path is not UTF-8")?),
            "moved entry still contains old Exec",
        )?;
        require(
            inode(&desktop)? != desktop_inode,
            "moved entry was not replaced",
        )?;
        require(
            inode(&installed_icon)? == icon_inode,
            "move replaced the unchanged icon",
        )?;
        validate_desktop(&desktop)?;
        println!("PASS direct launch after a move updates the real desktop entry");

        let preferences = moved_dir.join("data/preferences.ini");
        fs::remove_file(&preferences)?;
        launch_from_desktop(&desktop, &moved_image, &moved_dir, &cwd, &xdg, &preferences)?;
        println!("PASS GIO launcher starts the real AppImage and finds its adjacent data");

        let read_only = work.join("read-only XDG");
        fs::create_dir(&read_only)?;
        let restore = RestorePermissions {
            path: read_only.clone(),
            permissions: fs::metadata(&read_only)?.permissions(),
        };
        fs::set_permissions(&read_only, fs::Permissions::from_mode(0o555))?;
        let unexpected = read_only.join("unexpected");
        if OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&unexpected)
            .is_ok()
        {
            fs::remove_file(unexpected)?;
            return Err("read-only XDG probe requires an unprivileged user".into());
        }
        let log = run_gui(&moved_image, &cwd, &read_only, &work, "read-only", false)?;
        require(
            log.contains("Linux desktop integration failed"),
            "real AppImage did not log integration failure",
        )?;
        drop(restore);
        println!("PASS real AppImage opens its GUI when integration fails");
        println!("appimage_desktop_probe: PASS");
        Ok(())
    }

    fn inspect_appimage(image: &Path, cwd: &Path, xdg: &Path, count: usize) -> ProbeResult<()> {
        let output = Command::new(image)
            .arg("--inspect-dat")
            .current_dir(cwd)
            .env_remove("APPIMAGE")
            .env_remove("APPDIR")
            .env("XDG_DATA_HOME", xdg)
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        require(
            output.status.success(),
            &format!(
                "AppImage DAT inspection failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        )?;
        require(
            stdout
                .lines()
                .any(|line| line == format!("DAT files: {count}")),
            "AppImage did not find its full adjacent DAT fileset",
        )?;
        require(
            stdout.lines().any(|line| {
                line.strip_prefix("SoundFonts: ")
                    .and_then(|value| value.parse::<usize>().ok())
                    .is_some_and(|value| value > 0)
            }),
            "AppImage did not find soundfonts in its adjacent DAT files",
        )
    }

    fn inspect_embedded_resources(
        image: &Path,
        generic: &Path,
        icon: &Path,
        work: &Path,
    ) -> ProbeResult<()> {
        let directory = work.join("extracted payload");
        fs::create_dir(&directory)?;
        let output = Command::new(image)
            .arg("--appimage-extract")
            .current_dir(&directory)
            .output()?;
        require(
            output.status.success(),
            &format!(
                "AppImage extraction failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        )?;
        let appdir = directory.join("squashfs-root");
        let embedded_entry = appdir.join("flutzplayer.desktop");
        let embedded_icon = appdir.join("usr/share/icons/hicolor/256x256/apps/flutzplayer.png");
        require(
            fs::read(&embedded_entry)? == fs::read(generic)?,
            "embedded desktop entry differs from its source",
        )?;
        require(
            fs::read(&embedded_icon)? == fs::read(icon)?,
            "embedded icon differs from its source",
        )?;
        validate_desktop(&embedded_entry)?;
        println!("PASS real AppImage embeds the validated generic entry and 256px icon");
        Ok(())
    }

    fn run_gui(
        image: &Path,
        cwd: &Path,
        xdg: &Path,
        work: &Path,
        name: &str,
        extract: bool,
    ) -> ProbeResult<String> {
        let log_path = work.join(format!("{name}.log"));
        let mut command = Command::new(image);
        if extract {
            command.arg("--appimage-extract-and-run");
        }
        let mut process = RunningGui(
            command
                .arg("--gui")
                .current_dir(cwd)
                .env_remove("APPIMAGE")
                .env_remove("APPDIR")
                .env_remove("APPIMAGE_EXTRACT_AND_RUN")
                .env("XDG_DATA_HOME", xdg)
                .env("SDL_AUDIODRIVER", "dummy")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(File::create(&log_path)?))
                .spawn()?,
        );
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(status) = process.0.try_wait()? {
                return Err(format!(
                    "{name} GUI exited with {status}: {}",
                    fs::read_to_string(&log_path)?
                )
                .into());
            }
            if let Some(pid) = appimage_window_pid(image)? {
                if pid != process.0.id() {
                    terminate_gui(pid)?;
                    let exit_deadline = Instant::now() + Duration::from_secs(5);
                    while process.0.try_wait()?.is_none() {
                        if Instant::now() >= exit_deadline {
                            process.0.kill()?;
                            break;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                } else {
                    process.0.kill()?;
                }
                process.0.wait()?;
                return Ok(fs::read_to_string(log_path)?);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{name} GUI did not create a visible window: {}",
                    fs::read_to_string(&log_path)?
                )
                .into());
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn launch_from_desktop(
        desktop: &Path,
        image: &Path,
        expected_cwd: &Path,
        cwd: &Path,
        xdg: &Path,
        preferences: &Path,
    ) -> ProbeResult<()> {
        let log_path = cwd.join("gio-launch.log");
        let status = Command::new("gio")
            .arg("launch")
            .arg(desktop)
            .current_dir(cwd)
            .env_remove("APPIMAGE")
            .env_remove("APPDIR")
            .env("XDG_DATA_HOME", xdg)
            .env("SDL_AUDIODRIVER", "dummy")
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&log_path)?))
            .status()?;
        require(
            status.success(),
            &format!("GIO launch failed: {}", fs::read_to_string(&log_path)?),
        )?;
        let deadline = Instant::now() + Duration::from_secs(45);
        let pid = loop {
            if let Some(pid) = appimage_window_pid(image)? {
                break pid;
            }
            if Instant::now() >= deadline {
                return Err("GIO did not open a window for the expected AppImage".into());
            }
            thread::sleep(Duration::from_millis(100));
        };
        let _process = DesktopProcess(pid);
        require(
            fs::canonicalize(format!("/proc/{pid}/cwd"))? == fs::canonicalize(expected_cwd)?,
            "desktop entry did not use the AppImage directory as its working directory",
        )?;
        require(
            preferences.is_file(),
            "desktop launch did not create adjacent preferences",
        )
    }

    fn appimage_window_pid(image: &Path) -> ProbeResult<Option<u32>> {
        let output = Command::new("xdotool")
            .args(["search", "--onlyvisible", "--name", "flutzPlayer"])
            .output()?;
        if !output.status.success() {
            return Ok(None);
        }
        let expected = [b"APPIMAGE=".as_slice(), image.as_os_str().as_bytes()].concat();
        for window in String::from_utf8_lossy(&output.stdout).lines() {
            let output = Command::new("xdotool")
                .args(["getwindowpid", window])
                .output()?;
            if !output.status.success() {
                continue;
            }
            let pid = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u32>()?;
            let environment = match fs::read(format!("/proc/{pid}/environ")) {
                Ok(environment) => environment,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if environment
                .split(|byte| *byte == 0)
                .any(|value| value == expected)
            {
                return Ok(Some(pid));
            }
        }
        Ok(None)
    }

    fn terminate_gui(pid: u32) -> std::io::Result<()> {
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "failed to stop probe GUI {pid}: {status}"
            )))
        }
    }

    struct RunningGui(Child);

    impl Drop for RunningGui {
        fn drop(&mut self) {
            match self.0.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if let Err(error) = self.0.kill() {
                        eprintln!("failed to stop probe GUI {}: {error}", self.0.id());
                    }
                    if let Err(error) = self.0.wait() {
                        eprintln!("failed to reap probe GUI {}: {error}", self.0.id());
                    }
                }
                Err(error) => eprintln!("failed to inspect probe GUI {}: {error}", self.0.id()),
            }
        }
    }

    struct DesktopProcess(u32);

    impl Drop for DesktopProcess {
        fn drop(&mut self) {
            if let Err(error) = terminate_gui(self.0) {
                eprintln!("{error}");
            }
        }
    }

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

    fn inode(path: &Path) -> ProbeResult<u64> {
        use std::os::unix::fs::MetadataExt;
        Ok(fs::metadata(path)?.ino())
    }

    fn validate_desktop(path: &Path) -> ProbeResult<()> {
        let output = Command::new("desktop-file-validate").arg(path).output()?;
        require(
            output.status.success(),
            &format!(
                "desktop-file-validate failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )
    }

    fn require(condition: bool, message: &str) -> ProbeResult<()> {
        if condition {
            Ok(())
        } else {
            Err(format!("FAIL {message}").into())
        }
    }
}
