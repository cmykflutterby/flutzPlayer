use std::{
    collections::HashSet,
    fs,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use flutz_core::{FlutzError, Result};

use crate::{
    assets::{
        DatAssetEntry, DatChunkExtent, DatChunkRecord, DatEntryRecord, PreparedDatAsset,
        PreparedDatAssetPayload,
    },
    DAT_FOOTER_MAGIC, DAT_INDEX_MAGIC, DAT_MAGIC,
};

pub const DEFAULT_CHUNK_SIZE: u64 = 256 * 1024 * 1024;
pub const DEFAULT_DAT_FILE_SIZE: u64 = DEFAULT_CHUNK_SIZE;

const DAT_HEADER_SIZE: u64 = 4 + 8 * 7;
const DAT_FOOTER_SIZE: u64 = 4 + 8 * 3;

pub fn dat_magic_bytes() -> &'static [u8; 4] {
    DAT_MAGIC
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatArchive {
    pub bytes: Vec<u8>,
    pub entry_records: Vec<DatEntryRecord>,
    pub chunk_records: Vec<DatChunkRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatBuildReport {
    pub output_files: Vec<DatBuildFileReport>,
    pub entry_count: u64,
    pub chunk_count: u64,
    pub byte_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatBuildFileReport {
    pub output_path: PathBuf,
    pub entry_count: u64,
    pub chunk_count: u64,
    pub byte_count: u64,
}

pub fn write_dat_archive_file(
    assets: Vec<PreparedDatAsset>,
    output_path: impl AsRef<Path>,
    chunk_size: u64,
) -> Result<DatBuildReport> {
    write_dat_archive_files(assets, output_path, chunk_size, DEFAULT_DAT_FILE_SIZE)
}

pub fn write_dat_archive_files(
    assets: Vec<PreparedDatAsset>,
    output_path: impl AsRef<Path>,
    chunk_size: u64,
    max_file_size: u64,
) -> Result<DatBuildReport> {
    let output_path = output_path.as_ref();
    if max_file_size == 0 {
        return Err(FlutzError::InvalidInput(
            "DAT max file size must be greater than zero".to_owned(),
        ));
    }
    validate_assets(&assets)?;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            FlutzError::Runtime(format!(
                "failed to create DAT output directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    clean_output_family(output_path)?;

    let groups = split_asset_groups(&assets, chunk_size, max_file_size)?;
    let multi_file = groups.len() > 1;
    let mut output_files = Vec::with_capacity(groups.len());

    for (part_index, group) in groups.into_iter().enumerate() {
        let part_output_path = if multi_file {
            part_path(output_path, part_index)
        } else {
            output_path.to_owned()
        };
        output_files.push(write_dat_archive_part(
            &assets,
            &group,
            &part_output_path,
            chunk_size,
        )?);
    }

    let entry_count = output_files.iter().map(|file| file.entry_count).sum();
    let chunk_count = output_files.iter().map(|file| file.chunk_count).sum();
    let byte_count = output_files.iter().map(|file| file.byte_count).sum();

    Ok(DatBuildReport {
        output_files,
        entry_count,
        chunk_count,
        byte_count,
    })
}

fn write_dat_archive_part(
    assets: &[PreparedDatAsset],
    asset_slices: &[AssetSlice],
    output_path: &Path,
    chunk_size: u64,
) -> Result<DatBuildFileReport> {
    let layout = plan_archive_layout(assets, asset_slices, chunk_size)?;

    let file = fs::File::create(output_path).map_err(|error| {
        FlutzError::Runtime(format!(
            "failed to create DAT output {}: {error}",
            output_path.display()
        ))
    })?;
    let mut output = BufWriter::new(file);

    encode_header(
        &mut output,
        chunk_size,
        DAT_HEADER_SIZE,
        layout.primary_index.len() as u64,
        layout.entry_records.len() as u64,
        layout.chunk_records.len() as u64,
    )?;
    output
        .write_all(&layout.primary_index)
        .map_err(write_error)?;
    write_payload_extents(assets, &layout.write_extents, &mut output)?;
    output
        .write_all(&layout.primary_index)
        .map_err(write_error)?;
    encode_footer(
        &mut output,
        layout.backup_index_offset,
        layout.primary_index.len() as u64,
    )?;
    output.flush().map_err(write_error)?;

    Ok(DatBuildFileReport {
        output_path: output_path.to_owned(),
        entry_count: layout.entry_records.len() as u64,
        chunk_count: layout.chunk_records.len() as u64,
        byte_count: layout.total_byte_count,
    })
}

pub fn build_dat_archive(assets: Vec<PreparedDatAsset>, chunk_size: u64) -> Result<DatArchive> {
    if chunk_size == 0 {
        return Err(FlutzError::InvalidInput(
            "DAT chunk size must be greater than zero".to_owned(),
        ));
    }

    validate_assets(&assets)?;

    let mut chunks = Vec::<Vec<u8>>::new();
    let mut entry_records = Vec::new();

    for asset in assets {
        let bytes = asset.payload.into_bytes()?;
        let extents = append_bytes_to_chunks(&bytes, chunk_size, &mut chunks)?;
        entry_records.push(DatEntryRecord {
            entry: asset.entry,
            total_size: bytes.len() as u64,
            flags: asset.flags,
            extents,
        });
    }

    let placeholder_chunk_records = chunks
        .iter()
        .enumerate()
        .map(|(chunk_id, chunk)| DatChunkRecord {
            chunk_id: chunk_id as u64,
            file_offset: 0,
            stored_length: chunk.len() as u64,
        })
        .collect::<Vec<_>>();

    let placeholder_index = encode_index(&placeholder_chunk_records, &entry_records)?;
    let blob_offset = DAT_HEADER_SIZE
        .checked_add(placeholder_index.len() as u64)
        .ok_or_else(|| FlutzError::InvalidInput("DAT size overflow".to_owned()))?;

    let mut next_offset = blob_offset;
    let mut chunk_records = Vec::with_capacity(chunks.len());
    for (chunk_id, chunk) in chunks.iter().enumerate() {
        let stored_length = chunk.len() as u64;
        chunk_records.push(DatChunkRecord {
            chunk_id: chunk_id as u64,
            file_offset: next_offset,
            stored_length,
        });
        next_offset = next_offset
            .checked_add(stored_length)
            .ok_or_else(|| FlutzError::InvalidInput("DAT size overflow".to_owned()))?;
    }

    let primary_index = encode_index(&chunk_records, &entry_records)?;
    let primary_index_offset = DAT_HEADER_SIZE;
    let primary_index_length = primary_index.len() as u64;
    let backup_index_offset = next_offset;
    let backup_index_length = primary_index_length;

    let mut bytes = Vec::new();
    encode_header(
        &mut bytes,
        chunk_size,
        primary_index_offset,
        primary_index_length,
        entry_records.len() as u64,
        chunk_records.len() as u64,
    )?;
    bytes.extend_from_slice(&primary_index);
    for chunk in &chunks {
        bytes.extend_from_slice(chunk);
    }
    bytes.extend_from_slice(&primary_index);
    encode_footer(&mut bytes, backup_index_offset, backup_index_length)?;

    Ok(DatArchive {
        bytes,
        entry_records,
        chunk_records,
    })
}

struct PlannedArchiveLayout {
    primary_index: Vec<u8>,
    entry_records: Vec<DatEntryRecord>,
    chunk_records: Vec<DatChunkRecord>,
    write_extents: Vec<PlannedWriteExtent>,
    backup_index_offset: u64,
    total_byte_count: u64,
}

struct PlannedWriteExtent {
    asset_index: usize,
    source_offset: u64,
    length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssetSlice {
    asset_index: usize,
    source_offset: u64,
    length: u64,
}

fn plan_archive_layout(
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

fn split_asset_groups(
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

fn max_slice_length_that_fits(
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

fn ensure_group_fits(
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

fn clean_output_family(output_path: &Path) -> Result<()> {
    if output_path.exists() {
        fs::remove_file(output_path).map_err(|error| {
            FlutzError::Runtime(format!(
                "failed to remove stale DAT output {}: {error}",
                output_path.display()
            ))
        })?;
    }

    let Some(parent) = output_path.parent() else {
        return Ok(());
    };
    let Some(stem) = output_path.file_stem().and_then(|value| value.to_str()) else {
        return Ok(());
    };
    let extension = output_path.extension().and_then(|value| value.to_str());
    let prefix = format!("{stem}-");

    for entry in fs::read_dir(parent).map_err(|error| {
        FlutzError::Runtime(format!(
            "failed to scan DAT output directory {}: {error}",
            parent.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            FlutzError::Runtime(format!(
                "failed to inspect DAT output directory {}: {error}",
                parent.display()
            ))
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let matches_extension = path.extension().and_then(|value| value.to_str()) == extension;
        let matches_stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(|value| value.starts_with(&prefix))
            .unwrap_or(false);
        if matches_extension && matches_stem {
            fs::remove_file(&path).map_err(|error| {
                FlutzError::Runtime(format!(
                    "failed to remove stale DAT output {}: {error}",
                    path.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn part_path(output_path: &Path, part_index: usize) -> PathBuf {
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

fn append_bytes_to_chunks(
    bytes: &[u8],
    chunk_size: u64,
    chunks: &mut Vec<Vec<u8>>,
) -> Result<Vec<DatChunkExtent>> {
    let mut extents = Vec::new();
    let total_len = bytes.len() as u64;

    if chunk_size == 0 {
        return Err(FlutzError::InvalidInput(
            "DAT chunk size must be greater than zero".to_owned(),
        ));
    }

    if total_len <= chunk_size {
        let needs_new_chunk = chunks
            .last()
            .map(|chunk| chunk_size.saturating_sub(chunk.len() as u64) < total_len)
            .unwrap_or(true);
        if needs_new_chunk {
            chunks.push(Vec::new());
        }

        let chunk_id = chunks.len() - 1;
        let offset_in_chunk = chunks[chunk_id].len() as u64;
        chunks[chunk_id].extend_from_slice(bytes);
        extents.push(DatChunkExtent {
            chunk_id: chunk_id as u64,
            offset_in_chunk,
            length: total_len,
        });
        return Ok(extents);
    }

    let mut cursor = 0usize;
    while cursor < bytes.len() {
        chunks.push(Vec::new());
        let chunk_id = chunks.len() - 1;
        let remaining = bytes.len() - cursor;
        let take = remaining.min(chunk_size as usize);
        chunks[chunk_id].extend_from_slice(&bytes[cursor..cursor + take]);
        extents.push(DatChunkExtent {
            chunk_id: chunk_id as u64,
            offset_in_chunk: 0,
            length: take as u64,
        });
        cursor += take;
    }

    Ok(extents)
}

fn write_payload_extents<W: Write>(
    assets: &[PreparedDatAsset],
    extents: &[PlannedWriteExtent],
    output: &mut W,
) -> Result<()> {
    for extent in extents {
        let payload = &assets[extent.asset_index].payload;
        match payload {
            PreparedDatAssetPayload::Bytes(bytes) => {
                let start = extent.source_offset as usize;
                let end = start.checked_add(extent.length as usize).ok_or_else(|| {
                    FlutzError::InvalidInput("DAT payload slice overflow".to_owned())
                })?;
                let slice = bytes.get(start..end).ok_or_else(|| {
                    FlutzError::InvalidInput("DAT payload slice out of range".to_owned())
                })?;
                output.write_all(slice).map_err(write_error)?;
            }
            PreparedDatAssetPayload::File(path) => {
                let mut input = fs::File::open(path).map_err(|error| {
                    FlutzError::Runtime(format!(
                        "failed to open DAT asset source {}: {error}",
                        path.display()
                    ))
                })?;
                input
                    .seek(SeekFrom::Start(extent.source_offset))
                    .map_err(read_error)?;
                let copied =
                    io::copy(&mut input.take(extent.length), output).map_err(read_error)?;
                if copied != extent.length {
                    return Err(FlutzError::Runtime(format!(
                        "short read while writing DAT asset {}: expected {}, copied {} bytes",
                        path.display(),
                        extent.length,
                        copied
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_entry(entry: &DatAssetEntry) -> Result<()> {
    for (name, value) in [
        ("internal_id", &entry.internal_id),
        ("display_name", &entry.display_name),
        ("asset_type", &entry.asset_type),
        ("source_format", &entry.source_format),
        ("storage_format", &entry.storage_format),
        ("runtime_format", &entry.runtime_format),
        ("original_filename", &entry.original_filename),
    ] {
        if value.trim().is_empty() {
            return Err(FlutzError::InvalidInput(format!(
                "DAT asset {name} must not be empty"
            )));
        }
    }
    Ok(())
}

fn validate_assets(assets: &[PreparedDatAsset]) -> Result<()> {
    let mut seen_ids = HashSet::new();
    for asset in assets {
        validate_entry(&asset.entry)?;
        if !seen_ids.insert(asset.entry.internal_id.clone()) {
            return Err(FlutzError::InvalidInput(format!(
                "duplicate DAT internal ID: {}",
                asset.entry.internal_id
            )));
        }
    }
    Ok(())
}

fn encode_header<W: Write>(
    output: &mut W,
    chunk_size: u64,
    primary_index_offset: u64,
    primary_index_length: u64,
    entry_count: u64,
    chunk_count: u64,
) -> Result<()> {
    output.write_all(DAT_MAGIC).map_err(write_error)?;
    push_u64(output, DAT_HEADER_SIZE)?;
    push_u64(output, 0)?;
    push_u64(output, chunk_size)?;
    push_u64(output, primary_index_offset)?;
    push_u64(output, primary_index_length)?;
    push_u64(output, entry_count)?;
    push_u64(output, chunk_count)?;
    Ok(())
}

fn encode_index(
    chunk_records: &[DatChunkRecord],
    entry_records: &[DatEntryRecord],
) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    push_u64(&mut body, 0)?;
    push_u64(&mut body, entry_records.len() as u64)?;
    push_u64(&mut body, chunk_records.len() as u64)?;

    for chunk in chunk_records {
        push_u64(&mut body, chunk.chunk_id)?;
        push_u64(&mut body, chunk.file_offset)?;
        push_u64(&mut body, chunk.stored_length)?;
    }

    for entry in entry_records {
        push_string(&mut body, &entry.entry.internal_id)?;
        push_string(&mut body, &entry.entry.display_name)?;
        push_string(&mut body, &entry.entry.asset_type)?;
        push_string(&mut body, &entry.entry.source_format)?;
        push_string(&mut body, &entry.entry.storage_format)?;
        push_string(&mut body, &entry.entry.runtime_format)?;
        push_string(&mut body, &entry.entry.original_filename)?;
        push_u64(&mut body, entry.total_size)?;
        push_u64(&mut body, entry.flags)?;
        push_u64(&mut body, entry.extents.len() as u64)?;
        for extent in &entry.extents {
            push_u64(&mut body, extent.chunk_id)?;
            push_u64(&mut body, extent.offset_in_chunk)?;
            push_u64(&mut body, extent.length)?;
        }
    }

    let index_length = 4u64
        .checked_add(8)
        .and_then(|value| value.checked_add(body.len() as u64))
        .ok_or_else(|| FlutzError::InvalidInput("DAT index size overflow".to_owned()))?;

    let mut output = Vec::new();
    output.write_all(DAT_INDEX_MAGIC).map_err(write_error)?;
    push_u64(&mut output, index_length)?;
    output.extend_from_slice(&body);
    Ok(output)
}

fn encode_footer<W: Write>(
    output: &mut W,
    backup_index_offset: u64,
    backup_index_length: u64,
) -> Result<()> {
    output.write_all(DAT_FOOTER_MAGIC).map_err(write_error)?;
    push_u64(output, backup_index_offset)?;
    push_u64(output, backup_index_length)?;
    push_u64(output, DAT_FOOTER_SIZE)?;
    Ok(())
}

fn push_string<W: Write>(output: &mut W, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    push_u64(output, bytes.len() as u64)?;
    output.write_all(bytes).map_err(write_error)
}

fn push_u64<W: Write>(output: &mut W, value: u64) -> Result<()> {
    output.write_all(&value.to_le_bytes()).map_err(write_error)
}

impl PreparedDatAssetPayload {
    fn len(&self) -> Result<u64> {
        match self {
            Self::Bytes(bytes) => Ok(bytes.len() as u64),
            Self::File(path) => {
                fs::metadata(path)
                    .map(|metadata| metadata.len())
                    .map_err(|error| {
                        FlutzError::Runtime(format!(
                            "failed to read DAT asset metadata {}: {error}",
                            path.display()
                        ))
                    })
            }
        }
    }

    fn into_bytes(self) -> Result<Vec<u8>> {
        match self {
            Self::Bytes(bytes) => Ok(bytes),
            Self::File(path) => fs::read(&path).map_err(|error| {
                FlutzError::Runtime(format!(
                    "failed to read DAT asset source {}: {error}",
                    path.display()
                ))
            }),
        }
    }
}

fn read_error(error: io::Error) -> FlutzError {
    FlutzError::Runtime(format!("DAT read failed: {error}"))
}

fn write_error(error: io::Error) -> FlutzError {
    FlutzError::Runtime(format!("DAT write failed: {error}"))
}
