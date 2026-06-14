use std::{cmp::Ordering, env, fs, path::PathBuf};

use flutz_app::{
    app::{DatStartupSummary, SoundFontCatalogEntry},
    dat_startup_summary_for_data_dir,
    playback::{
        AudioBackend, AudioPlaybackStatus, PlaybackController, PlaybackMidiScanDiagnostics,
        RealtimeStripSnapshot, RenderErrorTraceEvent, RenderProbeStripMixDiagnostics,
        RenderStemProbeReport,
    },
};
use flutz_core::default_preset_set;
use flutz_fmid::{read_fmid, FmidFile, LoopMode as FmidLoopMode, MixerSourceMode};
use flutz_synth::{PlaybackLoopSettings, StemRenderBlock};
use rustystem::{MidiEventSummary, MidiFile};

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
        args.debug_render_errors,
    );

    let loaded = load_input(&mut playback, &args.input)?;
    let midi_file = MidiFile::new(&mut loaded.midi_bytes.as_slice())?;
    let scan = playback.midi_scan_diagnostics();
    let transport = playback.midi_transport_metadata();
    println!(
        "{{\"event\":\"render_window_probe_case\",\"input\":{},\"load_message\":{},\"tick_start\":{},\"tick_end\":{},\"preroll_ticks\":{},\"postroll_ticks\":{},\"frames\":{},\"audio_ms\":{},\"tick_length\":{},\"duration_seconds\":{:.6}}}",
        json_string(&args.input.display().to_string()),
        json_string(&loaded.load_message.replace('\n', " | ")),
        args.start_tick,
        args.end_tick,
        args.preroll_ticks,
        args.postroll_ticks,
        args.frames,
        args.audio_ms,
        transport.tick_length,
        transport.duration_seconds,
    );
    print_midi_scan(&scan);
    print_event_summaries(
        &midi_file.get_event_summaries_in_tick_range(
            args.start_tick.saturating_sub(args.event_context_ticks),
            args.end_tick.saturating_add(args.event_context_ticks),
        ),
    );

    if args.audio_ms > 0 {
        match playback.play()? {
            AudioPlaybackStatus::Audible => {
                println!(
                    "{{\"event\":\"render_window_probe_audio\",\"status\":\"audible\",\"audio_ms\":{}}}",
                    args.audio_ms,
                );
                std::thread::sleep(std::time::Duration::from_millis(args.audio_ms));
            }
            AudioPlaybackStatus::AudioUnavailable(message) => {
                println!(
                    "{{\"event\":\"render_window_probe_audio\",\"status\":\"audio_unavailable\",\"audio_ms\":{},\"message\":{}}}",
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
    let mut errors = Vec::new();
    let mut iterations = 0usize;
    let mut stalled_iterations = 0usize;
    while playback.transport_tick() <= stop_tick && iterations < args.max_iterations {
        iterations = iterations.saturating_add(1);
        let tick_before = playback.transport_tick();
        let seconds_before = playback.audible_transport_seconds();
        let report = render_without_stop(&mut playback, args.frames)?;
        let tick_after = playback.transport_tick();
        let seconds_after = playback.audible_transport_seconds();
        let live_view = playback.live_output_view();
        let snapshot = live_view.snapshot;
        let top_strips = top_strips(&snapshot.strips.values().cloned().collect::<Vec<_>>(), 4);
        let top_stems = top_stems(&report.stems, 4);
        let top_mix = top_mix(&report.mix_diagnostics, 4);
        let record = WindowRecord {
            iteration: iterations,
            tick_before,
            tick_after,
            seconds_before,
            seconds_after,
            report,
            output_peak: snapshot.output_meter.peak,
            output_rms: snapshot.output_meter.rms,
            active_strip_count: snapshot
                .strips
                .values()
                .filter(|strip| strip.audible || !strip.active_notes.is_empty())
                .count(),
            top_strips,
            top_stems,
            top_mix,
        };
        if overlaps(record.tick_before, record.tick_after, seek_start, stop_tick)
            || record.tick_before >= seek_start
        {
            print_record(&record);
        }
        records.push(record);
        errors.extend(playback.take_render_error_events());
        if tick_after <= tick_before {
            stalled_iterations = stalled_iterations.saturating_add(1);
            if stalled_iterations >= 256 {
                println!(
                    "{{\"event\":\"render_window_probe_stalled\",\"tick\":{},\"iterations\":{}}}",
                    tick_after,
                    stalled_iterations,
                );
                break;
            }
        } else {
            stalled_iterations = 0;
        }
    }
    playback.stop()?;

    let summary = summarize(&records, &errors, &midi_file, &args, &scan);
    print_summary(&summary);
    for error in &errors {
        print_render_error(error);
    }

    Ok(())
}

struct ProbeArgs {
    data_dir: PathBuf,
    input: PathBuf,
    start_tick: i32,
    end_tick: i32,
    preroll_ticks: i32,
    postroll_ticks: i32,
    event_context_ticks: i32,
    frames: usize,
    audio_ms: u64,
    max_iterations: usize,
    debug_render_errors: bool,
}

impl ProbeArgs {
    fn parse() -> Result<Self, String> {
        let mut data_dir = PathBuf::from(DEFAULT_DATA_DIR);
        let mut input = PathBuf::from(DEFAULT_INPUT);
        let mut start_tick = 14600i32;
        let mut end_tick = 14800i32;
        let mut preroll_ticks = 400i32;
        let mut postroll_ticks = 400i32;
        let mut event_context_ticks = 128i32;
        let mut frames = 256usize;
        let mut audio_ms = 0u64;
        let mut max_iterations = 4096usize;
        let mut debug_render_errors = false;
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
                "--event-context-ticks" => {
                    event_context_ticks = next_arg(&mut args, "--event-context-ticks")?
                        .parse()
                        .map_err(|_| "--event-context-ticks requires an integer".to_owned())?;
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
                "--debug-render-errors" => debug_render_errors = true,
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
            event_context_ticks: event_context_ticks.max(0),
            frames: frames.max(1),
            audio_ms,
            max_iterations: max_iterations.max(1),
            debug_render_errors,
        })
    }
}

#[derive(Clone)]
struct LoadedInput {
    load_message: String,
    midi_bytes: Vec<u8>,
}

struct WindowRecord {
    iteration: usize,
    tick_before: u64,
    tick_after: u64,
    seconds_before: f64,
    seconds_after: f64,
    report: RenderStemProbeReport,
    output_peak: f32,
    output_rms: f32,
    active_strip_count: usize,
    top_strips: Vec<StripSummary>,
    top_stems: Vec<StemSummary>,
    top_mix: Vec<MixSummary>,
}

#[derive(Clone)]
struct StripSummary {
    soundfont_id: String,
    midi_channel: u8,
    midi_bank: u16,
    midi_program: u8,
    peak: f32,
    rms: f32,
    audible: bool,
    active_notes: Vec<u8>,
}

#[derive(Clone)]
struct StemSummary {
    soundfont_id: String,
    display_name: Option<String>,
    midi_channel: Option<u8>,
    midi_bank: Option<u16>,
    midi_program: Option<u8>,
    is_percussion: bool,
    peak: f32,
    rms: f32,
    active_notes: Vec<u8>,
}

#[derive(Clone)]
struct MixSummary {
    soundfont_id: String,
    display_name: Option<String>,
    midi_channel: u8,
    midi_program: u8,
    input_peak: f32,
    input_rms: f32,
    estimated_post_peak: f32,
    estimated_post_rms: f32,
    applied_gain: f32,
    session_gain: f32,
    smart_mix_gain: f32,
    lookahead_gain: f32,
    normalization_gain: f32,
    left_pan_gain: f32,
    right_pan_gain: f32,
    automatic_processing: bool,
    audible: bool,
    active_notes: Vec<u8>,
}

struct ProbeSummary {
    record_count: usize,
    baseline_rms: f32,
    baseline_peak: f32,
    window_min_rms: f32,
    window_min_peak: f32,
    min_rms_tick_before: u64,
    min_rms_tick_after: u64,
    largest_drop_ratio: f32,
    largest_drop_from_tick: u64,
    largest_drop_to_tick: u64,
    likely_cause: String,
    nearby_event_count: usize,
    recovered_error_count: usize,
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} requires a value"))
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
        _ => {
            let midi_bytes = fs::read(input)?;
            let load_message = playback.load_midi_bytes(
                midi_bytes.clone(),
                input.display().to_string(),
                &[],
            )?;
            Ok(LoadedInput {
                load_message,
                midi_bytes,
            })
        }
    }
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
    playback.set_loop_settings(playback_loop_settings(&fmid))?;
    Ok(LoadedInput {
        load_message,
        midi_bytes: fmid.midi_bytes,
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
    let mut settings = PlaybackLoopSettings::default();
    settings.enabled = matches!(fmid.looping.mode, FmidLoopMode::Infinite | FmidLoopMode::Counted)
        && fmid.looping.enabled
        && fmid.looping.end_tick > fmid.looping.start_tick;
    settings.start_tick = fmid.looping.start_tick;
    settings.end_tick = fmid.looping.end_tick;
    settings.loop_count = (fmid.looping.loop_count as u32).max(1);
    settings
}

fn render_without_stop(
    playback: &mut PlaybackController,
    frames: usize,
) -> Result<RenderStemProbeReport, Box<dyn std::error::Error>> {
    let mut reports = playback.render_probe_sequence_with_stems(&[frames], false)?;
    reports
        .pop()
        .ok_or_else(|| "render sequence produced no report".into())
}

fn overlaps(start_a: u64, end_a: u64, start_b: u64, end_b: u64) -> bool {
    start_a <= end_b && end_a >= start_b
}

fn top_strips(strips: &[RealtimeStripSnapshot], limit: usize) -> Vec<StripSummary> {
    let mut summaries = strips
        .iter()
        .map(|strip| StripSummary {
            soundfont_id: strip.soundfont_id.clone(),
            midi_channel: strip.midi_channel,
            midi_bank: strip.midi_bank,
            midi_program: strip.midi_program,
            peak: strip.meter.peak,
            rms: strip.meter.rms,
            audible: strip.audible,
            active_notes: strip.active_notes.clone(),
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .rms
            .partial_cmp(&left.rms)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.peak.partial_cmp(&left.peak).unwrap_or(Ordering::Equal))
    });
    summaries.truncate(limit);
    summaries
}

fn top_stems(stems: &[StemRenderBlock], limit: usize) -> Vec<StemSummary> {
    let mut summaries = stems
        .iter()
        .filter_map(|stem| {
            let sample_count = stem.left.len().saturating_add(stem.right.len());
            if sample_count == 0 {
                return None;
            }
            let peak = stem
                .left
                .iter()
                .chain(stem.right.iter())
                .map(|sample| sample.abs())
                .fold(0.0f32, f32::max);
            let sum_squares = stem
                .left
                .iter()
                .chain(stem.right.iter())
                .map(|sample| sample * sample)
                .sum::<f32>();
            let rms = (sum_squares / sample_count as f32).sqrt();
            if peak <= 0.0 && rms <= 0.0 && stem.active_notes.is_empty() {
                return None;
            }
            Some(StemSummary {
                soundfont_id: stem.identity.soundfont_id.clone(),
                display_name: stem.display_name.clone(),
                midi_channel: stem.identity.midi_channel,
                midi_bank: stem.identity.midi_bank,
                midi_program: stem.identity.midi_program,
                is_percussion: stem.identity.is_percussion,
                peak,
                rms,
                active_notes: stem.active_notes.clone(),
            })
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .rms
            .partial_cmp(&left.rms)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.peak.partial_cmp(&left.peak).unwrap_or(Ordering::Equal))
    });
    summaries.truncate(limit);
    summaries
}

