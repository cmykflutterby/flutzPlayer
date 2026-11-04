use super::encode::{encode_index, DAT_FOOTER_SIZE, DAT_HEADER_SIZE};
use super::validate::validate_entry;
use super::*;

pub(super) struct PlannedArchiveLayout {
    pub(super) primary_index: Vec<u8>,
    pub(super) entry_records: Vec<DatEntryRecord>,
    pub(super) chunk_records: Vec<DatChunkRecord>,
    pub(super) write_extents: Vec<PlannedWriteExtent>,
    pub(super) backup_index_offset: u64,
    pub(super) total_byte_count: u64,
}

pub(super) struct PlannedWriteExtent {
    pub(super) asset_index: usize,
    pub(super) source_offset: u64,
    pub(super) length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AssetSlice {
    pub(super) asset_index: usize,
    pub(super) source_offset: u64,
    pub(super) length: u64,
}

pub(super) fn plan_archive_layout(
    assets: &[PreparedDatAsset],
    asset_slices: &[AssetSlice],
    chunk_size: u64,
) -> Result<PlannedArchiveLayout> {
    if chunk_size == 0 {
        return Err(FlutzError::InvalidInput(
            "DAT chunk size must be greater than zero".to_owned(),
        ));
    }

    let mut seen_ids = HashSet::new();
    for asset_slice in asset_slices {
        let asset = &assets[asset_slice.asset_index];
        validate_entry(&asset.entry)?;
        if !seen_ids.insert(asset.entry.internal_id.clone()) {
            return Err(FlutzError::InvalidInput(format!(
                "duplicate DAT internal ID: {}",
                asset.entry.internal_id
            )));
        }
    }

    let mut chunk_lengths = Vec::<u64>::new();
    let mut entry_records = Vec::with_capacity(asset_slices.len());
    let mut write_extents = Vec::new();

    for asset_slice in asset_slices {
        let asset = &assets[asset_slice.asset_index];
        let total_size = asset_slice.length;
        let mut remaining = total_size;
        let mut source_offset = asset_slice.source_offset;
        let mut extents = Vec::new();

        if total_size == 0 {
            chunk_lengths.push(0);
            extents.push(DatChunkExtent {
                chunk_id: (chunk_lengths.len() - 1) as u64,
                offset_in_chunk: 0,
                length: 0,
            });
        }

        while remaining > 0 {
            if chunk_lengths
                .last()
                .map(|length| *length == chunk_size)
                .unwrap_or(true)
            {
                chunk_lengths.push(0);
            }

            let chunk_id = chunk_lengths.len() - 1;
            let offset_in_chunk = chunk_lengths[chunk_id];
            let available = chunk_size - offset_in_chunk;
            let length = remaining.min(available);

            extents.push(DatChunkExtent {
                chunk_id: chunk_id as u64,
                offset_in_chunk,
                length,
            });
            write_extents.push(PlannedWriteExtent {
                asset_index: asset_slice.asset_index,
                source_offset,
                length,
            });

            chunk_lengths[chunk_id] += length;
            source_offset += length;
            remaining -= length;
        }

        entry_records.push(DatEntryRecord {
            entry: asset.entry.clone(),
            total_size,
            flags: asset.flags,
            extents,
        });
    }

    let placeholder_chunk_records = chunk_lengths
        .iter()
        .enumerate()
        .map(|(chunk_id, stored_length)| DatChunkRecord {
            chunk_id: chunk_id as u64,
            file_offset: 0,
            stored_length: *stored_length,
        })
        .collect::<Vec<_>>();

    let placeholder_index = encode_index(&placeholder_chunk_records, &entry_records)?;
    let blob_offset = DAT_HEADER_SIZE
        .checked_add(placeholder_index.len() as u64)
        .ok_or_else(|| FlutzError::InvalidInput("DAT size overflow".to_owned()))?;

    let mut next_offset = blob_offset;
    let mut chunk_records = Vec::with_capacity(chunk_lengths.len());
    for (chunk_id, stored_length) in chunk_lengths.iter().enumerate() {
        chunk_records.push(DatChunkRecord {
            chunk_id: chunk_id as u64,
            file_offset: next_offset,
            stored_length: *stored_length,
        });
        next_offset = next_offset
            .checked_add(*stored_length)
            .ok_or_else(|| FlutzError::InvalidInput("DAT size overflow".to_owned()))?;
    }

    let primary_index = encode_index(&chunk_records, &entry_records)?;
    let backup_index_offset = next_offset;
    let total_byte_count = backup_index_offset
        .checked_add(primary_index.len() as u64)
        .and_then(|value| value.checked_add(DAT_FOOTER_SIZE))
        .ok_or_else(|| FlutzError::InvalidInput("DAT size overflow".to_owned()))?;

    Ok(PlannedArchiveLayout {
        primary_index,
        entry_records,
        chunk_records,
        write_extents,
        backup_index_offset,
        total_byte_count,
    })
}

pub(super) fn split_asset_groups(
    assets: &[PreparedDatAsset],
    chunk_size: u64,
    max_file_size: u64,
) -> Result<Vec<Vec<AssetSlice>>> {
    if assets.is_empty() {
        return Ok(Vec::new());
    }

    let mut groups = Vec::new();
    let mut current = Vec::new();

    for asset_index in 0..assets.len() {
        let asset_len = assets[asset_index].payload.len()?;
        let mut source_offset = 0;
        let mut remaining = asset_len;

        if remaining == 0 {
            let zero_slice = AssetSlice {
                asset_index,
                source_offset: 0,
                length: 0,
            };
            let mut candidate = current.clone();
            candidate.push(zero_slice.clone());
            if plan_archive_layout(assets, &candidate, chunk_size)?.total_byte_count
                <= max_file_size
            {
                current = candidate;
            } else {
                if !current.is_empty() {
                    groups.push(current);
                }
                current = vec![zero_slice];
                ensure_group_fits(assets, &current, chunk_size, max_file_size)?;
            }
            continue;
        }

        while remaining > 0 {
            let full_slice = AssetSlice {
                asset_index,
                source_offset,
                length: remaining,
            };
            let mut candidate = current.clone();
            candidate.push(full_slice.clone());
            if plan_archive_layout(assets, &candidate, chunk_size)?.total_byte_count
                <= max_file_size
            {
                current = candidate;
                break;
            }

            match max_slice_length_that_fits(
                assets,
                &current,
                asset_index,
                source_offset,
                remaining,
                chunk_size,
                max_file_size,
            )? {
                Some(length) => {
                    current.push(AssetSlice {
                        asset_index,
                        source_offset,
                        length,
                    });
                    groups.push(current);
                    current = Vec::new();
                    source_offset += length;
                    remaining -= length;
                }
                None if current.is_empty() => {
                    return Err(FlutzError::InvalidInput(format!(
                        "DAT asset {} cannot fit any payload bytes within max DAT file size {}",
                        assets[asset_index].entry.internal_id, max_file_size
                    )));
                }
                None => {
                    groups.push(current);
                    current = Vec::new();
                }
            }
        }
    }

    if !current.is_empty() {
        groups.push(current);
    }

    Ok(groups)
}

pub(super) fn max_slice_length_that_fits(
    assets: &[PreparedDatAsset],
    current: &[AssetSlice],
    asset_index: usize,
    source_offset: u64,
    max_length: u64,
    chunk_size: u64,
    max_file_size: u64,
) -> Result<Option<u64>> {
    let mut low = 1;
    let mut high = max_length;
    let mut best = None;

    while low <= high {
        let length = low + (high - low) / 2;
        let mut candidate = current.to_vec();
        candidate.push(AssetSlice {
            asset_index,
            source_offset,
            length,
        });
        let candidate_size = plan_archive_layout(assets, &candidate, chunk_size)?.total_byte_count;

        if candidate_size <= max_file_size {
            best = Some(length);
            low = length + 1;
        } else if length == 1 {
            break;
        } else {
            high = length - 1;
        }
    }

    Ok(best)
}

pub(super) fn ensure_group_fits(
    assets: &[PreparedDatAsset],
    group: &[AssetSlice],
    chunk_size: u64,
    max_file_size: u64,
) -> Result<()> {
    let size = plan_archive_layout(assets, group, chunk_size)?.total_byte_count;
    if size > max_file_size {
        return Err(FlutzError::InvalidInput(format!(
            "DAT metadata needs {} bytes and exceeds max DAT file size {}",
            size, max_file_size
        )));
    }
    Ok(())
}

pub(super) fn part_path(output_path: &Path, part_index: usize) -> PathBuf {
    let parent = output_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = output_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("assets");
    let extension = output_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("dat");
    parent.join(format!("{stem}-{part_index:03}.{extension}"))
}
