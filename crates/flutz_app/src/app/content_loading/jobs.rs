use super::*;

use super::*;

pub(in crate::app) struct LoadedContentState {
    pub(in crate::app) kind: ContentKind,
    pub(in crate::app) format_id: String,
    pub(in crate::app) friendly_name: String,
    pub(in crate::app) mastering: MasteringCapability,
    pub(in crate::app) wrapped_extension: Option<String>,
    pub(in crate::app) decoded_wrapper: Option<FlutzAudioWrapper>,
    pub(in crate::app) decoded_source_path: Option<PathBuf>,
    pub(in crate::app) decoded_source_bytes: Option<Arc<[u8]>>,
}

pub(in crate::app) struct PendingDecodedAudioLoad {
    pub(in crate::app) token: u64,
    pub(in crate::app) receiver: mpsc::Receiver<DecodedAudioLoadJobResult>,
    pub(in crate::app) resume_after_load: bool,
    pub(in crate::app) playlist_entry_load: bool,
}

pub(in crate::app) struct DecodedAudioLoadJobResult {
    pub(in crate::app) token: u64,
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) path_display: String,
    pub(in crate::app) descriptor: FormatDescriptor,
    pub(in crate::app) is_wrapped: bool,
    pub(in crate::app) wrapper: Option<FlutzAudioWrapper>,
    pub(in crate::app) stream_source: Option<DecodedAudioStreamSource>,
    pub(in crate::app) source_bytes: Option<Arc<[u8]>>,
    pub(in crate::app) error: Option<String>,
    pub(in crate::app) error_stage: &'static str,
}

pub(in crate::app) fn prepare_decoded_audio_load_job(
    token: u64,
    path: PathBuf,
    descriptor: FormatDescriptor,
) -> DecodedAudioLoadJobResult {
    let path_display = path.display().to_string();
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    let is_wrapped = extension.as_deref().is_some_and(|ext| {
        descriptor
            .wrapped_extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(ext))
    });
    let mut result = DecodedAudioLoadJobResult {
        token,
        path: path.clone(),
        path_display,
        descriptor,
        is_wrapped,
        wrapper: None,
        stream_source: None,
        source_bytes: None,
        error: None,
        error_stage: "prepare",
    };

    if is_wrapped {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                result.error = Some(format!("failed to read {}: {error}", path.display()));
                result.error_stage = "read";
                return result;
            }
        };
        let mut wrapper = match read_flutz_wrapper(&bytes) {
            Ok(wrapper) => wrapper,
            Err(error) => {
                result.error = Some(format!("{error}"));
                result.error_stage = "parse";
                return result;
            }
        };
        let hint_extension = Path::new(&wrapper.source.original_filename)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_owned)
            .or_else(|| descriptor.extensions.first().map(|ext| (*ext).to_owned()));
        let source_bytes = Arc::<[u8]>::from(std::mem::take(&mut wrapper.source.bytes));
        result.stream_source = Some(DecodedAudioStreamSource::Bytes {
            bytes: Arc::clone(&source_bytes),
            hint_extension,
        });
        result.source_bytes = Some(source_bytes);
        result.wrapper = Some(wrapper);
        return result;
    }

    let source_filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("embedded.audio")
        .to_owned();
    let fallback_project_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(&source_filename)
        .to_owned();
    let (os_metadata, os_extra_fields) = read_os_track_metadata(&path);
    let (mut metadata, native_metadata) = read_track_metadata_with_symphonia(
        DecodedAudioStreamSource::Path(path.clone()),
        descriptor.id,
        &fallback_project_name,
        &source_filename,
    )
    .unwrap_or_else(|_| {
        (
            TrackMetadata {
                source_filename: source_filename.clone(),
                ..TrackMetadata::default()
            },
            Vec::new(),
        )
    });
    metadata.extra_fields = merge_metadata_field_lists(os_extra_fields, metadata.extra_fields);
    metadata = merge_imported_track_metadata(os_metadata, metadata);
    if metadata.project_name.trim().is_empty() {
        metadata.project_name = fallback_project_name;
    }
    if metadata.source_filename.trim().is_empty() {
        metadata.source_filename = source_filename.clone();
    }
    result.wrapper = Some(FlutzAudioWrapper {
        source: SourceAudioBlock {
            format_id: descriptor.id.to_owned(),
            original_filename: source_filename.clone(),
            media_type: format!("audio/{}", descriptor.id),
            bytes: Vec::new(),
        },
        metadata,
        native_metadata,
        ..FlutzAudioWrapper::default()
    });
    result.stream_source = Some(DecodedAudioStreamSource::Path(path));
    result
}
