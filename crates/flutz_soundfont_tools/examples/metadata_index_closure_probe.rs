use std::{env, fs, io::Cursor, path::PathBuf};

use flutz_dat::{
    assets::{DatAssetEntry, PreparedDatAssetPayload},
    soundfont_json::{SoundFontIndexJson, SOUNDFONT_INDEX_ASSET_TYPE},
};
use flutz_soundfont_tools::soundfont_json_pack::generate_soundfont_json_assets;
use rustystem::SoundFont;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("_local/runtime-tests/retro.sf2"));
    let bytes = fs::read(&path)?;
    let entry = DatAssetEntry {
        internal_id: "probe".to_owned(),
        display_name: "Probe".to_owned(),
        asset_type: "soundfont".to_owned(),
        source_format: "sf2".to_owned(),
        storage_format: "sf2".to_owned(),
        runtime_format: "sf2".to_owned(),
        original_filename: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("probe.sf2")
            .to_owned(),
    };

    let metadata = SoundFont::metadata_only(&mut Cursor::new(&bytes))?;
    let assets = generate_soundfont_json_assets(&entry, &bytes, false)?;
    let index_asset = assets
        .iter()
        .find(|asset| asset.entry.asset_type == SOUNDFONT_INDEX_ASSET_TYPE)
        .ok_or("generated index JSON asset missing")?;
    let PreparedDatAssetPayload::Bytes(index_bytes) = &index_asset.payload else {
        return Err("generated index payload was not bytes".into());
    };
    let index: SoundFontIndexJson = serde_json::from_slice(index_bytes)?;
    let first_preset = index
        .presets
        .first()
        .ok_or("generated index has no presets")?;
    let metadata_closure =
        metadata.closure_for_preset(first_preset.bank as i32, first_preset.program as i32);
    let index_instrument_ids = first_preset
        .instrument_ids
        .iter()
        .map(|id| *id as usize)
        .collect::<Vec<_>>();
    let mut index_sample_ids = Vec::<usize>::new();
    for instrument_id in &first_preset.instrument_ids {
        if let Some(load_map) = index
            .instrument_load_map
            .iter()
            .find(|load_map| load_map.instrument_id == *instrument_id)
        {
            for sample_id in &load_map.sample_ids {
                let sample_id = *sample_id as usize;
                if !index_sample_ids.contains(&sample_id) {
                    index_sample_ids.push(sample_id);
                }
            }
        }
    }
    index_sample_ids.sort_unstable();

    if metadata_closure.instrument_ids != index_instrument_ids {
        return Err(format!(
            "instrument closure mismatch: metadata={:?} index={:?}",
            metadata_closure.instrument_ids, index_instrument_ids
        )
        .into());
    }
    if metadata_closure.sample_ids != index_sample_ids {
        return Err(format!(
            "sample closure mismatch: metadata={:?} index={:?}",
            metadata_closure.sample_ids, index_sample_ids
        )
        .into());
    }

    println!(
        "metadata_index_closure_probe ok: bank={} program={} instruments={} samples={}",
        first_preset.bank,
        first_preset.program,
        index_instrument_ids.len(),
        index_sample_ids.len()
    );
    Ok(())
}
