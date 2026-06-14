#![allow(dead_code)]

use std::{io::Read, mem};

use crate::binary_reader::BinaryReader;
use crate::four_cc::FourCC;
use std::collections::BTreeSet;

use crate::midi_interpretation::{apply_sysex_event, MidiChannelProgramRole, MidiInterpretation};
use crate::read_counter::ReadCounter;
use crate::MidiFileError;
use crate::MidiFileLoopType;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub(crate) enum Message {
    Normal { status: u8, data1: u8, data2: u8 },
    SysEx { bytes: Vec<u8> },
    TempoChange { bytes: [u8; 3] },
    LoopStart,
    LoopEnd,
    EndOfTrack,
}

impl Message {
    pub(crate) fn common1(status: u8, data1: u8) -> Self {
        Self::Normal {
            status,
            data1,
            data2: 0,
        }
    }

    pub(crate) fn common2(status: u8, data1: u8, data2: u8, loop_type: MidiFileLoopType) -> Self {
        let command = status & 0xF0;

        if command == 0xB0 {
            match loop_type {
                MidiFileLoopType::RpgMaker => {
                    if data1 == 111 {
                        return Message::LoopStart;
                    }
                }

                MidiFileLoopType::IncredibleMachine => {
                    if data1 == 110 {
                        return Message::LoopStart;
                    }
                    if data1 == 111 {
                        return Message::LoopEnd;
                    }
                }

                MidiFileLoopType::FinalFantasy => {
                    if data1 == 116 {
                        return Message::LoopStart;
                    }
                    if data1 == 117 {
                        return Message::LoopEnd;
                    }
                }

                _ => (),
            }
        }

        Self::Normal {
            status,
            data1,
            data2,
        }
    }

    pub(crate) fn tempo_change(tempo: i32) -> Self {
        // Truncate to u24
        let bytes = tempo.to_be_bytes()[1..].try_into().unwrap();
        Self::TempoChange { bytes }
    }
}

/// Represents a standard MIDI file.
#[derive(Debug)]
#[non_exhaustive]
pub struct MidiFile {
    pub(crate) messages: Vec<Message>,
    pub(crate) ticks: Vec<i32>,
    pub(crate) times: Vec<f64>,
    interpretation: MidiInterpretation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MidiEventSummary {
    pub tick: i32,
    pub time_seconds: f64,
    pub kind: String,
    pub channel: Option<u8>,
    pub data1: Option<u8>,
    pub data2: Option<u8>,
    pub description: String,
}

impl MidiFile {
    /// Loads a MIDI file from the stream.
    ///
    /// # Arguments
    ///
    /// * `reader` - The data stream used to load the MIDI file.
    pub fn new<R: Read>(reader: &mut R) -> Result<Self, MidiFileError> {
        MidiFile::new_with_loop_type(reader, MidiFileLoopType::LoopPoint(0))
    }

    pub fn memory_debug(&self) -> MidiFileMemoryDebug {
        let sysex_bytes = self
            .messages
            .iter()
            .map(|message| match message {
                Message::SysEx { bytes } => bytes.capacity(),
                _ => 0,
            })
            .sum::<usize>();
        let sysex_events = self
            .messages
            .iter()
            .filter(|message| matches!(message, Message::SysEx { .. }))
            .count();
        let message_bytes = self.messages.capacity() * mem::size_of::<Message>();
        let tick_bytes = self.ticks.capacity() * mem::size_of::<i32>();
        let time_bytes = self.times.capacity() * mem::size_of::<f64>();
        MidiFileMemoryDebug {
            message_count: self.messages.len(),
            message_capacity: self.messages.capacity(),
            tick_count: self.ticks.len(),
            tick_capacity: self.ticks.capacity(),
            time_count: self.times.len(),
            time_capacity: self.times.capacity(),
            sysex_events,
            sysex_bytes,
            message_bytes,
            tick_bytes,
            time_bytes,
            estimated_bytes: message_bytes
                .saturating_add(tick_bytes)
                .saturating_add(time_bytes)
                .saturating_add(sysex_bytes),
        }
    }

