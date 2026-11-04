use super::*;

pub(super) const DECAY_RATE_LIMIT: Duration = Duration::from_millis(250);
pub(super) const PURGE_RATE_LIMIT: Duration = Duration::from_secs(2);

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
    pub(super) fn from_env(debug_memory: bool) -> Self {
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

    pub(super) fn summary(&self) -> String {
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

impl fmt::Display for MemoryDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

pub(super) fn domain_from_index(index: usize) -> Option<MemoryDomain> {
    MemoryDomain::ALL
        .iter()
        .copied()
        .find(|domain| *domain as usize == index)
}

pub(super) fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

pub(super) fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

pub(super) fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

pub(super) fn env_isize(name: &str, default: isize) -> isize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

pub(super) fn allocator_name() -> &'static str {
    #[cfg(feature = "jemalloc-memory")]
    {
        "jemalloc"
    }
    #[cfg(not(feature = "jemalloc-memory"))]
    {
        "system"
    }
}
