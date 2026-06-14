use crate::{
    chunks::ChunkId,
    records::{
        FmidChunkRecord, FmidFile, MixerSourceMode, MixerStripRecord, ProjectRecord,
        SmartMixRecord, SoundFontSlot, UnknownChunk,
    },
    FMID_CHUNK_TABLE_MAGIC, FMID_MAGIC,
};

pub fn fmid_magic_bytes() -> &'static [u8; 4] {
    FMID_MAGIC
}

pub fn write_fmid(file: &FmidFile) -> Vec<u8> {
    let mut payloads = vec![
        (ChunkId::MIDI.0, 0u64, write_blob(&file.midi_bytes), 0u64),
        (ChunkId::PROJ.0, 0u64, write_project(&file.project), 1u64),
        (
            ChunkId::FONT.0,
            0u64,
            write_soundfonts(&file.soundfonts),
            2u64,
        ),
        (ChunkId::MIXR.0, 0u64, write_mixer(file), 3u64),
        (ChunkId::LOOP.0, 0u64, write_loop(file), 4u64),
        (
            ChunkId::SMIX.0,
            0u64,
            write_smart_mix(&file.smart_mix),
            5u64,
        ),
        (
            ChunkId::MSRC.0,
            0u64,
            write_mixer_source(&file.mixer_source_mode),
            6u64,
        ),
    ];

    if let Some(note) = &file.note {
        payloads.push((ChunkId::NOTE.0, 0u64, write_utf8(note), 7u64));
    }

    let mut unknown = file.unknown_chunks.clone();
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
    let chunk_table_length = 4 + 8 * 3 + chunk_count * (4 + 8 * 4);
    let header_size = 4 + 8 * 6;
    let payload_offset_start = header_size + chunk_table_length;

    let mut chunk_records = Vec::with_capacity(chunk_count);
    let mut payload_cursor = payload_offset_start as u64;
    for (chunk_id, flags, payload, ordinal) in &payloads {
        chunk_records.push(FmidChunkRecord {
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

    write_exact_4(&mut out, *FMID_MAGIC);
    write_u64(&mut out, header_size as u64);
    write_u64(&mut out, file.header_flags);
    write_u64(&mut out, file_size as u64);
    write_u64(&mut out, header_size as u64);
    write_u64(&mut out, chunk_table_length as u64);
    write_u64(&mut out, chunk_count as u64);

    write_exact_4(&mut out, *FMID_CHUNK_TABLE_MAGIC);
    write_u64(&mut out, chunk_table_length as u64);
    write_u64(&mut out, file.chunk_table_flags);
    write_u64(&mut out, chunk_count as u64);
    for record in &chunk_records {
        write_exact_4(&mut out, record.chunk_id);
        write_u64(&mut out, record.offset);
        write_u64(&mut out, record.length);
        write_u64(&mut out, record.flags);
        write_u64(&mut out, record.ordinal);
    }

    for (_chunk_id, _flags, payload, _ordinal) in payloads {
        out.extend_from_slice(&payload);
    }

    out
}

fn write_blob(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + bytes.len());
    write_u64(&mut out, bytes.len() as u64);
    out.extend_from_slice(bytes);
    out
}

fn write_project(project: &ProjectRecord) -> Vec<u8> {
    let mut out = Vec::new();
    write_utf8_into(&mut out, &project.project_name);
    write_utf8_into(&mut out, &project.source_midi_filename);
    write_u64(&mut out, project.project_flags);
    write_utf8_into(&mut out, &project.notes);
    out
}

fn write_soundfonts(soundfonts: &[SoundFontSlot]) -> Vec<u8> {
    let mut out = Vec::new();
    write_u64(&mut out, soundfonts.len() as u64);
    for slot in soundfonts {
        write_utf8_into(&mut out, &slot.internal_id);
    }
    out
}

fn write_mixer(file: &FmidFile) -> Vec<u8> {
    let mut out = Vec::new();
    write_f64(&mut out, file.mixer.master.volume_db);
    write_bool(&mut out, file.mixer.master.limiter_enabled);
    write_f64(&mut out, file.mixer.master.limiter_amount);
    write_f64(&mut out, file.mixer.master.limiter_release);
    write_f64(&mut out, file.mixer.master.reverb);
    write_f64(&mut out, file.mixer.master.chorus);
    write_f64(&mut out, file.mixer.master.eq_low_db);
    write_f64(&mut out, file.mixer.master.eq_mid_db);
    write_f64(&mut out, file.mixer.master.eq_high_db);

    write_u64(&mut out, file.mixer.row_mutes.len() as u64);
    for row in &file.mixer.row_mutes {
        write_utf8_into(&mut out, &row.soundfont_id);
        write_bool(&mut out, row.muted);
    }

    write_u64(&mut out, file.mixer.strips.len() as u64);
    for strip in &file.mixer.strips {
        write_strip(&mut out, strip);
    }
    out
}

fn write_strip(out: &mut Vec<u8>, strip: &MixerStripRecord) {
    write_utf8_into(out, &strip.identity.soundfont_id);
    write_u64(out, strip.identity.midi_channel);
    write_u64(out, strip.identity.midi_program);
    write_bool(out, strip.identity.is_percussion);

    write_f64(out, strip.controls.volume);
    write_bool(out, strip.controls.mute);
    write_f64(out, strip.controls.pan);
    write_f64(out, strip.controls.gain_db);
    write_bool(out, strip.controls.limiter_enabled);
    write_f64(out, strip.controls.limiter_amount);
    write_f64(out, strip.controls.limiter_release);
    write_f64(out, strip.controls.reverb);
    write_f64(out, strip.controls.chorus);
}

fn write_mixer_source(mode: &MixerSourceMode) -> Vec<u8> {
    let mut out = Vec::new();
    match mode {
        MixerSourceMode::Custom => write_u64(&mut out, 0),
        MixerSourceMode::PresetDefault(preset_id) => {
            write_u64(&mut out, 1);
            write_utf8_into(&mut out, preset_id);
        }
    }
    out
}

fn write_loop(file: &FmidFile) -> Vec<u8> {
    let mut out = Vec::new();
    write_bool(&mut out, file.looping.enabled);
    write_u64(&mut out, file.looping.mode.to_u64());
    write_u64(&mut out, file.looping.start_tick);
    write_u64(&mut out, file.looping.end_tick);
    write_u64(&mut out, file.looping.loop_count);
    out
}

fn write_smart_mix(smix: &SmartMixRecord) -> Vec<u8> {
    let mut out = Vec::new();
    write_bool(&mut out, smix.enabled);
    write_f64(&mut out, smix.target_headroom);
    write_f64(&mut out, smix.attack);
    write_f64(&mut out, smix.release);
    write_f64(&mut out, smix.lookahead);
    write_bool(&mut out, smix.auto_normalization_enabled);
    write_f64(&mut out, smix.auto_normalization_amount);
    out
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

#[allow(dead_code)]
fn _keep_unknown_type(_chunk: &UnknownChunk) {}
