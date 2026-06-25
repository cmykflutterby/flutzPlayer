use std::{env, path::PathBuf};

use flutz_formats::decode_path_with_symphonia;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("encoded-audio/aac-m4a-44100-stereo.m4a"));
    let summary = decode_path_with_symphonia(&path, flutz_format_aac::AAC_FORMAT.id, Some(96_000))?;
    println!(
        "{}",
        serde_json::to_string(
            &json!({"format": summary.format, "path": path.display().to_string(), "sample_rate": summary.sample_rate, "channels": summary.channels, "frames_decoded": summary.frames_decoded, "peak": summary.peak, "rms": summary.rms, "status": "ok"})
        )?
    );
    Ok(())
}
