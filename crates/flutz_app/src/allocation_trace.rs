#![allow(unsafe_code)]

#[cfg(debug_assertions)]
use std::cell::Cell;

#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(debug_assertions)]
static TRACE_SUSPENDED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum AllocationScope {
    Unscoped = 0,
    AppUpdate = 1,
    PerfTrace = 2,
    FileLoad = 3,
    PlaybackLoad = 4,
    AudioCallback = 5,
    PlaybackRender = 6,
    RenderStems = 7,
    RenderMixer = 8,
    RenderSnapshot = 9,
    Ui = 10,
}

impl AllocationScope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unscoped => "unscoped",
            Self::AppUpdate => "app.update",
            Self::PerfTrace => "perf.trace",
            Self::FileLoad => "file.load",
            Self::PlaybackLoad => "playback.load",
            Self::AudioCallback => "audio.callback",
            Self::PlaybackRender => "playback.render",
            Self::RenderStems => "render.stems",
            Self::RenderMixer => "render.mixer",
            Self::RenderSnapshot => "render.snapshot",
            Self::Ui => "ui",
        }
    }

    #[cfg(debug_assertions)]
    fn from_index(index: usize) -> Self {
        match index {
            1 => Self::AppUpdate,
            2 => Self::PerfTrace,
            3 => Self::FileLoad,
            4 => Self::PlaybackLoad,
            5 => Self::AudioCallback,
            6 => Self::PlaybackRender,
            7 => Self::RenderStems,
            8 => Self::RenderMixer,
            9 => Self::RenderSnapshot,
            10 => Self::Ui,
            _ => Self::Unscoped,
        }
    }
}

#[cfg(debug_assertions)]
const SCOPE_COUNT: usize = 11;

#[cfg(debug_assertions)]
thread_local! {
    static CURRENT_SCOPE: Cell<usize> = const { Cell::new(AllocationScope::Unscoped as usize) };
}

pub struct AllocationScopeGuard {
    #[cfg(debug_assertions)]
    previous: usize,
    _memory_guard: Option<crate::memory_runtime::MemoryThreadGuard>,
}

impl AllocationScopeGuard {
    pub fn enter(scope: AllocationScope) -> Self {
        #[cfg(not(debug_assertions))]
        {
            let _ = scope;
            Self {
                _memory_guard: None,
            }
        }
        #[cfg(debug_assertions)]
        {
            let previous = CURRENT_SCOPE
                .try_with(|current| {
                    let previous = current.get();
                    current.set(scope as usize);
                    previous
                })
                .unwrap_or(AllocationScope::Unscoped as usize);
            let memory_guard =
                memory_domain_for_scope(scope).map(crate::memory_runtime::bind_current_thread);
            Self {
                previous,
                _memory_guard: memory_guard,
            }
        }
    }
}

fn memory_domain_for_scope(scope: AllocationScope) -> Option<crate::memory_runtime::MemoryDomain> {
    use crate::memory_runtime::MemoryDomain;
    match scope {
        AllocationScope::Unscoped => None,
        AllocationScope::AppUpdate => Some(MemoryDomain::App),
        AllocationScope::PerfTrace => Some(MemoryDomain::PerfTrace),
        AllocationScope::FileLoad => Some(MemoryDomain::FileLoad),
        AllocationScope::PlaybackLoad => Some(MemoryDomain::PlaybackLoad),
        AllocationScope::AudioCallback => Some(MemoryDomain::AudioCallback),
        AllocationScope::PlaybackRender => Some(MemoryDomain::PlaybackRender),
        AllocationScope::RenderStems => Some(MemoryDomain::RenderStems),
        AllocationScope::RenderMixer => Some(MemoryDomain::RenderMixer),
        AllocationScope::RenderSnapshot => Some(MemoryDomain::RenderSnapshot),
        AllocationScope::Ui => Some(MemoryDomain::Ui),
    }
}

impl Drop for AllocationScopeGuard {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        let _ = CURRENT_SCOPE.try_with(|current| current.set(self.previous));
    }
}

struct TraceSuspendGuard {
    #[cfg(debug_assertions)]
    previous: bool,
}

impl TraceSuspendGuard {
    fn enter() -> Self {
        #[cfg(debug_assertions)]
        let previous = TRACE_SUSPENDED.swap(true, Ordering::Acquire);
        #[cfg(not(debug_assertions))]
        {
            Self {}
        }
        #[cfg(debug_assertions)]
        {
            Self { previous }
        }
    }
}

