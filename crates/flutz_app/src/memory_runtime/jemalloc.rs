use super::*;

#[cfg(feature = "jemalloc-memory")]
pub(super) fn configure_global(config: &MemoryRuntimeConfig) -> Result<(), String> {
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

#[cfg(feature = "jemalloc-memory")]
pub(super) fn create_arena() -> Result<usize, String> {
    mallctl_read_u32("arenas.create").map(|arena| arena as usize)
}

#[cfg(feature = "jemalloc-memory")]
pub(super) fn configure_arena(
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
pub(super) fn bind_thread_to_arena(arena: usize) -> Result<(), String> {
    mallctl_write_u32("thread.arena", arena as u32)
}

#[cfg(feature = "jemalloc-memory")]
pub(super) fn decay_arena(arena: usize) -> Result<(), String> {
    mallctl_action(&format!("arena.{arena}.decay"))
}

#[cfg(feature = "jemalloc-memory")]
pub(super) fn purge_arena(arena: usize) -> Result<(), String> {
    mallctl_action(&format!("arena.{arena}.purge"))
}

/// (active, dirty) bytes of one arena; dirty pages are committed and reusable.
#[cfg(feature = "jemalloc-memory")]
pub(super) fn arena_page_bytes(arena: usize) -> Result<(usize, usize), String> {
    use tikv_jemalloc_ctl::epoch;
    epoch::mib()
        .and_then(|mib| mib.advance())
        .map_err(|error| format!("jemalloc epoch advance failed: {error}"))?;
    let page = mallctl_read_usize("arenas.page")?;
    let active = mallctl_read_usize(&format!("stats.arenas.{arena}.pactive"))?;
    let dirty = mallctl_read_usize(&format!("stats.arenas.{arena}.pdirty"))?;
    Ok((active.saturating_mul(page), dirty.saturating_mul(page)))
}

/// Commits `bytes` of pages in `arena` and frees them again, leaving them as dirty
/// extents the arena reuses without OS calls. Returns the bytes actually warmed.
#[cfg(feature = "jemalloc-memory")]
#[allow(unsafe_code)]
pub(super) fn prewarm_arena(arena: usize, bytes: usize) -> Result<usize, String> {
    use tikv_jemalloc_sys::{dallocx, mallocx, MALLOCX_ARENA, MALLOCX_TCACHE_NONE};
    const CHUNK_BYTES: usize = 1 << 20;
    const PAGE_STRIDE: usize = 4096;
    let flags = MALLOCX_ARENA(arena) | MALLOCX_TCACHE_NONE;
    let mut chunks = Vec::with_capacity(bytes.div_ceil(CHUNK_BYTES));
    let mut remaining = bytes;
    let mut warmed = 0usize;
    while remaining > 0 {
        let size = remaining.min(CHUNK_BYTES);
        let ptr = unsafe { mallocx(size, flags) }.cast::<u8>();
        if ptr.is_null() {
            break;
        }
        for offset in (0..size).step_by(PAGE_STRIDE) {
            unsafe { ptr.add(offset).write_volatile(0) };
        }
        chunks.push(ptr);
        warmed = warmed.saturating_add(size);
        remaining -= size;
    }
    for ptr in chunks {
        unsafe { dallocx(ptr.cast(), flags) };
    }
    if warmed < bytes {
        return Err(format!(
            "jemalloc prewarm of arena {arena} stopped at {warmed} of {bytes} bytes"
        ));
    }
    Ok(warmed)
}

#[cfg(feature = "jemalloc-memory")]
pub(super) fn read_totals() -> Result<MemoryTotalsSnapshot, String> {
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

#[cfg(feature = "jemalloc-memory")]
fn mallctl_write_isize(name: &str, value: isize) -> Result<(), String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::write(name.as_bytes_with_nul(), value) }
        .map_err(|error| format!("jemalloc mallctl write {name:?} failed: {error}"))
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_write_bool(name: &str, value: bool) -> Result<(), String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::write(name.as_bytes_with_nul(), value) }
        .map_err(|error| format!("jemalloc mallctl write {name:?} failed: {error}"))
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_write_u32(name: &str, value: u32) -> Result<(), String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::write(name.as_bytes_with_nul(), value) }
        .map_err(|error| format!("jemalloc mallctl write {name:?} failed: {error}"))
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_write_usize(name: &str, value: usize) -> Result<(), String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::write(name.as_bytes_with_nul(), value) }
        .map_err(|error| format!("jemalloc mallctl write {name:?} failed: {error}"))
}

#[cfg(feature = "jemalloc-memory")]
#[allow(unsafe_code)]
fn mallctl_read_usize(name: &str) -> Result<usize, String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::read::<usize>(name.as_bytes_with_nul()) }
        .map_err(|error| format!("jemalloc mallctl read {name:?} failed: {error}"))
}

#[cfg(feature = "jemalloc-memory")]
fn mallctl_read_u32(name: &str) -> Result<u32, String> {
    use std::ffi::CString;
    use tikv_jemalloc_ctl::raw;
    let name = CString::new(name).map_err(|error| error.to_string())?;
    unsafe { raw::read::<u32>(name.as_bytes_with_nul()) }
        .map_err(|error| format!("jemalloc mallctl read {name:?} failed: {error}"))
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
