use flutz_formats::{BackendKind, ContentKind, FormatDescriptor, MasteringCapability};

pub const FORMAT: FormatDescriptor = FormatDescriptor {
    id: "mp3",
    friendly_name: "MP3 audio",
    extensions: &["mp3"],
    wrapped_extensions: &["fmp3"],
    content_kind: ContentKind::DecodedAudio,
    backend: BackendKind::Symphonia,
    mastering: MasteringCapability::DecodedAudioPeq,
    supports_metadata: true,
    supports_looping: true,
};
