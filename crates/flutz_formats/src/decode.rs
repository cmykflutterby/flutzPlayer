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

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioSummary {
    pub format: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub frames_decoded: u64,
    pub peak: f32,
    pub rms: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioBuffer {
    pub summary: DecodedAudioSummary,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioStreamMetadata {
    pub format: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub frame_length: Option<u64>,
    pub duration_seconds: Option<f64>,
    pub source_byte_len: Option<u64>,
    pub seekable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioStreamWindow {
    pub start_frame: u64,
    pub frames_decoded: usize,
    pub samples_decoded: usize,
    pub end_of_stream: bool,
    pub peak: f32,
    pub rms: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedAudioStreamConfig {
    pub media_source_buffer_bytes: usize,
    pub seek_index_fill_rate_seconds: u16,
}

impl Default for DecodedAudioStreamConfig {
    fn default() -> Self {
        Self {
            media_source_buffer_bytes: 128 * 1024,
            seek_index_fill_rate_seconds: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DecodedAudioStreamSource {
    Path(PathBuf),
    Bytes {
        bytes: Arc<[u8]>,
        hint_extension: Option<String>,
    },
}

pub struct DecodedAudioStreamSession {
    format_id: String,
    metadata: DecodedAudioStreamMetadata,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    reusable: Option<(SignalSpec, SampleBuffer<f32>)>,
    pending_samples: Vec<f32>,
    track_id: u32,
    time_base: Option<TimeBase>,
    current_frame: u64,
    finished: bool,
}

impl DecodedAudioStreamSession {
    pub fn open_path(path: &Path, format_id: &str) -> Result<Self> {
        Self::open(
            DecodedAudioStreamSource::Path(path.to_path_buf()),
            format_id,
            DecodedAudioStreamConfig::default(),
        )
    }

    pub fn open_bytes(
        bytes: Arc<[u8]>,
        hint_extension: Option<String>,
        format_id: &str,
    ) -> Result<Self> {
        Self::open(
            DecodedAudioStreamSource::Bytes {
                bytes,
                hint_extension,
            },
            format_id,
            DecodedAudioStreamConfig::default(),
        )
    }

    pub fn open(
        source: DecodedAudioStreamSource,
        format_id: &str,
        config: DecodedAudioStreamConfig,
    ) -> Result<Self> {
        let (media_source, hint, source_byte_len, seekable) = stream_media_source(source)?;
        let media_source = MediaSourceStream::new(
            media_source,
            MediaSourceStreamOptions {
                buffer_len: config
                    .media_source_buffer_bytes
                    .next_power_of_two()
                    .max(64 * 1024),
            },
        );
        let format_options = FormatOptions {
            seek_index_fill_rate: config.seek_index_fill_rate_seconds.max(1),
            ..FormatOptions::default()
        };
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                media_source,
                &format_options,
                &MetadataOptions::default(),
            )
            .map_err(|error| FlutzError::UnsupportedFormat(format!("probe failed: {error}")))?;
        let mut format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| FlutzError::UnsupportedFormat("no default audio track".to_owned()))?;
        if track.codec_params.codec == CODEC_TYPE_NULL {
            return Err(FlutzError::UnsupportedFormat(
                "default track has no codec".to_owned(),
            ));
        }
        let track_id = track.id;
        let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0);
        let mut channels = track
            .codec_params
            .channels
            .map_or(0, |channels| channels.count());
        let frame_length = track.codec_params.n_frames;
        let time_base = track.codec_params.time_base;
        let duration_seconds = match (frame_length, time_base) {
            (Some(frames), Some(time_base)) => Some(time_to_seconds(time_base.calc_time(frames))),
            (Some(frames), None) => Some(frames as f64 / sample_rate as f64),
            _ => None,
        };
        let mut decoder = make_decoder(track.codec_params.clone())?;
        let mut reusable = None;
        let mut pending_samples = Vec::new();
        if sample_rate == 0 || channels == 0 {
            let spec = decode_prime_packet(
                &mut format,
                decoder.as_mut(),
                track_id,
                &mut reusable,
                &mut pending_samples,
            )?;
            sample_rate = spec.rate;
            channels = spec.channels.count();
        }
        if sample_rate == 0 || channels == 0 {
            return Err(FlutzError::UnsupportedFormat(
                "decoded audio stream has no usable sample rate or channel layout".to_owned(),
            ));
        }
        let metadata = DecodedAudioStreamMetadata {
            format: format_id.to_owned(),
            sample_rate,
            channels,
            frame_length,
            duration_seconds,
            source_byte_len,
            seekable,
        };
        Ok(Self {
            format_id: format_id.to_owned(),
            metadata,
            format,
            decoder,
            reusable,
            pending_samples,
            track_id,
            time_base,
            current_frame: 0,
            finished: false,
        })
    }

    pub fn metadata(&self) -> &DecodedAudioStreamMetadata {
        &self.metadata
    }

    pub fn current_frame(&self) -> u64 {
        self.current_frame
    }

    pub fn seek_frame(&mut self, target_frame: u64) -> Result<u64> {
        if !self.metadata.seekable {
            return Err(FlutzError::UnsupportedFormat(
                "decoded audio source is not seekable".to_owned(),
            ));
        }
        let target_frame = self
            .metadata
            .frame_length
            .map_or(target_frame, |length| target_frame.min(length));
        let target_seconds = target_frame as f64 / self.metadata.sample_rate.max(1) as f64;
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: target_seconds.into(),
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|error| FlutzError::Runtime(format!("decoded audio seek failed: {error}")))?;
        self.decoder.reset();
        self.reusable = None;
        self.pending_samples.clear();
        self.current_frame = self.timestamp_to_frame(seeked.actual_ts).min(target_frame);
        self.finished = false;
        Ok(self.current_frame)
    }

    pub fn decode_next_frames(
        &mut self,
        target_frames: usize,
        output: &mut Vec<f32>,
    ) -> Result<DecodedAudioStreamWindow> {
        output.clear();
        if target_frames == 0 || self.finished {
            return Ok(DecodedAudioStreamWindow {
                start_frame: self.current_frame,
                frames_decoded: 0,
                samples_decoded: 0,
                end_of_stream: self.finished,
                peak: 0.0,
                rms: 0.0,
            });
        }

        let start_frame = self.current_frame;
        let target_samples = target_frames.saturating_mul(self.metadata.channels.max(1));
        let mut sum_squares = 0.0f64;
        let mut sample_count = 0u64;
        let mut peak = 0.0f32;
        if !self.pending_samples.is_empty() {
            let copy_len = self.pending_samples.len().min(target_samples);
            for sample in &self.pending_samples[..copy_len] {
                let abs = sample.abs();
                peak = peak.max(abs);
                sum_squares += f64::from(*sample) * f64::from(*sample);
            }
            output.extend_from_slice(&self.pending_samples[..copy_len]);
            self.pending_samples.drain(..copy_len);
            sample_count += copy_len as u64;
            self.current_frame = self
                .current_frame
                .saturating_add((copy_len / self.metadata.channels.max(1)) as u64);
        }
        while output.len() < target_samples {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.finished = true;
                    break;
                }
                Err(error) => {
                    return Err(FlutzError::Runtime(format!(
                        "packet read failed for {}: {error}",
                        self.format_id
                    )))
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(error) => return Err(FlutzError::Runtime(format!("decode failed: {error}"))),
            };
            let spec = *decoded.spec();
            let frame_capacity = decoded.capacity() as u64;
            copy_audio_buffer(decoded, spec, frame_capacity, &mut self.reusable);
            let (_, buffer) = self.reusable.as_ref().expect("sample buffer initialized");
            let remaining = target_samples.saturating_sub(output.len());
            let samples = &buffer.samples()[..buffer.samples().len().min(remaining)];
            for sample in samples {
                let abs = sample.abs();
                peak = peak.max(abs);
                sum_squares += f64::from(*sample) * f64::from(*sample);
            }
            output.extend_from_slice(samples);
            sample_count += samples.len() as u64;
            self.current_frame = self
                .current_frame
                .saturating_add((samples.len() / self.metadata.channels.max(1)) as u64);
        }

        let rms = if sample_count == 0 {
            0.0
        } else {
            (sum_squares / sample_count as f64).sqrt() as f32
        };
        Ok(DecodedAudioStreamWindow {
            start_frame,
            frames_decoded: output.len() / self.metadata.channels.max(1),
            samples_decoded: output.len(),
            end_of_stream: self.finished,
            peak,
            rms,
        })
    }

    fn timestamp_to_frame(&self, timestamp: u64) -> u64 {
        if let Some(time_base) = self.time_base {
            (time_to_seconds(time_base.calc_time(timestamp)) * self.metadata.sample_rate as f64)
                .round()
                .max(0.0) as u64
        } else {
            timestamp
        }
    }
}

pub fn decode_path_with_symphonia(
    path: &Path,
    format_id: &str,
    max_frames: Option<u64>,
) -> Result<DecodedAudioSummary> {
    Ok(decode_path_samples_with_symphonia(path, format_id, max_frames)?.summary)
}

pub fn decode_path_samples_with_symphonia(
    path: &Path,
    format_id: &str,
    max_frames: Option<u64>,
) -> Result<DecodedAudioBuffer> {
    let file = File::open(path).map_err(|error| {
        FlutzError::InvalidInput(format!("failed to open {}: {error}", path.display()))
    })?;
    let media_source = Box::new(file) as Box<dyn MediaSource>;
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }

    decode_media_source_samples(media_source, hint, format_id, max_frames)
}

pub fn decode_bytes_samples_with_symphonia(
    bytes: Vec<u8>,
    hint_extension: Option<&str>,
    format_id: &str,
    max_frames: Option<u64>,
) -> Result<DecodedAudioBuffer> {
    let media_source = Box::new(Cursor::new(bytes)) as Box<dyn MediaSource>;
    let mut hint = Hint::new();
    if let Some(extension) = hint_extension {
        hint.with_extension(extension);
    }

    decode_media_source_samples(media_source, hint, format_id, max_frames)
}

pub fn read_track_metadata_with_symphonia(
    source: DecodedAudioStreamSource,
    format_id: &str,
    project_name_fallback: &str,
    source_filename: &str,
) -> Result<(TrackMetadata, NativeMetadata)> {
    let (media_source, hint, _source_byte_len, _seekable) = stream_media_source(source)?;
    let media_source = MediaSourceStream::new(media_source, Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            media_source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| FlutzError::UnsupportedFormat(format!("probe failed: {error}")))?;
    let mut format = probed.format;
    let mut metadata_log = format.metadata();
    let tags = metadata_log
        .skip_to_latest()
        .map(|revision| revision.tags())
        .unwrap_or(&[]);
    Ok(extract_track_and_native_metadata(
        tags,
        format_id,
        project_name_fallback,
        source_filename,
    ))
}

fn decode_media_source_samples(
    media_source: Box<dyn MediaSource>,
    hint: Hint,
    format_id: &str,
    max_frames: Option<u64>,
) -> Result<DecodedAudioBuffer> {
    let media_source = MediaSourceStream::new(media_source, Default::default());

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            media_source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| FlutzError::UnsupportedFormat(format!("probe failed: {error}")))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| FlutzError::UnsupportedFormat("no default audio track".to_owned()))?;
    if track.codec_params.codec == CODEC_TYPE_NULL {
        return Err(FlutzError::UnsupportedFormat(
            "default track has no codec".to_owned(),
        ));
    }
    let track_id = track.id;
    let mut decoder = make_decoder(track.codec_params.clone())?;

    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut channels = track
        .codec_params
        .channels
        .map_or(0, |channels| channels.count());
    let mut reusable = None::<(SignalSpec, SampleBuffer<f32>)>;
    let mut frames_decoded = 0u64;
    let mut sum_squares = 0.0f64;
    let mut sample_count = 0u64;
    let mut peak = 0.0f32;
    let mut collected_samples = Vec::new();

    loop {
        if max_frames.is_some_and(|limit| frames_decoded >= limit) {
            break;
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(error) => return Err(FlutzError::Runtime(format!("packet read failed: {error}"))),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(error) => return Err(FlutzError::Runtime(format!("decode failed: {error}"))),
        };
        let spec = *decoded.spec();
        let frame_capacity = decoded.capacity() as u64;
        sample_rate = spec.rate;
        channels = spec.channels.count();
        copy_audio_buffer(decoded, spec, frame_capacity, &mut reusable);
        let (_, buffer) = reusable.as_ref().expect("sample buffer initialized");
        let samples = buffer.samples();
        let remaining_samples = max_frames
            .and_then(|limit| {
                let remaining_frames = limit.saturating_sub(frames_decoded);
                remaining_frames.checked_mul(channels.max(1) as u64)
            })
            .unwrap_or(samples.len() as u64) as usize;
        let samples = &samples[..samples.len().min(remaining_samples)];
        for sample in samples {
            let abs = sample.abs();
            peak = peak.max(abs);
            sum_squares += f64::from(*sample) * f64::from(*sample);
        }
        collected_samples.extend_from_slice(samples);
        sample_count += samples.len() as u64;
        if channels != 0 {
            frames_decoded += (samples.len() / channels) as u64;
        }
    }

    let rms = if sample_count == 0 {
        0.0
    } else {
        (sum_squares / sample_count as f64).sqrt() as f32
    };

    Ok(DecodedAudioBuffer {
        summary: DecodedAudioSummary {
            format: format_id.to_owned(),
            sample_rate,
            channels,
            frames_decoded,
            peak,
            rms,
        },
        samples: collected_samples,
    })
}

