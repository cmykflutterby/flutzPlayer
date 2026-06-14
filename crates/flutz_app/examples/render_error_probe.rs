use std::{env, fs, path::PathBuf, thread, time::Duration};

use flutz_app::{
    app::{DatStartupSummary, SoundFontCatalogEntry},
    dat_startup_summary_for_data_dir,
    playback::{
        AudioBackend, AudioPlaybackStatus, PlaybackController, PlaybackMidiChannelRoleChange,
        PlaybackMidiScanDiagnostics, PlaybackMidiSysexEvent, RenderErrorTraceEvent,
        RenderProbeReport,
    },
};
use flutz_core::default_preset_set;
use flutz_fmid::{read_fmid, FmidFile, LoopMode as FmidLoopMode, MixerSourceMode};
use flutz_synth::{PlaybackLoopMode, PlaybackLoopSettings};

const DEFAULT_DATA_DIR: &str = "drops/flutzplayer/data";
const DEFAULT_FIRST_INPUT: &str = "MIDI Files/rendering-parity-midi/smb-fx-test.fmid";
const DEFAULT_SECOND_INPUT: &str = "MIDI Files/rendering-parity-midi/smb-fx-test2.fmid";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = ProbeArgs::parse()?;
    let catalog = soundfont_catalog(&args.data_dir)?;
    let mut playback = PlaybackController::new(
        args.data_dir.clone(),
        catalog,
        AudioBackend::Sdl3,
        false,
        args.debug_render_errors,
    );

    for (label, input) in [("first", &args.first_input), ("second", &args.second_input)] {
        run_case(
            &mut playback,
            label,
            input,
            args.frames,
            args.audio_ms,
            args.max_completion_seconds,
        )?;
    }

    Ok(())
}

struct ProbeArgs {
    data_dir: PathBuf,
    first_input: PathBuf,
    second_input: PathBuf,
    frames: usize,
    audio_ms: u64,
    max_completion_seconds: f64,
    debug_render_errors: bool,
}

impl ProbeArgs {
    fn parse() -> Result<Self, String> {
        let mut data_dir = PathBuf::from(DEFAULT_DATA_DIR);
        let mut first_input = PathBuf::from(DEFAULT_FIRST_INPUT);
        let mut second_input = PathBuf::from(DEFAULT_SECOND_INPUT);
        let mut frames = 512usize;
        let mut audio_ms = 250u64;
        let mut max_completion_seconds = 900.0f64;
        let mut debug_render_errors = false;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => data_dir = PathBuf::from(next_arg(&mut args, "--data-dir")?),
                "--first-input" | "--input-a" => {
                    first_input = PathBuf::from(next_arg(&mut args, "--first-input")?)
                }
                "--second-input" | "--input-b" => {
                    second_input = PathBuf::from(next_arg(&mut args, "--second-input")?)
                }
                "--frames" => {
                    frames = next_arg(&mut args, "--frames")?
                        .parse()
                        .map_err(|_| "--frames requires a positive integer".to_owned())?;
                }
                "--audio-ms" => {
                    audio_ms = next_arg(&mut args, "--audio-ms")?
                        .parse()
                        .map_err(|_| "--audio-ms requires a non-negative integer".to_owned())?;
                }
                "--max-completion-seconds" => {
                    max_completion_seconds = next_arg(&mut args, "--max-completion-seconds")?
                        .parse()
                        .map_err(|_| {
                            "--max-completion-seconds requires a positive number".to_owned()
                        })?;
                }
                "--debug-render-errors" => debug_render_errors = true,
                other => return Err(format!("unsupported argument: {other}")),
            }
        }
        Ok(Self {
            data_dir,
            first_input,
            second_input,
            frames: frames.max(1),
            audio_ms,
            max_completion_seconds: max_completion_seconds.max(1.0),
            debug_render_errors,
        })
    }
}

#[derive(Debug, Clone)]
struct LoadedInput {
    load_message: String,
    loop_settings: PlaybackLoopSettings,
}

struct CompletionOutcome {
    loop_settings: PlaybackLoopSettings,
    jump_start_tick: Option<u64>,
    jump_end_tick: Option<u64>,
    render_calls: usize,
    max_peak: f32,
    ended_naturally: bool,
    loop_wrap_observed: bool,
    timed_out: bool,
    saw_loop_end: bool,
    last_tick: Option<u64>,
}

