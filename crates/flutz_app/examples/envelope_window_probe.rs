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
        "{{\"event\":\"envelope_window_probe_case\",\"input\":{},\"load_message\":{},\"tick_start\":{},\"tick_end\":{},\"preroll_ticks\":{},\"postroll_ticks\":{},\"frames\":{},\"tick_length\":{},\"duration_seconds\":{:.6}}}",
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
                    "{{\"event\":\"envelope_window_probe_audio\",\"status\":\"audible\",\"audio_ms\":{}}}",
                    args.audio_ms,
                );
                std::thread::sleep(std::time::Duration::from_millis(args.audio_ms));
            }
            AudioPlaybackStatus::AudioUnavailable(message) => {
                println!(
                    "{{\"event\":\"envelope_window_probe_audio\",\"status\":\"audio_unavailable\",\"audio_ms\":{},\"message\":{}}}",
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
        let record = EnvelopeRecord {
            iteration: iterations,
            tick_before,
            tick_after,
            seconds_before,
            seconds_after,
            render_peak: report.peak,
            active_voices: memory.active_voices,
            total_voices: memory.total_voices,
            env_delay_voices: memory.env_delay_voices,
            env_attack_voices: memory.env_attack_voices,
            env_hold_voices: memory.env_hold_voices,
            env_decay_voices: memory.env_decay_voices,
            env_release_voices: memory.env_release_voices,
            env_value_avg: memory.env_value_avg,
            env_value_sum: memory.env_value_sum,
            top_instances: top_instances(&memory, 4),
        };
        if overlaps(record.tick_before, record.tick_after, seek_start, stop_tick)
            || record.tick_before >= seek_start
        {
            print_record(&record);
        }
        records.push(record);
        if tick_after <= tick_before {
            stalled_iterations = stalled_iterations.saturating_add(1);
            if stalled_iterations >= 256 {
                println!(
                    "{{\"event\":\"envelope_window_probe_stalled\",\"tick\":{},\"iterations\":{}}}",
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

#[derive(Clone)]
struct InstanceEnvelopeSummary {
    id: String,
    display_name: String,
    active_voices: usize,
    env_attack_voices: usize,
    env_hold_voices: usize,
    env_decay_voices: usize,
    env_release_voices: usize,
    env_value_avg: f32,
}

struct EnvelopeRecord {
    iteration: usize,
    tick_before: u64,
    tick_after: u64,
    seconds_before: f64,
    seconds_after: f64,
    render_peak: f32,
    active_voices: usize,
    total_voices: usize,
    env_delay_voices: usize,
    env_attack_voices: usize,
    env_hold_voices: usize,
    env_decay_voices: usize,
    env_release_voices: usize,
    env_value_avg: f32,
    env_value_sum: f32,
    top_instances: Vec<InstanceEnvelopeSummary>,
}

struct EnvelopeSummary {
    record_count: usize,
    range_record_count: usize,
    peak_active_voices: usize,
    min_env_value_avg: f32,
    min_env_tick_before: u64,
    max_env_value_avg: f32,
    max_env_tick_before: u64,
    max_release_voices: usize,
    max_release_tick_before: u64,
    max_decay_voices: usize,
    max_decay_tick_before: u64,
    min_render_peak: f32,
    min_render_peak_tick_before: u64,
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

fn top_instances(memory: &PlaybackMemoryDebug, limit: usize) -> Vec<InstanceEnvelopeSummary> {
    let mut instances = memory
        .instances
        .iter()
        .map(|instance| InstanceEnvelopeSummary {
            id: instance.internal_id.clone(),
            display_name: instance.display_name.clone(),
            active_voices: instance.synth.active_voices,
            env_attack_voices: instance.synth.env_attack_voices,
            env_hold_voices: instance.synth.env_hold_voices,
            env_decay_voices: instance.synth.env_decay_voices,
            env_release_voices: instance.synth.env_release_voices,
            env_value_avg: instance.synth.env_value_avg,
        })
        .collect::<Vec<_>>();
    instances.sort_by(|left, right| {
        right
            .env_value_avg
            .partial_cmp(&left.env_value_avg)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.active_voices.cmp(&left.active_voices))
    });
    instances.truncate(limit);
    instances
}

fn print_record(record: &EnvelopeRecord) {
    println!(
        "{{\"event\":\"envelope_window_probe_sample\",\"iteration\":{},\"tick_before\":{},\"tick_after\":{},\"seconds_before\":{:.6},\"seconds_after\":{:.6},\"render_peak\":{:.6},\"active_voices\":{},\"total_voices\":{},\"env_delay_voices\":{},\"env_attack_voices\":{},\"env_hold_voices\":{},\"env_decay_voices\":{},\"env_release_voices\":{},\"env_value_avg\":{:.6},\"env_value_sum\":{:.6},\"top_instances\":{}}}",
        record.iteration,
        record.tick_before,
        record.tick_after,
        record.seconds_before,
        record.seconds_after,
        record.render_peak,
        record.active_voices,
        record.total_voices,
        record.env_delay_voices,
        record.env_attack_voices,
        record.env_hold_voices,
        record.env_decay_voices,
        record.env_release_voices,
        record.env_value_avg,
        record.env_value_sum,
        json_top_instances(&record.top_instances),
    );
}

fn summarize(
    records: &[EnvelopeRecord],
    analysis_start: u64,
    analysis_end: u64,
) -> EnvelopeSummary {
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
    let min_env_record = window_records
        .iter()
        .min_by(|left, right| {
            left.env_value_avg
                .partial_cmp(&right.env_value_avg)
                .unwrap_or(Ordering::Equal)
        })
        .copied();
    let max_env_record = window_records
        .iter()
        .max_by(|left, right| {
            left.env_value_avg
                .partial_cmp(&right.env_value_avg)
                .unwrap_or(Ordering::Equal)
        })
        .copied();
    let max_release_record = window_records
        .iter()
        .max_by_key(|record| record.env_release_voices)
        .copied();
    let max_decay_record = window_records
        .iter()
        .max_by_key(|record| record.env_decay_voices)
        .copied();
    let min_render_record = window_records
        .iter()
        .min_by(|left, right| {
            left.render_peak
                .partial_cmp(&right.render_peak)
                .unwrap_or(Ordering::Equal)
        })
        .copied();
    let peak_active_voices = window_records
        .iter()
        .map(|record| record.active_voices)
        .max()
        .unwrap_or_default();
    let min_env_value_avg = min_env_record
        .map(|record| record.env_value_avg)
        .unwrap_or_default();
    let max_env_value_avg = max_env_record
        .map(|record| record.env_value_avg)
        .unwrap_or_default();
    let max_release_voices = max_release_record
        .map(|record| record.env_release_voices)
        .unwrap_or_default();
    let max_decay_voices = max_decay_record
        .map(|record| record.env_decay_voices)
        .unwrap_or_default();
    let min_render_peak = min_render_record
        .map(|record| record.render_peak)
        .unwrap_or_default();
    let finding = if max_env_value_avg > 0.0 && min_env_value_avg < max_env_value_avg * 0.65 {
        format!(
            "envelope energy dropped inside the window: avg envelope value fell from {:.3} to {:.3}",
            max_env_value_avg, min_env_value_avg
        )
    } else if max_release_voices > peak_active_voices / 2 {
        format!(
            "release-stage voices dominated part of the window: {} of {} active voices were in release at peak",
            max_release_voices, peak_active_voices
        )
    } else {
        format!(
            "no strong envelope collapse observed; avg envelope value stayed between {:.3} and {:.3}",
            min_env_value_avg, max_env_value_avg
        )
    };
    EnvelopeSummary {
        record_count: records.len(),
        range_record_count: window_records.len(),
        peak_active_voices,
        min_env_value_avg,
        min_env_tick_before: min_env_record
            .map(|record| record.tick_before)
            .unwrap_or_default(),
        max_env_value_avg,
        max_env_tick_before: max_env_record
            .map(|record| record.tick_before)
            .unwrap_or_default(),
        max_release_voices,
        max_release_tick_before: max_release_record
            .map(|record| record.tick_before)
            .unwrap_or_default(),
        max_decay_voices,
        max_decay_tick_before: max_decay_record
            .map(|record| record.tick_before)
            .unwrap_or_default(),
        min_render_peak,
        min_render_peak_tick_before: min_render_record
            .map(|record| record.tick_before)
            .unwrap_or_default(),
        finding,
    }
}

fn print_summary(summary: &EnvelopeSummary) {
    println!(
        "{{\"event\":\"envelope_window_probe_summary\",\"record_count\":{},\"range_record_count\":{},\"peak_active_voices\":{},\"min_env_value_avg\":{:.6},\"min_env_tick_before\":{},\"max_env_value_avg\":{:.6},\"max_env_tick_before\":{},\"max_release_voices\":{},\"max_release_tick_before\":{},\"max_decay_voices\":{},\"max_decay_tick_before\":{},\"min_render_peak\":{:.6},\"min_render_peak_tick_before\":{},\"finding\":{}}}",
        summary.record_count,
        summary.range_record_count,
        summary.peak_active_voices,
        summary.min_env_value_avg,
        summary.min_env_tick_before,
        summary.max_env_value_avg,
        summary.max_env_tick_before,
        summary.max_release_voices,
        summary.max_release_tick_before,
        summary.max_decay_voices,
        summary.max_decay_tick_before,
        summary.min_render_peak,
        summary.min_render_peak_tick_before,
        json_string(&summary.finding),
    );
}

fn json_top_instances(instances: &[InstanceEnvelopeSummary]) -> String {
    format!(
        "[{}]",
        instances
            .iter()
            .map(|instance| {
                format!(
                    "{{\"id\":{},\"display_name\":{},\"active_voices\":{},\"env_attack_voices\":{},\"env_hold_voices\":{},\"env_decay_voices\":{},\"env_release_voices\":{},\"env_value_avg\":{:.6}}}",
                    json_string(&instance.id),
                    json_string(&instance.display_name),
                    instance.active_voices,
                    instance.env_attack_voices,
                    instance.env_hold_voices,
                    instance.env_decay_voices,
                    instance.env_release_voices,
                    instance.env_value_avg,
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
