use std::{
    fmt::{Display, Formatter},
    fs,
    path::Path,
};

const HEADER_SIGNATURE_OFFSET: usize = 26;
const HEADER_CHECKSUM_OFFSET: usize = 16;
const HEADER_FIXED_SIZE: usize = 298;
const HEADER_PREFIX_SIZE: usize = 42;
const HEADER_MAX_OFFSET: usize = 128 * 1024;
const MAX_FILENAME_SIZE: usize = 256;
const ZBUF_SIZE: usize = 256 * 1024;
const SHIFT_WINDOW_WORDS: usize = 64;
const OPT_WINDOW_SIZE: usize = 32;
const MAX_DIFF_LOOPS: usize = 20;
const LPC_WINDOW_WORDS: usize = 4096;
const LPC_ANALYSIS_WINDOW_WORDS: usize = 128;
const LPC_MAX_COEFFICIENTS: usize = 128;
const LPC_HISTORY_SIZE: usize = 4;
const LPC_SCALE_BITS: u32 = 14;
const LPC_SCALE: f64 = (1 << LPC_SCALE_BITS) as f64;

const FLAG_NOTES: u32 = 1 << 0;
const FLAG_LICENSE: u32 = 1 << 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SfArkError {
    MissingSignature,
    HeaderChecksumMismatch,
    IncompatibleVersion(u8),
    UnsupportedCompressionMethod(u8),
    UnsupportedStage(&'static str),
    CorruptData(String),
    Io(String),
}

impl Display for SfArkError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSignature => write!(formatter, "missing sfArk signature"),
            Self::HeaderChecksumMismatch => write!(formatter, "sfArk header checksum mismatch"),
            Self::IncompatibleVersion(version) => {
                write!(formatter, "sfArk version {version} is not supported")
            }
            Self::UnsupportedCompressionMethod(method) => {
                write!(formatter, "unsupported sfArk compression method {method}")
            }
            Self::UnsupportedStage(stage) => write!(formatter, "unsupported sfArk stage: {stage}"),
            Self::CorruptData(message) => write!(formatter, "corrupt sfArk data: {message}"),
            Self::Io(message) => write!(formatter, "sfArk I/O error: {message}"),
        }
    }
}

impl std::error::Error for SfArkError {}

pub type Result<T> = std::result::Result<T, SfArkError>;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CompressionMethod {
    V2NonAudio,
    V2Turbo,
    V2Fast,
    V2Standard,
    V2Max,
}

impl CompressionMethod {
    fn from_byte(value: u8) -> Result<Self> {
        match value {
            3 => Ok(Self::V2NonAudio),
            4 => Ok(Self::V2Turbo),
            5 => Ok(Self::V2Fast),
            6 => Ok(Self::V2Standard),
            7 => Ok(Self::V2Max),
            0..=2 => Err(SfArkError::IncompatibleVersion(value)),
            other => Err(SfArkError::UnsupportedCompressionMethod(other)),
        }
    }

    fn audio_params(self) -> Result<AudioParams> {
        match self {
            Self::V2Max => Ok(AudioParams {
                read_words: 4096,
                max_loops: 3,
                max_bd4_loops: 5,
                lpc_coefficients: 128,
                window_words: OPT_WINDOW_SIZE,
            }),
            Self::V2Standard => Ok(AudioParams {
                read_words: 4096,
                max_loops: 3,
                max_bd4_loops: 3,
                lpc_coefficients: 8,
                window_words: OPT_WINDOW_SIZE,
            }),
            Self::V2Fast => Ok(AudioParams {
                read_words: 1024,
                max_loops: 20,
                max_bd4_loops: 20,
                lpc_coefficients: 0,
                window_words: OPT_WINDOW_SIZE,
            }),
            Self::V2Turbo => Ok(AudioParams {
                read_words: 4096,
                max_loops: 3,
                max_bd4_loops: 0,
                lpc_coefficients: 0,
                window_words: OPT_WINDOW_SIZE * 8,
            }),
            Self::V2NonAudio => Err(SfArkError::UnsupportedStage(
                "audio section in non-audio sfArk method",
            )),
        }
    }