fn top_mix(strips: &[RenderProbeStripMixDiagnostics], limit: usize) -> Vec<MixSummary> {
    let mut summaries = strips
        .iter()
        .filter(|strip| {
            strip.estimated_post_peak > 0.0
                || strip.estimated_post_rms > 0.0
                || !strip.active_notes.is_empty()
        })
        .map(|strip| MixSummary {
            soundfont_id: strip.soundfont_id.clone(),
            display_name: strip.display_name.clone(),
            midi_channel: strip.midi_channel,
            midi_program: strip.midi_program,
            input_peak: strip.input_meter.peak,
            input_rms: strip.input_meter.rms,
            estimated_post_peak: strip.estimated_post_peak,
            estimated_post_rms: strip.estimated_post_rms,
            applied_gain: strip.applied_gain,
            session_gain: strip.session_gain,
            smart_mix_gain: strip.smart_mix_gain,
            lookahead_gain: strip.lookahead_gain,
            normalization_gain: strip.normalization_gain,
            left_pan_gain: strip.left_pan_gain,
            right_pan_gain: strip.right_pan_gain,
            automatic_processing: strip.automatic_processing,
            audible: strip.audible,
            active_notes: strip.active_notes.clone(),
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .estimated_post_rms
            .partial_cmp(&left.estimated_post_rms)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                right
                    .estimated_post_peak
                    .partial_cmp(&left.estimated_post_peak)
                    .unwrap_or(Ordering::Equal)
            })
    });
    summaries.truncate(limit);
    summaries
}

