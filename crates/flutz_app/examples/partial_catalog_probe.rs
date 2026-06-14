use std::{env, path::PathBuf};

use flutz_app::{
    app::{DatStartupSummary, SoundFontCatalogEntry},
    dat_startup_summary_for_data_dir, memory_runtime,
    playback::{AudioBackend, PlaybackController},
};
use flutz_synth::SoundFontCoverage;

#[cfg(feature = "jemalloc-memory")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_DATA_DIR: &str = "drops/flutzplayer/data";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    memory_runtime::initialize_from_env(env::args().any(|arg| arg == "--debug-memory"));
    let args = Args::parse()?;
    let catalog = soundfont_catalog(&args.data_dir)?;
    let mut checked = 0usize;
    let mut failures = Vec::<String>::new();

    for font in catalog
        .iter()
        .filter(|font| font.storage_format == "sf2" && font.runtime_format == "sf2")
    {
        let Some(coverage) = &font.coverage else {
            failures.push(format!("{} has no startup coverage", font.internal_id));
            continue;
        };
        let Some(probe_midi) = probe_midi_for_coverage(coverage) else {
            failures.push(format!("{} has no probeable coverage", font.internal_id));
            continue;
        };
        checked = checked.saturating_add(1);
        match probe_font(&args.data_dir, font, probe_midi) {
            Ok(()) => {}
            Err(error) => failures.push(format!("{}: {error}", font.internal_id)),
        }
    }

    println!(
        "{{\"event\":\"partial_catalog_summary\",\"checked\":{},\"failures\":{}}}",
        checked,
        failures.len()
    );
    if !failures.is_empty() {
        return Err(failures.join(" | ").into());
    }
    Ok(())
}

struct Args {
    data_dir: PathBuf,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut data_dir = PathBuf::from(DEFAULT_DATA_DIR);
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => {
                    data_dir = PathBuf::from(
                        args.next()
                            .ok_or_else(|| "--data-dir requires a value".to_owned())?,
                    );
                }
                "--debug-memory" => {}
                _ => return Err(format!("unsupported argument: {arg}")),
            }
        }
        Ok(Self { data_dir })
    }
}

fn soundfont_catalog(
    data_dir: &PathBuf,
) -> Result<Vec<SoundFontCatalogEntry>, Box<dyn std::error::Error>> {
    match dat_startup_summary_for_data_dir(data_dir)? {
        DatStartupSummary::Available { soundfonts, .. } => Ok(soundfonts),
        DatStartupSummary::Unavailable(error) => Err(error.into()),
    }
}

fn probe_font(
    data_dir: &PathBuf,
    font: &SoundFontCatalogEntry,
    probe_midi: ProbeMidi,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut playback = PlaybackController::new(
        data_dir.clone(),
        vec![font.clone()],
        AudioBackend::Sdl3,
        false,
        false,
    );
    let requested = vec![font.internal_id.clone()];
    playback.load_midi_bytes(
        probe_midi.bytes,
        format!("{}-partial-probe.mid", font.internal_id),
        &requested,
    )?;
    let report = playback.render_probe(512)?;
    let metrics = playback.debug_metrics();
    let memory = memory_runtime::snapshot();

    println!(
        "{{\"event\":\"partial_catalog_font\",\"font\":\"{}\",\"role\":\"{}\",\"frames\":{},\"peak\":{:.6},\"subset_plans\":{},\"subset_samples\":{},\"subset_entries\":{},\"full_entries\":{},\"metadata_entries\":{},\"sample_range_entries\":{},\"soundfont_cache_resident_bytes\":{},\"working_set_bytes\":{},\"peak_working_set_bytes\":{}}}",
        escape_json(&font.internal_id),
        probe_midi.role,
        report.frames,
        report.peak,
        metrics.subset_plans.plan_count,
        metrics.subset_plans.sample_count,
        metrics.soundfont_cache.subset_entries,
        metrics.soundfont_cache.full_entries,
        metrics.soundfont_cache.metadata_entries,
        metrics.soundfont_cache.sample_range_entries,
        metrics.soundfont_cache.resident_bytes,
        memory.os.working_set_bytes,
        memory.os.peak_working_set_bytes,
    );

    if metrics.subset_plans.plan_count == 0 {
        return Err("no subset plan was built".into());
    }
    if metrics.soundfont_cache.subset_entries == 0 || metrics.soundfont_cache.full_entries != 0 {
        return Err(format!(
            "font was not partially loaded: subset_entries={} full_entries={}",
            metrics.soundfont_cache.subset_entries, metrics.soundfont_cache.full_entries
        )
        .into());
    }
    Ok(())
}

struct ProbeMidi {
    bytes: Vec<u8>,
    role: &'static str,
}

fn probe_midi_for_coverage(coverage: &SoundFontCoverage) -> Option<ProbeMidi> {
    if let Some(program) = coverage.melodic.bank_programs.iter().next() {
        return Some(ProbeMidi {
            bytes: melodic_midi(program.bank, program.program),
            role: "melodic",
        });
    }
    if coverage.provides_percussion() {
        let note = coverage
            .percussion
            .key_ranges
            .iter()
            .next()
            .map(|range| range.low_key)
            .unwrap_or(36);
        return Some(ProbeMidi {
            bytes: percussion_midi(note),
            role: "percussion",
        });
    }
    None
}

fn melodic_midi(bank: u16, program: u8) -> Vec<u8> {
    let bank_msb = ((bank >> 7) & 0x7f) as u8;
    let bank_lsb = (bank & 0x7f) as u8;
    let mut track = Vec::new();
    track.extend_from_slice(&[0x00, 0xB0, 0x00, bank_msb]);
    track.extend_from_slice(&[0x00, 0xB0, 0x20, bank_lsb]);
    track.extend_from_slice(&[0x00, 0xC0, program]);
    track.extend_from_slice(&[0x00, 0x90, 60, 96]);
    track.extend_from_slice(&[0x83, 0x60, 0x80, 60, 0]);
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    midi_file(track)
}

fn percussion_midi(note: u8) -> Vec<u8> {
    let mut track = Vec::new();
    track.extend_from_slice(&[0x00, 0x99, note, 110]);
    track.extend_from_slice(&[0x83, 0x60, 0x89, note, 0]);
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    midi_file(track)
}

fn midi_file(track: Vec<u8>) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MThd");
    bytes.extend_from_slice(&6u32.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&480u16.to_be_bytes());
    bytes.extend_from_slice(b"MTrk");
    bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&track);
    bytes
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
