use super::encode::{encode_footer, encode_header};
use super::layout::{plan_archive_layout, AssetSlice, PlannedWriteExtent};
use super::*;

pub(super) fn write_dat_archive_part(
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
pub(super) fn clean_output_family(output_path: &Path) -> Result<()> {
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

pub(super) fn append_bytes_to_chunks(
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
impl PreparedDatAssetPayload {
    pub(super) fn len(&self) -> Result<u64> {
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

    pub(super) fn into_bytes(self) -> Result<Vec<u8>> {
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

pub(super) fn read_error(error: io::Error) -> FlutzError {
    FlutzError::Runtime(format!("DAT read failed: {error}"))
}

pub(super) fn write_error(error: io::Error) -> FlutzError {
    FlutzError::Runtime(format!("DAT write failed: {error}"))
}