fn print_record(record: &WindowRecord) {
    println!(
        "{{\"event\":\"render_window_probe_sample\",\"iteration\":{},\"tick_before\":{},\"tick_after\":{},\"seconds_before\":{:.6},\"seconds_after\":{:.6},\"frames\":{},\"samples\":{},\"render_peak\":{:.6},\"output_peak\":{:.6},\"output_rms\":{:.6},\"active_strip_count\":{},\"recovered_error_count\":{},\"top_strips\":{}}}",
        record.iteration,
        record.tick_before,
        record.tick_after,
        record.seconds_before,
        record.seconds_after,
        record.report.report.frames,
        record.report.report.samples,
        record.report.report.peak,
        record.output_peak,
        record.output_rms,
        record.active_strip_count,
        record.report.report.recovered_error_count,
        json_top_strips(&record.top_strips),
    );
    println!(
        "{{\"event\":\"render_window_probe_stems\",\"iteration\":{},\"tick_before\":{},\"tick_after\":{},\"top_stems\":{}}}",
        record.iteration,
        record.tick_before,
        record.tick_after,
        json_top_stems(&record.top_stems),
    );
    println!(
        "{{\"event\":\"render_window_probe_mix\",\"iteration\":{},\"tick_before\":{},\"tick_after\":{},\"top_mix\":{}}}",
        record.iteration,
        record.tick_before,
        record.tick_after,
        json_top_mix(&record.top_mix),
    );
}

