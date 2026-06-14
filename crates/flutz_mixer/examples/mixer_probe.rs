use std::{env, mem};

use flutz_core::{SoundFontId, StripId};
use flutz_mixer::{
    AudioBlock, AudioBlockView, AutoNormalization, MasterControls, MixerEngine, MixerSettings,
    MixerStripControls, MixerStripIdentity, MixerStripInput, MixerStripInputView, SmartMixSettings,
    StereoFrame,
};

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

fn run_baseline() {
    let mut engine = MixerEngine::new(MixerSettings::default());
    let strips = vec![
        fixture_strip(
            1,
            "retro_gm",
            0,
            0,
            0.82,
            -0.25,
            MixerStripControls::default(),
            512,
        ),
        fixture_strip(
            2,
            "realistic_gm",
            1,
            48,
            0.62,
            0.35,
            MixerStripControls::default(),
            512,
        ),
    ];
    let report = engine.mix(&strips).expect("baseline fixture should mix");
    println!("mode=baseline input_strip_count={}", strips.len());
    print_report("baseline", &report);
}

fn run_stateful() {
    let mut engine = MixerEngine::new(MixerSettings::default());
    let first_block = vec![
        fixture_strip(
            1,
            "retro_gm",
            0,
            0,
            0.82,
            -0.25,
            MixerStripControls::default(),
            512,
        ),
        fixture_strip(
            2,
            "realistic_gm",
            1,
            48,
            0.62,
            0.35,
            MixerStripControls::default(),
            512,
        ),
    ];
    let second_block = vec![
        fixture_strip(
            1,
            "retro_gm",
            0,
            0,
            0.42,
            -0.25,
            MixerStripControls::default(),
            512,
        ),
        fixture_strip(
            2,
            "realistic_gm",
            1,
            48,
            0.98,
            0.35,
            MixerStripControls::default(),
            512,
        ),
    ];

    println!("mode=stateful input_strip_count={}", first_block.len());
    let report = engine
        .mix(&first_block)
        .expect("stateful first block should mix");
    print_report("stateful_block_1", &report);
    let report = engine
        .mix(&second_block)
        .expect("stateful second block should mix");
    print_report("stateful_block_2", &report);
}

fn run_solo() {
    let mut engine = MixerEngine::new(MixerSettings::default());
    let strips = vec![
        fixture_strip(
            1,
            "retro_gm",
            0,
            0,
            0.82,
            -0.25,
            MixerStripControls::default(),
            512,
        ),
        fixture_strip(
            2,
            "realistic_gm",
            9,
            0,
            0.62,
            0.35,
            MixerStripControls {
                mute: true,
                solo: true,
                ..MixerStripControls::default()
            },
            512,
        ),
    ];
    println!("mode=solo input_strip_count={}", strips.len());
    let report = engine.mix(&strips).expect("solo fixture should mix");
    print_report("solo", &report);
}

fn run_effects() {
    let settings = MixerSettings {
        master: MasterControls {
            reverb: 35.0,
            eq_low_db: 2.5,
            eq_mid_db: -1.5,
            eq_high_db: 3.0,
            ..MasterControls::default()
        },
        ..MixerSettings::default()
    };
    let mut engine = MixerEngine::new(settings);
    let strips = vec![
        fixture_strip(
            1,
            "retro_gm",
            0,
            0,
            0.58,
            -0.15,
            MixerStripControls {
                reverb: 42.0,
                chorus: 65.0,
                ..MixerStripControls::default()
            },
            1_024,
        ),
        fixture_strip(
            2,
            "realistic_gm",
            4,
            81,
            0.44,
            0.22,
            MixerStripControls {
                gain_db: -3.0,
                reverb: 20.0,
                chorus: 30.0,
                ..MixerStripControls::default()
            },
            1_024,
        ),
    ];
    println!("mode=effects input_strip_count={}", strips.len());
    let report = engine.mix(&strips).expect("effects fixture should mix");
    print_report("effects", &report);
}

fn run_invalid() {
    let mut engine = MixerEngine::new(MixerSettings::default());
    let strips = vec![
        fixture_strip(
            1,
            "retro_gm",
            0,
            0,
            0.5,
            0.0,
            MixerStripControls::default(),
            512,
        ),
        fixture_strip(
            2,
            "retro_gm",
            1,
            40,
            0.5,
            0.0,
            MixerStripControls::default(),
            256,
        ),
    ];
    println!("mode=invalid input_strip_count={}", strips.len());
    match engine.mix(&strips) {
        Ok(_) => println!("invalid_expected_error=false"),
        Err(error) => println!("invalid_expected_error=true message={error}"),
    }
}

