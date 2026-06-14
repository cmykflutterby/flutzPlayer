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

const DECAY_RATE_LIMIT: Duration = Duration::from_millis(250);
const PURGE_RATE_LIMIT: Duration = Duration::from_secs(2);

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryDomain {
    App,
    Ui,
    PerfTrace,
    FileLoad,
    PlaybackLoad,
    AudioCallback,
    PlaybackRender,
    RenderStems,
    RenderMixer,
    RenderSnapshot,
    SoundFontDecode,
    AudioBackend,
}

impl MemoryDomain {
    pub const ALL: [Self; 12] = [
        Self::App,
        Self::Ui,
        Self::PerfTrace,
        Self::FileLoad,
        Self::PlaybackLoad,
        Self::AudioCallback,
        Self::PlaybackRender,
        Self::RenderStems,
        Self::RenderMixer,
        Self::RenderSnapshot,
        Self::SoundFontDecode,
        Self::AudioBackend,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Ui => "ui",
            Self::PerfTrace => "perf.trace",
            Self::FileLoad => "file.load",
            Self::PlaybackLoad => "playback.load",
            Self::AudioCallback => "audio.callback",
            Self::PlaybackRender => "playback.render",
            Self::RenderStems => "render.stems",
            Self::RenderMixer => "render.mixer",
            Self::RenderSnapshot => "render.snapshot",
            Self::SoundFontDecode => "soundfont.decode",
            Self::AudioBackend => "audio.backend",
        }
    }

    pub const fn reuse_critical(self) -> bool {
        matches!(
            self,
            Self::AudioCallback
                | Self::PlaybackRender
                | Self::RenderStems
                | Self::RenderMixer
                | Self::AudioBackend
        )
    }
}

#[derive(Debug, Clone)]
pub struct MemoryRuntimeConfig {
    pub debug_memory: bool,
    pub retain: bool,
    pub background_thread: bool,
    pub max_background_threads: usize,
    pub dirty_decay_ms: isize,
    pub muzzy_decay_ms: isize,
    pub hot_dirty_decay_ms: isize,
    pub hot_muzzy_decay_ms: isize,
    pub lg_extent_max_active_fit: Option<usize>,
    pub dss: Option<String>,
    pub idle_working_set_growth_bytes: u64,
}

impl MemoryRuntimeConfig {
    fn from_env(debug_memory: bool) -> Self {
        Self {
            debug_memory,
            retain: env_bool("FLUTZ_JEMALLOC_RETAIN", true),
            background_thread: env_bool("FLUTZ_JEMALLOC_BACKGROUND_THREAD", false),
            max_background_threads: env_usize("FLUTZ_JEMALLOC_MAX_BACKGROUND_THREADS", 2),
            dirty_decay_ms: env_isize("FLUTZ_JEMALLOC_DIRTY_DECAY_MS", 30_000),
            muzzy_decay_ms: env_isize("FLUTZ_JEMALLOC_MUZZY_DECAY_MS", 30_000),
            hot_dirty_decay_ms: env_isize("FLUTZ_JEMALLOC_HOT_DIRTY_DECAY_MS", 120_000),
            hot_muzzy_decay_ms: env_isize("FLUTZ_JEMALLOC_HOT_MUZZY_DECAY_MS", 120_000),
            lg_extent_max_active_fit: env::var("FLUTZ_JEMALLOC_LG_EXTENT_MAX_ACTIVE_FIT")
                .ok()
                .and_then(|value| value.parse().ok()),
            dss: env::var("FLUTZ_JEMALLOC_DSS")
                .ok()
                .filter(|value| !value.is_empty()),
            idle_working_set_growth_bytes: env_u64(
                "FLUTZ_MEMORY_IDLE_WORKING_SET_GROWTH_BYTES",
                32 * 1024 * 1024,
            ),
        }
    }

