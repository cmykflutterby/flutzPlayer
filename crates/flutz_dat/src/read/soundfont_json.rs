use super::*;

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

pub(super) fn read_json_entry_from_file_tolerant<T>(
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

pub(super) fn soundfont_json_internal_id(parent_soundfont_id: &str, suffix: &str) -> String {
    format!("{parent_soundfont_id}.{suffix}")
}