impl CompletionOutcome {
    fn new(
        loop_settings: PlaybackLoopSettings,
        jump_start_tick: Option<u64>,
        jump_end_tick: Option<u64>,
    ) -> Self {
        Self {
            loop_settings,
            jump_start_tick,
            jump_end_tick,
            render_calls: 0,
            max_peak: 0.0,
            ended_naturally: false,
            loop_wrap_observed: false,
            timed_out: false,
            saw_loop_end: false,
            last_tick: None,
        }
    }

    fn record_render(&mut self, playback: &PlaybackController, report: &RenderProbeReport) {
        self.render_calls = self.render_calls.saturating_add(1);
        self.max_peak = self.max_peak.max(report.peak);

        let tick = playback.transport_tick();
        if let Some(loop_end) = self.jump_end_tick {
            if tick >= loop_end {
                self.saw_loop_end = true;
            }
        }

        if self.loop_settings.enabled {
            if let Some(previous_tick) = self.last_tick {
                if previous_tick > tick {
                    let plain_wrap = self.jump_start_tick.is_none() && self.jump_end_tick.is_none();
                    let bounded_wrap = self
                        .jump_start_tick
                        .is_some_and(|loop_start| tick <= loop_start.saturating_add(1024));
                    if plain_wrap || (self.saw_loop_end && bounded_wrap) {
                        self.loop_wrap_observed = true;
                    }
                }
            }
        }

        self.last_tick = Some(tick);
        if !playback.playback_active() {
            self.ended_naturally = true;
        }
    }

    fn is_complete(&self) -> bool {
        self.ended_naturally || self.loop_wrap_observed
    }

    fn mark_timeout(&mut self) {
        self.timed_out = true;
    }

    fn reason(&self) -> &'static str {
        if self.loop_wrap_observed {
            "loop-wrap"
        } else if self.ended_naturally {
            "end-of-sequence"
        } else if self.timed_out {
            "timeout"
        } else {
            "in-progress"
        }
    }
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn soundfont_catalog(
    data_dir: &PathBuf,
) -> Result<Vec<SoundFontCatalogEntry>, Box<dyn std::error::Error>> {
    match dat_startup_summary_for_data_dir(data_dir)? {
        DatStartupSummary::Available { soundfonts, .. } => Ok(soundfonts),
        DatStartupSummary::Unavailable(error) => Err(error.into()),
    }
}

fn run_case(
    playback: &mut PlaybackController,
    label: &str,
    input: &PathBuf,
    frames: usize,
    audio_ms: u64,
    max_completion_seconds: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_input(playback, input)?;
    let transport = playback.midi_transport_metadata();
    println!(
        "{{\"event\":\"render_error_probe_case\",\"label\":{},\"input\":{},\"load_message\":{},\"frames\":{},\"audio_ms\":{},\"duration_seconds\":{:.6},\"tick_length\":{},\"jump_start_tick\":{},\"jump_end_tick\":{},\"loop_enabled\":{},\"loop_mode\":{},\"loop_count\":{},\"max_completion_seconds\":{:.3}}}",
        json_string(label),
        json_string(&input.display().to_string()),
        json_string(&loaded.load_message.replace('\n', " | ")),
        frames,
        audio_ms,
        transport.duration_seconds,
        transport.tick_length,
        json_option_u64(transport.jump_start_tick),
        json_option_u64(transport.jump_end_tick),
        if loaded.loop_settings.enabled { "true" } else { "false" },
        json_string(loop_mode_label(loaded.loop_settings.mode)),
        loaded.loop_settings.loop_count,
        max_completion_seconds,
    );
    print_midi_scan(label, &playback.midi_scan_diagnostics());

    let mut collected_errors = Vec::new();
    if audio_ms > 0 {
        match playback.play()? {
            AudioPlaybackStatus::Audible => {
                println!(
                    "{{\"event\":\"render_error_probe_audio\",\"label\":{},\"status\":\"audible\",\"audio_ms\":{}}}",
                    json_string(label),
                    audio_ms,
                );
                thread::sleep(Duration::from_millis(audio_ms));
            }
            AudioPlaybackStatus::AudioUnavailable(message) => {
                println!(
                    "{{\"event\":\"render_error_probe_audio\",\"label\":{},\"status\":\"audio_unavailable\",\"audio_ms\":{},\"message\":{}}}",
                    json_string(label),
                    audio_ms,
                    json_string(&message),
                );
            }
        }
        drain_render_errors(playback, label, &mut collected_errors);
        playback.pause()?;
    }

    playback.seek_transport_tick(0)?;
    playback.set_loop_settings(loaded.loop_settings)?;
    playback.play()?;

    let started = std::time::Instant::now();
    let mut outcome = CompletionOutcome::new(
        loaded.loop_settings,
        transport.jump_start_tick,
        transport.jump_end_tick,
    );
    while started.elapsed().as_secs_f64() < max_completion_seconds {
        let report = render_without_stop(playback, frames)?;
        outcome.record_render(playback, &report);
        if outcome.render_calls == 1
            || outcome.render_calls % 32 == 0
            || report.recovered_error_count > 0
            || outcome.is_complete()
        {
            println!(
                "{{\"event\":\"render_error_probe_render\",\"label\":{},\"iteration\":{},\"frames\":{},\"samples\":{},\"peak\":{:.6},\"recovered_error_count\":{},\"transport_tick\":{},\"transport_seconds\":{:.6},\"engine_state\":{},\"completion_candidate\":{}}}",
                json_string(label),
                outcome.render_calls,
                report.frames,
                report.samples,
                report.peak,
                report.recovered_error_count,
                playback.transport_tick(),
                playback.audible_transport_seconds(),
                json_string(&playback.debug_metrics().engine_state),
                if outcome.is_complete() { "true" } else { "false" },
            );
        }
        drain_render_errors(playback, label, &mut collected_errors);
        if outcome.is_complete() {
            break;
        }
    }

    if !outcome.is_complete() {
        outcome.mark_timeout();
    }

    println!(
        "{{\"event\":\"render_error_probe_summary\",\"label\":{},\"completion_reason\":{},\"render_calls\":{},\"recovered_error_count\":{},\"max_peak\":{:.6},\"final_tick\":{},\"final_seconds\":{:.6},\"final_state\":{},\"loop_wrap_observed\":{},\"ended_naturally\":{},\"timeout\":{},\"elapsed_seconds\":{:.6}}}",
        json_string(label),
        json_string(outcome.reason()),
        outcome.render_calls,
        collected_errors.len(),
        outcome.max_peak,
        playback.transport_tick(),
        playback.audible_transport_seconds(),
        json_string(&playback.debug_metrics().engine_state),
        if outcome.loop_wrap_observed { "true" } else { "false" },
        if outcome.ended_naturally { "true" } else { "false" },
        if outcome.timed_out { "true" } else { "false" },
        started.elapsed().as_secs_f64(),
    );

    for event in &collected_errors {
        print_render_error(label, event);
    }
    playback.stop()?;
    Ok(())
}