    fn summary(&self) -> String {
        format!(
            "retain={} background_thread={} max_background_threads={} dirty_decay_ms={} muzzy_decay_ms={} hot_dirty_decay_ms={} hot_muzzy_decay_ms={} lg_extent_max_active_fit={} dss={} idle_working_set_growth_bytes={}",
            self.retain,
            self.background_thread,
            self.max_background_threads,
            self.dirty_decay_ms,
            self.muzzy_decay_ms,
            self.hot_dirty_decay_ms,
            self.hot_muzzy_decay_ms,
            self.lg_extent_max_active_fit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<default>".to_owned()),
            self.dss.as_deref().unwrap_or("<default>"),
            self.idle_working_set_growth_bytes
        )
    }
}

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
struct DomainRuntime {
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
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SamplePoint {
    at: Instant,
    totals: MemoryTotalsSnapshot,
    os: OsMemorySnapshot,
}

#[derive(Debug, Clone, Copy)]
struct MaintenancePoint {
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
    fn new(config: MemoryRuntimeConfig) -> Self {
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
                },
            );
        }

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

impl fmt::Display for MemoryDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

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

fn domain_from_index(index: usize) -> Option<MemoryDomain> {
    MemoryDomain::ALL
        .iter()
        .copied()
        .find(|domain| *domain as usize == index)
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_isize(name: &str, default: isize) -> isize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn allocator_name() -> &'static str {
    #[cfg(feature = "jemalloc-memory")]
    {
        "jemalloc"
    }
    #[cfg(not(feature = "jemalloc-memory"))]
    {
        "system"
    }
}

#[cfg(feature = "jemalloc-memory")]
fn configure_global(config: &MemoryRuntimeConfig) -> Result<(), String> {
    let mut errors = Vec::new();
    if config.background_thread {
        if let Err(error) = mallctl_write_bool("background_thread", true) {
            errors.push(error);
        }
        if let Err(error) =
            mallctl_write_usize("max_background_threads", config.max_background_threads)
        {
            errors.push(error);
        }
    }
    if let Some(value) = config.lg_extent_max_active_fit {
        if let Err(error) = mallctl_write_usize("opt.lg_extent_max_active_fit", value) {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(not(feature = "jemalloc-memory"))]
fn configure_global(_config: &MemoryRuntimeConfig) -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "jemalloc-memory")]
fn create_arena() -> Result<usize, String> {
    mallctl_read_u32("arenas.create").map(|arena| arena as usize)
}

#[cfg(not(feature = "jemalloc-memory"))]
fn create_arena() -> Result<usize, String> {
    Err("jemalloc-memory feature is disabled".to_owned())
}

fn configure_arena(
    arena: usize,
    dirty_decay_ms: isize,
    muzzy_decay_ms: isize,
) -> Result<(), String> {
    let dirty_name = format!("arena.{arena}.dirty_decay_ms");
    let muzzy_name = format!("arena.{arena}.muzzy_decay_ms");
    mallctl_write_isize(&dirty_name, dirty_decay_ms)?;
    mallctl_write_isize(&muzzy_name, muzzy_decay_ms)?;
    Ok(())
}

#[cfg(feature = "jemalloc-memory")]
fn bind_thread_to_arena(arena: usize) -> Result<(), String> {
    mallctl_write_u32("thread.arena", arena as u32)
}

#[cfg(not(feature = "jemalloc-memory"))]
fn bind_thread_to_arena(_arena: usize) -> Result<(), String> {
    Ok(())
}

fn decay_arena(arena: usize) -> Result<(), String> {
    mallctl_action(&format!("arena.{arena}.decay"))
}

fn purge_arena(arena: usize) -> Result<(), String> {
    mallctl_action(&format!("arena.{arena}.purge"))
}

