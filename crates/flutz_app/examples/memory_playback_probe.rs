use std::{env, fs, path::PathBuf, thread, time::Duration};

use flutz_app::{
    app::{DatStartupSummary, SoundFontCatalogEntry},
    dat_startup_summary_for_data_dir,
    memory_runtime::{self, MemoryRuntimeSnapshot},
    playback::{AudioBackend, PlaybackController, RenderProbeReport},
};
use flutz_core::default_preset_set;
use flutz_fmid::{read_fmid, FmidFile, LoopMode as FmidLoopMode, MixerSourceMode};
use flutz_synth::{PlaybackLoopMode, PlaybackLoopSettings};

#[cfg(feature = "jemalloc-memory")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_INPUT: &str = "MIDI Files/rendering-parity-midi/tmnt-water-loop-test.fmid";
const DEFAULT_DATA_DIR: &str = "drops/flutzplayer/data";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    memory_runtime::initialize_from_env(env::args().any(|arg| arg == "--debug-memory"));
    let args = ProbeArgs::parse()?;
    let catalog = soundfont_catalog(&args.data_dir)?;
    let mut playback = PlaybackController::new(
        args.data_dir.clone(),
        catalog,
        AudioBackend::Sdl3,
        false,
        false,
    );
    let loaded = load_probe_input(&mut playback, &args.input)?;
    if args.require_partial {
        ensure_partial_playback(&playback)?;
    }

    println!("memory_playback_probe: loaded");
    println!("input={}", args.input.display());
    println!("load_message={}", loaded.load_message.replace('\n', " | "));
    if let Some(loop_settings) = loaded.loop_settings {
        println!(
            "fmid_loop enabled={} mode={:?} start_tick={} end_tick={} count={}",
            loop_settings.enabled,
            loop_settings.mode,
            loop_settings.start_tick,
            loop_settings.end_tick,
            loop_settings.loop_count
        );
    }

    let initial = memory_runtime::snapshot();
    print_sample("loaded", 0, None, &playback, &initial, &initial);

    for scenario in args.scenarios() {
        run_scenario(scenario, &args, &loaded, &mut playback)?;
    }

    playback.stop()?;
    let final_snapshot = memory_runtime::snapshot_after_pressure_remediation(false);
    print_sample(
        "final_release",
        0,
        None,
        &playback,
        &initial,
        &final_snapshot,
    );
    if !final_snapshot.errors.is_empty() {
        println!("errors={}", final_snapshot.errors.join(" | "));
    }
    check_thresholds(&args, &playback, &initial, &final_snapshot)?;
    Ok(())
}

#[derive(Debug, Clone)]
struct LoadedInput {
    load_message: String,
    loop_settings: Option<PlaybackLoopSettings>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ProbeScenario {
    RenderLoopOff,
    RenderLoopOn,
    SeekSweep,
    LongStabilize,
    ReleaseFloor,
}

impl ProbeScenario {
    fn as_str(self) -> &'static str {
        match self {
            Self::RenderLoopOff => "render_loop_off",
            Self::RenderLoopOn => "render_loop_on",
            Self::SeekSweep => "seek_sweep",
            Self::LongStabilize => "long_stabilize",
            Self::ReleaseFloor => "release_floor",
        }
    }
}

struct ProbeArgs {
    data_dir: PathBuf,
    input: PathBuf,
    scenario: String,
    iterations: usize,
    sample_every: usize,
    frames: Vec<usize>,
    require_partial: bool,
    max_peak_working_set_bytes: Option<u64>,
    max_loaded_working_set_bytes: Option<u64>,
    max_full_font_read_bytes: Option<u64>,
}