fn run_neutral_probe() {
    let mut engine = MixerEngine::new(MixerSettings::default());
    let frames = vec![
        StereoFrame {
            left: 0.25,
            right: -0.5,
        },
        StereoFrame {
            left: -0.75,
            right: 0.125,
        },
    ];
    let report = engine
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            frames.clone(),
            MixerStripControls::default(),
        )])
        .expect("neutral fixture should mix");
    let passed = report.output.frames == frames;
    println!("mode=neutral pass={passed}");
    println!("neutral_expected_frames={frames:?}");
    println!("neutral_actual_frames={:?}", report.output.frames);
}

fn run_ramp_probe() {
    let mut engine = MixerEngine::new(MixerSettings::default());
    let frames = vec![
        StereoFrame {
            left: 1.0,
            right: 1.0,
        };
        4
    ];
    engine
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            frames.clone(),
            MixerStripControls::default(),
        )])
        .expect("ramp warmup block should mix");
    let report = engine
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            frames,
            MixerStripControls {
                volume: 0.0,
                ..MixerStripControls::default()
            },
        )])
        .expect("ramp fixture should mix");
    let rendered = report
        .output
        .frames
        .iter()
        .map(|frame| frame.left)
        .collect::<Vec<_>>();
    let expected = vec![1.0, 0.75, 0.5, 0.25];
    let passed = rendered == expected;
    println!("mode=ramp pass={passed}");
    println!("ramp_expected_left={expected:?}");
    println!("ramp_actual_left={rendered:?}");
}

fn run_pan_probe() {
    let center = MixerStripControls::default().pan_gains();
    let left = MixerStripControls {
        pan: -1.0,
        ..MixerStripControls::default()
    }
    .pan_gains();
    let right = MixerStripControls {
        pan: 1.0,
        ..MixerStripControls::default()
    }
    .pan_gains();
    let passed = center == (1.0, 1.0)
        && (left.0 - std::f32::consts::SQRT_2).abs() < f32::EPSILON
        && left.1.abs() < f32::EPSILON
        && right.0.abs() < 0.000_001
        && (right.1 - std::f32::consts::SQRT_2).abs() < f32::EPSILON;
    println!("mode=pan pass={passed}");
    println!("pan_center={center:?}");
    println!("pan_left={left:?}");
    println!("pan_right={right:?}");
}

fn run_mute_tail_probe() {
    let impulse = {
        let mut frames = vec![StereoFrame::default(); 1_536];
        frames[0] = StereoFrame {
            left: 1.0,
            right: 1.0,
        };
        frames
    };
    let silence = vec![StereoFrame::default(); 1_536];
    let wet_controls = MixerStripControls {
        reverb: 100.0,
        ..MixerStripControls::default()
    };

    let mut unmuted_engine = MixerEngine::new(MixerSettings::default());
    unmuted_engine
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            impulse.clone(),
            wet_controls,
        )])
        .expect("mute-tail warmup should mix");
    let unmuted_tail = unmuted_engine
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            silence.clone(),
            wet_controls,
        )])
        .expect("unmuted tail should mix");

    let mut muted_engine = MixerEngine::new(MixerSettings::default());
    muted_engine
        .mix(&[block_strip(1, "retro_gm", 0, 0, impulse, wet_controls)])
        .expect("muted warmup should mix");
    let muted_tail = muted_engine
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            silence,
            MixerStripControls {
                mute: true,
                reverb: 100.0,
                ..MixerStripControls::default()
            },
        )])
        .expect("muted tail should mix");
    let passed = unmuted_tail.output_meter.peak > 0.0 && muted_tail.output_meter.peak == 0.0;
    println!("mode=mute-tail pass={passed}");
    println!(
        "mute_tail_unmuted_peak={:.9}",
        unmuted_tail.output_meter.peak
    );
    println!("mute_tail_muted_peak={:.9}", muted_tail.output_meter.peak);
}

