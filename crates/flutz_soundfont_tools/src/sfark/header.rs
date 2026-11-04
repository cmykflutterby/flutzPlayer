use super::*;

const HEADER_SIGNATURE_OFFSET: usize = 26;
const HEADER_CHECKSUM_OFFSET: usize = 16;
const HEADER_FIXED_SIZE: usize = 298;
const HEADER_PREFIX_SIZE: usize = 42;
const HEADER_MAX_OFFSET: usize = 128 * 1024;
const MAX_FILENAME_SIZE: usize = 256;

pub(super) const FLAG_NOTES: u32 = 1 << 0;
pub(super) const FLAG_LICENSE: u32 = 1 << 1;

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
    pub(super) fn from_byte(value: u8) -> Result<Self> {
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

    pub(super) fn audio_params(self) -> Result<AudioParams> {
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

    pub(super) fn uses_lpc(self) -> bool {
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

pub(super) fn find_header_offset(input: &[u8]) -> Option<usize> {
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

pub(super) fn has_signature_at(input: &[u8], offset: usize) -> bool {
    input.get(offset + HEADER_SIGNATURE_OFFSET..offset + HEADER_SIGNATURE_OFFSET + 5)
        == Some(b"sfArk")
}

pub(super) fn consume_text_block(input: &[u8], cursor: &mut usize, file_check: u32) -> Result<u32> {
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

pub(super) fn read_u16_le(input: &[u8], offset: usize) -> Result<u16> {
    let bytes = input
        .get(offset..offset + 2)
        .ok_or_else(|| SfArkError::CorruptData("truncated u16 field".to_owned()))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

pub(super) fn read_u32_le(input: &[u8], offset: usize) -> Result<u32> {
    let bytes = input
        .get(offset..offset + 4)
        .ok_or_else(|| SfArkError::CorruptData("truncated u32 field".to_owned()))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(super) fn ascii_field(input: &[u8], offset: usize, len: usize) -> String {
    String::from_utf8_lossy(input.get(offset..offset + len).unwrap_or_default())
        .trim_end_matches('\0')
        .trim()
        .to_owned()
}
