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

    if full.get_info().get_bank_name() != metadata.get_info().get_bank_name() {
        return Err("metadata-only parser did not preserve INFO bank name".into());
    }
    if full.get_bits_per_sample() != metadata.get_bits_per_sample() {
        return Err("metadata-only parser reported different bits per sample".into());
    }
    if full.get_wave_data().len() != metadata.get_sample_data_sample_count() {
        return Err(format!(
            "metadata-only sample count mismatch: full={} metadata={}",
            full.get_wave_data().len(),
            metadata.get_sample_data_sample_count()
        )
        .into());
    }
    if full.get_sample_headers().len() != metadata.get_sample_headers().len() {
        return Err("metadata-only parser reported different sample header count".into());
    }
    if full.get_presets().len() != metadata.get_presets().len() {
        return Err("metadata-only parser reported different preset count".into());
    }
    if full.get_instruments().len() != metadata.get_instruments().len() {
        return Err("metadata-only parser reported different instrument count".into());
    }
    if metadata.get_sample_data_sample_count() == 0 {
        return Err("metadata-only parser reported no sample data".into());
    }

    println!(
        "metadata_only_parser_probe ok: presets={} instruments={} samples={} wave_samples={}",
        metadata.get_presets().len(),
        metadata.get_instruments().len(),
        metadata.get_sample_headers().len(),
        metadata.get_sample_data_sample_count()
    );
    Ok(())
}
