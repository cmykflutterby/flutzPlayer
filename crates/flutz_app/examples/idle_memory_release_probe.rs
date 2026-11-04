//! External probe for the idle memory release work tracked in
//! `design-docs/memory-idle-release-tracker.md`.
//!
//! Run: `cargo run -q -p flutzplayer --example idle_memory_release_probe [--features jemalloc-memory]`
//! Prints one JSON line per check and exits non-zero if any check fails.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use flutz_app::{
    app::{DatStartupSummary, SoundFontCatalogEntry},
    dat_startup_summary_for_data_dir,
    memory_runtime::{self, MemoryRuntimeSnapshot},
    playback::{AudioBackend, AudioPlaybackStatus, PlaybackController},
};
use flutz_core::default_preset_set;
use flutz_fmid::{read_fmid, FmidFile, MixerSourceMode};
use flutz_formats::{builtin_registry, DecodedAudioStreamSource};

#[cfg(feature = "jemalloc-memory")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_MIDI: &str = "MIDI Files/rendering-parity-midi/tmnt-water-loop-test.fmid";
const DEFAULT_DECODED: &str = "encoded-audio/flac-44100-stereo.flac";
const DEFAULT_DATA_DIR: &str = "drops/flutzplayer/data";

struct Checks {
    failures: Vec<String>,
}

