use super::*;

pub(super) fn totals_json(totals: &MemoryTotalsSnapshot) -> String {
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

pub(super) fn os_json(os: &OsMemorySnapshot) -> String {
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

pub(super) fn domain_json(domain: &MemoryDomainSnapshot) -> String {
    format!(
        "{{\"name\":\"{}\",\"arena\":{},\"reuse_critical\":{},\"dirty_decay_ms\":{},\"muzzy_decay_ms\":{},\"bind_count\":{},\"decay_count\":{},\"purge_count\":{},\"growth_events\":{},\"retained_floor_hits\":{},\"zero_growth_streak\":{},\"warm_target_bytes\":{},\"prewarmed_bytes\":{}}}",
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
        domain.zero_growth_streak,
        domain.warm_target_bytes,
        domain.prewarmed_bytes
    )
}

pub(super) fn escape_json(value: &str) -> String {
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
