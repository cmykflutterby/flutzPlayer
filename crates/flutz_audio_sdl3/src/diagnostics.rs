use crate::{AudioCallbackStats, AudioDeviceConfig, AudioDeviceState, RingBufferConfig};

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct RenderAheadTarget {
    pub target_frames: u32,
    pub low_water_frames: u32,
    pub high_water_frames: u32,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct RenderAheadState {
    pub effective_target_frames: u32,
    pub current_buffered_frames: u32,
    pub available_frames: u32,
    pub largest_callback_frames: u32,
    pub underrun_count: u64,
    pub queue_error_count: u64,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct UnderrunReport {
    pub total_underruns: u64,
    pub last_missing_frames: u32,
}

impl UnderrunReport {
    pub fn from_counts(total_underruns: u64, frames_requested: u64, frames_delivered: u64) -> Self {
        Self {
            total_underruns,
            last_missing_frames: frames_requested.saturating_sub(frames_delivered) as u32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFormatDiagnostics {
    pub format: &'static str,
    pub channels: u16,
    pub sample_rate: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceDiagnostics {
    pub requested_device: &'static str,
    pub opened_device_id: u32,
    pub opened_device_name: Option<String>,
    pub requested_format: AudioFormatDiagnostics,
    pub stream_input_format: Option<AudioFormatDiagnostics>,
    pub stream_output_format: Option<AudioFormatDiagnostics>,
    pub internal_block_frames: u16,
    pub ring_buffer: RingBufferConfig,
    pub ring_capacity_frames: usize,
    pub ring_available_frames: usize,
    pub ring_retained_bytes: usize,
    pub device_buffer_frames: usize,
    pub device_available_frames: usize,
    pub callback_scratch_bytes: usize,
    pub producer_render_block_bytes: u64,
    pub state: AudioDeviceState,
    pub shutdown_started: bool,
    pub stats: AudioCallbackStats,
    pub sdl_lifecycle: AudioSdlLifecycleStats,
    pub references: AudioReferenceDiagnostics,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct AudioSdlLifecycleStats {
    pub init_calls: u64,
    pub quit_calls: u64,
    pub stream_opens: u64,
    pub stream_destroys: u64,
    pub stream_resumes: u64,
    pub stream_pauses: u64,
    pub callback_states_created: u64,
    pub callback_states_dropped: u64,
    pub producer_threads_started: u64,
    pub producer_threads_finished: u64,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct AudioReferenceDiagnostics {
    pub stream_id: u64,
    pub shared_strong_count: usize,
    pub shared_weak_count: usize,
    pub callback_shared_strong_count: usize,
    pub callback_shared_weak_count: usize,
}

impl AudioFormatDiagnostics {
    pub fn f32_stereo(sample_rate: u32, channels: u16) -> Self {
        Self {
            format: "f32",
            channels,
            sample_rate,
        }
    }
}

impl AudioDeviceDiagnostics {
    pub fn from_config(
        config: AudioDeviceConfig,
        state: AudioDeviceState,
        ring_capacity_frames: usize,
        ring_available_frames: usize,
        ring_retained_bytes: usize,
        callback_scratch_bytes: usize,
        shutdown_started: bool,
        stats: AudioCallbackStats,
        sdl_lifecycle: AudioSdlLifecycleStats,
        references: AudioReferenceDiagnostics,
    ) -> Self {
        Self {
            requested_device: "default playback",
            opened_device_id: 0,
            opened_device_name: None,
            requested_format: AudioFormatDiagnostics::f32_stereo(
                config.sample_rate,
                config.channels,
            ),
            stream_input_format: None,
            stream_output_format: None,
            internal_block_frames: config.internal_block_frames,
            ring_buffer: config.ring_buffer,
            ring_capacity_frames,
            ring_available_frames,
            ring_retained_bytes,
            device_buffer_frames: 0,
            device_available_frames: 0,
            callback_scratch_bytes,
            producer_render_block_bytes: stats.producer_render_block_bytes,
            state,
            shutdown_started,
            stats,
            sdl_lifecycle,
            references,
        }
    }
}
