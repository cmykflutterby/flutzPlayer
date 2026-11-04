use super::super::*;

#[derive(Debug, Clone, Default)]
pub struct MidiTransportMetadata {
    pub duration_seconds: f64,
    pub tick_length: u64,
    pub jump_start_tick: Option<u64>,
    pub jump_end_tick: Option<u64>,
    pub jump_points: Vec<u64>,
}

impl MidiTransportMetadata {
    pub(in crate::playback) fn from_loaded_midi(loaded_midi: &LoadedMidi) -> Self {
        let mut jump_points = loaded_midi.loop_start_ticks();
        jump_points.extend(loaded_midi.loop_end_ticks());
        jump_points.sort_unstable();
        jump_points.dedup();

        let jump_start_tick = loaded_midi.loop_start_ticks().into_iter().next();
        let jump_end_tick = loaded_midi.loop_end_ticks().into_iter().next();

        Self {
            duration_seconds: loaded_midi.duration_seconds(),
            tick_length: loaded_midi.tick_length(),
            jump_start_tick,
            jump_end_tick,
            jump_points,
        }
    }
}

pub struct RenderProbeReport {
    pub frames: usize,
    pub samples: usize,
    pub peak: f32,
    pub soundfont_count: usize,
    pub midi_strip_count: usize,
    pub recovered_error_count: usize,
}

pub struct RenderStemProbeReport {
    pub report: RenderProbeReport,
    pub stems: Vec<StemRenderBlock>,
    pub mix_diagnostics: Vec<RenderProbeStripMixDiagnostics>,
}

