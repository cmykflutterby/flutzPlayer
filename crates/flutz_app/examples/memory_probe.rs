use flutz_app::memory_runtime::{self, MemoryDomain};

fn main() {
    memory_runtime::initialize_from_env(true);
    {
        let _domain = memory_runtime::bind_current_thread(MemoryDomain::PlaybackRender);
        let mut blocks = Vec::with_capacity(64);
        for index in 0..64usize {
            blocks.push(vec![index as f32; 1024]);
        }
        blocks.clear();
    }
    memory_runtime::decay_domain(MemoryDomain::PlaybackRender);
    let snapshot = memory_runtime::snapshot();
    println!("memory_probe: ok");
    println!("allocator={}", snapshot.allocator);
    println!("enabled={}", snapshot.enabled);
    println!("stats_available={}", snapshot.totals.available);
    println!("resident_bytes={}", snapshot.totals.resident_bytes);
    println!("retained_bytes={}", snapshot.totals.retained_bytes);
    println!("domain_count={}", snapshot.domains.len());
    println!("json={}", snapshot.to_json());
}
