#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux_probe::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("desktop_integration_probe requires Linux");
}

#[cfg(target_os = "linux")]
mod linux_probe {
    use std::{
        env,
        error::Error,
        ffi::{OsStr, OsString},
        fs::{self, File, OpenOptions},
        io::{self, Write},
        os::unix::fs::{symlink, MetadataExt, PermissionsExt},
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    type ProbeResult<T> = Result<T, Box<dyn Error>>;

    pub fn run() -> ProbeResult<()> {
        let args: Vec<OsString> = env::args_os().collect();
        if args.first().and_then(|value| Path::new(value).file_name())
            == Some(OsStr::new("update-desktop-database"))
        {
            return database_helper(&args[1..]);
        }
        if let Some(capture) = env::var_os("FLUTZ_DESKTOP_PROBE_CAPTURE") {
            return capture_helper(Path::new(&capture), &args[1..]);
        }

        let binary = fs::canonicalize(args.get(1).ok_or("pass the debug flutzplayer binary")?)?;
        let icon = fs::canonicalize(args.get(2).ok_or("pass the 256x256 AppImage icon")?)?;
        let base = PathBuf::from(args.get(3).ok_or("pass an isolated probe directory")?);
        fs::create_dir_all(&base)?;
        let work = fs::canonicalize(base)?
            .join(format!("desktop-integration-probe-{}", std::process::id()));
        fs::create_dir(&work)?;
        let harness = Harness::new(binary, &icon, work)?;
        harness.check()?;
        println!("desktop_integration_probe: PASS");
        Ok(())
    }

    fn database_helper(args: &[OsString]) -> ProbeResult<()> {
        let log = env::var_os("FLUTZ_DESKTOP_PROBE_DB_LOG").ok_or("missing database log")?;
        let mut file = OpenOptions::new().create(true).append(true).open(log)?;
        writeln!(file, "{}", args.len())?;
        if env::var_os("FLUTZ_DESKTOP_PROBE_DB_FAIL").is_some() {
            std::process::exit(7);
        }
        Ok(())
    }

    fn capture_helper(path: &Path, args: &[OsString]) -> ProbeResult<()> {
        let arguments = args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let capture = serde_json::json!({
            "arguments": arguments,
            "cwd": env::current_dir()?.to_string_lossy(),
        });
        flutz_fs_sdl3::atomic_write(path, &serde_json::to_vec(&capture)?)?;
        Ok(())
    }

    struct Harness {
        binary: PathBuf,
        work: PathBuf,
        cwd: PathBuf,
        appdir: PathBuf,
        source_icon: PathBuf,
        home: PathBuf,
        data_dir: PathBuf,
        spy_path: OsString,
        database_log: PathBuf,
        executable: PathBuf,
    }

    struct GuiOptions<'a> {
        name: &'a str,
        image: Option<&'a Path>,
        appdir: Option<&'a Path>,
        xdg_data_home: Option<&'a OsStr>,
        home: &'a Path,
        path: &'a OsStr,
        fail_database: bool,
    }

    impl Harness {
        fn new(binary: PathBuf, icon: &Path, work: PathBuf) -> ProbeResult<Self> {
            let cwd = work.join("unrelated working directory");
            let appdir = work.join("AppImage mount");
            let source_icon = appdir.join("usr/share/icons/hicolor/256x256/apps/flutzplayer.png");
            let home = work.join("test home");
            let data_dir = work.join("application data");
            let tools = work.join("probe utilities");
            for directory in [&cwd, &home, &data_dir, &tools] {
                fs::create_dir_all(directory)?;
            }
            fs::create_dir_all(source_icon.parent().ok_or("icon has no parent")?)?;
            fs::copy(icon, &source_icon)?;
            let executable = env::current_exe()?;
            symlink(&executable, tools.join("update-desktop-database"))?;
            let system_path = env::var_os("PATH").ok_or("PATH is not set")?;
            let spy_path =
                env::join_paths(std::iter::once(tools).chain(env::split_paths(&system_path)))?;
            let database_log = work.join("database-calls.log");
            Ok(Self {
                binary,
                work,
                cwd,
                appdir,
                source_icon,
                home,
                data_dir,
                spy_path,
                database_log,
                executable,
            })
        }