#[cfg(feature = "jemalloc-memory")]
fn read_totals() -> Result<MemoryTotalsSnapshot, String> {
    use tikv_jemalloc_ctl::{epoch, stats};
    epoch::mib()
        .and_then(|mib| mib.advance())
        .map_err(|error| format!("jemalloc epoch advance failed: {error}"))?;
    Ok(MemoryTotalsSnapshot {
        available: true,
        allocated_bytes: stats::allocated::mib()
            .and_then(|mib| mib.read())
            .map_err(|error| format!("jemalloc stats.allocated read failed: {error}"))?,
        active_bytes: stats::active::mib()
            .and_then(|mib| mib.read())
            .map_err(|error| format!("jemalloc stats.active read failed: {error}"))?,
        resident_bytes: stats::resident::mib()
            .and_then(|mib| mib.read())
            .map_err(|error| format!("jemalloc stats.resident read failed: {error}"))?,
        retained_bytes: stats::retained::mib()
            .and_then(|mib| mib.read())
            .map_err(|error| format!("jemalloc stats.retained read failed: {error}"))?,
        mapped_bytes: stats::mapped::mib()
            .and_then(|mib| mib.read())
            .map_err(|error| format!("jemalloc stats.mapped read failed: {error}"))?,
        ..MemoryTotalsSnapshot::default()
    })
}

#[cfg(not(feature = "jemalloc-memory"))]
fn read_totals() -> Result<MemoryTotalsSnapshot, String> {
    Ok(MemoryTotalsSnapshot::default())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn read_os_memory(totals: MemoryTotalsSnapshot) -> Result<OsMemorySnapshot, String> {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let process = unsafe { GetCurrentProcess() };
    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    memory.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    if unsafe { GetProcessMemoryInfo(process, &mut memory, memory.cb) } == 0 {
        return Err("GetProcessMemoryInfo failed".to_owned());
    }
    Ok(OsMemorySnapshot {
        available: true,
        working_set_bytes: memory.WorkingSetSize as u64,
        peak_working_set_bytes: memory.PeakWorkingSetSize as u64,
        pagefile_bytes: memory.PagefileUsage as u64,
        peak_pagefile_bytes: memory.PeakPagefileUsage as u64,
        page_fault_count: memory.PageFaultCount,
        working_set_minus_jemalloc_resident_bytes: memory.WorkingSetSize as i64
            - totals.resident_bytes as i64,
        pagefile_minus_jemalloc_active_bytes: memory.PagefileUsage as i64
            - totals.active_bytes as i64,
        ..OsMemorySnapshot::default()
    })
}

#[cfg(not(windows))]
fn read_os_memory(totals: MemoryTotalsSnapshot) -> Result<OsMemorySnapshot, String> {
    Ok(OsMemorySnapshot {
        working_set_minus_jemalloc_resident_bytes: -(totals.resident_bytes as i64),
        pagefile_minus_jemalloc_active_bytes: -(totals.active_bytes as i64),
        ..OsMemorySnapshot::default()
    })
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_write_isize(name: &str, value: isize) -> Result<(), String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::write(name.as_bytes_with_nul(), value) }
        .map_err(|error| format!("jemalloc mallctl write {name:?} failed: {error}"))
}

#[cfg(not(feature = "jemalloc-memory"))]
fn mallctl_write_isize(_name: &str, _value: isize) -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_write_bool(name: &str, value: bool) -> Result<(), String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::write(name.as_bytes_with_nul(), value) }
        .map_err(|error| format!("jemalloc mallctl write {name:?} failed: {error}"))
}

#[cfg(not(feature = "jemalloc-memory"))]
#[allow(dead_code)]
fn mallctl_write_bool(_name: &str, _value: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_write_u32(name: &str, value: u32) -> Result<(), String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::write(name.as_bytes_with_nul(), value) }
        .map_err(|error| format!("jemalloc mallctl write {name:?} failed: {error}"))
}

