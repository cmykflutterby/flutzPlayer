use super::*;

const LPC_WINDOW_WORDS: usize = 4096;
const LPC_ANALYSIS_WINDOW_WORDS: usize = 128;
pub(super) const LPC_MAX_COEFFICIENTS: usize = 128;
pub(super) const LPC_HISTORY_SIZE: usize = 4;
const LPC_SCALE_BITS: u32 = 14;
const LPC_SCALE: f64 = (1 << LPC_SCALE_BITS) as f64;

pub(super) struct LpcState {
    history_prefix: [i32; LPC_MAX_COEFFICIENTS * 2],
    autocorrelation_history: [[f32; LPC_MAX_COEFFICIENTS + 1]; LPC_HISTORY_SIZE],
    history_index: usize,
    synthesis_state: [i32; LPC_MAX_COEFFICIENTS + 1],
}

impl LpcState {
    pub(super) fn new() -> Self {
        Self {
            history_prefix: [0; LPC_MAX_COEFFICIENTS * 2],
            autocorrelation_history: [[0.0; LPC_MAX_COEFFICIENTS + 1]; LPC_HISTORY_SIZE],
            history_index: 0,
            synthesis_state: [0; LPC_MAX_COEFFICIENTS + 1],
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    pub(super) fn decode(
        &mut self,
        input: &[i16],
        coefficients: usize,
        flags: u32,
        audio_block_index: usize,
    ) -> Result<Vec<i16>> {
        if coefficients == 0 || coefficients > LPC_MAX_COEFFICIENTS {
            return Err(SfArkError::CorruptData(format!(
                "invalid LPC coefficient count {coefficients}"
            )));
        }

        let wide_input: Vec<i32> = input.iter().map(|word| i32::from(*word)).collect();
        let mut wide_output = vec![0; input.len()];
        let mut position = 0;

        while position < input.len() {
            let remaining = input.len() - position;
            let window_len = remaining.min(LPC_WINDOW_WORDS);
            if window_len < LPC_ANALYSIS_WINDOW_WORDS {
                wide_output[position..position + window_len]
                    .copy_from_slice(&wide_input[position..position + window_len]);
            } else {
                self.decode_window(
                    &wide_input[position..position + window_len],
                    &mut wide_output[position..position + window_len],
                    coefficients,
                    flags,
                    audio_block_index,
                );
            }
            position += window_len;
        }

        Ok(wide_output.into_iter().map(|word| word as i16).collect())
    }

    pub(super) fn decode_window(
        &mut self,
        input: &[i32],
        output: &mut [i32],
        coefficients: usize,
        flags: u32,
        audio_block_index: usize,
    ) {
        let mut flag_mask = 1u32;
        let mut position = 0;

        while position < input.len() {
            let window_len = (input.len() - position).min(LPC_ANALYSIS_WINDOW_WORDS);
            if window_len < coefficients {
                output[position..position + window_len]
                    .copy_from_slice(&input[position..position + window_len]);
                position += window_len;
                continue;
            }

            let mut combined_ac = [0.0f32; LPC_MAX_COEFFICIENTS + 1];
            let subwindow = position / LPC_ANALYSIS_WINDOW_WORDS;
            dump_trace_f32_matrix(
                "FLUTZ_SFARK_DUMP_LPC_HIST_BLOCK",
                "FLUTZ_SFARK_DUMP_LPC_HIST_SUBWINDOW",
                "FLUTZ_SFARK_DUMP_LPC_HIST_PATH",
                audio_block_index,
                subwindow,
                &self.autocorrelation_history,
                coefficients + 1,
            );
            for (index, target) in combined_ac.iter_mut().enumerate().take(coefficients + 1) {
                *target = (f64::from(self.autocorrelation_history[0][index])
                    + f64::from(self.autocorrelation_history[1][index])
                    + f64::from(self.autocorrelation_history[2][index])
                    + f64::from(self.autocorrelation_history[3][index]))
                    as f32;
            }

            if flags & flag_mask == 0 {
                dump_trace_f32_words(
                    "FLUTZ_SFARK_DUMP_LPC_AC_BLOCK",
                    "FLUTZ_SFARK_DUMP_LPC_AC_SUBWINDOW",
                    "FLUTZ_SFARK_DUMP_LPC_AC_PATH",
                    audio_block_index,
                    subwindow,
                    &combined_ac[..coefficients + 1],
                );
                let reflection = schur_reflection(&combined_ac, coefficients);
                dump_trace_i32_words(
                    "FLUTZ_SFARK_DUMP_LPC_REF_BLOCK",
                    "FLUTZ_SFARK_DUMP_LPC_REF_SUBWINDOW",
                    "FLUTZ_SFARK_DUMP_LPC_REF_PATH",
                    audio_block_index,
                    subwindow,
                    &reflection,
                );
                self.lpc_synthesize(
                    &reflection,
                    &input[position..position + window_len],
                    &mut output[position..position + window_len],
                    coefficients,
                );
            } else {
                self.reset();
                output[position..position + window_len]
                    .copy_from_slice(&input[position..position + window_len]);
            }
            flag_mask <<= 1;

            add_autocorrelation_overlap(
                &self.history_prefix,
                &output[position..position + window_len],
                coefficients + 1,
                &mut self.autocorrelation_history[self.history_index],
            );

            self.history_index = (self.history_index + 1) % LPC_HISTORY_SIZE;
            self.autocorrelation_history[self.history_index] =
                autocorrelation(&output[position..position + window_len], coefficients + 1);

            self.history_prefix[..coefficients]
                .copy_from_slice(&output[position..position + coefficients]);
            position += window_len;
        }
    }

    fn lpc_synthesize(
        &mut self,
        reflection: &[i32],
        input: &[i32],
        output: &mut [i32],
        coefficients: usize,
    ) {
        for (residual, out) in input.iter().zip(output.iter_mut()) {
            let mut sample = *residual;
            for index in (0..coefficients).rev() {
                sample = sample.wrapping_sub(lpc_product_shift(
                    reflection[index],
                    self.synthesis_state[index],
                ));
                self.synthesis_state[index + 1] = self.synthesis_state[index]
                    .wrapping_add(lpc_product_shift(reflection[index], sample));
            }
            self.synthesis_state[0] = sample;
            *out = sample;
        }
    }
}

pub(super) fn lpc_product_shift(left: i32, right: i32) -> i32 {
    sdiv(left.wrapping_mul(right), LPC_SCALE_BITS)
}

pub(super) fn schur_reflection(autocorrelation: &[f32], coefficients: usize) -> Vec<i32> {
    let mut reflection = vec![0; coefficients];
    if autocorrelation[0] == 0.0 {
        return reflection;
    }

    let mut error = autocorrelation[0] as f64;
    let mut generator_0 = vec![0.0f64; coefficients];
    let mut generator_1 = vec![0.0f64; coefficients];
    for index in 0..coefficients {
        generator_0[index] = autocorrelation[index + 1] as f64;
        generator_1[index] = autocorrelation[index + 1] as f64;
    }

    for index in 0..coefficients {
        if error == 0.0 {
            break;
        }
        let r = (-(generator_1[0] / error)) as f32 as f64;
        error = (error + generator_1[0] * r) as f32 as f64;
        reflection[index] = ((r * LPC_SCALE) as f32) as i32;

        if index + 1 >= coefficients {
            break;
        }

        let old_generator_0 = generator_0.clone();
        let old_generator_1 = generator_1.clone();
        for item in 0..coefficients - index - 1 {
            generator_1[item] =
                (old_generator_1[item + 1] + r * old_generator_0[item]) as f32 as f64;
            generator_0[item] =
                (old_generator_0[item] + r * old_generator_1[item + 1]) as f32 as f64;
        }
    }

    reflection
}

pub(super) fn autocorrelation(input: &[i32], count: usize) -> [f32; LPC_MAX_COEFFICIENTS + 1] {
    let mut output = [0.0f32; LPC_MAX_COEFFICIENTS + 1];
    for lag in 0..count {
        let mut correlation = 0.0f64;
        let stop = input.len().saturating_sub(lag);
        let mut index = 0;

        while index + 15 < stop {
            let group = f64::from(input[index] as f32) * f64::from(input[index + lag] as f32)
                + f64::from(input[index + 1] as f32) * f64::from(input[index + lag + 1] as f32)
                + f64::from(input[index + 2] as f32) * f64::from(input[index + lag + 2] as f32)
                + f64::from(input[index + 3] as f32) * f64::from(input[index + lag + 3] as f32)
                + f64::from(input[index + 4] as f32) * f64::from(input[index + lag + 4] as f32)
                + f64::from(input[index + 5] as f32) * f64::from(input[index + lag + 5] as f32)
                + f64::from(input[index + 6] as f32) * f64::from(input[index + lag + 6] as f32)
                + f64::from(input[index + 7] as f32) * f64::from(input[index + lag + 7] as f32)
                + f64::from(input[index + 8] as f32) * f64::from(input[index + lag + 8] as f32)
                + f64::from(input[index + 9] as f32) * f64::from(input[index + lag + 9] as f32)
                + f64::from(input[index + 10] as f32) * f64::from(input[index + lag + 10] as f32)
                + f64::from(input[index + 11] as f32) * f64::from(input[index + lag + 11] as f32)
                + f64::from(input[index + 12] as f32) * f64::from(input[index + lag + 12] as f32)
                + f64::from(input[index + 13] as f32) * f64::from(input[index + lag + 13] as f32)
                + f64::from(input[index + 14] as f32) * f64::from(input[index + lag + 14] as f32)
                + f64::from(input[index + 15] as f32) * f64::from(input[index + lag + 15] as f32);
            correlation = (correlation + group) as f32 as f64;
            index += 16;
        }

        while index < stop {
            correlation = (correlation
                + f64::from(input[index] as f32) * f64::from(input[index + lag] as f32))
                as f32 as f64;
            index += 1;
        }
        output[lag] = correlation as f32;
    }
    output
}

pub(super) fn add_autocorrelation_overlap(
    history_prefix: &[i32],
    input: &[i32],
    count: usize,
    output: &mut [f32; LPC_MAX_COEFFICIENTS + 1],
) {
    let n = count - 1;
    let mut buffer = [0.0f32; LPC_MAX_COEFFICIENTS * 2];
    for index in 0..n {
        buffer[index] = history_prefix[index] as f32;
        buffer[index + n] = input[index] as f32;
    }

    for lag in 1..=n {
        let mut correlation = 0.0f64;
        let mut index = n - lag;

        while index + 15 < n {
            let group = f64::from(buffer[index]) * f64::from(buffer[index + lag])
                + f64::from(buffer[index + 1]) * f64::from(buffer[index + lag + 1])
                + f64::from(buffer[index + 2]) * f64::from(buffer[index + lag + 2])
                + f64::from(buffer[index + 3]) * f64::from(buffer[index + lag + 3])
                + f64::from(buffer[index + 4]) * f64::from(buffer[index + lag + 4])
                + f64::from(buffer[index + 5]) * f64::from(buffer[index + lag + 5])
                + f64::from(buffer[index + 6]) * f64::from(buffer[index + lag + 6])
                + f64::from(buffer[index + 7]) * f64::from(buffer[index + lag + 7])
                + f64::from(buffer[index + 8]) * f64::from(buffer[index + lag + 8])
                + f64::from(buffer[index + 9]) * f64::from(buffer[index + lag + 9])
                + f64::from(buffer[index + 10]) * f64::from(buffer[index + lag + 10])
                + f64::from(buffer[index + 11]) * f64::from(buffer[index + lag + 11])
                + f64::from(buffer[index + 12]) * f64::from(buffer[index + lag + 12])
                + f64::from(buffer[index + 13]) * f64::from(buffer[index + lag + 13])
                + f64::from(buffer[index + 14]) * f64::from(buffer[index + lag + 14])
                + f64::from(buffer[index + 15]) * f64::from(buffer[index + lag + 15]);
            correlation = (correlation + group) as f32 as f64;
            index += 16;
        }

        while index < n {
            correlation = (correlation + f64::from(buffer[index]) * f64::from(buffer[index + lag]))
                as f32 as f64;
            index += 1;
        }

        output[lag] = (f64::from(output[lag]) + correlation) as f32;
    }
}
