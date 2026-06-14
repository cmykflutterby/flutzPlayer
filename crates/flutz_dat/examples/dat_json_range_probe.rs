use std::{fs, path::PathBuf};

use flutz_dat::{
    assets::{DatAssetEntry, PreparedDatAsset, PreparedDatAssetPayload},
    read::{
        absolute_soundfont_sample_ranges, coalesce_entry_ranges,
        find_soundfont_json_resource_entries, parse_dat_index_file, read_entry_range_plan,
        read_entry_ranges_from_file, read_soundfont_json_resources_from_file,
        read_soundfont_sample_ranges_from_files, DatEntryRange, SplitDatEntryPart,
    },
    soundfont_json::{
        to_json_bytes, GeneratedSoundFontJsonResources, InstrumentLoadMapJson, SampleHeaderJson,
        SoundFontCoverageJson, SoundFontIndexJson, SoundFontMetadataJson, SoundFontWaveRangeJson,
        SOUNDFONT_COVERAGE_ASSET_TYPE, SOUNDFONT_INDEX_ASSET_TYPE, SOUNDFONT_METADATA_ASSET_TYPE,
    },
    write::build_dat_archive,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_path = PathBuf::from("_local/runtime-tests/dat-json-range-probe.dat");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let raw_bytes = (0u8..96).collect::<Vec<_>>();
    let resources = GeneratedSoundFontJsonResources {
        metadata_internal_id: "probe.metadata".to_owned(),
        coverage_internal_id: "probe.coverage".to_owned(),
        index_internal_id: "probe.index".to_owned(),
        pack_report_internal_id: None,
    };
    let assets = vec![
        PreparedDatAsset {
            entry: entry("probe", "Probe", "soundfont", "sf2"),
            flags: 0,
            payload: PreparedDatAssetPayload::Bytes(raw_bytes.clone()),
        },
        PreparedDatAsset {
            entry: entry(
                "probe.metadata",
                "Probe metadata",
                SOUNDFONT_METADATA_ASSET_TYPE,
                "json",
            ),
            flags: 0,
            payload: PreparedDatAssetPayload::Bytes(to_json_bytes(&SoundFontMetadataJson {
                parent_soundfont_id: "probe".to_owned(),
                display_name: "Probe".to_owned(),
                source_format: "sf2".to_owned(),
                storage_format: "sf2".to_owned(),
                runtime_format: "sf2".to_owned(),
                original_filename: "probe.sf2".to_owned(),
                generated_resources: resources.clone(),
                preset_count: 1,
                sample_count: 1,
                pack_measurements: None,
            })?),
        },
        PreparedDatAsset {
            entry: entry(
                "probe.coverage",
                "Probe coverage",
                SOUNDFONT_COVERAGE_ASSET_TYPE,
                "json",
            ),
            flags: 0,
            payload: PreparedDatAssetPayload::Bytes(to_json_bytes(&SoundFontCoverageJson {
                parent_soundfont_id: "probe".to_owned(),
                melodic: Vec::new(),
                percussion: false,
                percussion_key_ranges: Vec::new(),
                presets: Vec::new(),
                sample_count: 1,
            })?),
        },
        PreparedDatAsset {
            entry: entry(
                "probe.index",
                "Probe index",
                SOUNDFONT_INDEX_ASSET_TYPE,
                "json",
            ),
            flags: 0,
            payload: PreparedDatAssetPayload::Bytes(to_json_bytes(&SoundFontIndexJson {
                parent_soundfont_id: "probe".to_owned(),
                smpl_data_start_byte: Some(32),
                presets: Vec::new(),
                instruments: Vec::new(),
                samples: vec![SampleHeaderJson {
                    sample_id: 0,
                    name: "Probe sample".to_owned(),
                    start: 8,
                    end: 24,
                    start_loop: 10,
                    end_loop: 22,
                    sample_rate: 44100,
                    original_pitch: 60,
                    pitch_correction: 0,
                    link: 0,
                    sample_type: 1,
                    wave_range: SoundFontWaveRangeJson {
                        smpl_start_sample: 8,
                        smpl_end_sample: 24,
                        smpl_start_byte: 16,
                        byte_length: 32,
                    },
                }],
                preset_load_map: Vec::new(),
                instrument_load_map: vec![InstrumentLoadMapJson {
                    instrument_id: 7,
                    instrument_name: Some("Probe instrument".to_owned()),
                    region_ids: vec![11],
                    sample_ids: vec![0],
                    sample_header_ids: vec![0],
                    generator_region_ids: vec![11],
                    wave_ranges: vec![SoundFontWaveRangeJson {
                        smpl_start_sample: 8,
                        smpl_end_sample: 24,
                        smpl_start_byte: 16,
                        byte_length: 32,
                    }],
                }],
            })?),
        },
        PreparedDatAsset {
            entry: entry(
                "probe.broken.coverage",
                "Broken",
                SOUNDFONT_COVERAGE_ASSET_TYPE,
                "json",
            ),
            flags: 0,
            payload: PreparedDatAssetPayload::Bytes(br#"{"parent_soundfont_id":42}"#.to_vec()),
        },
    ];

    let archive = build_dat_archive(assets, 24)?;
    fs::write(&output_path, archive.bytes)?;

    let index = parse_dat_index_file(&output_path)?;
    let json_entries = find_soundfont_json_resource_entries(&index, "probe");
    if json_entries.metadata.is_none()
        || json_entries.coverage.is_none()
        || json_entries.index.is_none()
    {
        return Err("failed to locate generated JSON resource entries".into());
    }
    let json = read_soundfont_json_resources_from_file(&output_path, &index, "probe")?;
    if json.metadata.is_none()
        || json.coverage.is_none()
        || json
            .index
            .as_ref()
            .map(|index| index.instrument_load_map.len())
            != Some(1)
    {
        return Err("failed to parse generated JSON resource entries tolerantly".into());
    }

    let raw_entry = index
        .entries
        .iter()
        .find(|entry| entry.entry.internal_id == "probe")
        .ok_or("missing raw probe entry")?;
    let requested = [
        DatEntryRange {
            offset: 8,
            length: 10,
        },
        DatEntryRange {
            offset: 18,
            length: 6,
        },
    ];
    let coalesced = coalesce_entry_ranges(&requested)?;
    if coalesced
        != [DatEntryRange {
            offset: 8,
            length: 16,
        }]
    {
        return Err(format!("unexpected coalesced ranges: {coalesced:?}").into());
    }
    let plan = read_entry_range_plan(&index, raw_entry, &requested)?;
    if plan.is_empty() || plan.iter().map(|range| range.length).sum::<u64>() != 16 {
        return Err(format!("unexpected range plan: {plan:?}").into());
    }
    let bytes = read_entry_ranges_from_file(&output_path, &index, raw_entry, &requested)?;
    if bytes != raw_bytes[8..24] {
        return Err("range read bytes did not match expected raw entry slice".into());
    }

    let index_json = json.index.as_ref().ok_or("missing soundfont index JSON")?;
    let sample_ranges = absolute_soundfont_sample_ranges(index_json, &[0])?;
    if sample_ranges.len() != 1
        || sample_ranges[0].range
            != (DatEntryRange {
                offset: 48,
                length: 32,
            })
    {
        return Err(format!("unexpected absolute sample ranges: {sample_ranges:?}").into());
    }
    let parts = [SplitDatEntryPart {
        path: output_path.as_path(),
        index: &index,
        entry: raw_entry,
    }];
    let sample_bytes = read_soundfont_sample_ranges_from_files(&parts, index_json, &[0])?;
    if sample_bytes.samples.len() != 1 || sample_bytes.samples[0].bytes != raw_bytes[48..80] {
        return Err("soundfont sample range helper did not match expected raw entry slice".into());
    }

    fs::remove_file(&output_path)?;
    println!("dat_json_range_probe ok: JSON lookup/tolerant parse and DAT soundfont range reads validated");
    Ok(())
}

fn entry(internal_id: &str, display_name: &str, asset_type: &str, format: &str) -> DatAssetEntry {
    DatAssetEntry {
        internal_id: internal_id.to_owned(),
        display_name: display_name.to_owned(),
        asset_type: asset_type.to_owned(),
        source_format: format.to_owned(),
        storage_format: format.to_owned(),
        runtime_format: format.to_owned(),
        original_filename: format!("{internal_id}.{format}"),
    }
}