fn extract_track_and_native_metadata(
    tags: &[Tag],
    format_id: &str,
    project_name_fallback: &str,
    source_filename: &str,
) -> (TrackMetadata, NativeMetadata) {
    let mut metadata = TrackMetadata {
        project_name: project_name_fallback.to_owned(),
        source_filename: source_filename.to_owned(),
        ..TrackMetadata::default()
    };
    let mut native_metadata = Vec::new();

    for tag in tags {
        let value = tag.value.to_string();
        if value.trim().is_empty() {
            continue;
        }
        if apply_standard_tag(&mut metadata, tag.std_key, &value)
            || apply_tag_key_hint(&mut metadata, &tag.key, &value)
        {
            continue;
        }
        native_metadata.push(MetadataField {
            key: if tag.key.trim().is_empty() {
                tag.std_key
                    .map(standard_tag_key_name)
                    .unwrap_or("tag")
                    .to_owned()
            } else {
                tag.key.clone()
            },
            value,
        });
    }

    if metadata.project_name.trim().is_empty() {
        metadata.project_name = if project_name_fallback.trim().is_empty() {
            format!("Loaded {format_id}")
        } else {
            project_name_fallback.to_owned()
        };
    }
    if metadata.source_filename.trim().is_empty() {
        metadata.source_filename = source_filename.to_owned();
    }

    (metadata, native_metadata)
}

