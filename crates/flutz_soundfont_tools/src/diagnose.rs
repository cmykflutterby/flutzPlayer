use super::*;

pub(super) fn diagnose_fmid(args: DiagnoseFmidArgs) -> Result<(), String> {
    let bytes = fs::read(&args.input)
        .map_err(|error| format!("failed to read FMID {}: {error}", args.input.display()))?;
    let fmid = read_fmid(&bytes).map_err(|error| error.to_string())?;
    let preset_set = default_preset_set();

    let (mode_label, requested_preset_id, resolved_preset, soundfont_ids, warning) =
        match &fmid.mixer_source_mode {
            MixerSourceMode::Custom => (
                "custom",
                None,
                None,
                fmid.soundfonts
                    .iter()
                    .map(|slot| slot.internal_id.clone())
                    .collect::<Vec<_>>(),
                None,
            ),
            MixerSourceMode::PresetDefault(preset_id) => {
                let preset = preset_set
                    .find_preset(preset_id)
                    .unwrap_or_else(|| preset_set.default_preset());
                let warning = (preset.id != preset_id).then(|| {
                    format!(
                    "missing_preset_fallback_applied requested_preset_id={} fallback_preset_id={}",
                    quote_field(preset_id),
                    quote_field(preset.id)
                )
                });
                (
                    "preset_default",
                    Some(preset_id.as_str()),
                    Some(preset),
                    preset
                        .font_ids
                        .iter()
                        .map(|font_id| (*font_id).to_owned())
                        .collect::<Vec<_>>(),
                    warning,
                )
            }
        };

    let coverage = match args.dat_input.as_ref() {
        Some(dat_input) => load_soundfont_coverages(dat_input, &soundfont_ids)?,
        None => BTreeMap::new(),
    };

    println!("FMID Diagnostics");
    println!("================");
    println!("Input: {}", args.input.display());
    println!("Mode: {mode_label}");
    if let Some(preset_id) = requested_preset_id {
        println!("Requested preset: {preset_id}");
    }
    if let Some(preset) = resolved_preset {
        println!("Resolved preset: {} ({})", preset.display_name, preset.id);
    }
    if let Some(dat_input) = args.dat_input.as_ref() {
        println!("DAT input: {}", dat_input.display());
    } else {
        println!("DAT input: none; coverage/provider diagnostics are limited");
    }
    if let Some(warning) = &warning {
        println!("Warning: {warning}");
    }
    println!("SoundFonts: {}", soundfont_ids.join(" | "));
    println!("Saved strips: {}", fmid.mixer.strips.len());

    let midi_file = parse_midi_bytes(&fmid.midi_bytes)?;
    print_midi_interpretation_diagnostics("FMID_DIAG", &midi_file);

    println!(
        "FMID_DIAG kind=summary input={} mode={} requested_preset_id={} resolved_preset_id={} soundfont_count={} strip_count={}",
        quote_field(&args.input.display().to_string()),
        quote_field(mode_label),
        requested_preset_id.map(quote_field).unwrap_or_else(|| "-".to_owned()),
        resolved_preset
            .map(|preset| quote_field(preset.id))
            .unwrap_or_else(|| "-".to_owned()),
        soundfont_ids.len(),
        fmid.mixer.strips.len()
    );
    if let Some(warning) = &warning {
        println!("FMID_DIAG kind=warning code={warning}");
    }

    for (index, soundfont_id) in soundfont_ids.iter().enumerate() {
        let coverage = coverage.get(soundfont_id);
        println!(
            "FMID_DIAG kind=soundfont index={} id={} coverage_available={} melodic_presets={} percussion={} samples={}",
            index,
            quote_field(soundfont_id),
            coverage.is_some(),
            coverage.map(|coverage| coverage.melodic.len()).unwrap_or(0),
            coverage
                .map(|coverage| coverage.provides_percussion())
                .unwrap_or(false),
            coverage.map(|coverage| coverage.metadata.sample_count).unwrap_or(0)
        );
    }

    for strip in &fmid.mixer.strips {
        let provider = strip_provider(
            &soundfont_ids,
            &coverage,
            0,
            strip.identity.midi_program as u8,
            strip.identity.is_percussion,
        );
        println!(
            "FMID_DIAG kind=strip channel={} program={} percussion={} source_soundfont_id={} provider_index={} provider_id={} unsupported={} muted={} volume={:.6}",
            strip.identity.midi_channel,
            strip.identity.midi_program,
            strip.identity.is_percussion,
            quote_field(&strip.identity.soundfont_id),
            provider
                .map(|(provider_index, _)| provider_index.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            provider
                .map(|(_, provider_id)| quote_field(provider_id))
                .unwrap_or_else(|| "-".to_owned()),
            args.dat_input.is_some() && provider.is_none(),
            strip.controls.mute,
            strip.controls.volume
        );
        if args.dat_input.is_some() && provider.is_none() {
            let code = if strip.identity.is_percussion {
                "percussion_provider_missing"
            } else {
                "melodic_provider_missing"
            };
            println!(
                "FMID_DIAG kind=warning code={} channel={} bank=0 program={} percussion={}",
                code,
                strip.identity.midi_channel,
                strip.identity.midi_program,
                strip.identity.is_percussion
            );
        }
    }

    for role in midi_file.get_channel_program_roles() {
        let provider = strip_provider(
            &soundfont_ids,
            &coverage,
            role.bank,
            role.program,
            role.is_percussion,
        );
        if args.dat_input.is_some() && provider.is_none() {
            let code = if role.is_percussion {
                "parsed_percussion_provider_missing"
            } else if role.bank != 0 {
                "parsed_melodic_bank_provider_missing"
            } else {
                "parsed_melodic_provider_missing"
            };
            println!(
                "FMID_DIAG kind=warning code={} channel={} bank={} program={} percussion={}",
                code, role.channel, role.bank, role.program, role.is_percussion
            );
        }
    }

    Ok(())
}

pub(super) fn diagnose_midi(args: DiagnoseMidiArgs) -> Result<(), String> {
    let bytes = fs::read(&args.input)
        .map_err(|error| format!("failed to read MIDI {}: {error}", args.input.display()))?;
    let midi_file = parse_midi_bytes(&bytes)?;

    println!("MIDI Diagnostics");
    println!("================");
    println!("Input: {}", args.input.display());
    println!("Duration seconds: {:.6}", midi_file.get_length());
    println!("Tick length: {}", midi_file.get_tick_length().max(0));
    print_midi_interpretation_diagnostics("MIDI_DIAG", &midi_file);

    Ok(())
}

pub(super) fn parse_midi_bytes(bytes: &[u8]) -> Result<MidiFile, String> {
    let mut cursor = Cursor::new(bytes);
    MidiFile::new_with_loop_type(&mut cursor, MidiFileLoopType::LoopPoint(0))
        .map_err(|error| format!("invalid MIDI data: {error:?}"))
}

pub(super) fn print_midi_interpretation_diagnostics(prefix: &str, midi_file: &MidiFile) {
    let interpretation = midi_file.get_interpretation();
    println!(
        "{prefix} kind=midi_summary duration_seconds={:.6} tick_length={} system_modes={} sysex_events={} recognized_sysex_events={} percussion_channels={}",
        midi_file.get_length(),
        midi_file.get_tick_length().max(0),
        quote_field(&system_modes_field(interpretation)),
        interpretation.sysex_event_count,
        interpretation.recognized_sysex_event_count,
        quote_field(&u8_set_field(&interpretation.percussion_channels))
    );
    for warning in &interpretation.warnings {
        println!("{prefix} kind=midi_warning code={}", quote_field(warning));
    }
    for event in &interpretation.sysex_events {
        let (role_channel, role_kind) = event
            .channel_role
            .map(|(channel, role)| (channel.to_string(), format!("{role:?}")))
            .unwrap_or_else(|| ("-".to_owned(), "-".to_owned()));
        println!(
            "{prefix} kind=sysex index={} status=0x{:02X} bytes={} manufacturer={} recognized={} system_mode={} role_channel={} role={} warning={} data={}",
            event.index,
            event.status,
            event.byte_len,
            quote_field(&event.manufacturer_id),
            event.recognized,
            event
                .system_mode
                .map(|mode| quote_field(&format!("{mode:?}")))
                .unwrap_or_else(|| "-".to_owned()),
            role_channel,
            quote_field(&role_kind),
            event
                .warning
                .as_ref()
                .map(|warning| quote_field(warning))
                .unwrap_or_else(|| "-".to_owned()),
            quote_field(&event.bytes_hex)
        );
    }
    for role in midi_file.get_channel_program_roles() {
        println!(
            "{prefix} kind=midi_strip channel={} bank={} program={} percussion={}",
            role.channel, role.bank, role.program, role.is_percussion
        );
    }
}

pub(super) fn system_modes_field(interpretation: &MidiInterpretation) -> String {
    if interpretation.system_modes.is_empty() {
        return "none".to_owned();
    }
    interpretation
        .system_modes
        .iter()
        .map(|mode| format!("{mode:?}"))
        .collect::<Vec<_>>()
        .join("|")
}

pub(super) fn u8_set_field(values: &BTreeSet<u8>) -> String {
    values
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join("|")
}

pub(super) fn load_soundfont_coverages(
    dat_input: &Path,
    requested_soundfonts: &[String],
) -> Result<BTreeMap<String, SoundFontCoverage>, String> {
    let dat_paths = collect_dat_input_paths(dat_input)?;
    let requested = requested_soundfonts
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let mut coverages = BTreeMap::new();
    for asset in collect_extracted_assets(&dat_paths, None)? {
        if asset.entry.asset_type != "soundfont"
            || !requested.contains(asset.entry.internal_id.as_str())
        {
            continue;
        }
        let mut cursor = Cursor::new(asset.bytes);
        match SoundFont::new(&mut cursor) {
            Ok(soundfont) => {
                coverages.insert(
                    asset.entry.internal_id,
                    extract_coverage_from_sf2(&soundfont),
                );
            }
            Err(error) => {
                println!(
                    "FMID_DIAG kind=warning code=coverage_extract_failed soundfont_id={} detail={}",
                    quote_field(&asset.entry.internal_id),
                    quote_field(&format!("{error:?}"))
                );
            }
        }
    }

    for soundfont_id in requested_soundfonts {
        if !coverages.contains_key(soundfont_id) {
            println!(
                "FMID_DIAG kind=warning code=dat_asset_missing soundfont_id={}",
                quote_field(soundfont_id)
            );
        }
    }
    Ok(coverages)
}

pub(super) fn strip_provider<'a>(
    soundfont_ids: &'a [String],
    coverage: &BTreeMap<String, SoundFontCoverage>,
    bank: u16,
    program: u8,
    is_percussion: bool,
) -> Option<(usize, &'a str)> {
    soundfont_ids
        .iter()
        .enumerate()
        .rev()
        .find(|(_, soundfont_id)| {
            coverage.get(*soundfont_id).is_some_and(|coverage| {
                if is_percussion {
                    coverage.provides_percussion()
                } else {
                    coverage.provides_melodic(bank, program)
                }
            })
        })
        .map(|(index, soundfont_id)| (index, soundfont_id.as_str()))
}

pub(super) fn quote_field(value: &str) -> String {
    format!("{:?}", value)
}
