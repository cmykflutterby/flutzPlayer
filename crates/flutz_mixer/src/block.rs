use super::engine::{meter_from_frames, meter_from_split_frames};
use super::*;

#[derive(Debug, Copy, Clone, Default, PartialEq)]
pub struct StereoFrame {
    pub left: f32,
    pub right: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioBlock {
    pub frames: Vec<StereoFrame>,
}

impl AudioBlock {
    pub fn silence(frame_count: usize) -> Self {
        Self {
            frames: vec![StereoFrame::default(); frame_count],
        }
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn meter(&self) -> MeterReading {
        meter_from_frames(&self.frames)
    }

    pub fn view(&self) -> AudioBlockView<'_> {
        AudioBlockView::Stereo(&self.frames)
    }
}

#[derive(Debug, Copy, Clone)]
pub enum AudioBlockView<'a> {
    Stereo(&'a [StereoFrame]),
    Split {
        left: &'a [f32],
        right: &'a [f32],
        gain: f32,
    },
    Silence {
        frame_count: usize,
    },
}

impl<'a> AudioBlockView<'a> {
    pub fn frame_count(&self) -> usize {
        match self {
            Self::Stereo(frames) => frames.len(),
            Self::Split { left, .. } => left.len(),
            Self::Silence { frame_count } => *frame_count,
        }
    }

    pub fn valid(&self) -> bool {
        match self {
            Self::Stereo(_) | Self::Silence { .. } => true,
            Self::Split { left, right, .. } => left.len() == right.len(),
        }
    }

    pub fn frame(&self, index: usize) -> StereoFrame {
        match self {
            Self::Stereo(frames) => frames[index],
            Self::Split { left, right, gain } => StereoFrame {
                left: left[index] * *gain,
                right: right[index] * *gain,
            },
            Self::Silence { .. } => StereoFrame::default(),
        }
    }

    pub fn meter(&self) -> MeterReading {
        match self {
            Self::Stereo(frames) => meter_from_frames(frames),
            Self::Split { left, right, gain } => meter_from_split_frames(left, right, *gain),
            Self::Silence { .. } => MeterReading::default(),
        }
    }

    pub fn is_silence(&self) -> bool {
        matches!(self, Self::Silence { .. })
    }
}
