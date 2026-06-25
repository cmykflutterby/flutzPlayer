use std::{cmp::Ordering, env, fs, path::PathBuf};

use flutz_app::{
    app::{DatStartupSummary, SoundFontCatalogEntry},
    dat_startup_summary_for_data_dir,
    playback::{AudioBackend, AudioPlaybackStatus, PlaybackController},
};
use flutz_core::default_preset_set;
use flutz_fmid::{read_fmid, FmidFile, LoopMode as FmidLoopMode, MixerSourceMode};
use flutz_synth::{PlaybackLoopSettings, PlaybackMemoryDebug};

const DEFAULT_DATA_DIR: &str = "drops/flutzplayer/data";
const DEFAULT_INPUT: &str = "MIDI Files/rendering-parity-midi/DKC - Fear Factory.fmid";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = ProbeArgs::parse()?;
    let catalog = soundfont_catalog(&args.data_dir)?;
    let mut playback = PlaybackController::new(
        args.data_dir.clone(),
        catalog,
        AudioBackend::Sdl3,
        false,
        false,
    );

    let loaded = load_input(&mut playback, &args.input)?;
    let transport = playback.midi_transport_metadata();
    println!(
        "{{\"event\":\"voice_window_probe_case\",\"input\":{},\"load_message\":{},\"tick_start\":{},\"tick_end\":{},\"preroll_ticks\":{},\"postroll_ticks\":{},\"frames\":{},\"tick_length\":{},\"duration_seconds\":{:.6}}}",
        json_string(&args.input.display().to_string()),
        json_string(&loaded.load_message.replace('\n', " | ")),
        args.start_tick,
        args.end_tick,
        args.preroll_ticks,
        args.postroll_ticks,
        args.frames,
        transport.tick_length,
        transport.duration_seconds,
    );

    if args.audio_ms > 0 {
        match playback.play()? {
            AudioPlaybackStatus::Audible => {
                println!(
                    "{{\"event\":\"voice_window_probe_audio\",\"status\":\"audible\",\"audio_ms\":{}}}",
                    args.audio_ms,
                );
                std::thread::sleep(std::time::Duration::from_millis(args.audio_ms));
            }
            AudioPlaybackStatus::AudioUnavailable(message) => {
                println!(
                    "{{\"event\":\"voice_window_probe_audio\",\"status\":\"audio_unavailable\",\"audio_ms\":{},\"message\":{}}}",
                    args.audio_ms,
                    json_string(&message),
                );
            }
        }
        playback.pause()?;
    }

    playback.set_loop_enabled(false)?;
    let seek_start = args.start_tick.saturating_sub(args.preroll_ticks) as u64;
    let stop_tick = args.end_tick.saturating_add(args.postroll_ticks) as u64;
    playback.seek_transport_tick(seek_start)?;
    playback.play()?;

    let mut previous_memory = playback.playback_memory_debug().unwrap_or_default();
    let mut records = Vec::new();
    let mut iterations = 0usize;
    let mut stalled_iterations = 0usize;
    while playback.transport_tick() <= stop_tick && iterations < args.max_iterations {
        iterations = iterations.saturating_add(1);
        let tick_before = playback.transport_tick();
        let seconds_before = playback.audible_transport_seconds();
        let report = render_without_stop(&mut playback, args.frames)?;
        let tick_after = playback.transport_tick();
        let seconds_after = playback.audible_transport_seconds();
        let memory = playback.playback_memory_debug().unwrap_or_default();
        let deltas = VoiceCounterDeltas::from_snapshots(&previous_memory, &memory);
        let top_instances = top_instances(&memory, 4);
        let utilization = if memory.total_voices > 0 {
            memory.active_voices as f32 / memory.total_voices as f32
        } else {
            0.0
        };
        let record = VoiceRecord {
            iteration: iterations,
            tick_before,
            tick_after,
            seconds_before,
            seconds_after,
            render_peak: report.peak,
            active_voices: memory.active_voices,
            total_voices: memory.total_voices,
            utilization,
            max_active_voices_seen: memory.max_active_voices,
            deltas,
            top_instances,
        };
        if overlaps(record.tick_before, record.tick_after, seek_start, stop_tick)
            || record.tick_before >= seek_start
        {
            print_record(&record);
        }
        records.push(record);
        previous_memory = memory;
        if tick_after <= tick_before {
            stalled_iterations = stalled_iterations.saturating_add(1);
            if stalled_iterations >= 256 {
                println!(
                    "{{\"event\":\"voice_window_probe_stalled\",\"tick\":{},\"iterations\":{}}}",
                    tick_after, stalled_iterations,
                );
                break;
            }
        } else {
            stalled_iterations = 0;
        }
    }
    playback.stop()?;

    let summary = summarize(&records, args.start_tick as u64, args.end_tick as u64);
    print_summary(&summary);
    Ok(())
}