impl ProbeArgs {
    fn parse() -> Result<Self, String> {
        let mut data_dir = PathBuf::from(DEFAULT_DATA_DIR);
        let mut input = PathBuf::from(DEFAULT_INPUT);
        let mut scenario = "all".to_owned();
        let mut iterations = 96usize;
        let mut sample_every = 8usize;
        let mut frames = vec![256usize, 512, 1024, 2048];
        let mut require_partial = false;
        let mut max_peak_working_set_bytes = None;
        let mut max_loaded_working_set_bytes = None;
        let mut max_full_font_read_bytes = None;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => data_dir = PathBuf::from(next_arg(&mut args, "--data-dir")?),
                "--input" | "--midi" => input = PathBuf::from(next_arg(&mut args, "--input")?),
                "--scenario" => scenario = next_arg(&mut args, "--scenario")?,
                "--iterations" => {
                    iterations = next_arg(&mut args, "--iterations")?
                        .parse()
                        .map_err(|_| "--iterations requires a positive integer".to_owned())?;
                }
                "--sample-every" => {
                    sample_every = next_arg(&mut args, "--sample-every")?
                        .parse()
                        .map_err(|_| "--sample-every requires a positive integer".to_owned())?;
                }
                "--frames" => frames = parse_frames(&next_arg(&mut args, "--frames")?)?,
                "--require-partial" => require_partial = true,
                "--max-peak-working-set-bytes" => {
                    max_peak_working_set_bytes = Some(parse_u64_arg(
                        &next_arg(&mut args, "--max-peak-working-set-bytes")?,
                        "--max-peak-working-set-bytes",
                    )?);
                }
                "--max-loaded-working-set-bytes" => {
                    max_loaded_working_set_bytes = Some(parse_u64_arg(
                        &next_arg(&mut args, "--max-loaded-working-set-bytes")?,
                        "--max-loaded-working-set-bytes",
                    )?);
                }
                "--max-full-font-read-bytes" => {
                    max_full_font_read_bytes = Some(parse_u64_arg(
                        &next_arg(&mut args, "--max-full-font-read-bytes")?,
                        "--max-full-font-read-bytes",
                    )?);
                }
                "--debug-memory" => {}
                value if !value.starts_with('-') => input = PathBuf::from(value),
                _ => return Err(format!("unsupported argument: {arg}")),
            }
        }
        if frames.is_empty() {
            return Err("--frames must contain at least one frame count".to_owned());
        }
        Ok(Self {
            data_dir,
            input,
            scenario,
            iterations: iterations.max(1),
            sample_every: sample_every.max(1),
            frames,
            require_partial,
            max_peak_working_set_bytes,
            max_loaded_working_set_bytes,
            max_full_font_read_bytes,
        })
    }

    fn scenarios(&self) -> Vec<ProbeScenario> {
        match self.scenario.as_str() {
            "render-loop-off" | "render_loop_off" => vec![ProbeScenario::RenderLoopOff],
            "render-loop-on" | "render_loop_on" => vec![ProbeScenario::RenderLoopOn],
            "seek-sweep" | "seek_sweep" => vec![ProbeScenario::SeekSweep],
            "long-stabilize" | "long_stabilize" => vec![ProbeScenario::LongStabilize],
            "release-floor" | "release_floor" => vec![ProbeScenario::ReleaseFloor],
            "all" => vec![
                ProbeScenario::RenderLoopOff,
                ProbeScenario::RenderLoopOn,
                ProbeScenario::SeekSweep,
                ProbeScenario::LongStabilize,
                ProbeScenario::ReleaseFloor,
            ],
            _ => vec![ProbeScenario::RenderLoopOff],
        }
    }
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_frames(value: &str) -> Result<Vec<usize>, String> {
    value
        .split(',')
        .map(|part| {
            part.trim()
                .parse()
                .map_err(|_| format!("invalid frame count: {part}"))
        })
        .collect()
}

