use super::*;

pub(super) fn validate_generated_soundfont_json_resources(
    dat_paths: &[PathBuf],
) -> Result<usize, String> {
    let assets = collect_extracted_assets(dat_paths, None)?;
    let raw_soundfont_ids = assets
        .iter()
        .filter(|asset| asset.entry.asset_type == "soundfont")
        .map(|asset| asset.entry.internal_id.clone())
        .collect::<BTreeSet<_>>();
    let generated_ids = assets
        .iter()
        .filter(|asset| is_generated_soundfont_json_asset_type(&asset.entry.asset_type))
        .map(|asset| asset.entry.internal_id.clone())
        .collect::<BTreeSet<_>>();

    let mut generated_count = 0usize;
    for asset in assets
        .iter()
        .filter(|asset| is_generated_soundfont_json_asset_type(&asset.entry.asset_type))
    {
        generated_count += 1;
        match asset.entry.asset_type.as_str() {
            SOUNDFONT_METADATA_ASSET_TYPE => {
                let metadata =
                    from_json_bytes::<SoundFontMetadataJson>(&asset.bytes).map_err(|error| {
                        format!(
                            "failed to parse generated JSON resource {}: {error}",
                            asset.entry.internal_id
                        )
                    })?;
                validate_parent_soundfont(
                    asset,
                    &metadata.parent_soundfont_id,
                    &raw_soundfont_ids,
                )?;
                validate_generated_resource_link(
                    asset,
                    &metadata.generated_resources.metadata_internal_id,
                    &generated_ids,
                )?;
                validate_generated_resource_link(
                    asset,
                    &metadata.generated_resources.coverage_internal_id,
                    &generated_ids,
                )?;
                validate_generated_resource_link(
                    asset,
                    &metadata.generated_resources.index_internal_id,
                    &generated_ids,
                )?;
                if let Some(pack_report_internal_id) = metadata
                    .generated_resources
                    .pack_report_internal_id
                    .as_deref()
                {
                    validate_generated_resource_link(
                        asset,
                        pack_report_internal_id,
                        &generated_ids,
                    )?;
                }
            }
            SOUNDFONT_COVERAGE_ASSET_TYPE => {
                let coverage =
                    from_json_bytes::<SoundFontCoverageJson>(&asset.bytes).map_err(|error| {
                        format!(
                            "failed to parse generated JSON resource {}: {error}",
                            asset.entry.internal_id
                        )
                    })?;
                validate_parent_soundfont(
                    asset,
                    &coverage.parent_soundfont_id,
                    &raw_soundfont_ids,
                )?;
            }
            SOUNDFONT_INDEX_ASSET_TYPE => {
                let index =
                    from_json_bytes::<SoundFontIndexJson>(&asset.bytes).map_err(|error| {
                        format!(
                            "failed to parse generated JSON resource {}: {error}",
                            asset.entry.internal_id
                        )
                    })?;
                validate_parent_soundfont(asset, &index.parent_soundfont_id, &raw_soundfont_ids)?;
                validate_soundfont_index_json(asset, &index)?;
            }
            SOUNDFONT_PACK_REPORT_ASSET_TYPE => {
                let report =
                    from_json_bytes::<SoundFontPackReportJson>(&asset.bytes).map_err(|error| {
                        format!(
                            "failed to parse generated JSON resource {}: {error}",
                            asset.entry.internal_id
                        )
                    })?;
                validate_parent_soundfont(asset, &report.parent_soundfont_id, &raw_soundfont_ids)?;
            }
            _ => {}
        }
    }

    Ok(generated_count)
}

pub(super) fn is_generated_soundfont_json_asset_type(asset_type: &str) -> bool {
    matches!(
        asset_type,
        SOUNDFONT_METADATA_ASSET_TYPE
            | SOUNDFONT_COVERAGE_ASSET_TYPE
            | SOUNDFONT_INDEX_ASSET_TYPE
            | SOUNDFONT_PACK_REPORT_ASSET_TYPE
    )
}

pub(super) fn validate_parent_soundfont(
    asset: &ReconstructedDatAsset,
    parent_soundfont_id: &str,
    raw_soundfont_ids: &BTreeSet<String>,
) -> Result<(), String> {
    if raw_soundfont_ids.contains(parent_soundfont_id) {
        Ok(())
    } else {
        Err(format!(
            "generated JSON resource {} references missing parent soundfont {}",
            asset.entry.internal_id, parent_soundfont_id
        ))
    }
}

