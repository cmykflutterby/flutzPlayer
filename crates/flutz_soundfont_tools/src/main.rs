use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use flutz_core::default_preset_set;
use flutz_dat::{
    assets::{DatAssetEntry, PreparedDatAsset, PreparedDatAssetPayload, DAT_ENTRY_FLAG_DEFAULT},
    manifest::parse_dat_manifest,
    read::{
        extract_all_entries, parse_dat_index_file, read_dat_file, read_entry_range_plan,
        read_soundfont_json_resources_from_file, DatArchiveIndex, DatEntryRange, DatEntryRangePlan,
        SoundFontJsonResources,
    },
    soundfont_json::{
        from_json_bytes, SoundFontCoverageJson, SoundFontIndexJson, SoundFontMetadataJson,
        SoundFontPackReportJson, SOUNDFONT_COVERAGE_ASSET_TYPE, SOUNDFONT_INDEX_ASSET_TYPE,
        SOUNDFONT_METADATA_ASSET_TYPE, SOUNDFONT_PACK_REPORT_ASSET_TYPE,
    },
    write::{write_dat_archive_files, DEFAULT_CHUNK_SIZE, DEFAULT_DAT_FILE_SIZE},
};
use flutz_fmid::{read_fmid, MixerSourceMode};
use flutz_soundfont_tools::{
    cli::{
        help_text, parse_env_args, CliCommand, ConvertArgs, DiagnoseFmidArgs, DiagnoseMidiArgs,
        ExtractArgs, InspectArgs, InspectFormat, PackArgs,
    },
    sfark::{decode_sfark_file_to_sf2, decode_sfark_to_sf2_diagnostics_with_progress},
    soundfont_json_pack::generate_soundfont_json_assets,
};
use flutz_synth::{extract_coverage_from_sf2, SoundFontCoverage};
use rustystem::{MidiFile, MidiFileLoopType, MidiInterpretation, SoundFont};

fn main() {
    match parse_env_args() {
        Ok(CliCommand::Help) => println!("{}", help_text()),
        Ok(CliCommand::Convert(args)) => match convert_soundfont(args) {
            Ok(()) => {}
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(2);
            }
        },
        Ok(CliCommand::DiagnoseFmid(args)) => match diagnose_fmid(args) {
            Ok(()) => {}
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(2);
            }
        },
        Ok(CliCommand::DiagnoseMidi(args)) => match diagnose_midi(args) {
            Ok(()) => {}
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(2);
            }
        },
        Ok(CliCommand::Extract(args)) => match extract_dat(args) {
            Ok(()) => {}
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(2);
            }
        },
        Ok(CliCommand::Inspect(args)) => match inspect_dat(args) {
            Ok(()) => {}
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(2);
            }
        },
        Ok(CliCommand::Pack(args)) => match pack_dat(args) {
            Ok(()) => {}
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(2);
            }
        },
        Err(message) => {
            eprintln!("{message}\n\n{}", help_text());
            std::process::exit(1);
        }
    }
}