fn print_summary(summary: &ProbeSummary) {
    println!(
        "{{\"event\":\"render_window_probe_summary\",\"record_count\":{},\"baseline_rms\":{:.6},\"baseline_peak\":{:.6},\"window_min_rms\":{:.6},\"window_min_peak\":{:.6},\"min_rms_tick_before\":{},\"min_rms_tick_after\":{},\"largest_drop_ratio\":{:.6},\"largest_drop_from_tick\":{},\"largest_drop_to_tick\":{},\"nearby_event_count\":{},\"recovered_error_count\":{},\"likely_cause\":{}}}",
        summary.record_count,
        summary.baseline_rms,
        summary.baseline_peak,
        summary.window_min_rms,
        summary.window_min_peak,
        summary.min_rms_tick_before,
        summary.min_rms_tick_after,
        summary.largest_drop_ratio,
        summary.largest_drop_from_tick,
        summary.largest_drop_to_tick,
        summary.nearby_event_count,
        summary.recovered_error_count,
        json_string(&summary.likely_cause),
    );
}

fn print_render_error(event: &RenderErrorTraceEvent) {
    println!(
        "{{\"event\":\"render_window_probe_failure\",\"source\":{},\"failure_kind\":{},\"detail\":{},\"backtrace\":{},\"frames_requested\":{}}}",
        json_string(&event.source),
        json_string(&event.failure_kind),
        json_string(&event.detail),
        json_option_string(event.backtrace.as_deref()),
        event.frames_requested,
    );
}

