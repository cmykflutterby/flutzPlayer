use std::{env, fs, io::Cursor, path::PathBuf};

use flutz_dat::{
    assets::{DatAssetEntry, PreparedDatAsset, PreparedDatAssetPayload},
    read::{parse_dat_index_file, read_entry_ranges_from_file, DatEntryRange},
    write::write_dat_archive_files,
};
use rustystem::SoundFont;

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
    let closure = full.closure_for_preset(preset.get_bank_number(), preset.get_patch_number());

    let wave_bytes = i16_slice_to_le_bytes(full.get_wave_data());
    let data_dir = PathBuf::from("_local/runtime-tests/range-fetch-assemble-probe");
    if data_dir.exists() {
        fs::remove_dir_all(&data_dir)?;
    }
    fs::create_dir_all(&data_dir)?;
    let dat_path = data_dir.join("range-fetch-assemble-probe.dat");
    write_dat_archive_files(
        vec![PreparedDatAsset {
            entry: DatAssetEntry {
                internal_id: "probe.wave-data".to_owned(),
                display_name: "Probe Wave Data".to_owned(),
                asset_type: "soundfont-wave-data".to_owned(),
                source_format: "i16-le".to_owned(),
                storage_format: "i16-le".to_owned(),
                runtime_format: "i16-le".to_owned(),
                original_filename: "retro.wave.i16".to_owned(),
            },
            flags: 0,
            payload: PreparedDatAssetPayload::Bytes(wave_bytes),
        }],
        &dat_path,
        1_048_576,
        268_435_456,
    )?;

    let index = parse_dat_index_file(&dat_path)?;
    let record = index
        .entries
        .iter()
        .find(|record| record.entry.internal_id == "probe.wave-data")
        .ok_or("wave data DAT entry missing")?;
    let ranges = closure
        .sample_ids
        .iter()
        .map(|sample_id| {
            let sample = full
                .get_sample_headers()
                .get(*sample_id)
                .ok_or("closure referenced missing sample")?;
            Ok(DatEntryRange {
                offset: sample.get_start() as u64 * 2,
                length: (sample.get_end() - sample.get_start()) as u64 * 2,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let fetched = read_entry_ranges_from_file(&dat_path, &index, record, &ranges)?;
    let compact_wave_data = le_bytes_to_i16_vec(&fetched)?;
    let compact = full.compact_from_closure_and_wave_data(&closure, compact_wave_data)?;

    if compact.get_wave_data().len() >= full.get_wave_data().len() {
        return Err("range-fetched compact font did not reduce wave data".into());
    }
    if compact.get_sample_headers().len() != closure.sample_ids.len() {
        return Err("compact font sample count does not match fetched closure".into());
    }

    fs::remove_dir_all(&data_dir)?;
    println!(
        "range_fetch_and_assemble_probe ok: fetched_bytes={} compact_wave_samples={} full_wave_samples={}",
        fetched.len(),
        compact.get_wave_data().len(),
        full.get_wave_data().len()
    );
    Ok(())
}

fn i16_slice_to_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn le_bytes_to_i16_vec(bytes: &[u8]) -> Result<Vec<i16>, Box<dyn std::error::Error>> {
    if bytes.len() % 2 != 0 {
        return Err("odd byte count for i16 wave data".into());
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}
