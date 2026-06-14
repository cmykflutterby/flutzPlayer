#![allow(unsafe_code)]

pub mod callback;
pub mod device;
pub mod diagnostics;
pub mod ring_buffer;
pub mod stream;

pub use callback::{AudioCallbackCounters, AudioCallbackStats};
pub use device::{AudioDeviceConfig, AudioDeviceState, AudioMemoryHooks, AudioThreadBinder};
pub use diagnostics::{
    AudioDeviceDiagnostics, AudioFormatDiagnostics, AudioReferenceDiagnostics,
    AudioSdlLifecycleStats, RenderAheadState, RenderAheadTarget, UnderrunReport,
};
pub use ring_buffer::{AudioRingBuffer, RingBufferConfig};
pub use stream::SdlAudioOutput;