pub(super) fn validate_generated_resource_link(
    asset: &ReconstructedDatAsset,
    internal_id: &str,
    generated_ids: &BTreeSet<String>,
) -> Result<(), String> {
    if generated_ids.contains(internal_id) {
        Ok(())
    } else {
        Err(format!(
            "generated JSON resource {} references missing generated resource {}",
            asset.entry.internal_id, internal_id
        ))
    }
}

pub(super) fn validate_soundfont_index_json(
    asset: &ReconstructedDatAsset,
    index: &SoundFontIndexJson,
) -> Result<(), String> {
    let instrument_ids = index
        .instruments
        .iter()
        .map(|instrument| instrument.instrument_id)
        .collect::<BTreeSet<_>>();
    let sample_ids = index
        .samples
        .iter()
        .map(|sample| sample.sample_id)
        .collect::<BTreeSet<_>>();

    for load_map in &index.instrument_load_map {
        if !instrument_ids.contains(&load_map.instrument_id) {
            return Err(format!(
                "generated index {} load map references missing instrument {}",
                asset.entry.internal_id, load_map.instrument_id
            ));
        }
        for sample_id in load_map
            .sample_ids
            .iter()
            .chain(&load_map.sample_header_ids)
        {
            if !sample_ids.contains(sample_id) {
                return Err(format!(
                    "generated index {} instrument {} references missing sample {}",
                    asset.entry.internal_id, load_map.instrument_id, sample_id
                ));
            }
        }
    }

    for load_map in &index.preset_load_map {
        for instrument_id in &load_map.instrument_ids {
            if !instrument_ids.contains(instrument_id) {
                return Err(format!(
                    "generated index {} preset {} references missing instrument {}",
                    asset.entry.internal_id, load_map.preset_id, instrument_id
                ));
            }
        }
    }

    Ok(())
}

pub(super) fn validate_generated_soundfont_json_semantics(
    expected_assets: &[PreparedDatAsset],
    dat_paths: &[PathBuf],
) -> Result<usize, String> {
    let actual_assets = collect_extracted_assets(dat_paths, None)?
        .into_iter()
        .map(|asset| (asset.entry.internal_id.clone(), asset))
        .collect::<BTreeMap<_, _>>();
    let mut validated = 0usize;

    for expected in expected_assets
        .iter()
        .filter(|asset| asset.entry.asset_type == "soundfont")
    {
        let raw_bytes = payload_bytes(&expected.payload)?;
        let mut cursor = Cursor::new(&raw_bytes);
        let soundfont = SoundFont::new(&mut cursor).map_err(|error| {
            format!(
                "failed to parse source soundfont {} for semantic validation: {error:?}",
                expected.entry.internal_id
            )
        })?;
        let coverage_id = format!("{}.coverage", expected.entry.internal_id);
        let index_id = format!("{}.index", expected.entry.internal_id);
        let Some(coverage_asset) = actual_assets.get(&coverage_id) else {
            continue;
        };
        let Some(index_asset) = actual_assets.get(&index_id) else {
            continue;
        };
        let coverage = from_json_bytes::<SoundFontCoverageJson>(&coverage_asset.bytes)
            .map_err(|error| format!("failed to parse {coverage_id}: {error}"))?;
        let index = from_json_bytes::<SoundFontIndexJson>(&index_asset.bytes)
            .map_err(|error| format!("failed to parse {index_id}: {error}"))?;
        validate_coverage_semantics(expected, &soundfont, &coverage)?;
        validate_index_semantics(expected, &soundfont, &index)?;
        validate_index_ranges_map_to_dat(expected, dat_paths, &index)?;
        validated += 1;
    }

    Ok(validated)
}

pub(super) fn validate_coverage_semantics(
    expected: &PreparedDatAsset,
    soundfont: &SoundFont,
    coverage: &SoundFontCoverageJson,
) -> Result<(), String> {
    if coverage.parent_soundfont_id != expected.entry.internal_id {
        return Err(format!(
            "coverage JSON parent mismatch for {}",
            expected.entry.internal_id
        ));
    }
    let expected_coverage = extract_coverage_from_sf2(soundfont);
    let expected_melodic = expected_coverage
        .melodic
        .bank_programs
        .iter()
        .map(|entry| (entry.bank, entry.program))
        .collect::<BTreeSet<_>>();
    let actual_melodic = coverage
        .melodic
        .iter()
        .map(|entry| (entry.bank, entry.program))
        .collect::<BTreeSet<_>>();
    if expected_melodic != actual_melodic {
        return Err(format!(
            "coverage JSON melodic bank/program set mismatch for {}",
            expected.entry.internal_id
        ));
    }
    if expected_coverage.provides_percussion() != coverage.percussion {
        return Err(format!(
            "coverage JSON percussion flag mismatch for {}",
            expected.entry.internal_id
        ));
    }
    if coverage.presets.len() != soundfont.get_presets().len() {
        return Err(format!(
            "coverage JSON preset count mismatch for {}",
            expected.entry.internal_id
        ));
    }
    Ok(())
}

