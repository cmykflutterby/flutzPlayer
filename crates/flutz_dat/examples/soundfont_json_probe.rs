use flutz_dat::soundfont_json::{
    from_json_bytes, to_json_bytes, BankProgramCoverageJson, DatPhysicalRangeJson,
    GeneratedSoundFontJsonResources, GeneratorValueJson, InstrumentIndexJson,
    InstrumentLoadMapJson, InstrumentRegionJson, KeyRangeJson, PresetCoverageJson, PresetIndexJson,
    PresetLoadMapJson, SampleHeaderJson, SoundFontCoverageJson, SoundFontIndexJson,
    SoundFontMetadataJson, SoundFontPackMeasurementsJson, SoundFontPackReportJson,
    SoundFontPackWarningJson, SoundFontWaveRangeJson, VelocityRangeJson,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let resources = GeneratedSoundFontJsonResources {
        metadata_internal_id: "retro.metadata".to_owned(),
        coverage_internal_id: "retro.coverage".to_owned(),
        index_internal_id: "retro.index".to_owned(),
        pack_report_internal_id: Some("retro.pack-report".to_owned()),
    };

    let metadata = SoundFontMetadataJson {
        parent_soundfont_id: "retro".to_owned(),
        display_name: "Retro".to_owned(),
        source_format: "sf2".to_owned(),
        storage_format: "sf2".to_owned(),
        runtime_format: "sf2".to_owned(),
        original_filename: "retro.sf2".to_owned(),
        generated_resources: resources.clone(),
        preset_count: 1,
        sample_count: 1,
        pack_measurements: Some(SoundFontPackMeasurementsJson {
            source_byte_count: Some(4096),
            runtime_byte_count: Some(4096),
            preset_count: Some(1),
            instrument_count: Some(1),
            sample_count: Some(1),
        }),
    };
    assert_roundtrip(&metadata)?;

    let coverage = SoundFontCoverageJson {
        parent_soundfont_id: "retro".to_owned(),
        melodic: vec![BankProgramCoverageJson {
            bank: 0,
            program: 1,
        }],
        percussion: false,
        percussion_key_ranges: Vec::new(),
        presets: vec![PresetCoverageJson {
            preset_id: 0,
            name: "Bright Piano".to_owned(),
            bank: 0,
            program: 1,
            percussion: false,
        }],
        sample_count: 1,
    };
    assert_roundtrip(&coverage)?;

    let wave_range = SoundFontWaveRangeJson {
        smpl_start_sample: 100,
        smpl_end_sample: 240,
        smpl_start_byte: 200,
        byte_length: 280,
    };
    let sample = SampleHeaderJson {
        sample_id: 0,
        name: "piano-c4".to_owned(),
        start: 100,
        end: 240,
        start_loop: 140,
        end_loop: 220,
        sample_rate: 44100,
        original_pitch: 60,
        pitch_correction: 0,
        link: 0,
        sample_type: 1,
        wave_range: wave_range.clone(),
    };
    let region = InstrumentRegionJson {
        region_id: 7,
        sample_id: Some(0),
        key_range: Some(KeyRangeJson {
            low_key: 0,
            high_key: 127,
        }),
        velocity_range: Some(VelocityRangeJson {
            low_velocity: 1,
            high_velocity: 127,
        }),
        generators: vec![GeneratorValueJson {
            generator_type: "sampleID".to_owned(),
            raw_value: 0,
        }],
    };
    let index = SoundFontIndexJson {
        parent_soundfont_id: "retro".to_owned(),
        smpl_data_start_byte: None,
        presets: vec![PresetIndexJson {
            preset_id: 0,
            name: "Bright Piano".to_owned(),
            bank: 0,
            program: 1,
            instrument_ids: vec![3],
        }],
        instruments: vec![InstrumentIndexJson {
            instrument_id: 3,
            name: "Piano Layer".to_owned(),
            regions: vec![region],
        }],
        samples: vec![sample],
        preset_load_map: vec![PresetLoadMapJson {
            preset_id: 0,
            preset_name: Some("Bright Piano".to_owned()),
            instrument_ids: vec![3],
        }],
        instrument_load_map: vec![InstrumentLoadMapJson {
            instrument_id: 3,
            instrument_name: Some("Piano Layer".to_owned()),
            region_ids: vec![7],
            sample_ids: vec![0],
            sample_header_ids: vec![0],
            generator_region_ids: vec![7],
            wave_ranges: vec![wave_range],
        }],
    };
    let decoded_index = assert_roundtrip(&index)?;
    let instrument_plan = decoded_index
        .instrument_load_map
        .iter()
        .find(|entry| entry.instrument_id == 3)
        .ok_or("missing instrument load-map entry")?;
    if instrument_plan.sample_ids != [0] || instrument_plan.wave_ranges.len() != 1 {
        return Err("instrument load-map entry did not preserve sample range data".into());
    }

    let pack_report = SoundFontPackReportJson {
        parent_soundfont_id: "retro".to_owned(),
        generated_resources: resources,
        measurements: SoundFontPackMeasurementsJson {
            source_byte_count: Some(4096),
            runtime_byte_count: Some(4096),
            preset_count: Some(1),
            instrument_count: Some(1),
            sample_count: Some(1),
        },
        warnings: vec![SoundFontPackWarningJson {
            code: "demo".to_owned(),
            message: "probe warning".to_owned(),
        }],
        raw_soundfont_ranges: vec![DatPhysicalRangeJson {
            dat_file: "assets.dat".to_owned(),
            chunk_id: 0,
            file_offset: 1024,
            extent_offset: 64,
            length: 280,
        }],
    };
    assert_roundtrip(&pack_report)?;

    let tolerant_json = br#"{
        "parent_soundfont_id": "retro",
        "melodic": [],
        "percussion": false,
        "sample_count": 0,
        "future_field": { "ignored": true }
    }"#;
    let tolerant_coverage: SoundFontCoverageJson = from_json_bytes(tolerant_json)?;
    if tolerant_coverage.parent_soundfont_id != "retro" || !tolerant_coverage.presets.is_empty() {
        return Err("tolerant JSON loading did not preserve defaults".into());
    }

    println!(
        "soundfont_json_probe ok: metadata/coverage/index/report roundtrips and instrument load-map lookup validated"
    );
    Ok(())
}

fn assert_roundtrip<T>(value: &T) -> Result<T, Box<dyn std::error::Error>>
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = to_json_bytes(value)?;
    let decoded = from_json_bytes::<T>(&json)?;
    if &decoded != value {
        return Err(format!("JSON roundtrip mismatch: {decoded:?}").into());
    }
    Ok(decoded)
}