struct ProbeArgs {
    data_dir: PathBuf,
    input: PathBuf,
    start_tick: i32,
    end_tick: i32,
    preroll_ticks: i32,
    postroll_ticks: i32,
    frames: usize,
    audio_ms: u64,
    max_iterations: usize,
}

impl ProbeArgs {
    fn parse() -> Result<Self, String> {
        let mut data_dir = PathBuf::from(DEFAULT_DATA_DIR);
        let mut input = PathBuf::from(DEFAULT_INPUT);
        let mut start_tick = 14600i32;
        let mut end_tick = 14800i32;
        let mut preroll_ticks = 400i32;
        let mut postroll_ticks = 400i32;
        let mut frames = 256usize;
        let mut audio_ms = 0u64;
        let mut max_iterations = 4096usize;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => data_dir = PathBuf::from(next_arg(&mut args, "--data-dir")?),
                "--input" => input = PathBuf::from(next_arg(&mut args, "--input")?),
                "--start-tick" => {
                    start_tick = next_arg(&mut args, "--start-tick")?
                        .parse()
                        .map_err(|_| "--start-tick requires an integer".to_owned())?;
                }
                "--end-tick" => {
                    end_tick = next_arg(&mut args, "--end-tick")?
                        .parse()
                        .map_err(|_| "--end-tick requires an integer".to_owned())?;
                }
                "--preroll-ticks" => {
                    preroll_ticks = next_arg(&mut args, "--preroll-ticks")?
                        .parse()
                        .map_err(|_| "--preroll-ticks requires an integer".to_owned())?;
                }
                "--postroll-ticks" => {
                    postroll_ticks = next_arg(&mut args, "--postroll-ticks")?
                        .parse()
                        .map_err(|_| "--postroll-ticks requires an integer".to_owned())?;
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
                "--max-iterations" => {
                    max_iterations = next_arg(&mut args, "--max-iterations")?
                        .parse()
                        .map_err(|_| "--max-iterations requires a positive integer".to_owned())?;
                }
                other => return Err(format!("unsupported argument: {other}")),
            }
        }
        Ok(Self {
            data_dir,
            input,
            start_tick: start_tick.min(end_tick),
            end_tick: start_tick.max(end_tick),
            preroll_ticks: preroll_ticks.max(0),
            postroll_ticks: postroll_ticks.max(0),
            frames: frames.max(1),
            audio_ms,
            max_iterations: max_iterations.max(1),
        })
    }
}

#[derive(Clone)]
struct LoadedInput {
    load_message: String,
}

#[derive(Clone, Copy, Default)]
struct VoiceCounterDeltas {
    voice_requests: u64,
    exclusive_reuses: u64,
    free_allocations: u64,
    contention_steals: u64,
}

impl VoiceCounterDeltas {
    fn from_snapshots(previous: &PlaybackMemoryDebug, current: &PlaybackMemoryDebug) -> Self {
        Self {
            voice_requests: current
                .total_voice_requests
                .saturating_sub(previous.total_voice_requests),
            exclusive_reuses: current
                .exclusive_class_reuses
                .saturating_sub(previous.exclusive_class_reuses),
            free_allocations: current
                .free_voice_allocations
                .saturating_sub(previous.free_voice_allocations),
            contention_steals: current
                .contention_steals
                .saturating_sub(previous.contention_steals),
        }
    }
}

#[derive(Clone)]
struct InstanceVoiceSummary {
    id: String,
    display_name: String,
    active_voices: usize,
    total_voices: usize,
    max_active_voices: usize,
    contention_steals: u64,
}

struct VoiceRecord {
    iteration: usize,
    tick_before: u64,
    tick_after: u64,
    seconds_before: f64,
    seconds_after: f64,
    render_peak: f32,
    active_voices: usize,
    total_voices: usize,
    utilization: f32,
    max_active_voices_seen: usize,
    deltas: VoiceCounterDeltas,
    top_instances: Vec<InstanceVoiceSummary>,
}

