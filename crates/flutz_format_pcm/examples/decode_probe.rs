use std::{env, path::PathBuf};

use flutz_formats::decode_path_with_symphonia;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("encoded-audio/wav-16bit-44100-stereo.wav"));
    let format_id = if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("aiff") || ext.eq_ignore_ascii_case("aif"))
    {
        flutz_format_pcm::AIFF_FORMAT.id
    } else {
        flutz_format_pcm::WAV_FORMAT.id
    };
    let summary = decode_path_with_symphonia(&path, format_id, Some(96_000))?;
    println!(
        "{}",
        serde_json::to_string(
            &json!({"format": summary.format, "path": path.display().to_string(), "sample_rate": summary.sample_rate, "channels": summary.channels, "frames_decoded": summary.frames_decoded, "peak": summary.peak, "rms": summary.rms, "status": "ok"})
        )?
    );
    Ok(())
}
