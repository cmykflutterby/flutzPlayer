use super::*;

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

pub(super) fn extract_entry_from_index(
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

pub(super) fn checked_range_end(offset: u64, length: u64, entry_total_size: u64) -> Result<u64> {
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
