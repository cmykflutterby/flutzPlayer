use std::{collections::BTreeMap, env, fs, path::PathBuf};

use flutz_formats::{
    read_flutz_wrapper, write_flutz_wrapper, FlutzAudioWrapper, LoopMode, LoopUnit, MediaLoop,
    MetadataField, SourceAudioBlock, TrackMetadata, UnknownBlock,
};
use flutz_peq::{Bandwidth, ChannelLayout, PeqBandConfig, PeqConfig, PeqFilterType, PeqPresetFile};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("encoded-audio/mp3-44100-stereo.mp3"));
    let source_bytes = fs::read(&source_path)?;
    let wrapper = FlutzAudioWrapper {
        source: SourceAudioBlock {
            format_id: String::from("mp3"),
            original_filename: source_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("source.mp3")
                .to_owned(),
            media_type: String::from("audio/mpeg"),
            bytes: source_bytes,
        },
        metadata: TrackMetadata {
            project_name: String::from("Wrapper Probe"),
            source_filename: String::from("mp3-44100-stereo.mp3"),
            notes: String::from("Decoded audio wrapper roundtrip probe."),
        },
        loop_region: Some(MediaLoop {
            enabled: true,
            mode: LoopMode::Infinite,
            unit: LoopUnit::SampleFrames {
                start: 1_024,
                end: 44_100,
            },
            loop_count: 0,
        }),
        native_metadata: vec![MetadataField {
            key: String::from("title"),
            value: String::from("Wrapper Probe Source"),
        }],
        peq: Some(example_peq()),
        unknown_blocks: vec![UnknownBlock {
            chunk_id: *b"XTRA",
            flags: 7,
            ordinal: 99,
            payload: b"preserve-me".to_vec(),
        }],
        ..FlutzAudioWrapper::default()
    };

    let encoded = write_flutz_wrapper(&wrapper)?;
    let decoded = read_flutz_wrapper(&encoded)?;
    let reencoded = write_flutz_wrapper(&decoded)?;
    let no_version_field = !String::from_utf8_lossy(&encoded)
        .to_ascii_lowercase()
        .contains("version");
    let record = json!({
        "scenario": "wrapper-roundtrip",
        "source_len": decoded.source.bytes.len(),
        "wrapper_len": encoded.len(),
        "reencoded_len": reencoded.len(),
        "project_name": decoded.metadata.project_name,
        "loop_unit": decoded.loop_region.map(|value| value.unit.unit_name()).unwrap_or("none"),
        "peq_band_count": decoded.peq.as_ref().map_or(0, |preset| preset.config.bands.len()),
        "unknown_block_count": decoded.unknown_blocks.len(),
        "unknown_preserved": decoded.unknown_blocks.first().is_some_and(|block| block.payload == b"preserve-me"),
        "no_version_field": no_version_field,
        "status": if no_version_field { "ok" } else { "error" },
    });
    println!("{}", serde_json::to_string(&record)?);
    if !no_version_field {
        return Err("wrapper unexpectedly contains a version field".into());
    }
    Ok(())
}

fn example_peq() -> PeqPresetFile {
    PeqPresetFile {
        metadata: flutz_peq::PresetMetadata::default(),
        config: PeqConfig {
            sample_rate_hz: 44_100,
            channel_count: 2,
            channel_layout: ChannelLayout::Interleaved,
            bands: vec![PeqBandConfig {
                enabled: true,
                filter_type: PeqFilterType::Bell,
                frequency_hz: 1_000.0,
                gain_db: 1.5,
                bandwidth: Bandwidth::Q { value: 0.8 },
                attack_ms: 5.0,
                release_ms: 60.0,
                extra_fields: BTreeMap::new(),
            }],
            ..PeqConfig::default()
        },
        extra_fields: BTreeMap::new(),
    }
}
