use super::chunks::write_error;
use super::*;

pub(super) const DAT_HEADER_SIZE: u64 = 4 + 8 * 7;
pub(super) const DAT_FOOTER_SIZE: u64 = 4 + 8 * 3;
pub(super) fn encode_header<W: Write>(
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

pub(super) fn encode_index(
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

pub(super) fn encode_footer<W: Write>(
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