#[cfg(not(feature = "jemalloc-memory"))]
#[allow(dead_code)]
fn mallctl_write_u32(_name: &str, _value: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_write_usize(name: &str, value: usize) -> Result<(), String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::write(name.as_bytes_with_nul(), value) }
        .map_err(|error| format!("jemalloc mallctl write {name:?} failed: {error}"))
}

#[cfg(not(feature = "jemalloc-memory"))]
#[allow(dead_code)]
fn mallctl_write_usize(_name: &str, _value: usize) -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_read_u32(name: &str) -> Result<u32, String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::read::<u32>(name.as_bytes_with_nul()) }
        .map_err(|error| format!("jemalloc mallctl read {name:?} failed: {error}"))
}

#[cfg(not(feature = "jemalloc-memory"))]
#[allow(dead_code)]
fn mallctl_read_u32(_name: &str) -> Result<u32, String> {
    Ok(0)
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_action(name: &str) -> Result<(), String> {
    use std::ffi::c_char;
    use std::ffi::CString;
    use std::ptr;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    let result = unsafe {
        tikv_jemalloc_sys::mallctl(
            name.as_ptr() as *const c_char,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "jemalloc mallctl action {name:?} failed with errno {result}"
        ))
    }
}

#[cfg(not(feature = "jemalloc-memory"))]
fn mallctl_action(_name: &str) -> Result<(), String> {
    Ok(())
}

fn totals_json(totals: &MemoryTotalsSnapshot) -> String {
    format!(
        "{{\"available\":{},\"allocated_bytes\":{},\"active_bytes\":{},\"resident_bytes\":{},\"retained_bytes\":{},\"mapped_bytes\":{},\"allocated_bytes_per_sec\":{:.3},\"active_growth_bytes\":{},\"resident_growth_bytes\":{},\"retained_growth_bytes\":{}}}",
        totals.available,
        totals.allocated_bytes,
        totals.active_bytes,
        totals.resident_bytes,
        totals.retained_bytes,
        totals.mapped_bytes,
        totals.allocated_bytes_per_sec,
        totals.active_growth_bytes,
        totals.resident_growth_bytes,
        totals.retained_growth_bytes
    )
}

fn os_json(os: &OsMemorySnapshot) -> String {
    format!(
        "{{\"available\":{},\"working_set_bytes\":{},\"peak_working_set_bytes\":{},\"pagefile_bytes\":{},\"peak_pagefile_bytes\":{},\"page_fault_count\":{},\"working_set_growth_bytes\":{},\"pagefile_growth_bytes\":{},\"working_set_minus_jemalloc_resident_bytes\":{},\"pagefile_minus_jemalloc_active_bytes\":{}}}",
        os.available,
        os.working_set_bytes,
        os.peak_working_set_bytes,
        os.pagefile_bytes,
        os.peak_pagefile_bytes,
        os.page_fault_count,
        os.working_set_growth_bytes,
        os.pagefile_growth_bytes,
        os.working_set_minus_jemalloc_resident_bytes,
        os.pagefile_minus_jemalloc_active_bytes
    )
}

fn domain_json(domain: &MemoryDomainSnapshot) -> String {
    format!(
        "{{\"name\":\"{}\",\"arena\":{},\"reuse_critical\":{},\"dirty_decay_ms\":{},\"muzzy_decay_ms\":{},\"bind_count\":{},\"decay_count\":{},\"purge_count\":{},\"growth_events\":{},\"retained_floor_hits\":{},\"zero_growth_streak\":{}}}",
        escape_json(domain.name),
        domain
            .arena
            .map(|arena| arena.to_string())
            .unwrap_or_else(|| "null".to_owned()),
        domain.reuse_critical,
        domain.dirty_decay_ms,
        domain.muzzy_decay_ms,
        domain.bind_count,
        domain.decay_count,
        domain.purge_count,
        domain.growth_events,
        domain.retained_floor_hits,
        domain.zero_growth_streak
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
                escaped.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => escaped.push(character),
        }
    }
    escaped
}
