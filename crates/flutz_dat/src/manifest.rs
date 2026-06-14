use std::path::PathBuf;

use flutz_core::{FlutzError, Result};
use serde::Deserialize;

/// Determines the order in which assets are packed into the DAT archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackingOrder {
    /// Pack assets in order from smallest to largest source file size
    SmallestFirst,
    /// Pack assets in the order they appear in the manifest
    ManifestOrder,
}

impl Default for PackingOrder {
    fn default() -> Self {
        PackingOrder::SmallestFirst
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatManifest {
    pub default_soundfont_id: Option<String>,
    pub packing_order: PackingOrder,
    pub generate_soundfont_json: bool,
    pub generate_soundfont_pack_report: bool,
    pub assets: Vec<DatManifestAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatManifestAsset {
    pub internal_id: String,
    pub display_name: String,
    pub asset_type: String,
    pub source_format: String,
    pub runtime_format: String,
    pub source_path: PathBuf,
    pub include_in_dat: bool,
}

#[derive(Debug, Deserialize)]
struct RawDatManifest {
    default_soundfont_id: Option<String>,
    packing_order: Option<String>,
    generate_soundfont_json: Option<bool>,
    generate_soundfont_pack_report: Option<bool>,
    #[serde(default)]
    assets: Vec<RawDatManifestAsset>,
}

#[derive(Debug, Deserialize)]
struct RawDatManifestAsset {
    internal_id: String,
    display_name: String,
    asset_type: String,
    source_format: Option<String>,
    storage_format: Option<String>,
    runtime_format: String,
    source_path: PathBuf,
    include_in_dat: Option<bool>,
    pack: Option<bool>,
    include: Option<bool>,
}

pub fn parse_dat_manifest(input: &str) -> Result<DatManifest> {
    let raw: RawDatManifest = toml::from_str(input)
        .map_err(|error| FlutzError::InvalidInput(format!("invalid DAT manifest TOML: {error}")))?;

    if raw.assets.is_empty() {
        return Err(FlutzError::InvalidInput(
            "DAT manifest contains no [[assets]] entries".to_owned(),
        ));
    }

    let assets = raw
        .assets
        .into_iter()
        .map(|asset| {
            let source_format = asset
                .source_format
                .or(asset.storage_format)
                .ok_or_else(|| {
                    FlutzError::InvalidInput(format!(
                        "DAT manifest asset {} is missing source_format",
                        asset.internal_id
                    ))
                })?;
            Ok(DatManifestAsset {
                internal_id: asset.internal_id,
                display_name: asset.display_name,
                asset_type: asset.asset_type,
                source_format,
                runtime_format: asset.runtime_format,
                source_path: asset.source_path,
                include_in_dat: asset
                    .include_in_dat
                    .or(asset.pack)
                    .or(asset.include)
                    .unwrap_or(true),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let packing_order = match raw.packing_order.as_deref() {
        Some("smallest-first") | None => PackingOrder::SmallestFirst,
        Some("manifest-order") => PackingOrder::ManifestOrder,
        Some(other) => {
            eprintln!(
                "warning: unknown packing_order '{}'; defaulting to 'smallest-first'",
                other
            );
            PackingOrder::SmallestFirst
        }
    };

    Ok(DatManifest {
        default_soundfont_id: raw.default_soundfont_id,
        packing_order,
        generate_soundfont_json: raw.generate_soundfont_json.unwrap_or(true),
        generate_soundfont_pack_report: raw.generate_soundfont_pack_report.unwrap_or(false),
        assets,
    })
}
