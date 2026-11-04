use super::biquad::{db_to_linear, BiquadState, RuntimeBand};
use super::*;

#[derive(Debug, Clone)]
pub struct PreparedConfig {
    config: PeqConfig,
    runtime_bands: Vec<RuntimeBand>,
    estimated_state_bytes: usize,
}

impl PreparedConfig {
    /// Builds a runtime-ready config outside the audio callback or steady-state chunk path.
    pub fn from_config(config: PeqConfig) -> Result<Self> {
        config.validate()?;
        let channel_count = usize::from(config.channel_count);
        let runtime_bands = config
            .bands
            .iter()
            .cloned()
            .map(|band| RuntimeBand::new(config.sample_rate_hz, channel_count, band))
            .collect::<Result<Vec<_>>>()?;
        let estimated_state_bytes =
            runtime_bands.len() * channel_count * mem::size_of::<BiquadState>();
        Ok(Self {
            config,
            runtime_bands,
            estimated_state_bytes,
        })
    }

    pub fn config(&self) -> &PeqConfig {
        &self.config
    }

    pub fn estimated_state_bytes(&self) -> usize {
        self.estimated_state_bytes
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProcessorMetrics {
    pub active_config_generation: u64,
    pub pending_config_generation: Option<u64>,
    pub config_swaps: u64,
    pub process_calls: u64,
    pub processed_frames: u64,
    pub retained_state_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDisposition {
    Retained,
    Released,
}

#[derive(Debug, Clone)]
pub struct ProcessSummary {
    pub frames: usize,
    pub channels: usize,
    pub layout: ChannelLayout,
    pub finite_sample_count: usize,
}

#[derive(Debug, Clone)]
pub struct PeqProcessor {
    active_config: Option<PeqConfig>,
    bands: Vec<RuntimeBand>,
    pending: Option<PreparedConfig>,
    metrics: ProcessorMetrics,
    output_gain_linear: f32,
    wet_mix: f32,
}

impl Default for PeqProcessor {
    fn default() -> Self {
        Self {
            active_config: None,
            bands: Vec::new(),
            pending: None,
            metrics: ProcessorMetrics::default(),
            output_gain_linear: 1.0,
            wet_mix: 1.0,
        }
    }
}

impl PeqProcessor {
    pub fn new(prepared: PreparedConfig) -> Self {
        let mut processor = Self::default();
        processor.activate_prepared(prepared);
        processor
    }

    pub fn from_config(config: PeqConfig) -> Result<Self> {
        let prepared = PreparedConfig::from_config(config)?;
        Ok(Self::new(prepared))
    }

    pub fn prepare_config(config: PeqConfig) -> Result<PreparedConfig> {
        PreparedConfig::from_config(config)
    }

    /// Queues a config prepared off-thread/off-callback.
    ///
    /// The new runtime state is swapped in at the next `process_interleaved` or
    /// `process_planar` call so live updates take effect on the next processed chunk.
    pub fn set_prepared_config(&mut self, prepared: PreparedConfig) -> Result<()> {
        if prepared.config.channel_count == 0 {
            return Err(PeqError::InvalidConfig(
                "channel_count must be greater than zero".to_owned(),
            ));
        }
        let next_generation =
            self.metrics.active_config_generation + u64::from(self.pending.is_some()) + 1;
        self.metrics.pending_config_generation = Some(next_generation);
        self.pending = Some(prepared);
        Ok(())
    }

    /// Convenience helper that validates and prepares a config before queueing it.
    ///
    /// This method may allocate while building runtime state, so it should be called
    /// outside the audio callback or other steady-state realtime path.
    pub fn set_config(&mut self, config: PeqConfig) -> Result<()> {
        let prepared = Self::prepare_config(config)?;
        self.set_prepared_config(prepared)
    }

    pub fn active_config(&self) -> Option<&PeqConfig> {
        self.active_config.as_ref()
    }

    pub fn metrics(&self) -> &ProcessorMetrics {
        &self.metrics
    }

    pub fn reset_state(&mut self) -> Result<()> {
        if self.active_config.is_none() {
            return Err(PeqError::InvalidState(
                "cannot reset an uninitialized PEQ processor".to_owned(),
            ));
        }
        for band in &mut self.bands {
            band.reset_state();
        }
        Ok(())
    }

    pub fn release_state(&mut self) -> ReleaseDisposition {
        if self.bands.is_empty() {
            return ReleaseDisposition::Retained;
        }
        self.bands.clear();
        self.active_config = None;
        self.pending = None;
        self.output_gain_linear = 1.0;
        self.wet_mix = 1.0;
        self.metrics.retained_state_bytes = 0;
        self.metrics.pending_config_generation = None;
        ReleaseDisposition::Released
    }

    pub fn process_interleaved(
        &mut self,
        input: &[f32],
        output: &mut [f32],
    ) -> Result<ProcessSummary> {
        self.apply_pending_config();
        let config = self.ensure_active_config()?;
        if config.channel_layout != ChannelLayout::Interleaved {
            return Err(PeqError::InvalidState(
                "active config expects planar processing, not interleaved".to_owned(),
            ));
        }
        if input.len() != output.len() {
            return Err(PeqError::BufferMismatch(format!(
                "input/output sample count mismatch: {} != {}",
                input.len(),
                output.len()
            )));
        }
        let channels = usize::from(config.channel_count);
        if input.len() % channels != 0 {
            return Err(PeqError::BufferMismatch(format!(
                "interleaved input length {} is not divisible by channel count {}",
                input.len(),
                channels
            )));
        }
        let frames = input.len() / channels;
        let mut finite_sample_count = 0;
        for frame_index in 0..frames {
            self.advance_chunk_frame();
            let base = frame_index * channels;
            for channel in 0..channels {
                let sample = input[base + channel];
                let processed = self.process_sample(channel, sample);
                if processed.is_finite() {
                    finite_sample_count += 1;
                }
                output[base + channel] = processed;
            }
        }
        self.metrics.process_calls += 1;
        self.metrics.processed_frames += frames as u64;
        Ok(ProcessSummary {
            frames,
            channels,
            layout: ChannelLayout::Interleaved,
            finite_sample_count,
        })
    }

    pub fn process_planar(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
    ) -> Result<ProcessSummary> {
        self.apply_pending_config();
        let config = self.ensure_active_config()?;
        if config.channel_layout != ChannelLayout::Planar {
            return Err(PeqError::InvalidState(
                "active config expects interleaved processing, not planar".to_owned(),
            ));
        }
        let channels = usize::from(config.channel_count);
        if input.len() != channels || output.len() != channels {
            return Err(PeqError::BufferMismatch(format!(
                "planar channel slice count mismatch: input {}, output {}, channels {}",
                input.len(),
                output.len(),
                channels
            )));
        }
        let frames = input.first().map_or(0, |channel| channel.len());
        for channel in input {
            if channel.len() != frames {
                return Err(PeqError::BufferMismatch(
                    "planar input channel lengths do not match".to_owned(),
                ));
            }
        }
        for channel in output.iter() {
            if channel.len() != frames {
                return Err(PeqError::BufferMismatch(
                    "planar output channel lengths do not match input frame count".to_owned(),
                ));
            }
        }
        let mut finite_sample_count = 0;
        for frame_index in 0..frames {
            self.advance_chunk_frame();
            for channel_index in 0..channels {
                let processed =
                    self.process_sample(channel_index, input[channel_index][frame_index]);
                if processed.is_finite() {
                    finite_sample_count += 1;
                }
                output[channel_index][frame_index] = processed;
            }
        }
        self.metrics.process_calls += 1;
        self.metrics.processed_frames += frames as u64;
        Ok(ProcessSummary {
            frames,
            channels,
            layout: ChannelLayout::Planar,
            finite_sample_count,
        })
    }

    fn ensure_active_config(&self) -> Result<&PeqConfig> {
        self.active_config
            .as_ref()
            .ok_or_else(|| PeqError::InvalidState("PEQ processor has no active config".to_owned()))
    }

    fn apply_pending_config(&mut self) {
        if let Some(mut prepared) = self.pending.take() {
            for (old_band, new_band) in self.bands.iter().zip(prepared.runtime_bands.iter_mut()) {
                new_band.current_gain_db = old_band.current_gain_db;
                new_band.current_coefficients =
                    new_band.design_coefficients(new_band.current_gain_db);
            }
            self.activate_prepared(prepared);
            self.metrics.config_swaps += 1;
        }
    }

    fn activate_prepared(&mut self, prepared: PreparedConfig) {
        self.output_gain_linear = db_to_linear(prepared.config.output_gain_db);
        self.wet_mix = prepared.config.wet_mix;
        self.metrics.active_config_generation += 1;
        self.metrics.pending_config_generation = None;
        self.metrics.retained_state_bytes = prepared.estimated_state_bytes;
        self.active_config = Some(prepared.config);
        self.bands = prepared.runtime_bands;
    }

    fn advance_chunk_frame(&mut self) {
        for band in &mut self.bands {
            band.advance_for_next_sample();
        }
    }

    fn process_sample(&mut self, channel_index: usize, input_sample: f32) -> f32 {
        let mut wet = input_sample;
        for band in &mut self.bands {
            wet = band.process_sample(channel_index, wet);
        }
        let mixed = input_sample + ((wet - input_sample) * self.wet_mix);
        mixed * self.output_gain_linear
    }
}
