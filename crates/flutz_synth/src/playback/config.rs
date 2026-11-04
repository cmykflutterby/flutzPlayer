use super::*;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum PlaybackLoopMode {
    #[default]
    None,
    Infinite,
    Counted,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PlaybackLoopSettings {
    pub enabled: bool,
    pub mode: PlaybackLoopMode,
    pub start_tick: u64,
    pub end_tick: u64,
    pub loop_count: u32,
}

impl Default for PlaybackLoopSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: PlaybackLoopMode::None,
            start_tick: 0,
            end_tick: 0,
            loop_count: 1,
        }
    }
}

impl PlaybackLoopSettings {
    pub(super) fn as_sequencer_settings(self) -> SequencerLoopSettings {
        SequencerLoopSettings {
            enabled: self.enabled,
            mode: match self.mode {
                PlaybackLoopMode::None => SequencerLoopMode::None,
                PlaybackLoopMode::Infinite => SequencerLoopMode::Infinite,
                PlaybackLoopMode::Counted => SequencerLoopMode::Counted,
            },
            start_tick: self.start_tick.min(i32::MAX as u64) as i32,
            end_tick: self.end_tick.min(i32::MAX as u64) as i32,
            loop_count: self.loop_count.max(1),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PlaybackConfig {
    pub sample_rate: u32,
    pub block_frames: usize,
    pub maximum_polyphony: usize,
    pub enable_reverb_and_chorus: bool,
    pub reposition_preroll_ms: u32,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            block_frames: 512,
            maximum_polyphony: 128,
            enable_reverb_and_chorus: true,
            reposition_preroll_ms: 750,
        }
    }
}
