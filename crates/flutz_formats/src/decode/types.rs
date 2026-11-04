use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioSummary {
    pub format: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub frames_decoded: u64,
    pub peak: f32,
    pub rms: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioBuffer {
    pub summary: DecodedAudioSummary,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioStreamMetadata {
    pub format: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub frame_length: Option<u64>,
    pub duration_seconds: Option<f64>,
    pub source_byte_len: Option<u64>,
    pub seekable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioStreamWindow {
    pub start_frame: u64,
    pub frames_decoded: usize,
    pub samples_decoded: usize,
    pub end_of_stream: bool,
    pub peak: f32,
    pub rms: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedAudioStreamConfig {
    pub media_source_buffer_bytes: usize,
    pub seek_index_fill_rate_seconds: u16,
}

impl Default for DecodedAudioStreamConfig {
    fn default() -> Self {
        Self {
            media_source_buffer_bytes: 128 * 1024,
            seek_index_fill_rate_seconds: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DecodedAudioStreamSource {
    Path(PathBuf),
    Bytes {
        bytes: Arc<[u8]>,
        hint_extension: Option<String>,
    },
}
