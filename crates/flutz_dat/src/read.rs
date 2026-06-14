use std::{
    collections::HashMap,
    fs,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatArchiveIndex {
    pub chunk_size: u64,
    pub entries: Vec<DatEntryRecord>,
    pub chunks: Vec<DatChunkRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontJsonResourceEntries {
    pub metadata: Option<DatEntryRecord>,
    pub coverage: Option<DatEntryRecord>,
    pub index: Option<DatEntryRecord>,
    pub pack_report: Option<DatEntryRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontJsonResources {
    pub metadata: Option<SoundFontMetadataJson>,
    pub coverage: Option<SoundFontCoverageJson>,
    pub index: Option<SoundFontIndexJson>,
    pub pack_report: Option<SoundFontPackReportJson>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatEntryRange {
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatEntryRangePlan {
    pub entry_offset: u64,
    pub length: u64,
    pub chunk_id: u64,
    pub offset_in_chunk: u64,
    pub file_offset: u64,
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

pub fn parse_dat_index(bytes: &[u8]) -> Result<DatArchiveIndex> {
    validate_magic(bytes)?;
    let header = parse_dat_header(bytes)?;

    let index = parse_index_block(
        bytes,
        header.primary_index_offset,
        header.primary_index_length,
    )?;
    if index.entries.len() as u64 != header.entry_count
        || index.chunks.len() as u64 != header.chunk_count
    {
        return Err(FlutzError::InvalidInput(
            "DAT header counts do not match primary index".to_owned(),
        ));
    }

    Ok(DatArchiveIndex {
        chunk_size: header.chunk_size,
        entries: index.entries,
        chunks: index.chunks,
    })
}

pub fn parse_dat_index_file(path: impl AsRef<Path>) -> Result<DatArchiveIndex> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|error| {
        FlutzError::Runtime(format!(
            "failed to open DAT file {}: {error}",
            path.display()
        ))
    })?;
    let mut header_bytes = [0u8; DAT_HEADER_SIZE];
    file.read_exact(&mut header_bytes).map_err(|error| {
        FlutzError::Runtime(format!(
            "failed to read DAT header {}: {error}",
            path.display()
        ))
    })?;
    validate_magic(&header_bytes)?;
    let header = parse_dat_header(&header_bytes)?;
    file.seek(SeekFrom::Start(header.primary_index_offset))
        .map_err(|error| {
            FlutzError::Runtime(format!(
                "failed to seek DAT index {}: {error}",
                path.display()
            ))
        })?;
    let mut index_bytes = vec![0u8; header.primary_index_length as usize];
    file.read_exact(&mut index_bytes).map_err(|error| {
        FlutzError::Runtime(format!(
            "failed to read DAT index {}: {error}",
            path.display()
        ))
    })?;
    let index = parse_index_block_payload(&index_bytes, header.primary_index_length)?;
    if index.entries.len() as u64 != header.entry_count
        || index.chunks.len() as u64 != header.chunk_count
    {
        return Err(FlutzError::InvalidInput(
            "DAT header counts do not match primary index".to_owned(),
        ));
    }
    Ok(DatArchiveIndex {
        chunk_size: header.chunk_size,
        entries: index.entries,
        chunks: index.chunks,
    })
}

pub fn extract_entry_bytes(bytes: &[u8], internal_id: &str) -> Result<Vec<u8>> {
    let index = parse_dat_index(bytes)?;
    let entry = index
        .entries
        .iter()
        .find(|entry| entry.entry.internal_id == internal_id)
        .ok_or_else(|| FlutzError::InvalidInput(format!("DAT entry not found: {internal_id}")))?;
    extract_entry_from_index(bytes, &index, entry)
}

pub fn extract_all_entries(bytes: &[u8]) -> Result<Vec<(DatEntryRecord, Vec<u8>)>> {
    let index = parse_dat_index(bytes)?;
    let mut output = Vec::with_capacity(index.entries.len());
    for entry in &index.entries {
        output.push((
            entry.clone(),
            extract_entry_from_index(bytes, &index, entry)?,
        ));
    }
    Ok(output)
}

pub fn extract_entry_from_file(
    path: impl AsRef<Path>,
    index: &DatArchiveIndex,
    entry: &DatEntryRecord,
) -> Result<Vec<u8>> {
    let path = path.as_ref();
    let chunks = index
        .chunks
        .iter()
        .map(|chunk| (chunk.chunk_id, chunk))
        .collect::<HashMap<_, _>>();
    let mut file = File::open(path).map_err(|error| {
        FlutzError::Runtime(format!(
            "failed to open DAT file {}: {error}",
            path.display()
        ))
    })?;
    let mut output = Vec::with_capacity(entry.total_size as usize);

    for extent in &entry.extents {
        let chunk = chunks.get(&extent.chunk_id).ok_or_else(|| {
            FlutzError::InvalidInput(format!(
                "DAT extent references missing chunk {}",
                extent.chunk_id
            ))
        })?;
        let extent_end = extent
            .offset_in_chunk
            .checked_add(extent.length)
            .ok_or_else(|| FlutzError::InvalidInput("DAT extent length overflow".to_owned()))?;
        if extent_end > chunk.stored_length {
            return Err(FlutzError::InvalidInput(
                "DAT extent points outside chunk".to_owned(),
            ));
        }
        let start = chunk
            .file_offset
            .checked_add(extent.offset_in_chunk)
            .ok_or_else(|| FlutzError::InvalidInput("DAT extent offset overflow".to_owned()))?;
        file.seek(SeekFrom::Start(start)).map_err(|error| {
            FlutzError::Runtime(format!(
                "failed to seek DAT extent {}: {error}",
                path.display()
            ))
        })?;
        let output_start = output.len();
        output.resize(output_start + extent.length as usize, 0);
        file.read_exact(&mut output[output_start..])
            .map_err(|error| {
                FlutzError::Runtime(format!(
                    "failed to read DAT extent {}: {error}",
                    path.display()
                ))
            })?;
    }

    if output.len() as u64 != entry.total_size {
        return Err(FlutzError::InvalidInput(format!(
            "DAT entry {} size mismatch: expected {}, read {}",
            entry.entry.internal_id,
            entry.total_size,
            output.len()
        )));
    }

    Ok(output)
}

pub fn find_soundfont_json_resource_entries(
    index: &DatArchiveIndex,
    parent_soundfont_id: &str,
) -> SoundFontJsonResourceEntries {
    let metadata_id = soundfont_json_internal_id(parent_soundfont_id, "metadata");
    let coverage_id = soundfont_json_internal_id(parent_soundfont_id, "coverage");
    let index_id = soundfont_json_internal_id(parent_soundfont_id, "index");
    let pack_report_id = soundfont_json_internal_id(parent_soundfont_id, "pack-report");

    let mut entries = SoundFontJsonResourceEntries::default();
    for record in &index.entries {
        match record.entry.asset_type.as_str() {
            SOUNDFONT_METADATA_ASSET_TYPE if record.entry.internal_id == metadata_id => {
                entries.metadata = Some(record.clone());
            }
            SOUNDFONT_COVERAGE_ASSET_TYPE if record.entry.internal_id == coverage_id => {
                entries.coverage = Some(record.clone());
            }
            SOUNDFONT_INDEX_ASSET_TYPE if record.entry.internal_id == index_id => {
                entries.index = Some(record.clone());
            }
            SOUNDFONT_PACK_REPORT_ASSET_TYPE if record.entry.internal_id == pack_report_id => {
                entries.pack_report = Some(record.clone());
            }
            _ => {}
        }
    }
    entries
}

pub fn read_soundfont_json_resources_from_file(
    path: impl AsRef<Path>,
    index: &DatArchiveIndex,
    parent_soundfont_id: &str,
) -> Result<SoundFontJsonResources> {
    let entries = find_soundfont_json_resource_entries(index, parent_soundfont_id);
    Ok(SoundFontJsonResources {
        metadata: read_json_entry_from_file_tolerant(
            path.as_ref(),
            index,
            entries.metadata.as_ref(),
        )?
        .filter(|metadata: &SoundFontMetadataJson| {
            metadata.parent_soundfont_id == parent_soundfont_id
        }),
        coverage: read_json_entry_from_file_tolerant(
            path.as_ref(),
            index,
            entries.coverage.as_ref(),
        )?
        .filter(|coverage: &SoundFontCoverageJson| {
            coverage.parent_soundfont_id == parent_soundfont_id
        }),
        index: read_json_entry_from_file_tolerant(path.as_ref(), index, entries.index.as_ref())?
            .filter(|index_json: &SoundFontIndexJson| {
                index_json.parent_soundfont_id == parent_soundfont_id
            }),
        pack_report: read_json_entry_from_file_tolerant(
            path.as_ref(),
            index,
            entries.pack_report.as_ref(),
        )?
        .filter(|report: &SoundFontPackReportJson| {
            report.parent_soundfont_id == parent_soundfont_id
        }),
    })
}

pub fn read_soundfont_coverage_json_from_file(
    path: impl AsRef<Path>,
    index: &DatArchiveIndex,
    parent_soundfont_id: &str,
) -> Result<Option<SoundFontCoverageJson>> {
    Ok(read_soundfont_json_resources_from_file(path, index, parent_soundfont_id)?.coverage)
}

pub fn read_soundfont_index_json_from_file(
    path: impl AsRef<Path>,
    index: &DatArchiveIndex,
    parent_soundfont_id: &str,
) -> Result<Option<SoundFontIndexJson>> {
    Ok(read_soundfont_json_resources_from_file(path, index, parent_soundfont_id)?.index)
}

pub fn read_entry_range_plan(
    index: &DatArchiveIndex,
    entry: &DatEntryRecord,
    ranges: &[DatEntryRange],
) -> Result<Vec<DatEntryRangePlan>> {
    let chunks = index
        .chunks
        .iter()
        .map(|chunk| (chunk.chunk_id, chunk))
        .collect::<HashMap<_, _>>();
    let ranges = coalesce_entry_ranges(ranges)?;
    let mut plan = Vec::new();

    for range in ranges {
        let requested_end = checked_range_end(range.offset, range.length, entry.total_size)?;
        let mut entry_extent_offset = 0u64;
        for extent in &entry.extents {
            let extent_entry_start = entry_extent_offset;
            let extent_entry_end =
                extent_entry_start
                    .checked_add(extent.length)
                    .ok_or_else(|| {
                        FlutzError::InvalidInput("DAT extent entry offset overflow".to_owned())
                    })?;
            entry_extent_offset = extent_entry_end;

            if requested_end <= extent_entry_start || range.offset >= extent_entry_end {
                continue;
            }

            let chunk = chunks.get(&extent.chunk_id).ok_or_else(|| {
                FlutzError::InvalidInput(format!(
                    "DAT extent references missing chunk {}",
                    extent.chunk_id
                ))
            })?;
            let extent_chunk_end = extent
                .offset_in_chunk
                .checked_add(extent.length)
                .ok_or_else(|| FlutzError::InvalidInput("DAT extent length overflow".to_owned()))?;
            if extent_chunk_end > chunk.stored_length {
                return Err(FlutzError::InvalidInput(
                    "DAT extent points outside chunk".to_owned(),
                ));
            }

            let overlap_start = range.offset.max(extent_entry_start);
            let overlap_end = requested_end.min(extent_entry_end);
            let overlap_length = overlap_end.checked_sub(overlap_start).ok_or_else(|| {
                FlutzError::InvalidInput("DAT range overlap underflow".to_owned())
            })?;
            let offset_in_extent =
                overlap_start
                    .checked_sub(extent_entry_start)
                    .ok_or_else(|| {
                        FlutzError::InvalidInput("DAT range extent offset underflow".to_owned())
                    })?;
            let offset_in_chunk = extent
                .offset_in_chunk
                .checked_add(offset_in_extent)
                .ok_or_else(|| FlutzError::InvalidInput("DAT chunk offset overflow".to_owned()))?;
            let file_offset = chunk
                .file_offset
                .checked_add(offset_in_chunk)
                .ok_or_else(|| FlutzError::InvalidInput("DAT file offset overflow".to_owned()))?;
            plan.push(DatEntryRangePlan {
                entry_offset: overlap_start,
                length: overlap_length,
                chunk_id: extent.chunk_id,
                offset_in_chunk,
                file_offset,
            });
        }
    }

    Ok(coalesce_extents(&plan))
}

pub fn read_entry_ranges_from_file(
    path: impl AsRef<Path>,
    index: &DatArchiveIndex,
    entry: &DatEntryRecord,
    ranges: &[DatEntryRange],
) -> Result<Vec<u8>> {
    let plan = read_entry_range_plan(index, entry, ranges)?;
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|error| {
        FlutzError::Runtime(format!(
            "failed to open DAT file {}: {error}",
            path.display()
        ))
    })?;
    let total_length = plan.iter().map(|range| range.length as usize).sum();
    let mut output = Vec::with_capacity(total_length);
    for range in plan {
        file.seek(SeekFrom::Start(range.file_offset))
            .map_err(|error| {
                FlutzError::Runtime(format!(
                    "failed to seek DAT range {}: {error}",
                    path.display()
                ))
            })?;
        let output_start = output.len();
        output.resize(output_start + range.length as usize, 0);
        file.read_exact(&mut output[output_start..])
            .map_err(|error| {
                FlutzError::Runtime(format!(
                    "failed to read DAT range {}: {error}",
                    path.display()
                ))
            })?;
    }
    Ok(output)
}

#[derive(Debug, Clone, Copy)]
pub struct SplitDatEntryPart<'a> {
    pub path: &'a Path,
    pub index: &'a DatArchiveIndex,
    pub entry: &'a DatEntryRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitDatRangeRead {
    pub bytes: Vec<u8>,
    pub read_count: usize,
    pub byte_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundFontSampleRangeRequest {
    pub sample_id: u32,
    pub range: DatEntryRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundFontSampleRangeRead {
    pub sample_id: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundFontSampleRangeReadSet {
    pub samples: Vec<SoundFontSampleRangeRead>,
    pub read_count: usize,
    pub byte_count: usize,
}

pub fn logical_split_entry_size(parts: &[SplitDatEntryPart<'_>]) -> Result<u64> {
    parts.iter().try_fold(0u64, |total, part| {
        total.checked_add(part.entry.total_size).ok_or_else(|| {
            FlutzError::InvalidInput("split DAT entry logical size overflow".to_owned())
        })
    })
}

pub fn read_split_entry_range_from_files(
    parts: &[SplitDatEntryPart<'_>],
    offset: u64,
    length: u64,
) -> Result<SplitDatRangeRead> {
    let requested_end = offset
        .checked_add(length)
        .ok_or_else(|| FlutzError::InvalidInput("split DAT logical range overflow".to_owned()))?;
    let capacity = usize::try_from(length).map_err(|_| {
        FlutzError::InvalidInput("split DAT logical range is too large to allocate".to_owned())
    })?;
    let mut entry_start = 0u64;
    let mut output = Vec::with_capacity(capacity);
    let mut read_count = 0usize;

    for part in parts {
        let entry_end = entry_start
            .checked_add(part.entry.total_size)
            .ok_or_else(|| FlutzError::InvalidInput("split DAT part overflow".to_owned()))?;
        if requested_end > entry_start && offset < entry_end {
            let overlap_start = offset.max(entry_start);
            let overlap_end = requested_end.min(entry_end);
            let local_offset = overlap_start.checked_sub(entry_start).ok_or_else(|| {
                FlutzError::InvalidInput("split DAT part offset underflow".to_owned())
            })?;
            let local_length = overlap_end.checked_sub(overlap_start).ok_or_else(|| {
                FlutzError::InvalidInput("split DAT part range underflow".to_owned())
            })?;
            let bytes = read_entry_ranges_from_file(
                part.path,
                part.index,
                part.entry,
                &[DatEntryRange {
                    offset: local_offset,
                    length: local_length,
                }],
            )?;
            read_count = read_count.saturating_add(1);
            output.extend_from_slice(&bytes);
        }
        entry_start = entry_end;
    }

    if output.len() != capacity {
        return Err(FlutzError::InvalidInput(format!(
            "split DAT logical range read {} bytes, expected {length}",
            output.len()
        )));
    }
    let byte_count = output.len();
    Ok(SplitDatRangeRead {
        bytes: output,
        read_count,
        byte_count,
    })
}

pub fn absolute_soundfont_sample_ranges(
    index: &SoundFontIndexJson,
    sample_ids: &[u32],
) -> Result<Vec<SoundFontSampleRangeRequest>> {
    let smpl_data_start = index.smpl_data_start_byte.ok_or_else(|| {
        FlutzError::InvalidInput("soundfont index is missing SMPL data base".to_owned())
    })?;
    let samples = index
        .samples
        .iter()
        .map(|sample| (sample.sample_id, sample))
        .collect::<HashMap<_, _>>();
    let mut ranges = Vec::with_capacity(sample_ids.len());
    for sample_id in sample_ids {
        let sample = samples.get(sample_id).ok_or_else(|| {
            FlutzError::InvalidInput(format!(
                "soundfont index is missing requested sample {sample_id}"
            ))
        })?;
        let offset = smpl_data_start
            .checked_add(sample.wave_range.smpl_start_byte)
            .ok_or_else(|| {
                FlutzError::InvalidInput("soundfont sample offset overflow".to_owned())
            })?;
        ranges.push(SoundFontSampleRangeRequest {
            sample_id: *sample_id,
            range: DatEntryRange {
                offset,
                length: sample.wave_range.byte_length,
            },
        });
    }
    Ok(ranges)
}

pub fn read_soundfont_sample_ranges_from_files(
    parts: &[SplitDatEntryPart<'_>],
    index: &SoundFontIndexJson,
    sample_ids: &[u32],
) -> Result<SoundFontSampleRangeReadSet> {
    let ranges = absolute_soundfont_sample_ranges(index, sample_ids)?;
    let mut samples = Vec::with_capacity(ranges.len());
    let mut read_count = 0usize;
    let mut byte_count = 0usize;
    for request in ranges {
        let read =
            read_split_entry_range_from_files(parts, request.range.offset, request.range.length)?;
        read_count = read_count.saturating_add(read.read_count);
        byte_count = byte_count.saturating_add(read.byte_count);
        samples.push(SoundFontSampleRangeRead {
            sample_id: request.sample_id,
            bytes: read.bytes,
        });
    }
    Ok(SoundFontSampleRangeReadSet {
        samples,
        read_count,
        byte_count,
    })
}

pub fn read_soundfont_thin_metadata_from_files(
    parts: &[SplitDatEntryPart<'_>],
    index: &SoundFontIndexJson,
) -> Result<SplitDatRangeRead> {
    let smpl_data_start = index.smpl_data_start_byte.ok_or_else(|| {
        FlutzError::InvalidInput("soundfont index is missing SMPL data base".to_owned())
    })?;
    let smpl_header_start = smpl_data_start.checked_sub(8).ok_or_else(|| {
        FlutzError::InvalidInput("soundfont SMPL data base is too small".to_owned())
    })?;
    let smpl_header = read_split_entry_range_from_files(parts, smpl_header_start, 8)?;
    if smpl_header.bytes.get(0..4) != Some(b"smpl") {
        return Err(FlutzError::InvalidInput(
            "soundfont SMPL header was not found at indexed offset".to_owned(),
        ));
    }
    let smpl_data_len = u32::from_le_bytes(
        smpl_header.bytes[4..8]
            .try_into()
            .map_err(|_| FlutzError::InvalidInput("invalid soundfont SMPL size".to_owned()))?,
    ) as u64;
    let smpl_data_end = smpl_data_start
        .checked_add(smpl_data_len)
        .ok_or_else(|| FlutzError::InvalidInput("soundfont SMPL end overflow".to_owned()))?;
    let post_smpl_start = smpl_data_end
        .checked_add(smpl_data_len & 1)
        .ok_or_else(|| FlutzError::InvalidInput("soundfont SMPL padding overflow".to_owned()))?;
    let total_size = logical_split_entry_size(parts)?;
    if post_smpl_start > total_size {
        return Err(FlutzError::InvalidInput(
            "soundfont SMPL range points outside logical DAT entry".to_owned(),
        ));
    }

    let pre_smpl = read_split_entry_range_from_files(parts, 0, smpl_header_start)?;
    let sdta_end = top_level_sdta_list_end(&pre_smpl.bytes, total_size)?;
    let post_metadata_start = post_smpl_start.max(sdta_end);
    let post_len = total_size - post_metadata_start;
    let post_smpl = if post_len > 0 {
        read_split_entry_range_from_files(parts, post_metadata_start, post_len)?
    } else {
        SplitDatRangeRead {
            bytes: Vec::new(),
            read_count: 0,
            byte_count: 0,
        }
    };
    let metadata_bytes = thin_soundfont_metadata_bytes(pre_smpl.bytes, post_smpl.bytes)?;
    Ok(SplitDatRangeRead {
        bytes: metadata_bytes,
        read_count: smpl_header
            .read_count
            .saturating_add(pre_smpl.read_count)
            .saturating_add(post_smpl.read_count),
        byte_count: smpl_header
            .byte_count
            .saturating_add(pre_smpl.byte_count)
            .saturating_add(post_smpl.byte_count),
    })
}

fn thin_soundfont_metadata_bytes(mut pre_smpl: Vec<u8>, post_smpl: Vec<u8>) -> Result<Vec<u8>> {
    let sdta_start = find_top_level_sdta_list(&pre_smpl).ok_or_else(|| {
        FlutzError::InvalidInput("soundfont metadata prefix is missing sdta LIST".to_owned())
    })?;
    let sdta_payload_len = pre_smpl
        .len()
        .checked_sub(sdta_start + 8)
        .and_then(|length| length.checked_add(12))
        .ok_or_else(|| FlutzError::InvalidInput("thin soundfont sdta size overflow".to_owned()))?;
    let sdta_payload_len = u32::try_from(sdta_payload_len)
        .map_err(|_| FlutzError::InvalidInput("thin soundfont sdta size too large".to_owned()))?;
    pre_smpl[sdta_start + 4..sdta_start + 8].copy_from_slice(&sdta_payload_len.to_le_bytes());
    pre_smpl.extend_from_slice(b"smpl");
    pre_smpl.extend_from_slice(&4u32.to_le_bytes());
    pre_smpl.extend_from_slice(&[0, 0, 0, 0]);
    pre_smpl.extend_from_slice(&post_smpl);
    Ok(pre_smpl)
}

fn find_top_level_sdta_list(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"sfbk" {
        return None;
    }
    let mut offset = 12usize;
    while offset.checked_add(12)? <= bytes.len() {
        let id = bytes.get(offset..offset + 4)?;
        let length =
            u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        let data_start = offset.checked_add(8)?;
        if id == b"LIST" && bytes.get(data_start..data_start + 4)? == b"sdta" {
            return Some(offset);
        }
        offset = data_start.checked_add(length)?.checked_add(length & 1)?;
    }
    None
}

fn top_level_sdta_list_end(bytes: &[u8], total_size: u64) -> Result<u64> {
    let sdta_start = find_top_level_sdta_list(bytes).ok_or_else(|| {
        FlutzError::InvalidInput("soundfont metadata prefix is missing sdta LIST".to_owned())
    })?;
    let length = u32::from_le_bytes(
        bytes[sdta_start + 4..sdta_start + 8]
            .try_into()
            .map_err(|_| FlutzError::InvalidInput("invalid soundfont sdta size".to_owned()))?,
    ) as u64;
    let end = (sdta_start as u64)
        .checked_add(8)
        .and_then(|offset| offset.checked_add(length))
        .and_then(|offset| offset.checked_add(length & 1))
        .ok_or_else(|| FlutzError::InvalidInput("soundfont sdta end overflow".to_owned()))?;
    if end > total_size {
        return Err(FlutzError::InvalidInput(
            "soundfont sdta LIST points outside logical DAT entry".to_owned(),
        ));
    }
    Ok(end)
}

pub fn coalesce_entry_ranges(ranges: &[DatEntryRange]) -> Result<Vec<DatEntryRange>> {
    let mut sorted = Vec::new();
    for range in ranges {
        if range.length == 0 {
            continue;
        }
        let end = range.offset.checked_add(range.length).ok_or_else(|| {
            FlutzError::InvalidInput("DAT requested range length overflow".to_owned())
        })?;
        sorted.push((range.offset, end));
    }
    sorted.sort_unstable();

    let mut coalesced = Vec::<DatEntryRange>::new();
    for (offset, end) in sorted {
        let Some(last) = coalesced.last_mut() else {
            coalesced.push(DatEntryRange {
                offset,
                length: end - offset,
            });
            continue;
        };
        let last_end = last.offset.checked_add(last.length).ok_or_else(|| {
            FlutzError::InvalidInput("DAT coalesced range length overflow".to_owned())
        })?;
        if offset <= last_end {
            if end > last_end {
                last.length = end - last.offset;
            }
        } else {
            coalesced.push(DatEntryRange {
                offset,
                length: end - offset,
            });
        }
    }
    Ok(coalesced)
}

pub fn coalesce_extents(plan: &[DatEntryRangePlan]) -> Vec<DatEntryRangePlan> {
    let mut coalesced = Vec::<DatEntryRangePlan>::new();
    for range in plan {
        if range.length == 0 {
            continue;
        }
        let Some(last) = coalesced.last_mut() else {
            coalesced.push(*range);
            continue;
        };
        let last_entry_end = last.entry_offset + last.length;
        let last_file_end = last.file_offset + last.length;
        let last_chunk_end = last.offset_in_chunk + last.length;
        if last.chunk_id == range.chunk_id
            && last_entry_end == range.entry_offset
            && last_file_end == range.file_offset
            && last_chunk_end == range.offset_in_chunk
        {
            last.length += range.length;
        } else {
            coalesced.push(*range);
        }
    }
    coalesced
}

pub fn parse_dat_footer(bytes: &[u8]) -> Result<(u64, u64)> {
    let footer_size = 4 + 8 * 3;
    let footer_start = bytes
        .len()
        .checked_sub(footer_size)
        .ok_or_else(|| FlutzError::InvalidInput("DAT file is too small for footer".to_owned()))?;
    let mut cursor = Cursor::new(&bytes[footer_start..]);
    let magic = cursor.take_bytes(4)?;
    if magic != DAT_FOOTER_MAGIC {
        return Err(FlutzError::InvalidInput("missing DEND magic".to_owned()));
    }
    let backup_index_offset = cursor.take_u64()?;
    let backup_index_length = cursor.take_u64()?;
    let parsed_footer_size = cursor.take_u64()?;
    if parsed_footer_size != footer_size as u64 {
        return Err(FlutzError::InvalidInput(
            "DAT footer size mismatch".to_owned(),
        ));
    }
    Ok((backup_index_offset, backup_index_length))
}

fn extract_entry_from_index(
    bytes: &[u8],
    index: &DatArchiveIndex,
    entry: &DatEntryRecord,
) -> Result<Vec<u8>> {
    let chunks = index
        .chunks
        .iter()
        .map(|chunk| (chunk.chunk_id, chunk))
        .collect::<HashMap<_, _>>();
    let mut output = Vec::with_capacity(entry.total_size as usize);

    for extent in &entry.extents {
        let chunk = chunks.get(&extent.chunk_id).ok_or_else(|| {
            FlutzError::InvalidInput(format!(
                "DAT extent references missing chunk {}",
                extent.chunk_id
            ))
        })?;
        let start = chunk
            .file_offset
            .checked_add(extent.offset_in_chunk)
            .ok_or_else(|| FlutzError::InvalidInput("DAT extent offset overflow".to_owned()))?;
        let end = start
            .checked_add(extent.length)
            .ok_or_else(|| FlutzError::InvalidInput("DAT extent length overflow".to_owned()))?;
        let slice = bytes
            .get(start as usize..end as usize)
            .ok_or_else(|| FlutzError::InvalidInput("DAT extent points outside file".to_owned()))?;
        output.extend_from_slice(slice);
    }

    if output.len() as u64 != entry.total_size {
        return Err(FlutzError::InvalidInput(format!(
            "DAT entry {} size mismatch: expected {}, read {}",
            entry.entry.internal_id,
            entry.total_size,
            output.len()
        )));
    }

    Ok(output)
}

fn read_json_entry_from_file_tolerant<T>(
    path: &Path,
    index: &DatArchiveIndex,
    entry: Option<&DatEntryRecord>,
) -> Result<Option<T>>
where
    T: serde::de::DeserializeOwned,
{
    let Some(entry) = entry else {
        return Ok(None);
    };
    let bytes = extract_entry_from_file(path, index, entry)?;
    Ok(from_json_bytes(&bytes).ok())
}

fn soundfont_json_internal_id(parent_soundfont_id: &str, suffix: &str) -> String {
    format!("{parent_soundfont_id}.{suffix}")
}

fn checked_range_end(offset: u64, length: u64, entry_total_size: u64) -> Result<u64> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| FlutzError::InvalidInput("DAT requested range overflow".to_owned()))?;
    if end > entry_total_size {
        return Err(FlutzError::InvalidInput(
            "DAT requested range points outside entry".to_owned(),
        ));
    }
    Ok(end)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedIndexBlock {
    entries: Vec<DatEntryRecord>,
    chunks: Vec<DatChunkRecord>,
}

const DAT_HEADER_SIZE: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedDatHeader {
    chunk_size: u64,
    primary_index_offset: u64,
    primary_index_length: u64,
    entry_count: u64,
    chunk_count: u64,
}

fn parse_dat_header(bytes: &[u8]) -> Result<ParsedDatHeader> {
    let mut cursor = Cursor::new(bytes);
    cursor.take_bytes(4)?;
    let header_size = cursor.take_u64()?;
    cursor.take_u64()?;
    let chunk_size = cursor.take_u64()?;
    let primary_index_offset = cursor.take_u64()?;
    let primary_index_length = cursor.take_u64()?;
    let entry_count = cursor.take_u64()?;
    let chunk_count = cursor.take_u64()?;

    if header_size < DAT_HEADER_SIZE as u64 {
        return Err(FlutzError::InvalidInput(
            "DAT header is too small".to_owned(),
        ));
    }

    Ok(ParsedDatHeader {
        chunk_size,
        primary_index_offset,
        primary_index_length,
        entry_count,
        chunk_count,
    })
}

fn parse_index_block(bytes: &[u8], offset: u64, length: u64) -> Result<ParsedIndexBlock> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| FlutzError::InvalidInput("DAT index offset overflow".to_owned()))?;
    let block = bytes
        .get(offset as usize..end as usize)
        .ok_or_else(|| FlutzError::InvalidInput("DAT index points outside file".to_owned()))?;
    parse_index_block_payload(block, length)
}

fn parse_index_block_payload(block: &[u8], length: u64) -> Result<ParsedIndexBlock> {
    let mut cursor = Cursor::new(block);
    let magic = cursor.take_bytes(4)?;
    if magic != DAT_INDEX_MAGIC {
        return Err(FlutzError::InvalidInput("missing DIDX magic".to_owned()));
    }
    let index_length = cursor.take_u64()?;
    if index_length != length {
        return Err(FlutzError::InvalidInput(
            "DAT index length mismatch".to_owned(),
        ));
    }
    cursor.take_u64()?;
    let entry_count = cursor.take_u64()?;
    let chunk_count = cursor.take_u64()?;

    let mut chunks = Vec::with_capacity(chunk_count as usize);
    for _ in 0..chunk_count {
        chunks.push(DatChunkRecord {
            chunk_id: cursor.take_u64()?,
            file_offset: cursor.take_u64()?,
            stored_length: cursor.take_u64()?,
        });
    }

    let mut entries = Vec::with_capacity(entry_count as usize);
    for _ in 0..entry_count {
        let entry = DatAssetEntry {
            internal_id: cursor.take_string()?,
            display_name: cursor.take_string()?,
            asset_type: cursor.take_string()?,
            source_format: cursor.take_string()?,
            storage_format: cursor.take_string()?,
            runtime_format: cursor.take_string()?,
            original_filename: cursor.take_string()?,
        };
        let total_size = cursor.take_u64()?;
        let flags = cursor.take_u64()?;
        let extent_count = cursor.take_u64()?;
        let mut extents = Vec::with_capacity(extent_count as usize);
        for _ in 0..extent_count {
            extents.push(DatChunkExtent {
                chunk_id: cursor.take_u64()?,
                offset_in_chunk: cursor.take_u64()?,
                length: cursor.take_u64()?,
            });
        }
        entries.push(DatEntryRecord {
            entry,
            total_size,
            flags,
            extents,
        });
    }

    Ok(ParsedIndexBlock { entries, chunks })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take_bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(|| FlutzError::InvalidInput("DAT cursor overflow".to_owned()))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| FlutzError::InvalidInput("truncated DAT data".to_owned()))?;
        self.offset = end;
        Ok(bytes)
    }

    fn take_u64(&mut self) -> Result<u64> {
        let bytes = self.take_bytes(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
            FlutzError::InvalidInput("truncated DAT u64 field".to_owned())
        })?))
    }

    fn take_string(&mut self) -> Result<String> {
        let length = self.take_u64()?;
        let bytes = self.take_bytes(length as usize)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| FlutzError::InvalidInput(format!("invalid DAT UTF-8 string: {error}")))
    }
}
