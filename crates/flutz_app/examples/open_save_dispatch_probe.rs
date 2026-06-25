use std::{env, fs, path::PathBuf};

use flutz_formats::{
    builtin_registry, read_flutz_wrapper, write_flutz_wrapper, ContentKind, FlutzAudioWrapper,
    MasteringCapability, SourceAudioBlock, TrackMetadata,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("encoded-audio/mp3-44100-stereo.mp3"));
    let registry = builtin_registry();
    for descriptor in registry.descriptors() {
        for extension in descriptor.extensions {
            println!(
                "extension={} loader={} saver={} initial_dirty={} status=ok",
                extension,
                loader_name(descriptor.content_kind, descriptor.mastering),
                saver_name(
                    descriptor.content_kind,
                    descriptor.wrapped_extensions.first().copied()
                ),
                initial_dirty(descriptor.content_kind),
            );
        }
        for extension in descriptor.wrapped_extensions {
            println!(
                "extension={} loader={} saver={} initial_dirty=false status=ok",
                extension,
                loader_name(descriptor.content_kind, descriptor.mastering),
                saver_name(descriptor.content_kind, Some(extension)),
            );
        }
    }

    let Some(mp3) = registry.find_by_extension("mp3").copied() else {
        return Err("mp3 descriptor is missing".into());
    };
    let source_bytes = fs::read(&source_path)?;
    let wrapper = FlutzAudioWrapper {
        source: SourceAudioBlock {
            format_id: mp3.id.to_owned(),
            original_filename: source_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("source.mp3")
                .to_owned(),
            media_type: "audio/mpeg".to_owned(),
            bytes: source_bytes,
        },
        metadata: TrackMetadata {
            project_name: "Phase 4 Open Save Probe".to_owned(),
            source_filename: source_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("source.mp3")
                .to_owned(),
            notes: "Native decoded audio save-as wrapper probe.".to_owned(),
        },
        ..FlutzAudioWrapper::default()
    };
    let out_dir = PathBuf::from("_local/runtime-tests");
    fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join("phase4-open-save-probe.fmp3");
    fs::write(&out_path, write_flutz_wrapper(&wrapper)?)?;
    let reopened = read_flutz_wrapper(&fs::read(&out_path)?)?;
    println!(
        "action=native-save-as-wrapper source_ext=mp3 wrapper_ext=fmp3 path={} project_name={} source_bytes={} status=ok",
        out_path.display(),
        reopened.metadata.project_name,
        reopened.source.bytes.len(),
    );
    println!("status=ok");
    Ok(())
}

fn loader_name(content_kind: ContentKind, mastering: MasteringCapability) -> &'static str {
    match (content_kind, mastering) {
        (ContentKind::Midi, _) => "load_midi_path",
        (ContentKind::Fmid, _) => "load_fmid_path",
        (_, MasteringCapability::DecodedAudioPeq) => "load_decoded_audio_path",
        _ => "unsupported",
    }
}

fn saver_name(content_kind: ContentKind, wrapped_extension: Option<&str>) -> String {
    match content_kind {
        ContentKind::Midi | ContentKind::Fmid => "save_fmid_to_path".to_owned(),
        ContentKind::DecodedAudio | ContentKind::DecodedAudioWrapper => wrapped_extension
            .map(|extension| format!("save_decoded_wrapper_to_path:{extension}"))
            .unwrap_or_else(|| "save_decoded_wrapper_to_path".to_owned()),
    }
}

fn initial_dirty(content_kind: ContentKind) -> bool {
    matches!(content_kind, ContentKind::DecodedAudio)
}