struct VoiceSummary {
    record_count: usize,
    range_record_count: usize,
    total_voices: usize,
    peak_active_voices: usize,
    peak_utilization: f32,
    peak_utilization_tick_before: u64,
    window_voice_requests: u64,
    window_exclusive_reuses: u64,
    window_free_allocations: u64,
    window_contention_steals: u64,
    contention_sample_count: usize,
    peak_contention_delta: u64,
    peak_contention_tick_before: u64,
    peak_contention_tick_after: u64,
    finding: String,
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

fn load_input(
    playback: &mut PlaybackController,
    input: &PathBuf,
) -> Result<LoadedInput, Box<dyn std::error::Error>> {
    match input.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("fmid") => load_fmid(playback, input),
        _ => Ok(LoadedInput {
            load_message: playback.load_midi_file(input, &[])?,
        }),
    }
}

fn load_fmid(
    playback: &mut PlaybackController,
    input: &PathBuf,
) -> Result<LoadedInput, Box<dyn std::error::Error>> {
    let bytes = fs::read(input)?;
    let fmid = read_fmid(&bytes)?;
    let requested_soundfonts = requested_soundfonts(&fmid);
    let loop_settings = playback_loop_settings(&fmid);
    let load_message = playback.load_midi_bytes(
        fmid.midi_bytes,
        fmid.project.source_midi_filename.clone(),
        &requested_soundfonts,
    )?;
    playback.set_loop_settings(loop_settings)?;
    Ok(LoadedInput { load_message })
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
    let mut settings = PlaybackLoopSettings::default();
    settings.enabled = matches!(
        fmid.looping.mode,
        FmidLoopMode::Infinite | FmidLoopMode::Counted
    ) && fmid.looping.enabled
        && fmid.looping.end_tick > fmid.looping.start_tick;
    settings.start_tick = fmid.looping.start_tick;
    settings.end_tick = fmid.looping.end_tick;
    settings.loop_count = (fmid.looping.loop_count as u32).max(1);
    settings
}

fn render_without_stop(
    playback: &mut PlaybackController,
    frames: usize,
) -> Result<flutz_app::playback::RenderProbeReport, Box<dyn std::error::Error>> {
    let mut reports = playback.render_probe_sequence(&[frames], false)?;
    reports
        .pop()
        .ok_or_else(|| "render sequence produced no report".into())
}

fn overlaps(start_a: u64, end_a: u64, start_b: u64, end_b: u64) -> bool {
    start_a <= end_b && end_a >= start_b
}

fn top_instances(memory: &PlaybackMemoryDebug, limit: usize) -> Vec<InstanceVoiceSummary> {
    let mut instances = memory
        .instances
        .iter()
        .map(|instance| InstanceVoiceSummary {
            id: instance.internal_id.clone(),
            display_name: instance.display_name.clone(),
            active_voices: instance.synth.active_voices,
            total_voices: instance.synth.total_voices,
            max_active_voices: instance.synth.max_active_voices,
            contention_steals: instance.synth.contention_steals,
        })
        .collect::<Vec<_>>();
    instances.sort_by(|left, right| {
        right
            .active_voices
            .cmp(&left.active_voices)
            .then_with(|| right.max_active_voices.cmp(&left.max_active_voices))
    });
    instances.truncate(limit);
    instances
}

fn print_record(record: &VoiceRecord) {
    println!(
        "{{\"event\":\"voice_window_probe_sample\",\"iteration\":{},\"tick_before\":{},\"tick_after\":{},\"seconds_before\":{:.6},\"seconds_after\":{:.6},\"render_peak\":{:.6},\"active_voices\":{},\"total_voices\":{},\"utilization\":{:.6},\"max_active_voices_seen\":{},\"voice_requests_delta\":{},\"exclusive_reuses_delta\":{},\"free_allocations_delta\":{},\"contention_steals_delta\":{},\"top_instances\":{}}}",
        record.iteration,
        record.tick_before,
        record.tick_after,
        record.seconds_before,
        record.seconds_after,
        record.render_peak,
        record.active_voices,
        record.total_voices,
        record.utilization,
        record.max_active_voices_seen,
        record.deltas.voice_requests,
        record.deltas.exclusive_reuses,
        record.deltas.free_allocations,
        record.deltas.contention_steals,
        json_top_instances(&record.top_instances),
    );
}