#[derive(Debug, Clone)]
pub struct RenderProbeStripMixDiagnostics {
    pub strip_id: StripId,
    pub soundfont_id: String,
    pub display_name: Option<String>,
    pub midi_channel: u8,
    pub midi_program: u8,
    pub is_percussion: bool,
    pub input_meter: MeterReading,
    pub estimated_post_peak: f32,
    pub estimated_post_rms: f32,
    pub applied_gain: f32,
    pub session_gain: f32,
    pub smart_mix_gain: f32,
    pub lookahead_gain: f32,
    pub normalization_gain: f32,
    pub left_pan_gain: f32,
    pub right_pan_gain: f32,
    pub automatic_processing: bool,
    pub mute: bool,
    pub solo: bool,
    pub audible: bool,
    pub active_notes: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackMidiScanDiagnostics {
    pub loop_style: String,
    pub system_modes: Vec<String>,
    pub percussion_channels: Vec<u8>,
    pub warnings: Vec<String>,
    pub sysex_event_count: usize,
    pub recognized_sysex_event_count: usize,
    pub sysex_events: Vec<PlaybackMidiSysexEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackMidiSysexEvent {
    pub index: usize,
    pub status: u8,
    pub byte_len: usize,
    pub manufacturer_id: String,
    pub recognized: bool,
    pub system_mode: Option<String>,
    pub channel_role: Option<PlaybackMidiChannelRoleChange>,
    pub warning: Option<String>,
    pub bytes_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackMidiChannelRoleChange {
    pub channel: u8,
    pub role: String,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MidiStripDescriptor {
    pub channel: u8,
    pub bank: u16,
    pub program: u8,
    pub is_percussion: bool,
}

impl Default for MidiStripDescriptor {
    fn default() -> Self {
        Self {
            channel: 0,
            bank: 0,
            program: 0,
            is_percussion: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiDemandProfile {
    pub channel_roles: Vec<MidiStripDescriptor>,
    pub melodic_programs: Vec<BankProgramDemand>,
    pub percussion_channels: Vec<u8>,
}

impl MidiDemandProfile {
    fn from_channel_roles(channel_roles: Vec<MidiStripDescriptor>) -> Self {
        let melodic_programs = channel_roles
            .iter()
            .filter(|role| !role.is_percussion)
            .map(|role| BankProgramDemand {
                bank: role.bank,
                program: role.program,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let percussion_channels = channel_roles
            .iter()
            .filter(|role| role.is_percussion)
            .map(|role| role.channel)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        Self {
            channel_roles,
            melodic_programs,
            percussion_channels,
        }
    }

    pub fn role_count(&self) -> usize {
        self.channel_roles.len()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BankProgramDemand {
    pub bank: u16,
    pub program: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontDemandSignature {
    pub soundfont_id: String,
    pub preset_ids: Vec<u32>,
    pub instrument_ids: Vec<u32>,
    pub sample_ids: Vec<u32>,
    pub logical_wave_range_count: usize,
    pub full_font_fallback: bool,
}

impl SoundFontDemandSignature {
    pub(in crate::playback) fn from_subset_plan(plan: &SoundFontSubsetPlanDebug) -> Self {
        Self {
            soundfont_id: plan.soundfont_id.clone(),
            preset_ids: plan.preset_ids.clone(),
            instrument_ids: plan.instrument_ids.clone(),
            sample_ids: plan.sample_ids.clone(),
            logical_wave_range_count: plan.logical_wave_range_count,
            full_font_fallback: !plan.used_index_json,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontDemandDiagnostics {
    pub requested_soundfont_count: usize,
    pub loaded_provider_count: usize,
    pub pruned_soundfont_count: usize,
    pub demand_profile: MidiDemandProfile,
}

impl MidiStripDescriptor {
    #[allow(dead_code)]
    fn strip_id_for_soundfont(self, soundfont_index: usize) -> StripId {
        let soundfont_slot = (soundfont_index as u64 + 1) * 10_000;
        let channel_slot = self.channel as u64 * 16_384;
        let bank_slot = self.bank as u64 * 128;
        StripId(soundfont_slot + channel_slot + bank_slot + self.program as u64 + 1)
    }
}

#[derive(Debug, Clone)]
pub(in crate::playback) struct MidiFileScan {
    pub(in crate::playback) detected_loop_type: MidiFileLoopType,
    pub(in crate::playback) channel_roles: Vec<MidiStripDescriptor>,
    pub(in crate::playback) demand_profile: MidiDemandProfile,
    pub(in crate::playback) diagnostics: PlaybackMidiScanDiagnostics,
}

#[derive(Debug, Copy, Clone, Default)]
struct LoopMarkerStats {
    rpg_start_count: u32,
    im_start_count: u32,
    im_end_count: u32,
    ff_start_count: u32,
    ff_end_count: u32,
}

impl LoopMarkerStats {
    fn note_cc(&mut self, controller: u8) {
        match controller {
            110 => self.im_start_count += 1,
            111 => {
                self.rpg_start_count += 1;
                self.im_end_count += 1;
            }
            116 => self.ff_start_count += 1,
            117 => self.ff_end_count += 1,
            _ => {}
        }
    }

    fn detected_loop_type(self) -> MidiFileLoopType {
        let im_pair_count = self.im_start_count.min(self.im_end_count);
        let ff_pair_count = self.ff_start_count.min(self.ff_end_count);

        if im_pair_count > 0 && im_pair_count >= ff_pair_count {
            MidiFileLoopType::IncredibleMachine
        } else if ff_pair_count > 0 {
            MidiFileLoopType::FinalFantasy
        } else if self.rpg_start_count > 0 {
            MidiFileLoopType::RpgMaker
        } else {
            MidiFileLoopType::LoopPoint(0)
        }
    }
}

pub(in crate::playback) fn loop_type_label(loop_type: MidiFileLoopType) -> &'static str {
    match loop_type {
        MidiFileLoopType::LoopPoint(_) => "none",
        MidiFileLoopType::RpgMaker => "rpg-maker",
        MidiFileLoopType::IncredibleMachine => "incredible-machine",
        MidiFileLoopType::FinalFantasy => "final-fantasy",
        _ => "other",
    }
}

pub(in crate::playback) fn analyze_midi_file(bytes: &[u8]) -> Result<MidiFileScan> {
    if bytes.len() < 14 || !bytes.starts_with(b"MThd") {
        return Err(FlutzError::InvalidInput(
            "missing standard MIDI MThd header".to_owned(),
        ));
    }

    let header_len = read_be_u32(bytes, 4)? as usize;
    if header_len < 6 || bytes.len() < 8 + header_len {
        return Err(FlutzError::InvalidInput(
            "invalid MIDI header length".to_owned(),
        ));
    }
    let track_count = read_be_u16(bytes, 10)? as usize;
    let mut offset = 8 + header_len;
    let mut loop_markers = LoopMarkerStats::default();

    for _ in 0..track_count {
        if offset + 8 > bytes.len() {
            break;
        }
        if &bytes[offset..offset + 4] != b"MTrk" {
            return Err(FlutzError::InvalidInput(
                "expected MIDI MTrk chunk".to_owned(),
            ));
        }
        let track_len = read_be_u32(bytes, offset + 4)? as usize;
        offset += 8;
        let track_end = offset.saturating_add(track_len).min(bytes.len());
        analyze_track(&bytes[offset..track_end], &mut loop_markers)?;
        offset = track_end;
    }

    let detected_loop_type = loop_markers.detected_loop_type();
    let mut cursor = Cursor::new(bytes);
    let midi_file =
        MidiFile::new_with_loop_type(&mut cursor, detected_loop_type).map_err(|error| {
            FlutzError::InvalidInput(format!("failed to parse MIDI roles: {error:?}"))
        })?;
    let channel_roles = midi_file
        .get_channel_program_roles()
        .into_iter()
        .map(|role| MidiStripDescriptor {
            channel: role.channel,
            bank: role.bank,
            program: role.program,
            is_percussion: role.is_percussion,
        })
        .collect::<Vec<_>>();
    let demand_profile = MidiDemandProfile::from_channel_roles(channel_roles.clone());
    let diagnostics =
        PlaybackMidiScanDiagnostics::from_parts(detected_loop_type, midi_file.get_interpretation());

    Ok(MidiFileScan {
        detected_loop_type,
        channel_roles,
        demand_profile,
        diagnostics,
    })
}

impl PlaybackMidiScanDiagnostics {
    fn from_parts(loop_type: MidiFileLoopType, interpretation: &MidiInterpretation) -> Self {
        Self {
            loop_style: loop_type_label(loop_type).to_owned(),
            system_modes: interpretation
                .system_modes
                .iter()
                .map(|mode| midi_system_mode_label(*mode).to_owned())
                .collect(),
            percussion_channels: interpretation.percussion_channels.iter().copied().collect(),
            warnings: interpretation.warnings.clone(),
            sysex_event_count: interpretation.sysex_event_count,
            recognized_sysex_event_count: interpretation.recognized_sysex_event_count,
            sysex_events: interpretation
                .sysex_events
                .iter()
                .map(|event| PlaybackMidiSysexEvent {
                    index: event.index,
                    status: event.status,
                    byte_len: event.byte_len,
                    manufacturer_id: event.manufacturer_id.clone(),
                    recognized: event.recognized,
                    system_mode: event
                        .system_mode
                        .map(|mode| midi_system_mode_label(mode).to_owned()),
                    channel_role: event.channel_role.map(|(channel, role)| {
                        PlaybackMidiChannelRoleChange {
                            channel,
                            role: format!("{:?}", role).to_ascii_lowercase(),
                        }
                    }),
                    warning: event.warning.clone(),
                    bytes_hex: event.bytes_hex.clone(),
                })
                .collect(),
        }
    }
}

fn midi_system_mode_label(mode: MidiSystemMode) -> &'static str {
    match mode {
        MidiSystemMode::GeneralMidi => "general-midi",
        MidiSystemMode::GeneralMidi2 => "general-midi-2",
        MidiSystemMode::RolandGs => "roland-gs",
        MidiSystemMode::YamahaXg => "yamaha-xg",
    }
}

pub(in crate::playback) fn midi_scan_trace_json(
    sequence: u64,
    midi_source: Option<&Path>,
    diagnostics: &PlaybackMidiScanDiagnostics,
) -> String {
    format!(
        "{{\"schema\":\"flutz.render_debug.midi_scan.v1\",\"sequence\":{sequence},\"midi_source\":{},\"midi_scan\":{}}}",
        json_option_string(midi_source.map(|path| path.display().to_string()).as_deref()),
        midi_scan_json(diagnostics)
    )
}

pub(in crate::playback) fn midi_scan_json(diagnostics: &PlaybackMidiScanDiagnostics) -> String {
    let sysex = diagnostics
        .sysex_events
        .iter()
        .map(|event| {
            format!(
                "{{\"index\":{},\"status\":{},\"byte_len\":{},\"manufacturer_id\":{},\"recognized\":{},\"system_mode\":{},\"channel_role\":{},\"warning\":{},\"bytes_hex\":{}}}",
                event.index,
                event.status,
                event.byte_len,
                json_string(&event.manufacturer_id),
                if event.recognized { "true" } else { "false" },
                json_option_string(event.system_mode.as_deref()),
                json_channel_role(event.channel_role.as_ref()),
                json_option_string(event.warning.as_deref()),
                json_string(&event.bytes_hex)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"loop_style\":{},\"system_modes\":{},\"percussion_channels\":{},\"warnings\":{},\"sysex_event_count\":{},\"recognized_sysex_event_count\":{},\"sysex_events\":[{}]}}",
        json_string(&diagnostics.loop_style),
        json_string_array(&diagnostics.system_modes),
        json_u8_array(&diagnostics.percussion_channels),
        json_string_array(&diagnostics.warnings),
        diagnostics.sysex_event_count,
        diagnostics.recognized_sysex_event_count,
        sysex
    )
}

fn json_channel_role(role: Option<&PlaybackMidiChannelRoleChange>) -> String {
    role.map(|role| {
        format!(
            "{{\"channel\":{},\"role\":{}}}",
            role.channel,
            json_string(&role.role)
        )
    })
    .unwrap_or_else(|| "null".to_owned())
}

pub(in crate::playback) fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                let _ = write!(escaped, "\\u{:04x}", ch as u32);
            }
            ch => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

pub(in crate::playback) fn json_option_string(value: Option<&str>) -> String {
    value.map(json_string).unwrap_or_else(|| "null".to_owned())
}

pub(in crate::playback) fn json_string_array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| json_string(value))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_u8_array(values: &[u8]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn analyze_track(track: &[u8], loop_markers: &mut LoopMarkerStats) -> Result<()> {
    let mut offset = 0usize;
    let mut running_status = None::<u8>;
    while offset < track.len() {
        read_var_len(track, &mut offset)?;
        if offset >= track.len() {
            break;
        }

        let first = track[offset];
        let status = if first & 0x80 != 0 {
            offset += 1;
            if first < 0xF0 {
                running_status = Some(first);
            }
            first
        } else {
            running_status.ok_or_else(|| {
                FlutzError::InvalidInput("MIDI running status used before status byte".to_owned())
            })?
        };

        match status {
            0xFF => {
                if offset >= track.len() {
                    break;
                }
                offset += 1;
                let len = read_var_len(track, &mut offset)? as usize;
                offset = offset.saturating_add(len).min(track.len());
            }
            0xF0 | 0xF7 => {
                let len = read_var_len(track, &mut offset)? as usize;
                offset = offset.saturating_add(len).min(track.len());
            }
            0x80..=0xEF => {
                let event = status & 0xF0;
                let data_len = if event == 0xC0 || event == 0xD0 { 1 } else { 2 };
                if offset + data_len > track.len() {
                    break;
                }
                let data_1 = track[offset];
                offset += data_len;

                match event {
                    0xB0 => {
                        loop_markers.note_cc(data_1);
                    }
                    _ => {}
                }
            }
            _ => break,
        }
    }
    Ok(())
}

fn read_var_len(bytes: &[u8], offset: &mut usize) -> Result<u32> {
    let mut value = 0u32;
    for _ in 0..4 {
        if *offset >= bytes.len() {
            return Err(FlutzError::InvalidInput(
                "unterminated MIDI variable length value".to_owned(),
            ));
        }
        let byte = bytes[*offset];
        *offset += 1;
        value = (value << 7) | (byte & 0x7F) as u32;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(FlutzError::InvalidInput(
        "MIDI variable length value is too long".to_owned(),
    ))
}

fn read_be_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    if offset + 2 > bytes.len() {
        return Err(FlutzError::InvalidInput("truncated MIDI u16".to_owned()));
    }
    Ok(u16::from_be_bytes([bytes[offset], bytes[offset + 1]]))
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    if offset + 4 > bytes.len() {
        return Err(FlutzError::InvalidInput("truncated MIDI u32".to_owned()));
    }
    Ok(u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}
