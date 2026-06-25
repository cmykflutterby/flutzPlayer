use flutz_formats::{BackendKind, ContentKind, FormatDescriptor, MasteringCapability};

pub const FORMAT: FormatDescriptor = FormatDescriptor {
    id: "flac",
    friendly_name: "FLAC audio",
    extensions: &["flac"],
    wrapped_extensions: &["fflac"],
    content_kind: ContentKind::DecodedAudio,
    backend: BackendKind::Symphonia,
    mastering: MasteringCapability::DecodedAudioPeq,
    supports_metadata: true,
    supports_looping: true,
};
