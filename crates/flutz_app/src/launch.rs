use super::dat_startup::{inspect_dat_fileset, summarize_dat_data_dir};
use super::*;

pub(super) fn run() -> Result<()> {
    #[cfg(windows)]
    let _ = windows_file_association::ensure_file_associations();

    let debug_memory_requested = env::args().skip(1).any(|arg| arg == "--debug-memory");
    memory_runtime::initialize_from_env(debug_memory_requested);

    let args = env::args().skip(1).collect::<Vec<_>>();
    let launched_from_terminal = launched_from_terminal();
    let launch_mode = LaunchMode::parse(&args, launched_from_terminal)?;

    #[cfg(windows)]
    if matches!(&launch_mode, LaunchMode::Gui(_)) {
        let _ = flutz_media_controls::register_media_control_jump_list_tasks();
    }

    match launch_mode {
        LaunchMode::Gui(config) => {
            #[cfg(target_os = "linux")]
            linux_desktop_integration::integrate_best_effort();
            app::run_gui(config)
        }
        LaunchMode::Tool(command) => run_tool(command),
    }
}

fn launched_from_terminal() -> bool {
    let stdio_terminal = std::io::stdout().is_terminal() || std::io::stderr().is_terminal();

    #[cfg(windows)]
    {
        if !stdio_terminal {
            return false;
        }

        return match windows_parent_process_name() {
            Some(parent) if parent.eq_ignore_ascii_case("explorer.exe") => false,
            Some(parent) if is_windows_terminal_parent(&parent) => true,
            Some(_) => windows_console_process_count().map_or(stdio_terminal, |count| count > 2),
            None => windows_console_process_count().map_or(stdio_terminal, |count| count > 2),
        };
    }

    #[cfg(not(windows))]
    {
        stdio_terminal
    }
}

#[cfg(windows)]
fn is_windows_terminal_parent(parent_exe_name: &str) -> bool {
    matches!(
        parent_exe_name.to_ascii_lowercase().as_str(),
        "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "windowsterminal.exe"
            | "wt.exe"
            | "conhost.exe"
            | "bash.exe"
            | "wsl.exe"
            | "nushell.exe"
    )
}

#[cfg(windows)]
fn windows_parent_process_name() -> Option<String> {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }

    let current_pid = unsafe { GetCurrentProcessId() };
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;

    let mut current_parent_pid = None;
    let mut parent_name = None;

    let has_first = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    if has_first {
        loop {
            if entry.th32ProcessID == current_pid {
                current_parent_pid = Some(entry.th32ParentProcessID);
                break;
            }

            let has_next = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
            if !has_next {
                break;
            }
        }

        if let Some(parent_pid) = current_parent_pid {
            let mut parent_entry: PROCESSENTRY32W = unsafe { zeroed() };
            parent_entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let has_parent_first = unsafe { Process32FirstW(snapshot, &mut parent_entry) } != 0;
            if has_parent_first {
                loop {
                    if parent_entry.th32ProcessID == parent_pid {
                        parent_name = Some(windows_utf16_name(&parent_entry.szExeFile));
                        break;
                    }

                    let has_next = unsafe { Process32NextW(snapshot, &mut parent_entry) } != 0;
                    if !has_next {
                        break;
                    }
                }
            }
        }
    }

    unsafe {
        CloseHandle(snapshot);
    }

    parent_name
}

#[cfg(windows)]
fn windows_console_process_count() -> Option<u32> {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;

    let mut pids = [0u32; 64];
    let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    if count == 0 {
        None
    } else {
        Some(count)
    }
}

