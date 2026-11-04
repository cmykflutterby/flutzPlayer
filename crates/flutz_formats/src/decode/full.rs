use super::stream::{copy_audio_buffer, make_decoder};
use super::*;

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
pub(super) fn decode_media_source_samples(
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
