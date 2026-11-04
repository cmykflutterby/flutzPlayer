use super::*;

#[derive(Debug, Clone)]
pub(super) struct ProcessMetricSampler {
    #[cfg(debug_assertions)]
    last_sample_at: Instant,
    #[cfg(debug_assertions)]
    last_cpu_time_ms: Option<u128>,
    #[cfg(debug_assertions)]
    last_read_bytes: Option<u64>,
    #[cfg(debug_assertions)]
    last_write_bytes: Option<u64>,
    #[cfg(debug_assertions)]
    last_page_fault_count: Option<u32>,
}

impl ProcessMetricSampler {
    pub(super) fn new(
        #[cfg_attr(not(debug_assertions), allow(unused_variables))] now: Instant,
    ) -> Self {
        #[cfg(not(debug_assertions))]
        {
            Self {}
        }
        #[cfg(debug_assertions)]
        {
            Self {
                last_sample_at: now,
                last_cpu_time_ms: None,
                last_read_bytes: None,
                last_write_bytes: None,
                last_page_fault_count: None,
            }
        }
    }

    pub(super) fn sample(&mut self) -> Option<PerfProcessSnapshot> {
        #[cfg(not(debug_assertions))]
        {
            None
        }
        #[cfg(debug_assertions)]
        {
            let now = Instant::now();
            let raw = process_resource_snapshot()?;
            let elapsed_ms = now.duration_since(self.last_sample_at).as_millis();
            let cpu_percent = self
                .last_cpu_time_ms
                .and_then(|previous| raw.cpu_time_ms.checked_sub(previous))
                .and_then(|delta| process_cpu_percent(delta, elapsed_ms));

            let read_delta_bytes = self
                .last_read_bytes
                .map(|previous| raw.read_bytes.saturating_sub(previous));
            let write_delta_bytes = self
                .last_write_bytes
                .map(|previous| raw.write_bytes.saturating_sub(previous));
            let page_fault_delta = self
                .last_page_fault_count
                .map(|previous| raw.page_fault_count.saturating_sub(previous));
            let read_bytes_per_sec = bytes_per_second(read_delta_bytes, elapsed_ms);
            let write_bytes_per_sec = bytes_per_second(write_delta_bytes, elapsed_ms);
            let page_faults_per_sec = count_per_second(page_fault_delta, elapsed_ms);

            self.last_sample_at = now;
            self.last_cpu_time_ms = Some(raw.cpu_time_ms);
            self.last_read_bytes = Some(raw.read_bytes);
            self.last_write_bytes = Some(raw.write_bytes);
            self.last_page_fault_count = Some(raw.page_fault_count);

            Some(PerfProcessSnapshot {
                cpu_percent,
                cpu_time_ms: raw.cpu_time_ms,
                working_set_bytes: raw.working_set_bytes,
                peak_working_set_bytes: raw.peak_working_set_bytes,
                pagefile_bytes: raw.pagefile_bytes,
                peak_pagefile_bytes: raw.peak_pagefile_bytes,
                page_fault_count: raw.page_fault_count,
                page_fault_delta,
                page_faults_per_sec,
                thread_count: raw.thread_count,
                handle_count: raw.handle_count,
                read_bytes: raw.read_bytes,
                write_bytes: raw.write_bytes,
                read_delta_bytes,
                write_delta_bytes,
                read_bytes_per_sec,
                write_bytes_per_sec,
                native_heaps: raw.native_heaps,
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct PerfProcessSnapshot {
    pub cpu_percent: Option<f64>,
    pub cpu_time_ms: u128,
    pub working_set_bytes: u64,
    pub peak_working_set_bytes: u64,
    pub pagefile_bytes: u64,
    pub peak_pagefile_bytes: u64,
    pub page_fault_count: u32,
    pub page_fault_delta: Option<u32>,
    pub page_faults_per_sec: Option<f64>,
    pub thread_count: Option<u32>,
    pub handle_count: Option<u32>,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_delta_bytes: Option<u64>,
    pub write_delta_bytes: Option<u64>,
    pub read_bytes_per_sec: Option<f64>,
    pub write_bytes_per_sec: Option<f64>,
    pub native_heaps: Option<NativeHeapSnapshot>,
}

impl PerfProcessSnapshot {
    pub(super) fn to_json(&self) -> String {
        let mut json = format!(
            "{{\"cpu_time_ms\":{},\"working_set_bytes\":{},\"peak_working_set_bytes\":{},\"pagefile_bytes\":{},\"peak_pagefile_bytes\":{},\"page_fault_count\":{},\"read_bytes\":{},\"write_bytes\":{}",
            self.cpu_time_ms,
            self.working_set_bytes,
            self.peak_working_set_bytes,
            self.pagefile_bytes,
            self.peak_pagefile_bytes,
            self.page_fault_count,
            self.read_bytes,
            self.write_bytes
        );
        push_json_f64_opt(&mut json, "cpu_percent", self.cpu_percent);
        push_json_u64_opt(&mut json, "read_delta_bytes", self.read_delta_bytes);
        push_json_u64_opt(&mut json, "write_delta_bytes", self.write_delta_bytes);
        push_json_u32_opt(&mut json, "page_fault_delta", self.page_fault_delta);
        push_json_f64_opt(&mut json, "read_bytes_per_sec", self.read_bytes_per_sec);
        push_json_f64_opt(&mut json, "write_bytes_per_sec", self.write_bytes_per_sec);
        push_json_f64_opt(&mut json, "page_faults_per_sec", self.page_faults_per_sec);
        push_json_u32_opt(&mut json, "thread_count", self.thread_count);
        push_json_u32_opt(&mut json, "handle_count", self.handle_count);
        if let Some(native_heaps) = &self.native_heaps {
            json.push_str(",\"native_heaps\":");
            json.push_str(&native_heaps.to_json());
        }
        json.push('}');
        json
    }
}

#[derive(Debug, Clone, Default)]
pub struct NativeHeapSnapshot {
    pub heap_count: u32,
    pub walked_heaps: u32,
    pub failed_heaps: u32,
    pub busy_blocks: u64,
    pub busy_bytes: u64,
    pub free_blocks: u64,
    pub free_bytes: u64,
    pub region_count: u64,
    pub committed_region_bytes: u64,
    pub uncommitted_region_bytes: u64,
    pub uncommitted_ranges: u64,
    pub uncommitted_range_bytes: u64,
    pub block_overhead_bytes: u64,
}

impl NativeHeapSnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"heap_count\":{},\"walked_heaps\":{},\"failed_heaps\":{},\"busy_blocks\":{},\"busy_bytes\":{},\"free_blocks\":{},\"free_bytes\":{},\"region_count\":{},\"committed_region_bytes\":{},\"uncommitted_region_bytes\":{},\"uncommitted_ranges\":{},\"uncommitted_range_bytes\":{},\"block_overhead_bytes\":{}}}",
            self.heap_count,
            self.walked_heaps,
            self.failed_heaps,
            self.busy_blocks,
            self.busy_bytes,
            self.free_blocks,
            self.free_bytes,
            self.region_count,
            self.committed_region_bytes,
            self.uncommitted_region_bytes,
            self.uncommitted_ranges,
            self.uncommitted_range_bytes,
            self.block_overhead_bytes
        )
    }
}

#[derive(Debug, Clone)]
#[cfg(debug_assertions)]
struct RawProcessResourceSnapshot {
    cpu_time_ms: u128,
    working_set_bytes: u64,
    peak_working_set_bytes: u64,
    pagefile_bytes: u64,
    peak_pagefile_bytes: u64,
    page_fault_count: u32,
    thread_count: Option<u32>,
    handle_count: Option<u32>,
    read_bytes: u64,
    write_bytes: u64,
    native_heaps: Option<NativeHeapSnapshot>,
}

#[cfg(debug_assertions)]
fn process_cpu_percent(delta_cpu_ms: u128, elapsed_ms: u128) -> Option<f64> {
    if elapsed_ms == 0 {
        return None;
    }
    let parallelism = std::thread::available_parallelism()
        .map(|value| value.get() as f64)
        .unwrap_or(1.0);
    Some((delta_cpu_ms as f64 / (elapsed_ms as f64 * parallelism)) * 100.0)
}

#[cfg(debug_assertions)]
fn bytes_per_second(delta_bytes: Option<u64>, elapsed_ms: u128) -> Option<f64> {
    if elapsed_ms == 0 {
        return None;
    }
    delta_bytes.map(|bytes| bytes as f64 * 1_000.0 / elapsed_ms as f64)
}

#[cfg(debug_assertions)]
fn count_per_second(delta: Option<u32>, elapsed_ms: u128) -> Option<f64> {
    if elapsed_ms == 0 {
        return None;
    }
    delta.map(|count| count as f64 * 1_000.0 / elapsed_ms as f64)
}

#[cfg(all(debug_assertions, windows))]
fn process_resource_snapshot() -> Option<RawProcessResourceSnapshot> {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS,
    };

    let process = unsafe { GetCurrentProcess() };

    let mut creation_time: FILETIME = unsafe { zeroed() };
    let mut exit_time: FILETIME = unsafe { zeroed() };
    let mut kernel_time: FILETIME = unsafe { zeroed() };
    let mut user_time: FILETIME = unsafe { zeroed() };
    if unsafe {
        GetProcessTimes(
            process,
            &mut creation_time,
            &mut exit_time,
            &mut kernel_time,
            &mut user_time,
        )
    } == 0
    {
        return None;
    }

    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    memory.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    if unsafe { GetProcessMemoryInfo(process, &mut memory, memory.cb) } == 0 {
        return None;
    }

    let mut io_counters: IO_COUNTERS = unsafe { zeroed() };
    if unsafe { GetProcessIoCounters(process, &mut io_counters) } == 0 {
        return None;
    }

    Some(RawProcessResourceSnapshot {
        cpu_time_ms: (filetime_100ns(&kernel_time) + filetime_100ns(&user_time)) / 10_000,
        working_set_bytes: memory.WorkingSetSize as u64,
        peak_working_set_bytes: memory.PeakWorkingSetSize as u64,
        pagefile_bytes: memory.PagefileUsage as u64,
        peak_pagefile_bytes: memory.PeakPagefileUsage as u64,
        page_fault_count: memory.PageFaultCount,
        thread_count: current_process_thread_count(),
        handle_count: current_process_handle_count(process),
        read_bytes: io_counters.ReadTransferCount,
        write_bytes: io_counters.WriteTransferCount,
        native_heaps: current_process_heap_snapshot(),
    })
}

#[cfg(all(debug_assertions, windows))]
fn current_process_heap_snapshot() -> Option<NativeHeapSnapshot> {
    use std::{mem::zeroed, ptr};

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Memory::{
        GetProcessHeaps, HeapLock, HeapUnlock, HeapWalk, PROCESS_HEAP_ENTRY,
    };

    const PROCESS_HEAP_REGION: u32 = 1;
    const PROCESS_HEAP_UNCOMMITTED_RANGE: u32 = 2;
    const PROCESS_HEAP_ENTRY_BUSY: u32 = 4;

    let heap_count = unsafe { GetProcessHeaps(0, ptr::null_mut()) };
    if heap_count == 0 {
        return None;
    }
    let mut heaps = vec![0 as HANDLE; heap_count as usize];
    let copied = unsafe { GetProcessHeaps(heap_count, heaps.as_mut_ptr()) };
    if copied == 0 {
        return None;
    }
    heaps.truncate(copied as usize);

    let mut snapshot = NativeHeapSnapshot {
        heap_count: copied,
        ..NativeHeapSnapshot::default()
    };

    for heap in heaps {
        if unsafe { HeapLock(heap) } == 0 {
            snapshot.failed_heaps = snapshot.failed_heaps.saturating_add(1);
            continue;
        }
        snapshot.walked_heaps = snapshot.walked_heaps.saturating_add(1);
        let mut entry: PROCESS_HEAP_ENTRY = unsafe { zeroed() };
        loop {
            if unsafe { HeapWalk(heap, &mut entry) } == 0 {
                break;
            }
            let flags = entry.wFlags as u32;
            if flags & PROCESS_HEAP_ENTRY_BUSY != 0 {
                snapshot.busy_blocks = snapshot.busy_blocks.saturating_add(1);
                snapshot.busy_bytes = snapshot.busy_bytes.saturating_add(entry.cbData as u64);
                snapshot.block_overhead_bytes = snapshot
                    .block_overhead_bytes
                    .saturating_add(entry.cbOverhead as u64);
            } else if flags & PROCESS_HEAP_REGION != 0 {
                snapshot.region_count = snapshot.region_count.saturating_add(1);
                let region = unsafe { entry.Anonymous.Region };
                snapshot.committed_region_bytes = snapshot
                    .committed_region_bytes
                    .saturating_add(region.dwCommittedSize as u64);
                snapshot.uncommitted_region_bytes = snapshot
                    .uncommitted_region_bytes
                    .saturating_add(region.dwUnCommittedSize as u64);
            } else if flags & PROCESS_HEAP_UNCOMMITTED_RANGE != 0 {
                snapshot.uncommitted_ranges = snapshot.uncommitted_ranges.saturating_add(1);
                snapshot.uncommitted_range_bytes = snapshot
                    .uncommitted_range_bytes
                    .saturating_add(entry.cbData as u64);
            } else {
                snapshot.free_blocks = snapshot.free_blocks.saturating_add(1);
                snapshot.free_bytes = snapshot.free_bytes.saturating_add(entry.cbData as u64);
            }
        }
        unsafe {
            HeapUnlock(heap);
        }
    }

    Some(snapshot)
}

#[cfg(all(debug_assertions, windows))]
fn current_process_handle_count(process: windows_sys::Win32::Foundation::HANDLE) -> Option<u32> {
    use windows_sys::Win32::System::Threading::GetProcessHandleCount;

    let mut count = 0u32;
    if unsafe { GetProcessHandleCount(process, &mut count) } == 0 {
        None
    } else {
        Some(count)
    }
}

#[cfg(all(debug_assertions, windows))]
fn current_process_thread_count() -> Option<u32> {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }

    let current_pid = unsafe { GetCurrentProcessId() };
    let mut entry: THREADENTRY32 = unsafe { zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    let mut count = 0u32;
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == current_pid {
            count = count.saturating_add(1);
        }
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe {
        CloseHandle(snapshot);
    }
    Some(count)
}

#[cfg(all(debug_assertions, not(windows)))]
fn process_resource_snapshot() -> Option<RawProcessResourceSnapshot> {
    None
}

#[cfg(all(debug_assertions, windows))]
fn filetime_100ns(filetime: &windows_sys::Win32::Foundation::FILETIME) -> u128 {
    ((filetime.dwHighDateTime as u128) << 32) | filetime.dwLowDateTime as u128
}
