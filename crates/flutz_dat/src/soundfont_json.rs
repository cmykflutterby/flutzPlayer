use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const SOUNDFONT_METADATA_ASSET_TYPE: &str = "soundfont-metadata-json";
pub const SOUNDFONT_COVERAGE_ASSET_TYPE: &str = "soundfont-coverage-json";
pub const SOUNDFONT_INDEX_ASSET_TYPE: &str = "soundfont-index-json";
pub const SOUNDFONT_PACK_REPORT_ASSET_TYPE: &str = "soundfont-pack-report-json";
pub const JSON_FORMAT: &str = "json";

pub fn to_json_bytes<T>(value: &T) -> serde_json::Result<Vec<u8>>
where
    T: Serialize,
{
    serde_json::to_vec_pretty(value)
}

pub fn from_json_bytes<T>(bytes: &[u8]) -> serde_json::Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(bytes)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedSoundFontJsonResources {
    pub metadata_internal_id: String,
    pub coverage_internal_id: String,
    pub index_internal_id: String,
    #[serde(default)]
    pub pack_report_internal_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundFontPackMeasurementsJson {
    #[serde(default)]
    pub source_byte_count: Option<u64>,
    #[serde(default)]
    pub runtime_byte_count: Option<u64>,
    #[serde(default)]
    pub preset_count: Option<u32>,
    #[serde(default)]
    pub instrument_count: Option<u32>,
    #[serde(default)]
    pub sample_count: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundFontMetadataJson {
    pub parent_soundfont_id: String,
    pub display_name: String,
    pub source_format: String,
    pub storage_format: String,
    pub runtime_format: String,
    pub original_filename: String,
    pub generated_resources: GeneratedSoundFontJsonResources,
    pub preset_count: u32,
    pub sample_count: u32,
    #[serde(default)]
    pub pack_measurements: Option<SoundFontPackMeasurementsJson>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BankProgramCoverageJson {
    pub bank: u16,
    pub program: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PercussionKeyRangeJson {
    pub low_key: u8,
    pub high_key: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetCoverageJson {
    pub preset_id: u32,
    pub name: String,
    pub bank: u16,
    pub program: u8,
    pub percussion: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundFontCoverageJson {
    pub parent_soundfont_id: String,
    #[serde(default)]
    pub melodic: Vec<BankProgramCoverageJson>,
    pub percussion: bool,
    #[serde(default)]
    pub percussion_key_ranges: Vec<PercussionKeyRangeJson>,
    #[serde(default)]
    pub presets: Vec<PresetCoverageJson>,
    pub sample_count: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRangeJson {
    pub low_key: u8,
    pub high_key: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VelocityRangeJson {
    pub low_velocity: u8,
    pub high_velocity: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratorValueJson {
    pub generator_type: String,
    pub raw_value: i16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundFontWaveRangeJson {
    pub smpl_start_sample: u32,
    pub smpl_end_sample: u32,
    pub smpl_start_byte: u64,
    pub byte_length: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleHeaderJson {
    pub sample_id: u32,
    pub name: String,
    pub start: u32,
    pub end: u32,
    pub start_loop: u32,
    pub end_loop: u32,
    pub sample_rate: u32,
    pub original_pitch: u8,
    pub pitch_correction: i8,
    pub link: u16,
    pub sample_type: u16,
    pub wave_range: SoundFontWaveRangeJson,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentRegionJson {
    pub region_id: u32,
    #[serde(default)]
    pub sample_id: Option<u32>,
    #[serde(default)]
    pub key_range: Option<KeyRangeJson>,
    #[serde(default)]
    pub velocity_range: Option<VelocityRangeJson>,
    #[serde(default)]
    pub generators: Vec<GeneratorValueJson>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentIndexJson {
    pub instrument_id: u32,
    pub name: String,
    #[serde(default)]
    pub regions: Vec<InstrumentRegionJson>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetIndexJson {
    pub preset_id: u32,
    pub name: String,
    pub bank: u16,
    pub program: u8,
    #[serde(default)]
    pub instrument_ids: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentLoadMapJson {
    pub instrument_id: u32,
    #[serde(default)]
    pub instrument_name: Option<String>,
    #[serde(default)]
    pub region_ids: Vec<u32>,
    #[serde(default)]
    pub sample_ids: Vec<u32>,
    #[serde(default)]
    pub sample_header_ids: Vec<u32>,
    #[serde(default)]
    pub generator_region_ids: Vec<u32>,
    #[serde(default)]
    pub wave_ranges: Vec<SoundFontWaveRangeJson>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetLoadMapJson {
    pub preset_id: u32,
    #[serde(default)]
    pub preset_name: Option<String>,
    #[serde(default)]
    pub instrument_ids: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundFontIndexJson {
    pub parent_soundfont_id: String,
    #[serde(default)]
    pub smpl_data_start_byte: Option<u64>,
    #[serde(default)]
    pub presets: Vec<PresetIndexJson>,
    #[serde(default)]
    pub instruments: Vec<InstrumentIndexJson>,
    #[serde(default)]
    pub samples: Vec<SampleHeaderJson>,
    #[serde(default)]
    pub preset_load_map: Vec<PresetLoadMapJson>,
    #[serde(default)]
    pub instrument_load_map: Vec<InstrumentLoadMapJson>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatPhysicalRangeJson {
    pub dat_file: String,
    pub chunk_id: u64,
    pub file_offset: u64,
    pub extent_offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundFontPackWarningJson {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundFontPackReportJson {
    pub parent_soundfont_id: String,
    pub generated_resources: GeneratedSoundFontJsonResources,
    #[serde(default)]
    pub measurements: SoundFontPackMeasurementsJson,
    #[serde(default)]
    pub warnings: Vec<SoundFontPackWarningJson>,
    #[serde(default)]
    pub raw_soundfont_ranges: Vec<DatPhysicalRangeJson>,
}