    /// Loads a MIDI file from the stream with a specified loop type.
    ///
    /// # Arguments
    ///
    /// * `reader` - The data stream used to load the MIDI file.
    /// * `loop_type` - The type of the loop extension to be used.
    ///
    /// # Remarks
    ///
    /// `MidiFileLoopType` has the following variants:
    /// * `LoopPoint(usize)` - Specifies the loop start point by a tick value.
    /// * `RpgMaker` - The RPG Maker style loop.
    ///   CC #111 will be the loop start point.
    /// * `IncredibleMachine` - The Incredible Machine style loop.
    ///   CC #110 and #111 will be the start and end points of the loop.
    /// * `FinalFantasy` - The Final Fantasy style loop.
    ///   CC #116 and #117 will be the start and end points of the loop.
    pub fn new_with_loop_type<R: Read>(
        reader: &mut R,
        loop_type: MidiFileLoopType,
    ) -> Result<Self, MidiFileError> {
        let chunk_type = BinaryReader::read_four_cc(reader)?;
        if chunk_type != b"MThd" {
            return Err(MidiFileError::InvalidChunkType {
                expected: FourCC::from_bytes(*b"MThd"),
                actual: chunk_type,
            });
        }

        let size = BinaryReader::read_i32_big_endian(reader)?;
        if size != 6 {
            return Err(MidiFileError::InvalidChunkData(FourCC::from_bytes(
                *b"MThd",
            )));
        }

        let format = BinaryReader::read_i16_big_endian(reader)?;
        if !(format == 0 || format == 1) {
            return Err(MidiFileError::UnsupportedFormat(format));
        }

        let track_count = BinaryReader::read_i16_big_endian(reader)? as i32;
        let resolution = BinaryReader::read_i16_big_endian(reader)? as i32;

        let mut message_lists: Vec<Vec<Message>> = Vec::new();
        let mut tick_lists: Vec<Vec<i32>> = Vec::new();

        for _i in 0..track_count {
            let (message_list, tick_list) = MidiFile::read_track(reader, loop_type)?;
            message_lists.push(message_list);
            tick_lists.push(tick_list);
        }

        match loop_type {
            MidiFileLoopType::LoopPoint(loop_point) if loop_point != 0 => {
                let loop_point = loop_point as i32;
                let tick_list = &mut tick_lists[0];
                let message_list = &mut message_lists[0];

                if loop_point <= *tick_list.last().unwrap() {
                    for i in 0..tick_list.len() {
                        if tick_list[i] >= loop_point {
                            tick_list.insert(i, loop_point);
                            message_list.insert(i, Message::LoopStart);
                            break;
                        }
                    }
                } else {
                    tick_list.push(loop_point);
                    message_list.push(Message::LoopStart);
                }
            }
            _ => (),
        }

        let (messages, ticks, times) =
            MidiFile::merge_tracks(&message_lists, &tick_lists, resolution);

        let interpretation = MidiFile::interpret_messages(&messages);

        Ok(Self {
            messages,
            ticks,
            times,
            interpretation,
        })
    }

    fn discard_data<R: Read>(reader: &mut R) -> Result<(), MidiFileError> {
        let size = BinaryReader::read_i32_variable_length(reader)? as usize;
        BinaryReader::discard_data(reader, size)?;
        Ok(())
    }

    fn read_sysex<R: Read>(reader: &mut R, status: u8) -> Result<Message, MidiFileError> {
        let size = BinaryReader::read_i32_variable_length(reader)? as usize;
        let mut bytes = Vec::with_capacity(size + 1);
        bytes.push(status);
        for _ in 0..size {
            bytes.push(BinaryReader::read_u8(reader)?);
        }
        Ok(Message::SysEx { bytes })
    }

    fn read_tempo<R: Read>(reader: &mut R) -> Result<i32, MidiFileError> {
        let size = BinaryReader::read_i32_variable_length(reader)?;
        if size != 3 {
            return Err(MidiFileError::InvalidTempoValue);
        }

        let b1 = BinaryReader::read_u8(reader)? as i32;
        let b2 = BinaryReader::read_u8(reader)? as i32;
        let b3 = BinaryReader::read_u8(reader)? as i32;

        Ok((b1 << 16) | (b2 << 8) | b3)
    }