fn diagnose_fmid(args: DiagnoseFmidArgs) -> Result<(), String> {
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

fn diagnose_midi(args: DiagnoseMidiArgs) -> Result<(), String> {
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

fn parse_midi_bytes(bytes: &[u8]) -> Result<MidiFile, String> {
    let mut cursor = Cursor::new(bytes);
    MidiFile::new_with_loop_type(&mut cursor, MidiFileLoopType::LoopPoint(0))
        .map_err(|error| format!("invalid MIDI data: {error:?}"))
}

fn print_midi_interpretation_diagnostics(prefix: &str, midi_file: &MidiFile) {
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

fn system_modes_field(interpretation: &MidiInterpretation) -> String {
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

fn u8_set_field(values: &BTreeSet<u8>) -> String {
    values
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join("|")
}

fn load_soundfont_coverages(
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

fn strip_provider<'a>(
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

fn quote_field(value: &str) -> String {
    format!("{:?}", value)
}

fn pack_dat(args: PackArgs) -> Result<(), String> {
    if let Some(registry_path) = args.registry.as_ref() {
        eprintln!(
            "warning: --registry {} is deprecated and ignored; DAT pack metadata now comes only from the manifest",
            registry_path.display()
        );
    }

    let manifest_text = fs::read_to_string(&args.manifest).map_err(|error| {
        format!(
            "failed to read DAT manifest {}: {error}",
            args.manifest.display()
        )
    })?;
    let manifest = parse_dat_manifest(&manifest_text).map_err(|error| error.to_string())?;
    let generate_soundfont_json = manifest.generate_soundfont_json;
    let generate_soundfont_pack_report = manifest.generate_soundfont_pack_report;

    let default_soundfont_id = manifest.default_soundfont_id.as_deref();
    if let Some(default_soundfont_id) = default_soundfont_id {
        let default_exists = manifest.assets.iter().any(|asset| {
            asset.include_in_dat
                && asset.asset_type == "soundfont"
                && asset.internal_id == default_soundfont_id
        });
        if !default_exists {
            return Err(format!(
                "default soundfont ID {default_soundfont_id} is not a pack-enabled soundfont in the DAT manifest"
            ));
        }
    }

    let mut included_assets = manifest
        .assets
        .into_iter()
        .filter(|asset| asset.include_in_dat)
        .collect::<Vec<_>>();

    if included_assets.is_empty() {
        return Err("DAT manifest has no pack-enabled assets".to_owned());
    }

    // Sort assets according to the packing order from manifest
    use flutz_dat::manifest::PackingOrder;
    if manifest.packing_order == PackingOrder::SmallestFirst {
        included_assets.sort_by_key(|asset| {
            std::fs::metadata(args.base_dir.join(&asset.source_path))
                .map(|meta| meta.len())
                .unwrap_or(u64::MAX)
        });
        println!(
            "packing {} manifest entries in smallest-first order",
            included_assets.len()
        );
    } else {
        println!(
            "packing {} manifest entries in manifest order",
            included_assets.len()
        );
    }

    let mut assets = Vec::with_capacity(included_assets.len());

    for asset in included_assets {
        let source_path = args.base_dir.join(&asset.source_path);
        println!(
            "packing {} from {}",
            asset.internal_id,
            source_path.display()
        );
        let payload = prepare_runtime_asset_payload(&source_path, &asset.asset_type)?;
        let runtime_format = runtime_format_for_source(&source_path, &asset.asset_type)
            .unwrap_or_else(|| asset.runtime_format.clone());
        let storage_format = storage_format_for_source(&source_path, &asset.asset_type)
            .unwrap_or_else(|| asset.source_format.clone());
        let original_filename = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("source path has no filename: {}", source_path.display()))?
            .to_owned();
        let is_default = default_soundfont_id == Some(asset.internal_id.as_str());
        let raw_asset = PreparedDatAsset {
            flags: if is_default {
                DAT_ENTRY_FLAG_DEFAULT
            } else {
                0
            },
            entry: DatAssetEntry {
                internal_id: asset.internal_id,
                display_name: asset.display_name,
                asset_type: asset.asset_type,
                source_format: asset.source_format,
                storage_format,
                runtime_format,
                original_filename,
            },
            payload,
        };

        let generated_json_assets =
            if generate_soundfont_json && raw_asset.entry.asset_type == "soundfont" {
                let runtime_bytes = payload_bytes(&raw_asset.payload)?;
                let generated = generate_soundfont_json_assets(
                    &raw_asset.entry,
                    &runtime_bytes,
                    generate_soundfont_pack_report,
                )?;
                println!(
                    "  generated {} soundfont JSON resource(s) for {}",
                    generated.len(),
                    raw_asset.entry.internal_id
                );
                generated
            } else {
                Vec::new()
            };

        assets.push(raw_asset);
        assets.extend(generated_json_assets);
    }

    let expected_assets = assets.clone();
    let report = write_dat_archive_files(
        assets,
        &args.output,
        args.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE),
        args.max_file_size.unwrap_or(DEFAULT_DAT_FILE_SIZE),
    )
    .map_err(|error| error.to_string())?;
    println!(
        "wrote {} entries in {} chunks across {} DAT file(s) ({} bytes total)",
        report.entry_count,
        report.chunk_count,
        report.output_files.len(),
        report.byte_count
    );
    for file in &report.output_files {
        println!(
            "  wrote {} entries in {} chunks to {} ({} bytes)",
            file.entry_count,
            file.chunk_count,
            file.output_path.display(),
            file.byte_count
        );
    }
    let output_paths = report
        .output_files
        .iter()
        .map(|file| file.output_path.clone())
        .collect::<Vec<_>>();
    validate_dat_payload_roundtrip(&expected_assets, &output_paths)?;
    println!(
        "validated DAT payload byte identity for {} entries",
        expected_assets.len()
    );
    let generated_json_count = validate_generated_soundfont_json_resources(&output_paths)?;
    if generated_json_count > 0 {
        println!(
            "validated {} generated soundfont JSON resource(s)",
            generated_json_count
        );
    }
    let semantic_count =
        validate_generated_soundfont_json_semantics(&expected_assets, &output_paths)?;
    if semantic_count > 0 {
        println!(
            "validated generated soundfont JSON semantics for {} soundfont(s)",
            semantic_count
        );
    }
    Ok(())
}

fn inspect_dat(args: InspectArgs) -> Result<(), String> {
    let dat_paths = collect_dat_input_paths(&args.input)?;
    let mut fonts = BTreeMap::<String, FontMetadataSummary>::new();
    let mut dat_count = 0usize;
    let mut dat_bytes = 0u64;

    for dat_path in &dat_paths {
        dat_count += 1;
        dat_bytes += fs::metadata(dat_path)
            .map_err(|error| format!("failed to inspect DAT file {}: {error}", dat_path.display()))?
            .len();
        let index = parse_dat_index_file(dat_path).map_err(|error| error.to_string())?;

        for record in &index.entries {
            if record.entry.asset_type != "soundfont" {
                continue;
            }
            let resources = read_soundfont_json_resources_from_file(
                dat_path,
                &index,
                &record.entry.internal_id,
            )
            .map_err(|error| error.to_string())?;
            let location = RawSoundFontLocation {
                dat_path: dat_path.clone(),
                index: index.clone(),
                record: record.clone(),
            };
            fonts
                .entry(record.entry.internal_id.clone())
                .and_modify(|summary| {
                    summary.add_part(record, dat_path, &resources, location.clone())
                })
                .or_insert_with(|| {
                    FontMetadataSummary::from_part(record, dat_path, &resources, location)
                });
        }
    }

    let selected_fonts = fonts
        .values()
        .filter(|font| {
            args.inspect_font
                .as_deref()
                .map(|id| id == font.internal_id)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if args.inspect_font.is_some() && selected_fonts.is_empty() {
        return Err(format!(
            "DAT input contains no soundfont {}",
            args.inspect_font.as_deref().unwrap_or_default()
        ));
    }

    if args.format == InspectFormat::Json {
        print_inspect_json(&args, dat_count, dat_bytes, &selected_fonts)?;
        return Ok(());
    }

    println!("DAT SoundFont Metadata");
    println!("======================");
    println!("Input: {}", args.input.display());
    println!("DAT files: {}", dat_count);
    println!("DAT bytes: {}", format_bytes(dat_bytes));
    println!("SoundFonts: {}", selected_fonts.len());

    if selected_fonts.is_empty() {
        println!();
        println!("No soundfont entries found.");
        return Ok(());
    }

    for font in selected_fonts {
        println!();
        println!(
            "{}{}",
            font.internal_id,
            if font.is_default { " [default]" } else { "" }
        );
        println!("  Display name:      {}", font.display_name);
        println!("  Asset type:        {}", font.asset_type);
        println!("  Source format:     {}", font.source_format);
        println!("  Storage format:    {}", font.storage_format);
        println!("  Runtime format:    {}", font.runtime_format);
        println!("  Original filename: {}", font.original_filename);
        println!("  Total bytes:       {}", format_bytes(font.total_size));
        println!("  DAT parts:         {}", font.part_count);
        println!("  Chunk extents:     {}", font.extent_count);
        println!("  Flags:             0x{:016x}", font.flags);
        println!("  Files:             {}", font.files.join(", "));
        println!("  JSON resources:");
        println!(
            "    Metadata:        {}",
            json_status(font.metadata.as_ref())
        );
        println!(
            "    Coverage:        {}",
            json_status(font.coverage.as_ref())
        );
        println!("    Index:           {}", json_status(font.index.as_ref()));
        println!(
            "    Pack report:     {}",
            json_status(font.pack_report.as_ref())
        );
        if let Some(metadata) = &font.metadata {
            println!("  Metadata presets:  {}", metadata.preset_count);
            println!("  Metadata samples:  {}", metadata.sample_count);
        }
        if let Some(coverage) = &font.coverage {
            println!("  Coverage presets:  {}", coverage.presets.len());
            println!("  Coverage melodic:  {}", coverage.melodic.len());
            println!("  Coverage drums:    {}", coverage.percussion);
        }
        if let Some(index) = &font.index {
            println!("  Index presets:     {}", index.presets.len());
            println!("  Index instruments: {}", index.instruments.len());
            println!("  Index samples:     {}", index.samples.len());
        }
        if args.dump_coverage {
            print_json_block("Coverage JSON", font.coverage.as_ref())?;
        }
        if args.dump_index {
            print_json_block("Index JSON", font.index.as_ref())?;
        }
        if args.dump_pack_report {
            print_json_block("Pack-report JSON", font.pack_report.as_ref())?;
        }
        print_selected_location_report(&args, font)?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FontMetadataSummary {
    internal_id: String,
    display_name: String,
    asset_type: String,
    source_format: String,
    storage_format: String,
    runtime_format: String,
    original_filename: String,
    flags: u64,
    total_size: u64,
    part_count: u64,
    extent_count: u64,
    files: Vec<String>,
    is_default: bool,
    metadata: Option<SoundFontMetadataJson>,
    coverage: Option<SoundFontCoverageJson>,
    index: Option<SoundFontIndexJson>,
    pack_report: Option<SoundFontPackReportJson>,
    raw_locations: Vec<RawSoundFontLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawSoundFontLocation {
    dat_path: PathBuf,
    index: DatArchiveIndex,
    record: flutz_dat::assets::DatEntryRecord,
}

impl FontMetadataSummary {
    fn from_part(
        record: &flutz_dat::assets::DatEntryRecord,
        dat_path: &Path,
        resources: &SoundFontJsonResources,
        location: RawSoundFontLocation,
    ) -> Self {
        let mut summary = Self {
            internal_id: record.entry.internal_id.clone(),
            display_name: record.entry.display_name.clone(),
            asset_type: record.entry.asset_type.clone(),
            source_format: record.entry.source_format.clone(),
            storage_format: record.entry.storage_format.clone(),
            runtime_format: record.entry.runtime_format.clone(),
            original_filename: record.entry.original_filename.clone(),
            flags: 0,
            total_size: 0,
            part_count: 0,
            extent_count: 0,
            files: Vec::new(),
            is_default: false,
            metadata: None,
            coverage: None,
            index: None,
            pack_report: None,
            raw_locations: Vec::new(),
        };
        summary.add_part(record, dat_path, resources, location);
        summary
    }

    fn add_part(
        &mut self,
        record: &flutz_dat::assets::DatEntryRecord,
        dat_path: &Path,
        resources: &SoundFontJsonResources,
        location: RawSoundFontLocation,
    ) {
        self.flags |= record.flags;
        self.total_size += record.total_size;
        self.part_count += 1;
        self.extent_count += record.extents.len() as u64;
        self.is_default = self.is_default || record.flags & DAT_ENTRY_FLAG_DEFAULT != 0;

        let file_name = dat_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| dat_path.display().to_string());
        if self.files.last() != Some(&file_name) {
            self.files.push(file_name);
        }
        merge_optional(&mut self.metadata, &resources.metadata);
        merge_optional(&mut self.coverage, &resources.coverage);
        merge_optional(&mut self.index, &resources.index);
        merge_optional(&mut self.pack_report, &resources.pack_report);
        self.raw_locations.push(location);
    }
}

fn merge_optional<T: Clone>(target: &mut Option<T>, source: &Option<T>) {
    if target.is_none() {
        *target = source.clone();
    }
}

fn json_status<T>(value: Option<&T>) -> &'static str {
    if value.is_some() {
        "available"
    } else {
        "unavailable"
    }
}

fn print_json_block<T>(label: &str, value: Option<&T>) -> Result<(), String>
where
    T: serde::Serialize,
{
    println!("  {label}:");
    let Some(value) = value else {
        println!("    unavailable");
        return Ok(());
    };
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("failed to serialize {label}: {error}"))?;
    for line in text.lines() {
        println!("    {line}");
    }
    Ok(())
}

fn print_inspect_json(
    args: &InspectArgs,
    dat_count: usize,
    dat_bytes: u64,
    fonts: &[&FontMetadataSummary],
) -> Result<(), String> {
    let fonts_json = fonts
        .iter()
        .map(|font| {
            let metadata = if args.dump_index || args.dump_coverage || args.dump_pack_report {
                font.metadata.as_ref().map(serde_json::to_value).transpose()
            } else {
                Ok(None)
            }
            .map_err(|error| format!("failed to serialize metadata JSON: {error}"))?;
            let coverage = if args.dump_coverage {
                font.coverage.as_ref().map(serde_json::to_value).transpose()
            } else {
                Ok(None)
            }
            .map_err(|error| format!("failed to serialize coverage JSON: {error}"))?;
            let index = if args.dump_index {
                font.index.as_ref().map(serde_json::to_value).transpose()
            } else {
                Ok(None)
            }
            .map_err(|error| format!("failed to serialize index JSON: {error}"))?;
            let pack_report = if args.dump_pack_report {
                font.pack_report
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
            } else {
                Ok(None)
            }
            .map_err(|error| format!("failed to serialize pack-report JSON: {error}"))?;
            Ok(serde_json::json!({
                "internal_id": font.internal_id,
                "display_name": font.display_name,
                "source_format": font.source_format,
                "storage_format": font.storage_format,
                "runtime_format": font.runtime_format,
                "original_filename": font.original_filename,
                "is_default": font.is_default,
                "total_size": font.total_size,
                "part_count": font.part_count,
                "extent_count": font.extent_count,
                "files": font.files,
                "json_resources": {
                    "metadata": font.metadata.is_some(),
                    "coverage": font.coverage.is_some(),
                    "index": font.index.is_some(),
                    "pack_report": font.pack_report.is_some(),
                },
                "metadata": metadata,
                "coverage": coverage,
                "index": index,
                "pack_report": pack_report,
                "selection": selected_location_json(args, font)?,
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let output = serde_json::json!({
        "dat_file_count": dat_count,
        "dat_byte_count": dat_bytes,
        "soundfont_count": fonts.len(),
        "soundfonts": fonts_json,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&output)
            .map_err(|error| format!("failed to serialize inspect JSON: {error}"))?
    );
    Ok(())
}

fn print_selected_location_report(
    args: &InspectArgs,
    font: &FontMetadataSummary,
) -> Result<(), String> {
    if args.preset.is_none() && args.instrument.is_none() && args.sample.is_none() {
        return Ok(());
    }
    let Some(index) = &font.index else {
        println!("  Selection:         unavailable; index JSON missing or malformed");
        return Ok(());
    };
    println!("  Selection:");
    let sample_ids = selected_sample_ids(args, index)?;
    if let Some(preset_id) = args.preset {
        if let Some(preset) = index
            .presets
            .iter()
            .find(|preset| preset.preset_id == preset_id)
        {
            println!(
                "    Preset:          {} ({}) bank={} program={}",
                preset.preset_id, preset.name, preset.bank, preset.program
            );
        }
    }
    if let Some(instrument_id) = args.instrument {
        if let Some(instrument) = index
            .instruments
            .iter()
            .find(|instrument| instrument.instrument_id == instrument_id)
        {
            println!(
                "    Instrument:      {} ({}) regions={}",
                instrument.instrument_id,
                instrument.name,
                instrument.regions.len()
            );
        }
        if let Some(load_map) = index
            .instrument_load_map
            .iter()
            .find(|load_map| load_map.instrument_id == instrument_id)
        {
            println!("    Load samples:    {:?}", load_map.sample_ids);
            println!("    Load regions:    {:?}", load_map.region_ids);
        }
    }
    for sample_id in sample_ids {
        print_sample_location(font, index, sample_id)?;
    }
    Ok(())
}

fn selected_location_json(
    args: &InspectArgs,
    font: &FontMetadataSummary,
) -> Result<Option<serde_json::Value>, String> {
    if args.preset.is_none() && args.instrument.is_none() && args.sample.is_none() {
        return Ok(None);
    }
    let Some(index) = &font.index else {
        return Ok(Some(serde_json::json!({ "available": false })));
    };
    let sample_ids = selected_sample_ids(args, index)?;
    let samples = sample_ids
        .into_iter()
        .map(|sample_id| sample_location_json(font, index, sample_id))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Some(serde_json::json!({
        "available": true,
        "preset": args.preset,
        "instrument": args.instrument,
        "sample": args.sample,
        "samples": samples,
    })))
}

fn selected_sample_ids(args: &InspectArgs, index: &SoundFontIndexJson) -> Result<Vec<u32>, String> {
    let mut sample_ids = BTreeSet::<u32>::new();
    if let Some(sample_id) = args.sample {
        sample_ids.insert(sample_id);
    }
    if let Some(instrument_id) = args.instrument {
        let load_map = index
            .instrument_load_map
            .iter()
            .find(|load_map| load_map.instrument_id == instrument_id)
            .ok_or_else(|| format!("index JSON has no instrument load map for {instrument_id}"))?;
        sample_ids.extend(load_map.sample_ids.iter().copied());
    }
    if let Some(preset_id) = args.preset {
        let load_map = index
            .preset_load_map
            .iter()
            .find(|load_map| load_map.preset_id == preset_id)
            .ok_or_else(|| format!("index JSON has no preset load map for {preset_id}"))?;
        for instrument_id in &load_map.instrument_ids {
            let instrument = index
                .instrument_load_map
                .iter()
                .find(|load_map| load_map.instrument_id == *instrument_id)
                .ok_or_else(|| {
                    format!("index JSON has no instrument load map for {instrument_id}")
                })?;
            sample_ids.extend(instrument.sample_ids.iter().copied());
        }
    }
    Ok(sample_ids.into_iter().collect())
}

fn print_sample_location(
    font: &FontMetadataSummary,
    index: &SoundFontIndexJson,
    sample_id: u32,
) -> Result<(), String> {
    let sample = index
        .samples
        .iter()
        .find(|sample| sample.sample_id == sample_id)
        .ok_or_else(|| format!("index JSON has no sample {sample_id}"))?;
    println!(
        "    Sample:          {} ({}) start={} end={} loop={}..{}",
        sample.sample_id, sample.name, sample.start, sample.end, sample.start_loop, sample.end_loop
    );
    println!(
        "      Wave bytes:    {}..{} ({} bytes)",
        sample.wave_range.smpl_start_byte,
        sample
            .wave_range
            .smpl_start_byte
            .saturating_add(sample.wave_range.byte_length),
        sample.wave_range.byte_length
    );
    for planned in sample_location_plans(
        font,
        sample.wave_range.smpl_start_byte,
        sample.wave_range.byte_length,
    )? {
        println!(
            "      DAT range:     file={} chunk={} file_offset={} extent_offset={} length={}",
            planned.dat_file,
            planned.chunk_id,
            planned.file_offset,
            planned.extent_offset,
            planned.length
        );
    }
    Ok(())
}

fn sample_location_json(
    font: &FontMetadataSummary,
    index: &SoundFontIndexJson,
    sample_id: u32,
) -> Result<serde_json::Value, String> {
    let sample = index
        .samples
        .iter()
        .find(|sample| sample.sample_id == sample_id)
        .ok_or_else(|| format!("index JSON has no sample {sample_id}"))?;
    let physical_ranges = sample_location_plans(
        font,
        sample.wave_range.smpl_start_byte,
        sample.wave_range.byte_length,
    )?;
    Ok(serde_json::json!({
        "sample_id": sample.sample_id,
        "name": sample.name,
        "start": sample.start,
        "end": sample.end,
        "start_loop": sample.start_loop,
        "end_loop": sample.end_loop,
        "wave_range": sample.wave_range,
        "dat_ranges": physical_ranges.iter().map(|range| serde_json::json!({
            "dat_file": range.dat_file,
            "chunk_id": range.chunk_id,
            "file_offset": range.file_offset,
            "extent_offset": range.extent_offset,
            "length": range.length,
        })).collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SamplePhysicalRange {
    dat_file: String,
    chunk_id: u64,
    file_offset: u64,
    extent_offset: u64,
    length: u64,
}

fn sample_location_plans(
    font: &FontMetadataSummary,
    offset: u64,
    length: u64,
) -> Result<Vec<SamplePhysicalRange>, String> {
    let mut ranges = Vec::new();
    for location in &font.raw_locations {
        let plans = plan_range_for_location(location, offset, length)?;
        ranges.extend(plans.into_iter().map(|plan| SamplePhysicalRange {
            dat_file: location.dat_path.display().to_string(),
            chunk_id: plan.chunk_id,
            file_offset: plan.file_offset,
            extent_offset: plan.offset_in_chunk,
            length: plan.length,
        }));
    }
    Ok(ranges)
}

fn plan_range_for_location(
    location: &RawSoundFontLocation,
    offset: u64,
    length: u64,
) -> Result<Vec<DatEntryRangePlan>, String> {
    if length == 0 || offset >= location.record.total_size {
        return Ok(Vec::new());
    }
    let clamped_length = length.min(location.record.total_size - offset);
    read_entry_range_plan(
        &location.index,
        &location.record,
        &[DatEntryRange {
            offset,
            length: clamped_length,
        }],
    )
    .map_err(|error| error.to_string())
}

fn format_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;

    if bytes >= MIB {
        format!("{} bytes ({:.2} MiB)", bytes, bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{} bytes ({:.2} KiB)", bytes, bytes as f64 / KIB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

fn extract_dat(args: ExtractArgs) -> Result<(), String> {
    let dat_paths = collect_dat_input_paths(&args.input)?;
    fs::create_dir_all(&args.output_dir).map_err(|error| {
        format!(
            "failed to create extraction directory {}: {error}",
            args.output_dir.display()
        )
    })?;

    let assets = collect_extracted_assets(&dat_paths, args.internal_id.as_deref())?;
    if assets.is_empty() {
        let target = args.internal_id.as_deref().unwrap_or("any entries");
        return Err(format!(
            "DAT input contains no matching entries for {target}"
        ));
    }

    for asset in assets {
        let path = args.output_dir.join(extracted_filename(&asset.entry));
        fs::write(&path, &asset.bytes).map_err(|error| {
            format!(
                "failed to write extracted entry {}: {error}",
                path.display()
            )
        })?;
        println!(
            "extracted {}{} to {} ({} bytes from {} DAT file(s))",
            asset.entry.internal_id,
            if asset.flags & DAT_ENTRY_FLAG_DEFAULT != 0 {
                " [default]"
            } else {
                ""
            },
            path.display(),
            asset.bytes.len(),
            dat_paths.len()
        );
    }

    Ok(())
}

struct ReconstructedDatAsset {
    entry: DatAssetEntry,
    flags: u64,
    bytes: Vec<u8>,
}

fn validate_dat_payload_roundtrip(
    expected_assets: &[PreparedDatAsset],
    dat_paths: &[PathBuf],
) -> Result<(), String> {
    let mut actual_assets = collect_extracted_assets(dat_paths, None)?
        .into_iter()
        .map(|asset| (asset.entry.internal_id.clone(), asset))
        .collect::<BTreeMap<_, _>>();

    for expected in expected_assets {
        let expected_bytes = payload_bytes(&expected.payload)?;
        let Some(actual) = actual_assets.remove(&expected.entry.internal_id) else {
            return Err(format!(
                "DAT roundtrip validation failed: missing entry {}",
                expected.entry.internal_id
            ));
        };
        if expected.entry != actual.entry {
            return Err(format!(
                "DAT roundtrip validation failed: metadata mismatch for {}",
                expected.entry.internal_id
            ));
        }
        if expected.flags != actual.flags {
            return Err(format!(
                "DAT roundtrip validation failed: flags mismatch for {}",
                expected.entry.internal_id
            ));
        }
        if expected_bytes != actual.bytes {
            return Err(format!(
                "DAT roundtrip validation failed: payload bytes differ for {}; expected {} bytes, extracted {} bytes",
                expected.entry.internal_id,
                expected_bytes.len(),
                actual.bytes.len()
            ));
        }
    }

    if !actual_assets.is_empty() {
        let extra_ids = actual_assets.into_keys().collect::<Vec<_>>().join(", ");
        return Err(format!(
            "DAT roundtrip validation failed: unexpected extracted entries {extra_ids}"
        ));
    }

    Ok(())
}

fn validate_generated_soundfont_json_resources(dat_paths: &[PathBuf]) -> Result<usize, String> {
    let assets = collect_extracted_assets(dat_paths, None)?;
    let raw_soundfont_ids = assets
        .iter()
        .filter(|asset| asset.entry.asset_type == "soundfont")
        .map(|asset| asset.entry.internal_id.clone())
        .collect::<BTreeSet<_>>();
    let generated_ids = assets
        .iter()
        .filter(|asset| is_generated_soundfont_json_asset_type(&asset.entry.asset_type))
        .map(|asset| asset.entry.internal_id.clone())
        .collect::<BTreeSet<_>>();

    let mut generated_count = 0usize;
    for asset in assets
        .iter()
        .filter(|asset| is_generated_soundfont_json_asset_type(&asset.entry.asset_type))
    {
        generated_count += 1;
        match asset.entry.asset_type.as_str() {
            SOUNDFONT_METADATA_ASSET_TYPE => {
                let metadata =
                    from_json_bytes::<SoundFontMetadataJson>(&asset.bytes).map_err(|error| {
                        format!(
                            "failed to parse generated JSON resource {}: {error}",
                            asset.entry.internal_id
                        )
                    })?;
                validate_parent_soundfont(
                    asset,
                    &metadata.parent_soundfont_id,
                    &raw_soundfont_ids,
                )?;
                validate_generated_resource_link(
                    asset,
                    &metadata.generated_resources.metadata_internal_id,
                    &generated_ids,
                )?;
                validate_generated_resource_link(
                    asset,
                    &metadata.generated_resources.coverage_internal_id,
                    &generated_ids,
                )?;
                validate_generated_resource_link(
                    asset,
                    &metadata.generated_resources.index_internal_id,
                    &generated_ids,
                )?;
                if let Some(pack_report_internal_id) = metadata
                    .generated_resources
                    .pack_report_internal_id
                    .as_deref()
                {
                    validate_generated_resource_link(
                        asset,
                        pack_report_internal_id,
                        &generated_ids,
                    )?;
                }
            }
            SOUNDFONT_COVERAGE_ASSET_TYPE => {
                let coverage =
                    from_json_bytes::<SoundFontCoverageJson>(&asset.bytes).map_err(|error| {
                        format!(
                            "failed to parse generated JSON resource {}: {error}",
                            asset.entry.internal_id
                        )
                    })?;
                validate_parent_soundfont(
                    asset,
                    &coverage.parent_soundfont_id,
                    &raw_soundfont_ids,
                )?;
            }
            SOUNDFONT_INDEX_ASSET_TYPE => {
                let index =
                    from_json_bytes::<SoundFontIndexJson>(&asset.bytes).map_err(|error| {
                        format!(
                            "failed to parse generated JSON resource {}: {error}",
                            asset.entry.internal_id
                        )
                    })?;
                validate_parent_soundfont(asset, &index.parent_soundfont_id, &raw_soundfont_ids)?;
                validate_soundfont_index_json(asset, &index)?;
            }
            SOUNDFONT_PACK_REPORT_ASSET_TYPE => {
                let report =
                    from_json_bytes::<SoundFontPackReportJson>(&asset.bytes).map_err(|error| {
                        format!(
                            "failed to parse generated JSON resource {}: {error}",
                            asset.entry.internal_id
                        )
                    })?;
                validate_parent_soundfont(asset, &report.parent_soundfont_id, &raw_soundfont_ids)?;
            }
            _ => {}
        }
    }

    Ok(generated_count)
}

fn is_generated_soundfont_json_asset_type(asset_type: &str) -> bool {
    matches!(
        asset_type,
        SOUNDFONT_METADATA_ASSET_TYPE
            | SOUNDFONT_COVERAGE_ASSET_TYPE
            | SOUNDFONT_INDEX_ASSET_TYPE
            | SOUNDFONT_PACK_REPORT_ASSET_TYPE
    )
}

fn validate_parent_soundfont(
    asset: &ReconstructedDatAsset,
    parent_soundfont_id: &str,
    raw_soundfont_ids: &BTreeSet<String>,
) -> Result<(), String> {
    if raw_soundfont_ids.contains(parent_soundfont_id) {
        Ok(())
    } else {
        Err(format!(
            "generated JSON resource {} references missing parent soundfont {}",
            asset.entry.internal_id, parent_soundfont_id
        ))
    }
}

fn validate_generated_resource_link(
    asset: &ReconstructedDatAsset,
    internal_id: &str,
    generated_ids: &BTreeSet<String>,
) -> Result<(), String> {
    if generated_ids.contains(internal_id) {
        Ok(())
    } else {
        Err(format!(
            "generated JSON resource {} references missing generated resource {}",
            asset.entry.internal_id, internal_id
        ))
    }
}

fn validate_soundfont_index_json(
    asset: &ReconstructedDatAsset,
    index: &SoundFontIndexJson,
) -> Result<(), String> {
    let instrument_ids = index
        .instruments
        .iter()
        .map(|instrument| instrument.instrument_id)
        .collect::<BTreeSet<_>>();
    let sample_ids = index
        .samples
        .iter()
        .map(|sample| sample.sample_id)
        .collect::<BTreeSet<_>>();

    for load_map in &index.instrument_load_map {
        if !instrument_ids.contains(&load_map.instrument_id) {
            return Err(format!(
                "generated index {} load map references missing instrument {}",
                asset.entry.internal_id, load_map.instrument_id
            ));
        }
        for sample_id in load_map
            .sample_ids
            .iter()
            .chain(&load_map.sample_header_ids)
        {
            if !sample_ids.contains(sample_id) {
                return Err(format!(
                    "generated index {} instrument {} references missing sample {}",
                    asset.entry.internal_id, load_map.instrument_id, sample_id
                ));
            }
        }
    }

    for load_map in &index.preset_load_map {
        for instrument_id in &load_map.instrument_ids {
            if !instrument_ids.contains(instrument_id) {
                return Err(format!(
                    "generated index {} preset {} references missing instrument {}",
                    asset.entry.internal_id, load_map.preset_id, instrument_id
                ));
            }
        }
    }

    Ok(())
}

fn validate_generated_soundfont_json_semantics(
    expected_assets: &[PreparedDatAsset],
    dat_paths: &[PathBuf],
) -> Result<usize, String> {
    let actual_assets = collect_extracted_assets(dat_paths, None)?
        .into_iter()
        .map(|asset| (asset.entry.internal_id.clone(), asset))
        .collect::<BTreeMap<_, _>>();
    let mut validated = 0usize;

    for expected in expected_assets
        .iter()
        .filter(|asset| asset.entry.asset_type == "soundfont")
    {
        let raw_bytes = payload_bytes(&expected.payload)?;
        let mut cursor = Cursor::new(&raw_bytes);
        let soundfont = SoundFont::new(&mut cursor).map_err(|error| {
            format!(
                "failed to parse source soundfont {} for semantic validation: {error:?}",
                expected.entry.internal_id
            )
        })?;
        let coverage_id = format!("{}.coverage", expected.entry.internal_id);
        let index_id = format!("{}.index", expected.entry.internal_id);
        let Some(coverage_asset) = actual_assets.get(&coverage_id) else {
            continue;
        };
        let Some(index_asset) = actual_assets.get(&index_id) else {
            continue;
        };
        let coverage = from_json_bytes::<SoundFontCoverageJson>(&coverage_asset.bytes)
            .map_err(|error| format!("failed to parse {coverage_id}: {error}"))?;
        let index = from_json_bytes::<SoundFontIndexJson>(&index_asset.bytes)
            .map_err(|error| format!("failed to parse {index_id}: {error}"))?;
        validate_coverage_semantics(expected, &soundfont, &coverage)?;
        validate_index_semantics(expected, &soundfont, &index)?;
        validate_index_ranges_map_to_dat(expected, dat_paths, &index)?;
        validated += 1;
    }

    Ok(validated)
}

fn validate_coverage_semantics(
    expected: &PreparedDatAsset,
    soundfont: &SoundFont,
    coverage: &SoundFontCoverageJson,
) -> Result<(), String> {
    if coverage.parent_soundfont_id != expected.entry.internal_id {
        return Err(format!(
            "coverage JSON parent mismatch for {}",
            expected.entry.internal_id
        ));
    }
    let expected_coverage = extract_coverage_from_sf2(soundfont);
    let expected_melodic = expected_coverage
        .melodic
        .bank_programs
        .iter()
        .map(|entry| (entry.bank, entry.program))
        .collect::<BTreeSet<_>>();
    let actual_melodic = coverage
        .melodic
        .iter()
        .map(|entry| (entry.bank, entry.program))
        .collect::<BTreeSet<_>>();
    if expected_melodic != actual_melodic {
        return Err(format!(
            "coverage JSON melodic bank/program set mismatch for {}",
            expected.entry.internal_id
        ));
    }
    if expected_coverage.provides_percussion() != coverage.percussion {
        return Err(format!(
            "coverage JSON percussion flag mismatch for {}",
            expected.entry.internal_id
        ));
    }
    if coverage.presets.len() != soundfont.get_presets().len() {
        return Err(format!(
            "coverage JSON preset count mismatch for {}",
            expected.entry.internal_id
        ));
    }
    Ok(())
}

fn validate_index_semantics(
    expected: &PreparedDatAsset,
    soundfont: &SoundFont,
    index: &SoundFontIndexJson,
) -> Result<(), String> {
    if index.parent_soundfont_id != expected.entry.internal_id {
        return Err(format!(
            "index JSON parent mismatch for {}",
            expected.entry.internal_id
        ));
    }
    if index.presets.len() != soundfont.get_presets().len()
        || index.instruments.len() != soundfont.get_instruments().len()
        || index.samples.len() != soundfont.get_sample_headers().len()
    {
        return Err(format!(
            "index JSON top-level counts mismatch for {}",
            expected.entry.internal_id
        ));
    }
    for (instrument_id, instrument) in soundfont.get_instruments().iter().enumerate() {
        let load_map = index
            .instrument_load_map
            .iter()
            .find(|load_map| load_map.instrument_id == instrument_id as u32)
            .ok_or_else(|| {
                format!(
                    "index JSON missing instrument load map {} for {}",
                    instrument_id, expected.entry.internal_id
                )
            })?;
        let expected_sample_ids = instrument
            .get_regions()
            .iter()
            .map(|region| region.get_sample_id() as u32)
            .collect::<BTreeSet<_>>();
        let actual_sample_ids = load_map.sample_ids.iter().copied().collect::<BTreeSet<_>>();
        if expected_sample_ids != actual_sample_ids {
            return Err(format!(
                "index JSON instrument {} sample closure mismatch for {}",
                instrument_id, expected.entry.internal_id
            ));
        }
        if load_map.generator_region_ids != load_map.region_ids {
            return Err(format!(
                "index JSON instrument {} generator region closure mismatch for {}",
                instrument_id, expected.entry.internal_id
            ));
        }
        if load_map.wave_ranges.len() != instrument.get_regions().len() {
            return Err(format!(
                "index JSON instrument {} wave-range count mismatch for {}",
                instrument_id, expected.entry.internal_id
            ));
        }
    }
    Ok(())
}

fn validate_index_ranges_map_to_dat(
    expected: &PreparedDatAsset,
    dat_paths: &[PathBuf],
    index: &SoundFontIndexJson,
) -> Result<(), String> {
    let Some(load_map) = index
        .instrument_load_map
        .iter()
        .find(|load_map| !load_map.wave_ranges.is_empty())
    else {
        return Ok(());
    };
    let raw_location = find_raw_soundfont_location(dat_paths, &expected.entry.internal_id)?;
    let smpl_data_start_byte = index.smpl_data_start_byte.ok_or_else(|| {
        format!(
            "index JSON is missing SMPL data base for {}",
            expected.entry.internal_id
        )
    })?;
    for range in &load_map.wave_ranges {
        if range.byte_length == 0 {
            continue;
        }
        let entry_offset = smpl_data_start_byte
            .checked_add(range.smpl_start_byte)
            .ok_or_else(|| {
                format!(
                    "index JSON wave range offset overflow for {}",
                    expected.entry.internal_id
                )
            })?;
        let clamped_length = range
            .byte_length
            .min(raw_location.record.total_size.saturating_sub(entry_offset));
        if clamped_length == 0 {
            continue;
        }
        let plan = read_entry_range_plan(
            &raw_location.index,
            &raw_location.record,
            &[DatEntryRange {
                offset: entry_offset,
                length: clamped_length,
            }],
        )
        .map_err(|error| error.to_string())?;
        if plan.is_empty() {
            return Err(format!(
                "index JSON wave range did not map to DAT extents for {}",
                expected.entry.internal_id
            ));
        }
    }
    Ok(())
}

fn find_raw_soundfont_location(
    dat_paths: &[PathBuf],
    internal_id: &str,
) -> Result<RawSoundFontLocation, String> {
    for dat_path in dat_paths {
        let index = parse_dat_index_file(dat_path).map_err(|error| error.to_string())?;
        if let Some(record) = index.entries.iter().find(|record| {
            record.entry.asset_type == "soundfont" && record.entry.internal_id == internal_id
        }) {
            return Ok(RawSoundFontLocation {
                dat_path: dat_path.clone(),
                index: index.clone(),
                record: record.clone(),
            });
        }
    }
    Err(format!("DAT output is missing raw soundfont {internal_id}"))
}

fn payload_bytes(payload: &PreparedDatAssetPayload) -> Result<Vec<u8>, String> {
    match payload {
        PreparedDatAssetPayload::Bytes(bytes) => Ok(bytes.clone()),
        PreparedDatAssetPayload::File(path) => fs::read(path).map_err(|error| {
            format!(
                "failed to read prepared DAT source {} for roundtrip validation: {error}",
                path.display()
            )
        }),
    }
}

fn collect_extracted_assets(
    dat_paths: &[PathBuf],
    internal_id: Option<&str>,
) -> Result<Vec<ReconstructedDatAsset>, String> {
    let mut assets = BTreeMap::<String, ReconstructedDatAsset>::new();

    for dat_path in dat_paths {
        let bytes = read_dat_file(dat_path).map_err(|error| error.to_string())?;
        let entries = extract_all_entries(&bytes).map_err(|error| error.to_string())?;
        for (record, entry_bytes) in entries {
            let entry = record.entry;
            if internal_id
                .map(|target| target != entry.internal_id)
                .unwrap_or(false)
            {
                continue;
            }

            let key = entry.internal_id.clone();
            let flags = record.flags;
            assets
                .entry(key)
                .and_modify(|asset| {
                    asset.flags |= flags;
                    asset.bytes.extend_from_slice(&entry_bytes);
                })
                .or_insert_with(|| ReconstructedDatAsset {
                    entry,
                    flags,
                    bytes: entry_bytes,
                });
        }
    }

    Ok(assets.into_values().collect())
}

fn collect_dat_input_paths(input: &Path) -> Result<Vec<PathBuf>, String> {
    let metadata = fs::metadata(input)
        .map_err(|error| format!("failed to inspect DAT input {}: {error}", input.display()))?;

    let mut paths = if metadata.is_dir() {
        fs::read_dir(input)
            .map_err(|error| format!("failed to read DAT directory {}: {error}", input.display()))?
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|error| format!("failed to read DAT directory entry: {error}"))
            })
            .filter_map(|entry| match entry {
                Ok(path) if normalized_extension(&path).as_deref() == Some("dat") => Some(Ok(path)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, _>>()?
    } else if let Some((parent, prefix, extension)) = numbered_dat_family(input) {
        let family_prefix = format!("{prefix}-");
        fs::read_dir(&parent)
            .map_err(|error| format!("failed to read DAT directory {}: {error}", parent.display()))?
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|error| format!("failed to read DAT directory entry: {error}"))
            })
            .filter_map(|entry| match entry {
                Ok(path)
                    if path.extension().and_then(|value| value.to_str())
                        == Some(extension.as_str())
                        && path
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .map(|stem| is_numbered_dat_stem(stem, &family_prefix))
                            .unwrap_or(false) =>
                {
                    Some(Ok(path))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![input.to_owned()]
    };

    paths.sort();
    if paths.is_empty() {
        return Err(format!(
            "DAT input contains no .dat files: {}",
            input.display()
        ));
    }
    Ok(paths)
}

fn numbered_dat_family(input: &Path) -> Option<(PathBuf, String, String)> {
    let extension = input.extension()?.to_str()?.to_owned();
    if !extension.eq_ignore_ascii_case("dat") {
        return None;
    }
    let stem = input.file_stem()?.to_str()?;
    let (prefix, suffix) = stem.rsplit_once('-')?;
    if suffix.len() != 3 || !suffix.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    Some((
        input.parent().unwrap_or_else(|| Path::new("")).to_owned(),
        prefix.to_owned(),
        extension,
    ))
}

fn is_numbered_dat_stem(stem: &str, family_prefix: &str) -> bool {
    stem.strip_prefix(family_prefix)
        .map(|suffix| {
            suffix.len() == 3 && suffix.chars().all(|character| character.is_ascii_digit())
        })
        .unwrap_or(false)
}

fn extracted_filename(entry: &DatAssetEntry) -> String {
    let extension = runtime_extension(entry)
        .or_else(|| {
            Path::new(&entry.original_filename)
                .extension()
                .and_then(|value| value.to_str())
        })
        .unwrap_or("bin");
    format!(
        "{}.{}",
        sanitize_filename(&entry.internal_id),
        sanitize_filename(extension)
    )
}

fn runtime_extension(entry: &DatAssetEntry) -> Option<&str> {
    match entry.runtime_format.as_str() {
        "sf2" => Some("sf2"),
        _ => None,
    }
}

fn convert_soundfont(args: ConvertArgs) -> Result<(), String> {
    match normalized_extension(&args.input).as_deref() {
        Some("sf2") => fs::copy(&args.input, &args.output)
            .map(|_| ())
            .map_err(|error| format!("failed to copy SF2 input: {error}")),
        Some("sfark") => decode_sfark_file_to_sf2(&args.input, &args.output)
            .map_err(|error| format!("sfArk conversion failed: {error}")),
        Some(extension) => Err(format!("unsupported soundfont extension: .{extension}")),
        None => Err("input path has no extension".to_owned()),
    }
}

fn normalized_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
}

fn prepare_runtime_asset_payload(
    path: &Path,
    asset_type: &str,
) -> Result<PreparedDatAssetPayload, String> {
    match (asset_type, normalized_extension(path).as_deref()) {
        ("soundfont", Some("sf2")) => Ok(PreparedDatAssetPayload::File(path.to_owned())),
        ("soundfont", Some("sfark")) => {
            let input = fs::read(path).map_err(|error| {
                format!("failed to read sfArk source {}: {error}", path.display())
            })?;
            let mut last_reported_block = None;
            let diagnostics = decode_sfark_to_sf2_diagnostics_with_progress(&input, |progress| {
                let should_report = last_reported_block
                    .map(|block| progress.audio_block_index >= block + 512)
                    .unwrap_or(true);
                if should_report {
                    last_reported_block = Some(progress.audio_block_index);
                    println!(
                        "  sfArk {} block {} wrote {}/{} bytes",
                        progress.section,
                        progress.audio_block_index,
                        progress.total_written,
                        progress.original_size
                    );
                }
            })
            .map_err(|error| format!("sfArk conversion failed for {}: {error}", path.display()))?;

            if diagnostics.actual_file_check != diagnostics.expected_file_check {
                return Err(format!(
                    "sfArk conversion failed for {}: file checksum mismatch",
                    path.display()
                ));
            }

            Ok(PreparedDatAssetPayload::Bytes(diagnostics.output))
        }
        ("soundfont", Some(extension)) => Err(format!(
            "unsupported soundfont source extension for {}: .{extension}",
            path.display()
        )),
        ("soundfont", None) => Err(format!(
            "soundfont source has no extension: {}",
            path.display()
        )),
        (_, _) => Ok(PreparedDatAssetPayload::File(path.to_owned())),
    }
}

fn runtime_format_for_source(path: &Path, asset_type: &str) -> Option<String> {
    if asset_type == "soundfont" {
        match normalized_extension(path).as_deref() {
            Some("sf2" | "sfark") => Some("sf2".to_owned()),
            _ => None,
        }
    } else {
        None
    }
}

fn storage_format_for_source(path: &Path, asset_type: &str) -> Option<String> {
    if asset_type == "soundfont" {
        match normalized_extension(path).as_deref() {
            Some("sf2" | "sfark") => Some("sf2".to_owned()),
            _ => None,
        }
    } else {
        None
    }
}

fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => character,
            _ => '_',
        })
        .collect()
}
