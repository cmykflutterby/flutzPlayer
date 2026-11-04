use std::{env, mem};

use flutz_core::{SoundFontId, StripId};
use flutz_mixer::{
    AudioBlock, AudioBlockView, AutoNormalization, MasterControls, MixerEngine, MixerSettings,
    MixerStripControls, MixerStripIdentity, MixerStripInput, MixerStripInputView, SmartMixSettings,
    StereoFrame,
};

#[path = "mixer_probe/fixtures.rs"]
mod fixtures;
#[path = "mixer_probe/reporting.rs"]
mod reporting;
#[path = "mixer_probe/scenarios.rs"]
mod scenarios;
use scenarios::*;

fn main() {
    let requested = env::args().nth(1).unwrap_or_else(|| "all".to_owned());
    let modes = match requested.as_str() {
        "all" => vec![
            "baseline",
            "stateful",
            "solo",
            "effects",
            "invalid",
            "neutral",
            "ramp",
            "pan",
            "mute-tail",
            "automatic",
            "master-chain",
            "silent-solo",
            "smart-mix-balance",
            "effects-quality",
            "sparse-silence",
            "scratch-reuse",
            "extent-reuse",
            "scratch-release",
        ],
        "baseline" | "stateful" | "solo" | "effects" | "invalid" | "neutral" | "ramp" | "pan"
        | "mute-tail" | "automatic" | "master-chain" | "silent-solo" | "smart-mix-balance"
        | "effects-quality" | "sparse-silence" | "scratch-reuse" | "extent-reuse"
        | "scratch-release" => {
            vec![requested.as_str()]
        }
        other => {
            eprintln!("unknown mixer_probe mode: {other}");
            eprintln!(
                "valid modes: all, baseline, stateful, solo, effects, invalid, neutral, ramp, pan, mute-tail, automatic, master-chain, silent-solo, smart-mix-balance, effects-quality, sparse-silence, scratch-reuse, extent-reuse, scratch-release"
            );
            std::process::exit(2);
        }
    };

    println!("mixer_probe: ok");
    for mode in modes {
        match mode {
            "baseline" => run_baseline(),
            "stateful" => run_stateful(),
            "solo" => run_solo(),
            "effects" => run_effects(),
            "invalid" => run_invalid(),
            "neutral" => run_neutral_probe(),
            "ramp" => run_ramp_probe(),
            "pan" => run_pan_probe(),
            "mute-tail" => run_mute_tail_probe(),
            "automatic" => run_automatic_probe(),
            "master-chain" => run_master_chain_probe(),
            "silent-solo" => run_silent_solo_probe(),
            "smart-mix-balance" => run_smart_mix_balance_probe(),
            "effects-quality" => run_effects_quality_probe(),
            "sparse-silence" => run_sparse_silence_probe(),
            "scratch-reuse" => run_scratch_reuse_probe(),
            "extent-reuse" => run_extent_reuse_probe(),
            "scratch-release" => run_scratch_release_probe(),
            _ => unreachable!(),
        }
    }
}