        fn check(&self) -> ProbeResult<()> {
            let app_dir = self
                .work
                .join("app \"quoted\" $dollar `tick folder \\backslash");
            fs::create_dir_all(&app_dir)?;
            let image = app_dir.join("flutz \"player\" $money `tick file \\part.AppImage");
            symlink(&self.executable, &image)?;
            let xdg = self
                .work
                .join("XDG \"quoted\" $dollar `tick %folder \\backslash");
            let desktop = desktop_path(&xdg);
            let icon = icon_path(&xdg);

            let development_xdg = self.work.join("development xdg");
            self.gui(GuiOptions {
                name: "development",
                image: None,
                appdir: Some(&self.appdir),
                xdg_data_home: Some(development_xdg.as_os_str()),
                home: &self.home,
                path: &self.spy_path,
                fail_database: false,
            })?;
            require(
                !development_xdg.exists(),
                "development GUI created integration",
            )?;
            self.check_tool_launch(&image, &development_xdg)?;
            require(
                database_calls(&self.database_log)? == 0,
                "non-GUI refreshed database",
            )?;
            println!("PASS development and diagnostic launches do not integrate");

            self.gui(self.options("first", &image, &xdg))?;
            require(
                desktop.is_file(),
                "first GUI launch did not create desktop entry",
            )?;
            require(
                fs::read(&icon)? == fs::read(&self.source_icon)?,
                "first GUI launch did not install the embedded icon",
            )?;
            require(
                database_calls(&self.database_log)? == 1,
                "first entry did not refresh database",
            )?;
            validate_desktop(&desktop)?;
            self.check_desktop_launch(&desktop, &app_dir, "first")?;
            println!("PASS XDG override, desktop validation, escaped launch, and file argument");

            let desktop_inode = fs::metadata(&desktop)?.ino();
            let icon_inode = fs::metadata(&icon)?.ino();
            self.gui(self.options("unchanged", &image, &xdg))?;
            require(
                fs::metadata(&desktop)?.ino() == desktop_inode,
                "unchanged entry was replaced",
            )?;
            require(
                fs::metadata(&icon)?.ino() == icon_inode,
                "unchanged icon was replaced",
            )?;
            require(
                database_calls(&self.database_log)? == 1,
                "unchanged entry refreshed database",
            )?;
            println!("PASS unchanged integration does not rewrite files or refresh the database");

            let mut custom = fs::read_to_string(&desktop)?;
            custom.push_str(
                "X-User-Note=keep this\nName[fr]=Lecteur\nActions=Probe;\n\n\
                 [Desktop Action Probe]\nName=Probe\nExec=/bin/true\n",
            );
            fs::write(&desktop, custom.as_bytes())?;
            let custom_inode = fs::metadata(&desktop)?.ino();
            self.gui(self.options("custom-unchanged", &image, &xdg))?;
            require(
                fs::metadata(&desktop)?.ino() == custom_inode,
                "user keys forced a rewrite",
            )?;
            require(
                database_calls(&self.database_log)? == 1,
                "user keys refreshed database",
            )?;

            let moved_dir = self.work.join("moved \"app\" $directory new \\path");
            fs::create_dir(&moved_dir)?;
            let moved_image = moved_dir.join(image.file_name().ok_or("image has no file name")?);
            fs::rename(&image, &moved_image)?;
            self.gui(self.options("moved", &moved_image, &xdg))?;
            let moved_entry = fs::read_to_string(&desktop)?;
            require(
                moved_entry.contains("X-User-Note=keep this\n"),
                "custom key was lost",
            )?;
            require(
                moved_entry.contains("Name[fr]=Lecteur\n"),
                "localized key was lost",
            )?;
            require(
                moved_entry.contains("[Desktop Action Probe]\nName=Probe\nExec=/bin/true\n"),
                "custom desktop action was lost",
            )?;
            require(
                fs::metadata(&icon)?.ino() == icon_inode,
                "move replaced unchanged icon",
            )?;
            require(
                database_calls(&self.database_log)? == 2,
                "move did not refresh database once",
            )?;
            validate_desktop(&desktop)?;
            self.check_desktop_launch(&desktop, &moved_dir, "moved")?;
            println!("PASS moved AppImage updates owned keys and preserves user additions");

            let original_icon = fs::read(&self.source_icon)?;
            let mut altered_icon = original_icon.clone();
            let last = altered_icon.last_mut().ok_or("empty source icon")?;
            *last ^= 1;
            fs::write(&self.source_icon, &altered_icon)?;
            self.gui(self.options("icon-changed", &moved_image, &xdg))?;
            require(
                fs::read(&icon)? == altered_icon,
                "same-length icon change was missed",
            )?;
            require(
                database_calls(&self.database_log)? == 2,
                "icon-only change refreshed database",
            )?;
            fs::write(&self.source_icon, &original_icon)?;
            self.gui(self.options("icon-restored", &moved_image, &xdg))?;
            require(fs::read(&icon)? == original_icon, "icon was not restored")?;
            require(
                database_calls(&self.database_log)? == 2,
                "icon restore refreshed database",
            )?;
            println!("PASS icon contents, not file length, control replacement");

            self.check_home_fallback(&moved_image)?;
            self.check_database_failures(&moved_image)?;
            self.check_read_only_xdg(&moved_image)?;
            self.check_invalid_environment(&moved_image)?;
            self.check_unsupported_image_paths()?;
            self.check_malformed_entry(&moved_image, &desktop)?;
            Ok(())
        }

