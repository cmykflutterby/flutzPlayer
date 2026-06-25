use std::{
    env, fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

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
    let mut playback = Some(new_controller(&args, catalog.clone()));

    print_policy_header(&args);
    let mut last_floor_comparison = None;
    for cycle in 0..args.cycles {
        last_floor_comparison = match args.sequence {
            ProbeSequence::HeavySmall => {
                run_heavy_small_cycle(&mut playback, &args, &catalog, cycle)?;
                None
            }
            ProbeSequence::SmallHeavySmall => Some(run_small_heavy_small_cycle(
                &mut playback,
                &args,
                &catalog,
                cycle,
            )?),
        };
    }

    let final_snapshot = memory_runtime::snapshot_after_pressure_remediation(false);
    print_sample(
        "final_release",
        args.cycles.saturating_sub(1),
        0,
        args.release_mode,
        playback.as_ref(),
        None,
        final_snapshot.clone(),
        None,
    );
    let comparison = last_floor_comparison.as_ref();
    println!(
        "{{\"event\":\"memory_reload_summary\",\"sequence\":\"{}\",\"release_mode\":\"{}\",\"cycles\":{},\"small_floor_restored\":{},\"small_floor_delta_working_set_bytes\":{},\"small_floor_delta_active_bytes\":{},\"small_floor_delta_resident_bytes\":{},\"small_floor_return_time_ms\":{},\"final_working_set_bytes\":{},\"final_jemalloc_active_bytes\":{},\"final_jemalloc_resident_bytes\":{},\"final_jemalloc_retained_bytes\":{},\"final_working_set_minus_jemalloc_resident_bytes\":{},\"errors\":\"{}\"}}",
        args.sequence.as_str(),
        args.release_mode.as_str(),
        args.cycles,
        optional_bool_json(comparison.map(|comparison| comparison.restored)),
        optional_i64_json(comparison.map(|comparison| comparison.delta_working_set_bytes)),
        optional_i64_json(comparison.map(|comparison| comparison.delta_active_bytes)),
        optional_i64_json(comparison.map(|comparison| comparison.delta_resident_bytes)),
        optional_u64_json(comparison.map(|comparison| comparison.return_time_ms)),
        final_snapshot.os.working_set_bytes,
        final_snapshot.totals.active_bytes,
        final_snapshot.totals.resident_bytes,
        final_snapshot.totals.retained_bytes,
        final_snapshot.os.working_set_minus_jemalloc_resident_bytes,
        escape_json(&final_snapshot.errors.join(" | ")),
    );
    Ok(())
}

#[derive(Debug, Clone)]
struct ProbeArgs {
    data_dir: PathBuf,
    heavy_input: PathBuf,
    small_input: PathBuf,
    cycles: usize,
    idle_samples: usize,
    idle_ms: u64,
    render_frames: usize,
    sequence: ProbeSequence,
    release_mode: ReleaseMode,
    require_partial: bool,
    skip_small: bool,
    settle_window: usize,
    settle_delta_bytes: u64,
    floor_tolerance_bytes: u64,
}

