pub mod effects;
pub mod master;
pub mod meters;
pub mod normalization;
pub mod smart_mix;
pub mod strip;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Range,
};

use flutz_core::{FlutzError, Result, StripId};

use crate::effects::{DelayEffectState, ThreeBandEqState};

pub use master::MasterControls;
pub use meters::MeterReading;
pub use normalization::AutoNormalization;
pub use smart_mix::SmartMixSettings;
pub use strip::{db_to_gain, MixerStripControls, MixerStripIdentity};

const GAIN_SMOOTHING_THRESHOLD: f32 = 1.0E-3;

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

#[derive(Debug, Clone, PartialEq)]
pub struct MixerStripInput {
    pub identity: MixerStripIdentity,
    pub controls: MixerStripControls,
    pub automatic_processing: bool,
    pub block: AudioBlock,
}

#[derive(Debug, Clone)]
pub struct MixerStripInputView<'a> {
    pub identity: MixerStripIdentity,
    pub controls: MixerStripControls,
    pub automatic_processing: bool,
    pub block: AudioBlockView<'a>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MixerSettings {
    pub master: MasterControls,
    pub smart_mix: SmartMixSettings,
    pub auto_normalization: AutoNormalization,
}

impl Default for MixerSettings {
    fn default() -> Self {
        Self {
            master: MasterControls::default(),
            smart_mix: SmartMixSettings::default(),
            auto_normalization: AutoNormalization::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StripMixReport {
    pub identity: MixerStripIdentity,
    pub input_meter: MeterReading,
    pub applied_gain: f32,
    pub smart_mix_gain: f32,
    pub lookahead_gain: f32,
    pub normalization_gain: f32,
    pub audible: bool,
    pub solo_active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixReport {
    pub output: AudioBlock,
    pub output_meter: MeterReading,
    pub strips: Vec<StripMixReport>,
    pub allocation_stats: MixerAllocationStats,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixOutputReport {
    pub output_meter: MeterReading,
    pub strips: Vec<StripMixReport>,
    pub allocation_stats: MixerAllocationStats,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct MixerAllocationStats {
    pub processed_inputs: usize,
    pub allocation_events: usize,
    pub deallocation_events: usize,
    pub allocated_bytes: usize,
    pub deallocated_bytes: usize,
    pub output_bytes: usize,
    pub reports_bytes: usize,
    pub input_meters_bytes: usize,
    pub prepared_vec_bytes: usize,
    pub prepared_rendered_frame_bytes: usize,
    pub smart_mix_contribution_bytes: usize,
    pub smart_mix_gain_bytes: usize,
    pub scratch_growth_bytes: usize,
    pub retained_scratch_bytes: usize,
    pub retained_scratch_prepared_frame_bytes: usize,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct MixerScratchStats {
    pub retained_bytes: usize,
    pub output_bytes: usize,
    pub reports_bytes: usize,
    pub input_meters_bytes: usize,
    pub prepared_vec_bytes: usize,
    pub prepared_rendered_frame_bytes: usize,
    pub smart_mix_contribution_bytes: usize,
    pub smart_mix_gain_bytes: usize,
    pub active_ids_bytes: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixerEngine {
    settings: MixerSettings,
    strips: HashMap<StripId, StripRuntimeState>,
    master: MasterRuntimeState,
    scratch: MixerScratch,
}

impl MixerEngine {
    pub fn new(settings: MixerSettings) -> Self {
        Self {
            settings,
            strips: HashMap::new(),
            master: MasterRuntimeState::default(),
            scratch: MixerScratch::default(),
        }
    }

    pub fn settings(&self) -> MixerSettings {
        self.settings
    }

    pub fn set_settings(&mut self, settings: MixerSettings) {
        self.settings = settings;
    }

    pub fn reset_state(&mut self) {
        self.strips.clear();
        self.master = MasterRuntimeState::default();
        self.scratch.clear_values();
    }

    pub fn reset_state_and_release_scratch(&mut self) {
        self.strips.clear();
        self.master = MasterRuntimeState::default();
        self.scratch.release_capacity();
    }

    pub fn release_scratch_capacity(&mut self) {
        self.scratch.release_capacity();
    }

    pub fn scratch_stats(&self) -> MixerScratchStats {
        self.scratch.stats()
    }

    pub fn strip_has_effect_tail(&self, strip_id: StripId, controls: MixerStripControls) -> bool {
        self.strips
            .get(&strip_id)
            .is_some_and(|runtime| runtime.has_effect_tail(controls))
    }

    pub fn mix(&mut self, strips: &[MixerStripInput]) -> Result<MixReport> {
        let views = strips
            .iter()
            .map(|strip| MixerStripInputView {
                identity: strip.identity.clone(),
                controls: strip.controls,
                automatic_processing: strip.automatic_processing,
                block: strip.block.view(),
            })
            .collect::<Vec<_>>();
        self.mix_views(&views)
    }

    pub fn mix_views(&mut self, strips: &[MixerStripInputView<'_>]) -> Result<MixReport> {
        let report = self.mix_views_to_scratch(strips)?;
        let output = AudioBlock {
            frames: self.scratch.output.clone(),
        };
        let strips = self.scratch.reports.clone();
        let output_bytes = output.frames.capacity() * std::mem::size_of::<StereoFrame>();
        let reports_bytes = strips.capacity() * std::mem::size_of::<StripMixReport>();
        let allocation_stats = report
            .allocation_stats
            .with_return_allocations(output_bytes, reports_bytes);

        Ok(MixReport {
            output,
            output_meter: report.output_meter,
            strips,
            allocation_stats,
        })
    }

    pub fn mix_views_interleaved(
        &mut self,
        strips: &[MixerStripInputView<'_>],
        output: &mut [f32],
    ) -> Result<MixOutputReport> {
        self.mix_generated_views_interleaved(strips.len(), output, |index| strips[index].clone())
    }

    pub fn mix_generated_views_interleaved<'a>(
        &mut self,
        strip_count: usize,
        output: &mut [f32],
        input_at: impl FnMut(usize) -> MixerStripInputView<'a>,
    ) -> Result<MixOutputReport> {
        if strip_count == 0 {
            output.fill(0.0);
            let scratch_before_bytes = self.scratch.stats().retained_bytes;
            self.scratch.begin(0, 0);
            let scratch_stats = self.scratch.stats();
            return Ok(MixOutputReport {
                output_meter: MeterReading::default(),
                strips: Vec::new(),
                allocation_stats: MixerAllocationStats::new(
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    scratch_stats
                        .retained_bytes
                        .saturating_sub(scratch_before_bytes),
                    scratch_stats.retained_bytes,
                    scratch_stats.prepared_rendered_frame_bytes,
                ),
            });
        }

        let report = self.mix_generated_views_to_scratch(strip_count, input_at)?;
        let expected_samples = self.scratch.output.len().saturating_mul(2);
        if output.len() != expected_samples {
            return Err(FlutzError::InvalidInput(format!(
                "mixer interleaved output must contain {expected_samples} samples"
            )));
        }
        for (index, frame) in self.scratch.output.iter().enumerate() {
            output[index * 2] = frame.left;
            output[index * 2 + 1] = frame.right;
        }

        Ok(report)
    }

    fn mix_views_to_scratch(
        &mut self,
        strips: &[MixerStripInputView<'_>],
    ) -> Result<MixOutputReport> {
        self.mix_generated_views_to_scratch(strips.len(), |index| strips[index].clone())
    }

    fn mix_generated_views_to_scratch<'a>(
        &mut self,
        strip_count: usize,
        mut input_at: impl FnMut(usize) -> MixerStripInputView<'a>,
    ) -> Result<MixOutputReport> {
        let frame_count = if strip_count == 0 {
            0
        } else {
            input_at(0).block.frame_count()
        };
        for index in 0..strip_count {
            let strip = input_at(index);
            if !strip.block.valid() {
                return Err(FlutzError::InvalidInput(
                    "mixer split input left/right blocks must have the same frame count".to_owned(),
                ));
            }
            if strip.block.frame_count() != frame_count {
                return Err(FlutzError::InvalidInput(
                    "all mixer strip blocks must have the same frame count".to_owned(),
                ));
            }
        }

        let scratch_before_bytes = self.scratch.stats().retained_bytes;
        self.scratch.begin(frame_count, strip_count);
        let target_peak = self.settings.smart_mix.target_peak();
        let lookahead_blocks = self.settings.smart_mix.lookahead_blocks();
        let solo_active = (0..strip_count).any(|index| input_at(index).controls.solo);
        for index in 0..strip_count {
            let strip = input_at(index);
            self.scratch.input_meters.push(strip.block.meter());
            self.scratch.active_ids.insert(strip.identity.strip_id);
        }
        self.strips
            .retain(|strip_id, _| self.scratch.active_ids.contains(strip_id));

        let mut processed_inputs = 0usize;
        for index in 0..strip_count {
            let strip = input_at(index);
            let session_excluded = strip.controls.session_excluded(solo_active);
            let session_gain = strip.controls.session_gain(solo_active);
            let runtime = self
                .strips
                .entry(strip.identity.strip_id)
                .or_insert_with(StripRuntimeState::default);

            if session_excluded {
                runtime.reverb.reset();
                runtime.chorus.reset();
                self.scratch.prepared.push(PreparedStrip {
                    rendered_frame_range: 0..0,
                    combined_meter: MeterReading::default(),
                    session_gain,
                    session_excluded,
                    processed: false,
                });
                continue;
            }

            let input_meter = self.scratch.input_meters[index];
            if strip.block.is_silence()
                && input_meter.peak <= f32::EPSILON
                && !runtime.has_effect_tail(strip.controls)
            {
                runtime.lookahead.clear();
                self.scratch.prepared.push(PreparedStrip {
                    rendered_frame_range: 0..0,
                    combined_meter: MeterReading::default(),
                    session_gain,
                    session_excluded,
                    processed: false,
                });
                continue;
            }

            processed_inputs = processed_inputs.saturating_add(1);
            let rendered_frame_start = self.scratch.prepared_frames.len();
            for frame_index in 0..frame_count {
                let source = strip.block.frame(frame_index);
                let left = strip.controls.limiter.apply(source.left);
                let right = strip.controls.limiter.apply(source.right);
                let (left, right) =
                    runtime
                        .reverb
                        .process_reverb(left, right, strip.controls.reverb);
                let (left, right) =
                    runtime
                        .chorus
                        .process_chorus(left, right, strip.controls.chorus);
                self.scratch
                    .prepared_frames
                    .push(StereoFrame { left, right });
            }
            let rendered_frame_end = self.scratch.prepared_frames.len();
            let combined_meter = meter_from_frames(
                &self.scratch.prepared_frames[rendered_frame_start..rendered_frame_end],
            );
            self.scratch.prepared.push(PreparedStrip {
                rendered_frame_range: rendered_frame_start..rendered_frame_end,
                combined_meter,
                session_gain,
                session_excluded,
                processed: true,
            });
        }

        let mut desired_sum = 0.0f32;
        let mut actual_sum = 0.0f32;
        for index in 0..strip_count {
            let strip = input_at(index);
            let prepared = &self.scratch.prepared[index];
            if !strip.automatic_processing
                || prepared.session_excluded
                || !prepared.processed
                || prepared.session_gain <= f32::EPSILON
            {
                continue;
            }

            let perceived_level = prepared
                .combined_meter
                .peak
                .max(prepared.combined_meter.rms * 1.6);
            let contribution = perceived_level * prepared.session_gain;

            desired_sum += prepared.session_gain;
            actual_sum += contribution;
            self.scratch.smart_mix_contributions[index] = contribution;
        }

        for index in 0..strip_count {
            let strip = input_at(index);
            let prepared = &self.scratch.prepared[index];
            if strip.automatic_processing && !prepared.session_excluded && prepared.processed {
                let desired_share = if desired_sum > f32::EPSILON {
                    prepared.session_gain / desired_sum
                } else {
                    0.0
                };
                let actual_share = if actual_sum > f32::EPSILON {
                    self.scratch.smart_mix_contributions[index] / actual_sum
                } else {
                    0.0
                };
                self.scratch.smart_mix_gains[index] =
                    self.settings.smart_mix.reduction_gain_balanced(
                        prepared.combined_meter.peak,
                        prepared.combined_meter.rms,
                        desired_share,
                        actual_share,
                    );
            }
        }

        for index in 0..strip_count {
            let strip = input_at(index);
            let input_meter = self.scratch.input_meters[index];
            let prepared = &self.scratch.prepared[index];
            let runtime = self
                .strips
                .entry(strip.identity.strip_id)
                .or_insert_with(StripRuntimeState::default);
            let lookahead_gain = if strip.automatic_processing && !prepared.session_excluded {
                if prepared.processed {
                    runtime.lookahead_gain(self.scratch.smart_mix_gains[index], lookahead_blocks)
                } else {
                    runtime.lookahead.clear();
                    1.0
                }
            } else {
                runtime.lookahead.clear();
                1.0
            };
            let smoothed_smart_mix_gain =
                if strip.automatic_processing && !prepared.session_excluded && prepared.processed {
                    self.settings
                        .smart_mix
                        .smooth_gain(runtime.smoothed_smart_mix_gain, lookahead_gain)
                } else {
                    1.0
                };
            runtime.smoothed_smart_mix_gain = smoothed_smart_mix_gain;
            let normalization_gain =
                if strip.automatic_processing && !prepared.session_excluded && prepared.processed {
                    self.settings.auto_normalization.gain(
                        prepared.combined_meter.peak * smoothed_smart_mix_gain,
                        target_peak,
                    )
                } else {
                    1.0
                };
            let strip_gain = prepared.session_gain * normalization_gain * smoothed_smart_mix_gain;
            let (left_pan, right_pan) = strip.controls.pan_gains();
            let target_left_gain = strip_gain * left_pan;
            let target_right_gain = strip_gain * right_pan;

            if prepared.session_excluded || !prepared.processed {
                runtime.previous_left_gain = target_left_gain;
                runtime.previous_right_gain = target_right_gain;
                runtime.gain_initialized = true;

                self.scratch.reports.push(StripMixReport {
                    identity: strip.identity.clone(),
                    input_meter,
                    applied_gain: strip_gain,
                    smart_mix_gain: smoothed_smart_mix_gain,
                    lookahead_gain,
                    normalization_gain,
                    audible: false,
                    solo_active,
                });
                continue;
            }

            let (previous_left_gain, previous_right_gain) =
                runtime.previous_or_initialize_gains(target_left_gain, target_right_gain);
            let left_gain_step = gain_step(previous_left_gain, target_left_gain, frame_count);
            let right_gain_step = gain_step(previous_right_gain, target_right_gain, frame_count);
            let mut left_gain = previous_left_gain;
            let mut right_gain = previous_right_gain;

            for (frame_index, source) in self.scratch.prepared_frames
                [prepared.rendered_frame_range.clone()]
            .iter()
            .enumerate()
            {
                self.scratch.output[frame_index].left += source.left * left_gain;
                self.scratch.output[frame_index].right += source.right * right_gain;
                left_gain += left_gain_step;
                right_gain += right_gain_step;
            }
            runtime.previous_left_gain = target_left_gain;
            runtime.previous_right_gain = target_right_gain;

            self.scratch.reports.push(StripMixReport {
                identity: strip.identity.clone(),
                input_meter,
                applied_gain: strip_gain,
                smart_mix_gain: smoothed_smart_mix_gain,
                lookahead_gain,
                normalization_gain,
                audible: prepared.session_gain > 0.0 && prepared.combined_meter.peak > f32::EPSILON,
                solo_active,
            });
        }

        let master_gain = db_to_gain(self.settings.master.volume_db) as f32;
        let master_low_gain = db_to_gain(self.settings.master.eq_low_db) as f32;
        let master_mid_gain = db_to_gain(self.settings.master.eq_mid_db) as f32;
        let master_high_gain = db_to_gain(self.settings.master.eq_high_db) as f32;
        let master_eq_enabled = (master_low_gain - 1.0).abs() > f32::EPSILON
            || (master_mid_gain - 1.0).abs() > f32::EPSILON
            || (master_high_gain - 1.0).abs() > f32::EPSILON;
        for frame in &mut self.scratch.output {
            let (left, right) = self.master.reverb.process_reverb(
                frame.left * master_gain,
                frame.right * master_gain,
                self.settings.master.reverb,
            );
            let (left, right) =
                self.master
                    .chorus
                    .process_chorus(left, right, self.settings.master.chorus);
            let (left, right) = if master_eq_enabled {
                self.master.eq.process(
                    left,
                    right,
                    master_low_gain,
                    master_mid_gain,
                    master_high_gain,
                )
            } else {
                (left, right)
            };
            frame.left = self.settings.master.limiter.apply(left);
            frame.right = self.settings.master.limiter.apply(right);
        }

        let output_meter = meter_from_frames(&self.scratch.output);
        let scratch_stats = self.scratch.stats();
        let allocation_stats = MixerAllocationStats::new(
            processed_inputs,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            scratch_stats
                .retained_bytes
                .saturating_sub(scratch_before_bytes),
            scratch_stats.retained_bytes,
            scratch_stats.prepared_rendered_frame_bytes,
        );
        let strips = self.scratch.reports.clone();
        let reports_bytes = strips.capacity() * std::mem::size_of::<StripMixReport>();
        let allocation_stats = allocation_stats.with_return_allocations(0, reports_bytes);
        Ok(MixOutputReport {
            output_meter,
            strips,
            allocation_stats,
        })
    }
}

impl MixerAllocationStats {
    fn new(
        processed_inputs: usize,
        output_bytes: usize,
        reports_bytes: usize,
        input_meters_bytes: usize,
        prepared_vec_bytes: usize,
        prepared_rendered_frame_bytes: usize,
        smart_mix_contribution_bytes: usize,
        smart_mix_gain_bytes: usize,
        scratch_growth_bytes: usize,
        retained_scratch_bytes: usize,
        retained_scratch_prepared_frame_bytes: usize,
    ) -> Self {
        let allocated_bytes = output_bytes
            .saturating_add(reports_bytes)
            .saturating_add(input_meters_bytes)
            .saturating_add(prepared_vec_bytes)
            .saturating_add(prepared_rendered_frame_bytes)
            .saturating_add(smart_mix_contribution_bytes)
            .saturating_add(smart_mix_gain_bytes);
        let allocation_events = [
            output_bytes,
            reports_bytes,
            input_meters_bytes,
            prepared_vec_bytes,
            prepared_rendered_frame_bytes,
            smart_mix_contribution_bytes,
            smart_mix_gain_bytes,
        ]
        .into_iter()
        .filter(|bytes| *bytes > 0)
        .count();
        Self {
            processed_inputs,
            allocation_events,
            deallocation_events: allocation_events,
            allocated_bytes,
            deallocated_bytes: allocated_bytes,
            output_bytes,
            reports_bytes,
            input_meters_bytes,
            prepared_vec_bytes,
            prepared_rendered_frame_bytes,
            smart_mix_contribution_bytes,
            smart_mix_gain_bytes,
            scratch_growth_bytes,
            retained_scratch_bytes,
            retained_scratch_prepared_frame_bytes,
        }
    }

    fn with_return_allocations(mut self, output_bytes: usize, reports_bytes: usize) -> Self {
        self.output_bytes = output_bytes;
        self.reports_bytes = reports_bytes;
        self.allocated_bytes = self
            .allocated_bytes
            .saturating_add(output_bytes)
            .saturating_add(reports_bytes);
        self.deallocated_bytes = self.allocated_bytes;
        let return_allocation_events = [output_bytes, reports_bytes]
            .into_iter()
            .filter(|bytes| *bytes > 0)
            .count();
        self.allocation_events = self
            .allocation_events
            .saturating_add(return_allocation_events);
        self.deallocation_events = self.allocation_events;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
struct StripRuntimeState {
    smoothed_smart_mix_gain: f32,
    previous_left_gain: f32,
    previous_right_gain: f32,
    gain_initialized: bool,
    lookahead: VecDeque<f32>,
    reverb: DelayEffectState,
    chorus: DelayEffectState,
}

#[derive(Debug, Clone, PartialEq)]
struct PreparedStrip {
    rendered_frame_range: Range<usize>,
    combined_meter: MeterReading,
    session_gain: f32,
    session_excluded: bool,
    processed: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct MixerScratch {
    output: Vec<StereoFrame>,
    reports: Vec<StripMixReport>,
    input_meters: Vec<MeterReading>,
    prepared: Vec<PreparedStrip>,
    prepared_frames: Vec<StereoFrame>,
    smart_mix_contributions: Vec<f32>,
    smart_mix_gains: Vec<f32>,
    active_ids: HashSet<StripId>,
}

impl MixerScratch {
    fn begin(&mut self, frame_count: usize, strip_count: usize) {
        self.output.clear();
        self.output.resize(frame_count, StereoFrame::default());
        self.reports.clear();
        self.reports.reserve(strip_count);
        self.input_meters.clear();
        self.input_meters.reserve(strip_count);
        self.prepared.clear();
        self.prepared.reserve(strip_count);
        self.prepared_frames.clear();
        let prepared_frame_floor = frame_count.saturating_mul(strip_count);
        if self.prepared_frames.capacity() < prepared_frame_floor {
            self.prepared_frames
                .reserve(prepared_frame_floor - self.prepared_frames.capacity());
        }
        self.smart_mix_contributions.clear();
        self.smart_mix_contributions.resize(strip_count, 0.0);
        self.smart_mix_gains.clear();
        self.smart_mix_gains.resize(strip_count, 1.0);
        self.active_ids.clear();
        self.active_ids.reserve(strip_count);
    }

    fn clear_values(&mut self) {
        self.output.clear();
        self.reports.clear();
        self.input_meters.clear();
        self.prepared.clear();
        self.prepared_frames.clear();
        self.smart_mix_contributions.clear();
        self.smart_mix_gains.clear();
        self.active_ids.clear();
    }

    fn release_capacity(&mut self) {
        self.clear_values();
        self.output.shrink_to_fit();
        self.reports.shrink_to_fit();
        self.input_meters.shrink_to_fit();
        self.prepared.shrink_to_fit();
        self.prepared_frames.shrink_to_fit();
        self.smart_mix_contributions.shrink_to_fit();
        self.smart_mix_gains.shrink_to_fit();
        self.active_ids.shrink_to_fit();
    }

    fn stats(&self) -> MixerScratchStats {
        let output_bytes = self.output.capacity() * std::mem::size_of::<StereoFrame>();
        let reports_bytes = self.reports.capacity() * std::mem::size_of::<StripMixReport>();
        let input_meters_bytes = self.input_meters.capacity() * std::mem::size_of::<MeterReading>();
        let prepared_vec_bytes = self.prepared.capacity() * std::mem::size_of::<PreparedStrip>();
        let prepared_rendered_frame_bytes =
            self.prepared_frames.capacity() * std::mem::size_of::<StereoFrame>();
        let smart_mix_contribution_bytes =
            self.smart_mix_contributions.capacity() * std::mem::size_of::<f32>();
        let smart_mix_gain_bytes = self.smart_mix_gains.capacity() * std::mem::size_of::<f32>();
        let active_ids_bytes = self.active_ids.capacity() * std::mem::size_of::<StripId>();
        let retained_bytes = output_bytes
            .saturating_add(reports_bytes)
            .saturating_add(input_meters_bytes)
            .saturating_add(prepared_vec_bytes)
            .saturating_add(prepared_rendered_frame_bytes)
            .saturating_add(smart_mix_contribution_bytes)
            .saturating_add(smart_mix_gain_bytes)
            .saturating_add(active_ids_bytes);
        MixerScratchStats {
            retained_bytes,
            output_bytes,
            reports_bytes,
            input_meters_bytes,
            prepared_vec_bytes,
            prepared_rendered_frame_bytes,
            smart_mix_contribution_bytes,
            smart_mix_gain_bytes,
            active_ids_bytes,
        }
    }
}

impl Default for StripRuntimeState {
    fn default() -> Self {
        Self {
            smoothed_smart_mix_gain: 1.0,
            previous_left_gain: 1.0,
            previous_right_gain: 1.0,
            gain_initialized: false,
            lookahead: VecDeque::new(),
            reverb: DelayEffectState::new(12_000),
            chorus: DelayEffectState::new(4_800),
        }
    }
}

impl StripRuntimeState {
    fn previous_or_initialize_gains(
        &mut self,
        target_left_gain: f32,
        target_right_gain: f32,
    ) -> (f32, f32) {
        if !self.gain_initialized {
            self.previous_left_gain = target_left_gain;
            self.previous_right_gain = target_right_gain;
            self.gain_initialized = true;
        }
        (self.previous_left_gain, self.previous_right_gain)
    }

    fn lookahead_gain(&mut self, gain: f32, lookahead_blocks: usize) -> f32 {
        if lookahead_blocks == 0 {
            self.lookahead.clear();
            return gain;
        }

        self.lookahead.push_back(gain);
        while self.lookahead.len() > lookahead_blocks + 1 {
            self.lookahead.pop_front();
        }
        self.lookahead.iter().copied().fold(1.0, f32::min)
    }

    fn has_effect_tail(&self, controls: MixerStripControls) -> bool {
        (controls.reverb > f64::EPSILON && self.reverb.has_signal())
            || (controls.chorus > f64::EPSILON && self.chorus.has_signal())
    }
}

fn gain_step(previous_gain: f32, current_gain: f32, frame_count: usize) -> f32 {
    if frame_count == 0 || (current_gain - previous_gain).abs() < GAIN_SMOOTHING_THRESHOLD {
        0.0
    } else {
        (current_gain - previous_gain) / frame_count as f32
    }
}

fn meter_from_frames(frames: &[StereoFrame]) -> MeterReading {
    if frames.is_empty() {
        return MeterReading::default();
    }

    let mut peak = 0.0f32;
    let mut sum_square = 0.0f64;
    for frame in frames {
        let left = frame.left.abs();
        let right = frame.right.abs();
        peak = peak.max(left).max(right);
        sum_square += (frame.left as f64) * (frame.left as f64);
        sum_square += (frame.right as f64) * (frame.right as f64);
    }

    let samples = frames.len() * 2;
    let rms = (sum_square / samples as f64).sqrt() as f32;
    MeterReading {
        peak,
        rms,
        headroom: (1.0 - peak).max(0.0),
    }
}

fn meter_from_split_frames(left: &[f32], right: &[f32], gain: f32) -> MeterReading {
    if left.is_empty() || right.is_empty() || left.len() != right.len() {
        return MeterReading::default();
    }

    let mut peak = 0.0f32;
    let mut sum_square = 0.0f64;
    for index in 0..left.len() {
        let sample_left = left[index] * gain;
        let sample_right = right[index] * gain;
        peak = peak.max(sample_left.abs()).max(sample_right.abs());
        sum_square += (sample_left as f64) * (sample_left as f64);
        sum_square += (sample_right as f64) * (sample_right as f64);
    }

    let samples = left.len() * 2;
    let rms = (sum_square / samples as f64).sqrt() as f32;
    MeterReading {
        peak,
        rms,
        headroom: (1.0 - peak).max(0.0),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct MasterRuntimeState {
    eq: ThreeBandEqState,
    reverb: DelayEffectState,
    chorus: DelayEffectState,
}

impl Default for MasterRuntimeState {
    fn default() -> Self {
        Self {
            eq: ThreeBandEqState::default(),
            reverb: DelayEffectState::new(24_000),
            chorus: DelayEffectState::new(7_200),
        }
    }
}
