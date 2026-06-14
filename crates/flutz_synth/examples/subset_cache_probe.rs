use std::{env, fs, io::Cursor, path::PathBuf};

use flutz_synth::{SoundFontRuntimeCache, SoundFontSubsetBytes, SoundFontSubsetSampleRange};
use rustystem::{SoundFont, SoundFontMetadataClosure};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("_local/runtime-tests/retro.sf2"));
    let bytes = fs::read(&path)?;
    let full = SoundFont::new(&mut Cursor::new(&bytes))?;
    let preset = full
        .get_presets()
        .first()
        .ok_or("full font has no presets")?;
    let full_closure = full.closure_for_preset(preset.get_bank_number(), preset.get_patch_number());
    if full_closure.sample_ids.len() < 2 {
        return Err("probe needs a preset with at least two samples".into());
    }

    let small_closure = SoundFontMetadataClosure::new(
        full_closure.preset_ids.clone(),
        full_closure.instrument_ids.clone(),
        vec![full_closure.sample_ids[0]],
    );
    let growth_closure = SoundFontMetadataClosure::new(
        full_closure.preset_ids.clone(),
        full_closure.instrument_ids.clone(),
        full_closure.sample_ids.iter().take(2).copied().collect(),
    );

    let mut cache = SoundFontRuntimeCache::default();
    let dat_files = vec![path.display().to_string()];
    let full_ranges = sample_ranges(&full, &full_closure)?;
    let first = cache.load_subset_soundfont(SoundFontSubsetBytes::from_dat_entry(
        "retro",
        "Retro",
        bytes.clone(),
        dat_files.clone(),
        "full-first-preset",
        full_closure.clone(),
        full_ranges.clone(),
    ))?;
    let after_first = cache.debug();
    if after_first.metadata_entries != 1
        || after_first.subset_entries != 1
        || after_first.sample_range_entries != full_closure.sample_ids.len()
        || after_first.subset_misses != 1
        || after_first.sample_range_misses != full_closure.sample_ids.len() as u64
    {
        return Err(
            format!("unexpected cache state after first subset load: {after_first:?}").into(),
        );
    }

    let exact = cache.load_subset_soundfont(SoundFontSubsetBytes::from_dat_entry(
        "retro",
        "Retro",
        bytes.clone(),
        dat_files.clone(),
        "full-first-preset",
        full_closure.clone(),
        Vec::new(),
    ))?;
    if !std::sync::Arc::ptr_eq(&first, &exact) {
        return Err("exact subset load did not reuse cached subset Arc".into());
    }
    let after_exact = cache.debug();
    if after_exact.subset_hits != 1 {
        return Err(format!("exact subset hit was not recorded: {after_exact:?}").into());
    }

    let contained = cache.load_subset_soundfont(SoundFontSubsetBytes::from_dat_entry(
        "retro",
        "Retro",
        bytes.clone(),
        dat_files.clone(),
        "smaller-first-sample",
        small_closure.clone(),
        Vec::new(),
    ))?;
    if !std::sync::Arc::ptr_eq(&first, &contained) {
        return Err("smaller subset did not reuse containing cached subset Arc".into());
    }
    let after_contained = cache.debug();
    if after_contained.subset_contained_hits != 1 {
        return Err(format!("contained subset hit was not recorded: {after_contained:?}").into());
    }

    let mut growth_cache = SoundFontRuntimeCache::default();
    let small_ranges = sample_ranges(&full, &small_closure)?;
    growth_cache.load_subset_soundfont(SoundFontSubsetBytes::from_dat_entry(
        "retro",
        "Retro",
        bytes.clone(),
        dat_files.clone(),
        "small",
        small_closure,
        small_ranges,
    ))?;
    let first_growth_state = growth_cache.debug();
    let growth_ranges = sample_ranges(&full, &growth_closure)?;
    growth_cache.load_subset_soundfont(SoundFontSubsetBytes::from_dat_entry(
        "retro",
        "Retro",
        bytes,
        dat_files,
        "growth",
        growth_closure,
        growth_ranges,
    ))?;
    let after_growth = growth_cache.debug();
    if after_growth.sample_range_hits <= first_growth_state.sample_range_hits {
        return Err(
            format!("superset growth did not hit existing sample range: {after_growth:?}").into(),
        );
    }
    if after_growth.sample_range_misses != first_growth_state.sample_range_misses + 1 {
        return Err(format!(
            "superset growth fetched more than the missing range: {after_growth:?}"
        )
        .into());
    }

    println!(
        "subset_cache_probe ok: exact_hits={} contained_hits={} sample_hits={} sample_misses={}",
        after_growth.subset_hits,
        after_contained.subset_contained_hits,
        after_growth.sample_range_hits,
        after_growth.sample_range_misses
    );
    Ok(())
}

fn sample_ranges(
    soundfont: &SoundFont,
    closure: &SoundFontMetadataClosure,
) -> Result<Vec<SoundFontSubsetSampleRange>, Box<dyn std::error::Error>> {
    closure
        .sample_ids
        .iter()
        .map(|sample_id| {
            let header = soundfont
                .get_sample_headers()
                .get(*sample_id)
                .ok_or("closure referenced missing sample")?;
            let start = header.get_start() as usize;
            let end = header.get_end() as usize;
            Ok(SoundFontSubsetSampleRange {
                sample_id: *sample_id,
                samples: soundfont.get_wave_data()[start..end].to_vec(),
            })
        })
        .collect()
}
