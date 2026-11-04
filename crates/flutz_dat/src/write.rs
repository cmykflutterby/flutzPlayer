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

mod chunks;
mod encode;
mod layout;
mod validate;

use chunks::{append_bytes_to_chunks, clean_output_family, write_dat_archive_part};
use encode::{encode_footer, encode_header, encode_index, DAT_HEADER_SIZE};
use layout::{part_path, split_asset_groups};
use validate::validate_assets;

pub const DEFAULT_CHUNK_SIZE: u64 = 256 * 1024 * 1024;
pub const DEFAULT_DAT_FILE_SIZE: u64 = DEFAULT_CHUNK_SIZE;

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
