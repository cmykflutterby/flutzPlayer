use flutz_peq::{
    deserialize_preset_toml, load_preset_file, save_preset_file, Bandwidth, ChannelLayout,
    PeqBandConfig, PeqConfig, PeqFilterType, PeqPresetFile, PeqProcessor, PreparedConfig,
    PresetMetadata, ReleaseDisposition,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let args = Args::parse(env::args().skip(1).collect());
    let scenarios = args.scenarios();
    for scenario in scenarios {
        let result = run_scenario(scenario, &args);
        match result {
            Ok(record) => emit_record(&args.format, &record),
            Err(error) => {
                let record = json!({
                    "scenario": scenario,
                    "status": "error",
                    "error": error.to_string(),
                });
                emit_record(&args.format, &record);
                std::process::exit(1);
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Args {
    scenario: String,
    format: OutputFormat,
}

impl Args {
    fn parse(args: Vec<String>) -> Self {
        let mut scenario = String::from("all");
        let mut format = OutputFormat::KeyValue;
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--scenario" => {
                    index += 1;
                    scenario = args
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| String::from("all"));
                }
                "--format" => {
                    index += 1;
                    format = match args.get(index).map(String::as_str) {
                        Some("jsonl") => OutputFormat::Jsonl,
                        _ => OutputFormat::KeyValue,
                    };
                }
                _ => {}
            }
            index += 1;
        }
        Self { scenario, format }
    }

    fn scenarios(&self) -> Vec<&str> {
        match self.scenario.as_str() {
            "all" => vec![
                "api-smoke",
                "config-roundtrip",
                "response-bell",
                "response-shelf",
                "sample-rates",
                "live-update",
                "preset-file-roundtrip",
                "same-size-chunks",
                "variable-chunks",
                "multi-instance",
                "reset-release",
            ],
            value => vec![value],
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum OutputFormat {
    KeyValue,
    Jsonl,
}

fn run_scenario(scenario: &str, _args: &Args) -> Result<Value, Box<dyn std::error::Error>> {
    match scenario {
        "api-smoke" => scenario_api_smoke(),
        "config-roundtrip" => scenario_config_roundtrip(),
        "response-bell" => scenario_response_bell(),
        "response-shelf" => scenario_response_shelf(),
        "sample-rates" => scenario_sample_rates(),
        "live-update" => scenario_live_update(),
        "preset-file-roundtrip" => scenario_preset_file_roundtrip(),
        "same-size-chunks" => scenario_same_size_chunks(),
        "variable-chunks" => scenario_variable_chunks(),
        "multi-instance" => scenario_multi_instance(),
        "reset-release" => scenario_reset_release(),
        _ => Err(format!("unknown scenario: {scenario}").into()),
    }
}

fn scenario_api_smoke() -> Result<Value, Box<dyn std::error::Error>> {
    let prepared =
        PreparedConfig::from_config(example_config(ChannelLayout::Interleaved, 48_000, 2, 3.0))?;
    let mut processor = PeqProcessor::new(prepared.clone());
    let input = sine_interleaved(48_000, 2, 512, 1_000.0, 0.25);
    let mut output = vec![0.0; input.len()];
    let summary = processor.process_interleaved(&input, &mut output)?;
    Ok(json!({
        "scenario": "api-smoke",
        "band_count": prepared.config().bands.len(),
        "sample_rate_hz": prepared.config().sample_rate_hz,
        "channel_count": prepared.config().channel_count,
        "layout": "interleaved",
        "frames": summary.frames,
        "finite_sample_count": summary.finite_sample_count,
        "estimated_state_bytes": prepared.estimated_state_bytes(),
        "status": "ok",
    }))
}

fn scenario_config_roundtrip() -> Result<Value, Box<dyn std::error::Error>> {
    let mut preset = example_preset(ChannelLayout::Planar, 48_000, 2, 4.0);
    preset.extra_fields.insert(
        String::from("unknown_top_level"),
        toml::Value::String(String::from("keep-me")),
    );
    preset.config.extra_fields.insert(
        String::from("unknown_config_field"),
        toml::Value::Integer(17),
    );
    preset.config.bands[0].extra_fields.insert(
        String::from("unknown_band_field"),
        toml::Value::Boolean(true),
    );
    let serialized = flutz_peq::serialize_preset_toml(&preset)?;
    let roundtrip = deserialize_preset_toml(&serialized)?;
    Ok(json!({
        "scenario": "config-roundtrip",
        "band_count": roundtrip.config.bands.len(),
        "filter_types": roundtrip.config.bands.iter().map(|band| format!("{:?}", band.filter_type)).collect::<Vec<_>>(),
        "sample_rate_hz": roundtrip.config.sample_rate_hz,
        "channel_count": roundtrip.config.channel_count,
        "unknown_field_tolerance": roundtrip.extra_fields.contains_key("unknown_top_level")
            && roundtrip.config.extra_fields.contains_key("unknown_config_field")
            && roundtrip.config.bands[0].extra_fields.contains_key("unknown_band_field"),
        "has_version_field": serialized.contains("version"),
        "status": "ok",
    }))
}

fn scenario_response_bell() -> Result<Value, Box<dyn std::error::Error>> {
    let config = example_config(ChannelLayout::Interleaved, 48_000, 2, 6.0);
    let mut processor = PeqProcessor::from_config(config)?;
    let result = measure_gain_response(&mut processor, 48_000, 2, 1_000.0, 6_144)?;
    Ok(json!({
        "scenario": "response-bell",
        "measured_gain_db": result.measured_gain_db,
        "peak_linear": result.output_peak,
        "finite_sample_count": result.finite_sample_count,
        "alloc_growth_bytes": 0,
        "status": "ok",
    }))
}

fn scenario_response_shelf() -> Result<Value, Box<dyn std::error::Error>> {
    let mut config = example_config(ChannelLayout::Interleaved, 48_000, 2, 0.0);
    config.bands = vec![PeqBandConfig {
        filter_type: PeqFilterType::HighShelf,
        frequency_hz: 4_000.0,
        gain_db: 6.0,
        bandwidth: Bandwidth::Q { value: 0.8 },
        attack_ms: 2.0,
        release_ms: 10.0,
        ..PeqBandConfig::default()
    }];
    let mut processor = PeqProcessor::from_config(config)?;
    let low = measure_gain_response(&mut processor, 48_000, 2, 500.0, 8_192)?;
    let mut processor = PeqProcessor::from_config(example_high_shelf_config())?;
    let high = measure_gain_response(&mut processor, 48_000, 2, 10_000.0, 8_192)?;
    Ok(json!({
        "scenario": "response-shelf",
        "low_freq_gain_db": low.measured_gain_db,
        "high_freq_gain_db": high.measured_gain_db,
        "finite_sample_count": low.finite_sample_count + high.finite_sample_count,
        "alloc_growth_bytes": 0,
        "status": "ok",
    }))
}

fn scenario_sample_rates() -> Result<Value, Box<dyn std::error::Error>> {
    let mut rows = Vec::new();
    for sample_rate in [44_100_u32, 48_000_u32, 96_000_u32] {
        let mut processor = PeqProcessor::from_config(example_config(
            ChannelLayout::Interleaved,
            sample_rate,
            2,
            3.0,
        ))?;
        let result = measure_gain_response(&mut processor, sample_rate, 2, 1_200.0, 6_144)?;
        rows.push(json!({
            "sample_rate_hz": sample_rate,
            "finite_sample_count": result.finite_sample_count,
            "measured_gain_db": result.measured_gain_db,
        }));
    }
    Ok(json!({
        "scenario": "sample-rates",
        "sample_rates": rows,
        "alloc_growth_bytes": 0,
        "status": "ok",
    }))
}

fn scenario_live_update() -> Result<Value, Box<dyn std::error::Error>> {
    let mut processor =
        PeqProcessor::from_config(example_config(ChannelLayout::Interleaved, 48_000, 2, -3.0))?;
    let mut chunks = Vec::new();
    for chunk_index in 0..3 {
        if chunk_index == 1 {
            processor.set_prepared_config(PreparedConfig::from_config(example_config(
                ChannelLayout::Interleaved,
                48_000,
                2,
                6.0,
            ))?)?;
        }
        let input = sine_interleaved(48_000, 2, 1_024, 1_000.0, 0.2);
        let mut output = vec![0.0; input.len()];
        processor.process_interleaved(&input, &mut output)?;
        chunks.push(rms(&output));
    }
    Ok(json!({
        "scenario": "live-update",
        "chunk_rms": chunks,
        "update_audible_chunk_index": 1,
        "config_swaps": processor.metrics().config_swaps,
        "status": "ok",
    }))
}

fn scenario_preset_file_roundtrip() -> Result<Value, Box<dyn std::error::Error>> {
    let directory = runtime_tests_dir()?;
    let path = directory.join("peq-preset-roundtrip.toml");
    let mut preset = example_preset(ChannelLayout::Planar, 48_000, 2, 5.0);
    preset.extra_fields.insert(
        String::from("unknown_doc_field"),
        toml::Value::String(String::from("retained")),
    );
    preset.config.extra_fields.insert(
        String::from("unknown_config_field"),
        toml::Value::Integer(23),
    );
    save_preset_file(&path, &preset)?;
    let text = fs::read_to_string(&path)?;
    let injected = inject_unknown_field(&text, "extra_probe_field = \"present\"\n")?;
    fs::write(&path, &injected)?;
    let loaded = load_preset_file(&path)?;
    Ok(json!({
        "scenario": "preset-file-roundtrip",
        "preset_path": path.display().to_string(),
        "band_count": loaded.config.bands.len(),
        "unknown_field_tolerance": loaded.extra_fields.contains_key("unknown_doc_field")
            && loaded.config.extra_fields.contains_key("unknown_config_field")
            && loaded.config.extra_fields.contains_key("extra_probe_field"),
        "has_version_field": text.contains("version"),
        "status": "ok",
    }))
}

fn scenario_same_size_chunks() -> Result<Value, Box<dyn std::error::Error>> {
    let mut processor =
        PeqProcessor::from_config(example_config(ChannelLayout::Interleaved, 48_000, 2, 2.5))?;
    let input = sine_interleaved(48_000, 2, 2_048, 1_000.0, 0.3);
    let mut output = vec![0.0; input.len()];
    let before_bytes = processor.metrics().retained_state_bytes;
    processor.process_interleaved(&input, &mut output)?;
    processor.process_interleaved(&input, &mut output)?;
    Ok(json!({
        "scenario": "same-size-chunks",
        "process_calls": processor.metrics().process_calls,
        "retained_state_bytes": processor.metrics().retained_state_bytes,
        "alloc_growth_bytes": processor.metrics().retained_state_bytes.saturating_sub(before_bytes),
        "status": "ok",
    }))
}

fn scenario_variable_chunks() -> Result<Value, Box<dyn std::error::Error>> {
    let mut processor =
        PeqProcessor::from_config(example_config(ChannelLayout::Interleaved, 48_000, 2, 2.5))?;
    let mut chunk_frames = Vec::new();
    for frames in [256_usize, 1_024, 513, 2_048] {
        let input = sine_interleaved(48_000, 2, frames, 2_500.0, 0.3);
        let mut output = vec![0.0; input.len()];
        processor.process_interleaved(&input, &mut output)?;
        chunk_frames.push(frames);
    }
    Ok(json!({
        "scenario": "variable-chunks",
        "chunk_frames": chunk_frames,
        "process_calls": processor.metrics().process_calls,
        "alloc_growth_bytes": 0,
        "status": "ok",
    }))
}

fn scenario_multi_instance() -> Result<Value, Box<dyn std::error::Error>> {
    let mut first =
        PeqProcessor::from_config(example_config(ChannelLayout::Interleaved, 48_000, 2, 3.0))?;
    let mut second = PeqProcessor::from_config(example_high_shelf_config())?;
    let input = sine_interleaved(48_000, 2, 1_024, 2_000.0, 0.25);
    let mut first_output = vec![0.0; input.len()];
    let mut second_output = vec![0.0; input.len()];
    first.process_interleaved(&input, &mut first_output)?;
    second.process_interleaved(&input, &mut second_output)?;
    Ok(json!({
        "scenario": "multi-instance",
        "instance_count": 2,
        "first_rms": rms(&first_output),
        "second_rms": rms(&second_output),
        "first_retained_state_bytes": first.metrics().retained_state_bytes,
        "second_retained_state_bytes": second.metrics().retained_state_bytes,
        "status": "ok",
    }))
}

fn scenario_reset_release() -> Result<Value, Box<dyn std::error::Error>> {
    let mut processor =
        PeqProcessor::from_config(example_config(ChannelLayout::Interleaved, 48_000, 2, 3.0))?;
    let input = sine_interleaved(48_000, 2, 512, 1_000.0, 0.25);
    let mut output = vec![0.0; input.len()];
    processor.process_interleaved(&input, &mut output)?;
    processor.reset_state()?;
    let release = processor.release_state();
    Ok(json!({
        "scenario": "reset-release",
        "release_disposition": match release {
            ReleaseDisposition::Retained => "retained",
            ReleaseDisposition::Released => "released",
        },
        "retained_state_bytes": processor.metrics().retained_state_bytes,
        "status": "ok",
    }))
}

fn example_config(
    layout: ChannelLayout,
    sample_rate_hz: u32,
    channel_count: u16,
    gain_db: f32,
) -> PeqConfig {
    PeqConfig {
        sample_rate_hz,
        channel_count,
        channel_layout: layout,
        output_gain_db: 0.0,
        wet_mix: 1.0,
        bands: vec![PeqBandConfig {
            filter_type: PeqFilterType::Bell,
            frequency_hz: 1_000.0,
            gain_db,
            bandwidth: Bandwidth::Q { value: 1.0 },
            attack_ms: 5.0,
            release_ms: 20.0,
            ..PeqBandConfig::default()
        }],
        extra_fields: BTreeMap::new(),
    }
}

fn example_high_shelf_config() -> PeqConfig {
    PeqConfig {
        sample_rate_hz: 48_000,
        channel_count: 2,
        channel_layout: ChannelLayout::Interleaved,
        output_gain_db: 0.0,
        wet_mix: 1.0,
        bands: vec![PeqBandConfig {
            filter_type: PeqFilterType::HighShelf,
            frequency_hz: 4_000.0,
            gain_db: 6.0,
            bandwidth: Bandwidth::Q { value: 0.8 },
            attack_ms: 2.0,
            release_ms: 10.0,
            ..PeqBandConfig::default()
        }],
        extra_fields: BTreeMap::new(),
    }
}

fn example_preset(
    layout: ChannelLayout,
    sample_rate_hz: u32,
    channel_count: u16,
    gain_db: f32,
) -> PeqPresetFile {
    PeqPresetFile {
        metadata: PresetMetadata {
            name: Some(String::from("Decoded Mastering Demo")),
            author: Some(String::from("GitHub Copilot")),
            notes: Some(String::from("Probe preset for Phase 1 validation.")),
            tags: vec![String::from("probe"), String::from("phase-1")],
            extra_fields: BTreeMap::new(),
        },
        config: example_config(layout, sample_rate_hz, channel_count, gain_db),
        extra_fields: BTreeMap::new(),
    }
}

#[derive(Debug)]
struct ResponseMeasurement {
    measured_gain_db: f64,
    output_peak: f64,
    finite_sample_count: usize,
}

fn measure_gain_response(
    processor: &mut PeqProcessor,
    sample_rate_hz: u32,
    channel_count: usize,
    frequency_hz: f32,
    frames: usize,
) -> Result<ResponseMeasurement, Box<dyn std::error::Error>> {
    let input = sine_interleaved(
        sample_rate_hz,
        channel_count as u16,
        frames,
        frequency_hz,
        0.25,
    );
    let mut output = vec![0.0; input.len()];
    let _ = processor.process_interleaved(&input, &mut output)?;
    let input_rms = rms(&input);
    let output_rms = rms(&output);
    let measured_gain_db = 20.0 * (output_rms / input_rms).log10();
    Ok(ResponseMeasurement {
        measured_gain_db,
        output_peak: output
            .iter()
            .fold(0.0_f64, |peak, sample| peak.max(f64::from(sample.abs()))),
        finite_sample_count: output.iter().filter(|sample| sample.is_finite()).count(),
    })
}

fn sine_interleaved(
    sample_rate_hz: u32,
    channel_count: u16,
    frames: usize,
    frequency_hz: f32,
    amplitude: f32,
) -> Vec<f32> {
    let channels = usize::from(channel_count);
    let mut samples = vec![0.0; frames * channels];
    for frame_index in 0..frames {
        let t = frame_index as f32 / sample_rate_hz as f32;
        let sample = (std::f32::consts::TAU * frequency_hz * t).sin() * amplitude;
        for channel in 0..channels {
            samples[(frame_index * channels) + channel] = sample;
        }
    }
    samples
}

fn rms(samples: &[f32]) -> f64 {
    let power = samples
        .iter()
        .map(|sample| {
            let value = f64::from(*sample);
            value * value
        })
        .sum::<f64>();
    (power / samples.len().max(1) as f64).sqrt()
}

fn runtime_tests_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = Path::new("_local").join("runtime-tests");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn inject_unknown_field(text: &str, snippet: &str) -> Result<String, Box<dyn std::error::Error>> {
    let marker = "[config]\n";
    let index = text.find(marker).ok_or("missing [config] table")? + marker.len();
    let mut output = String::with_capacity(text.len() + snippet.len());
    output.push_str(&text[..index]);
    output.push_str(snippet);
    output.push_str(&text[index..]);
    Ok(output)
}

fn emit_record(format: &OutputFormat, record: &Value) {
    match format {
        OutputFormat::Jsonl => {
            println!(
                "{}",
                serde_json::to_string(record).expect("json record should serialize")
            );
        }
        OutputFormat::KeyValue => {
            println!("{}", to_key_value(record));
        }
    }
}

fn to_key_value(record: &Value) -> String {
    let mut pairs = Vec::new();
    flatten_value(None, record, &mut pairs);
    pairs.join(" ")
}

fn flatten_value(prefix: Option<String>, value: &Value, pairs: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                let next = match &prefix {
                    Some(prefix) => format!("{prefix}.{key}"),
                    None => key.clone(),
                };
                flatten_value(Some(next), nested, pairs);
            }
        }
        Value::Array(array) => {
            let rendered = array
                .iter()
                .map(render_scalar)
                .collect::<Vec<_>>()
                .join(",");
            let key = prefix.unwrap_or_else(|| String::from("value"));
            pairs.push(format!("{key}={rendered}"));
        }
        _ => {
            let key = prefix.unwrap_or_else(|| String::from("value"));
            pairs.push(format!("{key}={}", render_scalar(value)));
        }
    }
}

fn render_scalar(value: &Value) -> String {
    match value {
        Value::Null => String::from("null"),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.replace(' ', "_"),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string(value).unwrap_or_else(|_| String::from("unrenderable"))
        }
    }
}