fn run_automatic_probe() {
    let settings = MixerSettings {
        smart_mix: SmartMixSettings {
            enabled: true,
            target_headroom: 0.5,
            attack: 1.0,
            release: 1.0,
            lookahead: 0.0,
        },
        auto_normalization: AutoNormalization {
            enabled: true,
            amount: 1.0,
        },
        ..MixerSettings::default()
    };
    let mut engine = MixerEngine::new(settings);
    let frames = vec![
        StereoFrame {
            left: 0.25,
            right: -0.25,
        };
        8
    ];
    let report = engine
        .mix(&[
            block_strip_with_auto(
                1,
                "retro_gm",
                0,
                0,
                frames.clone(),
                MixerStripControls::default(),
                true,
            ),
            block_strip_with_auto(
                2,
                "retro_gm",
                16,
                127,
                frames,
                MixerStripControls::default(),
                false,
            ),
        ])
        .expect("automatic processing fixture should mix");
    let real = report
        .strips
        .iter()
        .find(|strip| strip.identity.strip_id == StripId(1))
        .expect("real stem report should exist");
    let residual = report
        .strips
        .iter()
        .find(|strip| strip.identity.strip_id == StripId(2))
        .expect("residual report should exist");
    let passed = (real.normalization_gain - 2.0).abs() < f32::EPSILON
        && (real.smart_mix_gain - 1.0).abs() < f32::EPSILON
        && (residual.normalization_gain - 1.0).abs() < f32::EPSILON
        && (residual.smart_mix_gain - 1.0).abs() < f32::EPSILON;
    println!("mode=automatic pass={passed}");
    println!(
        "automatic_real_normalization={:.9} real_smart_mix={:.9}",
        real.normalization_gain, real.smart_mix_gain
    );
    println!(
        "automatic_residual_normalization={:.9} residual_smart_mix={:.9}",
        residual.normalization_gain, residual.smart_mix_gain
    );
}

fn run_master_chain_probe() {
    let source = vec![
        StereoFrame {
            left: 1.0,
            right: -1.0,
        };
        1_024
    ];
    let mut neutral = MixerEngine::new(MixerSettings::default());
    let neutral_report = neutral
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            source.clone(),
            MixerStripControls::default(),
        )])
        .expect("neutral master fixture should mix");

    let settings = MixerSettings {
        master: MasterControls {
            limiter: flutz_mixer::effects::LimiterControls {
                enabled: true,
                amount: 0.5,
                ..flutz_mixer::effects::LimiterControls::default()
            },
            reverb: 35.0,
            chorus: 45.0,
            eq_low_db: 3.0,
            eq_mid_db: -2.0,
            eq_high_db: 4.0,
            ..MasterControls::default()
        },
        ..MixerSettings::default()
    };
    let mut processed = MixerEngine::new(settings);
    let processed_report = processed
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            source,
            MixerStripControls::default(),
        )])
        .expect("processed master fixture should mix");
    let changed = processed_report.output.frames != neutral_report.output.frames;
    let limited = processed_report.output_meter.peak <= 0.5 + f32::EPSILON;
    let passed = changed && limited;
    println!("mode=master-chain pass={passed}");
    println!("master_chain_changed={changed}");
    println!("master_chain_limited={limited}");
    println!(
        "master_chain_neutral_peak={:.9} processed_peak={:.9}",
        neutral_report.output_meter.peak, processed_report.output_meter.peak
    );
}

fn run_silent_solo_probe() {
    let mut engine = MixerEngine::new(MixerSettings::default());
    let active_frames = vec![
        StereoFrame {
            left: 0.75,
            right: -0.25,
        };
        16
    ];
    let silent_frames = vec![StereoFrame::default(); 16];
    let report = engine
        .mix(&[
            block_strip(
                1,
                "retro_gm",
                7,
                102,
                active_frames,
                MixerStripControls::default(),
            ),
            block_strip(
                2,
                "retro_gm",
                9,
                25,
                silent_frames,
                MixerStripControls {
                    solo: true,
                    ..MixerStripControls::default()
                },
            ),
        ])
        .expect("silent solo fixture should mix");
    let passed = report.output_meter.peak == 0.0
        && report.strips.iter().any(|strip| {
            strip.identity.strip_id == StripId(1) && !strip.audible && strip.solo_active
        })
        && report.strips.iter().any(|strip| {
            strip.identity.strip_id == StripId(2) && !strip.audible && strip.solo_active
        });
    println!("mode=silent-solo pass={passed}");
    println!("silent_solo_output_peak={:.9}", report.output_meter.peak);
    print_report("silent_solo", &report);
}

