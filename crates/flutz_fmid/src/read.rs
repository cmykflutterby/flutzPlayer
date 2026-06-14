use flutz_core::{FlutzError, Result};

use crate::{
    chunks::ChunkId,
    records::{
        FmidChunkRecord, FmidFile, LoopMode, LoopRecord, MasterMixerRecord, MixerRecord,
        MixerSourceMode, MixerStripControls, MixerStripIdentity, MixerStripRecord, ProjectRecord,
        SmartMixRecord, SoundFontRowMuteRecord, SoundFontSlot, UnknownChunk,
    },
    FMID_CHUNK_TABLE_MAGIC, FMID_MAGIC,
};

const HEADER_SIZE: usize = 4 + 8 * 6;
const CHUNK_TABLE_HEADER_SIZE: usize = 4 + 8 * 3;
const CHUNK_RECORD_SIZE: usize = 4 + 8 * 4;

pub fn validate_magic(bytes: &[u8]) -> Result<()> {
    if bytes.starts_with(FMID_MAGIC) {
        Ok(())
    } else {
        Err(FlutzError::InvalidInput("missing FMID magic".to_owned()))
    }
}

pub fn read_fmid(bytes: &[u8]) -> Result<FmidFile> {
    validate_magic(bytes)?;
    if bytes.len() < HEADER_SIZE {
        return Err(FlutzError::InvalidInput(
            "FMID header is truncated".to_owned(),
        ));
    }

    let mut cursor = Cursor::new(bytes);
    let _magic = read_exact_4(&mut cursor)?;
    let header_size = read_u64(&mut cursor)? as usize;
    let header_flags = read_u64(&mut cursor)?;
    let file_size = read_u64(&mut cursor)? as usize;
    let chunk_table_offset = read_u64(&mut cursor)? as usize;
    let chunk_table_length = read_u64(&mut cursor)? as usize;
    let chunk_count = read_u64(&mut cursor)? as usize;

    if header_size < HEADER_SIZE || header_size > bytes.len() {
        return Err(FlutzError::InvalidInput(
            "invalid FMID header_size".to_owned(),
        ));
    }
    if file_size != bytes.len() {
        return Err(FlutzError::InvalidInput(
            "FMID header file_size does not match actual size".to_owned(),
        ));
    }
    if chunk_table_offset.saturating_add(chunk_table_length) > bytes.len() {
        return Err(FlutzError::InvalidInput(
            "FMID chunk table range is out of bounds".to_owned(),
        ));
    }

    let chunk_table = &bytes[chunk_table_offset..chunk_table_offset + chunk_table_length];
    let (chunk_table_flags, chunk_records) = read_chunk_table(chunk_table, chunk_count)?;

    let mut midi = None::<Vec<u8>>;
    let mut proj = None::<ProjectRecord>;
    let mut font = None::<Vec<SoundFontSlot>>;
    let mut mixr = None::<MixerRecord>;
    let mut loop_chunk = None::<LoopRecord>;
    let mut smix = None::<SmartMixRecord>;
    let mut msrc = None::<MixerSourceMode>;
    let mut note = None::<String>;
    let mut unknown_chunks = Vec::new();

    for chunk in &chunk_records {
        let payload_start = chunk.offset as usize;
        let payload_end = payload_start.saturating_add(chunk.length as usize);
        if payload_end > bytes.len() {
            return Err(FlutzError::InvalidInput(format!(
                "FMID chunk {:?} is out of bounds",
                chunk.chunk_id
            )));
        }

        let payload = &bytes[payload_start..payload_end];
        match chunk.chunk_id {
            id if id == ChunkId::MIDI.0 => {
                if midi.is_some() {
                    return Err(FlutzError::InvalidInput("duplicate MIDI chunk".to_owned()));
                }
                midi = Some(read_blob(payload)?);
            }
            id if id == ChunkId::PROJ.0 => {
                if proj.is_some() {
                    return Err(FlutzError::InvalidInput("duplicate PROJ chunk".to_owned()));
                }
                proj = Some(read_project(payload)?);
            }
            id if id == ChunkId::FONT.0 => {
                if font.is_some() {
                    return Err(FlutzError::InvalidInput("duplicate FONT chunk".to_owned()));
                }
                font = Some(read_soundfont_slots(payload)?);
            }
            id if id == ChunkId::MIXR.0 => {
                if mixr.is_some() {
                    return Err(FlutzError::InvalidInput("duplicate MIXR chunk".to_owned()));
                }
                mixr = Some(read_mixer(payload)?);
            }
            id if id == ChunkId::LOOP.0 => {
                if loop_chunk.is_some() {
                    return Err(FlutzError::InvalidInput("duplicate LOOP chunk".to_owned()));
                }
                loop_chunk = Some(read_loop(payload)?);
            }
            id if id == ChunkId::SMIX.0 => {
                if smix.is_some() {
                    return Err(FlutzError::InvalidInput("duplicate SMIX chunk".to_owned()));
                }
                smix = Some(read_smart_mix(payload)?);
            }
            id if id == ChunkId::MSRC.0 => {
                if msrc.is_some() {
                    return Err(FlutzError::InvalidInput("duplicate MSRC chunk".to_owned()));
                }
                msrc = Some(read_mixer_source(payload)?);
            }
            id if id == ChunkId::NOTE.0 => {
                if note.is_some() {
                    return Err(FlutzError::InvalidInput("duplicate NOTE chunk".to_owned()));
                }
                note = Some(read_utf8(payload)?);
            }
            id => unknown_chunks.push(UnknownChunk {
                chunk_id: id,
                flags: chunk.flags,
                ordinal: chunk.ordinal,
                payload: payload.to_vec(),
            }),
        }
    }

    for required in [
        ChunkId::MIDI.0,
        ChunkId::PROJ.0,
        ChunkId::FONT.0,
        ChunkId::MIXR.0,
        ChunkId::LOOP.0,
        ChunkId::SMIX.0,
    ] {
        if !chunk_records
            .iter()
            .any(|record| record.chunk_id == required)
        {
            return Err(FlutzError::InvalidInput(format!(
                "required chunk {:?} is missing",
                required
            )));
        }
    }

    Ok(FmidFile {
        header_flags,
        chunk_table_flags,
        midi_bytes: midi.unwrap_or_default(),
        project: proj.unwrap_or_default(),
        soundfonts: font.unwrap_or_default(),
        mixer: mixr.unwrap_or_default(),
        mixer_source_mode: msrc.unwrap_or_default(),
        looping: loop_chunk.unwrap_or_default(),
        smart_mix: smix.unwrap_or_default(),
        note,
        unknown_chunks,
    })
}

