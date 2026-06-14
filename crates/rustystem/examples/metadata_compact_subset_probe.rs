use std::{env, fs, io::Cursor, path::PathBuf, sync::Arc};

use rustystem::{SoundFont, Synthesizer, SynthesizerSettings};

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
        .ok_or("metadata font has no presets")?;
    let bank = preset.get_bank_number();
    let program = preset.get_patch_number();
    let closure = metadata.closure_for_preset(bank, program);
    if closure.sample_ids.is_empty() {
        return Err("metadata closure selected no samples".into());
    }

    let mut compact_wave_data = Vec::new();
    for sample_id in &closure.sample_ids {
        let header = metadata
            .get_sample_headers()
            .get(*sample_id)
            .ok_or("metadata closure referenced missing sample")?;
        if header.get_start() < 0
            || header.get_end() <= header.get_start()
            || header.get_end() as usize > full.get_wave_data().len()
        {
            return Err(format!("source sample range is out of bounds: {header:?}").into());
        }
        compact_wave_data.extend_from_slice(
            &full.get_wave_data()[header.get_start() as usize..header.get_end() as usize],
        );
    }

    let full_compact = full.compact_for_preset(bank, program)?;
    let metadata_compact =
        metadata.compact_from_closure_and_wave_data(&closure, compact_wave_data)?;

    if metadata_compact.get_wave_data() != full_compact.get_wave_data() {
        return Err("metadata compact wave data differs from full compact wave data".into());
    }
    if metadata_compact.get_sample_headers().len() != full_compact.get_sample_headers().len() {
        return Err("metadata compact sample header count differs from full compact".into());
    }
    if metadata_compact.get_instruments().len() != full_compact.get_instruments().len() {
        return Err("metadata compact instrument count differs from full compact".into());
    }
    if metadata_compact.get_presets().len() != full_compact.get_presets().len() {
        return Err("metadata compact preset count differs from full compact".into());
    }

    let compact_wave_len = full_compact.get_wave_data().len();
    let metadata_peak = render_peak(Arc::new(metadata_compact), bank, program)?;
    let full_peak = render_peak(Arc::new(full_compact), bank, program)?;
    if metadata_peak <= 0.0 || full_peak <= 0.0 {
        return Err("compact render produced silence".into());
    }

    println!(
        "metadata_compact_subset_probe ok: bank={} program={} samples={} wave_samples={} metadata_peak={:.6} full_peak={:.6}",
        bank,
        program,
        closure.sample_ids.len(),
        compact_wave_len,
        metadata_peak,
        full_peak,
    );
    Ok(())
}

fn render_peak(
    soundfont: Arc<SoundFont>,
    bank: i32,
    program: i32,
) -> Result<f32, Box<dyn std::error::Error>> {
    let settings = SynthesizerSettings::new(44_100);
    let mut synthesizer = Synthesizer::new(&soundfont, &settings)?;
    synthesizer.process_midi_message(0, 0xB0, 0, bank & 0x7F);
    synthesizer.process_midi_message(0, 0xC0, program, 0);
    synthesizer.note_on(0, 60, 100);

    let mut left = vec![0.0f32; 2048];
    let mut right = vec![0.0f32; 2048];
    synthesizer.render(&mut left, &mut right);
    Ok(left
        .iter()
        .chain(right.iter())
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max))
}
