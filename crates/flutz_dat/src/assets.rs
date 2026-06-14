pub const DAT_ENTRY_FLAG_DEFAULT: u64 = 1 << 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatAssetEntry {
    pub internal_id: String,
    pub display_name: String,
    pub asset_type: String,
    pub source_format: String,
    pub storage_format: String,
    pub runtime_format: String,
    pub original_filename: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedDatAsset {
    pub entry: DatAssetEntry,
    pub flags: u64,
    pub payload: PreparedDatAssetPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedDatAssetPayload {
    Bytes(Vec<u8>),
    File(std::path::PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatChunkRecord {
    pub chunk_id: u64,
    pub file_offset: u64,
    pub stored_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatChunkExtent {
    pub chunk_id: u64,
    pub offset_in_chunk: u64,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatEntryRecord {
    pub entry: DatAssetEntry,
    pub total_size: u64,
    pub flags: u64,
    pub extents: Vec<DatChunkExtent>,
}
