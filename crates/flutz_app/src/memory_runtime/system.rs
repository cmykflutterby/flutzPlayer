use super::*;

#[cfg(not(feature = "jemalloc-memory"))]
pub(super) fn configure_global(_config: &MemoryRuntimeConfig) -> Result<(), String> {
    Ok(())
}

#[cfg(not(feature = "jemalloc-memory"))]
pub(super) fn create_arena() -> Result<usize, String> {
    Err("jemalloc-memory feature is disabled".to_owned())
}

#[cfg(not(feature = "jemalloc-memory"))]
pub(super) fn configure_arena(
    _arena: usize,
    _dirty_decay_ms: isize,
    _muzzy_decay_ms: isize,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(feature = "jemalloc-memory"))]
pub(super) fn bind_thread_to_arena(_arena: usize) -> Result<(), String> {
    Ok(())
}

#[cfg(not(feature = "jemalloc-memory"))]
pub(super) fn decay_arena(_arena: usize) -> Result<(), String> {
    Ok(())
}

#[cfg(not(feature = "jemalloc-memory"))]
pub(super) fn purge_arena(_arena: usize) -> Result<(), String> {
    Ok(())
}

#[cfg(not(feature = "jemalloc-memory"))]
pub(super) fn arena_page_bytes(_arena: usize) -> Result<(usize, usize), String> {
    Ok((0, 0))
}

#[cfg(not(feature = "jemalloc-memory"))]
pub(super) fn prewarm_arena(_arena: usize, _bytes: usize) -> Result<usize, String> {
    Ok(0)
}

#[cfg(not(feature = "jemalloc-memory"))]
pub(super) fn read_totals() -> Result<MemoryTotalsSnapshot, String> {
    Ok(MemoryTotalsSnapshot::default())
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub(super) fn read_os_memory(totals: MemoryTotalsSnapshot) -> Result<OsMemorySnapshot, String> {
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
pub(super) fn read_os_memory(totals: MemoryTotalsSnapshot) -> Result<OsMemorySnapshot, String> {
    Ok(OsMemorySnapshot {
        working_set_minus_jemalloc_resident_bytes: -(totals.resident_bytes as i64),
        pagefile_minus_jemalloc_active_bytes: -(totals.active_bytes as i64),
        ..OsMemorySnapshot::default()
    })
}
