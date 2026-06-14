use std::{collections::BTreeMap, env, fs, path::PathBuf};

use flutz_dat::{
    assets::{DatAssetEntry, PreparedDatAsset, PreparedDatAssetPayload},
    read::extract_all_entries,
    soundfont_json::{
        from_json_bytes, SoundFontCoverageJson, SoundFontIndexJson, SoundFontMetadataJson,
        SoundFontPackReportJson, SOUNDFONT_COVERAGE_ASSET_TYPE, SOUNDFONT_INDEX_ASSET_TYPE,
        SOUNDFONT_METADATA_ASSET_TYPE, SOUNDFONT_PACK_REPORT_ASSET_TYPE,
    },
    write::{build_dat_archive, DEFAULT_CHUNK_SIZE},
};
use flutz_soundfont_tools::soundfont_json_pack::generate_soundfont_json_assets;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sf2_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("_local/runtime-tests/retro.sf2"));
    let sf2_bytes = fs::read(&sf2_path)
        .map_err(|error| format!("failed to read probe SF2 {}: {error}", sf2_path.display()))?;
    let original_filename = sf2_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("probe.sf2")
        .to_owned();
    let raw_entry = DatAssetEntry {
        internal_id: "probe-font".to_owned(),
        display_name: "Probe Font".to_owned(),
        asset_type: "soundfont".to_owned(),
        source_format: "sf2".to_owned(),
        storage_format: "sf2".to_owned(),
        runtime_format: "sf2".to_owned(),
        original_filename,
    };

    let mut assets = vec![PreparedDatAsset {
        entry: raw_entry.clone(),
        flags: 0,
        payload: PreparedDatAssetPayload::Bytes(sf2_bytes.clone()),
    }];
    assets.extend(generate_soundfont_json_assets(
        &raw_entry, &sf2_bytes, true,
    )?);

    let archive = build_dat_archive(assets, DEFAULT_CHUNK_SIZE)?;
    let extracted = extract_all_entries(&archive.bytes)?
        .into_iter()
        .map(|(record, bytes)| (record.entry.internal_id.clone(), (record.entry, bytes)))
        .collect::<BTreeMap<_, _>>();

    let metadata = parse_entry::<SoundFontMetadataJson>(&extracted, "probe-font.metadata")?;
    let coverage = parse_entry::<SoundFontCoverageJson>(&extracted, "probe-font.coverage")?;
    let index = parse_entry::<SoundFontIndexJson>(&extracted, "probe-font.index")?;
    let report = parse_entry::<SoundFontPackReportJson>(&extracted, "probe-font.pack-report")?;

    require_asset_type(
        &extracted,
        "probe-font.metadata",
        SOUNDFONT_METADATA_ASSET_TYPE,
    )?;
    require_asset_type(
        &extracted,
        "probe-font.coverage",
        SOUNDFONT_COVERAGE_ASSET_TYPE,
    )?;
    require_asset_type(&extracted, "probe-font.index", SOUNDFONT_INDEX_ASSET_TYPE)?;
    require_asset_type(
        &extracted,
        "probe-font.pack-report",
        SOUNDFONT_PACK_REPORT_ASSET_TYPE,
    )?;

    if metadata.parent_soundfont_id != "probe-font"
        || coverage.parent_soundfont_id != "probe-font"
        || index.parent_soundfont_id != "probe-font"
        || report.parent_soundfont_id != "probe-font"
    {
        return Err("generated JSON resource parent IDs did not match raw soundfont".into());
    }
    if metadata.generated_resources.index_internal_id != "probe-font.index" {
        return Err("metadata generated-resource links were not deterministic".into());
    }
    if index.instrument_load_map.is_empty() {
        return Err("index JSON did not contain instrument-level load-map entries".into());
    }
    let first_load_map = &index.instrument_load_map[0];
    if first_load_map.sample_ids.is_empty() || first_load_map.wave_ranges.is_empty() {
        return Err("instrument load map did not resolve samples and wave ranges".into());
    }

    println!(
        "soundfont_json_pack_probe ok: packed {} entries with {} presets, {} instruments, {} samples",
        extracted.len(),
        index.presets.len(),
        index.instruments.len(),
        index.samples.len()
    );
    Ok(())
}

fn parse_entry<T>(
    entries: &BTreeMap<String, (DatAssetEntry, Vec<u8>)>,
    internal_id: &str,
) -> Result<T, Box<dyn std::error::Error>>
where
    T: serde::de::DeserializeOwned,
{
    let (_, bytes) = entries
        .get(internal_id)
        .ok_or_else(|| format!("missing generated entry {internal_id}"))?;
    Ok(from_json_bytes(bytes)?)
}

fn require_asset_type(
    entries: &BTreeMap<String, (DatAssetEntry, Vec<u8>)>,
    internal_id: &str,
    expected_asset_type: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (entry, _) = entries
        .get(internal_id)
        .ok_or_else(|| format!("missing generated entry {internal_id}"))?;
    if entry.asset_type != expected_asset_type {
        return Err(format!(
            "entry {internal_id} has asset_type {}, expected {expected_asset_type}",
            entry.asset_type
        )
        .into());
    }
    Ok(())
}
