mod full;
mod metadata;
mod stream;
mod types;

pub use full::{
    decode_bytes_samples_with_symphonia, decode_path_samples_with_symphonia,
    decode_path_with_symphonia,
};
pub use metadata::read_track_metadata_with_symphonia;
pub use stream::DecodedAudioStreamSession;
pub use types::{
    DecodedAudioBuffer, DecodedAudioStreamConfig, DecodedAudioStreamMetadata,
    DecodedAudioStreamSource, DecodedAudioStreamWindow, DecodedAudioSummary,
};

use std::{
    fs::File,
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};

use flutz_core::{FlutzError, Result};
use symphonia::core::{
    audio::{AudioBufferRef, SampleBuffer, SignalSpec},
    codecs::{CodecRegistry, Decoder, DecoderOptions, CODEC_TYPE_NULL, CODEC_TYPE_OPUS},
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
    meta::{MetadataOptions, StandardTagKey, Tag},
    probe::Hint,
    units::TimeBase,
};
use symphonia_adapter_libopus::OpusDecoder;

use crate::model::{MetadataField, NativeMetadata, TrackMetadata};