fn parse_u64_arg(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn load_probe_input(
    playback: &mut PlaybackController,
    input: &PathBuf,
) -> Result<LoadedInput, Box<dyn std::error::Error>> {
    match input.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("fmid") => load_fmid(playback, input),
        _ => Ok(LoadedInput {
            load_message: playback.load_midi_file(input, &[])?,
            loop_settings: None,
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
    let load_message = playback.load_midi_bytes(
        fmid.midi_bytes.clone(),
        fmid.project.source_midi_filename.clone(),
        &requested_soundfonts,
    )?;
    let loop_settings = playback_loop_settings(&fmid);
    playback.set_loop_settings(loop_settings)?;
    Ok(LoadedInput {
        load_message,
        loop_settings: Some(loop_settings),
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

fn ensure_partial_playback(
    playback: &PlaybackController,
) -> Result<(), Box<dyn std::error::Error>> {
    let metrics = playback.debug_metrics();
    if metrics.subset_plans.plan_count == 0 {
        return Err("playback did not build any subset plans".into());
    }
    if metrics.soundfont_cache.subset_entries == 0 || metrics.soundfont_cache.full_entries != 0 {
        return Err(format!(
            "playback did not stay on compact subset fonts: subset_entries={} full_entries={}",
            metrics.soundfont_cache.subset_entries, metrics.soundfont_cache.full_entries
        )
        .into());
    }
    if metrics.subset_transition.full_fallback_count != 0 {
        return Err(format!(
            "playback reported full-font fallback for {} font(s)",
            metrics.subset_transition.full_fallback_count
        )
        .into());
    }
    Ok(())
}

fn run_scenario(
    scenario: ProbeScenario,
    args: &ProbeArgs,
    loaded: &LoadedInput,
    playback: &mut PlaybackController,
) -> Result<(), Box<dyn std::error::Error>> {
    let start = memory_runtime::snapshot_after_maintenance(false);
    print_sample(scenario.as_str(), 0, None, playback, &start, &start);
    match scenario {
        ProbeScenario::RenderLoopOff => {
            playback.set_loop_enabled(false)?;
            playback.seek_transport_fraction(0.0)?;
            render_iterations(scenario, args, playback, &start)?;
        }
        ProbeScenario::RenderLoopOn => {
            apply_loaded_loop(loaded, playback)?;
            let start_tick = loaded
                .loop_settings
                .map(|settings| settings.start_tick)
                .unwrap_or(0);
            playback.seek_transport_tick(start_tick)?;
            render_iterations(scenario, args, playback, &start)?;
        }
        ProbeScenario::SeekSweep => {
            apply_loaded_loop(loaded, playback)?;
            let fractions = [0.0f32, 0.17, 0.39, 0.63, 0.82, 0.96];
            for index in 0..args.iterations {
                playback.seek_transport_fraction(fractions[index % fractions.len()])?;
                let report = render_one(playback, args.frames[index % args.frames.len()])?;
                sample_render(scenario, index + 1, args, playback, &start, Some(report));
            }
        }
        ProbeScenario::LongStabilize => {
            apply_loaded_loop(loaded, playback)?;
            render_iterations(scenario, args, playback, &start)?;
        }
        ProbeScenario::ReleaseFloor => {
            playback.stop()?;
            for index in 0..6usize {
                let snapshot = memory_runtime::snapshot_after_pressure_remediation(false);
                print_sample(
                    scenario.as_str(),
                    index + 1,
                    None,
                    playback,
                    &start,
                    &snapshot,
                );
                thread::sleep(Duration::from_millis(450));
            }
            thread::sleep(Duration::from_millis(2100));
            let snapshot = memory_runtime::snapshot_after_pressure_remediation(false);
            print_sample(scenario.as_str(), 7, None, playback, &start, &snapshot);
        }
    }
    Ok(())
}

fn apply_loaded_loop(
    loaded: &LoadedInput,
    playback: &mut PlaybackController,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(settings) = loaded.loop_settings {
        playback.set_loop_settings(settings)?;
    }
    Ok(())
}

fn render_iterations(
    scenario: ProbeScenario,
    args: &ProbeArgs,
    playback: &mut PlaybackController,
    start: &MemoryRuntimeSnapshot,
) -> Result<(), Box<dyn std::error::Error>> {
    for index in 0..args.iterations {
        let report = render_one(playback, args.frames[index % args.frames.len()])?;
        sample_render(scenario, index + 1, args, playback, start, Some(report));
    }
    Ok(())
}

fn render_one(
    playback: &mut PlaybackController,
    frames: usize,
) -> Result<RenderProbeReport, Box<dyn std::error::Error>> {
    let mut reports = playback.render_probe_sequence(&[frames], false)?;
    reports
        .pop()
        .ok_or_else(|| "render sequence produced no report".into())
}

fn sample_render(
    scenario: ProbeScenario,
    step: usize,
    args: &ProbeArgs,
    playback: &PlaybackController,
    start: &MemoryRuntimeSnapshot,
    report: Option<RenderProbeReport>,
) {
    if step == 1 || step % args.sample_every == 0 || step == args.iterations {
        let snapshot = memory_runtime::snapshot();
        print_sample(
            scenario.as_str(),
            step,
            report.as_ref(),
            playback,
            start,
            &snapshot,
        );
    }
}

fn print_sample(
    scenario: &str,
    step: usize,
    report: Option<&RenderProbeReport>,
    playback: &PlaybackController,
    start: &MemoryRuntimeSnapshot,
    snapshot: &MemoryRuntimeSnapshot,
) {
    let debug = playback.debug_metrics();
    let diagnosis = diagnose(start, snapshot);
    println!(
        "{{\"scenario\":\"{}\",\"step\":{},\"diagnosis\":\"{}\",\"frames\":{},\"samples\":{},\"peak\":{:.6},\"transport_tick\":{},\"transport_seconds\":{:.6},\"render_calls\":{},\"tracked_component_bytes\":{},\"jemalloc_active_bytes\":{},\"jemalloc_resident_bytes\":{},\"jemalloc_retained_bytes\":{},\"jemalloc_mapped_bytes\":{},\"jemalloc_active_delta_bytes\":{},\"jemalloc_resident_delta_bytes\":{},\"jemalloc_retained_delta_bytes\":{},\"working_set_bytes\":{},\"peak_working_set_bytes\":{},\"pagefile_bytes\":{},\"page_fault_count\":{},\"working_set_delta_bytes\":{},\"pagefile_delta_bytes\":{},\"working_set_minus_jemalloc_resident_bytes\":{},\"pagefile_minus_jemalloc_active_bytes\":{},\"snapshot_growth_working_set_bytes\":{},\"snapshot_growth_jemalloc_resident_bytes\":{}}}",
        escape_json(scenario),
        step,
        diagnosis,
        report.map(|report| report.frames).unwrap_or_default(),
        report.map(|report| report.samples).unwrap_or_default(),
        report.map(|report| report.peak).unwrap_or_default(),
        debug.transport_tick,
        debug.transport_seconds,
        debug.render_churn.render_calls,
        debug.component_memory.tracked_total_bytes,
        snapshot.totals.active_bytes,
        snapshot.totals.resident_bytes,
        snapshot.totals.retained_bytes,
        snapshot.totals.mapped_bytes,
        snapshot.totals.active_bytes as i64 - start.totals.active_bytes as i64,
        snapshot.totals.resident_bytes as i64 - start.totals.resident_bytes as i64,
        snapshot.totals.retained_bytes as i64 - start.totals.retained_bytes as i64,
        snapshot.os.working_set_bytes,
        snapshot.os.peak_working_set_bytes,
        snapshot.os.pagefile_bytes,
        snapshot.os.page_fault_count,
        snapshot.os.working_set_bytes as i64 - start.os.working_set_bytes as i64,
        snapshot.os.pagefile_bytes as i64 - start.os.pagefile_bytes as i64,
        snapshot.os.working_set_minus_jemalloc_resident_bytes,
        snapshot.os.pagefile_minus_jemalloc_active_bytes,
        snapshot.os.working_set_growth_bytes,
        snapshot.totals.resident_growth_bytes,
    );
    println!(
        "{{\"scenario\":\"{}\",\"step\":{},\"event\":\"playback_metrics\",\"loaded_soundfonts\":{},\"subset_plans\":{},\"subset_samples\":{},\"subset_planned_ranges\":{},\"subset_planned_bytes\":{},\"subset_cache_entries\":{},\"full_cache_entries\":{},\"metadata_cache_entries\":{},\"sample_range_cache_entries\":{},\"cache_hits\":{},\"cache_misses\":{},\"subset_hits\":{},\"subset_contained_hits\":{},\"subset_misses\":{},\"compact_loaded\":{},\"full_fallback\":{},\"exact_subset_hits\":{},\"superset_growths\":{},\"missing_samples\":{},\"missing_bytes\":{},\"soundfont_cache_resident_bytes\":{},\"dat_read_task_count\":{},\"full_dat_entry_reads\":{},\"range_dat_reads\":{},\"full_dat_entry_bytes\":{},\"range_dat_read_bytes\":{},\"subset_request_count\":{},\"subset_source_clone_bytes\":{},\"non_subset_full_load_count\":{},\"json_resource_font_count\":{},\"index_json_plan_count\":{},\"soundfont_load_duration_ms\":{}}}",
        escape_json(scenario),
        step,
        debug.loaded_soundfont_count,
        debug.subset_plans.plan_count,
        debug.subset_plans.sample_count,
        debug.subset_plans.planned_range_count,
        debug.subset_plans.planned_byte_count,
        debug.soundfont_cache.subset_entries,
        debug.soundfont_cache.full_entries,
        debug.soundfont_cache.metadata_entries,
        debug.soundfont_cache.sample_range_entries,
        debug.soundfont_cache.hits,
        debug.soundfont_cache.misses,
        debug.soundfont_cache.subset_hits,
        debug.soundfont_cache.subset_contained_hits,
        debug.soundfont_cache.subset_misses,
        debug.subset_transition.compact_loaded_count,
        debug.subset_transition.full_fallback_count,
        debug.subset_transition.exact_subset_hits,
        debug.subset_transition.superset_growth_events,
        debug.subset_transition.missing_sample_count,
        debug.subset_transition.missing_planned_bytes,
        debug.soundfont_cache.resident_bytes,
        debug.soundfont_load.dat_read_task_count,
        debug.soundfont_load.full_dat_entry_reads,
        debug.soundfont_load.range_dat_reads,
        debug.soundfont_load.full_dat_entry_bytes,
        debug.soundfont_load.range_dat_read_bytes,
        debug.soundfont_load.subset_request_count,
        debug.soundfont_load.subset_source_clone_bytes,
        debug.soundfont_load.non_subset_full_load_count,
        debug.soundfont_load.json_resource_font_count,
        debug.soundfont_load.index_json_plan_count,
        debug.soundfont_load.load_duration_ms,
    );
}

fn check_thresholds(
    args: &ProbeArgs,
    playback: &PlaybackController,
    loaded: &MemoryRuntimeSnapshot,
    final_snapshot: &MemoryRuntimeSnapshot,
) -> Result<(), Box<dyn std::error::Error>> {
    let debug = playback.debug_metrics();
    let mut failures = Vec::new();
    if let Some(max) = args.max_peak_working_set_bytes {
        if final_snapshot.os.peak_working_set_bytes > max {
            failures.push(format!(
                "peak_working_set_bytes={} exceeded max={}",
                final_snapshot.os.peak_working_set_bytes, max
            ));
        }
    }
    if let Some(max) = args.max_loaded_working_set_bytes {
        if loaded.os.working_set_bytes > max {
            failures.push(format!(
                "loaded_working_set_bytes={} exceeded max={}",
                loaded.os.working_set_bytes, max
            ));
        }
    }
    if let Some(max) = args.max_full_font_read_bytes {
        if debug.soundfont_load.full_dat_entry_bytes > max {
            failures.push(format!(
                "full_dat_entry_bytes={} exceeded max={}",
                debug.soundfont_load.full_dat_entry_bytes, max
            ));
        }
    }

    println!(
        "{{\"event\":\"probe_thresholds\",\"status\":\"{}\",\"peak_working_set_bytes\":{},\"loaded_working_set_bytes\":{},\"full_dat_entry_bytes\":{},\"range_dat_read_bytes\":{},\"failures\":\"{}\"}}",
        if failures.is_empty() { "pass" } else { "fail" },
        final_snapshot.os.peak_working_set_bytes,
        loaded.os.working_set_bytes,
        debug.soundfont_load.full_dat_entry_bytes,
        debug.soundfont_load.range_dat_read_bytes,
        escape_json(&failures.join(" | ")),
    );
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; ").into())
    }
}

fn diagnose(start: &MemoryRuntimeSnapshot, snapshot: &MemoryRuntimeSnapshot) -> &'static str {
    let working_delta = snapshot.os.working_set_bytes as i64 - start.os.working_set_bytes as i64;
    let resident_delta = snapshot.totals.resident_bytes as i64 - start.totals.resident_bytes as i64;
    let active_delta = snapshot.totals.active_bytes as i64 - start.totals.active_bytes as i64;
    if working_delta.abs() < 4 * 1024 * 1024 {
        "stable"
    } else if resident_delta > 0 && (working_delta - resident_delta).abs() < working_delta.abs() / 3
    {
        "jemalloc_resident_tracks_working_set"
    } else if active_delta > 0 && resident_delta > active_delta.saturating_mul(2) {
        "allocator_retained_or_dirty_pages"
    } else if working_delta > resident_delta.saturating_add(16 * 1024 * 1024) {
        "native_or_os_working_set_gap"
    } else {
        "mixed_growth"
    }
}

fn soundfont_catalog(
    data_dir: &PathBuf,
) -> Result<Vec<SoundFontCatalogEntry>, Box<dyn std::error::Error>> {
    match dat_startup_summary_for_data_dir(data_dir)? {
        DatStartupSummary::Available { soundfonts, .. } => Ok(soundfonts),
        DatStartupSummary::Unavailable(error) => Err(error.into()),
    }
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
