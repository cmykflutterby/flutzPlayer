use super::*;

pub(super) fn inspect_dat_fileset(input: &Path) -> Result<()> {
    let (paths, fonts, total_bytes) = summarize_dat_fileset(input)?;

    println!("DAT files: {}", paths.len());
    println!("DAT bytes: {}", total_bytes);
    println!("SoundFonts: {}", fonts.len());
    for font in fonts.values() {
        println!();
        println!(
            "{}{}",
            font.internal_id,
            if font.is_default { " [default]" } else { "" }
        );
        println!("  display: {}", font.display_name);
        println!("  source:  {}", font.source_format);
        println!("  storage: {}", font.storage_format);
        println!("  runtime: {}", font.runtime_format);
        println!("  bytes:   {}", font.total_size);
        println!("  parts:   {}", font.part_count);
        println!(
            "  coverage JSON: {}",
            if font.coverage_json_available {
                "available"
            } else {
                "unavailable"
            }
        );
        println!("  files:   {}", font.files.join(", "));
    }
    Ok(())
}

pub fn dat_startup_summary_for_data_dir(data_dir: &Path) -> Result<app::DatStartupSummary> {
    summarize_dat_data_dir(data_dir)
}

pub(super) fn summarize_dat_data_dir(data_dir: &Path) -> Result<app::DatStartupSummary> {
    if !data_dir.exists() {
        return Ok(app::DatStartupSummary::Unavailable(format!(
            "data folder not found: {}",
            data_dir.display()
        )));
    }
    let (paths, fonts, total_bytes) = summarize_dat_fileset(data_dir)?;
    Ok(app::DatStartupSummary::Available {
        dat_file_count: paths.len(),
        dat_byte_count: total_bytes,
        soundfonts: fonts
            .values()
            .map(|font| app::SoundFontCatalogEntry {
                internal_id: font.internal_id.clone(),
                display_name: font.display_name.clone(),
                source_format: font.source_format.clone(),
                storage_format: font.storage_format.clone(),
                runtime_format: font.runtime_format.clone(),
                is_default: font.is_default,
                total_size: font.total_size,
                part_count: font.part_count,
                coverage: font.coverage.clone(),
            })
            .collect(),
    })
}

fn summarize_dat_fileset(
    input: &Path,
) -> Result<(Vec<PathBuf>, BTreeMap<String, DatFontSummary>, u64)> {
    let paths = collect_dat_paths(input)?;
    let mut fonts = BTreeMap::<String, DatFontSummary>::new();
    let mut total_bytes = 0u64;

    for path in &paths {
        total_bytes += fs::metadata(path)
            .map_err(|error| {
                FlutzError::Runtime(format!("failed to inspect {}: {error}", path.display()))
            })?
            .len();
        let index = parse_dat_index_file(path)?;
        let soundfont_ids = index
            .entries
            .iter()
            .filter(|record| record.entry.asset_type == "soundfont")
            .map(|record| record.entry.internal_id.clone())
            .collect::<Vec<_>>();
        for record in &index.entries {
            if record.entry.asset_type != "soundfont" {
                continue;
            }
            fonts
                .entry(record.entry.internal_id.clone())
                .and_modify(|summary| summary.add_part(record, path))
                .or_insert_with(|| DatFontSummary::from_part(record, path));
        }
        for soundfont_id in soundfont_ids {
            let coverage = read_soundfont_coverage_json_from_file(path, &index, &soundfont_id)?;
            if let Some(coverage) = coverage {
                if let Some(summary) = fonts.get_mut(&soundfont_id) {
                    summary.coverage_json_available = true;
                    summary.coverage = Some(soundfont_coverage_from_json(&coverage));
                }
            }
        }
    }

    Ok((paths, fonts, total_bytes))
}

#[derive(Debug, Clone)]
struct DatFontSummary {
    internal_id: String,
    display_name: String,
    source_format: String,
    storage_format: String,
    runtime_format: String,
    flags: u64,
    total_size: u64,
    part_count: u64,
    files: Vec<String>,
    is_default: bool,
    coverage_json_available: bool,
    coverage: Option<SoundFontCoverage>,
}