fn read_chunk_table(
    bytes: &[u8],
    header_chunk_count: usize,
) -> Result<(u64, Vec<FmidChunkRecord>)> {
    if bytes.len() < CHUNK_TABLE_HEADER_SIZE {
        return Err(FlutzError::InvalidInput(
            "FMID chunk table header is truncated".to_owned(),
        ));
    }

    let mut cursor = Cursor::new(bytes);
    let magic = read_exact_4(&mut cursor)?;
    if &magic != FMID_CHUNK_TABLE_MAGIC {
        return Err(FlutzError::InvalidInput(
            "missing FMID chunk table magic".to_owned(),
        ));
    }

    let table_length = read_u64(&mut cursor)? as usize;
    let table_flags = read_u64(&mut cursor)?;
    let chunk_count = read_u64(&mut cursor)? as usize;

    if table_length != bytes.len() {
        return Err(FlutzError::InvalidInput(
            "FMID chunk table length mismatch".to_owned(),
        ));
    }
    if chunk_count != header_chunk_count {
        return Err(FlutzError::InvalidInput(
            "FMID chunk count mismatch between header and table".to_owned(),
        ));
    }
    if CHUNK_TABLE_HEADER_SIZE + chunk_count.saturating_mul(CHUNK_RECORD_SIZE) != bytes.len() {
        return Err(FlutzError::InvalidInput(
            "FMID chunk table record length mismatch".to_owned(),
        ));
    }

    let mut records = Vec::with_capacity(chunk_count);
    for _ in 0..chunk_count {
        records.push(FmidChunkRecord {
            chunk_id: read_exact_4(&mut cursor)?,
            offset: read_u64(&mut cursor)?,
            length: read_u64(&mut cursor)?,
            flags: read_u64(&mut cursor)?,
            ordinal: read_u64(&mut cursor)?,
        });
    }

    Ok((table_flags, records))
}

