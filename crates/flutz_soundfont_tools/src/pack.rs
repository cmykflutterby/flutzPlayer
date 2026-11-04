use super::*;

pub(super) fn pack_dat(args: PackArgs) -> Result<(), String> {
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

pub(super) fn payload_bytes(payload: &PreparedDatAssetPayload) -> Result<Vec<u8>, String> {
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

pub(super) fn prepare_runtime_asset_payload(
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

pub(super) fn runtime_format_for_source(path: &Path, asset_type: &str) -> Option<String> {
    if asset_type == "soundfont" {
        match normalized_extension(path).as_deref() {
            Some("sf2" | "sfark") => Some("sf2".to_owned()),
            _ => None,
        }
    } else {
        None
    }
}

pub(super) fn storage_format_for_source(path: &Path, asset_type: &str) -> Option<String> {
    if asset_type == "soundfont" {
        match normalized_extension(path).as_deref() {
            Some("sf2" | "sfark") => Some("sf2".to_owned()),
            _ => None,
        }
    } else {
        None
    }
}

pub(super) fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => character,
            _ => '_',
        })
        .collect()
}
