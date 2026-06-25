use std::{env, fs, path::PathBuf};

use flutz_app::{app::SoundFontCatalogEntry, playback::PlaybackController};
use flutz_fmid::read_fmid;
use flutz_formats::{
    builtin_registry, decode_path_samples_with_symphonia, ContentKind, MasteringCapability,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        args.push("MIDI Files/rendering-parity-midi/smb-fx-test.fmid".to_owned());
        args.push("encoded-audio/mp3-44100-stereo.mp3".to_owned());
    }

    let registry = builtin_registry();
    let mut loaded_count = 0usize;
    for input in args {
        let path = PathBuf::from(&input);
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default();
        let Some(descriptor) = registry.find_by_extension(extension).copied() else {
            println!(
                "path={} content_kind=unsupported mastering=none status=error",
                path.display()
            );
            continue;
        };

        if descriptor.mastering == MasteringCapability::DecodedAudioPeq {
            let decoded = decode_path_samples_with_symphonia(&path, descriptor.id, Some(4_096))?;
            let mut playback = PlaybackController::new(
                PathBuf::from("drops/flutzplayer/data"),
                Vec::<SoundFontCatalogEntry>::new(),
                Default::default(),
                false,
                false,
            );
            playback.load_decoded_audio_buffer(
                path.clone(),
                descriptor.id,
                descriptor.friendly_name,
                decoded,
                None,
            )?;
            let metadata = playback
                .decoded_transport_metadata()
                .ok_or("decoded metadata was not loaded")?;
            let first_render = playback.decoded_render_probe(512)?;
            let second_render = playback.decoded_render_probe(512)?;
            println!(
                "path={} content_kind={} mastering={} duration_seconds={:.6} unit=sample-frames peq_generation={} first_scratch_growth_bytes={} second_scratch_growth_bytes={} render_peak={:.6} status=ok",
                path.display(),
                metadata.content_kind.as_str(),
                metadata.mastering.as_str(),
                metadata.duration_seconds,
                second_render.peq_generation,
                first_render.scratch_growth_bytes,
                second_render.scratch_growth_bytes,
                second_render.peak,
            );
            loaded_count += 1;
        } else if descriptor.content_kind == ContentKind::Fmid {
            let bytes = fs::read(&path)?;
            let fmid = read_fmid(&bytes)?;
            println!(
                "path={} content_kind={} mastering={} project_name={} midi_bytes={} unit=ticks status=ok",
                path.display(),
                descriptor.content_kind.as_str(),
                descriptor.mastering.as_str(),
                fmid.project.project_name,
                fmid.midi_bytes.len(),
            );
        } else {
            let unit = if descriptor.content_kind == ContentKind::Midi
                || descriptor.content_kind == ContentKind::Fmid
            {
                "ticks"
            } else {
                "unknown"
            };
            println!(
                "path={} content_kind={} mastering={} duration_seconds=0.000000 unit={} status=registered",
                path.display(),
                descriptor.content_kind.as_str(),
                descriptor.mastering.as_str(),
                unit
            );
        }
    }
    println!("loaded_decoded_count={} status=ok", loaded_count);
    Ok(())
}
