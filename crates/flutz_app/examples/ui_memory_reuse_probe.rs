//! External probe for UI-independent memory maintenance and per-frame UI buffer reuse.
//! Run: `cargo run -p flutzplayer --example ui_memory_reuse_probe`

use std::{
    collections::BTreeMap,
    fs, thread,
    time::{Duration, Instant},
};

use flutz_app::memory_runtime::{self, MemoryMaintenanceHandle, MEMORY_MAINTENANCE_INTERVAL};
use flutz_app::playback::{RealtimeMixerSnapshot, RealtimeStripSnapshot};
use flutz_core::StripId;
use flutz_mixer::MeterReading;

type ProbeResult = Result<(), Box<dyn std::error::Error>>;

fn main() -> ProbeResult {
    check_maintenance_thread()?;
    check_snapshot_copy_reuse()?;
    check_source_wiring()?;
    println!(
        "scenario=ui-memory-reuse maintenance_thread=ok snapshot_copy_reuse=ok source_wiring=ok status=ok"
    );
    Ok(())
}

fn decay_counts() -> BTreeMap<&'static str, (bool, u64)> {
    memory_runtime::snapshot()
        .domains
        .into_iter()
        .map(|domain| (domain.name, (domain.reuse_critical, domain.decay_count)))
        .collect()
}

fn check_maintenance_thread() -> ProbeResult {
    require(
        MEMORY_MAINTENANCE_INTERVAL <= Duration::from_secs(10),
        "maintenance interval is too long to bound arena growth",
    )?;

    let before = decay_counts();
    let handle = MemoryMaintenanceHandle::spawn(Duration::from_millis(300))?;
    thread::sleep(Duration::from_millis(1_400));
    drop(handle);
    let after = decay_counts();

    let mut idle_domains_decayed = 0;
    for (name, (reuse_critical, count_after)) in &after {
        let count_before = before.get(name).map_or(0, |(_, count)| *count);
        let ticks = count_after - count_before;
        if *reuse_critical {
            require(
                ticks == 0,
                &format!("maintenance thread decayed reuse-critical domain {name} ({ticks} times)"),
            )?;
        } else {
            require(
                ticks >= 2,
                &format!("domain {name} decayed {ticks} times in 1.4 s at a 300 ms interval"),
            )?;
            idle_domains_decayed += 1;
        }
    }
    require(idle_domains_decayed > 0, "no non-critical domains found")?;

    let settled = decay_counts();
    thread::sleep(Duration::from_millis(700));
    require(
        decay_counts() == settled,
        "maintenance thread kept running after its handle was dropped",
    )?;

    let long_interval = MemoryMaintenanceHandle::spawn(Duration::from_secs(60))?;
    let stop_started = Instant::now();
    drop(long_interval);
    require(
        stop_started.elapsed() < Duration::from_secs(1),
        "dropping the handle must stop the thread without waiting for the interval",
    )?;
    Ok(())
}

fn strip(soundfont_id: &str, name: Option<&str>, notes: &[u8], peak: f32) -> RealtimeStripSnapshot {
    RealtimeStripSnapshot {
        soundfont_id: soundfont_id.to_owned(),
        midi_channel: 1,
        midi_bank: 0,
        midi_program: 5,
        is_percussion: false,
        display_name: name.map(str::to_owned),
        meter: MeterReading {
            peak,
            ..MeterReading::default()
        },
        audible: peak > 0.0,
        active_notes: notes.to_vec(),
    }
}

fn snapshot(strips: Vec<(u64, RealtimeStripSnapshot)>) -> RealtimeMixerSnapshot {
    RealtimeMixerSnapshot {
        output_meter: MeterReading::default(),
        strips: strips
            .into_iter()
            .map(|(id, strip)| (StripId(id), strip))
            .collect(),
    }
}

fn same_contents(left: &RealtimeMixerSnapshot, right: &RealtimeMixerSnapshot) -> bool {
    left.output_meter == right.output_meter
        && left.strips.len() == right.strips.len()
        && left
            .strips
            .iter()
            .zip(&right.strips)
            .all(|((lk, l), (rk, r))| {
                lk == rk
                    && l.soundfont_id == r.soundfont_id
                    && l.midi_channel == r.midi_channel
                    && l.midi_bank == r.midi_bank
                    && l.midi_program == r.midi_program
                    && l.is_percussion == r.is_percussion
                    && l.display_name == r.display_name
                    && l.meter == r.meter
                    && l.audible == r.audible
                    && l.active_notes == r.active_notes
            })
}