impl DatFontSummary {
    fn from_part(record: &flutz_dat::assets::DatEntryRecord, path: &Path) -> Self {
        let mut summary = Self {
            internal_id: record.entry.internal_id.clone(),
            display_name: record.entry.display_name.clone(),
            source_format: record.entry.source_format.clone(),
            storage_format: record.entry.storage_format.clone(),
            runtime_format: record.entry.runtime_format.clone(),
            flags: 0,
            total_size: 0,
            part_count: 0,
            files: Vec::new(),
            is_default: false,
            coverage_json_available: false,
            coverage: None,
        };
        summary.add_part(record, path);
        summary
    }

    fn add_part(&mut self, record: &flutz_dat::assets::DatEntryRecord, path: &Path) {
        self.flags |= record.flags;
        self.total_size += record.total_size;
        self.part_count += 1;
        self.is_default = self.is_default || record.flags & DAT_ENTRY_FLAG_DEFAULT != 0;
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string());
        if self.files.last() != Some(&file) {
            self.files.push(file);
        }
    }
}

fn soundfont_coverage_from_json(value: &SoundFontCoverageJson) -> SoundFontCoverage {
    SoundFontCoverage {
        melodic: MelodicCoverage {
            bank_programs: value
                .melodic
                .iter()
                .map(|entry| BankProgram {
                    bank: entry.bank,
                    program: entry.program,
                })
                .collect::<BTreeSet<_>>(),
        },
        percussion: PercussionCoverage {
            has_bank_128: value.percussion,
            key_ranges: value
                .percussion_key_ranges
                .iter()
                .map(|entry| PercussionKeyRange {
                    low_key: entry.low_key,
                    high_key: entry.high_key,
                })
                .collect::<BTreeSet<_>>(),
        },
        metadata: SoundFontCoverageMetadata {
            preset_names: value
                .presets
                .iter()
                .map(|preset| preset.name.clone())
                .collect(),
            sample_count: value.sample_count as usize,
        },
    }
}

pub(crate) fn collect_dat_paths(input: &Path) -> Result<Vec<PathBuf>> {
    let metadata = fs::metadata(input).map_err(|error| {
        FlutzError::Runtime(format!("failed to inspect {}: {error}", input.display()))
    })?;
    let mut paths = if metadata.is_dir() {
        fs::read_dir(input)
            .map_err(|error| {
                FlutzError::Runtime(format!("failed to read {}: {error}", input.display()))
            })?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| normalized_extension(path).as_deref() == Some("dat"))
            .collect::<Vec<_>>()
    } else if let Some((parent, prefix, extension)) = numbered_dat_family(input) {
        let family_prefix = format!("{prefix}-");
        fs::read_dir(&parent)
            .map_err(|error| {
                FlutzError::Runtime(format!("failed to read {}: {error}", parent.display()))
            })?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                normalized_extension(path).as_deref() == Some(extension.as_str())
                    && path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .map(|stem| is_numbered_dat_stem(stem, &family_prefix))
                        .unwrap_or(false)
            })
            .collect::<Vec<_>>()
    } else {
        vec![input.to_owned()]
    };
    paths.sort();
    if paths.is_empty() {
        return Err(FlutzError::InvalidInput(format!(
            "no DAT files found in {}",
            input.display()
        )));
    }
    Ok(paths)
}

fn numbered_dat_family(path: &Path) -> Option<(PathBuf, String, String)> {
    let extension = normalized_extension(path)?;
    if extension != "dat" {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let (prefix, suffix) = stem.rsplit_once('-')?;
    if suffix.len() != 3 || !suffix.chars().all(|value| value.is_ascii_digit()) {
        return None;
    }
    let parent = path.parent().map(Path::to_owned).unwrap_or_default();
    Some((parent, prefix.to_owned(), extension))
}

fn is_numbered_dat_stem(stem: &str, family_prefix: &str) -> bool {
    let Some(suffix) = stem.strip_prefix(family_prefix) else {
        return false;
    };
    suffix.len() == 3 && suffix.chars().all(|value| value.is_ascii_digit())
}

fn normalized_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
}
