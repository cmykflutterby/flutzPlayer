use flutz_app::memory_runtime::{self, MemoryDomain};

#[cfg(feature = "jemalloc-memory")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    let summary_only = std::env::args().any(|arg| arg == "--summary");
    memory_runtime::initialize_from_env(true);

    for domain in MemoryDomain::ALL {
        let _guard = memory_runtime::bind_current_thread(domain);
        let mut buffers = Vec::with_capacity(16);
        for index in 0..16usize {
            buffers.push(vec![index as u8; 4096]);
        }
    }

    memory_runtime::decay_all_idle_domains();
    memory_runtime::purge_domain(MemoryDomain::FileLoad);
    memory_runtime::purge_domain(MemoryDomain::PlaybackLoad);
    let snapshot = memory_runtime::snapshot_after_maintenance(false);

    println!("memory_runtime_probe: ok");
    println!("allocator={}", snapshot.allocator);
    println!("enabled={}", snapshot.enabled);
    println!("stats_available={}", snapshot.totals.available);
    println!("allocated_bytes={}", snapshot.totals.allocated_bytes);
    println!("active_bytes={}", snapshot.totals.active_bytes);
    println!("resident_bytes={}", snapshot.totals.resident_bytes);
    println!("retained_bytes={}", snapshot.totals.retained_bytes);
    println!("domain_count={}", snapshot.domains.len());
    for domain in &snapshot.domains {
        println!(
            "domain.{}.arena={}",
            domain.name,
            domain
                .arena
                .map(|arena| arena.to_string())
                .unwrap_or_else(|| "none".to_owned())
        );
        println!("domain.{}.bind_count={}", domain.name, domain.bind_count);
        println!("domain.{}.decay_count={}", domain.name, domain.decay_count);
        println!("domain.{}.purge_count={}", domain.name, domain.purge_count);
    }
    if !snapshot.errors.is_empty() {
        println!("errors={}", snapshot.errors.join(" | "));
    }
    if !summary_only {
        println!("json={}", snapshot.to_json());
    }
}
