use std::{
    fmt::{Display, Formatter},
    fs,
    path::Path,
};

mod bits;
mod decoder;
mod header;
mod lpc;
mod trace;
use bits::{looks_like_sf2, nbits, nsdiv, quick_abs2, sdiv, BitReader};
use decoder::{inflate_zlib_block, AudioParams, DecoderState};
use header::{consume_text_block, FLAG_LICENSE, FLAG_NOTES};
pub use header::{parse_header, CompressionMethod, Result, SfArkError, SfArkHeader};
use lpc::{LpcState, LPC_HISTORY_SIZE, LPC_MAX_COEFFICIENTS};
use trace::{
    adler32, dump_trace_f32_matrix, dump_trace_f32_words, dump_trace_i32_words, dump_trace_words,
    trace_audio_block,
};

const ZBUF_SIZE: usize = 256 * 1024;
const SHIFT_WINDOW_WORDS: usize = 64;
const OPT_WINDOW_SIZE: usize = 32;
const MAX_DIFF_LOOPS: usize = 20;
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
