use std::{
    env,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
};

#[cfg(feature = "jemalloc-memory")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_INPUT: &str = "MIDI Files/rendering-parity-midi/tmnt-water-loop-test.fmid";
const DEFAULT_DATA_DIR: &str = "drops/flutzplayer/data";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = ProbeArgs::parse()?;
    for policy in policies() {
        println!(
            "{{\"event\":\"memory_decay_policy_start\",\"policy\":\"{}\",\"description\":\"{}\"}}",
            escape_json(policy.name),
            escape_json(policy.description),
        );
        let status = run_policy(&args, &policy)?;
        println!(
            "{{\"event\":\"memory_decay_policy_result\",\"policy\":\"{}\",\"success\":{},\"exit_code\":{},\"small_floor_restored\":{},\"time_to_floor_ms\":{},\"small_floor_delta_working_set_bytes\":{},\"small_floor_delta_active_bytes\":{},\"small_floor_delta_resident_bytes\":{},\"final_working_set_bytes\":{},\"final_jemalloc_active_bytes\":{},\"final_jemalloc_resident_bytes\":{},\"final_jemalloc_retained_bytes\":{},\"final_working_set_minus_jemalloc_resident_bytes\":{}}}",
            escape_json(policy.name),
            status.success,
            status.code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "null".to_owned()),
            optional_bool_json(status.summary.as_ref().and_then(|summary| summary.small_floor_restored)),
            optional_u64_json(status.summary.as_ref().and_then(|summary| summary.small_floor_return_time_ms)),
            optional_i64_json(status.summary.as_ref().and_then(|summary| summary.small_floor_delta_working_set_bytes)),
            optional_i64_json(status.summary.as_ref().and_then(|summary| summary.small_floor_delta_active_bytes)),
            optional_i64_json(status.summary.as_ref().and_then(|summary| summary.small_floor_delta_resident_bytes)),
            optional_u64_json(status.summary.as_ref().map(|summary| summary.final_working_set_bytes)),
            optional_u64_json(status.summary.as_ref().map(|summary| summary.final_jemalloc_active_bytes)),
            optional_u64_json(status.summary.as_ref().map(|summary| summary.final_jemalloc_resident_bytes)),
            optional_u64_json(status.summary.as_ref().map(|summary| summary.final_jemalloc_retained_bytes)),
            optional_i64_json(status.summary.as_ref().map(|summary| summary.final_working_set_minus_jemalloc_resident_bytes)),
        );
        if !status.success {
            return Err(format!("policy {} failed", policy.name).into());
        }
    }
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
    sequence: String,
    release_mode: String,
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
        let mut sequence = "small-heavy-small".to_owned();
        let mut release_mode = "stop-maintain".to_owned();
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
                "--sequence" => sequence = next_arg(&mut args, "--sequence")?,
                "--release-mode" => release_mode = next_arg(&mut args, "--release-mode")?,
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

#[derive(Debug, Clone, Copy)]
struct Policy {
    name: &'static str,
    description: &'static str,
    env: &'static [(&'static str, &'static str)],
}

#[derive(Debug, Clone, Copy)]
struct ChildStatus {
    success: bool,
    code: Option<i32>,
    summary: Option<ReloadSummary>,
}

#[derive(Debug, Clone, Copy)]
struct ReloadSummary {
    small_floor_restored: Option<bool>,
    small_floor_delta_working_set_bytes: Option<i64>,
    small_floor_delta_active_bytes: Option<i64>,
    small_floor_delta_resident_bytes: Option<i64>,
    small_floor_return_time_ms: Option<u64>,
    final_working_set_bytes: u64,
    final_jemalloc_active_bytes: u64,
    final_jemalloc_resident_bytes: u64,
    final_jemalloc_retained_bytes: u64,
    final_working_set_minus_jemalloc_resident_bytes: i64,
}

impl ReloadSummary {
    fn parse(line: &str) -> Option<Self> {
        Some(Self {
            small_floor_restored: json_bool(line, "small_floor_restored"),
            small_floor_delta_working_set_bytes: json_i64(
                line,
                "small_floor_delta_working_set_bytes",
            ),
            small_floor_delta_active_bytes: json_i64(line, "small_floor_delta_active_bytes"),
            small_floor_delta_resident_bytes: json_i64(line, "small_floor_delta_resident_bytes"),
            small_floor_return_time_ms: json_u64(line, "small_floor_return_time_ms"),
            final_working_set_bytes: json_u64(line, "final_working_set_bytes")?,
            final_jemalloc_active_bytes: json_u64(line, "final_jemalloc_active_bytes")?,
            final_jemalloc_resident_bytes: json_u64(line, "final_jemalloc_resident_bytes")?,
            final_jemalloc_retained_bytes: json_u64(line, "final_jemalloc_retained_bytes")?,
            final_working_set_minus_jemalloc_resident_bytes: json_i64(
                line,
                "final_working_set_minus_jemalloc_resident_bytes",
            )?,
        })
    }
}

