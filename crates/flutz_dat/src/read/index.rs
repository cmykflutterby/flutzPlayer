use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatArchiveIndex {
    pub chunk_size: u64,
    pub entries: Vec<DatEntryRecord>,
    pub chunks: Vec<DatChunkRecord>,
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

pub(super) struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(super) fn take_bytes(&mut self, count: usize) -> Result<&'a [u8]> {
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

    pub(super) fn take_u64(&mut self) -> Result<u64> {
        let bytes = self.take_bytes(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
            FlutzError::InvalidInput("truncated DAT u64 field".to_owned())
        })?))
    }

    pub(super) fn take_string(&mut self) -> Result<String> {
        let length = self.take_u64()?;
        let bytes = self.take_bytes(length as usize)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| FlutzError::InvalidInput(format!("invalid DAT UTF-8 string: {error}")))
    }
}
