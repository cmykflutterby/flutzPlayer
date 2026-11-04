use super::*;

pub(super) fn extract_dat(args: ExtractArgs) -> Result<(), String> {
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

pub(super) struct ReconstructedDatAsset {
    pub(super) entry: DatAssetEntry,
    pub(super) flags: u64,
    pub(super) bytes: Vec<u8>,
}

pub(super) fn validate_dat_payload_roundtrip(
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

pub(super) fn collect_extracted_assets(
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

pub(super) fn collect_dat_input_paths(input: &Path) -> Result<Vec<PathBuf>, String> {
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

pub(super) fn numbered_dat_family(input: &Path) -> Option<(PathBuf, String, String)> {
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

pub(super) fn is_numbered_dat_stem(stem: &str, family_prefix: &str) -> bool {
    stem.strip_prefix(family_prefix)
        .map(|suffix| {
            suffix.len() == 3 && suffix.chars().all(|character| character.is_ascii_digit())
        })
        .unwrap_or(false)
}

pub(super) fn extracted_filename(entry: &DatAssetEntry) -> String {
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

pub(super) fn runtime_extension(entry: &DatAssetEntry) -> Option<&str> {
    match entry.runtime_format.as_str() {
        "sf2" => Some("sf2"),
        _ => None,
    }
}
