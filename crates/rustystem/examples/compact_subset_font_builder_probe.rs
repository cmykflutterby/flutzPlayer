use std::{env, fs, io::Cursor, path::PathBuf};

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
    let compact = full.compact_for_preset(preset.get_bank_number(), preset.get_patch_number())?;

    if compact.get_wave_data().is_empty() {
        return Err("compact font has no wave data".into());
    }
    if compact.get_wave_data().len() >= full.get_wave_data().len() {
        return Err(format!(
            "compact font did not reduce wave data: compact={} full={}",
            compact.get_wave_data().len(),
            full.get_wave_data().len()
        )
        .into());
    }
    if compact.get_presets().len() != 1 {
        return Err(format!(
            "compact font should contain exactly one preset, found {}",
            compact.get_presets().len()
        )
        .into());
    }
    if compact.get_instruments().is_empty() || compact.get_sample_headers().is_empty() {
        return Err("compact font did not keep required instruments/samples".into());
    }
    for sample in compact.get_sample_headers() {
        if sample.get_start() < 0
            || sample.get_end() <= sample.get_start()
            || sample.get_end() as usize > compact.get_wave_data().len()
        {
            return Err(format!("compact sample offsets out of bounds: {sample:?}").into());
        }
    }
    for instrument in compact.get_instruments() {
        for region in instrument.get_regions() {
            if region.get_sample_end() as usize > compact.get_wave_data().len()
                || region.get_sample_end() <= region.get_sample_start()
            {
                return Err("compact instrument region offsets out of bounds".into());
            }
        }
    }

    println!(
        "compact_subset_font_builder_probe ok: full_wave_samples={} compact_wave_samples={} presets={} instruments={} samples={}",
        full.get_wave_data().len(),
        compact.get_wave_data().len(),
        compact.get_presets().len(),
        compact.get_instruments().len(),
        compact.get_sample_headers().len()
    );
    Ok(())
}