fn run_smart_mix_balance_probe() {
    let settings = MixerSettings {
        smart_mix: SmartMixSettings {
            enabled: true,
            target_headroom: 0.25,
            attack: 1.0,
            release: 1.0,
            lookahead: 0.0,
        },
        auto_normalization: AutoNormalization::default(),
        ..MixerSettings::default()
    };

    let frames_loud = (0..1024)
        .map(|i| {
            let sample = (i as f32 / 64.0 * std::f32::consts::TAU).sin() * 0.9;
            StereoFrame {
                left: sample,
                right: sample,
            }
        })
        .collect::<Vec<_>>();
    let frames_quiet = (0..1024)
        .map(|i| {
            let sample = (i as f32 / 64.0 * std::f32::consts::TAU).sin() * 0.3;
            StereoFrame {
                left: sample,
                right: sample,
            }
        })
        .collect::<Vec<_>>();

    let mut equal_mix = MixerEngine::new(settings);
    let equal_report = equal_mix
        .mix(&[
            block_strip_with_auto(
                1,
                "retro_gm",
                0,
                0,
                frames_loud.clone(),
                MixerStripControls::default(),
                true,
            ),
            block_strip_with_auto(
                2,
                "retro_gm",
                1,
                1,
                frames_quiet.clone(),
                MixerStripControls::default(),
                true,
            ),
        ])
        .expect("smart-mix equal fixture should mix");

    let loud_equal = equal_report
        .strips
        .iter()
        .find(|strip| strip.identity.strip_id == StripId(1))
        .expect("equal loud strip report should exist")
        .smart_mix_gain;
    let quiet_equal = equal_report
        .strips
        .iter()
        .find(|strip| strip.identity.strip_id == StripId(2))
        .expect("equal quiet strip report should exist")
        .smart_mix_gain;

    let mut weighted_mix = MixerEngine::new(settings);
    let weighted_report = weighted_mix
        .mix(&[
            block_strip_with_auto(
                1,
                "retro_gm",
                0,
                0,
                frames_loud,
                MixerStripControls {
                    volume: 2.0,
                    ..MixerStripControls::default()
                },
                true,
            ),
            block_strip_with_auto(
                2,
                "retro_gm",
                1,
                1,
                frames_quiet,
                MixerStripControls {
                    volume: 0.5,
                    ..MixerStripControls::default()
                },
                true,
            ),
        ])
        .expect("smart-mix weighted fixture should mix");

    let loud_weighted = weighted_report
        .strips
        .iter()
        .find(|strip| strip.identity.strip_id == StripId(1))
        .expect("weighted loud strip report should exist")
        .smart_mix_gain;
    let pass_equal_balancing = loud_equal < quiet_equal;
    let pass_weight_respected = loud_weighted > loud_equal;
    let passed = pass_equal_balancing && pass_weight_respected;

    println!("mode=smart-mix-balance pass={passed}");
    println!(
        "smart_mix_equal_loud={:.9} smart_mix_equal_quiet={:.9} smart_mix_weighted_loud={:.9}",
        loud_equal, quiet_equal, loud_weighted
    );
}

fn run_effects_quality_probe() {
    let source_frames = (0..2_048)
        .map(|i| {
            let sample = (i as f32 / 72.0 * std::f32::consts::TAU).sin() * 0.6;
            StereoFrame {
                left: sample,
                right: sample * 0.9,
            }
        })
        .collect::<Vec<_>>();

    let mut dry_engine = MixerEngine::new(MixerSettings::default());
    let dry_report = dry_engine
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            source_frames.clone(),
            MixerStripControls::default(),
        )])
        .expect("effects quality dry fixture should mix");

    let mut wet_engine = MixerEngine::new(MixerSettings {
        master: MasterControls {
            reverb: 65.0,
            chorus: 70.0,
            ..MasterControls::default()
        },
        ..MixerSettings::default()
    });
    let wet_report = wet_engine
        .mix(&[block_strip(
            1,
            "retro_gm",
            0,
            0,
            source_frames,
            MixerStripControls {
                reverb: 70.0,
                chorus: 75.0,
                ..MixerStripControls::default()
            },
        )])
        .expect("effects quality wet fixture should mix");

    let changed = wet_report.output.frames != dry_report.output.frames;
    let finite = wet_report
        .output
        .frames
        .iter()
        .all(|frame| frame.left.is_finite() && frame.right.is_finite());
    let max_jump = wet_report
        .output
        .frames
        .windows(2)
        .map(|window| (window[1].left - window[0].left).abs())
        .fold(0.0f32, f32::max);
    let passed = changed && finite && max_jump < 3.0;

    println!("mode=effects-quality pass={passed}");
    println!("effects_quality_changed={changed} finite={finite} max_jump={max_jump:.9}");
    println!(
        "effects_quality_dry_peak={:.9} wet_peak={:.9}",
        dry_report.output_meter.peak, wet_report.output_meter.peak
    );
}

