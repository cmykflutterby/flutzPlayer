use super::super::*;

#[derive(Debug, Clone, Default, PartialEq)]
pub(in crate::app) struct MixerEditSnapshot {
    pub(in crate::app) master: MasterControls,
    pub(in crate::app) smart_mix: SmartMixControls,
    pub(in crate::app) rows: Vec<RowEditSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::app) struct RowEditSnapshot {
    pub(in crate::app) internal_id: String,
    pub(in crate::app) muted: bool,
    pub(in crate::app) soloed: bool,
    pub(in crate::app) strips: Vec<StripEditSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::app) struct StripEditSnapshot {
    pub(in crate::app) channel: u8,
    pub(in crate::app) bank: u16,
    pub(in crate::app) program: u8,
    pub(in crate::app) is_percussion: bool,
    pub(in crate::app) volume: f32,
    pub(in crate::app) muted: bool,
    pub(in crate::app) soloed: bool,
    pub(in crate::app) pan: f32,
    pub(in crate::app) gain_db: f32,
    pub(in crate::app) limiter_enabled: bool,
    pub(in crate::app) limiter_amount: f32,
    pub(in crate::app) reverb: f32,
    pub(in crate::app) chorus: f32,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(in crate::app) struct PlaybackReloadResumeState {
    pub(in crate::app) run_state: AppRunState,
    pub(in crate::app) transport_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::app) struct RealtimeStripKey {
    pub(in crate::app) soundfont_id: String,
    channel: u8,
    bank: u16,
    program: u8,
    is_percussion: bool,
}

impl RealtimeStripKey {
    pub(in crate::app) fn from_snapshot(snapshot: &crate::playback::RealtimeStripSnapshot) -> Self {
        Self {
            soundfont_id: snapshot.soundfont_id.clone(),
            channel: snapshot.midi_channel,
            bank: snapshot.midi_bank,
            program: snapshot.midi_program,
            is_percussion: snapshot.is_percussion,
        }
    }
}

// Mixer Final Volume Values
pub const MASTER_VOLUME_MAX_DB: f32 = 32.0; // 24
pub const FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER: f32 = 10.0; // 2

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum MixerAssignmentMode {
    #[default]
    Manual,
    Balance,
    Layer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MasterControls {
    pub volume_db: f32,
    pub limiter_enabled: bool,
    pub limiter_amount: f32,
    pub reverb: f32,
    pub chorus: f32,
    pub eq_low: f32,
    pub eq_mid: f32,
    pub eq_high: f32,
}

impl Default for MasterControls {
    fn default() -> Self {
        Self {
            volume_db: -6.0,
            limiter_enabled: false,
            limiter_amount: 0.25,
            reverb: 0.0,
            chorus: 0.0,
            eq_low: 0.0,
            eq_mid: 0.0,
            eq_high: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SmartMixControls {
    pub enabled: bool,
    pub auto_normalize: bool,
    pub target_headroom_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub lookahead_ms: f32,
    pub normalization_amount: f32,
}

impl Default for SmartMixControls {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_normalize: false,
            target_headroom_db: -6.0,
            attack_ms: 1.0,
            release_ms: 10.0,
            lookahead_ms: 500.0,
            normalization_amount: 25.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SoundFontUiRow {
    pub internal_id: String,
    pub display_name: String,
    pub is_default: bool,
    pub collapsed: bool,
    pub muted: bool,
    pub soloed: bool,
    pub strips: Vec<MixerStripUiState>,
}

impl SoundFontUiRow {
    pub(in crate::app) fn from_catalog_entry(entry: &SoundFontCatalogEntry) -> Self {
        Self {
            internal_id: entry.internal_id.clone(),
            display_name: entry.display_name.clone(),
            is_default: entry.is_default,
            collapsed: true,
            muted: false,
            soloed: false,
            strips: Vec::new(),
        }
    }

    pub(in crate::app) fn default_retro() -> Self {
        Self {
            internal_id: "retro_gm".to_owned(),
            display_name: "Retro GM".to_owned(),
            is_default: true,
            collapsed: true,
            muted: false,
            soloed: false,
            strips: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MixerStripUiState {
    pub strip_id: StripId,
    pub channel: u8,
    pub bank: u16,
    pub program: u8,
    pub is_percussion: bool,
    pub program_name: String,
    pub volume: f32,
    pub muted: bool,
    pub unsupported: bool,
    pub mute_policy_default: bool,
    pub volume_policy_default: bool,
    pub soloed: bool,
    pub pan: f32,
    pub gain_db: f32,
    pub limiter_enabled: bool,
    pub limiter_amount: f32,
    pub reverb: f32,
    pub chorus: f32,
    pub meter: f32,
    pub active: bool,
    pub current_note: String,
}

impl MixerStripUiState {
    #[allow(dead_code)]
    fn for_channel(soundfont_index: usize, channel_index: usize) -> Self {
        let channel = channel_index as u8;
        Self {
            strip_id: strip_id_for(soundfont_index, channel, 0, 0, channel == 9),
            channel,
            bank: 0,
            program: 0,
            is_percussion: channel == 9,
            program_name: if channel == 9 {
                "Percussion".to_owned()
            } else {
                "Acoustic Grand".to_owned()
            },
            volume: 1.0,
            muted: false,
            unsupported: false,
            mute_policy_default: true,
            volume_policy_default: true,
            soloed: false,
            pan: 0.0,
            gain_db: 0.0,
            limiter_enabled: false,
            limiter_amount: 0.25,
            reverb: 0.0,
            chorus: 0.0,
            meter: 0.0,
            active: false,
            current_note: "--".to_owned(),
        }
    }
}

pub(in crate::app) fn coverage_supports_strip(
    coverage: Option<&SoundFontCoverage>,
    strip: &MixerStripUiState,
) -> bool {
    let Some(coverage) = coverage else {
        return false;
    };

    if strip.is_percussion {
        coverage.provides_percussion()
    } else {
        coverage.provides_melodic(strip.bank, strip.program)
    }
}

pub(in crate::app) fn strip_id_for(
    soundfont_index: usize,
    channel: u8,
    bank: u16,
    program: u8,
    is_percussion: bool,
) -> StripId {
    let soundfont_slot = (soundfont_index as u64 + 1) * 100_000;
    let channel_slot = channel as u64 * 16_384;
    let bank_slot = bank as u64 * 128;
    let percussion_slot = if is_percussion { 50_000 } else { 0 };
    StripId(soundfont_slot + percussion_slot + channel_slot + bank_slot + program as u64 + 1)
}

pub(in crate::app) fn write_note_name(target: &mut String, note: u8) {
    use std::fmt::Write as _;
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = note as i16 / 12 - 1;
    target.clear();
    let _ = write!(target, "{}{}", NAMES[note as usize % 12], octave);
}

pub(in crate::app) fn set_text_in_place(target: &mut String, value: &str) {
    if target != value {
        target.clear();
        target.push_str(value);
    }
}

pub(in crate::app) fn non_empty_media_metadata(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub(in crate::app) fn soundfont_rows_from_catalog(
    catalog_soundfonts: &[SoundFontCatalogEntry],
    ids: &[&str],
) -> Vec<SoundFontUiRow> {
    if ids.is_empty() {
        return vec![catalog_soundfonts
            .iter()
            .find(|font| font.is_default)
            .or_else(|| catalog_soundfonts.first())
            .map(SoundFontUiRow::from_catalog_entry)
            .unwrap_or_else(SoundFontUiRow::default_retro)];
    }

    ids.iter()
        .map(|id| {
            catalog_soundfonts
                .iter()
                .find(|entry| entry.internal_id == *id)
                .map(SoundFontUiRow::from_catalog_entry)
                .unwrap_or_else(|| SoundFontUiRow {
                    internal_id: (*id).to_owned(),
                    display_name: (*id).to_owned(),
                    is_default: false,
                    collapsed: true,
                    muted: false,
                    soloed: false,
                    strips: Vec::new(),
                })
        })
        .collect()
}