impl Checks {
    fn check(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        println!(
            "{{\"event\":\"check\",\"name\":\"{name}\",\"ok\":{ok},\"detail\":\"{}\"}}",
            escape_json(detail.as_ref())
        );
        if !ok {
            self.failures.push(name.to_owned());
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    memory_runtime::initialize_from_env(false);
    let mut checks = Checks {
        failures: Vec::new(),
    };

    policy_checks(&mut checks);
    clock_and_service_checks(&mut checks);
    source_wiring_checks(&mut checks);

    let data_dir = PathBuf::from(DEFAULT_DATA_DIR);
    let catalog = soundfont_catalog(&data_dir)?;
    let mut playback = PlaybackController::new(data_dir, catalog, AudioBackend::Sdl3, false, false);
    midi_release_checks(&mut checks, &mut playback, Path::new(DEFAULT_MIDI))?;
    decoded_content_change_checks(&mut checks, &mut playback, Path::new(DEFAULT_DECODED))?;

    println!(
        "{{\"event\":\"idle_memory_release_summary\",\"jemalloc\":{},\"failures\":{}}}",
        cfg!(feature = "jemalloc-memory"),
        checks.failures.len()
    );
    if checks.failures.is_empty() {
        Ok(())
    } else {
        Err(format!("failed checks: {}", checks.failures.join(", ")).into())
    }
}

fn policy_checks(checks: &mut Checks) {
    checks.check(
        "deep_release_threshold_is_15_minutes",
        memory_runtime::IDLE_DEEP_RELEASE_AFTER == Duration::from_secs(15 * 60),
        format!("{:?}", memory_runtime::IDLE_DEEP_RELEASE_AFTER),
    );
    checks.check(
        "resume_prewarm_threshold_is_30_seconds",
        memory_runtime::RESUME_PREWARM_AFTER == Duration::from_secs(30),
        format!("{:?}", memory_runtime::RESUME_PREWARM_AFTER),
    );
    let fifteen = Duration::from_secs(15 * 60);
    checks.check(
        "deep_release_policy",
        !memory_runtime::should_deep_release(fifteen - Duration::from_secs(1), false)
            && memory_runtime::should_deep_release(fifteen, false)
            && !memory_runtime::should_deep_release(fifteen * 2, true),
        "fires once at >= 15 min idle",
    );
    checks.check(
        "resume_prewarm_policy",
        !memory_runtime::should_prewarm_on_resume(Duration::from_secs(29), false)
            && memory_runtime::should_prewarm_on_resume(Duration::from_secs(30), false)
            && memory_runtime::should_prewarm_on_resume(Duration::ZERO, true),
        "prewarm at >= 30 s idle or whenever arenas are cold",
    );
}

fn clock_and_service_checks(checks: &mut Checks) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let wake_counter = Arc::clone(&wakes);
    memory_runtime::set_idle_release_waker(Arc::new(move || {
        wake_counter.fetch_add(1, Ordering::Relaxed);
    }));
    let _ = memory_runtime::take_ui_idle_release_request();

    memory_runtime::note_audio_activity();
    checks.check(
        "activity_resets_idle_clock",
        memory_runtime::audio_idle_duration() < Duration::from_millis(250)
            && !memory_runtime::deep_release_done(),
        format!("{:?}", memory_runtime::audio_idle_duration()),
    );
    checks.check(
        "service_waits_for_threshold",
        !memory_runtime::service_idle_release(Duration::from_secs(3600)),
        "no release before threshold",
    );
    thread::sleep(Duration::from_millis(80));
    let released = memory_runtime::service_idle_release(Duration::from_millis(50));
    checks.check(
        "service_releases_after_threshold",
        released
            && memory_runtime::deep_release_done()
            && memory_runtime::audio_arenas_cold()
            && wakes.load(Ordering::Relaxed) == 1,
        format!(
            "released={released} done={} cold={} wakes={}",
            memory_runtime::deep_release_done(),
            memory_runtime::audio_arenas_cold(),
            wakes.load(Ordering::Relaxed)
        ),
    );
    let first_take = memory_runtime::take_ui_idle_release_request();
    let second_take = memory_runtime::take_ui_idle_release_request();
    checks.check(
        "ui_release_request_is_one_shot",
        first_take && !second_take,
        format!("first={first_take} second={second_take}"),
    );
    checks.check(
        "service_releases_once_per_idle_period",
        !memory_runtime::service_idle_release(Duration::from_millis(50))
            && wakes.load(Ordering::Relaxed) == 1,
        "second service call is a no-op",
    );
    memory_runtime::note_audio_activity();
    checks.check(
        "activity_rearms_deep_release",
        !memory_runtime::deep_release_done(),
        "note_audio_activity clears the released flag",
    );
    memory_runtime::set_idle_release_waker(Arc::new(|| {}));
}

fn source_wiring_checks(checks: &mut Checks) {
    let controller = include_str!("../src/playback/controller.rs");
    let maintenance = include_str!("../src/memory_runtime/maintenance.rs");
    let lifecycle = include_str!("../src/app/lifecycle.rs");
    let midi = include_str!("../src/playback/midi.rs");
    let decoded = include_str!("../src/playback/decoded_audio.rs");
    let idle_memory = include_str!("../src/playback/idle_memory.rs");
    let idle = include_str!("../src/memory_runtime/idle.rs");

    checks.check(
        "audio_callbacks_note_activity",
        controller
            .matches("memory_runtime::note_audio_activity();")
            .count()
            >= 2,
        "decoded and MIDI callbacks",
    );
    checks.check(
        "play_prewarms_when_cold",
        controller.contains("self.prewarm_if_resuming_cold();"),
        "play() calls prewarm_if_resuming_cold",
    );
    checks.check(
        "maintenance_services_idle_release",
        maintenance.contains("idle::service_idle_release_with(")
            && maintenance.contains("idle::IDLE_DEEP_RELEASE_AFTER"),
        "maintenance thread owns the 15 min deadline",
    );
    checks.check(
        "ui_installs_waker_and_services_request",
        lifecycle.contains("set_idle_release_waker(")
            && lifecycle.contains("take_ui_idle_release_request()")
            && lifecycle.contains("self.service_idle_memory_release();"),
        "lifecycle.rs",
    );
    checks.check(
        "content_change_releases",
        midi.contains("memory_runtime::release_after_content_change();")
            && decoded
                .matches("memory_runtime::release_after_content_change();")
                .count()
                == 2
            && decoded
                .matches("self.soundfont_cache.release_unused();")
                .count()
                == 2,
        "MIDI load and both decoded loads",
    );
    let no_inline_tests = [
        controller,
        maintenance,
        lifecycle,
        midi,
        decoded,
        idle_memory,
        idle,
    ]
    .iter()
    .all(|source| !source.contains("#[cfg(test)]"));
    checks.check(
        "no_inline_tests",
        no_inline_tests,
        "validation lives in external probes",
    );
}

fn midi_release_checks(
    checks: &mut Checks,
    playback: &mut PlaybackController,
    input: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    load_fmid(playback, input)?;
    let before_content = domain_purges(&memory_runtime::snapshot(), "file.load");

    let status = playback.play()?;
    let audible = matches!(status, AudioPlaybackStatus::Audible);
    println!(
        "{{\"event\":\"midi_play\",\"audible\":{audible},\"status\":\"{}\"}}",
        escape_json(&format!("{status:?}"))
    );
    thread::sleep(Duration::from_millis(400));
    if audible {
        checks.check(
            "audio_callback_notes_activity",
            memory_runtime::audio_idle_duration() < Duration::from_millis(250),
            format!("idle={:?}", memory_runtime::audio_idle_duration()),
        );
    }
    checks.check(
        "release_refused_while_playing",
        !playback.release_idle_memory()?,
        "playback_active guard",
    );
    playback.pause()?;
    let paused = playback.debug_metrics();
    let soundfont_entries =
        paused.soundfont_cache.full_entries + paused.soundfont_cache.subset_entries;

    let released = playback.release_idle_memory()?;
    let after_release = playback.debug_metrics();
    checks.check(
        "release_keeps_content_position_and_pause",
        released
            && after_release.engine_state == paused.engine_state
            && after_release.transport_tick == paused.transport_tick
            && after_release.soundfont_cache.full_entries
                + after_release.soundfont_cache.subset_entries
                == soundfont_entries
            && memory_runtime::audio_arenas_cold(),
        format!(
            "released={released} state={}->{} tick={}->{} sf_entries={}->{} audio={}",
            paused.engine_state,
            after_release.engine_state,
            paused.transport_tick,
            after_release.transport_tick,
            soundfont_entries,
            after_release.soundfont_cache.full_entries
                + after_release.soundfont_cache.subset_entries,
            after_release.audio_status,
        ),
    );
    print_domains("after_idle_release", &memory_runtime::snapshot());

    let status = playback.play()?;
    let report = playback.last_prewarm_report();
    checks.check(
        "play_after_release_prewarms",
        report.is_some_and(|report| report.arenas_were_cold && report.soundfont_pages > 0)
            && !memory_runtime::audio_arenas_cold(),
        format!("status={status:?} report={report:?}"),
    );
    thread::sleep(Duration::from_millis(300));
    let resumed = playback.debug_metrics();
    if audible {
        checks.check(
            "play_resumes_from_paused_position",
            resumed.transport_tick >= paused.transport_tick && playback.playback_active(),
            format!(
                "tick {} -> {} state={}",
                paused.transport_tick, resumed.transport_tick, resumed.engine_state
            ),
        );
    }
    playback.pause()?;
    let warm = playback.prewarm_for_resume();
    checks.check(
        "manual_prewarm_reports_warm_arenas",
        !warm.arenas_were_cold && warm.soundfont_pages > 0,
        format!("{warm:?}"),
    );
    print_domains("after_prewarm", &memory_runtime::snapshot());

    let after_content = domain_purges(&memory_runtime::snapshot(), "file.load");
    if cfg!(feature = "jemalloc-memory") {
        checks.check(
            "idle_release_purged_file_load_domain",
            after_content > before_content,
            format!("purges {before_content} -> {after_content}"),
        );
    }
    Ok(())
}

fn decoded_content_change_checks(
    checks: &mut Checks,
    playback: &mut PlaybackController,
    input: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let before = playback.debug_metrics();
    let purges_before = domain_purges(&memory_runtime::snapshot(), "playback.load");
    let registry = builtin_registry();
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or("decoded input has no extension")?;
    let descriptor = registry
        .find_by_extension(extension)
        .ok_or("decoded input extension is not registered")?;
    playback.load_decoded_audio_stream(
        input.to_path_buf(),
        descriptor.id,
        descriptor.friendly_name,
        DecodedAudioStreamSource::Path(input.to_path_buf()),
        None,
    )?;
    let _ = playback.wait_for_decoded_stream_cache(Duration::from_secs(10));
    let after = playback.debug_metrics();
    let purges_after = domain_purges(&memory_runtime::snapshot(), "playback.load");
    checks.check(
        "decoded_load_trims_soundfont_cache",
        after.soundfont_cache.full_entries + after.soundfont_cache.subset_entries == 0
            && memory_runtime::audio_arenas_cold(),
        format!(
            "sf_entries {} -> {} cold={}",
            before.soundfont_cache.full_entries + before.soundfont_cache.subset_entries,
            after.soundfont_cache.full_entries + after.soundfont_cache.subset_entries,
            memory_runtime::audio_arenas_cold()
        ),
    );
    if cfg!(feature = "jemalloc-memory") {
        checks.check(
            "content_change_purges_load_domain",
            purges_after > purges_before,
            format!("playback.load purges {purges_before} -> {purges_after}"),
        );
    }

    playback.play()?;
    let report = playback.last_prewarm_report();
    checks.check(
        "decoded_play_prewarms_decoded_pages",
        report.is_some_and(|report| report.arenas_were_cold && report.decoded_pages > 0),
        format!("{report:?}"),
    );
    thread::sleep(Duration::from_millis(200));
    playback.pause()?;
    checks.check(
        "decoded_release_while_paused",
        playback.release_idle_memory()?,
        "paused decoded playback releases",
    );
    let warm = playback.prewarm_for_resume();
    checks.check(
        "decoded_prewarm_touches_samples",
        warm.decoded_pages > 0 && warm.soundfont_pages == 0,
        format!("{warm:?}"),
    );
    playback.stop()?;
    Ok(())
}

fn domain_purges(snapshot: &MemoryRuntimeSnapshot, name: &str) -> u64 {
    snapshot
        .domains
        .iter()
        .find(|domain| domain.name == name)
        .map(|domain| domain.purge_count)
        .unwrap_or(0)
}

fn print_domains(label: &str, snapshot: &MemoryRuntimeSnapshot) {
    for domain in snapshot.domains.iter().filter(|domain| {
        domain.warm_target_bytes > 0 || domain.prewarmed_bytes > 0 || domain.purge_count > 0
    }) {
        println!(
            "{{\"event\":\"domain\",\"label\":\"{label}\",\"name\":\"{}\",\"purges\":{},\"warm_target_bytes\":{},\"prewarmed_bytes\":{}}}",
            domain.name, domain.purge_count, domain.warm_target_bytes, domain.prewarmed_bytes
        );
    }
}

fn soundfont_catalog(
    data_dir: &Path,
) -> Result<Vec<SoundFontCatalogEntry>, Box<dyn std::error::Error>> {
    match dat_startup_summary_for_data_dir(data_dir)? {
        DatStartupSummary::Available { soundfonts, .. } => Ok(soundfonts),
        DatStartupSummary::Unavailable(error) => Err(error.into()),
    }
}

fn load_fmid(
    playback: &mut PlaybackController,
    input: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let fmid = read_fmid(&fs::read(input)?)?;
    playback.load_midi_bytes(
        fmid.midi_bytes.clone(),
        fmid.project.source_midi_filename.clone(),
        &requested_soundfonts(&fmid),
    )?;
    Ok(())
}

fn requested_soundfonts(fmid: &FmidFile) -> Vec<String> {
    match &fmid.mixer_source_mode {
        MixerSourceMode::Custom => fmid
            .soundfonts
            .iter()
            .map(|slot| slot.internal_id.clone())
            .collect(),
        MixerSourceMode::PresetDefault(preset_id) => default_preset_set()
            .find_preset(preset_id)
            .unwrap_or_else(|| default_preset_set().default_preset())
            .font_ids
            .iter()
            .map(|font_id| (*font_id).to_owned())
            .collect(),
    }
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
