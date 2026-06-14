use std::collections::BTreeSet;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MidiSystemMode {
    GeneralMidi,
    GeneralMidi2,
    RolandGs,
    YamahaXg,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MidiChannelRole {
    Melodic,
    Percussion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiInterpretation {
    pub system_modes: BTreeSet<MidiSystemMode>,
    pub percussion_channels: BTreeSet<u8>,
    pub warnings: Vec<String>,
    pub sysex_events: Vec<MidiSysexEventSummary>,
    pub sysex_event_count: usize,
    pub recognized_sysex_event_count: usize,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MidiChannelProgramRole {
    pub channel: u8,
    pub bank: u16,
    pub program: u8,
    pub is_percussion: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiSysexEventSummary {
    pub index: usize,
    pub status: u8,
    pub byte_len: usize,
    pub manufacturer_id: String,
    pub recognized: bool,
    pub system_mode: Option<MidiSystemMode>,
    pub channel_role: Option<(u8, MidiChannelRole)>,
    pub warning: Option<String>,
    pub bytes_hex: String,
}

impl Default for MidiInterpretation {
    fn default() -> Self {
        Self {
            system_modes: BTreeSet::new(),
            percussion_channels: BTreeSet::from([9]),
            warnings: Vec::new(),
            sysex_events: Vec::new(),
            sysex_event_count: 0,
            recognized_sysex_event_count: 0,
        }
    }
}

impl MidiInterpretation {
    pub fn is_percussion_channel(&self, channel: u8) -> bool {
        self.percussion_channels.contains(&channel)
    }

    pub(crate) fn note_sysex(&mut self, sysex: &[u8]) {
        self.sysex_event_count += 1;
        let event = decode_sysex_event(sysex);
        self.sysex_events.push(MidiSysexEventSummary::new(
            self.sysex_event_count - 1,
            sysex,
            &event,
        ));
        if apply_decoded_sysex_event(self, event) {
            self.recognized_sysex_event_count += 1;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SysexInterpretationEvent {
    pub system_mode: Option<MidiSystemMode>,
    pub channel_role: Option<(u8, MidiChannelRole)>,
    pub warning: Option<String>,
    pub recognized: bool,
}

pub(crate) fn decode_sysex_event(sysex: &[u8]) -> SysexInterpretationEvent {
    let payload = normalized_sysex_payload(sysex);
    if payload.is_empty() {
        return SysexInterpretationEvent::unrecognized();
    }

    if payload.len() >= 4 && payload[0] == 0x7E && payload[2] == 0x09 {
        return match payload[3] {
            0x01 => SysexInterpretationEvent::system_mode(MidiSystemMode::GeneralMidi),
            0x03 => SysexInterpretationEvent::system_mode(MidiSystemMode::GeneralMidi2),
            _ => SysexInterpretationEvent::unrecognized(),
        };
    }

    if payload.len() >= 7 && payload[0] == 0x43 && payload[2] == 0x4C {
        if payload[3..=6] == [0x00, 0x00, 0x7E, 0x00] {
            return SysexInterpretationEvent::system_mode(MidiSystemMode::YamahaXg);
        }
        if payload.len() >= 7 && payload[3] == 0x08 && payload[5] == 0x07 {
            let part = payload[4];
            let value = payload[6];
            if part < 16 {
                let role = if value == 0 {
                    MidiChannelRole::Melodic
                } else {
                    MidiChannelRole::Percussion
                };
                return SysexInterpretationEvent::channel_role(
                    part,
                    role,
                    MidiSystemMode::YamahaXg,
                );
            }
        }
    }

    if payload.len() >= 9 && payload[0] == 0x41 && payload[2] == 0x42 && payload[3] == 0x12 {
        let address = &payload[4..7];
        let data = &payload[7..payload.len().saturating_sub(1)];
        let checksum = payload[payload.len() - 1];
        if !roland_checksum_valid(address, data, checksum) {
            return SysexInterpretationEvent {
                system_mode: Some(MidiSystemMode::RolandGs),
                channel_role: None,
                warning: Some("roland_gs_checksum_invalid".to_owned()),
                recognized: true,
            };
        }

        if address == [0x40, 0x00, 0x7F] && data.first() == Some(&0x00) {
            return SysexInterpretationEvent::system_mode(MidiSystemMode::RolandGs);
        }

        if address[0] == 0x40 && (0x10..=0x1F).contains(&address[1]) && address[2] == 0x15 {
            if let Some(value) = data.first().copied() {
                let channel = address[1] - 0x10;
                let role = if value == 0 {
                    MidiChannelRole::Melodic
                } else {
                    MidiChannelRole::Percussion
                };
                return SysexInterpretationEvent::channel_role(
                    channel,
                    role,
                    MidiSystemMode::RolandGs,
                );
            }
        }

        return SysexInterpretationEvent::system_mode(MidiSystemMode::RolandGs);
    }

    SysexInterpretationEvent::unrecognized()
}

pub(crate) fn apply_sysex_event(interpretation: &mut MidiInterpretation, sysex: &[u8]) -> bool {
    let event = decode_sysex_event(sysex);
    apply_decoded_sysex_event(interpretation, event)
}

fn apply_decoded_sysex_event(
    interpretation: &mut MidiInterpretation,
    event: SysexInterpretationEvent,
) -> bool {
    if let Some(system_mode) = event.system_mode {
        interpretation.system_modes.insert(system_mode);
    }
    if let Some((channel, role)) = event.channel_role {
        match role {
            MidiChannelRole::Melodic => {
                interpretation.percussion_channels.remove(&channel);
            }
            MidiChannelRole::Percussion => {
                interpretation.percussion_channels.insert(channel);
            }
        }
    }
    if let Some(warning) = event.warning {
        interpretation.warnings.push(warning);
    }
    event.recognized
}

impl MidiSysexEventSummary {
    fn new(index: usize, sysex: &[u8], event: &SysexInterpretationEvent) -> Self {
        let payload = normalized_sysex_payload(sysex);
        Self {
            index,
            status: sysex.first().copied().unwrap_or(0),
            byte_len: sysex.len(),
            manufacturer_id: manufacturer_id_field(payload),
            recognized: event.recognized,
            system_mode: event.system_mode,
            channel_role: event.channel_role,
            warning: event.warning.clone(),
            bytes_hex: bytes_hex(sysex),
        }
    }
}

fn manufacturer_id_field(payload: &[u8]) -> String {
    match payload {
        [0x00, high, low, ..] => format!("00 {high:02X} {low:02X}"),
        [manufacturer_id, ..] => format!("{manufacturer_id:02X}"),
        [] => "-".to_owned(),
    }
}

fn bytes_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized_sysex_payload(sysex: &[u8]) -> &[u8] {
    let without_status = match sysex.first() {
        Some(0xF0 | 0xF7) => &sysex[1..],
        _ => sysex,
    };
    match without_status.last() {
        Some(0xF7) => &without_status[..without_status.len() - 1],
        _ => without_status,
    }
}

fn roland_checksum_valid(address: &[u8], data: &[u8], checksum: u8) -> bool {
    let sum = address
        .iter()
        .chain(data.iter())
        .fold(0u16, |acc, value| acc + *value as u16);
    let expected = ((128 - (sum % 128)) % 128) as u8;
    expected == checksum
}

impl SysexInterpretationEvent {
    fn unrecognized() -> Self {
        Self {
            system_mode: None,
            channel_role: None,
            warning: None,
            recognized: false,
        }
    }

    fn system_mode(system_mode: MidiSystemMode) -> Self {
        Self {
            system_mode: Some(system_mode),
            channel_role: None,
            warning: None,
            recognized: true,
        }
    }

    fn channel_role(channel: u8, role: MidiChannelRole, system_mode: MidiSystemMode) -> Self {
        Self {
            system_mode: Some(system_mode),
            channel_role: Some((channel, role)),
            warning: None,
            recognized: true,
        }
    }
}
