#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(feature = "jemalloc-memory")]
#[global_allocator]
static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/*
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: flutz_app::allocation_trace::TracingAllocator =
    flutz_app::allocation_trace::TracingAllocator;
*/

#[cfg(all(debug_assertions, not(feature = "jemalloc-memory")))]
#[global_allocator]
static ALLOCATOR: flutz_app::allocation_trace::TracingAllocator =
    flutz_app::allocation_trace::TracingAllocator;

fn main() {
    if let Err(error) = flutz_app::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
