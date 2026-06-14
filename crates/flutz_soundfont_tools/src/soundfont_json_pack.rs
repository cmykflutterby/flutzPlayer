use std::{collections::BTreeSet, io::Cursor};

use flutz_dat::{
    assets::{DatAssetEntry, PreparedDatAsset, PreparedDatAssetPayload},
    soundfont_json::{
        to_json_bytes, BankProgramCoverageJson, GeneratedSoundFontJsonResources,
        GeneratorValueJson, InstrumentIndexJson, InstrumentLoadMapJson, InstrumentRegionJson,
        KeyRangeJson, PresetCoverageJson, PresetIndexJson, PresetLoadMapJson, SampleHeaderJson,
        SoundFontCoverageJson, SoundFontIndexJson, SoundFontMetadataJson,
        SoundFontPackMeasurementsJson, SoundFontPackReportJson, SoundFontWaveRangeJson,
        VelocityRangeJson, JSON_FORMAT, SOUNDFONT_COVERAGE_ASSET_TYPE, SOUNDFONT_INDEX_ASSET_TYPE,
        SOUNDFONT_METADATA_ASSET_TYPE, SOUNDFONT_PACK_REPORT_ASSET_TYPE,
    },
};
use flutz_synth::extract_coverage_from_sf2;
use rustystem::SoundFont;

pub fn generate_soundfont_json_assets(
    parent_entry: &DatAssetEntry,
    runtime_bytes: &[u8],
    include_pack_report: bool,
) -> Result<Vec<PreparedDatAsset>, String> {
    let mut cursor = Cursor::new(runtime_bytes);
    let soundfont = SoundFont::new(&mut cursor).map_err(|error| {
        format!(
            "failed to preparse soundfont {} for generated JSON resources: {error:?}",
            parent_entry.internal_id
        )
    })?;

    let resources = GeneratedSoundFontJsonResources {
        metadata_internal_id: generated_internal_id(&parent_entry.internal_id, "metadata"),
        coverage_internal_id: generated_internal_id(&parent_entry.internal_id, "coverage"),
        index_internal_id: generated_internal_id(&parent_entry.internal_id, "index"),
        pack_report_internal_id: include_pack_report
            .then(|| generated_internal_id(&parent_entry.internal_id, "pack-report")),
    };

    let measurements = SoundFontPackMeasurementsJson {
        source_byte_count: Some(runtime_bytes.len() as u64),
        runtime_byte_count: Some(runtime_bytes.len() as u64),
        preset_count: Some(soundfont.get_presets().len() as u32),
        instrument_count: Some(soundfont.get_instruments().len() as u32),
        sample_count: Some(soundfont.get_sample_headers().len() as u32),
    };

    let metadata = SoundFontMetadataJson {
        parent_soundfont_id: parent_entry.internal_id.clone(),
        display_name: parent_entry.display_name.clone(),
        source_format: parent_entry.source_format.clone(),
        storage_format: parent_entry.storage_format.clone(),
        runtime_format: parent_entry.runtime_format.clone(),
        original_filename: parent_entry.original_filename.clone(),
        generated_resources: resources.clone(),
        preset_count: soundfont.get_presets().len() as u32,
        sample_count: soundfont.get_sample_headers().len() as u32,
        pack_measurements: Some(measurements.clone()),
    };
    let coverage = build_coverage_json(parent_entry, &soundfont);
    let index = build_index_json(parent_entry, &soundfont, runtime_bytes);

    let mut assets = vec![
        generated_asset(
            parent_entry,
            &resources.metadata_internal_id,
            SOUNDFONT_METADATA_ASSET_TYPE,
            "metadata",
            to_json_bytes(&metadata),
        )?,
        generated_asset(
            parent_entry,
            &resources.coverage_internal_id,
            SOUNDFONT_COVERAGE_ASSET_TYPE,
            "coverage",
            to_json_bytes(&coverage),
        )?,
        generated_asset(
            parent_entry,
            &resources.index_internal_id,
            SOUNDFONT_INDEX_ASSET_TYPE,
            "index",
            to_json_bytes(&index),
        )?,
    ];

    if let Some(pack_report_internal_id) = resources.pack_report_internal_id.clone() {
        let report = SoundFontPackReportJson {
            parent_soundfont_id: parent_entry.internal_id.clone(),
            generated_resources: resources,
            measurements,
            warnings: Vec::new(),
            raw_soundfont_ranges: Vec::new(),
        };
        assets.push(generated_asset(
            parent_entry,
            &pack_report_internal_id,
            SOUNDFONT_PACK_REPORT_ASSET_TYPE,
            "pack-report",
            to_json_bytes(&report),
        )?);
    }

    Ok(assets)
}