fn run_sparse_silence_probe() {
    let frame_count = 128;
    let active_frames = vec![
        StereoFrame {
            left: 0.25,
            right: -0.25,
        };
        frame_count
    ];
    let mut sparse_inputs = vec![MixerStripInputView {
        identity: probe_identity(1, 0, 0),
        controls: MixerStripControls::default(),
        automatic_processing: true,
        block: AudioBlockView::Stereo(&active_frames),
    }];
    for strip_id in 2..=5 {
        sparse_inputs.push(MixerStripInputView {
            identity: probe_identity(strip_id, 0, strip_id as u8),
            controls: MixerStripControls::default(),
            automatic_processing: true,
            block: AudioBlockView::Silence { frame_count },
        });
    }
    let mut sparse_engine = MixerEngine::new(MixerSettings::default());
    let sparse_report = sparse_engine
        .mix_views(&sparse_inputs)
        .expect("sparse silence fixture should mix");
    let expected_single_strip_bytes = frame_count * mem::size_of::<StereoFrame>();
    let sparse_pass = sparse_report.strips.len() == sparse_inputs.len()
        && sparse_report.allocation_stats.processed_inputs == 1
        && sparse_report.allocation_stats.prepared_rendered_frame_bytes == 0
        && sparse_report
            .allocation_stats
            .retained_scratch_prepared_frame_bytes
            >= expected_single_strip_bytes
        && sparse_report.output_meter.peak > 0.0;

    let mut impulse_frames = vec![StereoFrame::default(); 4_096];
    impulse_frames[0] = StereoFrame {
        left: 1.0,
        right: 1.0,
    };
    let mut tail_controls = MixerStripControls::default();
    tail_controls.reverb = 100.0;
    let tail_identity = probe_identity(10, 0, 10);
    let mut tail_engine = MixerEngine::new(MixerSettings::default());
    let warmup_inputs = [MixerStripInputView {
        identity: tail_identity.clone(),
        controls: tail_controls,
        automatic_processing: true,
        block: AudioBlockView::Stereo(&impulse_frames),
    }];
    let warmup_report = tail_engine
        .mix_views(&warmup_inputs)
        .expect("sparse tail warmup should mix");
    let tail_inputs = [MixerStripInputView {
        identity: tail_identity,
        controls: tail_controls,
        automatic_processing: true,
        block: AudioBlockView::Silence { frame_count: 4_096 },
    }];
    let tail_report = tail_engine
        .mix_views(&tail_inputs)
        .expect("sparse tail fixture should mix");
    let expected_tail_bytes = 4_096 * mem::size_of::<StereoFrame>();
    let tail_pass = warmup_report.allocation_stats.processed_inputs == 1
        && tail_report.allocation_stats.processed_inputs == 1
        && tail_report.allocation_stats.prepared_rendered_frame_bytes == 0
        && tail_report
            .allocation_stats
            .retained_scratch_prepared_frame_bytes
            >= expected_tail_bytes
        && tail_report.output_meter.peak > 0.0;

    let solo_frames = vec![
        StereoFrame {
            left: 0.5,
            right: 0.5,
        };
        frame_count
    ];
    let mut solo_controls = MixerStripControls::default();
    solo_controls.solo = true;
    let solo_inputs = [
        MixerStripInputView {
            identity: probe_identity(20, 0, 20),
            controls: MixerStripControls::default(),
            automatic_processing: true,
            block: AudioBlockView::Stereo(&solo_frames),
        },
        MixerStripInputView {
            identity: probe_identity(21, 0, 21),
            controls: solo_controls,
            automatic_processing: true,
            block: AudioBlockView::Silence { frame_count },
        },
    ];
    let mut solo_engine = MixerEngine::new(MixerSettings::default());
    let solo_report = solo_engine
        .mix_views(&solo_inputs)
        .expect("sparse silent solo fixture should mix");
    let solo_pass = solo_report.strips.len() == solo_inputs.len()
        && solo_report.allocation_stats.processed_inputs == 0
        && solo_report.allocation_stats.prepared_rendered_frame_bytes == 0
        && solo_report.output_meter.peak == 0.0
        && solo_report.strips.iter().all(|strip| strip.solo_active);

    let passed = sparse_pass && tail_pass && solo_pass;
    println!("mode=sparse-silence pass={passed}");
    println!(
        "sparse_silence_reports={} processed_inputs={} prepared_frame_bytes={} retained_prepared_frame_bytes={} expected_prepared_frame_bytes={} output_peak={:.9}",
        sparse_report.strips.len(),
        sparse_report.allocation_stats.processed_inputs,
        sparse_report.allocation_stats.prepared_rendered_frame_bytes,
        sparse_report
            .allocation_stats
            .retained_scratch_prepared_frame_bytes,
        expected_single_strip_bytes,
        sparse_report.output_meter.peak
    );
    println!(
        "sparse_silence_tail_processed_inputs={} tail_prepared_frame_bytes={} tail_retained_prepared_frame_bytes={} expected_tail_prepared_frame_bytes={} tail_output_peak={:.9}",
        tail_report.allocation_stats.processed_inputs,
        tail_report.allocation_stats.prepared_rendered_frame_bytes,
        tail_report
            .allocation_stats
            .retained_scratch_prepared_frame_bytes,
        expected_tail_bytes,
        tail_report.output_meter.peak
    );
    println!(
        "sparse_silence_solo_processed_inputs={} solo_prepared_frame_bytes={} solo_output_peak={:.9} solo_all_solo_active={}",
        solo_report.allocation_stats.processed_inputs,
        solo_report.allocation_stats.prepared_rendered_frame_bytes,
        solo_report.output_meter.peak,
        solo_report.strips.iter().all(|strip| strip.solo_active)
    );
}

