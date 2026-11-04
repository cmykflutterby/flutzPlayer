use super::*;

pub(super) fn inspect_dat(args: InspectArgs) -> Result<(), String> {
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
pub(super) struct RawSoundFontLocation {
    pub(super) dat_path: PathBuf,
    pub(super) index: DatArchiveIndex,
    pub(super) record: flutz_dat::assets::DatEntryRecord,
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
