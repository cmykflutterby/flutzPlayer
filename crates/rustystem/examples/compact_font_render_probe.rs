use std::{env, fs, io::Cursor, path::PathBuf, sync::Arc};

use rustystem::{SoundFont, Synthesizer, SynthesizerSettings};

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
    let bank = preset.get_bank_number();
    let program = preset.get_patch_number();
    let compact = Arc::new(full.compact_for_preset(bank, program)?);

    let settings = SynthesizerSettings::new(44_100);
    let mut synthesizer = Synthesizer::new(&compact, &settings)?;
    synthesizer.process_midi_message(0, 0xB0, 0, bank & 0x7F);
    synthesizer.process_midi_message(0, 0xC0, program, 0);
    synthesizer.note_on(0, 60, 100);

    let mut left = vec![0.0f32; 2048];
    let mut right = vec![0.0f32; 2048];
    synthesizer.render(&mut left, &mut right);
    let peak = left
        .iter()
        .chain(right.iter())
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return Err("compact font render produced silence".into());
    }

    println!(
        "compact_font_render_probe ok: bank={} program={} peak={:.6} wave_samples={}",
        bank,
        program,
        peak,
        compact.get_wave_data().len()
    );
    Ok(())
}
