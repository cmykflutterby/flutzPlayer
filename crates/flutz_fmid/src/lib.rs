pub mod chunks;
pub mod read;
pub mod records;
pub mod write;

pub use read::{read_fmid, validate_magic};
pub use records::{
    FmidChunkRecord, FmidFile, LoopMode, LoopRecord, MasterMixerRecord, MixerRecord,
    MixerSourceMode, MixerStripControls, MixerStripIdentity, MixerStripRecord, ProjectRecord,
    SmartMixRecord, SoundFontRowMuteRecord, SoundFontSlot, UnknownChunk,
};
pub use write::{fmid_magic_bytes, write_fmid};

pub const FMID_MAGIC: &[u8; 4] = b"FMID";
pub const FMID_CHUNK_TABLE_MAGIC: &[u8; 4] = b"FIDX";
