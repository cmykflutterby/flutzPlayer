use flutz_formats::{BackendKind, ContentKind, FormatDescriptor, MasteringCapability};

pub const WAV_FORMAT: FormatDescriptor = FormatDescriptor {
    id: "wav-pcm",
    friendly_name: "WAV PCM audio",
    extensions: &["wav"],
    wrapped_extensions: &["fwav"],
    content_kind: ContentKind::DecodedAudio,
    backend: BackendKind::Symphonia,
    mastering: MasteringCapability::DecodedAudioPeq,
    supports_metadata: true,
    supports_looping: true,
};

pub const AIFF_FORMAT: FormatDescriptor = FormatDescriptor {
    id: "aiff-pcm",
    friendly_name: "AIFF PCM audio",
    extensions: &["aiff", "aif"],
    wrapped_extensions: &["faiff"],
    content_kind: ContentKind::DecodedAudio,
    backend: BackendKind::Symphonia,
    mastering: MasteringCapability::DecodedAudioPeq,
    supports_metadata: true,
    supports_looping: true,
};