fn load_input(
    playback: &mut PlaybackController,
    input: &PathBuf,
) -> Result<LoadedInput, Box<dyn std::error::Error>> {
    match input.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("fmid") => load_fmid(playback, input),
        _ => Ok(LoadedInput {
            load_message: playback.load_midi_file(input, &[])?,
            loop_settings: PlaybackLoopSettings::default(),
        }),
    }
}

fn render_without_stop(
    playback: &mut PlaybackController,
    frames: usize,
) -> Result<RenderProbeReport, Box<dyn std::error::Error>> {
    let mut reports = playback.render_probe_sequence(&[frames], false)?;
    reports
        .pop()
        .ok_or_else(|| "render sequence produced no report".into())
}

fn load_fmid(
    playback: &mut PlaybackController,
    input: &PathBuf,
) -> Result<LoadedInput, Box<dyn std::error::Error>> {
    let bytes = fs::read(input)?;
    let fmid = read_fmid(&bytes)?;
    let requested_soundfonts = requested_soundfonts(&fmid);
    let load_message = playback.load_midi_bytes(
        fmid.midi_bytes.clone(),
        fmid.project.source_midi_filename.clone(),
        &requested_soundfonts,
    )?;
    let loop_settings = playback_loop_settings(&fmid);
    playback.set_loop_settings(loop_settings)?;
    Ok(LoadedInput {
        load_message,
        loop_settings,
    })
}

fn requested_soundfonts(fmid: &FmidFile) -> Vec<String> {
    match &fmid.mixer_source_mode {
        MixerSourceMode::Custom => fmid
            .soundfonts
            .iter()
            .map(|slot| slot.internal_id.clone())
            .collect(),
        MixerSourceMode::PresetDefault(preset_id) => default_preset_set()
            .find_preset(preset_id)
            .unwrap_or_else(|| default_preset_set().default_preset())
            .font_ids
            .iter()
            .map(|font_id| (*font_id).to_owned())
            .collect(),
    }
}

fn playback_loop_settings(fmid: &FmidFile) -> PlaybackLoopSettings {
    let mode = match fmid.looping.mode {
        FmidLoopMode::None => PlaybackLoopMode::None,
        FmidLoopMode::Infinite => PlaybackLoopMode::Infinite,
        FmidLoopMode::Counted => PlaybackLoopMode::Counted,
    };
    let mut settings = PlaybackLoopSettings {
        enabled: fmid.looping.enabled,
        mode,
        start_tick: fmid.looping.start_tick,
        end_tick: fmid.looping.end_tick,
        loop_count: (fmid.looping.loop_count as u32).max(1),
    };
    if matches!(settings.mode, PlaybackLoopMode::None) || settings.end_tick <= settings.start_tick {
        settings.enabled = false;
    }
    settings
}

