use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use flutz_app::{
    memory_runtime,
    playback::{AudioBackend, PlaybackController},
};
use flutz_formats::{builtin_registry, DecodedAudioStreamSource};
use flutz_synth::{PlaybackLoopMode, PlaybackLoopSettings};

#[cfg(feature = "jemalloc-memory")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_INPUT: &str = "encoded-audio/memory-test.m4a";
const DEFAULT_OUTPUT: &str = "_local/runtime-tests/phase65-decoded-stream-playback-probe.txt";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = ProbeArgs::parse()?;
    memory_runtime::initialize_from_env(args.debug_memory);
    let initial_memory = memory_runtime::snapshot();
    let registry = builtin_registry();
    let extension = args
        .input
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or("probe input has no extension")?;
    let descriptor = registry
        .find_by_extension(extension)
        .ok_or("probe input extension is not registered")?;

    let mut playback = PlaybackController::new(
        PathBuf::from("drops/flutzplayer/data"),
        Vec::new(),
        AudioBackend::Sdl3,
        false,
        false,
    );
    let load_started = Instant::now();
    let load_message = playback.load_decoded_audio_stream(
        args.input.clone(),
        descriptor.id,
        descriptor.friendly_name,
        DecodedAudioStreamSource::Path(args.input.clone()),
        None,
    )?;
    let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
    let metadata = playback
        .decoded_transport_metadata()
        .ok_or("decoded transport metadata was not installed")?;
    if metadata.frame_length == 0 {
        return Err("decoded stream metadata did not provide a frame length".into());
    }
    if !playback.wait_for_decoded_stream_cache(Duration::from_secs(10)) {
        return Err("decoded stream cache did not warm within 10s".into());
    }
    let load_cache = playback
        .decoded_stream_cache_debug()
        .ok_or("decoded stream cache diagnostics unavailable after load")?;
    require_streaming_cache(&load_cache, metadata.frame_length)?;

    playback.play()?;
    let first = playback.decoded_render_probe(args.frames)?;

    playback.seek_transport_fraction(0.5)?;
    if !playback.wait_for_decoded_stream_cache(Duration::from_secs(10)) {
        return Err("midpoint seek cache did not warm within 10s".into());
    }
    let midpoint_cache = playback
        .decoded_stream_cache_debug()
        .ok_or("decoded stream cache diagnostics unavailable after midpoint seek")?;
    require_streaming_cache(&midpoint_cache, metadata.frame_length)?;
    let midpoint = playback.decoded_render_probe(args.frames)?;

    let loop_start = metadata.frame_length / 3;
    let loop_end = loop_start.saturating_add((metadata.sample_rate as u64).max(args.frames as u64));
    playback.set_loop_settings(PlaybackLoopSettings {
        enabled: true,
        mode: PlaybackLoopMode::Counted,
        start_tick: loop_start,
        end_tick: loop_end.min(metadata.frame_length),
        loop_count: 2,
    })?;
    playback.seek_transport_tick(loop_end.saturating_sub((args.frames / 2) as u64))?;
    if !playback.wait_for_decoded_stream_cache(Duration::from_secs(10)) {
        return Err("loop-window seek cache did not warm within 10s".into());
    }
    let looped = playback.decoded_render_probe(args.frames)?;
    let visualizer = playback.visualizer_frame();
    let final_memory = memory_runtime::snapshot_after_pressure_remediation(false);
    let loaded_working_set_delta = final_memory
        .os
        .working_set_bytes
        .saturating_sub(initial_memory.os.working_set_bytes);

    if !args.allow_silent && first.peak == 0.0 && midpoint.peak == 0.0 && looped.peak == 0.0 {
        return Err("all decoded stream renders were silent".into());
    }
    if midpoint.scratch_growth_bytes > args.max_scratch_growth_bytes
        || looped.scratch_growth_bytes > args.max_scratch_growth_bytes
    {
        return Err("decoded stream render scratch grew after warmup".into());
    }
    if load_cache.full_sample_capacity != 0
        || midpoint_cache.full_sample_capacity != 0
        || load_cache.cache_capacity_frames as u64 >= metadata.frame_length
    {
        return Err("decoded stream cache is not bounded relative to source length".into());
    }

    let mut lines = Vec::new();
    lines.push("decoded_stream_playback_probe: ok".to_owned());
    lines.push(format!("input={}", args.input.display()));
    lines.push(format!(
        "load_message={}",
        load_message.replace('\n', " | ")
    ));
    lines.push(format!("load_ms={load_ms:.3}"));
    lines.push(format!(
        "metadata format={} sample_rate={} channels={} frame_length={} duration_seconds={:.6}",
        metadata.format_id,
        metadata.sample_rate,
        metadata.channels,
        metadata.frame_length,
        metadata.duration_seconds,
    ));
    lines.push(format!(
        "cache.load streaming={} start={} frames={} capacity_frames={} source_channels={} sample_capacity={} full_sample_capacity={} request_generation={} filled_generation={}",
        load_cache.streaming,
        load_cache.cache_start_frame,
        load_cache.cache_frames,
        load_cache.cache_capacity_frames,
        load_cache.source_channels,
        load_cache.cached_sample_capacity,
        load_cache.full_sample_capacity,
        load_cache.request_generation,
        load_cache.filled_generation,
    ));
    lines.push(format!(
        "cache.midpoint start={} frames={} capacity_frames={} last_error={}",
        midpoint_cache.cache_start_frame,
        midpoint_cache.cache_frames,
        midpoint_cache.cache_capacity_frames,
        midpoint_cache.last_error.as_deref().unwrap_or("none"),
    ));
    lines.push(format_render("render.first", &first));
    lines.push(format_render("render.midpoint", &midpoint));
    lines.push(format_render("render.loop", &looped));
    lines.push(format!(
        "visualizer sequence={} aggregate_peak={:.6} aggregate_rms={:.6}",
        visualizer.sequence, visualizer.aggregate_peak, visualizer.aggregate_rms,
    ));
    lines.push(format!(
        "memory allocator={} loaded_working_set_delta={} initial_working_set={} final_working_set={}",
        final_memory.allocator,
        loaded_working_set_delta,
        initial_memory.os.working_set_bytes,
        final_memory.os.working_set_bytes,
    ));
    lines.push("full_decode=false".to_owned());
    lines.push("status=ok".to_owned());

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.output, lines.join("\n"))?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