fn run_scratch_reuse_probe() {
    let frame_count = 512;
    let frames = vec![
        StereoFrame {
            left: 0.35,
            right: -0.2,
        };
        frame_count
    ];
    let inputs = [
        MixerStripInputView {
            identity: probe_identity(1, 0, 0),
            controls: MixerStripControls::default(),
            automatic_processing: true,
            block: AudioBlockView::Stereo(&frames),
        },
        MixerStripInputView {
            identity: probe_identity(2, 1, 48),
            controls: MixerStripControls::default(),
            automatic_processing: true,
            block: AudioBlockView::Stereo(&frames),
        },
    ];
    let mut output = vec![0.0; frame_count * 2];
    let mut engine = MixerEngine::new(MixerSettings::default());
    let first = engine
        .mix_views_interleaved(&inputs, &mut output)
        .expect("scratch reuse first block should mix");
    let first_scratch = engine.scratch_stats();
    let second = engine
        .mix_views_interleaved(&inputs, &mut output)
        .expect("scratch reuse second block should mix");
    let second_scratch = engine.scratch_stats();
    let mut empty_output = vec![1.0; frame_count * 2];
    let empty = engine
        .mix_views_interleaved(&[], &mut empty_output)
        .expect("empty scratch-backed mix should zero caller output");
    let empty_scratch = engine.scratch_stats();
    let expected_prepared = inputs.len() * frame_count * mem::size_of::<StereoFrame>();
    let passed = first.allocation_stats.scratch_growth_bytes > 0
        && second.allocation_stats.scratch_growth_bytes == 0
        && second.allocation_stats.output_bytes == 0
        && second.allocation_stats.prepared_rendered_frame_bytes == 0
        && second
            .allocation_stats
            .retained_scratch_prepared_frame_bytes
            >= expected_prepared
        && second_scratch.retained_bytes == first_scratch.retained_bytes
        && empty.allocation_stats.scratch_growth_bytes == 0
        && empty_scratch.retained_bytes == second_scratch.retained_bytes
        && empty_output.iter().all(|sample| *sample == 0.0)
        && second.output_meter.peak > 0.0;

    println!("mode=scratch-reuse pass={passed}");
    println!(
        "scratch_reuse_first_growth={} second_growth={} retained_bytes={} retained_prepared_frame_bytes={} expected_prepared_frame_bytes={}",
        first.allocation_stats.scratch_growth_bytes,
        second.allocation_stats.scratch_growth_bytes,
        second_scratch.retained_bytes,
        second_scratch.prepared_rendered_frame_bytes,
        expected_prepared
    );
    println!(
        "scratch_reuse_second_allocated_bytes={} output_bytes={} reports_bytes={} prepared_frame_bytes={} output_peak={:.9}",
        second.allocation_stats.allocated_bytes,
        second.allocation_stats.output_bytes,
        second.allocation_stats.reports_bytes,
        second.allocation_stats.prepared_rendered_frame_bytes,
        second.output_meter.peak
    );
    println!(
        "scratch_reuse_empty_growth={} empty_retained_bytes={} empty_output_zeroed={}",
        empty.allocation_stats.scratch_growth_bytes,
        empty_scratch.retained_bytes,
        empty_output.iter().all(|sample| *sample == 0.0)
    );
}

