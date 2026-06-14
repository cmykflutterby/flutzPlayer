use std::collections::BTreeMap;

use flutz_core::{BlendMode, Preset};
use flutz_synth::SoundFontCoverage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutingDecision {
    pub font_index: usize,
    pub volume: f32,
    pub muted: bool,
    pub unsupported: bool,
}

pub fn compute_strip_routing(
    preset: &Preset,
    font_ids: &[String],
    coverage_cache: &BTreeMap<String, SoundFontCoverage>,
    program: u8,
    bank: u16,
    is_percussion: bool,
) -> Vec<RoutingDecision> {
    let provider_indices = provider_indices(font_ids, coverage_cache, program, bank, is_percussion);
    if provider_indices.is_empty() {
        return font_ids
            .iter()
            .enumerate()
            .map(|(font_index, _)| RoutingDecision {
                font_index,
                volume: 0.0,
                muted: true,
                unsupported: true,
            })
            .collect();
    }

    match preset.blend_mode {
        BlendMode::ReplaceMute => replace_mute_decisions(font_ids, &provider_indices),
        BlendMode::BlendEven => blend_even_decisions(font_ids, &provider_indices),
        BlendMode::BlendWeight => blend_weight_decisions(preset, font_ids, &provider_indices),
    }
}

fn provider_indices(
    font_ids: &[String],
    coverage_cache: &BTreeMap<String, SoundFontCoverage>,
    program: u8,
    bank: u16,
    is_percussion: bool,
) -> Vec<usize> {
    font_ids
        .iter()
        .enumerate()
        .filter_map(|(index, font_id)| {
            coverage_cache.get(font_id).and_then(|coverage| {
                let provides = if is_percussion {
                    coverage.provides_percussion()
                } else {
                    coverage.provides_melodic(bank, program)
                };
                provides.then_some(index)
            })
        })
        .collect()
}

fn replace_mute_decisions(font_ids: &[String], provider_indices: &[usize]) -> Vec<RoutingDecision> {
    let newest_provider = provider_indices.last().copied();
    font_ids
        .iter()
        .enumerate()
        .map(|(font_index, _)| RoutingDecision {
            font_index,
            volume: if Some(font_index) == newest_provider {
                1.0
            } else {
                0.0
            },
            muted: Some(font_index) != newest_provider,
            unsupported: false,
        })
        .collect()
}

fn blend_even_decisions(font_ids: &[String], provider_indices: &[usize]) -> Vec<RoutingDecision> {
    let provider_count = provider_indices.len().max(1) as f32;
    font_ids
        .iter()
        .enumerate()
        .map(|(font_index, _)| {
            let is_provider = provider_indices.contains(&font_index);
            RoutingDecision {
                font_index,
                volume: if is_provider {
                    1.0 / provider_count
                } else {
                    0.0
                },
                muted: !is_provider,
                unsupported: false,
            }
        })
        .collect()
}

fn blend_weight_decisions(
    preset: &Preset,
    font_ids: &[String],
    provider_indices: &[usize],
) -> Vec<RoutingDecision> {
    let provider_weight_sum = provider_indices
        .iter()
        .filter_map(|font_index| preset.weight_for_font(&font_ids[*font_index]))
        .sum::<u32>()
        .max(1) as f32;

    font_ids
        .iter()
        .enumerate()
        .map(|(font_index, font_id)| {
            let provider_weight = provider_indices
                .contains(&font_index)
                .then(|| preset.weight_for_font(font_id))
                .flatten();
            RoutingDecision {
                font_index,
                volume: provider_weight
                    .map(|weight| weight as f32 / provider_weight_sum)
                    .unwrap_or(0.0),
                muted: provider_weight.is_none(),
                unsupported: false,
            }
        })
        .collect()
}