    fn read_track<R: Read>(
        reader: &mut R,
        loop_type: MidiFileLoopType,
    ) -> Result<(Vec<Message>, Vec<i32>), MidiFileError> {
        let chunk_type = BinaryReader::read_four_cc(reader)?;
        if chunk_type != b"MTrk" {
            return Err(MidiFileError::InvalidChunkType {
                expected: FourCC::from_bytes(*b"MTrk"),
                actual: chunk_type,
            });
        }

        let size = BinaryReader::read_i32_big_endian(reader)? as usize;
        let reader = &mut ReadCounter::new(reader);

        let mut messages: Vec<Message> = Vec::new();
        let mut ticks: Vec<i32> = Vec::new();

        let mut tick: i32 = 0;
        let mut last_status: u8 = 0;

        loop {
            let delta = BinaryReader::read_i32_variable_length(reader)?;
            let first = BinaryReader::read_u8(reader)?;

            tick += delta;

            if (first & 128) == 0 {
                let command = last_status & 0xF0;
                if command == 0xC0 || command == 0xD0 {
                    messages.push(Message::common1(last_status, first));
                    ticks.push(tick);
                } else {
                    let data2 = BinaryReader::read_u8(reader)?;
                    messages.push(Message::common2(last_status, first, data2, loop_type));
                    ticks.push(tick);
                }

                continue;
            }

            match first {
                0xF0 => {
                    messages.push(MidiFile::read_sysex(reader, first)?);
                    ticks.push(tick);
                }
                0xF7 => {
                    messages.push(MidiFile::read_sysex(reader, first)?);
                    ticks.push(tick);
                }
                0xFF => match BinaryReader::read_u8(reader)? {
                    0x2F => {
                        BinaryReader::read_u8(reader)?;
                        messages.push(Message::EndOfTrack);
                        ticks.push(tick);

                        // Some MIDI files may have events inserted after the EOT.
                        // Such events should be ignored.
                        if reader.bytes_read() < size {
                            BinaryReader::discard_data(reader, size - reader.bytes_read())?;
                        }

                        return Ok((messages, ticks));
                    }
                    0x51 => {
                        messages.push(Message::tempo_change(MidiFile::read_tempo(reader)?));
                        ticks.push(tick);
                    }
                    _ => MidiFile::discard_data(reader)?,
                },
                _ => {
                    let command = first & 0xF0;
                    if command == 0xC0 || command == 0xD0 {
                        let data1 = BinaryReader::read_u8(reader)?;
                        messages.push(Message::common1(first, data1));
                        ticks.push(tick);
                    } else {
                        let data1 = BinaryReader::read_u8(reader)?;
                        let data2 = BinaryReader::read_u8(reader)?;
                        messages.push(Message::common2(first, data1, data2, loop_type));
                        ticks.push(tick);
                    }
                }
            }

            last_status = first
        }
    }

    fn merge_tracks(
        message_lists: &[Vec<Message>],
        tick_lists: &[Vec<i32>],
        resolution: i32,
    ) -> (Vec<Message>, Vec<i32>, Vec<f64>) {
        let mut merged_messages: Vec<Message> = Vec::new();
        let mut merged_ticks: Vec<i32> = Vec::new();
        let mut merged_times: Vec<f64> = Vec::new();

        let mut indices: Vec<usize> = vec![0; message_lists.len()];

        let mut current_tick: i32 = 0;
        let mut current_time: f64 = 0.0;

        let mut tempo: f64 = 120.0;

        loop {
            let mut min_tick = i32::MAX;
            let mut min_index: i32 = -1;

            for ch in 0..tick_lists.len() {
                if indices[ch] < tick_lists[ch].len() {
                    let tick = tick_lists[ch][indices[ch]];
                    if tick < min_tick {
                        min_tick = tick;
                        min_index = ch as i32;
                    }
                }
            }

            if min_index == -1 {
                break;
            }

            let next_tick = tick_lists[min_index as usize][indices[min_index as usize]];
            let delta_tick = next_tick - current_tick;
            let delta_time = 60.0 / (resolution as f64 * tempo) * delta_tick as f64;

            current_tick += delta_tick;
            current_time += delta_time;

            let message = message_lists[min_index as usize][indices[min_index as usize]].clone();
            if let Message::TempoChange { bytes } = message {
                let tempo_i32 = i32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
                tempo = 60000000.0 / tempo_i32 as f64;
            } else {
                merged_messages.push(message);
                merged_ticks.push(current_tick);
                merged_times.push(current_time);
            }

            indices[min_index as usize] += 1;
        }

        (merged_messages, merged_ticks, merged_times)
    }

    fn interpret_messages(messages: &[Message]) -> MidiInterpretation {
        let mut interpretation = MidiInterpretation::default();
        for message in messages {
            if let Message::SysEx { bytes } = message {
                interpretation.note_sysex(bytes);
            }
        }
        interpretation
    }

    /// Get the length of the MIDI file in seconds.
    pub fn get_length(&self) -> f64 {
        *self.times.last().unwrap_or(&0.0)
    }

    /// Get the length of the MIDI file in ticks.
    pub fn get_tick_length(&self) -> i32 {
        *self.ticks.last().unwrap_or(&0)
    }

    /// Get the MIDI system interpretation derived from SysEx and GM defaults.
    pub fn get_interpretation(&self) -> &MidiInterpretation {
        &self.interpretation
    }

