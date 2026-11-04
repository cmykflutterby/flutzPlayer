use super::*;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(super) struct AudioParams {
    pub(super) read_words: usize,
    pub(super) max_loops: i16,
    pub(super) max_bd4_loops: i16,
    pub(super) lpc_coefficients: usize,
    pub(super) window_words: usize,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum FileSection {
    PreAudio,
    Audio,
    PostAudio,
    Finished,
}

impl FileSection {
    fn name(self) -> &'static str {
        match self {
            Self::PreAudio => "pre-audio",
            Self::Audio => "audio",
            Self::PostAudio => "post-audio",
            Self::Finished => "finished",
        }
    }
}

pub(super) struct DecoderState {
    header: SfArkHeader,
    section: FileSection,
    params: AudioParams,
    total_written: u32,
    file_check: u32,
    prev_in: [i16; MAX_DIFF_LOOPS],
    prev_encode_count: i16,
    bd4_prev_encode_count: i16,
    prev_shift: i16,
    prev_used_shift: i16,
    audio_block_index: usize,
    lpc: LpcState,
}
impl DecoderState {
    pub(super) fn new(header: SfArkHeader, file_check: u32) -> Result<Self> {
        let params = header.compression_method.audio_params()?;
        Ok(Self {
            header,
            section: FileSection::PreAudio,
            params,
            total_written: 0,
            file_check,
            prev_in: [0; MAX_DIFF_LOOPS],
            prev_encode_count: 0,
            bd4_prev_encode_count: 0,
            prev_shift: 0,
            prev_used_shift: 0,
            audio_block_index: 0,
            lpc: LpcState::new(),
        })
    }

    pub(super) fn decode_main_stream(
        &mut self,
        bit_reader: &mut BitReader<'_>,
        progress: &mut impl FnMut(DecodeProgress),
        max_audio_blocks: Option<usize>,
    ) -> Result<DecodeDiagnostics> {
        let mut output = Vec::with_capacity(self.header.original_size as usize);

        while self.section != FileSection::Finished {
            match self.section {
                FileSection::PreAudio | FileSection::PostAudio => {
                    self.decode_zlib_section_block(bit_reader, &mut output)?;
                    self.report_progress(progress);
                }
                FileSection::Audio => {
                    self.decode_audio_block(bit_reader, &mut output)?;
                    self.report_progress(progress);
                    if max_audio_blocks == Some(self.audio_block_index) {
                        break;
                    }
                }
                FileSection::Finished => {}
            }
        }

        if max_audio_blocks.is_none() && self.total_written != self.header.original_size {
            return Err(SfArkError::CorruptData(format!(
                "decoded {} bytes, expected {}",
                self.total_written, self.header.original_size
            )));
        }

        if !looks_like_sf2(&output) {
            return Err(SfArkError::CorruptData(
                "decoded bytes are not a RIFF sfbk SoundFont".to_owned(),
            ));
        }

        Ok(DecodeDiagnostics {
            output,
            expected_file_check: self.header.file_check,
            actual_file_check: self.file_check,
        })
    }

    fn report_progress(&self, progress: &mut impl FnMut(DecodeProgress)) {
        progress(DecodeProgress {
            audio_block_index: self.audio_block_index,
            total_written: self.total_written,
            original_size: self.header.original_size,
            section: self.section.name(),
        });
    }

    fn decode_zlib_section_block(
        &mut self,
        bit_reader: &mut BitReader<'_>,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        let compressed_len = bit_reader.read_u32_le()? as usize;
        if compressed_len > ZBUF_SIZE {
            return Err(SfArkError::CorruptData(format!(
                "zlib block length {compressed_len} exceeds {ZBUF_SIZE}"
            )));
        }

        let compressed = bit_reader.read_bytes(compressed_len)?;
        let decompressed = inflate_zlib_block(&compressed)?;
        if decompressed.len() > ZBUF_SIZE {
            return Err(SfArkError::CorruptData(
                "decompressed zlib block is too large".to_owned(),
            ));
        }

        self.file_check = adler32(self.file_check, &decompressed);
        self.write_output(output, &decompressed)?;

        if self.total_written >= self.header.original_size {
            self.section = FileSection::Finished;
        } else if self.section == FileSection::PreAudio
            && self.total_written >= self.header.audio_start
        {
            self.section = FileSection::Audio;
        }

        Ok(())
    }