fn policies() -> Vec<Policy> {
    vec![
        Policy {
            name: "current-defaults",
            description: "current runtime defaults",
            env: &[],
        },
        Policy {
            name: "background-thread",
            description: "jemalloc background purge thread enabled",
            env: &[("FLUTZ_JEMALLOC_BACKGROUND_THREAD", "1")],
        },
        Policy {
            name: "short-decay",
            description: "short non-hot dirty/muzzy decay with background thread",
            env: &[
                ("FLUTZ_JEMALLOC_BACKGROUND_THREAD", "1"),
                ("FLUTZ_JEMALLOC_DIRTY_DECAY_MS", "1000"),
                ("FLUTZ_JEMALLOC_MUZZY_DECAY_MS", "1000"),
            ],
        },
        Policy {
            name: "short-hot-decay",
            description: "short decay for both regular and reuse-critical arenas",
            env: &[
                ("FLUTZ_JEMALLOC_BACKGROUND_THREAD", "1"),
                ("FLUTZ_JEMALLOC_DIRTY_DECAY_MS", "1000"),
                ("FLUTZ_JEMALLOC_MUZZY_DECAY_MS", "1000"),
                ("FLUTZ_JEMALLOC_HOT_DIRTY_DECAY_MS", "1000"),
                ("FLUTZ_JEMALLOC_HOT_MUZZY_DECAY_MS", "1000"),
            ],
        },
        Policy {
            name: "retain-false",
            description: "disable jemalloc retain where supported and use short decay",
            env: &[
                ("MALLOC_CONF", "retain:false"),
                ("FLUTZ_JEMALLOC_RETAIN", "0"),
                ("FLUTZ_JEMALLOC_BACKGROUND_THREAD", "1"),
                ("FLUTZ_JEMALLOC_DIRTY_DECAY_MS", "1000"),
                ("FLUTZ_JEMALLOC_MUZZY_DECAY_MS", "1000"),
                ("FLUTZ_JEMALLOC_HOT_DIRTY_DECAY_MS", "1000"),
                ("FLUTZ_JEMALLOC_HOT_MUZZY_DECAY_MS", "1000"),
            ],
        },
    ]
}

fn run_policy(
    args: &ProbeArgs,
    policy: &Policy,
) -> Result<ChildStatus, Box<dyn std::error::Error>> {
    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("-p")
        .arg("flutzplayer")
        .arg("--example")
        .arg("memory_reload_probe")
        .arg("--features")
        .arg("jemalloc-memory")
        .arg("--")
        .arg("--data-dir")
        .arg(&args.data_dir)
        .arg("--heavy-input")
        .arg(&args.heavy_input)
        .arg("--small-input")
        .arg(&args.small_input)
        .arg("--cycles")
        .arg(args.cycles.to_string())
        .arg("--idle-samples")
        .arg(args.idle_samples.to_string())
        .arg("--idle-ms")
        .arg(args.idle_ms.to_string())
        .arg("--render-frames")
        .arg(args.render_frames.to_string())
        .arg("--sequence")
        .arg(&args.sequence)
        .arg("--release-mode")
        .arg(&args.release_mode)
        .arg("--settle-window")
        .arg(args.settle_window.to_string())
        .arg("--settle-delta-bytes")
        .arg(args.settle_delta_bytes.to_string())
        .arg("--floor-tolerance-bytes")
        .arg(args.floor_tolerance_bytes.to_string())
        .arg("--debug-memory");
    if args.require_partial {
        command.arg("--require-partial");
    }
    if args.skip_small {
        command.arg("--skip-small");
    }
    for (name, value) in policy.env {
        command.env(name, value);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut summary = None;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line?;
            if line.contains("\"event\":\"memory_reload_summary\"") {
                summary = ReloadSummary::parse(&line);
            }
            println!(
                "{{\"event\":\"memory_decay_policy_child_stdout\",\"policy\":\"{}\",\"line\":\"{}\"}}",
                escape_json(policy.name),
                escape_json(&line),
            );
        }
    }
    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            println!(
                "{{\"event\":\"memory_decay_policy_child_stderr\",\"policy\":\"{}\",\"line\":\"{}\"}}",
                escape_json(policy.name),
                escape_json(&line?),
            );
        }
    }
    let status = child.wait()?;
    Ok(ChildStatus {
        success: status.success(),
        code: status.code(),
        summary,
    })
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

fn json_u64(line: &str, key: &str) -> Option<u64> {
    json_number_slice(line, key)?.parse().ok()
}

fn json_i64(line: &str, key: &str) -> Option<i64> {
    json_number_slice(line, key)?.parse().ok()
}

fn json_bool(line: &str, key: &str) -> Option<bool> {
    let marker = format!("\"{key}\":");
    let start = line.find(&marker)? + marker.len();
    let rest = &line[start..];
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn json_number_slice<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\":");
    let start = line.find(&marker)? + marker.len();
    let rest = &line[start..];
    let end = rest
        .find(|character: char| character != '-' && !character.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(&rest[..end])
}

fn optional_u64_json(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned())
}

fn optional_bool_json(value: Option<bool>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned())
}

fn optional_i64_json(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_owned())
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
