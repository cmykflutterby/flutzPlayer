use super::*;

#[derive(Debug, Clone)]
pub struct LoadedMidi {
    pub byte_len: usize,
    midi_file: Arc<MidiFile>,
}

impl LoadedMidi {
    pub fn midi_file(&self) -> &Arc<MidiFile> {
        &self.midi_file
    }

    pub fn duration_seconds(&self) -> f64 {
        self.midi_file.get_length()
    }

    pub fn tick_length(&self) -> u64 {
        self.midi_file.get_tick_length().max(0) as u64
    }

    pub fn midi_interpretation(&self) -> &MidiInterpretation {
        self.midi_file.get_interpretation()
    }

    pub fn memory_debug(&self) -> MidiFileMemoryDebug {
        self.midi_file.memory_debug()
    }

    pub fn is_percussion_channel(&self, channel: u8) -> bool {
        self.midi_interpretation().is_percussion_channel(channel)
    }

    pub fn channel_program_roles(&self) -> Vec<MidiChannelProgramRole> {
        self.midi_file.get_channel_program_roles()
    }

    pub fn loop_start_ticks(&self) -> Vec<u64> {
        self.midi_file
            .get_loop_start_ticks()
            .into_iter()
            .map(|tick| tick.max(0) as u64)
            .collect()
    }

    pub fn loop_end_ticks(&self) -> Vec<u64> {
        self.midi_file
            .get_loop_end_ticks()
            .into_iter()
            .map(|tick| tick.max(0) as u64)
            .collect()
    }
}

pub fn validate_midi_bytes(bytes: &[u8]) -> Result<LoadedMidi> {
    validate_midi_bytes_with_loop_type(bytes, MidiFileLoopType::LoopPoint(0))
}

pub fn validate_midi_bytes_with_loop_type(
    bytes: &[u8],
    loop_type: MidiFileLoopType,
) -> Result<LoadedMidi> {
    if !bytes.starts_with(b"MThd") {
        return Err(FlutzError::InvalidInput(
            "missing standard MIDI MThd header".to_owned(),
        ));
    }
    let mut cursor = Cursor::new(bytes);
    let midi_file = MidiFile::new_with_loop_type(&mut cursor, loop_type)
        .map_err(|error| FlutzError::InvalidInput(format!("invalid MIDI file: {error:?}")))?;
    Ok(LoadedMidi {
        byte_len: bytes.len(),
        midi_file: Arc::new(midi_file),
    })
}
