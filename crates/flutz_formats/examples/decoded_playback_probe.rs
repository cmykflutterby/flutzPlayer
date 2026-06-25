use std::{collections::BTreeMap, env, path::PathBuf};

use flutz_formats::decode_path_samples_with_symphonia;
use flutz_peq::{Bandwidth, ChannelLayout, PeqBandConfig, PeqConfig, PeqFilterType, PeqProcessor};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("encoded-audio/mp3-44100-stereo.mp3"));
    let decoded = decode_path_samples_with_symphonia(&path, "decoded-audio", Some(48_000))?;
    let decode_summary = &decoded.summary;
    let config = PeqConfig {
        sample_rate_hz: decode_summary.sample_rate.max(1),
        channel_count: decode_summary.channels.max(1) as u16,
        channel_layout: ChannelLayout::Interleaved,
        output_gain_db: -1.0,
        wet_mix: 1.0,
        bands: vec![PeqBandConfig {
            enabled: true,
            filter_type: PeqFilterType::Bell,
            frequency_hz: 1_000.0,
            gain_db: 2.0,
            bandwidth: Bandwidth::Q { value: 1.0 },
            attack_ms: 5.0,
            release_ms: 60.0,
            extra_fields: BTreeMap::new(),
        }],
        extra_fields: BTreeMap::new(),
    };
    let mut processor = PeqProcessor::from_config(config)?;
    let frames = 512usize;
    let channels = decode_summary.channels.max(1);
    let sample_count = (frames * channels).min(decoded.samples.len());
    let input = decoded.samples[..sample_count].to_vec();
    let mut output = vec![0.0f32; input.len()];
    let warmup = processor.process_interleaved(&input, &mut output)?;
    let retained_after_warmup = processor.metrics().retained_state_bytes;
    let process_summary = processor.process_interleaved(&input, &mut output)?;
    let retained_after_second_chunk = processor.metrics().retained_state_bytes;
    let peak = output
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    println!(
        "{}",
        serde_json::to_string(&json!({
            "scenario": "decoded-playback-peq",
            "path": path.display().to_string(),
            "decode_frames": decode_summary.frames_decoded,
            "chunk_frames": process_summary.frames,
            "channels": process_summary.channels,
            "finite_sample_count": process_summary.finite_sample_count,
            "warmup_frames": warmup.frames,
            "output_peak": peak,
            "retained_after_warmup": retained_after_warmup,
            "retained_after_second_chunk": retained_after_second_chunk,
            "scratch_growth_bytes": retained_after_second_chunk.saturating_sub(retained_after_warmup),
            "status": "ok",
        }))?
    );
    Ok(())
}