impl ProbeArgs {
    fn parse() -> Result<Self, String> {
        let mut data_dir = PathBuf::from(DEFAULT_DATA_DIR);
        let mut heavy_input = PathBuf::from(DEFAULT_INPUT);
        let mut small_input = PathBuf::from(DEFAULT_INPUT);
        let mut cycles = 1usize;
        let mut idle_samples = 8usize;
        let mut idle_ms = 500u64;
        let mut render_frames = 512usize;
        let mut sequence = ProbeSequence::SmallHeavySmall;
        let mut release_mode = ReleaseMode::StopMaintain;
        let mut require_partial = false;
        let mut skip_small = false;
        let mut settle_window = 3usize;
        let mut settle_delta_bytes = 8 * 1024 * 1024u64;
        let mut floor_tolerance_bytes = 64 * 1024 * 1024u64;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => data_dir = PathBuf::from(next_arg(&mut args, "--data-dir")?),
                "--heavy-input" => {
                    heavy_input = PathBuf::from(next_arg(&mut args, "--heavy-input")?)
                }
                "--small-input" => {
                    small_input = PathBuf::from(next_arg(&mut args, "--small-input")?)
                }
                "--cycles" => cycles = parse_usize(&next_arg(&mut args, "--cycles")?, "--cycles")?,
                "--idle-samples" => {
                    idle_samples =
                        parse_usize(&next_arg(&mut args, "--idle-samples")?, "--idle-samples")?
                }
                "--idle-ms" => {
                    idle_ms = parse_u64(&next_arg(&mut args, "--idle-ms")?, "--idle-ms")?
                }
                "--render-frames" => {
                    render_frames =
                        parse_usize(&next_arg(&mut args, "--render-frames")?, "--render-frames")?
                }
                "--sequence" => {
                    sequence = ProbeSequence::parse(&next_arg(&mut args, "--sequence")?)?
                }
                "--release-mode" => {
                    release_mode = ReleaseMode::parse(&next_arg(&mut args, "--release-mode")?)?
                }
                "--require-partial" => require_partial = true,
                "--skip-small" => skip_small = true,
                "--settle-window" => {
                    settle_window =
                        parse_usize(&next_arg(&mut args, "--settle-window")?, "--settle-window")?
                }
                "--settle-delta-bytes" => {
                    settle_delta_bytes = parse_u64(
                        &next_arg(&mut args, "--settle-delta-bytes")?,
                        "--settle-delta-bytes",
                    )?
                }
                "--floor-tolerance-bytes" => {
                    floor_tolerance_bytes = parse_u64(
                        &next_arg(&mut args, "--floor-tolerance-bytes")?,
                        "--floor-tolerance-bytes",
                    )?
                }
                "--debug-memory" => {}
                value if !value.starts_with('-') => heavy_input = PathBuf::from(value),
                _ => return Err(format!("unsupported argument: {arg}")),
            }
        }
        Ok(Self {
            data_dir,
            heavy_input,
            small_input,
            cycles: cycles.max(1),
            idle_samples,
            idle_ms,
            render_frames: render_frames.max(1),
            sequence,
            release_mode,
            require_partial,
            skip_small,
            settle_window: settle_window.max(1),
            settle_delta_bytes,
            floor_tolerance_bytes,
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ProbeSequence {
    HeavySmall,
    SmallHeavySmall,
}

impl ProbeSequence {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "heavy-small" | "heavy_small" => Ok(Self::HeavySmall),
            "small-heavy-small" | "small_heavy_small" => Ok(Self::SmallHeavySmall),
            _ => Err(format!("unsupported --sequence: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::HeavySmall => "heavy-small",
            Self::SmallHeavySmall => "small-heavy-small",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ReleaseMode {
    StopOnly,
    StopMaintain,
    ControllerDrop,
    AggressiveRelease,
}

impl ReleaseMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "stop-only" | "stop_only" => Ok(Self::StopOnly),
            "stop-maintain" | "stop_maintain" => Ok(Self::StopMaintain),
            "controller-drop" | "controller_drop" => Ok(Self::ControllerDrop),
            "aggressive-release" | "aggressive_release" => Ok(Self::AggressiveRelease),
            _ => Err(format!("unsupported --release-mode: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::StopOnly => "stop-only",
            Self::StopMaintain => "stop-maintain",
            Self::ControllerDrop => "controller-drop",
            Self::AggressiveRelease => "aggressive-release",
        }
    }
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_usize(value: &str, flag: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} requires a positive integer"))
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

#[derive(Debug, Clone)]
struct FloorPoint {
    snapshot: MemoryRuntimeSnapshot,
    stable: bool,
    elapsed_ms: u64,
    samples: usize,
}

#[derive(Debug, Clone)]
struct FloorComparison {
    restored: bool,
    delta_working_set_bytes: i64,
    delta_active_bytes: i64,
    delta_resident_bytes: i64,
    return_time_ms: u64,
}

fn run_heavy_small_cycle(
    playback: &mut Option<PlaybackController>,
    args: &ProbeArgs,
    catalog: &[SoundFontCatalogEntry],
    cycle: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    run_load_render_stop(playback, args, catalog, &args.heavy_input, "heavy", cycle)?;
    idle_samples(playback, args, "heavy_idle", cycle)?;

    if args.skip_small {
        return Ok(());
    }

    run_load_render_stop(playback, args, catalog, &args.small_input, "small", cycle)?;
    idle_samples(playback, args, "small_idle", cycle)?;
    Ok(())
}

fn run_small_heavy_small_cycle(
    playback: &mut Option<PlaybackController>,
    args: &ProbeArgs,
    catalog: &[SoundFontCatalogEntry],
    cycle: usize,
) -> Result<FloorComparison, Box<dyn std::error::Error>> {
    run_load_render_stop(
        playback,
        args,
        catalog,
        &args.small_input,
        "baseline_small",
        cycle,
    )?;
    let baseline_floor = settle_to_floor(playback, args, "baseline_small_settle", cycle)?;

    run_load_render_stop(playback, args, catalog, &args.heavy_input, "heavy", cycle)?;
    let heavy_floor = settle_to_floor(playback, args, "heavy_settle", cycle)?;

    if args.skip_small {
        return Ok(compare_floors(args, &baseline_floor, &heavy_floor));
    }

    run_load_render_stop(
        playback,
        args,
        catalog,
        &args.small_input,
        "return_small",
        cycle,
    )?;
    let return_floor = settle_to_floor(playback, args, "return_small_settle", cycle)?;
    let comparison = compare_floors(args, &baseline_floor, &return_floor);
    println!(
        "{{\"event\":\"memory_floor_comparison\",\"cycle\":{},\"release_mode\":\"{}\",\"baseline_stable\":{},\"return_stable\":{},\"baseline_elapsed_ms\":{},\"return_elapsed_ms\":{},\"baseline_samples\":{},\"return_samples\":{},\"restored\":{},\"tolerance_bytes\":{},\"baseline_working_set_bytes\":{},\"return_working_set_bytes\":{},\"delta_working_set_bytes\":{},\"baseline_jemalloc_active_bytes\":{},\"return_jemalloc_active_bytes\":{},\"delta_active_bytes\":{},\"baseline_jemalloc_resident_bytes\":{},\"return_jemalloc_resident_bytes\":{},\"delta_resident_bytes\":{},\"return_time_ms\":{}}}",
        cycle,
        args.release_mode.as_str(),
        baseline_floor.stable,
        return_floor.stable,
        baseline_floor.elapsed_ms,
        return_floor.elapsed_ms,
        baseline_floor.samples,
        return_floor.samples,
        comparison.restored,
        args.floor_tolerance_bytes,
        baseline_floor.snapshot.os.working_set_bytes,
        return_floor.snapshot.os.working_set_bytes,
        comparison.delta_working_set_bytes,
        baseline_floor.snapshot.totals.active_bytes,
        return_floor.snapshot.totals.active_bytes,
        comparison.delta_active_bytes,
        baseline_floor.snapshot.totals.resident_bytes,
        return_floor.snapshot.totals.resident_bytes,
        comparison.delta_resident_bytes,
        comparison.return_time_ms,
    );
    Ok(comparison)
}

fn run_load_render_stop(
    playback: &mut Option<PlaybackController>,
    args: &ProbeArgs,
    catalog: &[SoundFontCatalogEntry],
    input: &PathBuf,
    label: &str,
    cycle: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if playback.is_none() {
        *playback = Some(new_controller(args, catalog.to_vec()));
    }
    print_sample(
        &format!("before_{label}_load"),
        cycle,
        0,
        args.release_mode,
        playback.as_ref(),
        None,
        memory_runtime::snapshot(),
        None,
    );
    load_probe_input(playback.as_mut().expect("controller exists"), input)?;
    ensure_partial_if_requested(args, playback.as_ref().expect("controller exists"))?;
    print_sample(
        &format!("after_{label}_load"),
        cycle,
        0,
        args.release_mode,
        playback.as_ref(),
        None,
        memory_runtime::snapshot(),
        None,
    );
    let report = render_one(
        playback.as_mut().expect("controller exists"),
        args.render_frames,
    )?;
    print_sample(
        &format!("after_{label}_render"),
        cycle,
        0,
        args.release_mode,
        playback.as_ref(),
        Some(&report),
        memory_runtime::snapshot(),
        None,
    );
    stop_and_apply_release(playback, args, catalog, label, cycle)
}

fn settle_to_floor(
    playback: &mut Option<PlaybackController>,
    args: &ProbeArgs,
    phase: &str,
    cycle: usize,
) -> Result<FloorPoint, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let target_samples = args.idle_samples.max(args.settle_window);
    let mut previous = None;
    let mut stable_streak = 0usize;
    let mut floor = None;
    for step in 0..target_samples {
        if step > 0 || args.idle_ms > 0 {
            thread::sleep(Duration::from_millis(args.idle_ms));
        }
        let snapshot = maintenance_snapshot(args.release_mode);
        let stable_now = previous
            .as_ref()
            .map(|previous| snapshots_within_delta(previous, &snapshot, args.settle_delta_bytes))
            .unwrap_or(false);
        stable_streak = if stable_now { stable_streak + 1 } else { 0 };
        let stable = stable_streak >= args.settle_window.saturating_sub(1);
        print_sample(
            phase,
            cycle,
            step + 1,
            args.release_mode,
            playback.as_ref(),
            None,
            snapshot.clone(),
            Some(if stable { "stable" } else { "settling" }),
        );
        floor = Some(FloorPoint {
            snapshot: snapshot.clone(),
            stable,
            elapsed_ms: elapsed_ms(start),
            samples: step + 1,
        });
        if stable {
            break;
        }
        previous = Some(snapshot);
    }
    floor.ok_or_else(|| "settle sampling produced no floor sample".into())
}

fn compare_floors(
    args: &ProbeArgs,
    baseline: &FloorPoint,
    current: &FloorPoint,
) -> FloorComparison {
    let delta_working_set_bytes = signed_delta(
        current.snapshot.os.working_set_bytes,
        baseline.snapshot.os.working_set_bytes,
    );
    let delta_active_bytes = signed_delta(
        current.snapshot.totals.active_bytes as u64,
        baseline.snapshot.totals.active_bytes as u64,
    );
    let delta_resident_bytes = signed_delta(
        current.snapshot.totals.resident_bytes as u64,
        baseline.snapshot.totals.resident_bytes as u64,
    );
    let tolerance = args.floor_tolerance_bytes as i64;
    FloorComparison {
        restored: delta_working_set_bytes <= tolerance
            && delta_active_bytes <= tolerance
            && delta_resident_bytes <= tolerance,
        delta_working_set_bytes,
        delta_active_bytes,
        delta_resident_bytes,
        return_time_ms: current.elapsed_ms,
    }
}

fn maintenance_snapshot(release_mode: ReleaseMode) -> MemoryRuntimeSnapshot {
    match release_mode {
        ReleaseMode::StopMaintain | ReleaseMode::ControllerDrop => {
            memory_runtime::snapshot_after_pressure_remediation(false)
        }
        ReleaseMode::StopOnly | ReleaseMode::AggressiveRelease => memory_runtime::snapshot(),
    }
}

fn snapshots_within_delta(
    previous: &MemoryRuntimeSnapshot,
    current: &MemoryRuntimeSnapshot,
    delta_bytes: u64,
) -> bool {
    abs_delta(previous.os.working_set_bytes, current.os.working_set_bytes) <= delta_bytes
        && abs_delta(
            previous.totals.active_bytes as u64,
            current.totals.active_bytes as u64,
        ) <= delta_bytes
        && abs_delta(
            previous.totals.resident_bytes as u64,
            current.totals.resident_bytes as u64,
        ) <= delta_bytes
}

fn abs_delta(left: u64, right: u64) -> u64 {
    left.abs_diff(right)
}

fn signed_delta(current: u64, baseline: u64) -> i64 {
    current as i64 - baseline as i64
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn new_controller(args: &ProbeArgs, catalog: Vec<SoundFontCatalogEntry>) -> PlaybackController {
    PlaybackController::new(
        args.data_dir.clone(),
        catalog,
        AudioBackend::Sdl3,
        false,
        false,
    )
}

fn soundfont_catalog(
    data_dir: &PathBuf,
) -> Result<Vec<SoundFontCatalogEntry>, Box<dyn std::error::Error>> {
    match dat_startup_summary_for_data_dir(data_dir)? {
        DatStartupSummary::Available { soundfonts, .. } => Ok(soundfonts),
        DatStartupSummary::Unavailable(error) => Err(error.into()),
    }
}

fn load_probe_input(
    playback: &mut PlaybackController,
    input: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    match input.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("fmid") => load_fmid(playback, input),
        _ => {
            playback.load_midi_file(input, &[])?;
            Ok(())
        }
    }
}

fn load_fmid(
    playback: &mut PlaybackController,
    input: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(input)?;
    let fmid = read_fmid(&bytes)?;
    let requested_soundfonts = requested_soundfonts(&fmid);
    playback.load_midi_bytes(
        fmid.midi_bytes.clone(),
        fmid.project.source_midi_filename.clone(),
        &requested_soundfonts,
    )?;
    playback.set_loop_settings(playback_loop_settings(&fmid))?;
    Ok(())
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

fn ensure_partial_if_requested(
    args: &ProbeArgs,
    playback: &PlaybackController,
) -> Result<(), Box<dyn std::error::Error>> {
    if !args.require_partial {
        return Ok(());
    }
    let metrics = playback.debug_metrics();
    if metrics.subset_plans.plan_count == 0
        || metrics.soundfont_cache.subset_entries == 0
        || metrics.soundfont_cache.full_entries != 0
        || metrics.subset_transition.full_fallback_count != 0
    {
        return Err(format!(
            "partial playback requirement failed: plans={} subset_entries={} full_entries={} full_fallback={}",
            metrics.subset_plans.plan_count,
            metrics.soundfont_cache.subset_entries,
            metrics.soundfont_cache.full_entries,
            metrics.subset_transition.full_fallback_count,
        )
        .into());
    }
    Ok(())
}

fn render_one(
    playback: &mut PlaybackController,
    frames: usize,
) -> Result<RenderProbeReport, Box<dyn std::error::Error>> {
    let mut reports = playback.render_probe_sequence(&[frames], true)?;
    reports
        .pop()
        .ok_or_else(|| "render sequence produced no report".into())
}

fn stop_and_apply_release(
    playback: &mut Option<PlaybackController>,
    args: &ProbeArgs,
    catalog: &[SoundFontCatalogEntry],
    label: &str,
    cycle: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(controller) = playback.as_mut() {
        controller.stop()?;
    }
    print_sample(
        &format!("after_{label}_stop"),
        cycle,
        0,
        args.release_mode,
        playback.as_ref(),
        None,
        memory_runtime::snapshot_after_maintenance(false),
        None,
    );
    match args.release_mode {
        ReleaseMode::StopOnly | ReleaseMode::StopMaintain => {}
        ReleaseMode::ControllerDrop => {
            *playback = None;
            let snapshot = memory_runtime::snapshot_after_pressure_remediation(false);
            print_sample(
                &format!("after_{label}_controller_drop"),
                cycle,
                0,
                args.release_mode,
                None,
                None,
                snapshot,
                None,
            );
        }
        ReleaseMode::AggressiveRelease => {
            if let Some(controller) = playback.as_mut() {
                controller.release_idle_resources()?;
            } else {
                *playback = Some(new_controller(args, catalog.to_vec()));
                playback
                    .as_mut()
                    .expect("controller exists")
                    .release_idle_resources()?;
            }
            let snapshot = memory_runtime::snapshot_after_pressure_remediation(false);
            print_sample(
                &format!("after_{label}_aggressive_release"),
                cycle,
                0,
                args.release_mode,
                playback.as_ref(),
                None,
                snapshot,
                None,
            );
        }
    }
    Ok(())
}

fn idle_samples(
    playback: &mut Option<PlaybackController>,
    args: &ProbeArgs,
    phase: &str,
    cycle: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for step in 0..args.idle_samples {
        if step > 0 || args.idle_ms > 0 {
            thread::sleep(Duration::from_millis(args.idle_ms));
        }
        let snapshot = maintenance_snapshot(args.release_mode);
        print_sample(
            phase,
            cycle,
            step + 1,
            args.release_mode,
            playback.as_ref(),
            None,
            snapshot,
            None,
        );
    }
    Ok(())
}

fn print_policy_header(args: &ProbeArgs) {
    let snapshot = memory_runtime::snapshot();
    println!(
        "{{\"event\":\"memory_reload_probe_start\",\"sequence\":\"{}\",\"heavy_input\":\"{}\",\"small_input\":\"{}\",\"data_dir\":\"{}\",\"cycles\":{},\"idle_samples\":{},\"idle_ms\":{},\"render_frames\":{},\"release_mode\":\"{}\",\"skip_small\":{},\"settle_window\":{},\"settle_delta_bytes\":{},\"floor_tolerance_bytes\":{},\"allocator\":\"{}\",\"config\":\"{}\"}}",
        args.sequence.as_str(),
        escape_json(&args.heavy_input.display().to_string()),
        escape_json(&args.small_input.display().to_string()),
        escape_json(&args.data_dir.display().to_string()),
        args.cycles,
        args.idle_samples,
        args.idle_ms,
        args.render_frames,
        args.release_mode.as_str(),
        args.skip_small,
        args.settle_window,
        args.settle_delta_bytes,
        args.floor_tolerance_bytes,
        escape_json(snapshot.allocator),
        escape_json(&snapshot.config_summary),
    );
}

fn print_sample(
    phase: &str,
    cycle: usize,
    step: usize,
    release_mode: ReleaseMode,
    playback: Option<&PlaybackController>,
    report: Option<&RenderProbeReport>,
    snapshot: MemoryRuntimeSnapshot,
    note: Option<&str>,
) {
    println!(
        "{{\"event\":\"memory_reload_sample\",\"phase\":\"{}\",\"cycle\":{},\"step\":{},\"release_mode\":\"{}\",\"note\":\"{}\",\"render\":{},\"playback\":{},\"memory\":{}}}",
        escape_json(phase),
        cycle,
        step,
        release_mode.as_str(),
        escape_json(note.unwrap_or_default()),
        render_json(report),
        playback_json(playback),
        snapshot.to_json(),
    );
}

fn render_json(report: Option<&RenderProbeReport>) -> String {
    if let Some(report) = report {
        format!(
            "{{\"frames\":{},\"samples\":{},\"peak\":{:.6},\"soundfont_count\":{},\"midi_strip_count\":{}}}",
            report.frames,
            report.samples,
            report.peak,
            report.soundfont_count,
            report.midi_strip_count,
        )
    } else {
        "null".to_owned()
    }
}

fn optional_bool_json(value: Option<bool>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned())
}

fn optional_u64_json(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned())
}

fn optional_i64_json(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned())
}

fn playback_json(playback: Option<&PlaybackController>) -> String {
    let Some(playback) = playback else {
        return "null".to_owned();
    };
    let debug = playback.debug_metrics();
    format!(
        "{{\"engine_state\":\"{}\",\"loaded_soundfonts\":{},\"loaded_midi_bytes\":{},\"tracked_component_bytes\":{},\"subset_plans\":{},\"subset_samples\":{},\"subset_planned_bytes\":{},\"subset_cache_entries\":{},\"full_cache_entries\":{},\"metadata_cache_entries\":{},\"sample_range_cache_entries\":{},\"soundfont_cache_resident_bytes\":{},\"cache_hits\":{},\"cache_misses\":{},\"subset_hits\":{},\"subset_contained_hits\":{},\"subset_misses\":{},\"compact_loaded\":{},\"full_fallback\":{},\"full_dat_entry_reads\":{},\"range_dat_reads\":{},\"full_dat_entry_bytes\":{},\"range_dat_read_bytes\":{},\"subset_source_clone_bytes\":{},\"soundfont_load_duration_ms\":{}}}",
        escape_json(&debug.engine_state),
        debug.loaded_soundfont_count,
        debug.loaded_midi_bytes,
        debug.component_memory.tracked_total_bytes,
        debug.subset_plans.plan_count,
        debug.subset_plans.sample_count,
        debug.subset_plans.planned_byte_count,
        debug.soundfont_cache.subset_entries,
        debug.soundfont_cache.full_entries,
        debug.soundfont_cache.metadata_entries,
        debug.soundfont_cache.sample_range_entries,
        debug.soundfont_cache.resident_bytes,
        debug.soundfont_cache.hits,
        debug.soundfont_cache.misses,
        debug.soundfont_cache.subset_hits,
        debug.soundfont_cache.subset_contained_hits,
        debug.soundfont_cache.subset_misses,
        debug.subset_transition.compact_loaded_count,
        debug.subset_transition.full_fallback_count,
        debug.soundfont_load.full_dat_entry_reads,
        debug.soundfont_load.range_dat_reads,
        debug.soundfont_load.full_dat_entry_bytes,
        debug.soundfont_load.range_dat_read_bytes,
        debug.soundfont_load.subset_source_clone_bytes,
        debug.soundfont_load.load_duration_ms,
    )
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}
