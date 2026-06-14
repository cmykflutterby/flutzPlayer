#![allow(dead_code)]

use crate::loop_mode::LoopMode;
use crate::synthesizer_settings::SynthesizerSettings;

// In this class, fixed-point numbers are used for speed-up.
// A fixed-point number is expressed by Int64, whose lower 24 bits represent the fraction part,
// and the rest represent the integer part.
// For clarity, fixed-point number variables have a suffix "_fp".

#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct Oscillator {
    synthesizer_sample_rate: i32,

    loop_mode: LoopMode,
    sample_sample_rate: i32,
    start: i32,
    end: i32,
    start_loop: i32,
    end_loop: i32,
    root_key: i32,

    tune: f32,
    pitch_change_scale: f32,
    sample_rate_ratio: f32,

    looping: bool,

    position_fp: i64,
}

impl Oscillator {
    const FRAC_BITS: i32 = 24;
    const FRAC_UNIT: i64 = 1_i64 << Oscillator::FRAC_BITS;
    const FP_TO_SAMPLE: f32 = 1_f32 / (32768 * Oscillator::FRAC_UNIT) as f32;

    pub(crate) fn new(settings: &SynthesizerSettings) -> Self {
        Self {
            synthesizer_sample_rate: settings.sample_rate,
            loop_mode: LoopMode::NoLoop,
            sample_sample_rate: 0,
            start: 0,
            end: 0,
            start_loop: 0,
            end_loop: 0,
            root_key: 0,
            tune: 0_f32,
            pitch_change_scale: 0_f32,
            sample_rate_ratio: 0_f32,
            looping: false,
            position_fp: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &mut self,
        loop_mode: LoopMode,
        sample_rate: i32,
        start: i32,
        end: i32,
        start_loop: i32,
        end_loop: i32,
        root_key: i32,
        coarse_tune: i32,
        fine_tune: i32,
        scale_tuning: i32,
    ) {
        self.loop_mode = loop_mode;
        self.sample_sample_rate = sample_rate;
        self.start = start;
        self.end = end;
        self.start_loop = start_loop;
        self.end_loop = end_loop;
        self.root_key = root_key;

        self.tune = coarse_tune as f32 + 0.01_f32 * fine_tune as f32;
        self.pitch_change_scale = 0.01_f32 * scale_tuning as f32;
        self.sample_rate_ratio = sample_rate as f32 / self.synthesizer_sample_rate as f32;
        self.looping = self.loop_mode != LoopMode::NoLoop;
        self.position_fp = (start as i64) << Oscillator::FRAC_BITS;
    }

    pub(crate) fn release(&mut self) {
        if self.loop_mode == LoopMode::LoopUntilNoteOff {
            self.looping = false;
        }
    }

    pub(crate) fn process(&mut self, data: &[i16], block: &mut [f32], pitch: f32) -> bool {
        let pitch_change = self.pitch_change_scale * (pitch - self.root_key as f32) + self.tune;
        let pitch_ratio = self.sample_rate_ratio * 2_f32.powf(pitch_change / 12_f32);
        self.fill_block(data, block, pitch_ratio as f64)
    }

    fn fill_block(&mut self, data: &[i16], block: &mut [f32], pitch_ratio: f64) -> bool {
        let pitch_ratio_fp = (Oscillator::FRAC_UNIT as f64 * pitch_ratio) as i64;

        if self.looping {
            self.fill_block_continuous(data, block, pitch_ratio_fp)
        } else {
            self.fill_block_no_loop(data, block, pitch_ratio_fp)
        }
    }

    fn fill_block_no_loop(&mut self, data: &[i16], block: &mut [f32], pitch_ratio_fp: i64) -> bool {
        let playback_end = (self.end.max(0) as usize).min(data.len());
        if playback_end == 0 {
            block.fill(0_f32);
            return false;
        }
        let final_index = playback_end - 1;

        for t in 0..block.len() {
            let index = (self.position_fp >> Oscillator::FRAC_BITS) as usize;
            if index >= playback_end {
                if t > 0 {
                    let len = block.len();
                    block[t..len].fill(0_f32);
                    return true;
                } else {
                    return false;
                }
            }

            let (x1, x2) = self.sample_pair_or_panic(
                data,
                index,
                (index + 1).min(final_index),
                t,
                pitch_ratio_fp,
                "no-loop",
            );
            let a_fp = self.position_fp & (Oscillator::FRAC_UNIT - 1);
            block[t] = Oscillator::FP_TO_SAMPLE
                * ((x1 << Oscillator::FRAC_BITS) + a_fp * (x2 - x1)) as f32;

            self.position_fp += pitch_ratio_fp;
        }

        true
    }

    fn fill_block_continuous(
        &mut self,
        data: &[i16],
        block: &mut [f32],
        pitch_ratio_fp: i64,
    ) -> bool {
        if self.start_loop < self.start
            || self.end_loop > self.end
            || self.end_loop <= self.start_loop
        {
            self.looping = false;
            return self.fill_block_no_loop(data, block, pitch_ratio_fp);
        }

        let end_loop_fp = (self.end_loop as i64) << Oscillator::FRAC_BITS;
        let loop_length = (self.end_loop - self.start_loop) as i64;
        if loop_length <= 1 {
            self.looping = false;
            return self.fill_block_no_loop(data, block, pitch_ratio_fp);
        }
        let loop_length_fp = loop_length << Oscillator::FRAC_BITS;

        for sample in block.iter_mut() {
            if self.position_fp >= end_loop_fp {
                self.position_fp -= loop_length_fp;
            }

            let index1 = (self.position_fp >> Oscillator::FRAC_BITS) as usize;
            let mut index2 = index1 + 1;
            if index2 >= self.end_loop as usize {
                index2 -= loop_length as usize;
            }

            let (x1, x2) = self.sample_pair_or_panic(
                data,
                index1,
                index2,
                0,
                pitch_ratio_fp,
                "continuous",
            );
            let a_fp = self.position_fp & (Oscillator::FRAC_UNIT - 1);
            *sample = Oscillator::FP_TO_SAMPLE
                * ((x1 << Oscillator::FRAC_BITS) + a_fp * (x2 - x1)) as f32;

            self.position_fp += pitch_ratio_fp;
        }

        true
    }

    fn sample_pair_or_panic(
        &self,
        data: &[i16],
        index1: usize,
        index2: usize,
        block_offset: usize,
        pitch_ratio_fp: i64,
        branch: &str,
    ) -> (i64, i64) {
        let Some(&x1) = data.get(index1) else {
            self.panic_index_bounds(branch, index1, index2, block_offset, pitch_ratio_fp, data.len());
        };
        let Some(&x2) = data.get(index2) else {
            self.panic_index_bounds(branch, index1, index2, block_offset, pitch_ratio_fp, data.len());
        };
        (x1 as i64, x2 as i64)
    }

    fn panic_index_bounds(
        &self,
        branch: &str,
        index1: usize,
        index2: usize,
        block_offset: usize,
        pitch_ratio_fp: i64,
        data_len: usize,
    ) -> ! {
        panic!(
            "oscillator sample index out of bounds: branch={branch} data_len={data_len} index1={index1} index2={index2} start={} end={} start_loop={} end_loop={} position_fp={} position_index={} block_offset={} pitch_ratio_fp={} looping={} sample_sample_rate={} synth_sample_rate={}",
            self.start,
            self.end,
            self.start_loop,
            self.end_loop,
            self.position_fp,
            self.position_fp >> Oscillator::FRAC_BITS,
            block_offset,
            pitch_ratio_fp,
            self.looping,
            self.sample_sample_rate,
            self.synthesizer_sample_rate,
        );
    }
}
