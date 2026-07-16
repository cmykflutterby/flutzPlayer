use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = fs::read_to_string("crates/flutz_app/src/app/mod.rs")?;
    let decoded_defaults = function_body(&source, "fn apply_decoded_audio_defaults")?;
    let plain_midi_defaults = function_body(&source, "fn reset_plain_midi_defaults")?;

    if decoded_defaults.contains("final_output_volume_percent = 0.0") {
        return Err("decoded audio defaults reset final output volume percent".into());
    }
    if plain_midi_defaults.contains("final_output_volume_percent = 0.0") {
        return Err("plain MIDI defaults reset final output volume percent".into());
    }
    if !source.contains("self.apply_decoded_audio_defaults(wrapper.loop_region);\n                self.sync_playback_output_volume();") {
        return Err("decoded audio load does not sync preserved final output volume after defaults".into());
    }

    println!(
        "scenario=bug05-volume-slider-sync decoded_defaults_preserve=true midi_defaults_preserve=true decoded_load_resync=true status=ok"
    );
    Ok(())
}

fn function_body<'a>(source: &'a str, signature: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    let signature_start = source
        .find(signature)
        .ok_or_else(|| format!("missing function signature: {signature}"))?;
    let body_start = source[signature_start..]
        .find('{')
        .map(|offset| signature_start + offset)
        .ok_or_else(|| format!("missing function body: {signature}"))?;
    let mut depth = 0usize;
    for (offset, byte) in source[body_start..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(&source[body_start..=body_start + offset]);
                }
            }
            _ => {}
        }
    }
    Err(format!("unterminated function body: {signature}").into())
}
