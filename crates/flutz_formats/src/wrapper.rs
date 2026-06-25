use flutz_core::{FlutzError, Result};
use flutz_peq::{deserialize_preset_toml, serialize_preset_toml, PeqPresetFile};

use crate::model::{LoopMode, LoopUnit, MediaLoop, MetadataField, NativeMetadata, TrackMetadata};

const MAGIC: &[u8; 4] = b"FWRP";
const TABLE_MAGIC: &[u8; 4] = b"FTBL";
const HEADER_SIZE: usize = 4 + 8 * 6;
const TABLE_HEADER_SIZE: usize = 4 + 8 * 3;
const RECORD_SIZE: usize = 4 + 8 * 4;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct WrapperChunkId(pub [u8; 4]);

impl WrapperChunkId {
    pub const SOURCE_AUDIO: Self = Self(*b"AUDO");
    pub const METADATA: Self = Self(*b"META");
    pub const LOOP: Self = Self(*b"LOOP");
    pub const NATIVE_METADATA: Self = Self(*b"NMDT");
    pub const PEQ: Self = Self(*b"PEQ ");

    pub fn is_known(id: [u8; 4]) -> bool {
        id == Self::SOURCE_AUDIO.0
            || id == Self::METADATA.0
            || id == Self::LOOP.0
            || id == Self::NATIVE_METADATA.0
            || id == Self::PEQ.0
    }

