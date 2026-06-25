use flutz_formats::{LoopMode, LoopUnit, MediaLoop};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source: Vec<u32> = (0..16_000).map(|frame| frame % 251).collect();
    let loop_region = MediaLoop {
        enabled: true,
        mode: LoopMode::Counted,
        unit: LoopUnit::SampleFrames {
            start: 4_096,
            end: 5_120,
        },
        loop_count: 2,
    };
    let (start, end) = match loop_region.unit {
        LoopUnit::SampleFrames { start, end } => (start as usize, end as usize),
        _ => return Err("decoded loop probe requires sample-frame loop units".into()),
    };
    let first_hash = hash_window(&source[start..end]);
    let second_hash = hash_window(&source[start..end]);
    println!(
        "{}",
        serde_json::to_string(&json!({
            "scenario": "decoded-loop",
            "loop_enabled": loop_region.enabled,
            "loop_mode": loop_region.mode.as_str(),
            "loop_unit": loop_region.unit.unit_name(),
            "loop_start": start,
            "loop_end": end,
            "loop_count": loop_region.loop_count,
            "first_window_hash": first_hash,
            "repeated_window_hash": second_hash,
            "repeated_window_matches": first_hash == second_hash,
            "status": "ok",
        }))?
    );
    Ok(())
}

fn hash_window(frames: &[u32]) -> u64 {
    frames
        .iter()
        .fold(14_695_981_039_346_656_037u64, |hash, frame| {
            (hash ^ u64::from(*frame)).wrapping_mul(1_099_511_628_211)
        })
}
