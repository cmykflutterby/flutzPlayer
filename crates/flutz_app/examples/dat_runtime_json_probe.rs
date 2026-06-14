use std::{collections::BTreeSet, fs, path::PathBuf};

use flutz_app::{
    app::{DatStartupSummary, SoundFontCatalogEntry},
    dat_startup_summary_for_data_dir,
    playback::{AudioBackend, PlaybackController},
};
use flutz_dat::{
    assets::{DatAssetEntry, PreparedDatAsset, PreparedDatAssetPayload, DAT_ENTRY_FLAG_DEFAULT},
    soundfont_json::{
        to_json_bytes, BankProgramCoverageJson, GeneratedSoundFontJsonResources,
        InstrumentIndexJson, InstrumentLoadMapJson, PresetCoverageJson, PresetIndexJson,
        PresetLoadMapJson, SampleHeaderJson, SoundFontCoverageJson, SoundFontIndexJson,
        SoundFontMetadataJson, SoundFontWaveRangeJson, JSON_FORMAT, SOUNDFONT_COVERAGE_ASSET_TYPE,
        SOUNDFONT_INDEX_ASSET_TYPE, SOUNDFONT_METADATA_ASSET_TYPE,
    },
    write::write_dat_archive_files,
};
use flutz_synth::{BankProgram, MelodicCoverage, PercussionCoverage, SoundFontCoverage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = PathBuf::from("_local/runtime-tests/phase7-runtime-probe");
    if data_dir.exists() {
        fs::remove_dir_all(&data_dir)?;
    }
    fs::create_dir_all(&data_dir)?;

    let sf2_bytes = fs::read("_local/runtime-tests/retro.sf2")?;
    let provider_entry = soundfont_entry("provider", "Provider", true);
    let assets = vec![
        PreparedDatAsset {
            entry: provider_entry.clone(),
            flags: DAT_ENTRY_FLAG_DEFAULT,
            payload: PreparedDatAssetPayload::Bytes(sf2_bytes),
        },
        generated_asset(
            "provider.metadata",
            SOUNDFONT_METADATA_ASSET_TYPE,
            to_json_bytes(&metadata_json("provider"))?,
        ),
        generated_asset(
            "provider.coverage",
            SOUNDFONT_COVERAGE_ASSET_TYPE,
            to_json_bytes(&coverage_json("provider", true))?,
        ),
        generated_asset(
            "provider.index",
            SOUNDFONT_INDEX_ASSET_TYPE,
            to_json_bytes(&index_json("provider"))?,
        ),
    ];
    write_dat_archive_files(
        assets,
        &data_dir.join("phase7-runtime-probe.dat"),
        1_048_576,
        268_435_456,
    )?;

    let startup = dat_startup_summary_for_data_dir(&data_dir)?;
    let DatStartupSummary::Available { soundfonts, .. } = startup else {
        return Err("startup summary was unavailable".into());
    };
    let provider = soundfonts
        .iter()
        .find(|font| font.internal_id == "provider")
        .ok_or("provider missing from startup summary")?;
    if !provider
        .coverage
        .as_ref()
        .map(|coverage| coverage.provides_melodic(0, 0))
        .unwrap_or(false)
    {
        return Err("startup summary did not consume coverage JSON".into());
    }

    let catalog = vec![
        SoundFontCatalogEntry {
            internal_id: "non_provider".to_owned(),
            display_name: "Non Provider".to_owned(),
            source_format: "sf2".to_owned(),
            storage_format: "sf2".to_owned(),
            runtime_format: "sf2".to_owned(),
            is_default: false,
            total_size: 1,
            part_count: 1,
            coverage: Some(coverage(false)),
        },
        provider.clone(),
    ];
    let mut controller =
        PlaybackController::new(data_dir.clone(), catalog, AudioBackend::Sdl3, false, false);
    let requested = vec!["non_provider".to_owned(), "provider".to_owned()];
    controller.load_midi_bytes(simple_midi_bytes(), "phase7-probe.mid", &requested)?;
    let loaded = controller.loaded_soundfont_ids();
    if loaded != ["provider".to_owned()] {
        return Err(format!("coverage pruning loaded unexpected fonts: {loaded:?}").into());
    }
    let metrics = controller.debug_metrics();
    if metrics.requested_soundfont_count != 2
        || metrics.loaded_provider_count != 1
        || metrics.pruned_soundfont_count != 1
        || metrics.midi_demand.role_count() != 1
        || metrics.midi_demand.melodic_programs.len() != 1
        || metrics.midi_demand.melodic_programs[0].bank != 0
        || metrics.midi_demand.melodic_programs[0].program != 0
    {
        return Err(format!("unexpected demand diagnostics: {metrics:?}").into());
    }
    let plans = controller.last_soundfont_subset_plans();
    let plan = plans
        .get("provider")
        .ok_or("missing provider subset plan from index JSON")?;
    if !plan.used_index_json
        || plan.preset_ids != [0]
        || plan.instrument_ids != [0]
        || plan.sample_ids != [0]
        || plan.planned_range_count == 0
        || plan.planned_byte_count == 0
    {
        return Err(format!("unexpected subset plan: {plan:?}").into());
    }
    if metrics.subset_plans.plan_count != 1
        || metrics.subset_plans.preset_count != 1
        || metrics.subset_plans.instrument_count != 1
        || metrics.subset_plans.sample_count != 1
        || metrics.subset_plans.planned_range_count == 0
        || metrics.subset_plans.planned_byte_count == 0
        || metrics.subset_plans.index_json_plan_count != 1
        || metrics.subset_plans.signatures.len() != 1
        || metrics.subset_plans.signatures[0].soundfont_id != "provider"
        || metrics.subset_plans.signatures[0].full_font_fallback
    {
        return Err(format!("unexpected subset aggregate diagnostics: {metrics:?}").into());
    }
    if metrics.soundfont_cache.subset_entries != 1
        || metrics.soundfont_cache.full_entries != 0
        || metrics.subset_transition.compact_loaded_count != 1
        || metrics.subset_transition.full_fallback_count != 0
        || metrics.loaded_subset_state.len() != 1
    {
        return Err(
            format!("subset playback was not loaded through compact cache: {metrics:?}").into(),
        );
    }

    controller.load_midi_bytes(simple_midi_bytes(), "phase7-probe-reload.mid", &requested)?;
    let reload_metrics = controller.debug_metrics();
    if reload_metrics.subset_transition.exact_subset_hits != 1
        || reload_metrics.soundfont_cache.subset_entries != 1
        || reload_metrics.soundfont_cache.full_entries != 0
    {
        return Err(
            format!("exact subset reload was not diagnosed as reuse: {reload_metrics:?}").into(),
        );
    }

    fs::remove_dir_all(&data_dir)?;
    println!("dat_runtime_json_probe ok: coverage pruning, demand diagnostics, compact subset playback, and exact reuse validated");
    Ok(())
}