    pub fn is_required(id: [u8; 4]) -> bool {
        id == Self::SOURCE_AUDIO.0 || id == Self::METADATA.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceAudioBlock {
    pub format_id: String,
    pub original_filename: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnknownBlock {
    pub chunk_id: [u8; 4],
    pub flags: u64,
    pub ordinal: u64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlutzAudioWrapper {
    pub header_flags: u64,
    pub chunk_table_flags: u64,
    pub source: SourceAudioBlock,
    pub metadata: TrackMetadata,
    pub loop_region: Option<MediaLoop>,
    pub native_metadata: NativeMetadata,
    pub peq: Option<PeqPresetFile>,
    pub unknown_blocks: Vec<UnknownBlock>,
}

impl Default for FlutzAudioWrapper {
    fn default() -> Self {
        Self {
            header_flags: 0,
            chunk_table_flags: 0,
            source: SourceAudioBlock::default(),
            metadata: TrackMetadata::default(),
            loop_region: None,
            native_metadata: Vec::new(),
            peq: None,
            unknown_blocks: Vec::new(),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct ChunkRecord {
    chunk_id: [u8; 4],
    offset: u64,
    length: u64,
    flags: u64,
    ordinal: u64,
}

pub fn write_flutz_wrapper(file: &FlutzAudioWrapper) -> Result<Vec<u8>> {
    if file.source.bytes.is_empty() {
        return Err(FlutzError::InvalidInput(
            "wrapper source audio payload is empty".to_owned(),
        ));
    }

    let mut payloads = vec![
        (
            WrapperChunkId::SOURCE_AUDIO.0,
            0u64,
            write_source(&file.source),
            0u64,
        ),
        (
            WrapperChunkId::METADATA.0,
            0u64,
            write_metadata(&file.metadata),
            1u64,
        ),
    ];
    if let Some(loop_region) = file.loop_region {
        payloads.push((WrapperChunkId::LOOP.0, 0, write_loop(loop_region), 2));
    }
    if !file.native_metadata.is_empty() {
        payloads.push((
            WrapperChunkId::NATIVE_METADATA.0,
            0,
            write_native_metadata(&file.native_metadata),
            3,
        ));
    }
    if let Some(peq) = &file.peq {
        let text = serialize_preset_toml(peq)
            .map_err(|error| FlutzError::InvalidInput(error.to_string()))?;
        payloads.push((WrapperChunkId::PEQ.0, 0, write_utf8(&text), 4));
    }

    let mut unknown = file.unknown_blocks.clone();
    unknown.sort_by_key(|chunk| chunk.ordinal);
    let base_ordinal = payloads.len() as u64;
    for (index, chunk) in unknown.into_iter().enumerate() {
        payloads.push((
            chunk.chunk_id,
            chunk.flags,
            chunk.payload,
            base_ordinal + index as u64,
        ));
    }

    let chunk_count = payloads.len();
    let table_length = TABLE_HEADER_SIZE + chunk_count * RECORD_SIZE;
    let payload_start = HEADER_SIZE + table_length;
    let mut payload_cursor = payload_start as u64;
    let mut records = Vec::with_capacity(chunk_count);
    for (chunk_id, flags, payload, ordinal) in &payloads {
        records.push(ChunkRecord {
            chunk_id: *chunk_id,
            offset: payload_cursor,
            length: payload.len() as u64,
            flags: *flags,
            ordinal: *ordinal,
        });
        payload_cursor += payload.len() as u64;
    }

    let file_size = payload_cursor as usize;
    let mut out = Vec::with_capacity(file_size);
    write_exact_4(&mut out, *MAGIC);
    write_u64(&mut out, HEADER_SIZE as u64);
    write_u64(&mut out, file.header_flags);
    write_u64(&mut out, file_size as u64);
    write_u64(&mut out, HEADER_SIZE as u64);
    write_u64(&mut out, table_length as u64);
    write_u64(&mut out, chunk_count as u64);

    write_exact_4(&mut out, *TABLE_MAGIC);
    write_u64(&mut out, table_length as u64);
    write_u64(&mut out, file.chunk_table_flags);
    write_u64(&mut out, chunk_count as u64);
    for record in &records {
        write_exact_4(&mut out, record.chunk_id);
        write_u64(&mut out, record.offset);
        write_u64(&mut out, record.length);
        write_u64(&mut out, record.flags);
        write_u64(&mut out, record.ordinal);
    }

    for (_chunk_id, _flags, payload, _ordinal) in payloads {
        out.extend_from_slice(&payload);
    }
    Ok(out)
}

pub fn read_flutz_wrapper(bytes: &[u8]) -> Result<FlutzAudioWrapper> {
    if !bytes.starts_with(MAGIC) {
        return Err(FlutzError::InvalidInput(
            "missing flutz wrapper magic".to_owned(),
        ));
    }
    if bytes.len() < HEADER_SIZE {
        return Err(FlutzError::InvalidInput(
            "flutz wrapper header is truncated".to_owned(),
        ));
    }

    let mut cursor = Cursor::new(bytes);
    let _magic = read_exact_4(&mut cursor)?;
    let header_size = read_u64(&mut cursor)? as usize;
    let header_flags = read_u64(&mut cursor)?;
    let file_size = read_u64(&mut cursor)? as usize;
    let table_offset = read_u64(&mut cursor)? as usize;
    let table_length = read_u64(&mut cursor)? as usize;
    let chunk_count = read_u64(&mut cursor)? as usize;

    if header_size < HEADER_SIZE || header_size > bytes.len() || file_size != bytes.len() {
        return Err(FlutzError::InvalidInput(
            "invalid flutz wrapper header range".to_owned(),
        ));
    }
    if table_offset.saturating_add(table_length) > bytes.len() {
        return Err(FlutzError::InvalidInput(
            "flutz wrapper table is out of bounds".to_owned(),
        ));
    }
    let (chunk_table_flags, records) = read_chunk_table(
        &bytes[table_offset..table_offset + table_length],
        chunk_count,
    )?;

    let mut source = None;
    let mut metadata = None;
    let mut loop_region = None;
    let mut native_metadata = Vec::new();
    let mut peq = None;
    let mut unknown_blocks = Vec::new();

    for record in &records {
        let start = record.offset as usize;
        let end = start.saturating_add(record.length as usize);
        if end > bytes.len() {
            return Err(FlutzError::InvalidInput(format!(
                "wrapper chunk {:?} is out of bounds",
                record.chunk_id
            )));
        }
        let payload = &bytes[start..end];
        match record.chunk_id {
            id if id == WrapperChunkId::SOURCE_AUDIO.0 => source = Some(read_source(payload)?),
            id if id == WrapperChunkId::METADATA.0 => metadata = Some(read_metadata(payload)?),
            id if id == WrapperChunkId::LOOP.0 => loop_region = Some(read_loop(payload)?),
            id if id == WrapperChunkId::NATIVE_METADATA.0 => {
                native_metadata = read_native_metadata(payload)?;
            }
            id if id == WrapperChunkId::PEQ.0 => {
                let text = read_utf8(payload)?;
                peq = Some(
                    deserialize_preset_toml(&text)
                        .map_err(|error| FlutzError::InvalidInput(error.to_string()))?,
                );
            }
            id => unknown_blocks.push(UnknownBlock {
                chunk_id: id,
                flags: record.flags,
                ordinal: record.ordinal,
                payload: payload.to_vec(),
            }),
        }
    }

    for required in [WrapperChunkId::SOURCE_AUDIO.0, WrapperChunkId::METADATA.0] {
        if !records.iter().any(|record| record.chunk_id == required) {
            return Err(FlutzError::InvalidInput(format!(
                "required wrapper chunk {:?} is missing",
                required
            )));
        }
    }

    Ok(FlutzAudioWrapper {
        header_flags,
        chunk_table_flags,
        source: source.unwrap_or_default(),
        metadata: metadata.unwrap_or_default(),
        loop_region,
        native_metadata,
        peq,
        unknown_blocks,
    })
}

fn read_chunk_table(bytes: &[u8], header_chunk_count: usize) -> Result<(u64, Vec<ChunkRecord>)> {
    if bytes.len() < TABLE_HEADER_SIZE {
        return Err(FlutzError::InvalidInput(
            "wrapper chunk table is truncated".to_owned(),
        ));
    }
    let mut cursor = Cursor::new(bytes);
    let magic = read_exact_4(&mut cursor)?;
    if &magic != TABLE_MAGIC {
        return Err(FlutzError::InvalidInput(
            "missing wrapper chunk table magic".to_owned(),
        ));
    }
    let table_length = read_u64(&mut cursor)? as usize;
    let flags = read_u64(&mut cursor)?;
    let chunk_count = read_u64(&mut cursor)? as usize;
    if table_length != bytes.len() || chunk_count != header_chunk_count {
        return Err(FlutzError::InvalidInput(
            "wrapper chunk table length/count mismatch".to_owned(),
        ));
    }
    if TABLE_HEADER_SIZE + chunk_count.saturating_mul(RECORD_SIZE) != bytes.len() {
        return Err(FlutzError::InvalidInput(
            "wrapper chunk table record length mismatch".to_owned(),
        ));
    }
    let mut records = Vec::with_capacity(chunk_count);
    for _ in 0..chunk_count {
        records.push(ChunkRecord {
            chunk_id: read_exact_4(&mut cursor)?,
            offset: read_u64(&mut cursor)?,
            length: read_u64(&mut cursor)?,
            flags: read_u64(&mut cursor)?,
            ordinal: read_u64(&mut cursor)?,
        });
    }
    Ok((flags, records))
}

fn write_source(source: &SourceAudioBlock) -> Vec<u8> {
    let mut out = Vec::new();
    write_utf8_into(&mut out, &source.format_id);
    write_utf8_into(&mut out, &source.original_filename);
    write_utf8_into(&mut out, &source.media_type);
    write_blob_into(&mut out, &source.bytes);
    out
}

fn read_source(bytes: &[u8]) -> Result<SourceAudioBlock> {
    let mut cursor = Cursor::new(bytes);
    Ok(SourceAudioBlock {
        format_id: read_utf8_from(&mut cursor)?,
        original_filename: read_utf8_from(&mut cursor)?,
        media_type: read_utf8_from(&mut cursor)?,
        bytes: read_blob_from(&mut cursor)?,
    })
}

fn write_metadata(metadata: &TrackMetadata) -> Vec<u8> {
    let mut out = Vec::new();
    write_utf8_into(&mut out, &metadata.project_name);
    write_utf8_into(&mut out, &metadata.source_filename);
    write_utf8_into(&mut out, &metadata.artist);
    write_utf8_into(&mut out, &metadata.album);
    write_utf8_into(&mut out, &metadata.album_artist);
    write_utf8_into(&mut out, &metadata.composer);
    write_utf8_into(&mut out, &metadata.conductor);
    write_utf8_into(&mut out, &metadata.genre);
    write_utf8_into(&mut out, &metadata.date);
    write_utf8_into(&mut out, &metadata.track_number);
    write_utf8_into(&mut out, &metadata.track_total);
    write_utf8_into(&mut out, &metadata.disc_number);
    write_utf8_into(&mut out, &metadata.disc_total);
    write_utf8_into(&mut out, &metadata.description);
    write_utf8_into(&mut out, &metadata.copyright);
    write_utf8_into(&mut out, &metadata.publisher);
    write_utf8_into(&mut out, &metadata.encoded_by);
    write_utf8_into(&mut out, &metadata.encoder);
    write_utf8_into(&mut out, &metadata.language);
    write_utf8_into(&mut out, &metadata.lyrics);
    write_utf8_into(&mut out, &metadata.url);
    write_utf8_into(&mut out, &metadata.notes);
    write_u64(&mut out, metadata.extra_fields.len() as u64);
    for field in &metadata.extra_fields {
        write_utf8_into(&mut out, &field.key);
        write_utf8_into(&mut out, &field.value);
    }
    out
}

fn read_metadata(bytes: &[u8]) -> Result<TrackMetadata> {
    let mut cursor = Cursor::new(bytes);
    let project_name = read_utf8_from(&mut cursor)?;
    let source_filename = read_utf8_from(&mut cursor)?;
    let legacy_offset = cursor.offset;
    let new_format = (|| -> Result<TrackMetadata> {
        let artist = read_utf8_from(&mut cursor)?;
        let album = read_utf8_from(&mut cursor)?;
        let album_artist = read_utf8_from(&mut cursor)?;
        let composer = read_utf8_from(&mut cursor)?;
        let conductor = read_utf8_from(&mut cursor)?;
        let genre = read_utf8_from(&mut cursor)?;
        let date = read_utf8_from(&mut cursor)?;
        let track_number = read_utf8_from(&mut cursor)?;
        let track_total = read_utf8_from(&mut cursor)?;
        let disc_number = read_utf8_from(&mut cursor)?;
        let disc_total = read_utf8_from(&mut cursor)?;
        let description = read_utf8_from(&mut cursor)?;
        let copyright = read_utf8_from(&mut cursor)?;
        let publisher = read_utf8_from(&mut cursor)?;
        let encoded_by = read_utf8_from(&mut cursor)?;
        let encoder = read_utf8_from(&mut cursor)?;
        let language = read_utf8_from(&mut cursor)?;
        let lyrics = read_utf8_from(&mut cursor)?;
        let url = read_utf8_from(&mut cursor)?;
        let notes = read_utf8_from(&mut cursor)?;
        let count = read_u64(&mut cursor)? as usize;
        let mut extra_fields = Vec::with_capacity(count);
        for _ in 0..count {
            extra_fields.push(MetadataField {
                key: read_utf8_from(&mut cursor)?,
                value: read_utf8_from(&mut cursor)?,
            });
        }
        if cursor.offset != bytes.len() {
            return Err(FlutzError::InvalidInput(
                "wrapper metadata payload has trailing bytes".to_owned(),
            ));
        }
        Ok(TrackMetadata {
            project_name: project_name.clone(),
            source_filename: source_filename.clone(),
            artist,
            album,
            album_artist,
            composer,
            conductor,
            genre,
            date,
            track_number,
            track_total,
            disc_number,
            disc_total,
            description,
            copyright,
            publisher,
            encoded_by,
            encoder,
            language,
            lyrics,
            url,
            notes,
            extra_fields,
        })
    })();

    match new_format {
        Ok(metadata) => Ok(metadata),
        Err(_) => Ok(TrackMetadata {
            project_name,
            source_filename,
            notes: read_utf8_from(&mut Cursor {
                bytes,
                offset: legacy_offset,
            })?,
            ..TrackMetadata::default()
        }),
    }
}

fn write_loop(loop_region: MediaLoop) -> Vec<u8> {
    let mut out = Vec::new();
    write_bool(&mut out, loop_region.enabled);
    write_u64(&mut out, loop_region.mode.to_u64());
    match loop_region.unit {
        LoopUnit::MidiTicks { start, end } => {
            write_u64(&mut out, 0);
            write_u64(&mut out, start);
            write_u64(&mut out, end);
        }
        LoopUnit::SampleFrames { start, end } => {
            write_u64(&mut out, 1);
            write_u64(&mut out, start);
            write_u64(&mut out, end);
        }
        LoopUnit::Seconds { start, end } => {
            write_u64(&mut out, 2);
            write_f64(&mut out, start);
            write_f64(&mut out, end);
        }
    }
    write_u64(&mut out, loop_region.loop_count);
    out
}

fn read_loop(bytes: &[u8]) -> Result<MediaLoop> {
    let mut cursor = Cursor::new(bytes);
    let enabled = read_bool(&mut cursor)?;
    let mode = LoopMode::from_u64(read_u64(&mut cursor)?)
        .ok_or_else(|| FlutzError::InvalidInput("unknown loop mode".to_owned()))?;
    let unit = match read_u64(&mut cursor)? {
        0 => LoopUnit::MidiTicks {
            start: read_u64(&mut cursor)?,
            end: read_u64(&mut cursor)?,
        },
        1 => LoopUnit::SampleFrames {
            start: read_u64(&mut cursor)?,
            end: read_u64(&mut cursor)?,
        },
        2 => LoopUnit::Seconds {
            start: read_f64(&mut cursor)?,
            end: read_f64(&mut cursor)?,
        },
        _ => return Err(FlutzError::InvalidInput("unknown loop unit".to_owned())),
    };
    Ok(MediaLoop {
        enabled,
        mode,
        unit,
        loop_count: read_u64(&mut cursor)?,
    })
}

fn write_native_metadata(fields: &[MetadataField]) -> Vec<u8> {
    let mut out = Vec::new();
    write_u64(&mut out, fields.len() as u64);
    for field in fields {
        write_utf8_into(&mut out, &field.key);
        write_utf8_into(&mut out, &field.value);
    }
    out
}

fn read_native_metadata(bytes: &[u8]) -> Result<Vec<MetadataField>> {
    let mut cursor = Cursor::new(bytes);
    let count = read_u64(&mut cursor)? as usize;
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        fields.push(MetadataField {
            key: read_utf8_from(&mut cursor)?,
            value: read_utf8_from(&mut cursor)?,
        });
    }
    Ok(fields)
}

fn write_utf8(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    write_utf8_into(&mut out, text);
    out
}

fn write_utf8_into(out: &mut Vec<u8>, text: &str) {
    write_u64(out, text.len() as u64);
    out.extend_from_slice(text.as_bytes());
}

fn write_blob_into(out: &mut Vec<u8>, bytes: &[u8]) {
    write_u64(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn write_bool(out: &mut Vec<u8>, value: bool) {
    out.push(if value { 1 } else { 0 });
}

fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_f64(out: &mut Vec<u8>, value: f64) {
    write_u64(out, value.to_bits());
}

fn write_exact_4(out: &mut Vec<u8>, value: [u8; 4]) {
    out.extend_from_slice(&value);
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.offset.saturating_add(len);
        if end > self.bytes.len() {
            return Err(FlutzError::InvalidInput(
                "wrapper payload is truncated".to_owned(),
            ));
        }
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }
}

fn read_exact_4(cursor: &mut Cursor<'_>) -> Result<[u8; 4]> {
    let bytes = cursor.read(4)?;
    Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_u64(cursor: &mut Cursor<'_>) -> Result<u64> {
    let bytes = cursor.read(8)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn read_f64(cursor: &mut Cursor<'_>) -> Result<f64> {
    Ok(f64::from_bits(read_u64(cursor)?))
}

fn read_bool(cursor: &mut Cursor<'_>) -> Result<bool> {
    match cursor.read(1)?[0] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(FlutzError::InvalidInput("invalid bool value".to_owned())),
    }
}

fn read_utf8(payload: &[u8]) -> Result<String> {
    read_utf8_from(&mut Cursor::new(payload))
}

fn read_utf8_from(cursor: &mut Cursor<'_>) -> Result<String> {
    let len = read_u64(cursor)? as usize;
    let bytes = cursor.read(len)?;
    String::from_utf8(bytes.to_vec())
        .map_err(|error| FlutzError::InvalidInput(format!("invalid UTF-8 field: {error}")))
}

fn read_blob_from(cursor: &mut Cursor<'_>) -> Result<Vec<u8>> {
    let len = read_u64(cursor)? as usize;
    Ok(cursor.read(len)?.to_vec())
}
