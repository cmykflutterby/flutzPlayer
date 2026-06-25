use flutz_formats::{BackendKind, ContentKind, FormatDescriptor, MasteringCapability};

pub const OGG_VORBIS_FORMAT: FormatDescriptor = FormatDescriptor {
    id: "ogg-vorbis",
    friendly_name: "Ogg Vorbis audio",
    extensions: &["ogg"],
    wrapped_extensions: &["fogg"],
    content_kind: ContentKind::DecodedAudio,
    backend: BackendKind::Symphonia,
    mastering: MasteringCapability::DecodedAudioPeq,
    supports_metadata: true,
    supports_looping: true,
};

pub const OPUS_FORMAT: FormatDescriptor = FormatDescriptor {
    id: "opus",
    friendly_name: "Opus audio",
    extensions: &["opus"],
    wrapped_extensions: &["fopus"],
    content_kind: ContentKind::DecodedAudio,
    backend: BackendKind::SymphoniaLibOpus,
    mastering: MasteringCapability::DecodedAudioPeq,
    supports_metadata: true,
    supports_looping: true,
};
