use std::sync::atomic::AtomicBool;

use super::*;

/// Time without audible playback before the deep idle release, regardless of window state.
pub const IDLE_DEEP_RELEASE_AFTER: Duration = Duration::from_secs(15 * 60);
/// Time without audible playback after which `play()` pre-touches memory before resuming.
pub const RESUME_PREWARM_AFTER: Duration = Duration::from_secs(30);

pub type IdleReleaseWaker = Arc<dyn Fn() + Send + Sync>;

static CLOCK_ORIGIN: OnceLock<Instant> = OnceLock::new();
static LAST_AUDIO_ACTIVITY_MS: AtomicU64 = AtomicU64::new(0);
static DEEP_RELEASED: AtomicBool = AtomicBool::new(false);
static AUDIO_ARENAS_COLD: AtomicBool = AtomicBool::new(false);
static UI_RELEASE_PENDING: AtomicBool = AtomicBool::new(false);
static IDLE_RELEASE_WAKER: Mutex<Option<IdleReleaseWaker>> = Mutex::new(None);

fn clock_ms() -> u64 {
    CLOCK_ORIGIN.get_or_init(Instant::now).elapsed().as_millis() as u64
}

pub(super) fn start_clock() {
    CLOCK_ORIGIN.get_or_init(Instant::now);
}

/// Records that audio is audibly playing right now. Allocation- and lock-free, so the
/// audio callback can call it every block.
pub fn note_audio_activity() {
    LAST_AUDIO_ACTIVITY_MS.store(clock_ms(), Ordering::Relaxed);
    if DEEP_RELEASED.load(Ordering::Relaxed) {
        DEEP_RELEASED.store(false, Ordering::Relaxed);
    }
}

/// Time since audio last played (or since process start if it never has).
pub fn audio_idle_duration() -> Duration {
    Duration::from_millis(clock_ms().saturating_sub(LAST_AUDIO_ACTIVITY_MS.load(Ordering::Relaxed)))
}

pub fn should_deep_release(idle: Duration, already_released: bool) -> bool {
    !already_released && idle >= IDLE_DEEP_RELEASE_AFTER
}

pub fn should_prewarm_on_resume(idle: Duration, audio_arenas_cold: bool) -> bool {
    audio_arenas_cold || idle >= RESUME_PREWARM_AFTER
}

pub fn audio_arenas_cold() -> bool {
    AUDIO_ARENAS_COLD.load(Ordering::Relaxed)
}

pub(super) fn set_audio_arenas_cold(cold: bool) {
    AUDIO_ARENAS_COLD.store(cold, Ordering::Relaxed);
}

pub fn deep_release_done() -> bool {
    DEEP_RELEASED.load(Ordering::Relaxed)
}

/// Returns true once per deep release so the UI can drop its playback-held buffers.
pub fn take_ui_idle_release_request() -> bool {
    UI_RELEASE_PENDING.swap(false, Ordering::AcqRel)
}

pub fn set_idle_release_waker(waker: IdleReleaseWaker) {
    if let Ok(mut slot) = IDLE_RELEASE_WAKER.lock() {
        *slot = Some(waker);
    }
}

/// Runs the deep idle release once audio has been idle for `threshold`. Called by the
/// maintenance thread with `IDLE_DEEP_RELEASE_AFTER`; probes pass a short threshold.
pub fn service_idle_release(threshold: Duration) -> bool {
    service_idle_release_with(&global(), threshold)
}

pub(super) fn service_idle_release_with(runtime: &MemoryRuntime, threshold: Duration) -> bool {
    let idle = audio_idle_duration();
    if DEEP_RELEASED.load(Ordering::Relaxed) || idle < threshold {
        return false;
    }
    DEEP_RELEASED.store(true, Ordering::Relaxed);
    runtime.release_idle_now();
    UI_RELEASE_PENDING.store(true, Ordering::Release);
    let waker = IDLE_RELEASE_WAKER
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(Arc::clone));
    if let Some(waker) = waker {
        waker();
    }
    true
}
