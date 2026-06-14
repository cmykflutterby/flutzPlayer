use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawPresetSet {
    default_preset_id: String,
    coverage: RawCoverageSettings,
    presets: Vec<RawPreset>,
}

#[derive(Debug, Deserialize)]
struct RawCoverageSettings {
    mode: String,
    strict_bank_program_match: bool,
    percussion_requires_bank_128: bool,
}

#[derive(Debug, Deserialize)]
struct RawPreset {
    id: String,
    display_name: String,
    description: String,
    font_ids: Vec<String>,
    blend_mode: String,
    #[serde(default)]
    weights: BTreeMap<String, u32>,
}

#[derive(Debug, Deserialize)]
struct RawDatManifest {
    assets: Vec<RawDatAsset>,
}

#[derive(Debug, Deserialize)]
struct RawDatAsset {
    internal_id: String,
    asset_type: String,
    #[serde(default = "default_true")]
    include_in_dat: bool,
}

fn default_true() -> bool {
    true
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("flutz_core should live under crates/flutz_core");
    let presets_path = repo_root.join("assets/multi-font-presets.toml");
    let dat_manifest_path = repo_root.join("assets/dat-manifest.toml");

    println!("cargo:rerun-if-changed={}", presets_path.display());
    println!("cargo:rerun-if-changed={}", dat_manifest_path.display());

    let preset_set = read_presets(&presets_path);
    let dat_font_ids = read_dat_soundfont_ids(&dat_manifest_path);
    validate_presets(&preset_set, &dat_font_ids);

    let generated = generate_rust(&preset_set);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("generated_presets.rs"), generated)
        .expect("failed to write generated preset constants");
}