fn summarize(records: &[VoiceRecord], analysis_start: u64, analysis_end: u64) -> VoiceSummary {
    let window_records = records
        .iter()
        .filter(|record| {
            overlaps(
                record.tick_before,
                record.tick_after,
                analysis_start,
                analysis_end,
            )
        })
        .collect::<Vec<_>>();
    let peak_record = window_records
        .iter()
        .max_by(|left, right| {
            left.utilization
                .partial_cmp(&right.utilization)
                .unwrap_or(Ordering::Equal)
        })
        .copied();
    let peak_contention_record = window_records
        .iter()
        .max_by_key(|record| record.deltas.contention_steals)
        .copied();
    let total_voices = peak_record
        .map(|record| record.total_voices)
        .unwrap_or_default();
    let peak_active_voices = window_records
        .iter()
        .map(|record| record.active_voices)
        .max()
        .unwrap_or_default();
    let peak_utilization = peak_record
        .map(|record| record.utilization)
        .unwrap_or_default();
    let window_voice_requests = window_records
        .iter()
        .map(|record| record.deltas.voice_requests)
        .sum();
    let window_exclusive_reuses = window_records
        .iter()
        .map(|record| record.deltas.exclusive_reuses)
        .sum();
    let window_free_allocations = window_records
        .iter()
        .map(|record| record.deltas.free_allocations)
        .sum();
    let window_contention_steals = window_records
        .iter()
        .map(|record| record.deltas.contention_steals)
        .sum();
    let contention_sample_count = window_records
        .iter()
        .filter(|record| record.deltas.contention_steals > 0)
        .count();
    let finding = if window_contention_steals > 0 {
        format!(
            "voice contention observed: {} steals across {} sample(s) in the requested tick window",
            window_contention_steals, contention_sample_count
        )
    } else if total_voices > 0
        && peak_active_voices.saturating_mul(10) >= total_voices.saturating_mul(9)
    {
        format!(
            "no steals observed, but utilization peaked high at {:.1}% of available voices",
            peak_utilization * 100.0
        )
    } else {
        format!(
            "no voice contention observed; peak utilization reached {:.1}% with {} of {} voices active",
            peak_utilization * 100.0,
            peak_active_voices,
            total_voices
        )
    };
    VoiceSummary {
        record_count: records.len(),
        range_record_count: window_records.len(),
        total_voices,
        peak_active_voices,
        peak_utilization,
        peak_utilization_tick_before: peak_record
            .map(|record| record.tick_before)
            .unwrap_or_default(),
        window_voice_requests,
        window_exclusive_reuses,
        window_free_allocations,
        window_contention_steals,
        contention_sample_count,
        peak_contention_delta: peak_contention_record
            .map(|record| record.deltas.contention_steals)
            .unwrap_or_default(),
        peak_contention_tick_before: peak_contention_record
            .map(|record| record.tick_before)
            .unwrap_or_default(),
        peak_contention_tick_after: peak_contention_record
            .map(|record| record.tick_after)
            .unwrap_or_default(),
        finding,
    }
}

fn print_summary(summary: &VoiceSummary) {
    println!(
        "{{\"event\":\"voice_window_probe_summary\",\"record_count\":{},\"range_record_count\":{},\"total_voices\":{},\"peak_active_voices\":{},\"peak_utilization\":{:.6},\"peak_utilization_tick_before\":{},\"window_voice_requests\":{},\"window_exclusive_reuses\":{},\"window_free_allocations\":{},\"window_contention_steals\":{},\"contention_sample_count\":{},\"peak_contention_delta\":{},\"peak_contention_tick_before\":{},\"peak_contention_tick_after\":{},\"finding\":{}}}",
        summary.record_count,
        summary.range_record_count,
        summary.total_voices,
        summary.peak_active_voices,
        summary.peak_utilization,
        summary.peak_utilization_tick_before,
        summary.window_voice_requests,
        summary.window_exclusive_reuses,
        summary.window_free_allocations,
        summary.window_contention_steals,
        summary.contention_sample_count,
        summary.peak_contention_delta,
        summary.peak_contention_tick_before,
        summary.peak_contention_tick_after,
        json_string(&summary.finding),
    );
}

fn json_top_instances(instances: &[InstanceVoiceSummary]) -> String {
    format!(
        "[{}]",
        instances
            .iter()
            .map(|instance| {
                format!(
                    "{{\"id\":{},\"display_name\":{},\"active_voices\":{},\"total_voices\":{},\"max_active_voices\":{},\"contention_steals\":{}}}",
                    json_string(&instance.id),
                    json_string(&instance.display_name),
                    instance.active_voices,
                    instance.total_voices,
                    instance.max_active_voices,
                    instance.contention_steals,
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    )
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
