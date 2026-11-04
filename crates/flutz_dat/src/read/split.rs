use super::*;

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