fn apply_standard_tag(
    metadata: &mut TrackMetadata,
    standard_key: Option<StandardTagKey>,
    value: &str,
) -> bool {
    let Some(standard_key) = standard_key else {
        return false;
    };

    match standard_key {
        StandardTagKey::TrackTitle => set_if_empty(&mut metadata.project_name, value),
        StandardTagKey::Artist => set_if_empty(&mut metadata.artist, value),
        StandardTagKey::Album => set_if_empty(&mut metadata.album, value),
        StandardTagKey::AlbumArtist => set_if_empty(&mut metadata.album_artist, value),
        StandardTagKey::Composer => set_if_empty(&mut metadata.composer, value),
        StandardTagKey::Conductor => set_if_empty(&mut metadata.conductor, value),
        StandardTagKey::Genre => set_if_empty(&mut metadata.genre, value),
        StandardTagKey::Date
        | StandardTagKey::ReleaseDate
        | StandardTagKey::OriginalDate
        | StandardTagKey::EncodingDate => set_if_empty(&mut metadata.date, value),
        StandardTagKey::TrackNumber => set_if_empty(&mut metadata.track_number, value),
        StandardTagKey::TrackTotal => set_if_empty(&mut metadata.track_total, value),
        StandardTagKey::DiscNumber => set_if_empty(&mut metadata.disc_number, value),
        StandardTagKey::DiscTotal => set_if_empty(&mut metadata.disc_total, value),
        StandardTagKey::Description => set_if_empty(&mut metadata.description, value),
        StandardTagKey::Comment => set_if_empty(&mut metadata.notes, value),
        StandardTagKey::Copyright => set_if_empty(&mut metadata.copyright, value),
        StandardTagKey::Label => set_if_empty(&mut metadata.publisher, value),
        StandardTagKey::EncodedBy => set_if_empty(&mut metadata.encoded_by, value),
        StandardTagKey::Encoder => set_if_empty(&mut metadata.encoder, value),
        StandardTagKey::Language => set_if_empty(&mut metadata.language, value),
        StandardTagKey::Lyrics => set_if_empty(&mut metadata.lyrics, value),
        StandardTagKey::Url
        | StandardTagKey::UrlOfficial
        | StandardTagKey::UrlSource => set_if_empty(&mut metadata.url, value),
        _ => false,
    }
}

