use std::sync::Arc;

use crate::RingBufferConfig;

pub type AudioThreadBinder = Arc<dyn Fn() -> Box<dyn Send> + Send + Sync>;

#[derive(Clone, Default)]
pub struct AudioMemoryHooks {
    pub producer: Option<AudioThreadBinder>,
    pub callback: Option<AudioThreadBinder>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct AudioDeviceConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub internal_block_frames: u16,
    pub ring_buffer: RingBufferConfig,
}

impl Default for AudioDeviceConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            internal_block_frames: 512,
            ring_buffer: RingBufferConfig::default(),
        }
    }
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum AudioDeviceState {
    #[default]
    Closed,
    Open,
    Running,
}
