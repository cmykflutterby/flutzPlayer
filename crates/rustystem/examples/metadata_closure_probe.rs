use std::{env, fs, io::Cursor, path::PathBuf};

use rustystem::SoundFont;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("_local/runtime-tests/retro.sf2"));
    let bytes = fs::read(&path)?;

    let full = SoundFont::new(&mut Cursor::new(&bytes))?;
    let metadata = SoundFont::metadata_only(&mut Cursor::new(&bytes))?;
    let preset = metadata
        .get_presets()
        .first()
        .ok_or("metadata parser found no presets")?;
    let closure = metadata.closure_for_preset(preset.get_bank_number(), preset.get_patch_number());

    if closure.preset_ids != [0] {
        return Err(format!(
            "unexpected first preset closure ids: {:?}",
            closure.preset_ids
        )
        .into());
    }
    if closure.instrument_ids.is_empty() {
        return Err("metadata closure found no instruments for first preset".into());
    }
    if closure.sample_ids.is_empty() {
        return Err("metadata closure found no samples for first preset".into());
    }
    if closure
        .instrument_ids
        .iter()
        .any(|id| *id >= metadata.get_instruments().len())
    {
        return Err("metadata closure returned an out-of-range instrument id".into());
    }
    if closure
        .sample_ids
        .iter()
        .any(|id| *id >= metadata.get_sample_headers().len())
    {
        return Err("metadata closure returned an out-of-range sample id".into());
    }

    let full_preset = full
        .get_presets()
        .first()
        .ok_or("full parser found no presets")?;
    if full_preset.get_bank_number() != preset.get_bank_number()
        || full_preset.get_patch_number() != preset.get_patch_number()
    {
        return Err("metadata closure preset does not match full parser first preset".into());
    }

    println!(
        "metadata_closure_probe ok: bank={} program={} instruments={} samples={}",
        preset.get_bank_number(),
        preset.get_patch_number(),
        closure.instrument_ids.len(),
        closure.sample_ids.len()
    );
    Ok(())
}