    fn decode_audio_block(
        &mut self,
        bit_reader: &mut BitReader<'_>,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        let mut byte_len = self.params.read_words * 2;
        if self.total_written + byte_len as u32 >= self.header.post_audio_start {
            byte_len = (self.header.post_audio_start - self.total_written) as usize;
            self.section = FileSection::PostAudio;
        }

        let word_len = byte_len / 2;
        let decoded_words = match self.header.compression_method {
            CompressionMethod::V2Turbo => self.decode_turbo_audio(bit_reader, word_len)?,
            CompressionMethod::V2Fast
            | CompressionMethod::V2Standard
            | CompressionMethod::V2Max => self.decode_fast_audio(bit_reader, word_len)?,
            _ => {
                return Err(SfArkError::UnsupportedStage(
                    "audio compression method without implemented audio path",
                ));
            }
        };

        self.audio_block_index += 1;

        let mut audio_bytes = Vec::with_capacity(byte_len);
        for word in decoded_words {
            audio_bytes.extend_from_slice(&word.to_le_bytes());
        }
        audio_bytes.truncate(byte_len);
        self.write_output(output, &audio_bytes)
    }

    fn decode_turbo_audio(
        &mut self,
        bit_reader: &mut BitReader<'_>,
        word_len: usize,
    ) -> Result<Vec<i16>> {
        let encode_count = bit_reader.input_diff(self.prev_encode_count)?;
        validate_encode_count(encode_count, self.params.max_loops)?;
        self.prev_encode_count = encode_count;

        let mut current = bit_reader.uncrunch_window(word_len, self.params.window_words)?;
        let mut scratch = vec![0; word_len];

        for index in (0..encode_count as usize).rev() {
            if index == 0 {
                self.file_check = self
                    .file_check
                    .wrapping_shl(1)
                    .wrapping_add(buf_sum(&current));
            }
            unbuf_dif2(&mut scratch, &current, &mut self.prev_in[index]);
            std::mem::swap(&mut current, &mut scratch);
        }

        Ok(current)
    }

    fn decode_fast_audio(
        &mut self,
        bit_reader: &mut BitReader<'_>,
        word_len: usize,
    ) -> Result<Vec<i16>> {
        let shift_values = self.read_shift_values(bit_reader, word_len)?;
        let using_bd4 = bit_reader.read_flag()?;
        let mut methods = Vec::new();
        let encode_count = if using_bd4 {
            let encode_count = bit_reader.input_diff(self.bd4_prev_encode_count)?;
            validate_encode_count(encode_count, self.params.max_bd4_loops)?;
            self.bd4_prev_encode_count = encode_count;
            encode_count
        } else {
            let encode_count = bit_reader.input_diff(self.prev_encode_count)?;
            validate_encode_count(encode_count, self.params.max_loops)?;
            self.prev_encode_count = encode_count;
            for _ in 0..encode_count {
                methods.push(bit_reader.read_flag()?);
            }
            encode_count
        };

        let lpc_flags = if self.header.compression_method.uses_lpc() {
            if bit_reader.read_flag()? {
                u32::from(bit_reader.read_bits(16)?) | (u32::from(bit_reader.read_bits(16)?) << 16)
            } else {
                0
            }
        } else {
            0
        };

        if trace_audio_block(self.audio_block_index) {
            eprintln!(
                "sfArk trace block={} total_written={} word_len={} shift={} bd4={} encode_count={} methods={:?} lpc_flags={:08x}",
                self.audio_block_index,
                self.total_written,
                word_len,
                shift_values.is_some(),
                using_bd4,
                encode_count,
                methods,
                lpc_flags
            );
        }

        let mut current = bit_reader.uncrunch_window(word_len, self.params.window_words)?;
        let mut scratch = vec![0; word_len];

        dump_trace_words(
            "FLUTZ_SFARK_DUMP_PRE_LPC_BLOCK",
            "FLUTZ_SFARK_DUMP_PRE_LPC_PATH",
            self.audio_block_index,
            &current,
        );

        if self.header.compression_method.uses_lpc() {
            current = self.lpc.decode(
                &current,
                self.params.lpc_coefficients,
                lpc_flags,
                self.audio_block_index,
            )?;
        }

        if using_bd4 {
            for index in (0..encode_count as usize).rev() {
                unbuf_dif4(&mut scratch, &current, &mut self.prev_in[index]);
                std::mem::swap(&mut current, &mut scratch);
            }
        } else {
            for index in (0..encode_count as usize).rev() {
                if methods[index] {
                    unbuf_dif3(&mut scratch, &current, &mut self.prev_in[index]);
                } else {
                    unbuf_dif2(&mut scratch, &current, &mut self.prev_in[index]);
                }
                std::mem::swap(&mut current, &mut scratch);
            }
        }

        if let Some(shifts) = shift_values {
            apply_shifts(&mut current, &shifts);
        }

        self.file_check = self
            .file_check
            .wrapping_mul(2)
            .wrapping_add(buf_sum(&current));
        Ok(current)
    }