fn check_snapshot_copy_reuse() -> ProbeResult {
    let first = snapshot(vec![
        (1, strip("font-a", Some("Piano"), &[60, 64, 67], 0.5)),
        (2, strip("font-a", None, &[], 0.0)),
    ]);
    let second = snapshot(vec![
        (1, strip("font-a", Some("Harp"), &[62], 0.25)),
        (3, strip("font-b", Some("Drums"), &[36], 0.75)),
    ]);

    let mut target = RealtimeMixerSnapshot::default();
    target.copy_from(&first);
    require(
        same_contents(&target, &first),
        "first copy differs from source",
    )?;

    let reused = &target.strips[&StripId(1)];
    let id_ptr = reused.soundfont_id.as_ptr();
    let notes_ptr = reused.active_notes.as_ptr();
    let name_ptr = reused.display_name.as_ref().map(|name| name.as_ptr());

    target.copy_from(&second);
    require(
        same_contents(&target, &second),
        "second copy differs from source",
    )?;
    let reused = &target.strips[&StripId(1)];
    require(
        reused.soundfont_id.as_ptr() == id_ptr,
        "soundfont_id buffer was reallocated for a retained strip",
    )?;
    require(
        reused.active_notes.as_ptr() == notes_ptr,
        "active_notes buffer was reallocated for a retained strip",
    )?;
    require(
        reused.display_name.as_ref().map(|name| name.as_ptr()) == name_ptr,
        "display_name buffer was reallocated for a retained strip",
    )?;
    require(
        !target.strips.contains_key(&StripId(2)),
        "strip missing from the source was not removed",
    )?;

    target.copy_from(&RealtimeMixerSnapshot::default());
    require(
        target.strips.is_empty(),
        "copying an empty snapshot left strips behind",
    )?;
    Ok(())
}

fn check_source_wiring() -> ProbeResult {
    let lifecycle = fs::read_to_string("crates/flutz_app/src/app/lifecycle.rs")?;
    let update = function_body(&lifecycle, "fn update(")?;
    require(
        !update.contains("memory_maintenance") && !update.contains("decay_"),
        "update() must not drive allocator maintenance",
    )?;
    require(
        update.contains("self.fill_mixer_edit_snapshot(&mut mixer_snapshot);")
            && update.contains("self.mixer_edit_buffer = mixer_snapshot;"),
        "update() must refill and keep the reusable mixer edit snapshot",
    )?;
    let new_fn = function_body(&lifecycle, "fn new(")?;
    require(
        new_fn.contains("MemoryMaintenanceHandle::spawn("),
        "new() must start the memory maintenance thread",
    )?;

    let maintenance = fs::read_to_string("crates/flutz_app/src/memory_runtime/maintenance.rs")?;
    require(
        maintenance.contains("decay_all_idle_domains()")
            && !maintenance.contains("decay_idle_reuse_preserving")
            && !maintenance.contains("purge_")
            && !maintenance.contains("release_idle_now")
            && maintenance.contains("service_idle_release_with(")
            && maintenance.contains("IDLE_DEEP_RELEASE_AFTER"),
        "maintenance thread must only decay non-reuse-critical domains; arena purges go through the 15 min idle service",
    )?;

    let diagnostics = fs::read_to_string("crates/flutz_app/src/app/diagnostics.rs")?;
    let feedback = function_body(&diagnostics, "fn refresh_realtime_feedback(")?;
    require(
        !feedback.contains(".to_owned()"),
        "refresh_realtime_feedback allocates strings per strip per frame",
    )?;
    require(
        feedback.contains("copy_audible_snapshot_into(")
            && !feedback.contains("audible_output_view()"),
        "refresh_realtime_feedback must copy into the reusable snapshot buffer",
    )?;

    let feedback_rs = fs::read_to_string("crates/flutz_app/src/app/mixer_state/feedback.rs")?;
    let apply = function_body(&feedback_rs, "fn apply_realtime_snapshot(")?;
    require(
        !apply.contains("display_name.clone()") && !apply.contains("\"--\".to_owned()"),
        "apply_realtime_snapshot reallocates strip text per frame",
    )?;

    let presets = fs::read_to_string("crates/flutz_app/src/app/mixer_state/presets.rs")?;
    let apply_edit = function_body(&presets, "fn apply_mixer_edit_snapshot(")?;
    require(
        !apply_edit.contains("BTreeMap"),
        "apply_mixer_edit_snapshot builds temporary maps per frame",
    )?;

    let transport = fs::read_to_string("crates/flutz_app/src/app/transport.rs")?;
    let sync = function_body(&transport, "fn sync_playback_controls(")?;
    let check_at = sync
        .find("mixer_controls_changed_since_publish(")
        .ok_or("sync_playback_controls does not skip unchanged frames")?;
    let build_at = sync
        .find("BTreeMap::new()")
        .ok_or("sync_playback_controls no longer builds the control map")?;
    require(
        check_at < build_at,
        "sync_playback_controls must check for changes before rebuilding",
    )?;

    for path in [
        "crates/flutz_app/src/memory_runtime/maintenance.rs",
        "crates/flutz_app/src/app/mixer_state/presets.rs",
        "crates/flutz_app/src/app/transport.rs",
        "crates/flutz_app/src/playback/render/core.rs",
    ] {
        let source = fs::read_to_string(path)?;
        require(
            !source.contains("#[test]") && !source.contains("cfg(test)"),
            &format!("inline tests are not allowed: {path}"),
        )?;
    }
    Ok(())
}

fn require(condition: bool, message: &str) -> ProbeResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn function_body<'a>(
    source: &'a str,
    signature: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    let signature_start = source
        .find(signature)
        .ok_or_else(|| format!("missing function signature: {signature}"))?;
    let body_start = source[signature_start..]
        .find('{')
        .map(|offset| signature_start + offset)
        .ok_or_else(|| format!("missing function body: {signature}"))?;
    let mut depth = 0usize;
    for (offset, byte) in source[body_start..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(&source[body_start..=body_start + offset]);
                }
            }
            _ => {}
        }
    }
    Err(format!("unterminated function body: {signature}").into())
}