fn read_blob(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(bytes);
    let len = read_u64(&mut cursor)? as usize;
    let start = cursor.position() as usize;
    let end = start.saturating_add(len);
    if end > bytes.len() {
        return Err(FlutzError::InvalidInput(
            "blob length exceeds chunk payload".to_owned(),
        ));
    }
    Ok(bytes[start..end].to_vec())
}

fn read_project(bytes: &[u8]) -> Result<ProjectRecord> {
    let mut cursor = Cursor::new(bytes);
    Ok(ProjectRecord {
        project_name: read_utf8_cursor(&mut cursor, bytes)?,
        source_midi_filename: read_utf8_cursor(&mut cursor, bytes)?,
        project_flags: read_u64(&mut cursor)?,
        notes: read_utf8_cursor(&mut cursor, bytes)?,
    })
}

fn read_soundfont_slots(bytes: &[u8]) -> Result<Vec<SoundFontSlot>> {
    let mut cursor = Cursor::new(bytes);
    let count = read_u64(&mut cursor)? as usize;
    let mut slots = Vec::with_capacity(count);
    for _ in 0..count {
        slots.push(SoundFontSlot {
            internal_id: read_utf8_cursor(&mut cursor, bytes)?,
        });
    }
    Ok(slots)
}

fn read_mixer(bytes: &[u8]) -> Result<MixerRecord> {
    let mut cursor = Cursor::new(bytes);
    let master = MasterMixerRecord {
        volume_db: read_f64(&mut cursor)?,
        limiter_enabled: read_bool(&mut cursor)?,
        limiter_amount: read_f64(&mut cursor)?,
        limiter_release: read_f64(&mut cursor)?,
        reverb: read_f64(&mut cursor)?,
        chorus: read_f64(&mut cursor)?,
        eq_low_db: read_f64(&mut cursor)?,
        eq_mid_db: read_f64(&mut cursor)?,
        eq_high_db: read_f64(&mut cursor)?,
    };
    let row_count = read_u64(&mut cursor)? as usize;
    let mut row_mutes = Vec::with_capacity(row_count);
    for _ in 0..row_count {
        row_mutes.push(SoundFontRowMuteRecord {
            soundfont_id: read_utf8_cursor(&mut cursor, bytes)?,
            muted: read_bool(&mut cursor)?,
        });
    }
    let strip_count = read_u64(&mut cursor)? as usize;
    let mut strips = Vec::with_capacity(strip_count);
    for _ in 0..strip_count {
        let identity = MixerStripIdentity {
            soundfont_id: read_utf8_cursor(&mut cursor, bytes)?,
            midi_channel: read_u64(&mut cursor)?,
            midi_program: read_u64(&mut cursor)?,
            is_percussion: read_bool(&mut cursor)?,
        };
        let controls = MixerStripControls {
            volume: read_f64(&mut cursor)?,
            mute: read_bool(&mut cursor)?,
            pan: read_f64(&mut cursor)?,
            gain_db: read_f64(&mut cursor)?,
            limiter_enabled: read_bool(&mut cursor)?,
            limiter_amount: read_f64(&mut cursor)?,
            limiter_release: read_f64(&mut cursor)?,
            reverb: read_f64(&mut cursor)?,
            chorus: read_f64(&mut cursor)?,
        };
        strips.push(MixerStripRecord { identity, controls });
    }
    Ok(MixerRecord {
        master,
        row_mutes,
        strips,
    })
}

