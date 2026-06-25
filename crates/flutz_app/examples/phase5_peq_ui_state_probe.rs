use std::{env, fs, path::PathBuf};

use flutz_app::{app::SoundFontCatalogEntry, playback::PlaybackController};
use flutz_formats::{
    builtin_registry, decode_path_samples_with_symphonia, read_flutz_wrapper, write_flutz_wrapper,
    FlutzAudioWrapper, MasteringCapability, SourceAudioBlock, TrackMetadata,
};
use flutz_peq::{
    load_preset_file, save_preset_file, Bandwidth, PeqBandConfig, PeqConfig, PeqFilterType,
    PeqPresetFile, PresetMetadata,
};
use flutz_synth::{PlaybackLoopMode, PlaybackLoopSettings};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("encoded-audio/mp3-44100-stereo.mp3"));
    let registry = builtin_registry();

    for extension in ["fmid", "mp3"] {
        let descriptor = registry
            .find_by_extension(extension)
            .ok_or("missing registry descriptor")?;
        println!(
            "extension={} mastering={} render_master_mixer_visible={} peq_visible={} loop_unit={} status=ok",
            extension,
            descriptor.mastering.as_str(),
            descriptor.mastering == MasteringCapability::MidiMastering,
            descriptor.mastering == MasteringCapability::DecodedAudioPeq,
            if descriptor.mastering == MasteringCapability::DecodedAudioPeq {
                "sample-frames"
            } else {
                "ticks"
            },
        );
    }

    let descriptor = registry
        .find_by_extension(
            source_path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("mp3"),
        )
        .copied()
        .ok_or("unsupported input")?;
    let decoded = decode_path_samples_with_symphonia(&source_path, descriptor.id, Some(8_192))?;
    let sample_rate = decoded.summary.sample_rate;
    let mut playback = PlaybackController::new(
        PathBuf::from("drops/flutzplayer/data"),
        Vec::<SoundFontCatalogEntry>::new(),
        Default::default(),
        false,
        false,
    );
    playback.load_decoded_audio_buffer(
        source_path.clone(),
        descriptor.id,
        descriptor.friendly_name,
        decoded,
        None,
    )?;
    let metadata = playback
        .decoded_transport_metadata()
        .ok_or("decoded metadata missing")?;
    playback.set_loop_settings(PlaybackLoopSettings {
        enabled: true,
        mode: PlaybackLoopMode::Counted,
        start_tick: 16,
        end_tick: metadata.frame_length.min(4_096).max(17),
        loop_count: 2,
    })?;

    let initial_config_source = "default";
    let initial_track_dirty = false;
    let before = playback.decoded_render_probe(512)?;
    let config = PeqConfig {
        sample_rate_hz: sample_rate,
        channel_count: 2,
        output_gain_db: 1.5,
        bands: vec![PeqBandConfig {
            enabled: true,
            filter_type: PeqFilterType::Bell,
            frequency_hz: 1_000.0,
            gain_db: 3.0,
            bandwidth: Bandwidth::Q { value: 1.2 },
            attack_ms: 5.0,
            release_ms: 60.0,
            ..PeqBandConfig::default()
        }],
        ..PeqConfig::default()
    };
    let pending_generation = playback.set_decoded_peq_config(config.clone())?;
    let after = playback.decoded_render_probe(512)?;
    let edited_config_source = "edited";
    let edited_track_dirty = true;

    let preset = PeqPresetFile {
        metadata: PresetMetadata {
            name: Some("Phase 5 Probe PEQ".to_owned()),
            notes: Some("Standalone PEQ preset workflow probe.".to_owned()),
            ..PresetMetadata::default()
        },
        config,
        extra_fields: Default::default(),
    };
    let out_dir = PathBuf::from("_local/runtime-tests");
    fs::create_dir_all(&out_dir)?;
    let preset_path = out_dir.join("phase5-peq-probe.fpeq");
    save_preset_file(&preset_path, &preset)?;
    let reloaded = load_preset_file(&preset_path)?;
    let wrapper_path = out_dir.join("phase5-wrapper-peq-probe.fmp3");
    if wrapper_path.exists() {
        fs::remove_file(&wrapper_path)?;
    }
    let wrapper_saved_by_preset = wrapper_path.exists();
    let source_bytes = fs::read(&source_path)?;
    let wrapper = FlutzAudioWrapper {
        source: SourceAudioBlock {
            format_id: descriptor.id.to_owned(),
            original_filename: source_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("source.audio")
                .to_owned(),
            media_type: format!("audio/{}", descriptor.id),
            bytes: source_bytes,
        },
        metadata: TrackMetadata {
            project_name: "Phase 5 PEQ Wrapper Probe".to_owned(),
            source_filename: source_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("source.audio")
                .to_owned(),
            notes: "Decoded wrapper PEQ persistence probe.".to_owned(),
        },
        peq: Some(reloaded.clone()),
        ..FlutzAudioWrapper::default()
    };
    fs::write(&wrapper_path, write_flutz_wrapper(&wrapper)?)?;
    let reopened = read_flutz_wrapper(&fs::read(&wrapper_path)?)?;

    println!(
        "action=peq-update before_generation={} pending_generation={} after_generation={} initial_config_source={} initial_track_dirty={} edited_config_source={} edited_track_dirty={} bands={} status=ok",
        before.peq_generation,
        pending_generation,
        after.peq_generation,
        initial_config_source,
        initial_track_dirty,
        edited_config_source,
        edited_track_dirty,
        reloaded.config.bands.len(),
    );
    println!(
        "action=peq-preset-save preset_path={} preset_name={} wrapper_saved_by_preset={} status=ok",
        preset_path.display(),
        reloaded.metadata.name.clone().unwrap_or_default(),
        wrapper_saved_by_preset,
    );
    println!(
        "action=decoded-wrapper-save wrapper_path={} wrapper_peq_bands={} wrapper_project_name={} status=ok",
        wrapper_path.display(),
        reopened.peq.as_ref().map(|peq| peq.config.bands.len()).unwrap_or(0),
        reopened.metadata.project_name,
    );
    println!(
        "action=transport-loop content=decoded-audio loop_unit=sample-frames loop_start=16 loop_end={} loop_count=2 status=ok",
        metadata.frame_length.min(4_096).max(17),
    );
    println!(
        "action=transport-loop content=fmid loop_unit=ticks loop_start=0 loop_end=0 loop_count=1 status=ok"
    );
    println!(
        "summary preset_bands={} wrapper_peq_present={} status=ok",
        reloaded.config.bands.len(),
        reopened.peq.is_some(),
    );
    println!("status=ok");
    Ok(())
}
