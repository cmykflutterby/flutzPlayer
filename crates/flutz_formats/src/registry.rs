use crate::model::{BackendKind, ContentKind, MasteringCapability};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct FormatDescriptor {
    pub id: &'static str,
    pub friendly_name: &'static str,
    pub extensions: &'static [&'static str],
    pub wrapped_extensions: &'static [&'static str],
    pub content_kind: ContentKind,
    pub backend: BackendKind,
    pub mastering: MasteringCapability,
    pub supports_metadata: bool,
    pub supports_looping: bool,
}

impl FormatDescriptor {
    pub fn handles_extension(&self, extension: &str) -> bool {
        let extension = extension.trim_start_matches('.');
        self.extensions
            .iter()
            .chain(self.wrapped_extensions.iter())
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    }
}

#[derive(Debug, Clone, Default)]
pub struct FormatRegistry {
    descriptors: Vec<FormatDescriptor>,
}

impl FormatRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtin_formats() -> Self {
        let mut registry = Self::new();
        registry.register_many(BUILTIN_FORMATS);
        registry
    }

    pub fn register(&mut self, descriptor: FormatDescriptor) {
        if !self
            .descriptors
            .iter()
            .any(|existing| existing.id == descriptor.id)
        {
            self.descriptors.push(descriptor);
        }
    }

    pub fn register_many(&mut self, descriptors: &[FormatDescriptor]) {
        for descriptor in descriptors {
            self.register(*descriptor);
        }
    }

    pub fn descriptors(&self) -> &[FormatDescriptor] {
        &self.descriptors
    }

    pub fn find_by_extension(&self, extension: &str) -> Option<&FormatDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.handles_extension(extension))
    }
}

pub fn builtin_registry() -> FormatRegistry {
    FormatRegistry::with_builtin_formats()
}

pub const BUILTIN_FORMATS: &[FormatDescriptor] = &[
    FormatDescriptor {
        id: "midi",
        friendly_name: "MIDI sequence",
        extensions: &["mid", "midi"],
        wrapped_extensions: &[],
        content_kind: ContentKind::Midi,
        backend: BackendKind::Midi,
        mastering: MasteringCapability::MidiMastering,
        supports_metadata: true,
        supports_looping: true,
    },
    FormatDescriptor {
        id: "fmid",
        friendly_name: "Flutz MIDI project",
        extensions: &[],
        wrapped_extensions: &["fmid"],
        content_kind: ContentKind::Fmid,
        backend: BackendKind::Fmid,
        mastering: MasteringCapability::MidiMastering,
        supports_metadata: true,
        supports_looping: true,
    },
    FormatDescriptor {
        id: "mp3",
        friendly_name: "MP3 audio",
        extensions: &["mp3"],
        wrapped_extensions: &["fmp3"],
        content_kind: ContentKind::DecodedAudio,
        backend: BackendKind::Symphonia,
        mastering: MasteringCapability::DecodedAudioPeq,
        supports_metadata: true,
        supports_looping: true,
    },
    FormatDescriptor {
        id: "flac",
        friendly_name: "FLAC audio",
        extensions: &["flac"],
        wrapped_extensions: &["fflac"],
        content_kind: ContentKind::DecodedAudio,
        backend: BackendKind::Symphonia,
        mastering: MasteringCapability::DecodedAudioPeq,
        supports_metadata: true,
        supports_looping: true,
    },
    FormatDescriptor {
        id: "ogg-vorbis",
        friendly_name: "Ogg Vorbis audio",
        extensions: &["ogg"],
        wrapped_extensions: &["fogg"],
        content_kind: ContentKind::DecodedAudio,
        backend: BackendKind::Symphonia,
        mastering: MasteringCapability::DecodedAudioPeq,
        supports_metadata: true,
        supports_looping: true,
    },
    FormatDescriptor {
        id: "opus",
        friendly_name: "Opus audio",
        extensions: &["opus"],
        wrapped_extensions: &["fopus"],
        content_kind: ContentKind::DecodedAudio,
        backend: BackendKind::SymphoniaLibOpus,
        mastering: MasteringCapability::DecodedAudioPeq,
        supports_metadata: true,
        supports_looping: true,
    },
    FormatDescriptor {
        id: "wav-pcm",
        friendly_name: "WAV PCM audio",
        extensions: &["wav"],
        wrapped_extensions: &["fwav"],
        content_kind: ContentKind::DecodedAudio,
        backend: BackendKind::Symphonia,
        mastering: MasteringCapability::DecodedAudioPeq,
        supports_metadata: true,
        supports_looping: true,
    },
    FormatDescriptor {
        id: "aiff-pcm",
        friendly_name: "AIFF PCM audio",
        extensions: &["aiff", "aif"],
        wrapped_extensions: &["faiff"],
        content_kind: ContentKind::DecodedAudio,
        backend: BackendKind::Symphonia,
        mastering: MasteringCapability::DecodedAudioPeq,
        supports_metadata: true,
        supports_looping: true,
    },
    FormatDescriptor {
        id: "aac",
        friendly_name: "AAC audio",
        extensions: &["aac", "m4a", "mp4"],
        wrapped_extensions: &["faac", "fm4a"],
        content_kind: ContentKind::DecodedAudio,
        backend: BackendKind::Symphonia,
        mastering: MasteringCapability::DecodedAudioPeq,
        supports_metadata: true,
        supports_looping: true,
    },
];