pub(super) fn validate_index_semantics(
    expected: &PreparedDatAsset,
    soundfont: &SoundFont,
    index: &SoundFontIndexJson,
) -> Result<(), String> {
    if index.parent_soundfont_id != expected.entry.internal_id {
        return Err(format!(
            "index JSON parent mismatch for {}",
            expected.entry.internal_id
        ));
    }
    if index.presets.len() != soundfont.get_presets().len()
        || index.instruments.len() != soundfont.get_instruments().len()
        || index.samples.len() != soundfont.get_sample_headers().len()
    {
        return Err(format!(
            "index JSON top-level counts mismatch for {}",
            expected.entry.internal_id
        ));
    }
    for (instrument_id, instrument) in soundfont.get_instruments().iter().enumerate() {
        let load_map = index
            .instrument_load_map
            .iter()
            .find(|load_map| load_map.instrument_id == instrument_id as u32)
            .ok_or_else(|| {
                format!(
                    "index JSON missing instrument load map {} for {}",
                    instrument_id, expected.entry.internal_id
                )
            })?;
        let expected_sample_ids = instrument
            .get_regions()
            .iter()
            .map(|region| region.get_sample_id() as u32)
            .collect::<BTreeSet<_>>();
        let actual_sample_ids = load_map.sample_ids.iter().copied().collect::<BTreeSet<_>>();
        if expected_sample_ids != actual_sample_ids {
            return Err(format!(
                "index JSON instrument {} sample closure mismatch for {}",
                instrument_id, expected.entry.internal_id
            ));
        }
        if load_map.generator_region_ids != load_map.region_ids {
            return Err(format!(
                "index JSON instrument {} generator region closure mismatch for {}",
                instrument_id, expected.entry.internal_id
            ));
        }
        if load_map.wave_ranges.len() != instrument.get_regions().len() {
            return Err(format!(
                "index JSON instrument {} wave-range count mismatch for {}",
                instrument_id, expected.entry.internal_id
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_index_ranges_map_to_dat(
    expected: &PreparedDatAsset,
    dat_paths: &[PathBuf],
    index: &SoundFontIndexJson,
) -> Result<(), String> {
    let Some(load_map) = index
        .instrument_load_map
        .iter()
        .find(|load_map| !load_map.wave_ranges.is_empty())
    else {
        return Ok(());
    };
    let raw_location = find_raw_soundfont_location(dat_paths, &expected.entry.internal_id)?;
    let smpl_data_start_byte = index.smpl_data_start_byte.ok_or_else(|| {
        format!(
            "index JSON is missing SMPL data base for {}",
            expected.entry.internal_id
        )
    })?;
    for range in &load_map.wave_ranges {
        if range.byte_length == 0 {
            continue;
        }
        let entry_offset = smpl_data_start_byte
            .checked_add(range.smpl_start_byte)
            .ok_or_else(|| {
                format!(
                    "index JSON wave range offset overflow for {}",
                    expected.entry.internal_id
                )
            })?;
        let clamped_length = range
            .byte_length
            .min(raw_location.record.total_size.saturating_sub(entry_offset));
        if clamped_length == 0 {
            continue;
        }
        let plan = read_entry_range_plan(
            &raw_location.index,
            &raw_location.record,
            &[DatEntryRange {
                offset: entry_offset,
                length: clamped_length,
            }],
        )
        .map_err(|error| error.to_string())?;
        if plan.is_empty() {
            return Err(format!(
                "index JSON wave range did not map to DAT extents for {}",
                expected.entry.internal_id
            ));
        }
    }
    Ok(())
}

pub(super) fn find_raw_soundfont_location(
    dat_paths: &[PathBuf],
    internal_id: &str,
) -> Result<RawSoundFontLocation, String> {
    for dat_path in dat_paths {
        let index = parse_dat_index_file(dat_path).map_err(|error| error.to_string())?;
        if let Some(record) = index.entries.iter().find(|record| {
            record.entry.asset_type == "soundfont" && record.entry.internal_id == internal_id
        }) {
            return Ok(RawSoundFontLocation {
                dat_path: dat_path.clone(),
                index: index.clone(),
                record: record.clone(),
            });
        }
    }
    Err(format!("DAT output is missing raw soundfont {internal_id}"))
}
