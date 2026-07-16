use flutz_app::app::{runtime_decoded_peq_default_for_load, saved_decoded_peq_default_from_current};
use flutz_peq::{Bandwidth, ChannelLayout, PeqBandConfig, PeqConfig, PeqFilterType, PeqPresetFile, PresetMetadata};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let edited = PeqPresetFile {
        metadata: PresetMetadata {
            name: Some("Edited Current Track".to_owned()),
            notes: Some("BUG-04 default save probe".to_owned()),
            tags: vec!["transient".to_owned()],
            ..PresetMetadata::default()
        },
        config: PeqConfig {
            sample_rate_hz: 44_100,
            channel_count: 2,
            channel_layout: ChannelLayout::Interleaved,
            output_gain_db: 2.75,
            wet_mix: 0.42,
            bands: vec![PeqBandConfig {
                enabled: true,
                filter_type: PeqFilterType::HighShelf,
                frequency_hz: 7_500.0,
                gain_db: -3.5,
                bandwidth: Bandwidth::Q { value: 0.7 },
                attack_ms: 4.0,
                release_ms: 45.0,
                ..PeqBandConfig::default()
            }],
            ..PeqConfig::default()
        },
        extra_fields: Default::default(),
    };

    let saved_default = saved_decoded_peq_default_from_current(&edited);
    if saved_default.metadata.name.as_deref() != Some("Default") {
        return Err("saved default did not receive Default metadata name".into());
    }
    if !saved_default.metadata.tags.is_empty() {
        return Err("saved default retained transient metadata tags".into());
    }
    if (saved_default.config.output_gain_db - edited.config.output_gain_db).abs() > f32::EPSILON {
        return Err("saved default did not retain edited output gain".into());
    }
    if (saved_default.config.wet_mix - edited.config.wet_mix).abs() > f32::EPSILON {
        return Err("saved default did not retain edited wet mix".into());
    }
    if saved_default.config.bands != edited.config.bands {
        return Err("saved default did not retain edited bands".into());
    }

    let later_load = runtime_decoded_peq_default_for_load(saved_default.clone(), 48_000, 2);
    if later_load.config.sample_rate_hz != 48_000 {
        return Err("later decoded load did not adapt default sample rate".into());
    }
    if later_load.config.channel_count != 2 {
        return Err("later decoded load did not adapt default channel count".into());
    }
    if (later_load.config.output_gain_db - edited.config.output_gain_db).abs() > f32::EPSILON {
        return Err("later decoded load used stale output gain instead of saved default".into());
    }
    if later_load.config.bands != edited.config.bands {
        return Err("later decoded load used stale bands instead of saved default".into());
    }

    println!(
        "scenario=bug04-decoded-peq-default-save saved_name={} saved_gain_db={:.2} saved_wet_mix={:.2} later_sample_rate={} later_bands={} status=ok",
        saved_default.metadata.name.unwrap_or_default(),
        saved_default.config.output_gain_db,
        saved_default.config.wet_mix,
        later_load.config.sample_rate_hz,
        later_load.config.bands.len(),
    );
    Ok(())
}