fn run_extent_reuse_probe() {
    let scenarios = [(256usize, 2usize), (512, 3), (1024, 3), (512, 2), (1024, 3)];
    let max_frames = scenarios
        .iter()
        .map(|(frames, _)| *frames)
        .max()
        .unwrap_or(0);
    let max_strips = scenarios
        .iter()
        .map(|(_, strips)| *strips)
        .max()
        .unwrap_or(0);
    let frame_bank = (0..max_strips)
        .map(|strip_index| {
            vec![
                StereoFrame {
                    left: 0.1 + strip_index as f32 * 0.03,
                    right: -0.08 + strip_index as f32 * 0.02,
                };
                max_frames
            ]
        })
        .collect::<Vec<_>>();
    let mut engine = MixerEngine::new(MixerSettings::default());
    let mut high_water_retained = 0usize;
    let mut plateau_ok = true;
    let mut output_peak = 0.0f32;

    println!("mode=extent-reuse");
    for (index, (frame_count, strip_count)) in scenarios.iter().copied().enumerate() {
        let inputs = (0..strip_count)
            .map(|strip_index| MixerStripInputView {
                identity: probe_identity((strip_index + 1) as u64, strip_index as u8, 8),
                controls: MixerStripControls::default(),
                automatic_processing: true,
                block: AudioBlockView::Stereo(&frame_bank[strip_index][..frame_count]),
            })
            .collect::<Vec<_>>();
        let mut output = vec![0.0; frame_count * 2];
        let report = engine
            .mix_views_interleaved(&inputs, &mut output)
            .expect("extent reuse scenario should mix");
        let scratch = engine.scratch_stats();
        let expected_prepared_bytes = frame_count
            .saturating_mul(strip_count)
            .saturating_mul(mem::size_of::<StereoFrame>());
        let expected_growth = scratch.retained_bytes > high_water_retained;
        let actual_growth = report.allocation_stats.scratch_growth_bytes > 0;
        if index > 0 && actual_growth != expected_growth {
            plateau_ok = false;
        }
        high_water_retained = high_water_retained.max(scratch.retained_bytes);
        output_peak = output_peak.max(report.output_meter.peak);
        println!(
            "extent_reuse_step={} frames={} strips={} growth={} retained={} retained_prepared={} expected_prepared={}",
            index,
            frame_count,
            strip_count,
            report.allocation_stats.scratch_growth_bytes,
            scratch.retained_bytes,
            scratch.prepared_rendered_frame_bytes,
            expected_prepared_bytes
        );
    }

    let post_plateau = {
        let frame_count = 512usize;
        let strip_count = 2usize;
        let inputs = (0..strip_count)
            .map(|strip_index| MixerStripInputView {
                identity: probe_identity((strip_index + 1) as u64, strip_index as u8, 8),
                controls: MixerStripControls::default(),
                automatic_processing: true,
                block: AudioBlockView::Stereo(&frame_bank[strip_index][..frame_count]),
            })
            .collect::<Vec<_>>();
        let mut output = vec![0.0; frame_count * 2];
        engine
            .mix_views_interleaved(&inputs, &mut output)
            .expect("extent reuse plateau should mix")
            .allocation_stats
            .scratch_growth_bytes
    };
    let passed = plateau_ok && post_plateau == 0 && output_peak > 0.0;
    println!(
        "extent_reuse_pass={} plateau_ok={} post_plateau_growth={} high_water_retained={} output_peak={:.9}",
        passed, plateau_ok, post_plateau, high_water_retained, output_peak
    );
}

fn run_scratch_release_probe() {
    let frame_count = 1024;
    let frames = vec![
        StereoFrame {
            left: 0.2,
            right: 0.15,
        };
        frame_count
    ];
    let inputs = [MixerStripInputView {
        identity: probe_identity(7, 3, 12),
        controls: MixerStripControls::default(),
        automatic_processing: true,
        block: AudioBlockView::Stereo(&frames),
    }];
    let mut output = vec![0.0; frame_count * 2];
    let mut engine = MixerEngine::new(MixerSettings::default());
    let report = engine
        .mix_views_interleaved(&inputs, &mut output)
        .expect("scratch release warmup should mix");
    let grown = engine.scratch_stats();
    engine.release_scratch_capacity();
    let released = engine.scratch_stats();
    let second = engine
        .mix_views_interleaved(&inputs, &mut output)
        .expect("scratch release second block should mix");
    let regrown = engine.scratch_stats();
    engine.reset_state_and_release_scratch();
    let reset_released = engine.scratch_stats();
    let passed = report.allocation_stats.scratch_growth_bytes > 0
        && grown.retained_bytes > 0
        && released.retained_bytes == 0
        && second.allocation_stats.scratch_growth_bytes > 0
        && regrown.retained_bytes > 0
        && reset_released.retained_bytes == 0
        && second.output_meter.peak > 0.0;

    println!("mode=scratch-release pass={passed}");
    println!(
        "scratch_release_growth={} grown_retained={} released_retained={} regrowth={} regrown_retained={} reset_released_retained={}",
        report.allocation_stats.scratch_growth_bytes,
        grown.retained_bytes,
        released.retained_bytes,
        second.allocation_stats.scratch_growth_bytes,
        regrown.retained_bytes,
        reset_released.retained_bytes
    );
}