fn read_presets(path: &Path) -> RawPresetSet {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    toml::from_str(&text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn read_dat_soundfont_ids(path: &Path) -> BTreeSet<String> {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let manifest: RawDatManifest = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    manifest
        .assets
        .into_iter()
        .filter(|asset| asset.include_in_dat && asset.asset_type == "soundfont")
        .map(|asset| asset.internal_id)
        .collect()
}

fn validate_presets(preset_set: &RawPresetSet, dat_font_ids: &BTreeSet<String>) {
    if preset_set.presets.is_empty() {
        panic!("assets/multi-font-presets.toml must define at least one preset");
    }

    if !preset_set
        .presets
        .iter()
        .any(|preset| preset.id == preset_set.default_preset_id)
    {
        panic!(
            "default_preset_id {:?} does not match any preset id",
            preset_set.default_preset_id
        );
    }

    let mut preset_ids = BTreeSet::new();
    for preset in &preset_set.presets {
        if !preset_ids.insert(preset.id.as_str()) {
            panic!("duplicate preset id {:?}", preset.id);
        }

        if preset.font_ids.is_empty() {
            panic!("preset {:?} must include at least one font_id", preset.id);
        }

        let mut font_ids = BTreeSet::new();
        for font_id in &preset.font_ids {
            if !font_ids.insert(font_id.as_str()) {
                panic!(
                    "preset {:?} contains duplicate font_id {:?}",
                    preset.id, font_id
                );
            }
            if !dat_font_ids.contains(font_id) {
                panic!(
                    "preset {:?} references font_id {:?}, but it is not a pack-enabled soundfont in assets/dat-manifest.toml",
                    preset.id, font_id
                );
            }
        }

        match preset.blend_mode.as_str() {
            "replace-mute" | "blend-even" => {
                if !preset.weights.is_empty() {
                    panic!(
                        "preset {:?} uses {:?} and must not define weights",
                        preset.id, preset.blend_mode
                    );
                }
            }
            "blend-weight" => validate_blend_weights(preset),
            other => panic!("preset {:?} has invalid blend_mode {:?}", preset.id, other),
        }
    }
}

fn validate_blend_weights(preset: &RawPreset) {
    for font_id in &preset.font_ids {
        match preset.weights.get(font_id) {
            Some(weight) if *weight >= 1 => {}
            Some(_) => panic!(
                "preset {:?} has invalid blend-weight value for font_id {:?}; weights must be integers >= 1",
                preset.id, font_id
            ),
            None => panic!(
                "preset {:?} is blend-weight but lacks a weight for font_id {:?}",
                preset.id, font_id
            ),
        }
    }

    for font_id in preset.weights.keys() {
        if !preset.font_ids.iter().any(|candidate| candidate == font_id) {
            panic!(
                "preset {:?} defines a weight for unknown font_id {:?}",
                preset.id, font_id
            );
        }
    }
}

fn generate_rust(preset_set: &RawPresetSet) -> String {
    let mut output = String::from("// @generated by flutz_core/build.rs\n");
    output
        .push_str("use super::{BlendMode, CoverageSettings, Preset, PresetSet, PresetWeight};\n\n");

    for preset in &preset_set.presets {
        let const_name = preset_const_name(&preset.id);
        output.push_str(&format!(
            "const {const_name}_FONTS: &[&str] = &[{}];\n",
            string_slice(&preset.font_ids)
        ));
        output.push_str(&format!(
            "const {const_name}_WEIGHTS: &[PresetWeight] = &[{}];\n",
            weight_slice(preset)
        ));
    }

    output.push_str("\npub static PRESET_SET: PresetSet = PresetSet {\n");
    output.push_str(&format!(
        "    default_preset_id: {},\n",
        string_literal(&preset_set.default_preset_id)
    ));
    output.push_str("    coverage: CoverageSettings {\n");
    output.push_str(&format!(
        "        mode: {},\n",
        string_literal(&preset_set.coverage.mode)
    ));
    output.push_str(&format!(
        "        strict_bank_program_match: {},\n",
        preset_set.coverage.strict_bank_program_match
    ));
    output.push_str(&format!(
        "        percussion_requires_bank_128: {},\n",
        preset_set.coverage.percussion_requires_bank_128
    ));
    output.push_str("    },\n");
    output.push_str("    presets: &[\n");
    for preset in &preset_set.presets {
        let const_name = preset_const_name(&preset.id);
        output.push_str("        Preset {\n");
        output.push_str(&format!(
            "            id: {},\n",
            string_literal(&preset.id)
        ));
        output.push_str(&format!(
            "            display_name: {},\n",
            string_literal(&preset.display_name)
        ));
        output.push_str(&format!(
            "            description: {},\n",
            string_literal(&preset.description)
        ));
        output.push_str(&format!("            font_ids: {const_name}_FONTS,\n"));
        output.push_str(&format!(
            "            blend_mode: {},\n",
            blend_mode_expr(&preset.blend_mode)
        ));
        output.push_str(&format!("            weights: {const_name}_WEIGHTS,\n"));
        output.push_str("        },\n");
    }
    output.push_str("    ],\n};\n");

    output
}

fn preset_const_name(id: &str) -> String {
    let mut name = String::from("PRESET");
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() {
            name.push('_');
            name.push(ch.to_ascii_uppercase());
        } else {
            name.push('_');
        }
    }
    name
}

fn string_slice(values: &[String]) -> String {
    values
        .iter()
        .map(|value| string_literal(value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn weight_slice(preset: &RawPreset) -> String {
    preset
        .font_ids
        .iter()
        .filter_map(|font_id| {
            preset.weights.get(font_id).map(|weight| {
                format!(
                    "PresetWeight {{ font_id: {}, weight: {weight} }}",
                    string_literal(font_id)
                )
            })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn string_literal(value: &str) -> String {
    format!("{value:?}")
}

fn blend_mode_expr(value: &str) -> &'static str {
    match value {
        "replace-mute" => "BlendMode::ReplaceMute",
        "blend-even" => "BlendMode::BlendEven",
        "blend-weight" => "BlendMode::BlendWeight",
        _ => unreachable!("blend mode validated before generation"),
    }
}