#[cfg(windows)]
fn windows_utf16_name(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

enum LaunchMode {
    Gui(app::AppStartupConfig),
    Tool(ToolCommand),
}

enum ToolCommand {
    Help(PathBuf),
    DebugEnvironment(PathBuf),
    MediaCommand(flutz_media_controls::MediaControlCommand),
    Verify(PathBuf),
    InspectDat(PathBuf),
    CheckPlayback {
        data_dir: PathBuf,
        midi_path: PathBuf,
        debug_render_errors: bool,
    },
}

impl LaunchMode {
    fn parse(args: &[String], launched_from_terminal: bool) -> Result<Self> {
        let ParsedArgs {
            command,
            data_dir,
            audio_backend,
            debug_memory,
            debug_latency,
            debug_analyzer,
            debug_render_errors,
            startup_path,
        } = ParsedArgs::parse(args)?;
        let data_dir = match data_dir {
            Some(data_dir) => data_dir,
            None => default_data_dir()?,
        };
        let audio_backend = match audio_backend {
            Some(audio_backend) => audio_backend,
            None => audio_backend_from_env()?.unwrap_or_default(),
        };

        if command.is_none() {
            return Ok(if let Some(startup_path) = startup_path {
                Self::Gui(startup_config(
                    data_dir,
                    audio_backend,
                    debug_memory,
                    debug_latency,
                    debug_analyzer,
                    debug_render_errors,
                    Some(startup_path),
                ))
            } else if launched_from_terminal {
                Self::Tool(ToolCommand::Help(data_dir))
            } else {
                Self::Gui(startup_config(
                    data_dir,
                    audio_backend,
                    debug_memory,
                    debug_latency,
                    debug_analyzer,
                    debug_render_errors,
                    None,
                ))
            });
        }

        match command.expect("checked above") {
            ParsedCommand::Gui => Ok(Self::Gui(startup_config(
                data_dir,
                audio_backend,
                debug_memory,
                debug_latency,
                debug_analyzer,
                debug_render_errors,
                startup_path,
            ))),
            ParsedCommand::Headless => {
                reject_startup_path(startup_path.as_deref(), "--headless")?;
                Ok(Self::Tool(ToolCommand::Help(data_dir)))
            }
            ParsedCommand::Help => {
                reject_startup_path(startup_path.as_deref(), "--help")?;
                Ok(Self::Tool(ToolCommand::Help(data_dir)))
            }
            ParsedCommand::DebugEnvironment => {
                reject_startup_path(startup_path.as_deref(), "--debug-env")?;
                Ok(Self::Tool(ToolCommand::DebugEnvironment(data_dir)))
            }
            ParsedCommand::MediaCommand(command) => {
                reject_startup_path(
                    startup_path.as_deref(),
                    flutz_media_controls::MEDIA_COMMAND_ARGUMENT_NAME,
                )?;
                Ok(Self::Tool(ToolCommand::MediaCommand(command)))
            }
            ParsedCommand::Verify(path) => {
                reject_startup_path(startup_path.as_deref(), "--verify")?;
                Ok(Self::Tool(ToolCommand::Verify(path)))
            }
            ParsedCommand::InspectDat(path) => {
                reject_startup_path(startup_path.as_deref(), "--inspect-dat")?;
                Ok(Self::Tool(ToolCommand::InspectDat(
                    path.unwrap_or(data_dir),
                )))
            }
            ParsedCommand::CheckPlayback(path) => {
                reject_startup_path(startup_path.as_deref(), "--check-playback")?;
                Ok(Self::Tool(ToolCommand::CheckPlayback {
                    data_dir,
                    midi_path: path,
                    debug_render_errors,
                }))
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ParsedArgs {
    command: Option<ParsedCommand>,
    data_dir: Option<PathBuf>,
    audio_backend: Option<playback::AudioBackend>,
    debug_memory: bool,
    debug_latency: bool,
    debug_analyzer: bool,
    debug_render_errors: bool,
    startup_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum ParsedCommand {
    Gui,
    Headless,
    Help,
    DebugEnvironment,
    MediaCommand(flutz_media_controls::MediaControlCommand),
    Verify(PathBuf),
    InspectDat(Option<PathBuf>),
    CheckPlayback(PathBuf),
}

impl ParsedArgs {
    fn parse(args: &[String]) -> Result<Self> {
        let mut parsed = Self::default();
        let mut index = 0usize;
        while index < args.len() {
            match args[index].as_str() {
                "--data-dir" | "--dat-dir" => {
                    index += 1;
                    let value = args.get(index).ok_or_else(|| {
                        FlutzError::InvalidInput("--data-dir requires a path".to_owned())
                    })?;
                    parsed.data_dir = Some(PathBuf::from(value));
                }
                "--audio-backend" => {
                    index += 1;
                    let value = args.get(index).ok_or_else(|| {
                        FlutzError::InvalidInput(
                            "--audio-backend requires sdl3 or wasapi".to_owned(),
                        )
                    })?;
                    parsed.audio_backend = Some(playback::AudioBackend::parse(value)?);
                }
                "--debug-memory" => parsed.debug_memory = true,
                "--debug-latency" => parsed.debug_latency = true,
                "--debug-analyzer" => parsed.debug_analyzer = true,
                "--debug-render-errors" => parsed.debug_render_errors = true,
                flutz_media_controls::MEDIA_COMMAND_ARGUMENT_NAME => {
                    index += 1;
                    let value = args.get(index).ok_or_else(|| {
                        FlutzError::InvalidInput(format!(
                            "{} requires play, pause, play-pause, stop, next, or previous",
                            flutz_media_controls::MEDIA_COMMAND_ARGUMENT_NAME
                        ))
                    })?;
                    let command = flutz_media_controls::media_command_from_argument(value)
                        .ok_or_else(|| {
                            FlutzError::InvalidInput(format!(
                                "unknown media command: {value}; expected play, pause, play-pause, stop, next, or previous"
                            ))
                        })?;
                    parsed.set_command(ParsedCommand::MediaCommand(command))?;
                }
                "--gui" => parsed.set_command(ParsedCommand::Gui)?,
                "--headless" => parsed.set_command(ParsedCommand::Headless)?,
                "--help" | "-h" => parsed.set_command(ParsedCommand::Help)?,
                "--debug-env" => parsed.set_command(ParsedCommand::DebugEnvironment)?,
                "--verify" => {
                    index += 1;
                    let value = args.get(index).ok_or_else(|| {
                        FlutzError::InvalidInput("--verify requires a path".to_owned())
                    })?;
                    parsed.set_command(ParsedCommand::Verify(PathBuf::from(value)))?;
                }
                "--inspect-dat" => {
                    let value = args.get(index + 1).filter(|value| !value.starts_with('-'));
                    if value.is_some() {
                        index += 1;
                    }
                    parsed.set_command(ParsedCommand::InspectDat(value.map(PathBuf::from)))?;
                }
                "--check-playback" => {
                    index += 1;
                    let value = args.get(index).ok_or_else(|| {
                        FlutzError::InvalidInput("--check-playback requires a MIDI path".to_owned())
                    })?;
                    parsed.set_command(ParsedCommand::CheckPlayback(PathBuf::from(value)))?;
                }
                flag => {
                    if flag.starts_with('-') {
                        return Err(FlutzError::InvalidInput(format!(
                            "unknown flutzplayer argument: {flag}\n\n{}",
                            tool_help(&default_data_dir()?)
                        )));
                    }

                    parsed.set_startup_path(PathBuf::from(flag))?;
                }
            }
            index += 1;
        }
        Ok(parsed)
    }

    fn set_command(&mut self, command: ParsedCommand) -> Result<()> {
        if self.command.is_some() {
            return Err(FlutzError::InvalidInput(
                "only one startup command may be provided".to_owned(),
            ));
        }
        self.command = Some(command);
        Ok(())
    }

    fn set_startup_path(&mut self, path: PathBuf) -> Result<()> {
        if self.startup_path.is_some() {
            return Err(FlutzError::InvalidInput(
                "only one startup file may be provided".to_owned(),
            ));
        }

        self.startup_path = Some(path);
        Ok(())
    }
}

fn reject_startup_path(startup_path: Option<&Path>, command: &str) -> Result<()> {
    if let Some(path) = startup_path {
        return Err(FlutzError::InvalidInput(format!(
            "{command} does not accept a startup file: {}",
            path.display()
        )));
    }

    Ok(())
}

fn run_tool(command: ToolCommand) -> Result<()> {
    match command {
        ToolCommand::Help(data_dir) => {
            println!("{}", tool_help(&data_dir));
            Ok(())
        }
        ToolCommand::DebugEnvironment(data_dir) => print_debug_environment(&data_dir),
        ToolCommand::MediaCommand(command) => {
            let _ = flutz_media_controls::send_media_command_to_running_instance(command);
            Ok(())
        }
        ToolCommand::Verify(path) => verify_file(&path),
        ToolCommand::InspectDat(path) => inspect_dat_fileset(&path),
        ToolCommand::CheckPlayback {
            data_dir,
            midi_path,
            debug_render_errors,
        } => check_playback(&data_dir, &midi_path, debug_render_errors),
    }
}

fn tool_help(default_data_dir: &Path) -> String {
    let open_inputs = gui_launch_input_summary();
    let playback_inputs = playback_check_input_summary();
    let verify_inputs = verify_input_summary();
    let audio_backends = crate::playback::AudioBackend::supported_values();
    format!(
        "flutzplayer\n\nGUI launch:\n  flutzplayer --gui [{open_inputs}] [--data-dir <dat-directory>] [--audio-backend {audio_backends}] [--debug-memory] [--debug-latency] [--debug-analyzer] [--debug-render-errors]\n  flutzplayer [{open_inputs}]\n\nHeadless/console launch:\n  flutzplayer --headless [--data-dir <dat-directory>]\n\nMedia command relay:\n  flutzplayer --media-command play-pause|stop|next|previous\n\nConsole diagnostics:\n  flutzplayer --verify <{verify_inputs}>\n  flutzplayer --inspect-dat [file.dat|dat-directory]\n  flutzplayer --check-playback <{playback_inputs}> [--data-dir <dat-directory>] [--debug-render-errors]\n  flutzplayer --debug-env\n  flutzplayer --help\n\nData files default to a data folder beside the executable or AppImage:\n  {}\n\nUse --data-dir to point the player at a different DAT data folder. Use --audio-backend or FLUTZ_AUDIO_BACKEND to choose {audio_backends}. Use --debug-memory to enable jemalloc/domain memory snapshots and session log output in debug builds. Use --debug-latency to write sparse latency JSONL in debug builds. Use --debug-analyzer to write spectrum analyzer JSONL under _local/analyzer-trace. Use --debug-render-errors to capture panic-safe render failure traces and MIDI SysEx interpretation detail under _local/render-error-trace in debug-friendly runs. Launch defaults: Explorer/no-terminal starts GUI; terminal/no-args prints this diagnostic surface; a bare project path opens the GUI directly; --headless always prints this diagnostic surface.",
        default_data_dir.display()
    )
}

fn gui_launch_input_summary() -> String {
    let mut extensions = builtin_registry()
        .descriptors()
        .iter()
        .flat_map(|descriptor| {
            descriptor
                .extensions
                .iter()
                .chain(descriptor.wrapped_extensions.iter())
                .map(|extension| format!("file.{extension}"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    extensions.push("playlist.fplist".to_owned());
    extensions.sort();
    extensions.dedup();
    extensions.join("|")
}

fn playback_check_input_summary() -> &'static str {
    "file.fmid|file.mid|file.midi"
}

fn verify_input_summary() -> &'static str {
    "file.fmid|file.mid|file.midi|file.dat"
}

#[cfg(windows)]
pub fn windows_file_association_specs() -> Vec<(String, String, String)> {
    windows_file_association::file_association_specs()
        .into_iter()
        .map(|spec| (spec.extension, spec.prog_id, spec.friendly_type_name))
        .collect()
}

fn check_playback(data_dir: &Path, midi_path: &Path, debug_render_errors: bool) -> Result<()> {
    let config = startup_config(
        data_dir.to_owned(),
        playback::AudioBackend::default(),
        false,
        false,
        false,
        debug_render_errors,
        None,
    );
    let soundfonts = match &config.dat_summary {
        app::DatStartupSummary::Available { soundfonts, .. } => soundfonts.clone(),
        app::DatStartupSummary::Unavailable(message) => {
            return Err(FlutzError::InvalidInput(format!(
                "DAT data is unavailable: {message}"
            )))
        }
    };
    let mut playback = playback::PlaybackController::new(
        config.data_dir,
        soundfonts,
        config.audio_backend,
        config.debug_analyzer,
        config.debug_render_errors,
    );
    let load_message = load_playback_input(&mut playback, midi_path)?;
    let probe = playback.render_probe(512)?;
    println!("{}: playback check OK", midi_path.display());
    println!("{load_message}");
    println!("render_frames: {}", probe.frames);
    println!("render_samples: {}", probe.samples);
    println!("soundfonts: {}", probe.soundfont_count);
    println!("midi_strips: {}", probe.midi_strip_count);
    println!("peak: {:.6}", probe.peak);
    println!("recovered_render_errors: {}", probe.recovered_error_count);
    Ok(())
}

fn load_playback_input(
    playback: &mut playback::PlaybackController,
    input_path: &Path,
) -> Result<String> {
    let bytes = fs::read(input_path).map_err(|error| {
        FlutzError::Runtime(format!("failed to read {}: {error}", input_path.display()))
    })?;
    if bytes.starts_with(flutz_fmid::FMID_MAGIC) {
        let fmid = flutz_fmid::read_fmid(&bytes)?;
        let requested_soundfonts = match &fmid.mixer_source_mode {
            flutz_fmid::MixerSourceMode::Custom => fmid
                .soundfonts
                .iter()
                .map(|slot| slot.internal_id.clone())
                .collect::<Vec<_>>(),
            flutz_fmid::MixerSourceMode::PresetDefault(preset_id) => default_preset_set()
                .find_preset(preset_id)
                .unwrap_or_else(|| default_preset_set().default_preset())
                .font_ids
                .iter()
                .map(|font_id| (*font_id).to_owned())
                .collect::<Vec<_>>(),
        };
        return playback.load_midi_bytes(
            fmid.midi_bytes,
            fmid.project.source_midi_filename,
            &requested_soundfonts,
        );
    }
    playback.load_midi_bytes(
        bytes,
        input_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("playback-check.mid"),
        &[],
    )
}

fn print_debug_environment(data_dir: &Path) -> Result<()> {
    println!("flutzplayer debug environment");
    let current_dir = env::current_dir()
        .map_err(|error| FlutzError::Runtime(format!("failed to read current dir: {error}")))?;
    let executable = env::current_exe()
        .map_err(|error| FlutzError::Runtime(format!("failed to read current exe: {error}")))?;
    println!("current_dir: {}", current_dir.display());
    println!("current_exe: {}", executable.display());
    println!("data_dir: {}", data_dir.display());
    println!("data_dir_exists: {}", data_dir.exists());
    println!("stdout_terminal: {}", std::io::stdout().is_terminal());
    println!("stderr_terminal: {}", std::io::stderr().is_terminal());
    println!("launched_from_terminal: {}", launched_from_terminal());
    #[cfg(windows)]
    {
        println!(
            "parent_process: {}",
            windows_parent_process_name().unwrap_or_else(|| "<unknown>".to_owned())
        );
        println!(
            "console_process_count: {}",
            windows_console_process_count()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<unknown>".to_owned())
        );
    }
    println!("target_os: {}", env::consts::OS);
    println!("target_arch: {}", env::consts::ARCH);
    println!(
        "audio_backend_env: {}",
        env::var("FLUTZ_AUDIO_BACKEND").unwrap_or_else(|_| "<unset>".to_owned())
    );
    let memory = memory_runtime::snapshot();
    println!("memory_allocator: {}", memory.allocator);
    println!("memory_enabled: {}", memory.enabled);
    println!("memory_config: {}", memory.config_summary);
    println!("memory_stats_available: {}", memory.totals.available);
    println!("memory_resident_bytes: {}", memory.totals.resident_bytes);
    println!("memory_retained_bytes: {}", memory.totals.retained_bytes);
    Ok(())
}

fn startup_config(
    data_dir: PathBuf,
    audio_backend: playback::AudioBackend,
    debug_memory: bool,
    debug_latency: bool,
    debug_analyzer: bool,
    debug_render_errors: bool,
    launch_open_path: Option<PathBuf>,
) -> app::AppStartupConfig {
    let dat_summary = summarize_dat_data_dir(&data_dir)
        .unwrap_or_else(|error| app::DatStartupSummary::Unavailable(format!("{error}")));
    app::AppStartupConfig {
        data_dir,
        dat_summary,
        audio_backend,
        debug_memory,
        debug_latency,
        debug_analyzer,
        debug_render_errors,
        launch_open_path,
    }
}

fn audio_backend_from_env() -> Result<Option<playback::AudioBackend>> {
    let value = match env::var("FLUTZ_AUDIO_BACKEND") {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    playback::AudioBackend::parse(&value).map(Some)
}

fn default_data_dir() -> Result<PathBuf> {
    Ok(application_directory()?.join("data"))
}

pub(crate) fn application_directory() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(appimage) = original_appimage_path()? {
        return Ok(appimage
            .parent()
            .expect("resolved AppImage path has a parent")
            .to_owned());
    }

    match env::current_exe() {
        Ok(executable) => {
            if let Some(parent) = executable.parent() {
                return Ok(parent.to_owned());
            }
            eprintln!("current executable has no parent; falling back to the working directory");
        }
        Err(error) => {
            eprintln!("failed to resolve current executable: {error}; falling back to the working directory");
        }
    }
    env::current_dir().map_err(|error| {
        FlutzError::Runtime(format!("failed to resolve the working directory: {error}"))
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn original_appimage_path() -> Result<Option<PathBuf>> {
    let Some(appimage) = env::var_os("APPIMAGE").map(PathBuf::from) else {
        return Ok(None);
    };
    if !appimage.is_absolute() || appimage.file_name().is_none() {
        return Err(FlutzError::InvalidInput(format!(
            "APPIMAGE must name an absolute AppImage path, not {}",
            appimage.display()
        )));
    }
    let parent = appimage.parent().expect("absolute path has a parent");
    let parent = fs::canonicalize(parent).map_err(|error| {
        FlutzError::Runtime(format!(
            "failed to resolve the directory containing APPIMAGE {}: {error}",
            appimage.display()
        ))
    })?;
    Ok(Some(
        parent.join(
            appimage
                .file_name()
                .expect("AppImage path was checked above"),
        ),
    ))
}

fn verify_file(path: &Path) -> Result<()> {
    let bytes = fs::read(path).map_err(|error| {
        FlutzError::Runtime(format!("failed to read {}: {error}", path.display()))
    })?;

    if bytes.starts_with(flutz_fmid::FMID_MAGIC) {
        flutz_fmid::read::validate_magic(&bytes)?;
        println!("{}: FMID container magic OK", path.display());
        println!("bytes: {}", bytes.len());
        return Ok(());
    }

    if bytes.starts_with(b"MThd") {
        let midi = flutz_synth::playback::validate_midi_bytes(&bytes)?;
        println!("{}: standard MIDI file OK", path.display());
        println!("bytes: {}", midi.byte_len);
        return Ok(());
    }

    if bytes.starts_with(flutz_dat::DAT_MAGIC) {
        let index = parse_dat_index(&bytes)?;
        println!("{}: DAT archive magic and index OK", path.display());
        println!("bytes: {}", bytes.len());
        println!("chunk_size: {}", index.chunk_size);
        println!("entries: {}", index.entries.len());
        println!("chunks: {}", index.chunks.len());
        for entry in index.entries {
            println!(
                "  {} ({}) source={} storage={} runtime={} bytes={} flags=0x{:016x}",
                entry.entry.internal_id,
                entry.entry.display_name,
                entry.entry.source_format,
                entry.entry.storage_format,
                entry.entry.runtime_format,
                entry.total_size,
                entry.flags
            );
        }
        return Ok(());
    }

    Err(FlutzError::InvalidInput(format!(
        "{} is not recognized as FMID, MIDI, or DAT",
        path.display()
    )))
}
