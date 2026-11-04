use std::{
    collections::HashMap,
    fs,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

mod entries;
mod index;
mod soundfont_json;
mod split;
use entries::checked_range_end;
pub use entries::{extract_all_entries, extract_entry_bytes, extract_entry_from_file};
pub use index::{parse_dat_footer, parse_dat_index, parse_dat_index_file, DatArchiveIndex};
pub use soundfont_json::{
    absolute_soundfont_sample_ranges, find_soundfont_json_resource_entries,
    read_soundfont_coverage_json_from_file, read_soundfont_index_json_from_file,
    read_soundfont_json_resources_from_file, read_soundfont_sample_ranges_from_files,
    read_soundfont_thin_metadata_from_files, SoundFontJsonResourceEntries, SoundFontJsonResources,
    SoundFontSampleRangeRead, SoundFontSampleRangeReadSet, SoundFontSampleRangeRequest,
};
pub use split::{
    coalesce_entry_ranges, coalesce_extents, logical_split_entry_size, read_entry_range_plan,
    read_entry_ranges_from_file, read_split_entry_range_from_files, DatEntryRange,
    DatEntryRangePlan, SplitDatEntryPart, SplitDatRangeRead,
};

use flutz_core::{FlutzError, Result};

use crate::{
    assets::{DatAssetEntry, DatChunkExtent, DatChunkRecord, DatEntryRecord},
    soundfont_json::{
        from_json_bytes, SoundFontCoverageJson, SoundFontIndexJson, SoundFontMetadataJson,
        SoundFontPackReportJson, SOUNDFONT_COVERAGE_ASSET_TYPE, SOUNDFONT_INDEX_ASSET_TYPE,
        SOUNDFONT_METADATA_ASSET_TYPE, SOUNDFONT_PACK_REPORT_ASSET_TYPE,
    },
    DAT_FOOTER_MAGIC, DAT_INDEX_MAGIC, DAT_MAGIC,
};

pub fn validate_magic(bytes: &[u8]) -> Result<()> {
    if bytes.starts_with(DAT_MAGIC) {
        Ok(())
    } else {
        Err(FlutzError::InvalidInput("missing FDAT magic".to_owned()))
    }
}

pub fn read_dat_file(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let path = path.as_ref();
    fs::read(path).map_err(|error| {
        FlutzError::Runtime(format!(
            "failed to read DAT file {}: {error}",
            path.display()
        ))
    })
}