fn soundfont_entry(internal_id: &str, display_name: &str, default: bool) -> DatAssetEntry {
    let _ = default;
    DatAssetEntry {
        internal_id: internal_id.to_owned(),
        display_name: display_name.to_owned(),
        asset_type: "soundfont".to_owned(),
        source_format: "sf2".to_owned(),
        storage_format: "sf2".to_owned(),
        runtime_format: "sf2".to_owned(),
        original_filename: "retro.sf2".to_owned(),
    }
}

fn generated_asset(internal_id: &str, asset_type: &str, bytes: Vec<u8>) -> PreparedDatAsset {
    PreparedDatAsset {
        entry: DatAssetEntry {
            internal_id: internal_id.to_owned(),
            display_name: internal_id.to_owned(),
            asset_type: asset_type.to_owned(),
            source_format: JSON_FORMAT.to_owned(),
            storage_format: JSON_FORMAT.to_owned(),
            runtime_format: JSON_FORMAT.to_owned(),
            original_filename: format!("{internal_id}.json"),
        },
        flags: 0,
        payload: PreparedDatAssetPayload::Bytes(bytes),
    }
}

fn metadata_json(parent: &str) -> SoundFontMetadataJson {
    SoundFontMetadataJson {
        parent_soundfont_id: parent.to_owned(),
        display_name: parent.to_owned(),
        source_format: "sf2".to_owned(),
        storage_format: "sf2".to_owned(),
        runtime_format: "sf2".to_owned(),
        original_filename: "retro.sf2".to_owned(),
        generated_resources: GeneratedSoundFontJsonResources {
            metadata_internal_id: format!("{parent}.metadata"),
            coverage_internal_id: format!("{parent}.coverage"),
            index_internal_id: format!("{parent}.index"),
            pack_report_internal_id: None,
        },
        preset_count: 1,
        sample_count: 1,
        pack_measurements: None,
    }
}