        fn options<'a>(&'a self, name: &'a str, image: &'a Path, xdg: &'a Path) -> GuiOptions<'a> {
            GuiOptions {
                name,
                image: Some(image),
                appdir: Some(&self.appdir),
                xdg_data_home: Some(xdg.as_os_str()),
                home: &self.home,
                path: &self.spy_path,
                fail_database: false,
            }
        }

        fn gui(&self, options: GuiOptions<'_>) -> ProbeResult<String> {
            let log_path = self.work.join(format!("gui-{}.log", options.name));
            let log = File::create(&log_path)?;
            let mut command = Command::new(&self.binary);
            command
                .arg("--gui")
                .arg("--data-dir")
                .arg(&self.data_dir)
                .current_dir(&self.cwd)
                .env_remove("APPIMAGE")
                .env_remove("APPDIR")
                .env_remove("XDG_DATA_HOME")
                .env_remove("FLUTZ_DESKTOP_PROBE_CAPTURE")
                .env_remove("FLUTZ_DESKTOP_PROBE_DB_FAIL")
                .env("HOME", options.home)
                .env("PATH", options.path)
                .env("SDL_AUDIODRIVER", "dummy")
                .env("FLUTZ_DESKTOP_PROBE_DB_LOG", &self.database_log)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(log));
            if let Some(image) = options.image {
                command.env("APPIMAGE", image);
            }
            if let Some(appdir) = options.appdir {
                command.env("APPDIR", appdir);
            }
            if let Some(xdg) = options.xdg_data_home {
                command.env("XDG_DATA_HOME", xdg);
            }
            if options.fail_database {
                command.env("FLUTZ_DESKTOP_PROBE_DB_FAIL", "1");
            }
            let mut process = RunningGui(command.spawn()?);
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Some(status) = process.0.try_wait()? {
                    return Err(format!(
                        "{} GUI exited with {status}: {}",
                        options.name,
                        fs::read_to_string(&log_path)?
                    )
                    .into());
                }
                let window = Command::new("xdotool")
                    .args(["search", "--onlyvisible", "--pid"])
                    .arg(process.0.id().to_string())
                    .output()?;
                if window.status.success() && !window.stdout.is_empty() {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(format!(
                        "{} GUI did not create a visible window: {}",
                        options.name,
                        fs::read_to_string(&log_path)?
                    )
                    .into());
                }
                thread::sleep(Duration::from_millis(100));
            }
            process.0.kill()?;
            process.0.wait()?;
            Ok(fs::read_to_string(log_path)?)
        }

        fn check_tool_launch(&self, image: &Path, xdg: &Path) -> ProbeResult<()> {
            let output = Command::new(&self.binary)
                .arg("--debug-env")
                .arg("--data-dir")
                .arg(&self.data_dir)
                .current_dir(&self.cwd)
                .env("APPIMAGE", image)
                .env("APPDIR", &self.appdir)
                .env("XDG_DATA_HOME", xdg)
                .env("PATH", &self.spy_path)
                .env("FLUTZ_DESKTOP_PROBE_DB_LOG", &self.database_log)
                .output()?;
            require(output.status.success(), "diagnostic launch failed")?;
            require(!xdg.exists(), "diagnostic launch created integration")
        }

        fn check_desktop_launch(
            &self,
            desktop: &Path,
            expected_cwd: &Path,
            name: &str,
        ) -> ProbeResult<()> {
            let input = self.work.join("song \"quoted\" $special %file.mid");
            fs::write(&input, b"probe fixture")?;
            let capture = self.work.join(format!("capture-{name}.json"));
            let output = Command::new("gio")
                .arg("launch")
                .arg(desktop)
                .arg(&input)
                .current_dir(&self.cwd)
                .env("FLUTZ_DESKTOP_PROBE_CAPTURE", &capture)
                .output()?;
            require(
                output.status.success(),
                &format!(
                    "gio launch failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            )?;
            let deadline = Instant::now() + Duration::from_secs(15);
            while !capture.is_file() {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "desktop launch did not invoke the AppImage: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )
                    .into());
                }
                thread::sleep(Duration::from_millis(50));
            }
            let result: serde_json::Value = serde_json::from_slice(&fs::read(capture)?)?;
            let expected_input = input.to_str().ok_or("input path is not UTF-8")?;
            require(
                result["arguments"] == serde_json::json!(["--gui", expected_input]),
                &format!("incorrect desktop arguments: {}", result["arguments"]),
            )?;
            require(
                result["cwd"] == expected_cwd.to_str().ok_or("cwd is not UTF-8")?,
                &format!("incorrect desktop working directory: {}", result["cwd"]),
            )
        }

        fn check_home_fallback(&self, image: &Path) -> ProbeResult<()> {
            let home = self.work.join("fallback home");
            fs::create_dir(&home)?;
            self.gui(GuiOptions {
                name: "home-fallback",
                image: Some(image),
                appdir: Some(&self.appdir),
                xdg_data_home: None,
                home: &home,
                path: &self.spy_path,
                fail_database: false,
            })?;
            require(
                desktop_path(&home.join(".local/share")).is_file(),
                "HOME fallback missing",
            )?;

            let relative_home = self.work.join("relative-xdg home");
            fs::create_dir(&relative_home)?;
            let log = self.gui(GuiOptions {
                name: "relative-xdg",
                image: Some(image),
                appdir: Some(&self.appdir),
                xdg_data_home: Some(OsStr::new("relative-xdg")),
                home: &relative_home,
                path: &self.spy_path,
                fail_database: false,
            })?;
            require(
                desktop_path(&relative_home.join(".local/share")).is_file(),
                "relative XDG_DATA_HOME did not fall back to HOME",
            )?;
            require(
                !self.cwd.join("relative-xdg").exists(),
                "relative XDG path was used",
            )?;
            require(
                log.contains("ignoring relative XDG_DATA_HOME"),
                "relative XDG was not logged",
            )?;
            println!("PASS HOME fallback and invalid relative XDG_DATA_HOME");
            Ok(())
        }

        fn check_database_failures(&self, image: &Path) -> ProbeResult<()> {
            let absent_tools = self.work.join("no utilities");
            fs::create_dir(&absent_tools)?;
            let absent_xdg = self.work.join("missing utility xdg");
            let log = self.gui(GuiOptions {
                name: "missing-database",
                image: Some(image),
                appdir: Some(&self.appdir),
                xdg_data_home: Some(absent_xdg.as_os_str()),
                home: &self.home,
                path: absent_tools.as_os_str(),
                fail_database: false,
            })?;
            require(
                desktop_path(&absent_xdg).is_file(),
                "missing utility blocked integration",
            )?;
            require(
                log.contains("could not run update-desktop-database"),
                "missing utility not logged",
            )?;

            let failing_xdg = self.work.join("failing utility xdg");
            let log = self.gui(GuiOptions {
                name: "failing-database",
                image: Some(image),
                appdir: Some(&self.appdir),
                xdg_data_home: Some(failing_xdg.as_os_str()),
                home: &self.home,
                path: &self.spy_path,
                fail_database: true,
            })?;
            require(
                desktop_path(&failing_xdg).is_file(),
                "failing utility blocked integration",
            )?;
            require(
                log.contains("update-desktop-database failed"),
                "utility failure not logged",
            )?;
            println!("PASS missing and failing desktop-database utilities do not block the GUI");
            Ok(())
        }

        fn check_read_only_xdg(&self, image: &Path) -> ProbeResult<()> {
            let xdg = self.work.join("read-only xdg");
            fs::create_dir(&xdg)?;
            let original = fs::metadata(&xdg)?.permissions();
            let _restore = RestorePermissions {
                path: xdg.clone(),
                permissions: original,
            };
            fs::set_permissions(&xdg, fs::Permissions::from_mode(0o555))?;
            let writable = xdg.join("should-not-exist");
            if OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&writable)
                .is_ok()
            {
                fs::remove_file(writable)?;
                return Err("read-only XDG probe requires an unprivileged user".into());
            }
            let log = self.gui(self.options("read-only-xdg", image, &xdg))?;
            require(
                log.contains("Linux desktop integration failed"),
                "XDG failure not logged",
            )?;
            require(
                !desktop_path(&xdg).exists(),
                "read-only XDG unexpectedly changed",
            )?;
            println!("PASS read-only XDG failure is logged and the GUI opens");
            Ok(())
        }

        fn check_invalid_environment(&self, image: &Path) -> ProbeResult<()> {
            let xdg = self.work.join("missing-appdir xdg");
            let log = self.gui(GuiOptions {
                name: "missing-appdir",
                image: Some(image),
                appdir: None,
                xdg_data_home: Some(xdg.as_os_str()),
                home: &self.home,
                path: &self.spy_path,
                fail_database: false,
            })?;
            require(
                log.contains("APPDIR is not set"),
                "missing APPDIR not logged",
            )?;
            require(!xdg.exists(), "missing APPDIR created integration")?;
            let invalid_xdg = self.work.join("invalid-appimage xdg");
            let log = self.gui(GuiOptions {
                name: "invalid-appimage",
                image: Some(Path::new("relative.AppImage")),
                appdir: Some(&self.appdir),
                xdg_data_home: Some(invalid_xdg.as_os_str()),
                home: &self.home,
                path: &self.spy_path,
                fail_database: false,
            })?;
            require(
                log.contains("APPIMAGE must name an absolute"),
                "invalid APPIMAGE not logged",
            )?;
            require(
                !invalid_xdg.exists(),
                "invalid APPIMAGE created integration",
            )?;
            println!("PASS missing APPDIR and invalid APPIMAGE do not block the GUI");
            Ok(())
        }

        fn check_unsupported_image_paths(&self) -> ProbeResult<()> {
            let percent_dir = self.work.join("unsupported%folder");
            fs::create_dir(&percent_dir)?;
            let percent_image = percent_dir.join("flutzPlayer.AppImage");
            let equals_image = self.work.join("flutz=Player.AppImage");
            symlink(&self.executable, &percent_image)?;
            symlink(&self.executable, &equals_image)?;
            let initial_calls = database_calls(&self.database_log)?;
            for (name, image) in [("percent", &percent_image), ("equals", &equals_image)] {
                let xdg = self.work.join(format!("unsupported-{name} xdg"));
                let log = self.gui(self.options(name, image, &xdg))?;
                require(
                    log.contains("direct desktop launch does not support '%' or '='"),
                    "unsupported AppImage path was not reported",
                )?;
                require(
                    !xdg.exists(),
                    "unsupported AppImage path created integration",
                )?;
            }
            require(
                database_calls(&self.database_log)? == initial_calls,
                "unsupported AppImage path refreshed the desktop database",
            )?;
            println!("PASS unsupported percent and equals AppImage paths do not block the GUI");
            Ok(())
        }

        fn check_malformed_entry(&self, image: &Path, source: &Path) -> ProbeResult<()> {
            let xdg = self.work.join("malformed xdg");
            let desktop = desktop_path(&xdg);
            fs::create_dir_all(desktop.parent().ok_or("desktop has no parent")?)?;
            let original = fs::read_to_string(source)?;
            let malformed = original.replacen(
                "Name=flutzPlayer\n",
                "Name=flutzPlayer\nName=duplicate\n",
                1,
            );
            fs::write(&desktop, &malformed)?;
            let log = self.gui(self.options("malformed", image, &xdg))?;
            require(
                log.contains("duplicate Name keys"),
                "malformed entry not reported",
            )?;
            require(
                fs::read_to_string(desktop)? == malformed,
                "malformed entry was overwritten",
            )?;
            require(
                !icon_path(&xdg).exists(),
                "malformed entry partially installed an icon",
            )?;
            println!("PASS ambiguous existing entries are preserved and reported");
            Ok(())
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

    fn desktop_path(xdg: &Path) -> PathBuf {
        xdg.join("applications/flutzplayer.desktop")
    }

    fn icon_path(xdg: &Path) -> PathBuf {
        xdg.join("icons/hicolor/256x256/apps/flutzplayer.png")
    }

    fn database_calls(path: &Path) -> io::Result<usize> {
        match fs::read_to_string(path) {
            Ok(contents) => Ok(contents.lines().count()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error),
        }
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