fn print_midi_scan(scan: &PlaybackMidiScanDiagnostics) {
    println!(
        "{{\"event\":\"render_window_probe_midi_scan\",\"loop_style\":{},\"system_modes\":{},\"percussion_channels\":{},\"warnings\":{},\"sysex_event_count\":{},\"recognized_sysex_event_count\":{}}}",
        json_string(&scan.loop_style),
        json_string_vec(&scan.system_modes),
        json_u8_vec(&scan.percussion_channels),
        json_string_vec(&scan.warnings),
        scan.sysex_event_count,
        scan.recognized_sysex_event_count,
    );
}

fn print_event_summaries(events: &[MidiEventSummary]) {
    for event in events {
        println!(
            "{{\"event\":\"render_window_probe_midi_event\",\"tick\":{},\"time_seconds\":{:.6},\"kind\":{},\"channel\":{},\"data1\":{},\"data2\":{},\"description\":{}}}",
            event.tick,
            event.time_seconds,
            json_string(&event.kind),
            json_option_u8(event.channel),
            json_option_u8(event.data1),
            json_option_u8(event.data2),
            json_string(&event.description),
        );
    }
}

fn summarize(
    records: &[WindowRecord],
    errors: &[RenderErrorTraceEvent],
    midi_file: &MidiFile,
    args: &ProbeArgs,
    scan: &PlaybackMidiScanDiagnostics,
) -> ProbeSummary {
    let analysis_start = args.start_tick as u64;
    let analysis_end = args.end_tick as u64;
    let baseline_records = records
        .iter()
        .filter(|record| record.tick_after < analysis_start)
        .collect::<Vec<_>>();
    let window_records = records
        .iter()
        .filter(|record| overlaps(record.tick_before, record.tick_after, analysis_start, analysis_end))
        .collect::<Vec<_>>();

    let baseline_rms = baseline_records
        .iter()
        .map(|record| record.output_rms)
        .fold(0.0f32, f32::max);
    let baseline_peak = baseline_records
        .iter()
        .map(|record| record.output_peak)
        .fold(0.0f32, f32::max);

    let min_record = window_records
        .iter()
        .min_by(|left, right| left.output_rms.partial_cmp(&right.output_rms).unwrap_or(Ordering::Equal))
        .copied();

    let mut largest_drop_ratio = 0.0f32;
    let mut largest_drop_from_tick = 0u64;
    let mut largest_drop_to_tick = 0u64;
    for pair in records.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        if !overlaps(current.tick_before, current.tick_after, analysis_start, analysis_end) {
            continue;
        }
        if previous.output_rms <= 0.0 {
            continue;
        }
        let ratio = 1.0 - (current.output_rms / previous.output_rms).clamp(0.0, 1.0);
        if ratio > largest_drop_ratio {
            largest_drop_ratio = ratio;
            largest_drop_from_tick = previous.tick_before;
            largest_drop_to_tick = current.tick_before;
        }
    }

    let nearby_events = midi_file.get_event_summaries_in_tick_range(
        args.start_tick.saturating_sub(args.event_context_ticks),
        args.end_tick.saturating_add(args.event_context_ticks),
    );
    let likely_cause = diagnose_cause(
        baseline_rms,
        largest_drop_ratio,
        min_record,
        &nearby_events,
        scan,
    );

    ProbeSummary {
        record_count: records.len(),
        baseline_rms,
        baseline_peak,
        window_min_rms: min_record.map(|record| record.output_rms).unwrap_or_default(),
        window_min_peak: min_record.map(|record| record.output_peak).unwrap_or_default(),
        min_rms_tick_before: min_record.map(|record| record.tick_before).unwrap_or_default(),
        min_rms_tick_after: min_record.map(|record| record.tick_after).unwrap_or_default(),
        largest_drop_ratio,
        largest_drop_from_tick,
        largest_drop_to_tick,
        likely_cause,
        nearby_event_count: nearby_events.len(),
        recovered_error_count: errors.len(),
    }
}

