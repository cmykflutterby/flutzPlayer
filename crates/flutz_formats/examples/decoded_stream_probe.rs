use std::{env, path::PathBuf, time::Instant};

use flutz_formats::{builtin_registry, DecodedAudioStreamSession};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("encoded-audio/memory-test.m4a"));
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    let registry = builtin_registry();
    let descriptor = registry
        .find_by_extension(extension)
        .ok_or("stream probe input extension is not registered")?;

    let opened_at = Instant::now();
    let mut session = DecodedAudioStreamSession::open_path(&path, descriptor.id)?;
    let open_ms = opened_at.elapsed().as_secs_f64() * 1000.0;
    let metadata = session.metadata().clone();
    println!(
        "scenario=open path={} format={} sample_rate={} channels={} frame_length={} duration_seconds={:.6} source_byte_len={} seekable={} full_decode=false open_ms={:.3} status=ok",
        path.display(),
        metadata.format,
        metadata.sample_rate,
        metadata.channels,
        metadata
            .frame_length
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_owned()),
        metadata.duration_seconds.unwrap_or(0.0),
        metadata
            .source_byte_len
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_owned()),
        metadata.seekable,
        open_ms,
    );

    let mut samples = Vec::with_capacity(4096usize.saturating_mul(metadata.channels.max(1)));
    let first = session.decode_next_frames(4096, &mut samples)?;
    println!(
        "scenario=decode-next start_frame={} frames={} samples={} peak={:.6} rms={:.6} capacity_samples={} end_of_stream={} status=ok",
        first.start_frame,
        first.frames_decoded,
        first.samples_decoded,
        first.peak,
        first.rms,
        samples.capacity(),
        first.end_of_stream,
    );

    if let Some(frame_length) = metadata.frame_length {
        let target = frame_length / 2;
        let seeked = session.seek_frame(target)?;
        let window = session.decode_next_frames(4096, &mut samples)?;
        println!(
            "scenario=seek target_frame={} actual_frame={} decode_start_frame={} frames={} capacity_samples={} status=ok",
            target,
            seeked,
            window.start_frame,
            window.frames_decoded,
            samples.capacity(),
        );
    }

    println!("status=ok");
    Ok(())
}