fn apply_tag_key_hint(metadata: &mut TrackMetadata, key: &str, value: &str) -> bool {
    let normalized = key
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-', '/'], "_");

    match normalized.as_str() {
        "title" | "tracktitle" | "track_title" => set_if_empty(&mut metadata.project_name, value),
        "artist" => set_if_empty(&mut metadata.artist, value),
        "album" => set_if_empty(&mut metadata.album, value),
        "albumartist" | "album_artist" => set_if_empty(&mut metadata.album_artist, value),
        "composer" => set_if_empty(&mut metadata.composer, value),
        "conductor" => set_if_empty(&mut metadata.conductor, value),
        "genre" => set_if_empty(&mut metadata.genre, value),
        "date" | "year" | "releasedate" | "release_date" => set_if_empty(&mut metadata.date, value),
        "tracknumber" | "track_number" => set_if_empty(&mut metadata.track_number, value),
        "tracktotal" | "track_total" | "totaltracks" => set_if_empty(&mut metadata.track_total, value),
        "discnumber" | "disc_number" => set_if_empty(&mut metadata.disc_number, value),
        "disctotal" | "disc_total" | "totaldiscs" => set_if_empty(&mut metadata.disc_total, value),
        "description" => set_if_empty(&mut metadata.description, value),
        "comment" | "comments" => set_if_empty(&mut metadata.notes, value),
        "copyright" => set_if_empty(&mut metadata.copyright, value),
        "publisher" | "label" => set_if_empty(&mut metadata.publisher, value),
        "encodedby" | "encoded_by" => set_if_empty(&mut metadata.encoded_by, value),
        "encoder" => set_if_empty(&mut metadata.encoder, value),
        "language" => set_if_empty(&mut metadata.language, value),
        "lyrics" => set_if_empty(&mut metadata.lyrics, value),
        "url" | "website" => set_if_empty(&mut metadata.url, value),
        _ => false,
    }
}

