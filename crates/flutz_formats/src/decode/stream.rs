use super::*;

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
pub(super) fn make_decoder(
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

pub(super) fn copy_audio_buffer(
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

pub(super) fn decode_prime_packet(
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

pub(super) fn stream_media_source(
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

pub(super) fn time_to_seconds(time: symphonia::core::units::Time) -> f64 {
    time.seconds as f64 + time.frac
}
