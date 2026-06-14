use std::fmt;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum StemRenderMode {
    WholeSoundFont,
    MidiChannel,
    MidiProgram,
    Percussion,
    ChannelProgram,
}

impl Default for StemRenderMode {
    fn default() -> Self {
        Self::WholeSoundFont
    }
}

impl StemRenderMode {
    pub fn requires_voice_grouping(self) -> bool {
        !matches!(self, Self::WholeSoundFont)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemRenderRequest {
    pub soundfont_id: String,
    pub modes: Vec<StemRenderMode>,
}

impl StemRenderRequest {
    pub fn whole_soundfont(soundfont_id: impl Into<String>) -> Self {
        Self {
            soundfont_id: soundfont_id.into(),
            modes: vec![StemRenderMode::WholeSoundFont],
        }
    }

    pub fn channel_program(soundfont_id: impl Into<String>) -> Self {
        Self {
            soundfont_id: soundfont_id.into(),
            modes: vec![StemRenderMode::ChannelProgram],
        }
    }

    pub fn with_modes(
        soundfont_id: impl Into<String>,
        modes: impl IntoIterator<Item = StemRenderMode>,
    ) -> Self {
        let modes = modes.into_iter().collect::<Vec<_>>();
        Self {
            soundfont_id: soundfont_id.into(),
            modes,
        }
    }

    pub fn requires_voice_grouping(&self) -> bool {
        self.modes.iter().any(|mode| mode.requires_voice_grouping())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StemIdentity {
    pub soundfont_id: String,
    pub midi_channel: Option<u8>,
    pub midi_bank: Option<u16>,
    pub midi_program: Option<u8>,
    pub is_percussion: bool,
}

impl StemIdentity {
    pub fn whole_soundfont(soundfont_id: impl Into<String>) -> Self {
        Self {
            soundfont_id: soundfont_id.into(),
            midi_channel: None,
            midi_bank: None,
            midi_program: None,
            is_percussion: false,
        }
    }

    pub fn midi_channel(soundfont_id: impl Into<String>, midi_channel: u8) -> Self {
        Self {
            soundfont_id: soundfont_id.into(),
            midi_channel: Some(midi_channel),
            midi_bank: None,
            midi_program: None,
            is_percussion: midi_channel == 9,
        }
    }

    pub fn midi_program(soundfont_id: impl Into<String>, midi_program: u8) -> Self {
        Self {
            soundfont_id: soundfont_id.into(),
            midi_channel: None,
            midi_bank: None,
            midi_program: Some(midi_program),
            is_percussion: false,
        }
    }

    pub fn percussion(soundfont_id: impl Into<String>) -> Self {
        Self::percussion_channel(soundfont_id, 9)
    }

    pub fn percussion_channel(soundfont_id: impl Into<String>, midi_channel: u8) -> Self {
        Self {
            soundfont_id: soundfont_id.into(),
            midi_channel: Some(midi_channel),
            midi_bank: Some(128),
            midi_program: None,
            is_percussion: true,
        }
    }

    pub fn channel_program(
        soundfont_id: impl Into<String>,
        midi_channel: u8,
        midi_program: u8,
        is_percussion: bool,
    ) -> Self {
        Self::channel_bank_program(
            soundfont_id,
            midi_channel,
            None,
            midi_program,
            is_percussion,
        )
    }

    pub fn channel_bank_program(
        soundfont_id: impl Into<String>,
        midi_channel: u8,
        midi_bank: Option<u16>,
        midi_program: u8,
        is_percussion: bool,
    ) -> Self {
        Self {
            soundfont_id: soundfont_id.into(),
            midi_channel: Some(midi_channel),
            midi_bank,
            midi_program: Some(midi_program),
            is_percussion,
        }
    }

    pub fn with_bank(mut self, midi_bank: Option<u16>) -> Self {
        self.midi_bank = midi_bank;
        self
    }

    pub fn global_effects(soundfont_id: impl Into<String>) -> Self {
        Self::whole_soundfont(soundfont_id)
    }

    pub fn mode(&self) -> StemRenderMode {
        match (self.midi_channel, self.midi_program, self.is_percussion) {
            (None, None, false) => StemRenderMode::WholeSoundFont,
            (Some(9), None, true) => StemRenderMode::Percussion,
            (Some(_), None, _) => StemRenderMode::MidiChannel,
            (None, None, true) => StemRenderMode::Percussion,
            (None, Some(_), false) => StemRenderMode::MidiProgram,
            (Some(_), Some(_), _) => StemRenderMode::ChannelProgram,
            (None, Some(_), true) => StemRenderMode::Percussion,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StemRenderBlock {
    pub identity: StemIdentity,
    pub display_name: Option<String>,
    pub active_notes: Vec<u8>,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl StemRenderBlock {
    pub fn new(identity: StemIdentity, left: Vec<f32>, right: Vec<f32>) -> Result<Self, StemError> {
        let block = Self {
            identity,
            display_name: None,
            active_notes: Vec::new(),
            left,
            right,
        };
        block.validate()?;
        Ok(block)
    }

    pub fn whole_soundfont(
        soundfont_id: impl Into<String>,
        left: Vec<f32>,
        right: Vec<f32>,
    ) -> Self {
        Self {
            identity: StemIdentity::whole_soundfont(soundfont_id),
            display_name: None,
            active_notes: Vec::new(),
            left,
            right,
        }
    }

    pub fn whole_soundfont_named(
        soundfont_id: impl Into<String>,
        display_name: impl Into<String>,
        left: Vec<f32>,
        right: Vec<f32>,
    ) -> Self {
        Self {
            identity: StemIdentity::whole_soundfont(soundfont_id),
            display_name: Some(display_name.into()),
            active_notes: Vec::new(),
            left,
            right,
        }
    }

    pub fn channel_program(
        soundfont_id: impl Into<String>,
        midi_channel: u8,
        midi_program: u8,
        is_percussion: bool,
        left: Vec<f32>,
        right: Vec<f32>,
    ) -> Self {
        Self {
            identity: StemIdentity::channel_program(
                soundfont_id,
                midi_channel,
                midi_program,
                is_percussion,
            ),
            display_name: None,
            active_notes: Vec::new(),
            left,
            right,
        }
    }

    pub fn midi_channel(
        soundfont_id: impl Into<String>,
        midi_channel: u8,
        left: Vec<f32>,
        right: Vec<f32>,
    ) -> Self {
        Self {
            identity: StemIdentity::midi_channel(soundfont_id, midi_channel),
            display_name: None,
            active_notes: Vec::new(),
            left,
            right,
        }
    }

    pub fn midi_program(
        soundfont_id: impl Into<String>,
        midi_program: u8,
        left: Vec<f32>,
        right: Vec<f32>,
    ) -> Self {
        Self {
            identity: StemIdentity::midi_program(soundfont_id, midi_program),
            display_name: None,
            active_notes: Vec::new(),
            left,
            right,
        }
    }

    pub fn percussion(soundfont_id: impl Into<String>, left: Vec<f32>, right: Vec<f32>) -> Self {
        Self::percussion_channel(soundfont_id, 9, left, right)
    }

    pub fn percussion_channel(
        soundfont_id: impl Into<String>,
        midi_channel: u8,
        left: Vec<f32>,
        right: Vec<f32>,
    ) -> Self {
        Self {
            identity: StemIdentity::percussion_channel(soundfont_id, midi_channel),
            display_name: None,
            active_notes: Vec::new(),
            left,
            right,
        }
    }

    pub fn with_active_note(mut self, note: u8) -> Self {
        if !self.active_notes.contains(&note) {
            self.active_notes.push(note);
        }
        self
    }

    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }

    pub fn validate(&self) -> Result<(), StemError> {
        if self.left.len() != self.right.len() {
            return Err(StemError::MismatchedChannels {
                left_frames: self.left.len(),
                right_frames: self.right.len(),
            });
        }
        Ok(())
    }

    pub fn frame_count(&self) -> usize {
        self.left.len().min(self.right.len())
    }

    pub fn is_empty(&self) -> bool {
        self.frame_count() == 0
    }

    pub fn copy_to_interleaved(&self, output: &mut [f32]) -> Result<(), StemError> {
        self.validate()?;
        let expected_samples = self.left.len() * 2;
        if output.len() != expected_samples {
            return Err(StemError::InterleavedLengthMismatch {
                expected_samples,
                actual_samples: output.len(),
            });
        }
        for frame in 0..self.left.len() {
            output[frame * 2] = self.left[frame];
            output[frame * 2 + 1] = self.right[frame];
        }
        Ok(())
    }

    pub fn to_interleaved(&self) -> Result<Vec<f32>, StemError> {
        let mut output = vec![0.0; self.frame_count() * 2];
        self.copy_to_interleaved(&mut output)?;
        Ok(output)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StemRenderSet {
    pub frame_count: usize,
    pub blocks: Vec<StemRenderBlock>,
}

impl StemRenderSet {
    pub fn new(frame_count: usize, blocks: Vec<StemRenderBlock>) -> Result<Self, StemError> {
        let set = Self {
            frame_count,
            blocks,
        };
        set.validate()?;
        Ok(set)
    }

    pub fn validate(&self) -> Result<(), StemError> {
        for block in &self.blocks {
            block.validate()?;
            if block.frame_count() != self.frame_count {
                return Err(StemError::FrameCountMismatch {
                    expected_frames: self.frame_count,
                    actual_frames: block.frame_count(),
                });
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() || self.frame_count == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StemError {
    MismatchedChannels {
        left_frames: usize,
        right_frames: usize,
    },
    FrameCountMismatch {
        expected_frames: usize,
        actual_frames: usize,
    },
    InterleavedLengthMismatch {
        expected_samples: usize,
        actual_samples: usize,
    },
}

impl fmt::Display for StemError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MismatchedChannels {
                left_frames,
                right_frames,
            } => write!(
                formatter,
                "stem block channel length mismatch: left={left_frames}, right={right_frames}"
            ),
            Self::FrameCountMismatch {
                expected_frames,
                actual_frames,
            } => write!(
                formatter,
                "stem block frame count mismatch: expected={expected_frames}, actual={actual_frames}"
            ),
            Self::InterleavedLengthMismatch {
                expected_samples,
                actual_samples,
            } => write!(
                formatter,
                "interleaved output length mismatch: expected={expected_samples}, actual={actual_samples}"
            ),
        }
    }
}

impl std::error::Error for StemError {}
