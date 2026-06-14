pub mod assets;
pub mod manifest;
pub mod read;
pub mod registry;
pub mod soundfont_json;
pub mod write;

pub const DAT_MAGIC: &[u8; 4] = b"FDAT";
pub const DAT_INDEX_MAGIC: &[u8; 4] = b"DIDX";
pub const DAT_FOOTER_MAGIC: &[u8; 4] = b"DEND";