fn diagnose_cause(
    baseline_rms: f32,
    largest_drop_ratio: f32,
    min_record: Option<&WindowRecord>,
    nearby_events: &[MidiEventSummary],
    scan: &PlaybackMidiScanDiagnostics,
) -> String {
    let volume_events = nearby_events
        .iter()
        .filter(|event| {
            event.kind == "control-change"
                && matches!(event.data1, Some(7 | 11 | 120 | 121 | 123))
        })
        .count();
    if volume_events > 0 {
        return format!(
            "probable MIDI controller change: {} volume/expression/all-notes-off style event(s) near the dip",
            volume_events,
        );
    }
    if scan.sysex_event_count > 0 {
        return "possible MIDI/system-mode event influence; inspect nearby SysEx summaries".to_owned();
    }
    if let Some(record) = min_record {
        if baseline_rms > 0.0
            && record.output_rms < baseline_rms * 0.35
            && largest_drop_ratio > 0.4
            && record.active_strip_count > 0
        {
            let loud_strips = record
                .top_strips
                .iter()
                .filter(|strip| strip.rms > 0.01)
                .count();
            if loud_strips == 0 {
                return "rendered mix drops with active strips present but no single strip staying loud; likely global gain or mix-path change".to_owned();
            }
            return "rendered mix drops sharply while some strips remain active; likely channel/controller or mix-balance change rather than injected silence".to_owned();
        }
    }
    "no clear render fault signature in this window; inspect sample records and MIDI events together".to_owned()
}

fn json_top_strips(strips: &[StripSummary]) -> String {
    format!(
        "[{}]",
        strips
            .iter()
            .map(|strip| {
                format!(
                    "{{\"soundfont_id\":{},\"midi_channel\":{},\"midi_bank\":{},\"midi_program\":{},\"peak\":{:.6},\"rms\":{:.6},\"audible\":{},\"active_notes\":{}}}",
                    json_string(&strip.soundfont_id),
                    strip.midi_channel,
                    strip.midi_bank,
                    strip.midi_program,
                    strip.peak,
                    strip.rms,
                    if strip.audible { "true" } else { "false" },
                    json_u8_vec(&strip.active_notes),
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_top_stems(stems: &[StemSummary]) -> String {
    format!(
        "[{}]",
        stems
            .iter()
            .map(|stem| {
                format!(
                    "{{\"soundfont_id\":{},\"display_name\":{},\"midi_channel\":{},\"midi_bank\":{},\"midi_program\":{},\"is_percussion\":{},\"peak\":{:.6},\"rms\":{:.6},\"active_notes\":{}}}",
                    json_string(&stem.soundfont_id),
                    json_option_string(stem.display_name.as_deref()),
                    json_option_u8(stem.midi_channel),
                    stem.midi_bank.map(|bank| bank.to_string()).unwrap_or_else(|| "null".to_owned()),
                    json_option_u8(stem.midi_program),
                    if stem.is_percussion { "true" } else { "false" },
                    stem.peak,
                    stem.rms,
                    json_u8_vec(&stem.active_notes),
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_top_mix(strips: &[MixSummary]) -> String {
    format!(
        "[{}]",
        strips
            .iter()
            .map(|strip| {
                format!(
                    "{{\"soundfont_id\":{},\"display_name\":{},\"midi_channel\":{},\"midi_program\":{},\"input_peak\":{:.6},\"input_rms\":{:.6},\"estimated_post_peak\":{:.6},\"estimated_post_rms\":{:.6},\"applied_gain\":{:.6},\"session_gain\":{:.6},\"smart_mix_gain\":{:.6},\"lookahead_gain\":{:.6},\"normalization_gain\":{:.6},\"left_pan_gain\":{:.6},\"right_pan_gain\":{:.6},\"automatic_processing\":{},\"audible\":{},\"active_notes\":{}}}",
                    json_string(&strip.soundfont_id),
                    json_option_string(strip.display_name.as_deref()),
                    strip.midi_channel,
                    strip.midi_program,
                    strip.input_peak,
                    strip.input_rms,
                    strip.estimated_post_peak,
                    strip.estimated_post_rms,
                    strip.applied_gain,
                    strip.session_gain,
                    strip.smart_mix_gain,
                    strip.lookahead_gain,
                    strip.normalization_gain,
                    strip.left_pan_gain,
                    strip.right_pan_gain,
                    if strip.automatic_processing { "true" } else { "false" },
                    if strip.audible { "true" } else { "false" },
                    json_u8_vec(&strip.active_notes),
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    )
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

fn json_option_u8(value: Option<u8>) -> String {
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
            ch if ch.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", ch as u32);
            }
            ch => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}