fn build_coverage_json(
    parent_entry: &DatAssetEntry,
    soundfont: &SoundFont,
) -> SoundFontCoverageJson {
    let coverage = extract_coverage_from_sf2(soundfont);
    let melodic = coverage
        .melodic
        .bank_programs
        .iter()
        .map(|entry| BankProgramCoverageJson {
            bank: entry.bank,
            program: entry.program,
        })
        .collect::<Vec<_>>();
    let presets = soundfont
        .get_presets()
        .iter()
        .enumerate()
        .filter_map(|(preset_id, preset)| {
            let bank = u16::try_from(preset.get_bank_number()).ok()?;
            let program = u8::try_from(preset.get_patch_number()).ok()?;
            Some(PresetCoverageJson {
                preset_id: preset_id as u32,
                name: preset.get_name().to_owned(),
                bank,
                program,
                percussion: bank == 128,
            })
        })
        .collect::<Vec<_>>();

    SoundFontCoverageJson {
        parent_soundfont_id: parent_entry.internal_id.clone(),
        melodic,
        percussion: coverage.provides_percussion(),
        percussion_key_ranges: Vec::new(),
        presets,
        sample_count: soundfont.get_sample_headers().len() as u32,
    }
}

fn build_index_json(
    parent_entry: &DatAssetEntry,
    soundfont: &SoundFont,
    runtime_bytes: &[u8],
) -> SoundFontIndexJson {
    let bytes_per_sample = bytes_per_sample(soundfont.get_bits_per_sample());
    let samples = soundfont
        .get_sample_headers()
        .iter()
        .enumerate()
        .map(|(sample_id, sample)| SampleHeaderJson {
            sample_id: sample_id as u32,
            name: sample.get_name().to_owned(),
            start: nonnegative_u32(sample.get_start()),
            end: nonnegative_u32(sample.get_end()),
            start_loop: nonnegative_u32(sample.get_start_loop()),
            end_loop: nonnegative_u32(sample.get_end_loop()),
            sample_rate: nonnegative_u32(sample.get_sample_rate()),
            original_pitch: nonnegative_u8(sample.get_original_pitch()),
            pitch_correction: sample
                .get_pitch_correction()
                .clamp(i8::MIN as i32, i8::MAX as i32) as i8,
            link: nonnegative_u16(sample.get_link()),
            sample_type: nonnegative_u16(sample.get_sample_type()),
            wave_range: wave_range(sample.get_start(), sample.get_end(), bytes_per_sample),
        })
        .collect::<Vec<_>>();

    let mut next_region_id = 0u32;
    let mut instruments = Vec::new();
    let mut instrument_load_map = Vec::new();
    for (instrument_id, instrument) in soundfont.get_instruments().iter().enumerate() {
        let mut sample_ids = BTreeSet::<u32>::new();
        let mut sample_header_ids = BTreeSet::<u32>::new();
        let mut region_ids = Vec::new();
        let mut regions = Vec::new();
        let mut wave_ranges = Vec::new();

        for region in instrument.get_regions() {
            let region_id = next_region_id;
            next_region_id += 1;
            let sample_id = region.get_sample_id() as u32;
            region_ids.push(region_id);
            sample_ids.insert(sample_id);
            sample_header_ids.insert(sample_id);
            let region_wave_range = wave_range(
                region.get_sample_start(),
                region.get_sample_end(),
                bytes_per_sample,
            );
            wave_ranges.push(region_wave_range);
            regions.push(InstrumentRegionJson {
                region_id,
                sample_id: Some(sample_id),
                key_range: Some(KeyRangeJson {
                    low_key: nonnegative_u8(region.get_key_range_start()),
                    high_key: nonnegative_u8(region.get_key_range_end()),
                }),
                velocity_range: Some(VelocityRangeJson {
                    low_velocity: nonnegative_u8(region.get_velocity_range_start()),
                    high_velocity: nonnegative_u8(region.get_velocity_range_end()),
                }),
                generators: vec![GeneratorValueJson {
                    generator_type: "sampleID".to_owned(),
                    raw_value: sample_id.clamp(0, i16::MAX as u32) as i16,
                }],
            });
        }

        instruments.push(InstrumentIndexJson {
            instrument_id: instrument_id as u32,
            name: instrument.get_name().to_owned(),
            regions,
        });
        instrument_load_map.push(InstrumentLoadMapJson {
            instrument_id: instrument_id as u32,
            instrument_name: Some(instrument.get_name().to_owned()),
            region_ids: region_ids.clone(),
            sample_ids: sample_ids.into_iter().collect(),
            sample_header_ids: sample_header_ids.into_iter().collect(),
            generator_region_ids: region_ids,
            wave_ranges,
        });
    }

    let mut presets = Vec::new();
    let mut preset_load_map = Vec::new();
    for (preset_id, preset) in soundfont.get_presets().iter().enumerate() {
        let instrument_ids = preset
            .get_regions()
            .iter()
            .map(|region| region.get_instrument_id() as u32)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        presets.push(PresetIndexJson {
            preset_id: preset_id as u32,
            name: preset.get_name().to_owned(),
            bank: nonnegative_u16(preset.get_bank_number()),
            program: nonnegative_u8(preset.get_patch_number()),
            instrument_ids: instrument_ids.clone(),
        });
        preset_load_map.push(PresetLoadMapJson {
            preset_id: preset_id as u32,
            preset_name: Some(preset.get_name().to_owned()),
            instrument_ids,
        });
    }

    SoundFontIndexJson {
        parent_soundfont_id: parent_entry.internal_id.clone(),
        smpl_data_start_byte: sf2_smpl_data_start_byte(runtime_bytes),
        presets,
        instruments,
        samples,
        preset_load_map,
        instrument_load_map,
    }
}

