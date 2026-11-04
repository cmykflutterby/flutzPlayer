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

mod cli_convert;
mod diagnose;
mod extract;
mod inspect;
mod pack;
mod validate;
use cli_convert::*;
use diagnose::*;
use extract::*;
use inspect::*;
use pack::*;
use validate::*;

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
