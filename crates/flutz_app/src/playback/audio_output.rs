use super::*;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum AudioBackend {
    #[default]
    Sdl3,
    #[cfg(windows)]
    Wasapi,
}

impl AudioBackend {
    pub(crate) fn supported_values() -> &'static str {
        if cfg!(windows) {
            "sdl3|wasapi"
        } else {
            "sdl3"
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Sdl3 => "sdl3",
            #[cfg(windows)]
            Self::Wasapi => "wasapi",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sdl3" => Ok(Self::Sdl3),
            #[cfg(windows)]
            "wasapi" => Ok(Self::Wasapi),
            _ => Err(FlutzError::InvalidInput(format!(
                "--audio-backend requires {}",
                Self::supported_values().replace('|', " or ")
            ))),
        }
    }
}

pub(super) enum AudioOutput {
    Sdl3(SdlAudioOutput),
    #[cfg(windows)]
    Wasapi(WasapiAudioOutput),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FluxGuardConfig {
    min_latency_ms: u32,
    initial_latency_ms: u32,
    max_latency_ms: u32,
    underrun_penalty_ms: u32,
    queue_error_penalty_ms: u32,
    low_water_headroom_ms: u32,
    stable_step_down_ms: u32,
    stable_observations_required: u32,
    cooldown_observations: u32,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum FluxGuardDecisionReason {
    #[default]
    NoAudio,
    Initialized,
    Stable,
    QueueErrorPressure,
    UnderrunPressure,
    LowWater,
    Cooldown,
}

impl FluxGuardDecisionReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoAudio => "no-audio",
            Self::Initialized => "initialized",
            Self::Stable => "stable",
            Self::QueueErrorPressure => "queue-error-pressure",
            Self::UnderrunPressure => "underrun-pressure",
            Self::LowWater => "low-water",
            Self::Cooldown => "cooldown",
        }
    }
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct FluxGuardDecision {
    pub target: RenderAheadTarget,
    pub state: RenderAheadState,
    pub reason: FluxGuardDecisionReason,
    pub changed: bool,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct FluxGuardSnapshot {
    pub decision: FluxGuardDecision,
    pub initialized: bool,
    pub stable_observations: u32,
    pub cooldown_observations: u32,
}

impl Default for FluxGuardConfig {
    fn default() -> Self {
        Self {
            min_latency_ms: 12,
            initial_latency_ms: 80,
            max_latency_ms: 250,
            underrun_penalty_ms: 40,
            queue_error_penalty_ms: 60,
            low_water_headroom_ms: 20,
            stable_step_down_ms: 5,
            stable_observations_required: 30,
            cooldown_observations: 8,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct FluxGuard {
    config: FluxGuardConfig,
    target_frames: u32,
    last_underrun_count: u64,
    last_queue_error_count: u64,
    stable_observations: u32,
    cooldown_observations: u32,
    initialized: bool,
    last_decision: FluxGuardDecision,
}

impl Default for FluxGuard {
    fn default() -> Self {
        Self {
            config: FluxGuardConfig::default(),
            target_frames: 0,
            last_underrun_count: 0,
            last_queue_error_count: 0,
            stable_observations: 0,
            cooldown_observations: 0,
            initialized: false,
            last_decision: FluxGuardDecision::default(),
        }
    }
}

impl FluxGuard {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn observe(
        &mut self,
        state: Option<RenderAheadState>,
        sample_rate: u32,
    ) -> FluxGuardSnapshot {
        let Some(state) = state else {
            self.last_decision = FluxGuardDecision::default();
            return self.snapshot();
        };

        let sample_rate = sample_rate.max(1);
        let min_frames = frames_from_ms(sample_rate, self.config.min_latency_ms);
        let initial_frames = frames_from_ms(sample_rate, self.config.initial_latency_ms);
        let max_frames = frames_from_ms(sample_rate, self.config.max_latency_ms).max(min_frames);
        if !self.initialized {
            self.initialized = true;
            self.target_frames = initial_frames.clamp(min_frames, max_frames);
        }

        let underrun_delta = state
            .underrun_count
            .saturating_sub(self.last_underrun_count);
        let queue_error_delta = state
            .queue_error_count
            .saturating_sub(self.last_queue_error_count);
        self.last_underrun_count = state.underrun_count;
        self.last_queue_error_count = state.queue_error_count;

        let callback_headroom = state
            .largest_callback_frames
            .saturating_mul(4)
            .max(min_frames);
        let mut proposed_target = self.target_frames.max(callback_headroom);
        let mut reason = FluxGuardDecisionReason::Stable;

        if queue_error_delta > 0 {
            reason = FluxGuardDecisionReason::QueueErrorPressure;
            proposed_target = proposed_target
                .max(state.current_buffered_frames)
                .saturating_add(frames_from_ms(
                    sample_rate,
                    self.config.queue_error_penalty_ms,
                ));
            self.cooldown_observations = self.config.cooldown_observations;
            self.stable_observations = 0;
        } else if underrun_delta > 0 {
            reason = FluxGuardDecisionReason::UnderrunPressure;
            proposed_target = proposed_target
                .max(state.current_buffered_frames)
                .saturating_add(frames_from_ms(sample_rate, self.config.underrun_penalty_ms));
            self.cooldown_observations = self.config.cooldown_observations;
            self.stable_observations = 0;
        } else if state.current_buffered_frames <= self.low_water_frames() {
            reason = FluxGuardDecisionReason::LowWater;
            proposed_target = proposed_target.saturating_add(frames_from_ms(
                sample_rate,
                self.config.low_water_headroom_ms,
            ));
            self.cooldown_observations = self.config.cooldown_observations;
            self.stable_observations = 0;
        } else if self.cooldown_observations > 0 {
            reason = FluxGuardDecisionReason::Cooldown;
            self.cooldown_observations = self.cooldown_observations.saturating_sub(1);
        } else if state.current_buffered_frames >= self.high_water_frames() {
            self.stable_observations = self.stable_observations.saturating_add(1);
            if self.stable_observations >= self.config.stable_observations_required {
                proposed_target = proposed_target
                    .saturating_sub(frames_from_ms(sample_rate, self.config.stable_step_down_ms));
                self.stable_observations = 0;
            }
        } else {
            self.stable_observations = 0;
        }

        let proposed_target = proposed_target.clamp(min_frames, max_frames);
        let material_delta = proposed_target.abs_diff(self.target_frames)
            >= frames_from_ms(sample_rate, self.config.stable_step_down_ms).max(1);
        let changed = material_delta || self.last_decision.reason != reason;
        if material_delta {
            self.target_frames = proposed_target;
        }

        self.last_decision = FluxGuardDecision {
            target: RenderAheadTarget {
                target_frames: self.target_frames,
                low_water_frames: self.low_water_frames(),
                high_water_frames: self.high_water_frames(),
            },
            state: RenderAheadState {
                effective_target_frames: self.target_frames,
                ..state
            },
            reason: if self.last_decision.reason == FluxGuardDecisionReason::NoAudio {
                FluxGuardDecisionReason::Initialized
            } else {
                reason
            },
            changed,
        };
        self.snapshot()
    }

    pub(super) fn snapshot(&self) -> FluxGuardSnapshot {
        FluxGuardSnapshot {
            decision: self.last_decision,
            initialized: self.initialized,
            stable_observations: self.stable_observations,
            cooldown_observations: self.cooldown_observations,
        }
    }

    fn low_water_frames(&self) -> u32 {
        self.target_frames / 2
    }

    fn high_water_frames(&self) -> u32 {
        self.target_frames.saturating_add(self.target_frames / 2)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum OutputViewKind {
    Live,
    Audible,
}

#[derive(Debug, Clone)]
pub struct OutputViewSnapshot {
    pub kind: OutputViewKind,
    pub rendered_frame_clock: u64,
    pub effective_latency_frames: u64,
    pub effective_latency_ms: f64,
    pub wrapper_queue_frames: u64,
    pub device_queue_frames: u64,
    pub snapshot: RealtimeMixerSnapshot,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct OutputLatencyBreakdown {
    pub total_frames: u64,
    pub wrapper_queue_frames: u64,
    pub device_queue_frames: u64,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub(super) struct RenderLayerTransition {
    pub(super) warmup_tick: Option<u64>,
}

impl AudioOutput {
    pub(super) fn open_f32_stream(
        backend: AudioBackend,
        config: AudioDeviceConfig,
        renderer: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> std::result::Result<Self, String> {
        let _memory_domain = memory_runtime::bind_current_thread(MemoryDomain::AudioBackend);
        let memory_hooks = audio_memory_hooks();
        match backend {
            AudioBackend::Sdl3 => {
                SdlAudioOutput::open_f32_stream_with_memory_hooks(config, memory_hooks, renderer)
                    .map(Self::Sdl3)
            }
            #[cfg(windows)]
            AudioBackend::Wasapi => {
                WasapiAudioOutput::open_f32_stream_with_memory_hooks(config, memory_hooks, renderer)
                    .map(Self::Wasapi)
            }
        }
    }

    pub(super) fn resume(&mut self) -> std::result::Result<(), String> {
        match self {
            Self::Sdl3(audio) => audio.resume(),
            #[cfg(windows)]
            Self::Wasapi(audio) => audio.resume(),
        }
    }

    pub(super) fn pause(&mut self) -> std::result::Result<(), String> {
        match self {
            Self::Sdl3(audio) => audio.pause(),
            #[cfg(windows)]
            Self::Wasapi(audio) => audio.pause(),
        }
    }

    pub(super) fn diagnostics(&self) -> AudioDeviceDiagnostics {
        match self {
            Self::Sdl3(audio) => audio.diagnostics(),
            #[cfg(windows)]
            Self::Wasapi(audio) => audio.diagnostics(),
        }
    }

    pub(super) fn render_ahead_state(&self) -> RenderAheadState {
        match self {
            Self::Sdl3(audio) => audio.render_ahead_state(),
            #[cfg(windows)]
            Self::Wasapi(audio) => audio.render_ahead_state(),
        }
    }

    pub(super) fn apply_render_ahead_target(
        &mut self,
        target: RenderAheadTarget,
    ) -> std::result::Result<(), String> {
        match self {
            Self::Sdl3(audio) => audio.apply_render_ahead_target(target),
            #[cfg(windows)]
            Self::Wasapi(audio) => audio.apply_render_ahead_target(target),
        }
    }

    pub(super) fn backend_name(&self) -> &'static str {
        match self {
            Self::Sdl3(_) => "sdl3",
            #[cfg(windows)]
            Self::Wasapi(_) => "wasapi",
        }
    }
}

fn audio_memory_hooks() -> AudioMemoryHooks {
    AudioMemoryHooks {
        producer: Some(Arc::new(|| {
            Box::new(memory_runtime::bind_current_thread(
                MemoryDomain::AudioBackend,
            ))
        })),
        callback: Some(Arc::new(|| {
            Box::new(memory_runtime::bind_current_thread(
                MemoryDomain::AudioCallback,
            ))
        })),
    }
}

pub(super) fn output_latency_breakdown(
    audio: Option<&AudioDeviceDiagnostics>,
) -> OutputLatencyBreakdown {
    let Some(audio) = audio else {
        return OutputLatencyBreakdown::default();
    };

    if audio.ring_retained_bytes > 0 {
        let device_queue_frames = audio
            .device_buffer_frames
            .saturating_sub(audio.device_available_frames) as u64;
        let wrapper_queue_frames = audio.ring_available_frames as u64;
        OutputLatencyBreakdown {
            total_frames: wrapper_queue_frames.saturating_add(device_queue_frames),
            wrapper_queue_frames,
            device_queue_frames,
        }
    } else {
        let total_frames = (audio
            .ring_capacity_frames
            .saturating_sub(audio.ring_available_frames)) as u64;
        OutputLatencyBreakdown {
            total_frames,
            wrapper_queue_frames: total_frames,
            device_queue_frames: 0,
        }
    }
}

pub(super) fn frames_from_ms(sample_rate: u32, milliseconds: u32) -> u32 {
    ((u64::from(sample_rate.max(1)) * u64::from(milliseconds)) / 1000)
        .max(1)
        .min(u64::from(u32::MAX)) as u32
}
