use super::*;

pub(super) fn validate_entry(entry: &DatAssetEntry) -> Result<()> {
    for (name, value) in [
        ("internal_id", &entry.internal_id),
        ("display_name", &entry.display_name),
        ("asset_type", &entry.asset_type),
        ("source_format", &entry.source_format),
        ("storage_format", &entry.storage_format),
        ("runtime_format", &entry.runtime_format),
        ("original_filename", &entry.original_filename),
    ] {
        if value.trim().is_empty() {
            return Err(FlutzError::InvalidInput(format!(
                "DAT asset {name} must not be empty"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_assets(assets: &[PreparedDatAsset]) -> Result<()> {
    let mut seen_ids = HashSet::new();
    for asset in assets {
        validate_entry(&asset.entry)?;
        if !seen_ids.insert(asset.entry.internal_id.clone()) {
            return Err(FlutzError::InvalidInput(format!(
                "duplicate DAT internal ID: {}",
                asset.entry.internal_id
            )));
        }
    }
    Ok(())
}