    fn uses_lpc(self) -> bool {
        matches!(self, Self::V2Max | Self::V2Standard)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfArkHeader {
    pub flags: u32,
    pub original_size: u32,
    pub compressed_size: u32,
    pub file_check: u32,
    pub header_check: u32,
    pub version_needed: u8,
    pub created_version: String,
    pub program_name: String,
    pub compression_method: CompressionMethod,
    pub file_type: u16,
    pub audio_start: u32,
    pub post_audio_start: u32,
    pub original_filename: String,
    pub payload_offset: usize,
}

pub fn decode_sfark_file_to_sf2(input: impl AsRef<Path>, output: impl AsRef<Path>) -> Result<()> {
    let input_bytes =
        fs::read(input.as_ref()).map_err(|error| SfArkError::Io(error.to_string()))?;
    let output_bytes = decode_sfark_to_sf2_bytes(&input_bytes)?;
    fs::write(output.as_ref(), output_bytes).map_err(|error| SfArkError::Io(error.to_string()))
}

pub fn decode_sfark_to_sf2_bytes(input: &[u8]) -> Result<Vec<u8>> {
    decode_sfark_to_sf2_diagnostics(input)?.into_result()
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeDiagnostics {
    pub output: Vec<u8>,
    pub expected_file_check: u32,
    pub actual_file_check: u32,
}

#[doc(hidden)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct DecodeProgress {
    pub audio_block_index: usize,
    pub total_written: u32,
    pub original_size: u32,
    pub section: &'static str,
}

impl DecodeDiagnostics {
    fn into_result(self) -> Result<Vec<u8>> {
        if self.actual_file_check != self.expected_file_check {
            return Err(SfArkError::CorruptData("file checksum mismatch".to_owned()));
        }

        Ok(self.output)
    }
}

#[doc(hidden)]
pub fn decode_sfark_to_sf2_diagnostics(input: &[u8]) -> Result<DecodeDiagnostics> {
    decode_sfark_to_sf2_diagnostics_with_progress(input, |_| {})
}

#[doc(hidden)]
pub fn decode_sfark_to_sf2_diagnostics_with_progress(
    input: &[u8],
    mut progress: impl FnMut(DecodeProgress),
) -> Result<DecodeDiagnostics> {
    decode_sfark_to_sf2_diagnostics_with_limit(input, None, &mut progress)
}

#[doc(hidden)]
pub fn decode_sfark_to_sf2_diagnostics_until_block(
    input: &[u8],
    max_audio_blocks: usize,
    mut progress: impl FnMut(DecodeProgress),
) -> Result<DecodeDiagnostics> {
    decode_sfark_to_sf2_diagnostics_with_limit(input, Some(max_audio_blocks), &mut progress)
}

fn decode_sfark_to_sf2_diagnostics_with_limit(
    input: &[u8],
    max_audio_blocks: Option<usize>,
    progress: &mut impl FnMut(DecodeProgress),
) -> Result<DecodeDiagnostics> {
    let header = parse_header(input)?;
    let mut cursor = header.payload_offset;
    let mut file_check = 0u32;

    if header.flags & FLAG_LICENSE != 0 {
        file_check = consume_text_block(input, &mut cursor, file_check)?;
    }

    if header.flags & FLAG_NOTES != 0 {
        file_check = consume_text_block(input, &mut cursor, file_check)?;
    }

    let mut bit_reader = BitReader::new(&input[cursor..]);
    let mut decoder = DecoderState::new(header, file_check)?;
    decoder.decode_main_stream(&mut bit_reader, progress, max_audio_blocks)
}

pub fn parse_header(input: &[u8]) -> Result<SfArkHeader> {
    let Some(offset) = find_header_offset(input) else {
        return Err(SfArkError::MissingSignature);
    };

    if offset + HEADER_PREFIX_SIZE > input.len() {
        return Err(SfArkError::CorruptData("truncated header".to_owned()));
    }

    let filename_start = offset + HEADER_PREFIX_SIZE;
    let filename_limit = (filename_start + MAX_FILENAME_SIZE).min(input.len());
    let filename_len = input[filename_start..filename_limit]
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| SfArkError::CorruptData("header filename is not terminated".to_owned()))?;
    let header_len = HEADER_PREFIX_SIZE + filename_len + 1;

    if offset + header_len > input.len() || header_len > HEADER_FIXED_SIZE {
        return Err(SfArkError::CorruptData("invalid header length".to_owned()));
    }

    let header_bytes = &input[offset..offset + header_len];
    let header_check = read_u32_le(input, offset + HEADER_CHECKSUM_OFFSET)?;
    let mut checksum_bytes = header_bytes.to_vec();
    checksum_bytes[HEADER_CHECKSUM_OFFSET..HEADER_CHECKSUM_OFFSET + 4].fill(0);
    if adler32(0, &checksum_bytes) != header_check {
        return Err(SfArkError::HeaderChecksumMismatch);
    }

    let method = CompressionMethod::from_byte(input[offset + 31])?;
    let version_needed = input[offset + 20];
    if version_needed > 30 {
        return Err(SfArkError::IncompatibleVersion(version_needed));
    }

    Ok(SfArkHeader {
        flags: read_u32_le(input, offset)?,
        original_size: read_u32_le(input, offset + 4)?,
        compressed_size: read_u32_le(input, offset + 8)?,
        file_check: read_u32_le(input, offset + 12)?,
        header_check,
        version_needed,
        created_version: ascii_field(input, offset + 21, 5),
        program_name: ascii_field(input, offset + 26, 5),
        compression_method: method,
        file_type: read_u16_le(input, offset + 32)?,
        audio_start: read_u32_le(input, offset + 34)?,
        post_audio_start: read_u32_le(input, offset + 38)?,
        original_filename: String::from_utf8_lossy(
            &input[filename_start..filename_start + filename_len],
        )
        .to_string(),
        payload_offset: offset + header_len,
    })
}

fn find_header_offset(input: &[u8]) -> Option<usize> {
    let max_start = input
        .len()
        .saturating_sub(HEADER_SIGNATURE_OFFSET + "sfArk".len());
    let search_limit = max_start.min(HEADER_MAX_OFFSET);

    if has_signature_at(input, 0) {
        return Some(0);
    }

    (0..=search_limit)
        .rev()
        .find(|offset| has_signature_at(input, *offset))
}

fn has_signature_at(input: &[u8], offset: usize) -> bool {
    input.get(offset + HEADER_SIGNATURE_OFFSET..offset + HEADER_SIGNATURE_OFFSET + 5)
        == Some(b"sfArk")
}

fn consume_text_block(input: &[u8], cursor: &mut usize, file_check: u32) -> Result<u32> {
    let compressed_len = read_u32_le(input, *cursor)? as usize;
    *cursor += 4;

    let compressed = input
        .get(*cursor..*cursor + compressed_len)
        .ok_or_else(|| SfArkError::CorruptData("truncated compressed text block".to_owned()))?;
    *cursor += compressed_len;

    let decompressed = inflate_zlib_block(compressed)?;
    if decompressed.len() > ZBUF_SIZE {
        return Err(SfArkError::CorruptData(
            "decompressed text block is too large".to_owned(),
        ));
    }

    Ok(adler32(file_check, &decompressed))
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct AudioParams {
    read_words: usize,
    max_loops: i16,
    max_bd4_loops: i16,
    lpc_coefficients: usize,
    window_words: usize,
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

struct DecoderState {
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
    fn new(header: SfArkHeader, file_check: u32) -> Result<Self> {
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

    fn decode_main_stream(
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

struct LpcState {
    history_prefix: [i32; LPC_MAX_COEFFICIENTS * 2],
    autocorrelation_history: [[f32; LPC_MAX_COEFFICIENTS + 1]; LPC_HISTORY_SIZE],
    history_index: usize,
    synthesis_state: [i32; LPC_MAX_COEFFICIENTS + 1],
}

impl LpcState {
    fn new() -> Self {
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

    fn decode(
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

    fn decode_window(
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

fn lpc_product_shift(left: i32, right: i32) -> i32 {
    sdiv(left.wrapping_mul(right), LPC_SCALE_BITS)
}

fn schur_reflection(autocorrelation: &[f32], coefficients: usize) -> Vec<i32> {
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

fn autocorrelation(input: &[i32], count: usize) -> [f32; LPC_MAX_COEFFICIENTS + 1] {
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

fn add_autocorrelation_overlap(
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

struct BitReader<'a> {
    input: &'a [u8],
    word_offset: usize,
    bits: u32,
    remaining_bits: usize,
    previous_fix_bits: i16,
}

impl<'a> BitReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            word_offset: 0,
            bits: 0,
            remaining_bits: 0,
            previous_fix_bits: 8,
        }
    }

    fn read_flag(&mut self) -> Result<bool> {
        Ok(self.read_bits(1)? != 0)
    }

    fn read_u32_le(&mut self) -> Result<u32> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            bytes.push(self.read_bits(8)? as u8);
        }
        Ok(bytes)
    }

    fn read_bits(&mut self, count: usize) -> Result<u16> {
        if count == 0 {
            return Ok(0);
        }
        if count > 16 {
            return Err(SfArkError::CorruptData(format!(
                "cannot read {count} bits at once"
            )));
        }
        if self.remaining_bits < 16 {
            let next = self.next_word()?;
            self.bits = (self.bits << 16) | u32::from(next);
            self.remaining_bits += 16;
        }

        if self.remaining_bits < count {
            return Err(SfArkError::CorruptData("bitstream underrun".to_owned()));
        }

        self.remaining_bits -= count;
        let value = (self.bits >> self.remaining_bits) & low_mask(count);
        self.bits &= low_mask(self.remaining_bits);
        Ok(value as u16)
    }

    fn input_diff(&mut self, previous: i16) -> Result<i16> {
        let magnitude = self.read_group_count()? as i32;
        let signed = if magnitude == 0 {
            0
        } else if self.read_flag()? {
            -magnitude
        } else {
            magnitude
        };
        Ok(previous.wrapping_add(signed as i16))
    }

    fn uncrunch_window(&mut self, word_len: usize, window_words: usize) -> Result<Vec<i16>> {
        let mut output = vec![0; word_len];
        let mut start = 0;
        while start < word_len {
            let end = (start + window_words).min(word_len);
            self.uncrunch(&mut output[start..end])?;
            start = end;
        }
        Ok(output)
    }

    fn uncrunch(&mut self, output: &mut [i16]) -> Result<()> {
        let fix_bits = self.input_diff(self.previous_fix_bits)?;
        self.previous_fix_bits = fix_bits;

        match fix_bits {
            0..=13 => {
                let low_bit_count = fix_bits as usize + 1;
                for word in output {
                    let low_bits = self.read_bits(low_bit_count)? as i32;
                    let group = self.read_group_count()? as i32;
                    let sign_mask = -((low_bits & 1) as i32);
                    *word = (((group << fix_bits) | (low_bits >> 1)) ^ sign_mask) as i16;
                }
            }
            14 => {
                for word in output {
                    *word = self.read_bits(16)? as i16;
                }
            }
            -1 => {
                for word in output {
                    *word = if self.read_flag()? { -1 } else { 0 };
                }
            }
            -2 => output.fill(0),
            other => {
                return Err(SfArkError::CorruptData(format!(
                    "invalid fix-bits value {other}"
                )));
            }
        }

        Ok(())
    }

    fn read_group_count(&mut self) -> Result<u16> {
        let mut count = 0u16;
        while !self.read_flag()? {
            count = count
                .checked_add(1)
                .ok_or_else(|| SfArkError::CorruptData("group count overflow".to_owned()))?;
        }
        Ok(count)
    }

    fn next_word(&mut self) -> Result<u16> {
        let byte_offset = self.word_offset * 2;
        let bytes = self
            .input
            .get(byte_offset..byte_offset + 2)
            .ok_or_else(|| SfArkError::CorruptData("bitstream ended early".to_owned()))?;
        self.word_offset += 1;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
}

fn inflate_zlib_block(compressed: &[u8]) -> Result<Vec<u8>> {
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

fn quick_abs2(value: i16) -> u32 {
    (value ^ (value >> 15)) as u16 as u32
}

fn nsdiv(value: i32, shift: u32) -> i32 {
    value >> shift
}

fn sdiv(value: i32, shift: u32) -> i32 {
    if value >= 0 {
        value >> shift
    } else {
        -((-value) >> shift)
    }
}

fn nbits(value: usize) -> usize {
    if value == 0 {
        0
    } else {
        usize::BITS as usize - value.leading_zeros() as usize
    }
}

fn low_mask(bits: usize) -> u32 {
    if bits == 0 {
        0
    } else if bits >= 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    }
}

fn read_u16_le(input: &[u8], offset: usize) -> Result<u16> {
    let bytes = input
        .get(offset..offset + 2)
        .ok_or_else(|| SfArkError::CorruptData("truncated u16 field".to_owned()))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32_le(input: &[u8], offset: usize) -> Result<u32> {
    let bytes = input
        .get(offset..offset + 4)
        .ok_or_else(|| SfArkError::CorruptData("truncated u32 field".to_owned()))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn ascii_field(input: &[u8], offset: usize, len: usize) -> String {
    String::from_utf8_lossy(input.get(offset..offset + len).unwrap_or_default())
        .trim_end_matches('\0')
        .trim()
        .to_owned()
}

fn looks_like_sf2(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"sfbk"
}

fn trace_audio_block(block_index: usize) -> bool {
    std::env::var("FLUTZ_SFARK_TRACE_BLOCK")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index)
}

fn dump_trace_words(block_env: &str, path_env: &str, block_index: usize, words: &[i16]) {
    let should_dump = std::env::var(block_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index);
    if !should_dump {
        return;
    }

    let Some(path) = std::env::var_os(path_env) else {
        return;
    };

    let mut bytes = Vec::with_capacity(words.len() * 2);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }

    if let Err(error) = fs::write(path, bytes) {
        eprintln!("sfArk trace dump failed: {error}");
    }
}

fn dump_trace_i32_words(
    block_env: &str,
    subwindow_env: &str,
    path_env: &str,
    block_index: usize,
    subwindow: usize,
    words: &[i32],
) {
    let should_dump_block = std::env::var(block_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index);
    let should_dump_subwindow = std::env::var(subwindow_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(subwindow);
    if !should_dump_block || !should_dump_subwindow {
        return;
    }

    let Some(path) = std::env::var_os(path_env) else {
        return;
    };

    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }

    if let Err(error) = fs::write(path, bytes) {
        eprintln!("sfArk trace dump failed: {error}");
    }
}

fn dump_trace_f32_words(
    block_env: &str,
    subwindow_env: &str,
    path_env: &str,
    block_index: usize,
    subwindow: usize,
    words: &[f32],
) {
    let should_dump_block = std::env::var(block_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index);
    let should_dump_subwindow = std::env::var(subwindow_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(subwindow);
    if !should_dump_block || !should_dump_subwindow {
        return;
    }

    let Some(path) = std::env::var_os(path_env) else {
        return;
    };

    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }

    if let Err(error) = fs::write(path, bytes) {
        eprintln!("sfArk trace dump failed: {error}");
    }
}

fn dump_trace_f32_matrix(
    block_env: &str,
    subwindow_env: &str,
    path_env: &str,
    block_index: usize,
    subwindow: usize,
    rows: &[[f32; LPC_MAX_COEFFICIENTS + 1]; LPC_HISTORY_SIZE],
    columns: usize,
) {
    let should_dump_block = std::env::var(block_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(block_index);
    let should_dump_subwindow = std::env::var(subwindow_env)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        == Some(subwindow);
    if !should_dump_block || !should_dump_subwindow {
        return;
    }

    let Some(path) = std::env::var_os(path_env) else {
        return;
    };

    let mut bytes = Vec::with_capacity(rows.len() * columns * 4);
    for row in rows {
        for value in row.iter().take(columns) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }

    if let Err(error) = fs::write(path, bytes) {
        eprintln!("sfArk trace dump failed: {error}");
    }
}

fn adler32(initial: u32, bytes: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let mut s1 = initial & 0xffff;
    let mut s2 = (initial >> 16) & 0xffff;

    for byte in bytes {
        s1 = (s1 + u32::from(*byte)) % MOD_ADLER;
        s2 = (s2 + s1) % MOD_ADLER;
    }

    (s2 << 16) | s1
}