fn set_if_empty(slot: &mut String, value: &str) -> bool {
    if slot.trim().is_empty() {
        *slot = value.to_owned();
        true
    } else {
        false
    }
}

fn standard_tag_key_name(standard_key: StandardTagKey) -> &'static str {
    match standard_key {
        StandardTagKey::TrackTitle => "TrackTitle",
        StandardTagKey::Artist => "Artist",
        StandardTagKey::Album => "Album",
        StandardTagKey::AlbumArtist => "AlbumArtist",
        StandardTagKey::Composer => "Composer",
        StandardTagKey::Conductor => "Conductor",
        StandardTagKey::Genre => "Genre",
        StandardTagKey::Date => "Date",
        StandardTagKey::ReleaseDate => "ReleaseDate",
        StandardTagKey::OriginalDate => "OriginalDate",
        StandardTagKey::EncodingDate => "EncodingDate",
        StandardTagKey::TrackNumber => "TrackNumber",
        StandardTagKey::TrackTotal => "TrackTotal",
        StandardTagKey::DiscNumber => "DiscNumber",
        StandardTagKey::DiscTotal => "DiscTotal",
        StandardTagKey::Description => "Description",
        StandardTagKey::Comment => "Comment",
        StandardTagKey::Copyright => "Copyright",
        StandardTagKey::Label => "Label",
        StandardTagKey::EncodedBy => "EncodedBy",
        StandardTagKey::Encoder => "Encoder",
        StandardTagKey::Language => "Language",
        StandardTagKey::Lyrics => "Lyrics",
        StandardTagKey::Url => "Url",
        StandardTagKey::UrlOfficial => "UrlOfficial",
        StandardTagKey::UrlSource => "UrlSource",
        _ => "tag",
    }
}