fn read_mixer_source(bytes: &[u8]) -> Result<MixerSourceMode> {
    let mut cursor = Cursor::new(bytes);
    let mode = read_u64(&mut cursor)?;
    match mode {
        0 => Ok(MixerSourceMode::Custom),
        1 => Ok(MixerSourceMode::PresetDefault(read_utf8_cursor(
            &mut cursor,
            bytes,
        )?)),
        other => Err(FlutzError::InvalidInput(format!(
            "invalid mixer source mode value: {other}"
        ))),
    }
}

fn read_loop(bytes: &[u8]) -> Result<LoopRecord> {
    let mut cursor = Cursor::new(bytes);
    let enabled = read_bool(&mut cursor)?;
    let mode_raw = read_u64(&mut cursor)?;
    let mode = LoopMode::from_u64(mode_raw)
        .ok_or_else(|| FlutzError::InvalidInput(format!("invalid loop mode value: {mode_raw}")))?;
    Ok(LoopRecord {
        enabled,
        mode,
        start_tick: read_u64(&mut cursor)?,
        end_tick: read_u64(&mut cursor)?,
        loop_count: read_u64(&mut cursor)?,
    })
}

fn read_smart_mix(bytes: &[u8]) -> Result<SmartMixRecord> {
    let mut cursor = Cursor::new(bytes);
    Ok(SmartMixRecord {
        enabled: read_bool(&mut cursor)?,
        target_headroom: read_f64(&mut cursor)?,
        attack: read_f64(&mut cursor)?,
        release: read_f64(&mut cursor)?,
        lookahead: read_f64(&mut cursor)?,
        auto_normalization_enabled: read_bool(&mut cursor)?,
        auto_normalization_amount: read_f64(&mut cursor)?,
    })
}

fn read_utf8(bytes: &[u8]) -> Result<String> {
    let mut cursor = Cursor::new(bytes);
    read_utf8_cursor(&mut cursor, bytes)
}

fn read_utf8_cursor(cursor: &mut Cursor<&[u8]>, bytes: &[u8]) -> Result<String> {
    let len = read_u64(cursor)? as usize;
    let start = cursor.position() as usize;
    let end = start.saturating_add(len);
    if end > bytes.len() {
        return Err(FlutzError::InvalidInput(
            "UTF-8 string length exceeds chunk payload".to_owned(),
        ));
    }
    cursor.set_position(end as u64);
    std::str::from_utf8(&bytes[start..end])
        .map(str::to_owned)
        .map_err(|_| FlutzError::InvalidInput("malformed UTF-8 string field".to_owned()))
}

fn read_bool(cursor: &mut Cursor<&[u8]>) -> Result<bool> {
    match read_u8(cursor)? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(FlutzError::InvalidInput(format!(
            "invalid boolean value: {other}"
        ))),
    }
}

fn read_u8(cursor: &mut Cursor<&[u8]>) -> Result<u8> {
    let mut buffer = [0u8; 1];
    cursor
        .read_exact(&mut buffer)
        .map_err(|_| FlutzError::InvalidInput("unexpected end of file".to_owned()))?;
    Ok(buffer[0])
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Result<u64> {
    let mut buffer = [0u8; 8];
    cursor
        .read_exact(&mut buffer)
        .map_err(|_| FlutzError::InvalidInput("unexpected end of file".to_owned()))?;
    Ok(u64::from_le_bytes(buffer))
}

fn read_f64(cursor: &mut Cursor<&[u8]>) -> Result<f64> {
    Ok(f64::from_bits(read_u64(cursor)?))
}

fn read_exact_4(cursor: &mut Cursor<&[u8]>) -> Result<[u8; 4]> {
    let mut buffer = [0u8; 4];
    cursor
        .read_exact(&mut buffer)
        .map_err(|_| FlutzError::InvalidInput("unexpected end of file".to_owned()))?;
    Ok(buffer)
}

use std::io::{Cursor, Read};