fn coverage_json(parent: &str, provides_program_zero: bool) -> SoundFontCoverageJson {
    SoundFontCoverageJson {
        parent_soundfont_id: parent.to_owned(),
        melodic: provides_program_zero
            .then_some(BankProgramCoverageJson {
                bank: 0,
                program: 0,
            })
            .into_iter()
            .collect(),
        percussion: false,
        percussion_key_ranges: Vec::new(),
        presets: provides_program_zero
            .then_some(PresetCoverageJson {
                preset_id: 0,
                name: "Program 0".to_owned(),
                bank: 0,
                program: 0,
                percussion: false,
            })
            .into_iter()
            .collect(),
        sample_count: 1,
    }
}

fn index_json(parent: &str) -> SoundFontIndexJson {
    SoundFontIndexJson {
        parent_soundfont_id: parent.to_owned(),
        smpl_data_start_byte: None,
        presets: vec![PresetIndexJson {
            preset_id: 0,
            name: "Program 0".to_owned(),
            bank: 0,
            program: 0,
            instrument_ids: vec![0],
        }],
        instruments: vec![InstrumentIndexJson {
            instrument_id: 0,
            name: "Instrument 0".to_owned(),
            regions: Vec::new(),
        }],
        samples: vec![SampleHeaderJson {
            sample_id: 0,
            name: "Sample 0".to_owned(),
            start: 0,
            end: 16,
            start_loop: 0,
            end_loop: 16,
            sample_rate: 44_100,
            original_pitch: 60,
            pitch_correction: 0,
            link: 0,
            sample_type: 1,
            wave_range: SoundFontWaveRangeJson {
                smpl_start_sample: 0,
                smpl_end_sample: 16,
                smpl_start_byte: 0,
                byte_length: 32,
            },
        }],
        preset_load_map: vec![PresetLoadMapJson {
            preset_id: 0,
            preset_name: Some("Program 0".to_owned()),
            instrument_ids: vec![0],
        }],
        instrument_load_map: vec![InstrumentLoadMapJson {
            instrument_id: 0,
            instrument_name: Some("Instrument 0".to_owned()),
            region_ids: vec![0],
            sample_ids: vec![0],
            sample_header_ids: vec![0],
            generator_region_ids: vec![0],
            wave_ranges: vec![SoundFontWaveRangeJson {
                smpl_start_sample: 0,
                smpl_end_sample: 16,
                smpl_start_byte: 0,
                byte_length: 32,
            }],
        }],
    }
}

fn coverage(provides_program_zero: bool) -> SoundFontCoverage {
    SoundFontCoverage {
        melodic: MelodicCoverage {
            bank_programs: provides_program_zero
                .then_some(BankProgram {
                    bank: 0,
                    program: 0,
                })
                .into_iter()
                .collect::<BTreeSet<_>>(),
        },
        percussion: PercussionCoverage::default(),
        metadata: Default::default(),
    }
}

fn simple_midi_bytes() -> Vec<u8> {
    vec![
        b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0, 96, b'M', b'T', b'r', b'k', 0, 0, 0, 15,
        0, 0xC0, 0, 0, 0x90, 60, 64, 0x60, 0x80, 60, 64, 0, 0xFF, 0x2F, 0,
    ]
}
