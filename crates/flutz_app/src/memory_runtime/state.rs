#[cfg(feature = "jemalloc-memory")]
use super::jemalloc::{
    arena_page_bytes, bind_thread_to_arena, configure_arena, configure_global, create_arena,
    decay_arena, prewarm_arena, purge_arena, read_totals,
};
use super::json::{domain_json, escape_json, os_json, totals_json};
use super::system::read_os_memory;
#[cfg(not(feature = "jemalloc-memory"))]
use super::system::{
    arena_page_bytes, bind_thread_to_arena, configure_arena, configure_global, create_arena,
    decay_arena, prewarm_arena, purge_arena, read_totals,
};
use super::*;

/// Upper bound on how much one reuse-critical arena is pre-touched before resume.
pub const PREWARM_MAX_BYTES_PER_DOMAIN: u64 = 64 << 20;

/// Load-path arenas purged on a content change; reuse-critical arenas are purged too.
const CONTENT_CHANGE_DOMAINS: [MemoryDomain; 3] = [
    MemoryDomain::FileLoad,
    MemoryDomain::PlaybackLoad,
    MemoryDomain::SoundFontDecode,
];

#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryTotalsSnapshot {
    pub available: bool,
    pub allocated_bytes: usize,
    pub active_bytes: usize,
    pub resident_bytes: usize,
    pub retained_bytes: usize,
    pub mapped_bytes: usize,
    pub allocated_bytes_per_sec: f64,
    pub active_growth_bytes: i64,
    pub resident_growth_bytes: i64,
    pub retained_growth_bytes: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OsMemorySnapshot {
    pub available: bool,
    pub working_set_bytes: u64,
    pub peak_working_set_bytes: u64,
    pub pagefile_bytes: u64,
    pub peak_pagefile_bytes: u64,
    pub page_fault_count: u32,
    pub working_set_growth_bytes: i64,
    pub pagefile_growth_bytes: i64,
    pub working_set_minus_jemalloc_resident_bytes: i64,
    pub pagefile_minus_jemalloc_active_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct MemoryDomainSnapshot {
    pub name: &'static str,
    pub arena: Option<usize>,
    pub reuse_critical: bool,
    pub dirty_decay_ms: isize,
    pub muzzy_decay_ms: isize,
    pub bind_count: u64,
    pub decay_count: u64,
    pub purge_count: u64,
    pub growth_events: u64,
    pub retained_floor_hits: u64,
    pub zero_growth_streak: u64,
    pub warm_target_bytes: u64,
    pub prewarmed_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct MemoryRuntimeSnapshot {
    pub enabled: bool,
    pub allocator: &'static str,
    pub config_summary: String,
    pub initialized: bool,
    pub totals: MemoryTotalsSnapshot,
    pub os: OsMemorySnapshot,
    pub domains: Vec<MemoryDomainSnapshot>,
    pub errors: Vec<String>,
}

impl MemoryRuntimeSnapshot {
    pub fn to_json(&self) -> String {
        let mut json = format!(
            "{{\"enabled\":{},\"allocator\":\"{}\",\"initialized\":{},\"config\":\"{}\",\"totals\":{},\"os\":{}",
            self.enabled,
            escape_json(self.allocator),
            self.initialized,
            escape_json(&self.config_summary),
            totals_json(&self.totals),
            os_json(&self.os)
        );
        json.push_str(",\"domains\":[");
        for (index, domain) in self.domains.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str(&domain_json(domain));
        }
        json.push(']');
        if !self.errors.is_empty() {
            json.push_str(",\"errors\":[");
            for (index, error) in self.errors.iter().enumerate() {
                if index > 0 {
                    json.push(',');
                }
                json.push('"');
                json.push_str(&escape_json(error));
                json.push('"');
            }
            json.push(']');
        }
        json.push('}');
        json
    }
}

#[derive(Debug)]
pub struct MemoryThreadGuard {
    previous_domain: Option<MemoryDomain>,
}

impl Drop for MemoryThreadGuard {
    fn drop(&mut self) {
        CURRENT_DOMAIN.with(|current| {
            current.set(
                self.previous_domain
                    .map(|domain| domain as usize)
                    .unwrap_or(usize::MAX),
            );
        });
    }
}

#[derive(Debug)]
pub(super) struct DomainRuntime {
    domain: MemoryDomain,
    arena: Option<usize>,
    dirty_decay_ms: isize,
    muzzy_decay_ms: isize,
    bind_count: AtomicU64,
    decay_count: AtomicU64,
    purge_count: AtomicU64,
    growth_events: AtomicU64,
    retained_floor_hits: AtomicU64,
    zero_growth_streak: AtomicU64,
    warm_target_bytes: AtomicU64,
    prewarmed_bytes: AtomicU64,
}

impl DomainRuntime {
    fn snapshot(&self) -> MemoryDomainSnapshot {
        MemoryDomainSnapshot {
            name: self.domain.label(),
            arena: self.arena,
            reuse_critical: self.domain.reuse_critical(),
            dirty_decay_ms: self.dirty_decay_ms,
            muzzy_decay_ms: self.muzzy_decay_ms,
            bind_count: self.bind_count.load(Ordering::Relaxed),
            decay_count: self.decay_count.load(Ordering::Relaxed),
            purge_count: self.purge_count.load(Ordering::Relaxed),
            growth_events: self.growth_events.load(Ordering::Relaxed),
            retained_floor_hits: self.retained_floor_hits.load(Ordering::Relaxed),
            zero_growth_streak: self.zero_growth_streak.load(Ordering::Relaxed),
            warm_target_bytes: self.warm_target_bytes.load(Ordering::Relaxed),
            prewarmed_bytes: self.prewarmed_bytes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SamplePoint {
    at: Instant,
    totals: MemoryTotalsSnapshot,
    os: OsMemorySnapshot,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MaintenancePoint {
    last_decay: Option<Instant>,
    last_purge: Option<Instant>,
}

#[derive(Debug)]
pub struct MemoryRuntime {
    enabled: bool,
    config: MemoryRuntimeConfig,
    domains: BTreeMap<MemoryDomain, DomainRuntime>,
    errors: Mutex<Vec<String>>,
    previous_sample: Mutex<Option<SamplePoint>>,
    maintenance: Mutex<BTreeMap<MemoryDomain, MaintenancePoint>>,
}

impl MemoryRuntime {
    pub(super) fn new(config: MemoryRuntimeConfig) -> Self {
        let mut errors = Vec::new();
        #[cfg(feature = "jemalloc-memory")]
        let enabled = true;
        #[cfg(not(feature = "jemalloc-memory"))]
        let enabled = false;

        if enabled {
            if let Err(error) = configure_global(&config) {
                errors.push(error);
            }
        }

        let mut domains = BTreeMap::new();
        for domain in MemoryDomain::ALL {
            let arena = if enabled {
                create_arena().map_err(|error| errors.push(error)).ok()
            } else {
                None
            };
            let dirty_decay_ms = if domain.reuse_critical() {
                config.hot_dirty_decay_ms
            } else {
                config.dirty_decay_ms
            };
            let muzzy_decay_ms = if domain.reuse_critical() {
                config.hot_muzzy_decay_ms
            } else {
                config.muzzy_decay_ms
            };
            if let Some(arena) = arena {
                if let Err(error) = configure_arena(arena, dirty_decay_ms, muzzy_decay_ms) {
                    errors.push(error);
                }
            }
            domains.insert(
                domain,
                DomainRuntime {
                    domain,
                    arena,
                    dirty_decay_ms,
                    muzzy_decay_ms,
                    bind_count: AtomicU64::new(0),
                    decay_count: AtomicU64::new(0),
                    purge_count: AtomicU64::new(0),
                    growth_events: AtomicU64::new(0),
                    retained_floor_hits: AtomicU64::new(0),
                    zero_growth_streak: AtomicU64::new(0),
                    warm_target_bytes: AtomicU64::new(0),
                    prewarmed_bytes: AtomicU64::new(0),
                },
            );
        }

        idle::start_clock();

        Self {
            enabled,
            config,
            domains,
            errors: Mutex::new(errors),
            previous_sample: Mutex::new(None),
            maintenance: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn bind_current_thread(&self, domain: MemoryDomain) -> MemoryThreadGuard {
        let previous_domain = CURRENT_DOMAIN.with(|current| {
            let previous = domain_from_index(current.get());
            current.set(domain as usize);
            previous
        });

        if let Some(runtime) = self.domains.get(&domain) {
            runtime.bind_count.fetch_add(1, Ordering::Relaxed);
            if let Some(arena) = runtime.arena {
                if let Err(error) = bind_thread_to_arena(arena) {
                    self.push_error(error);
                }
            }
        }

        MemoryThreadGuard { previous_domain }
    }

    pub fn decay_domain(&self, domain: MemoryDomain) {
        if !self.should_decay(domain) {
            return;
        }
        if let Some(runtime) = self.domains.get(&domain) {
            runtime.decay_count.fetch_add(1, Ordering::Relaxed);
            if let Some(arena) = runtime.arena {
                if let Err(error) = decay_arena(arena) {
                    self.push_error(error);
                }
            }
        }
    }

    pub fn purge_domain(&self, domain: MemoryDomain) {
        if !self.should_purge(domain) {
            return;
        }
        if let Some(runtime) = self.domains.get(&domain) {
            runtime.purge_count.fetch_add(1, Ordering::Relaxed);
            if let Some(arena) = runtime.arena {
                if let Err(error) = purge_arena(arena) {
                    self.push_error(error);
                }
            }
        }
    }

    pub fn decay_idle_reuse_preserving(&self, playback_active: bool) {
        for domain in MemoryDomain::ALL {
            if playback_active && domain.reuse_critical() {
                continue;
            }
            self.decay_domain(domain);
        }
    }

    pub fn remediate_idle_os_pressure(&self, playback_active: bool) {
        if playback_active {
            self.decay_all_idle_domains();
            return;
        }
        self.decay_idle_reuse_preserving(false);
        for domain in MemoryDomain::ALL {
            self.purge_domain(domain);
        }
    }

    pub fn decay_all_idle_domains(&self) {
        for domain in MemoryDomain::ALL {
            if !domain.reuse_critical() {
                self.decay_domain(domain);
            }
        }
    }

    pub fn aggressive_release(&self) {
        for domain in MemoryDomain::ALL {
            self.purge_domain(domain);
        }
    }

    /// Remembers each reuse-critical arena's in-use (active) bytes as its working set, so
    /// a later `prewarm_reuse_critical_domains` can commit that much again before audio
    /// resumes. Stale dirty pages are deliberately not counted.
    pub fn record_audio_working_set(&self) {
        for runtime in self.domains.values() {
            if !runtime.domain.reuse_critical() {
                continue;
            }
            let Some(arena) = runtime.arena else {
                continue;
            };
            match arena_page_bytes(arena) {
                Ok((active, _dirty)) => {
                    runtime
                        .warm_target_bytes
                        .fetch_max(active as u64, Ordering::Relaxed);
                }
                Err(error) => self.push_error(error),
            }
        }
    }

    /// New content is loaded and the audio stream is paused or closed: return the load
    /// arenas and the previous content's audio pages to the OS.
    pub fn release_after_content_change(&self) {
        self.record_audio_working_set();
        for domain in MemoryDomain::ALL {
            if domain.reuse_critical() || CONTENT_CHANGE_DOMAINS.contains(&domain) {
                self.purge_domain_now(domain);
            }
        }
        idle::set_audio_arenas_cold(true);
    }

    /// Deep idle release: purge every arena, including the reuse-critical ones.
    pub fn release_idle_now(&self) {
        self.record_audio_working_set();
        for domain in MemoryDomain::ALL {
            self.purge_domain_now(domain);
        }
        idle::set_audio_arenas_cold(true);
    }

    /// Commits the recorded footprint of each reuse-critical arena again (minus what it
    /// still holds), so the audio callback doesn't fault in fresh pages after a release.
    pub fn prewarm_reuse_critical_domains(&self) -> u64 {
        let mut warmed_total = 0u64;
        for runtime in self.domains.values() {
            if !runtime.domain.reuse_critical() {
                continue;
            }
            let target = runtime.warm_target_bytes.swap(0, Ordering::Relaxed);
            let Some(arena) = runtime.arena else {
                continue;
            };
            let current = match arena_page_bytes(arena) {
                Ok((active, dirty)) => active.saturating_add(dirty) as u64,
                Err(error) => {
                    self.push_error(error);
                    continue;
                }
            };
            let missing = target
                .saturating_sub(current)
                .min(PREWARM_MAX_BYTES_PER_DOMAIN);
            if missing == 0 {
                continue;
            }
            match prewarm_arena(arena, missing as usize) {
                Ok(warmed) => {
                    runtime
                        .prewarmed_bytes
                        .fetch_add(warmed as u64, Ordering::Relaxed);
                    warmed_total = warmed_total.saturating_add(warmed as u64);
                }
                Err(error) => self.push_error(error),
            }
        }
        idle::set_audio_arenas_cold(false);
        warmed_total
    }

    fn purge_domain_now(&self, domain: MemoryDomain) {
        if let Ok(mut maintenance) = self.maintenance.lock() {
            maintenance
                .entry(domain)
                .or_insert(MaintenancePoint {
                    last_decay: None,
                    last_purge: None,
                })
                .last_purge = Some(Instant::now());
        }
        if let Some(runtime) = self.domains.get(&domain) {
            runtime.purge_count.fetch_add(1, Ordering::Relaxed);
            if let Some(arena) = runtime.arena {
                if let Err(error) = purge_arena(arena) {
                    self.push_error(error);
                }
            }
        }
    }

    pub fn snapshot_after_maintenance(&self, playback_active: bool) -> MemoryRuntimeSnapshot {
        self.decay_idle_reuse_preserving(playback_active);
        self.snapshot()
    }

    pub fn snapshot_after_pressure_remediation(
        &self,
        playback_active: bool,
    ) -> MemoryRuntimeSnapshot {
        self.remediate_idle_os_pressure(playback_active);
        self.snapshot()
    }

    pub fn snapshot(&self) -> MemoryRuntimeSnapshot {
        let mut totals = read_totals().unwrap_or_default();
        let mut os = read_os_memory(totals).unwrap_or_default();
        let now = Instant::now();
        if let Ok(mut previous) = self.previous_sample.lock() {
            if let Some(previous_sample) = *previous {
                let elapsed = now.duration_since(previous_sample.at).as_secs_f64();
                if elapsed > 0.0 {
                    let allocated_delta = totals
                        .allocated_bytes
                        .saturating_sub(previous_sample.totals.allocated_bytes);
                    totals.allocated_bytes_per_sec = allocated_delta as f64 / elapsed;
                    totals.active_growth_bytes =
                        totals.active_bytes as i64 - previous_sample.totals.active_bytes as i64;
                    totals.resident_growth_bytes =
                        totals.resident_bytes as i64 - previous_sample.totals.resident_bytes as i64;
                    totals.retained_growth_bytes =
                        totals.retained_bytes as i64 - previous_sample.totals.retained_bytes as i64;
                    os.working_set_growth_bytes =
                        os.working_set_bytes as i64 - previous_sample.os.working_set_bytes as i64;
                    os.pagefile_growth_bytes =
                        os.pagefile_bytes as i64 - previous_sample.os.pagefile_bytes as i64;
                    self.record_growth(totals.retained_growth_bytes);
                    if !self.is_playback_pressure_safe(os) {
                        for domain in MemoryDomain::ALL {
                            if !domain.reuse_critical() {
                                self.purge_domain(domain);
                            }
                        }
                    }
                }
            }
            *previous = Some(SamplePoint {
                at: now,
                totals,
                os,
            });
        }

        MemoryRuntimeSnapshot {
            enabled: self.enabled,
            allocator: allocator_name(),
            config_summary: self.config.summary(),
            initialized: true,
            totals,
            os,
            domains: self.domains.values().map(DomainRuntime::snapshot).collect(),
            errors: self
                .errors
                .lock()
                .map(|errors| errors.clone())
                .unwrap_or_default(),
        }
    }

    fn record_growth(&self, retained_growth_bytes: i64) {
        for domain in self.domains.values() {
            if !domain.domain.reuse_critical() {
                continue;
            }
            if retained_growth_bytes > 0 {
                domain.growth_events.fetch_add(1, Ordering::Relaxed);
                domain.zero_growth_streak.store(0, Ordering::Relaxed);
            } else {
                domain.retained_floor_hits.fetch_add(1, Ordering::Relaxed);
                domain.zero_growth_streak.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn push_error(&self, error: String) {
        if let Ok(mut errors) = self.errors.lock() {
            if !errors.contains(&error) {
                errors.push(error);
            }
        }
    }

    fn should_decay(&self, domain: MemoryDomain) -> bool {
        self.should_run_maintenance(domain, DECAY_RATE_LIMIT, true)
    }

    fn should_purge(&self, domain: MemoryDomain) -> bool {
        self.should_run_maintenance(domain, PURGE_RATE_LIMIT, false)
    }

    fn should_run_maintenance(
        &self,
        domain: MemoryDomain,
        rate_limit: Duration,
        decay: bool,
    ) -> bool {
        let Ok(mut maintenance) = self.maintenance.lock() else {
            return true;
        };
        let now = Instant::now();
        let point = maintenance.entry(domain).or_insert(MaintenancePoint {
            last_decay: None,
            last_purge: None,
        });
        let last = if decay {
            &mut point.last_decay
        } else {
            &mut point.last_purge
        };
        if last.is_some_and(|previous| now.duration_since(previous) < rate_limit) {
            return false;
        }
        *last = Some(now);
        true
    }

    fn is_playback_pressure_safe(&self, os: OsMemorySnapshot) -> bool {
        os.working_set_growth_bytes <= self.config.idle_working_set_growth_bytes as i64
    }
}