impl Drop for TraceSuspendGuard {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        TRACE_SUSPENDED.store(self.previous, Ordering::Release);
    }
}

#[derive(Debug, Clone, Default)]
pub struct AllocationTraceSnapshot {
    pub alloc_calls: u64,
    pub dealloc_calls: u64,
    pub realloc_calls: u64,
    pub alloc_bytes: u64,
    pub dealloc_bytes: u64,
    pub realloc_in_bytes: u64,
    pub realloc_out_bytes: u64,
    pub live_bytes: i64,
    pub peak_live_bytes: u64,
    pub outstanding_allocations: i64,
    pub scopes: Vec<AllocationScopeSnapshot>,
    pub stack_buckets: Vec<AllocationStackBucketSnapshot>,
}

#[derive(Debug, Clone, Default)]
pub struct AllocationScopeSnapshot {
    pub label: &'static str,
    pub alloc_calls: u64,
    pub dealloc_calls: u64,
    pub realloc_calls: u64,
    pub alloc_bytes: u64,
    pub dealloc_bytes: u64,
    pub realloc_in_bytes: u64,
    pub realloc_out_bytes: u64,
    pub net_bytes: i64,
    pub peak_net_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct AllocationStackBucketSnapshot {
    pub hash: u64,
    pub scope: &'static str,
    pub sample_calls: u64,
    pub sample_bytes: u64,
    pub max_sample_bytes: u64,
}

pub fn snapshot() -> AllocationTraceSnapshot {
    let _suspend = TraceSuspendGuard::enter();
    snapshot_inner()
}

#[cfg(debug_assertions)]
mod debug_impl {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        ffi::c_void,
        ptr,
        sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering},
    };

    use super::{
        AllocationScope, AllocationScopeSnapshot, AllocationStackBucketSnapshot,
        AllocationTraceSnapshot, CURRENT_SCOPE, SCOPE_COUNT, TRACE_SUSPENDED,
    };

    const STACK_BUCKET_COUNT: usize = 64;
    const STACK_FRAMES: usize = 16;
    const LARGE_ALLOCATION_SAMPLE_BYTES: usize = 4096;
    const PERIODIC_SAMPLE_MASK: u64 = 1023;

    pub struct TracingAllocator;

    unsafe impl GlobalAlloc for TracingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                record_alloc(layout.size());
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            record_dealloc(layout.size());
            unsafe { System.dealloc(pointer, layout) };
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
            if !new_pointer.is_null() {
                record_realloc(layout.size(), new_size);
            }
            new_pointer
        }
    }

    #[derive(Debug)]
    struct ScopeStats {
        alloc_calls: AtomicU64,
        dealloc_calls: AtomicU64,
        realloc_calls: AtomicU64,
        alloc_bytes: AtomicU64,
        dealloc_bytes: AtomicU64,
        realloc_in_bytes: AtomicU64,
        realloc_out_bytes: AtomicU64,
        net_bytes: AtomicI64,
        peak_net_bytes: AtomicU64,
    }

    impl ScopeStats {
        const fn new() -> Self {
            Self {
                alloc_calls: AtomicU64::new(0),
                dealloc_calls: AtomicU64::new(0),
                realloc_calls: AtomicU64::new(0),
                alloc_bytes: AtomicU64::new(0),
                dealloc_bytes: AtomicU64::new(0),
                realloc_in_bytes: AtomicU64::new(0),
                realloc_out_bytes: AtomicU64::new(0),
                net_bytes: AtomicI64::new(0),
                peak_net_bytes: AtomicU64::new(0),
            }
        }
    }

    #[derive(Debug)]
    struct StackBucketStats {
        hash: AtomicU64,
        scope: AtomicUsize,
        sample_calls: AtomicU64,
        sample_bytes: AtomicU64,
        max_sample_bytes: AtomicU64,
    }

    impl StackBucketStats {
        const fn new() -> Self {
            Self {
                hash: AtomicU64::new(0),
                scope: AtomicUsize::new(0),
                sample_calls: AtomicU64::new(0),
                sample_bytes: AtomicU64::new(0),
                max_sample_bytes: AtomicU64::new(0),
            }
        }
    }

    static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
    static DEALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
    static REALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
    static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
    static DEALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
    static REALLOC_IN_BYTES: AtomicU64 = AtomicU64::new(0);
    static REALLOC_OUT_BYTES: AtomicU64 = AtomicU64::new(0);
    static LIVE_BYTES: AtomicI64 = AtomicI64::new(0);
    static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
    static OUTSTANDING_ALLOCATIONS: AtomicI64 = AtomicI64::new(0);
    static ALLOCATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static SCOPE_STATS: [ScopeStats; SCOPE_COUNT] = [const { ScopeStats::new() }; SCOPE_COUNT];
    static STACK_BUCKETS: [StackBucketStats; STACK_BUCKET_COUNT] =
        [const { StackBucketStats::new() }; STACK_BUCKET_COUNT];

    pub fn snapshot_inner() -> AllocationTraceSnapshot {
        let scopes = SCOPE_STATS
            .iter()
            .enumerate()
            .map(|(index, stats)| AllocationScopeSnapshot {
                label: AllocationScope::from_index(index).label(),
                alloc_calls: stats.alloc_calls.load(Ordering::Relaxed),
                dealloc_calls: stats.dealloc_calls.load(Ordering::Relaxed),
                realloc_calls: stats.realloc_calls.load(Ordering::Relaxed),
                alloc_bytes: stats.alloc_bytes.load(Ordering::Relaxed),
                dealloc_bytes: stats.dealloc_bytes.load(Ordering::Relaxed),
                realloc_in_bytes: stats.realloc_in_bytes.load(Ordering::Relaxed),
                realloc_out_bytes: stats.realloc_out_bytes.load(Ordering::Relaxed),
                net_bytes: stats.net_bytes.load(Ordering::Relaxed),
                peak_net_bytes: stats.peak_net_bytes.load(Ordering::Relaxed),
            })
            .collect();
        let mut stack_buckets = STACK_BUCKETS
            .iter()
            .filter_map(|bucket| {
                let hash = bucket.hash.load(Ordering::Relaxed);
                (hash != 0).then(|| AllocationStackBucketSnapshot {
                    hash,
                    scope: AllocationScope::from_index(bucket.scope.load(Ordering::Relaxed))
                        .label(),
                    sample_calls: bucket.sample_calls.load(Ordering::Relaxed),
                    sample_bytes: bucket.sample_bytes.load(Ordering::Relaxed),
                    max_sample_bytes: bucket.max_sample_bytes.load(Ordering::Relaxed),
                })
            })
            .collect::<Vec<_>>();
        stack_buckets.sort_by(|left, right| right.sample_bytes.cmp(&left.sample_bytes));
        stack_buckets.truncate(16);

        AllocationTraceSnapshot {
            alloc_calls: ALLOC_CALLS.load(Ordering::Relaxed),
            dealloc_calls: DEALLOC_CALLS.load(Ordering::Relaxed),
            realloc_calls: REALLOC_CALLS.load(Ordering::Relaxed),
            alloc_bytes: ALLOC_BYTES.load(Ordering::Relaxed),
            dealloc_bytes: DEALLOC_BYTES.load(Ordering::Relaxed),
            realloc_in_bytes: REALLOC_IN_BYTES.load(Ordering::Relaxed),
            realloc_out_bytes: REALLOC_OUT_BYTES.load(Ordering::Relaxed),
            live_bytes: LIVE_BYTES.load(Ordering::Relaxed),
            peak_live_bytes: PEAK_LIVE_BYTES.load(Ordering::Relaxed),
            outstanding_allocations: OUTSTANDING_ALLOCATIONS.load(Ordering::Relaxed),
            scopes,
            stack_buckets,
        }
    }

    fn record_alloc(size: usize) {
        record_with_guard(|| {
            let scope = current_scope_index();
            let size_u64 = size as u64;
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(size_u64, Ordering::Relaxed);
            OUTSTANDING_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            let live = LIVE_BYTES.fetch_add(size as i64, Ordering::Relaxed) + size as i64;
            update_peak(&PEAK_LIVE_BYTES, live.max(0) as u64);
            let stats = &SCOPE_STATS[scope];
            stats.alloc_calls.fetch_add(1, Ordering::Relaxed);
            stats.alloc_bytes.fetch_add(size_u64, Ordering::Relaxed);
            let net = stats.net_bytes.fetch_add(size as i64, Ordering::Relaxed) + size as i64;
            update_peak(&stats.peak_net_bytes, net.max(0) as u64);

            let sequence = ALLOCATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            if size >= LARGE_ALLOCATION_SAMPLE_BYTES || (sequence & PERIODIC_SAMPLE_MASK) == 0 {
                record_stack_sample(scope, size_u64);
            }
        });
    }

    fn record_dealloc(size: usize) {
        record_with_guard(|| {
            let scope = current_scope_index();
            let size_u64 = size as u64;
            DEALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            DEALLOC_BYTES.fetch_add(size_u64, Ordering::Relaxed);
            OUTSTANDING_ALLOCATIONS.fetch_sub(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_sub(size as i64, Ordering::Relaxed);
            let stats = &SCOPE_STATS[scope];
            stats.dealloc_calls.fetch_add(1, Ordering::Relaxed);
            stats.dealloc_bytes.fetch_add(size_u64, Ordering::Relaxed);
            stats.net_bytes.fetch_sub(size as i64, Ordering::Relaxed);
        });
    }

    fn record_realloc(old_size: usize, new_size: usize) {
        record_with_guard(|| {
            let scope = current_scope_index();
            let old_size_u64 = old_size as u64;
            let new_size_u64 = new_size as u64;
            REALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            REALLOC_IN_BYTES.fetch_add(old_size_u64, Ordering::Relaxed);
            REALLOC_OUT_BYTES.fetch_add(new_size_u64, Ordering::Relaxed);
            let delta = new_size as i64 - old_size as i64;
            let live = LIVE_BYTES.fetch_add(delta, Ordering::Relaxed) + delta;
            update_peak(&PEAK_LIVE_BYTES, live.max(0) as u64);
            let stats = &SCOPE_STATS[scope];
            stats.realloc_calls.fetch_add(1, Ordering::Relaxed);
            stats
                .realloc_in_bytes
                .fetch_add(old_size_u64, Ordering::Relaxed);
            stats
                .realloc_out_bytes
                .fetch_add(new_size_u64, Ordering::Relaxed);
            let net = stats.net_bytes.fetch_add(delta, Ordering::Relaxed) + delta;
            update_peak(&stats.peak_net_bytes, net.max(0) as u64);

            if new_size >= LARGE_ALLOCATION_SAMPLE_BYTES {
                record_stack_sample(scope, new_size_u64);
            }
        });
    }

    fn record_with_guard(record: impl FnOnce()) {
        if TRACE_SUSPENDED
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        record();
        TRACE_SUSPENDED.store(false, Ordering::Release);
    }

    fn current_scope_index() -> usize {
        let scope = CURRENT_SCOPE
            .try_with(|current| current.get())
            .unwrap_or(AllocationScope::Unscoped as usize);
        scope.min(SCOPE_COUNT - 1)
    }

    fn update_peak(peak: &AtomicU64, value: u64) {
        let mut current = peak.load(Ordering::Relaxed);
        while value > current {
            match peak.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(next) => current = next,
            }
        }
    }

    fn record_stack_sample(scope: usize, bytes: u64) {
        let hash = capture_stack_hash(scope as u64);
        if hash == 0 {
            return;
        }
        let mut index = hash as usize % STACK_BUCKET_COUNT;
        for _ in 0..STACK_BUCKET_COUNT {
            let bucket = &STACK_BUCKETS[index];
            let current = bucket.hash.load(Ordering::Relaxed);
            if current == hash
                || (current == 0
                    && bucket
                        .hash
                        .compare_exchange(0, hash, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok())
            {
                bucket.scope.store(scope, Ordering::Relaxed);
                bucket.sample_calls.fetch_add(1, Ordering::Relaxed);
                bucket.sample_bytes.fetch_add(bytes, Ordering::Relaxed);
                update_peak(&bucket.max_sample_bytes, bytes);
                return;
            }
            index = (index + 1) % STACK_BUCKET_COUNT;
        }
    }

    #[cfg(windows)]
    fn capture_stack_hash(seed: u64) -> u64 {
        use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;

        let mut frames = [ptr::null_mut::<c_void>(); STACK_FRAMES];
        let captured = unsafe {
            RtlCaptureStackBackTrace(4, STACK_FRAMES as u32, frames.as_mut_ptr(), ptr::null_mut())
        } as usize;
        if captured == 0 {
            return 0;
        }
        let mut hash = 0xcbf29ce484222325u64 ^ seed;
        for frame in frames.iter().take(captured) {
            hash ^= *frame as usize as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash.max(1)
    }

    #[cfg(not(windows))]
    fn capture_stack_hash(seed: u64) -> u64 {
        seed.max(1)
    }
}

#[cfg(debug_assertions)]
pub use debug_impl::TracingAllocator;

#[cfg(debug_assertions)]
fn snapshot_inner() -> AllocationTraceSnapshot {
    debug_impl::snapshot_inner()
}

#[cfg(not(debug_assertions))]
fn snapshot_inner() -> AllocationTraceSnapshot {
    AllocationTraceSnapshot::default()
}