fn make_decoder(
    codec_params: symphonia::core::codecs::CodecParameters,
) -> Result<Box<dyn Decoder>> {
    match symphonia::default::get_codecs().make(&codec_params, &DecoderOptions::default()) {
        Ok(decoder) => Ok(decoder),
        Err(error) if codec_params.codec == CODEC_TYPE_OPUS => {
            let mut registry = CodecRegistry::new();
            registry.register_all::<OpusDecoder>();
            registry
                .make(&codec_params, &DecoderOptions::default())
                .map_err(|opus_error| {
                    FlutzError::UnsupportedFormat(format!(
                        "opus decoder failed after default decoder error ({error}): {opus_error}"
                    ))
                })
        }
        Err(error) => Err(FlutzError::UnsupportedFormat(format!(
            "decoder failed: {error}"
        ))),
    }
}

fn copy_audio_buffer(
    decoded: AudioBufferRef<'_>,
    spec: SignalSpec,
    frame_capacity: u64,
    reusable: &mut Option<(SignalSpec, SampleBuffer<f32>)>,
) {
    let needs_new = reusable.as_ref().map_or(true, |(buffer_spec, _buffer)| {
        buffer_spec.rate != spec.rate || buffer_spec.channels != spec.channels
    });
    if needs_new {
        *reusable = Some((spec, SampleBuffer::<f32>::new(frame_capacity, spec)));
    }
    reusable
        .as_mut()
        .expect("sample buffer initialized")
        .1
        .copy_interleaved_ref(decoded);
}

fn decode_prime_packet(
    format: &mut Box<dyn FormatReader>,
    decoder: &mut dyn Decoder,
    track_id: u32,
    reusable: &mut Option<(SignalSpec, SampleBuffer<f32>)>,
    pending_samples: &mut Vec<f32>,
) -> Result<SignalSpec> {
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Err(FlutzError::UnsupportedFormat(
                    "decoded audio stream produced no audio packets".to_owned(),
                ));
            }
            Err(error) => {
                return Err(FlutzError::Runtime(format!(
                    "packet read failed while priming stream: {error}"
                )))
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(error) => {
                return Err(FlutzError::Runtime(format!(
                    "decode failed while priming stream: {error}"
                )))
            }
        };
        let spec = *decoded.spec();
        let frame_capacity = decoded.capacity() as u64;
        copy_audio_buffer(decoded, spec, frame_capacity, reusable);
        let (_, buffer) = reusable.as_ref().expect("sample buffer initialized");
        pending_samples.extend_from_slice(buffer.samples());
        return Ok(spec);
    }
}

fn stream_media_source(
    source: DecodedAudioStreamSource,
) -> Result<(Box<dyn MediaSource>, Hint, Option<u64>, bool)> {
    match source {
        DecodedAudioStreamSource::Path(path) => {
            let file = File::open(&path).map_err(|error| {
                FlutzError::InvalidInput(format!("failed to open {}: {error}", path.display()))
            })?;
            let source_byte_len = file.byte_len();
            let seekable = file.is_seekable();
            let mut hint = Hint::new();
            if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
                hint.with_extension(extension);
            }
            Ok((Box::new(file), hint, source_byte_len, seekable))
        }
        DecodedAudioStreamSource::Bytes {
            bytes,
            hint_extension,
        } => {
            let source_byte_len = Some(bytes.len() as u64);
            let mut hint = Hint::new();
            if let Some(extension) = hint_extension.as_deref() {
                hint.with_extension(extension);
            }
            Ok((Box::new(Cursor::new(bytes)), hint, source_byte_len, true))
        }
    }
}

fn time_to_seconds(time: symphonia::core::units::Time) -> f64 {
    time.seconds as f64 + time.frac
}