fn drain_render_errors(
    playback: &PlaybackController,
    label: &str,
    collected_errors: &mut Vec<RenderErrorTraceEvent>,
) {
    for event in playback.take_render_error_events() {
        println!(
            "{{\"event\":\"render_error_probe_failure_captured\",\"label\":{},\"source\":{},\"failure_kind\":{},\"detail\":{},\"backtrace\":{},\"frames_requested\":{}}}",
            json_string(label),
            json_string(&event.source),
            json_string(&event.failure_kind),
            json_string(&event.detail),
            json_option_string(event.backtrace.as_deref()),
            event.frames_requested,
        );
        collected_errors.push(event);
    }
}

fn loop_mode_label(mode: PlaybackLoopMode) -> &'static str {
    match mode {
        PlaybackLoopMode::None => "none",
        PlaybackLoopMode::Infinite => "infinite",
        PlaybackLoopMode::Counted => "counted",
    }
}

fn print_midi_scan(label: &str, diagnostics: &PlaybackMidiScanDiagnostics) {
    println!(
        "{{\"event\":\"render_error_probe_midi_scan\",\"label\":{},\"loop_style\":{},\"system_modes\":{},\"percussion_channels\":{},\"warnings\":{},\"sysex_event_count\":{},\"recognized_sysex_event_count\":{}}}",
        json_string(label),
        json_string(&diagnostics.loop_style),
        json_string_vec(&diagnostics.system_modes),
        json_u8_vec(&diagnostics.percussion_channels),
        json_string_vec(&diagnostics.warnings),
        diagnostics.sysex_event_count,
        diagnostics.recognized_sysex_event_count,
    );
    for event in &diagnostics.sysex_events {
        print_sysex_event(label, event);
    }
}

fn print_sysex_event(label: &str, event: &PlaybackMidiSysexEvent) {
    println!(
        "{{\"event\":\"render_error_probe_sysex\",\"label\":{},\"index\":{},\"status\":{},\"byte_len\":{},\"manufacturer_id\":{},\"recognized\":{},\"system_mode\":{},\"channel_role\":{},\"warning\":{},\"bytes_hex\":{}}}",
        json_string(label),
        event.index,
        event.status,
        event.byte_len,
        json_string(&event.manufacturer_id),
        if event.recognized { "true" } else { "false" },
        json_option_string(event.system_mode.as_deref()),
        json_channel_role(event.channel_role.as_ref()),
        json_option_string(event.warning.as_deref()),
        json_string(&event.bytes_hex),
    );
}

fn print_render_error(label: &str, event: &RenderErrorTraceEvent) {
    println!(
        "{{\"event\":\"render_error_probe_failure\",\"label\":{},\"source\":{},\"failure_kind\":{},\"detail\":{},\"backtrace\":{},\"frames_requested\":{},\"midi_source\":{},\"soundfont_ids\":{},\"loop_style\":{},\"system_modes\":{},\"percussion_channels\":{},\"warnings\":{},\"sysex_event_count\":{},\"recognized_sysex_event_count\":{}}}",
        json_string(label),
        json_string(&event.source),
        json_string(&event.failure_kind),
        json_string(&event.detail),
        json_option_string(event.backtrace.as_deref()),
        event.frames_requested,
        json_option_string(event.midi_source.as_deref()),
        json_string_vec(&event.soundfont_ids),
        json_string(&event.midi_scan.loop_style),
        json_string_vec(&event.midi_scan.system_modes),
        json_u8_vec(&event.midi_scan.percussion_channels),
        json_string_vec(&event.midi_scan.warnings),
        event.midi_scan.sysex_event_count,
        event.midi_scan.recognized_sysex_event_count,
    );
}

fn json_channel_role(role: Option<&PlaybackMidiChannelRoleChange>) -> String {
    role.map(|role| {
        format!(
            "{{\"channel\":{},\"role\":{}}}",
            role.channel,
            json_string(&role.role)
        )
    })
    .unwrap_or_else(|| "null".to_owned())
}

fn json_string_vec(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| json_string(value))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_u8_vec(values: &[u8]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_option_string(value: Option<&str>) -> String {
    value.map(json_string).unwrap_or_else(|| "null".to_owned())
}

fn json_option_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned())
}

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}