fn print_report(label: &str, report: &flutz_mixer::MixReport) {
    println!("{label}_output_frames: {}", report.output.frame_count());
    for strip in &report.strips {
        println!(
            "{label}_strip {} channel={} program={} percussion={} peak={:.6} rms={:.6} gain={:.6} smart_mix={:.6} lookahead={:.6} normalization={:.6} audible={} solo_active={}",
            strip.identity.strip_id.0,
            strip.identity.midi_channel,
            strip.identity.midi_program,
            strip.identity.is_percussion,
            strip.input_meter.peak,
            strip.input_meter.rms,
            strip.applied_gain,
            strip.smart_mix_gain,
            strip.lookahead_gain,
            strip.normalization_gain,
            strip.audible,
            strip.solo_active
        );
    }
    println!("{label}_output_peak: {:.6}", report.output_meter.peak);
    println!("{label}_output_rms: {:.6}", report.output_meter.rms);
    println!(
        "{label}_output_headroom: {:.6}",
        report.output_meter.headroom
    );
    println!(
        "{label}_allocation_stats processed_inputs={} allocated_bytes={} prepared_frame_bytes={} output_bytes={} reports_bytes={} scratch_growth_bytes={} retained_scratch_bytes={} retained_prepared_frame_bytes={}",
        report.allocation_stats.processed_inputs,
        report.allocation_stats.allocated_bytes,
        report.allocation_stats.prepared_rendered_frame_bytes,
        report.allocation_stats.output_bytes,
        report.allocation_stats.reports_bytes,
        report.allocation_stats.scratch_growth_bytes,
        report.allocation_stats.retained_scratch_bytes,
        report
            .allocation_stats
            .retained_scratch_prepared_frame_bytes
    );
}

fn probe_identity(strip_id: u64, channel: u8, program: u8) -> MixerStripIdentity {
    MixerStripIdentity {
        strip_id: StripId(strip_id),
        soundfont_id: SoundFontId::new("probe"),
        midi_channel: channel,
        midi_program: program,
        is_percussion: channel == 9,
    }
}

fn fixture_strip(
    strip_id: u64,
    soundfont_id: &str,
    channel: u8,
    program: u8,
    amplitude: f32,
    pan: f64,
    controls: MixerStripControls,
    frame_count: usize,
) -> MixerStripInput {
    let frames = (0..frame_count)
        .map(|index| {
            let phase = index as f32 / 512.0;
            let value = (phase * std::f32::consts::TAU * (channel as f32 + 1.0)).sin() * amplitude;
            StereoFrame {
                left: value,
                right: value * 0.75,
            }
        })
        .collect();

    MixerStripInput {
        identity: MixerStripIdentity {
            strip_id: StripId(strip_id),
            soundfont_id: SoundFontId::new(soundfont_id),
            midi_channel: channel,
            midi_program: program,
            is_percussion: channel == 9,
        },
        controls: MixerStripControls { pan, ..controls },
        automatic_processing: true,
        block: AudioBlock { frames },
    }
}

fn block_strip(
    strip_id: u64,
    soundfont_id: &str,
    channel: u8,
    program: u8,
    frames: Vec<StereoFrame>,
    controls: MixerStripControls,
) -> MixerStripInput {
    block_strip_with_auto(
        strip_id,
        soundfont_id,
        channel,
        program,
        frames,
        controls,
        true,
    )
}

fn block_strip_with_auto(
    strip_id: u64,
    soundfont_id: &str,
    channel: u8,
    program: u8,
    frames: Vec<StereoFrame>,
    controls: MixerStripControls,
    automatic_processing: bool,
) -> MixerStripInput {
    MixerStripInput {
        identity: MixerStripIdentity {
            strip_id: StripId(strip_id),
            soundfont_id: SoundFontId::new(soundfont_id),
            midi_channel: channel,
            midi_program: program,
            is_percussion: channel == 9,
        },
        controls,
        automatic_processing,
        block: AudioBlock { frames },
    }
}
