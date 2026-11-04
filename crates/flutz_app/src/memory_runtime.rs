use std::{
    cell::Cell,
    collections::BTreeMap,
    env, fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

mod config;
mod idle;
mod jemalloc;
mod json;
mod maintenance;
mod state;
mod system;
use config::{allocator_name, domain_from_index, DECAY_RATE_LIMIT, PURGE_RATE_LIMIT};
pub use config::{MemoryDomain, MemoryRuntimeConfig};
pub use idle::{
    audio_arenas_cold, audio_idle_duration, deep_release_done, note_audio_activity,
    service_idle_release, set_idle_release_waker, should_deep_release, should_prewarm_on_resume,
    take_ui_idle_release_request, IdleReleaseWaker, IDLE_DEEP_RELEASE_AFTER, RESUME_PREWARM_AFTER,
};
pub use maintenance::{
    MemoryMaintenanceHandle, MEMORY_MAINTENANCE_INTERVAL, MEMORY_MAINTENANCE_THREAD_NAME,
};
pub use state::{
    MemoryDomainSnapshot, MemoryRuntime, MemoryRuntimeSnapshot, MemoryThreadGuard,
    MemoryTotalsSnapshot, OsMemorySnapshot, PREWARM_MAX_BYTES_PER_DOMAIN,
};

static MEMORY_RUNTIME: OnceLock<Arc<MemoryRuntime>> = OnceLock::new();

thread_local! {
    static CURRENT_DOMAIN: Cell<usize> = const { Cell::new(usize::MAX) };
}

pub fn initialize_from_env(debug_memory: bool) -> Arc<MemoryRuntime> {
    MEMORY_RUNTIME
        .get_or_init(|| {
            Arc::new(MemoryRuntime::new(MemoryRuntimeConfig::from_env(
                debug_memory,
            )))
        })
        .clone()
}

pub fn global() -> Arc<MemoryRuntime> {
    MEMORY_RUNTIME
        .get_or_init(|| Arc::new(MemoryRuntime::new(MemoryRuntimeConfig::from_env(false))))
        .clone()
}

pub fn bind_current_thread(domain: MemoryDomain) -> MemoryThreadGuard {
    global().bind_current_thread(domain)
}

pub fn decay_domain(domain: MemoryDomain) {
    global().decay_domain(domain);
}

pub fn purge_domain(domain: MemoryDomain) {
    global().purge_domain(domain);
}

pub fn decay_idle_reuse_preserving(playback_active: bool) {
    global().decay_idle_reuse_preserving(playback_active);
}

pub fn decay_all_idle_domains() {
    global().decay_all_idle_domains();
}

pub fn remediate_idle_os_pressure(playback_active: bool) {
    global().remediate_idle_os_pressure(playback_active);
}

pub fn aggressive_release() {
    global().aggressive_release();
}

pub fn record_audio_working_set() {
    global().record_audio_working_set();
}

pub fn release_after_content_change() {
    global().release_after_content_change();
}

pub fn release_idle_now() {
    global().release_idle_now();
}

pub fn prewarm_reuse_critical_domains() -> u64 {
    global().prewarm_reuse_critical_domains()
}

pub fn snapshot_after_maintenance(playback_active: bool) -> MemoryRuntimeSnapshot {
    global().snapshot_after_maintenance(playback_active)
}

pub fn snapshot_after_pressure_remediation(playback_active: bool) -> MemoryRuntimeSnapshot {
    global().snapshot_after_pressure_remediation(playback_active)
}

pub fn snapshot() -> MemoryRuntimeSnapshot {
    global().snapshot()
}

pub fn status_summary() -> String {
    let snapshot = snapshot();
    format!(
        "allocator={} enabled={} resident={} retained={} config={}",
        snapshot.allocator,
        snapshot.enabled,
        snapshot.totals.resident_bytes,
        snapshot.totals.retained_bytes,
        snapshot.config_summary
    )
}
