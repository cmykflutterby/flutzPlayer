use flutz_formats::{BackendKind, ContentKind, FormatDescriptor, MasteringCapability};

pub const AAC_FORMAT: FormatDescriptor = FormatDescriptor {
    id: "aac",
    friendly_name: "AAC audio",
    extensions: &["aac", "m4a", "mp4"],
    wrapped_extensions: &["faac", "fm4a"],
    content_kind: ContentKind::DecodedAudio,
    backend: BackendKind::Symphonia,
    mastering: MasteringCapability::DecodedAudioPeq,
    supports_metadata: true,
    supports_looping: true,
};