    fn read_shift_values(
        &mut self,
        bit_reader: &mut BitReader<'_>,
        word_len: usize,
    ) -> Result<Option<Vec<i16>>> {
        if !bit_reader.read_flag()? {
            return Ok(None);
        }

        let shift_count = word_len.div_ceil(SHIFT_WINDOW_WORDS);
        let mut shifts = vec![0; shift_count];
        let mut change_pos = 0usize;
        let mut fill_pos = 0usize;

        let mut changes_seen = 0usize;
        while bit_reader.read_flag()? {
            changes_seen += 1;
            if changes_seen > shift_count {
                return Err(SfArkError::CorruptData(
                    "too many shift change positions".to_owned(),
                ));
            }

            let remaining = shift_count.saturating_sub(change_pos + 1);
            let bit_count = nbits(remaining);
            change_pos += bit_reader.read_bits(bit_count)? as usize;
            if change_pos > shift_count {
                return Err(SfArkError::CorruptData(
                    "invalid shift change position".to_owned(),
                ));
            }

            for value in shifts.iter_mut().take(change_pos).skip(fill_pos) {
                *value = self.prev_shift;
            }
            fill_pos = change_pos;

            let new_shift = if self.prev_shift == 0 {
                let value = bit_reader.input_diff(self.prev_used_shift)?;
                self.prev_used_shift = value;
                value
            } else {
                bit_reader.input_diff(0)?
            };
            self.prev_shift = new_shift;
        }

        for value in shifts.iter_mut().skip(fill_pos) {
            *value = self.prev_shift;
        }

        Ok(Some(shifts))
    }

    fn write_output(&mut self, output: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
        self.total_written = self
            .total_written
            .checked_add(bytes.len() as u32)
            .ok_or_else(|| SfArkError::CorruptData("decoded output is too large".to_owned()))?;
        if self.total_written > self.header.original_size {
            return Err(SfArkError::CorruptData(
                "decoded more bytes than the header declares".to_owned(),
            ));
        }
        output.extend_from_slice(bytes);
        Ok(())
    }
}

pub(super) fn inflate_zlib_block(compressed: &[u8]) -> Result<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib(compressed)
        .map_err(|status| SfArkError::CorruptData(format!("zlib inflate failed: {status:?}")))
}

fn validate_encode_count(value: i16, max: i16) -> Result<()> {
    if (0..=max).contains(&value) {
        Ok(())
    } else {
        Err(SfArkError::CorruptData(format!(
            "invalid encode count {value}; maximum is {max}"
        )))
    }
}

fn unbuf_dif2(output: &mut [i16], input: &[i16], previous: &mut i16) {
    output.copy_from_slice(input);
    if output.is_empty() {
        return;
    }

    output[0] = output[0].wrapping_add(*previous);
    for index in 1..output.len() {
        output[index] = output[index].wrapping_add(output[index - 1]);
    }
    *previous = *output.last().unwrap_or(previous);
}

fn unbuf_dif3(output: &mut [i16], input: &[i16], previous: &mut i16) {
    if input.is_empty() {
        return;
    }

    let last = input.len() - 1;
    output[last] = input[last];
    for index in (1..last).rev() {
        let estimate = nsdiv(
            i32::from(input[index - 1]) + i32::from(output[index + 1]),
            1,
        );
        output[index] = input[index].wrapping_add(estimate as i16);
    }

    if last > 0 {
        output[0] = input[0].wrapping_add(nsdiv(i32::from(output[1]), 1) as i16);
    }
    *previous = output[last];
}

fn unbuf_dif4(output: &mut [i16], input: &[i16], previous: &mut i16) {
    let mut average = *previous;
    for (out, value) in output.iter_mut().zip(input) {
        *out = value.wrapping_add(average);
        average = average.wrapping_add(sdiv(i32::from(*value), 1) as i16);
    }
    *previous = average;
}

fn apply_shifts(words: &mut [i16], shifts: &[i16]) {
    for (window_index, shift) in shifts.iter().enumerate() {
        if *shift == 0 {
            continue;
        }

        let start = window_index * SHIFT_WINDOW_WORDS;
        let end = (start + SHIFT_WINDOW_WORDS).min(words.len());
        for word in &mut words[start..end] {
            *word = word.wrapping_shl((*shift).max(0) as u32);
        }
    }
}

fn buf_sum(words: &[i16]) -> u32 {
    words
        .iter()
        .fold(0u32, |total, word| total.wrapping_add(quick_abs2(*word)))
}