fn require_streaming_cache(
    cache: &flutz_app::playback::DecodedStreamCacheDebug,
    frame_length: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    if !cache.streaming {
        return Err("decoded playback did not use streaming mode".into());
    }
    if cache.full_sample_capacity != 0 || cache.full_sample_len != 0 {
        return Err("decoded streaming playback retained a full decoded sample buffer".into());
    }
    if cache.cache_frames == 0 {
        return Err("decoded stream cache is empty".into());
    }
    if cache.cache_capacity_frames as u64 >= frame_length {
        return Err("decoded stream cache capacity covers the full source".into());
    }
    if let Some(error) = &cache.last_error {
        return Err(format!("decoded stream worker reported error: {error}").into());
    }
    Ok(())
}

fn format_render(label: &str, report: &flutz_app::playback::DecodedRenderProbeReport) -> String {
    format!(
        "{label} frames={} samples={} peak={:.6} rms={:.6} peq_generation={} scratch_growth_bytes={}",
        report.frames,
        report.samples,
        report.peak,
        report.rms,
        report.peq_generation,
        report.scratch_growth_bytes,
    )
}

struct ProbeArgs {
    input: PathBuf,
    output: PathBuf,
    frames: usize,
    max_scratch_growth_bytes: usize,
    debug_memory: bool,
    allow_silent: bool,
}

impl ProbeArgs {
    fn parse() -> Result<Self, Box<dyn std::error::Error>> {
        let mut input = PathBuf::from(DEFAULT_INPUT);
        let mut output = PathBuf::from(DEFAULT_OUTPUT);
        let mut frames = 4096usize;
        let mut max_scratch_growth_bytes = 0usize;
        let mut debug_memory = false;
        let mut allow_silent = false;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--input" => input = PathBuf::from(next_arg(&mut args, "--input")?),
                "--output" => output = PathBuf::from(next_arg(&mut args, "--output")?),
                "--frames" => {
                    frames = next_arg(&mut args, "--frames")?
                        .parse()
                        .map_err(|_| "--frames requires a positive integer")?;
                }
                "--max-scratch-growth-bytes" => {
                    max_scratch_growth_bytes = next_arg(&mut args, "--max-scratch-growth-bytes")?
                        .parse()
                        .map_err(|_| {
                            "--max-scratch-growth-bytes requires a non-negative integer"
                        })?;
                }
                "--debug-memory" => debug_memory = true,
                "--allow-silent" => allow_silent = true,
                "--help" | "-h" => {
                    println!(
                        "usage: decoded_stream_playback_probe [--input PATH] [--output PATH] [--frames N] [--max-scratch-growth-bytes N] [--debug-memory] [--allow-silent]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}").into()),
            }
        }
        if frames == 0 {
            return Err("--frames must be positive".into());
        }
        Ok(Self {
            input,
            output,
            frames,
            max_scratch_growth_bytes,
            debug_memory,
            allow_silent,
        })
    }
}

fn next_arg(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value").into())
}
