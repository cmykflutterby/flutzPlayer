pub use process::{NativeHeapSnapshot, PerfProcessSnapshot};
use std::{
    collections::VecDeque,
    env,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    allocation_trace::{
        snapshot as allocation_trace_snapshot, AllocationScope, AllocationScopeGuard,
        AllocationTraceSnapshot,
    },
    memory_runtime::{self, MemoryRuntimeSnapshot},
    playback::PlaybackDebugMetrics,
};
use flutz_synth::{
    PlaybackMemoryDebug, SoundFontRuntimeCacheDebug, SoundFontRuntimeCacheEntryDebug,
    StemRenderAllocationDebug, SynthInstanceMemoryDebug,
};

mod core;
mod latency;
mod process;
mod record;
mod snapshots;
pub use core::{PerfTrace, PERF_TRACE_COMPILED};
pub use latency::LatencyTrace;
use process::ProcessMetricSampler;
pub use record::{PerfMetricSnapshot, PerfTraceRecord};
pub use snapshots::{
    PerfArcReferenceSnapshot, PerfAudioMemorySnapshot, PerfAudioOutputSnapshot,
    PerfComponentMemorySnapshot, PerfLifecycleSnapshot, PerfMemoryInstanceSnapshot,
    PerfMemorySnapshot, PerfMidiMemorySnapshot, PerfReferenceSnapshot, PerfRenderChurnSnapshot,
    PerfRustystemMemorySnapshot, PerfSoundFontCacheEntrySnapshot, PerfSoundFontCacheSnapshot,
    PerfSoundFontMetadataMemorySnapshot,
};

fn append_record(path: &Path, record: &PerfTraceRecord) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", record.to_json_line())
}

fn append_json_line(path: &Path, json: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{json}")
}

fn push_bounded(records: &mut VecDeque<PerfTraceRecord>, record: PerfTraceRecord, max_len: usize) {
    while records.len() >= max_len {
        records.pop_front();
    }
    records.push_back(record);
}

fn default_log_path(session_id: &str) -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_owned))
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(format!("flutz-session-{session_id}.jsonl"))
}

#[cfg(debug_assertions)]
fn default_latency_log_path(session_id: &str) -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_owned))
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(format!("flutz-latency-{session_id}.jsonl"))
}

fn wall_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn allocation_trace_json(snapshot: &AllocationTraceSnapshot) -> String {
    let mut json = format!(
        "{{\"alloc_calls\":{},\"dealloc_calls\":{},\"realloc_calls\":{},\"alloc_bytes\":{},\"dealloc_bytes\":{},\"realloc_in_bytes\":{},\"realloc_out_bytes\":{},\"live_bytes\":{},\"peak_live_bytes\":{},\"outstanding_allocations\":{}",
        snapshot.alloc_calls,
        snapshot.dealloc_calls,
        snapshot.realloc_calls,
        snapshot.alloc_bytes,
        snapshot.dealloc_bytes,
        snapshot.realloc_in_bytes,
        snapshot.realloc_out_bytes,
        snapshot.live_bytes,
        snapshot.peak_live_bytes,
        snapshot.outstanding_allocations
    );
    json.push_str(",\"scopes\":[");
    let mut first_scope = true;
    for scope in snapshot.scopes.iter().filter(|scope| {
        scope.alloc_calls != 0
            || scope.dealloc_calls != 0
            || scope.realloc_calls != 0
            || scope.net_bytes != 0
    }) {
        if !first_scope {
            json.push(',');
        }
        first_scope = false;
        json.push_str(&format!(
            "{{\"name\":\"{}\",\"alloc_calls\":{},\"dealloc_calls\":{},\"realloc_calls\":{},\"alloc_bytes\":{},\"dealloc_bytes\":{},\"realloc_in_bytes\":{},\"realloc_out_bytes\":{},\"net_bytes\":{},\"peak_net_bytes\":{}}}",
            escape_json(scope.label),
            scope.alloc_calls,
            scope.dealloc_calls,
            scope.realloc_calls,
            scope.alloc_bytes,
            scope.dealloc_bytes,
            scope.realloc_in_bytes,
            scope.realloc_out_bytes,
            scope.net_bytes,
            scope.peak_net_bytes
        ));
    }
    json.push_str("],\"stack_buckets\":[");
    let mut first_bucket = true;
    for bucket in &snapshot.stack_buckets {
        if !first_bucket {
            json.push(',');
        }
        first_bucket = false;
        json.push_str(&format!(
            "{{\"hash\":\"{:016x}\",\"scope\":\"{}\",\"sample_calls\":{},\"sample_bytes\":{},\"max_sample_bytes\":{}}}",
            bucket.hash,
            escape_json(bucket.scope),
            bucket.sample_calls,
            bucket.sample_bytes,
            bucket.max_sample_bytes
        ));
    }
    json.push_str("]}");
    json
}

fn push_json_string_opt(json: &mut String, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        json.push_str(&format!(",\"{}\":\"{}\"", key, escape_json(value)));
    }
}

fn push_json_u64_opt(json: &mut String, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        json.push_str(&format!(",\"{}\":{}", key, value));
    }
}

fn push_json_u32_opt(json: &mut String, key: &str, value: Option<u32>) {
    if let Some(value) = value {
        json.push_str(&format!(",\"{}\":{}", key, value));
    }
}

fn push_json_f64_opt(json: &mut String, key: &str, value: Option<f64>) {
    if let Some(value) = value.filter(|value| value.is_finite()) {
        json.push_str(&format!(",\"{}\":{:.3}", key, value));
    }
}

fn push_json_usize_opt(json: &mut String, key: &str, value: Option<usize>) {
    if let Some(value) = value {
        json.push_str(&format!(",\"{}\":{}", key, value));
    }
}

fn memory_gap_json(
    tracked_total_bytes: usize,
    memory_runtime: &MemoryRuntimeSnapshot,
    process: &PerfProcessSnapshot,
) -> String {
    let tracked = tracked_total_bytes as u64;
    let active = memory_runtime.totals.active_bytes as u64;
    let resident = memory_runtime.totals.resident_bytes as u64;
    format!(
        "{{\"tracked_total_bytes\":{},\"jemalloc_active_bytes\":{},\"jemalloc_resident_bytes\":{},\"working_set_minus_tracked_bytes\":{},\"working_set_minus_jemalloc_resident_bytes\":{},\"pagefile_minus_tracked_bytes\":{},\"pagefile_minus_jemalloc_active_bytes\":{}}}",
        tracked_total_bytes,
        memory_runtime.totals.active_bytes,
        memory_runtime.totals.resident_bytes,
        process.working_set_bytes.saturating_sub(tracked),
        process.working_set_bytes.saturating_sub(resident),
        process.pagefile_bytes.saturating_sub(tracked),
        process.pagefile_bytes.saturating_sub(active)
    )
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}