fn sf2_smpl_data_start_byte(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"sfbk" {
        return None;
    }
    find_smpl_data_start(bytes, 12, bytes.len())
}

fn find_smpl_data_start(bytes: &[u8], mut offset: usize, end: usize) -> Option<u64> {
    while offset.checked_add(8)? <= end && offset.checked_add(8)? <= bytes.len() {
        let id = bytes.get(offset..offset + 4)?;
        let length =
            u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        let data_start = offset.checked_add(8)?;
        let data_end = data_start.checked_add(length)?;
        if data_end > end || data_end > bytes.len() {
            return None;
        }
        if id == b"smpl" {
            return Some(data_start as u64);
        }
        if id == b"LIST" && length >= 4 {
            if let Some(found) = find_smpl_data_start(bytes, data_start + 4, data_end) {
                return Some(found);
            }
        }
        offset = data_end + (length & 1);
    }
    None
}

fn generated_asset(
    parent_entry: &DatAssetEntry,
    internal_id: &str,
    asset_type: &str,
    suffix: &str,
    bytes: serde_json::Result<Vec<u8>>,
) -> Result<PreparedDatAsset, String> {
    let bytes = bytes.map_err(|error| {
        format!(
            "failed to serialize generated JSON resource {internal_id} for {}: {error}",
            parent_entry.internal_id
        )
    })?;
    Ok(PreparedDatAsset {
        entry: DatAssetEntry {
            internal_id: internal_id.to_owned(),
            display_name: format!("{} {suffix}", parent_entry.display_name),
            asset_type: asset_type.to_owned(),
            source_format: JSON_FORMAT.to_owned(),
            storage_format: JSON_FORMAT.to_owned(),
            runtime_format: JSON_FORMAT.to_owned(),
            original_filename: format!("{}.{}.json", parent_entry.internal_id, suffix),
        },
        flags: 0,
        payload: PreparedDatAssetPayload::Bytes(bytes),
    })
}

fn generated_internal_id(parent_internal_id: &str, suffix: &str) -> String {
    format!("{parent_internal_id}.{suffix}")
}

fn wave_range(start: i32, end: i32, bytes_per_sample: u64) -> SoundFontWaveRangeJson {
    let start_sample = nonnegative_u32(start);
    let end_sample = nonnegative_u32(end).max(start_sample);
    SoundFontWaveRangeJson {
        smpl_start_sample: start_sample,
        smpl_end_sample: end_sample,
        smpl_start_byte: start_sample as u64 * bytes_per_sample,
        byte_length: (end_sample - start_sample) as u64 * bytes_per_sample,
    }
}

fn bytes_per_sample(bits_per_sample: i32) -> u64 {
    let bits = bits_per_sample.max(1) as u64;
    bits.div_ceil(8).max(1)
}

fn nonnegative_u32(value: i32) -> u32 {
    value.max(0) as u32
}

fn nonnegative_u16(value: i32) -> u16 {
    value.clamp(0, u16::MAX as i32) as u16
}

fn nonnegative_u8(value: i32) -> u8 {
    value.clamp(0, u8::MAX as i32) as u8
}