    /// Get observed channel/program roles using SysEx role changes at event tick order.
    pub fn get_channel_program_roles(&self) -> Vec<MidiChannelProgramRole> {
        let mut interpretation = MidiInterpretation::default();
        let mut current_programs = [0u8; 16];
        let mut current_banks = [0u16; 16];
        current_banks[9] = 128;
        let mut note_roles = BTreeSet::new();
        let mut program_roles = BTreeSet::new();

        for message in &self.messages {
            match message {
                Message::SysEx { bytes } => {
                    let previous_percussion_channels = interpretation.percussion_channels.clone();
                    apply_sysex_event(&mut interpretation, bytes);
                    for channel in 0..16u8 {
                        let was_percussion = previous_percussion_channels.contains(&channel);
                        let is_percussion = interpretation.is_percussion_channel(channel);
                        if was_percussion != is_percussion {
                            current_banks[channel as usize] = if is_percussion { 128 } else { 0 };
                        }
                    }
                }
                Message::Normal {
                    status,
                    data1,
                    data2,
                } => {
                    let event = status & 0xF0;
                    let channel = status & 0x0F;
                    match event {
                        0x90 if *data2 > 0 => {
                            note_roles.insert(MidiChannelProgramRole {
                                channel,
                                bank: current_banks[channel as usize],
                                program: current_programs[channel as usize],
                                is_percussion: interpretation.is_percussion_channel(channel),
                            });
                        }
                        0xB0 if *data1 == 0x00 => {
                            current_banks[channel as usize] =
                                if interpretation.is_percussion_channel(channel) {
                                    128u16.saturating_add(*data2 as u16)
                                } else {
                                    *data2 as u16
                                };
                        }
                        0xC0 => {
                            current_programs[channel as usize] = *data1;
                            program_roles.insert(MidiChannelProgramRole {
                                channel,
                                bank: current_banks[channel as usize],
                                program: *data1,
                                is_percussion: interpretation.is_percussion_channel(channel),
                            });
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        if note_roles.is_empty() {
            program_roles.into_iter().collect()
        } else {
            note_roles.into_iter().collect()
        }
    }

    /// Get the playback time corresponding to a MIDI tick position.
    pub fn get_time_at_tick(&self, tick: i32) -> f64 {
        if self.ticks.is_empty() {
            return 0.0;
        }

        let target_tick = tick.clamp(0, self.get_tick_length());
        match self.ticks.binary_search(&target_tick) {
            Ok(index) => self.times[index],
            Err(0) => 0.0,
            Err(index) if index >= self.ticks.len() => self.get_length(),
            Err(index) => {
                let previous_tick = self.ticks[index - 1];
                let next_tick = self.ticks[index];
                let previous_time = self.times[index - 1];
                let next_time = self.times[index];
                if next_tick == previous_tick {
                    return next_time;
                }

                let fraction =
                    (target_tick - previous_tick) as f64 / (next_tick - previous_tick) as f64;
                previous_time + (next_time - previous_time) * fraction
            }
        }
    }

    /// Get the MIDI tick position corresponding to a playback time.
    pub fn get_tick_at_time(&self, seconds: f64) -> i32 {
        if self.times.is_empty() {
            return 0;
        }

        let target_time = seconds.clamp(0.0, self.get_length());
        match self.times.binary_search_by(|time| {
            time.partial_cmp(&target_time)
                .unwrap_or(std::cmp::Ordering::Less)
        }) {
            Ok(index) => self.ticks[index],
            Err(0) => 0,
            Err(index) if index >= self.times.len() => self.get_tick_length(),
            Err(index) => {
                let previous_time = self.times[index - 1];
                let next_time = self.times[index];
                let previous_tick = self.ticks[index - 1];
                let next_tick = self.ticks[index];
                if next_time <= previous_time {
                    return next_tick;
                }

                let fraction = (target_time - previous_time) / (next_time - previous_time);
                (previous_tick as f64 + (next_tick - previous_tick) as f64 * fraction).round()
                    as i32
            }
        }
    }

    /// Get loop start marker ticks parsed from the MIDI file.
    pub fn get_loop_start_ticks(&self) -> Vec<i32> {
        self.messages
            .iter()
            .zip(&self.ticks)
            .filter_map(|(message, tick)| match message {
                Message::LoopStart => Some(*tick),
                _ => None,
            })
            .collect()
    }

    /// Get loop end marker ticks parsed from the MIDI file.
    pub fn get_loop_end_ticks(&self) -> Vec<i32> {
        self.messages
            .iter()
            .zip(&self.ticks)
            .filter_map(|(message, tick)| match message {
                Message::LoopEnd => Some(*tick),
                _ => None,
            })
            .collect()
    }

    /// Get concise event summaries in an inclusive tick range.
    pub fn get_event_summaries_in_tick_range(
        &self,
        start_tick: i32,
        end_tick: i32,
    ) -> Vec<MidiEventSummary> {
        let range_start = start_tick.min(end_tick);
        let range_end = start_tick.max(end_tick);
        self.messages
            .iter()
            .zip(&self.ticks)
            .zip(&self.times)
            .filter_map(|((message, tick), time_seconds)| {
                if *tick < range_start || *tick > range_end {
                    return None;
                }
                summarize_event(message, *tick, *time_seconds)
            })
            .collect()
    }
}

fn summarize_event(message: &Message, tick: i32, time_seconds: f64) -> Option<MidiEventSummary> {
    match message {
        Message::Normal {
            status,
            data1,
            data2,
        } => {
            let event = status & 0xF0;
            let channel = status & 0x0F;
            let (kind, description) = match event {
                0x80 => (
                    "note-off",
                    format!("note {} off velocity {}", data1, data2),
                ),
                0x90 if *data2 == 0 => (
                    "note-off",
                    format!("note {} off velocity 0", data1),
                ),
                0x90 => (
                    "note-on",
                    format!("note {} on velocity {}", data1, data2),
                ),
                0xA0 => (
                    "poly-pressure",
                    format!("note {} pressure {}", data1, data2),
                ),
                0xB0 => (
                    "control-change",
                    format!(
                        "cc {} ({}) value {}",
                        data1,
                        controller_name(*data1),
                        data2
                    ),
                ),
                0xC0 => ("program-change", format!("program {}", data1)),
                0xD0 => ("channel-pressure", format!("pressure {}", data1)),
                0xE0 => {
                    let bend = ((*data2 as u16) << 7) | (*data1 as u16);
                    ("pitch-bend", format!("pitch bend {}", bend))
                }
                _ => return None,
            };
            Some(MidiEventSummary {
                tick,
                time_seconds,
                kind: kind.to_owned(),
                channel: Some(channel),
                data1: Some(*data1),
                data2: Some(*data2),
                description,
            })
        }
        Message::SysEx { bytes } => Some(MidiEventSummary {
            tick,
            time_seconds,
            kind: "sysex".to_owned(),
            channel: None,
            data1: None,
            data2: None,
            description: format!("sysex {} bytes", bytes.len()),
        }),
        Message::LoopStart => Some(MidiEventSummary {
            tick,
            time_seconds,
            kind: "loop-start".to_owned(),
            channel: None,
            data1: None,
            data2: None,
            description: "loop start marker".to_owned(),
        }),
        Message::LoopEnd => Some(MidiEventSummary {
            tick,
            time_seconds,
            kind: "loop-end".to_owned(),
            channel: None,
            data1: None,
            data2: None,
            description: "loop end marker".to_owned(),
        }),
        Message::EndOfTrack => Some(MidiEventSummary {
            tick,
            time_seconds,
            kind: "end-of-track".to_owned(),
            channel: None,
            data1: None,
            data2: None,
            description: "end of track".to_owned(),
        }),
        Message::TempoChange { bytes } => {
            let tempo = i32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
            Some(MidiEventSummary {
                tick,
                time_seconds,
                kind: "tempo-change".to_owned(),
                channel: None,
                data1: None,
                data2: None,
                description: format!("tempo {} us/qn", tempo),
            })
        }
    }
}

fn controller_name(controller: u8) -> &'static str {
    match controller {
        0 => "bank-select-msb",
        1 => "modulation",
        7 => "channel-volume",
        10 => "pan",
        11 => "expression",
        64 => "sustain",
        91 => "reverb-send",
        93 => "chorus-send",
        110 => "loop-start-im",
        111 => "loop-marker",
        116 => "loop-start-ff",
        117 => "loop-end-ff",
        120 => "all-sound-off",
        121 => "reset-all-controllers",
        123 => "all-notes-off",
        _ => "controller",
    }
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct MidiFileMemoryDebug {
    pub message_count: usize,
    pub message_capacity: usize,
    pub tick_count: usize,
    pub tick_capacity: usize,
    pub time_count: usize,
    pub time_capacity: usize,
    pub sysex_events: usize,
    pub sysex_bytes: usize,
    pub message_bytes: usize,
    pub tick_bytes: usize,
    pub time_bytes: usize,
    pub estimated_bytes: usize,
}
