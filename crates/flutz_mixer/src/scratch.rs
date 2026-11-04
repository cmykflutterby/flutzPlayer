use super::runtime::PreparedStrip;
use super::*;

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
impl MixerAllocationStats {
    pub(super) fn new(
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

    pub(super) fn with_return_allocations(
        mut self,
        output_bytes: usize,
        reports_bytes: usize,
    ) -> Self {
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
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct MixerScratch {
    pub(super) output: Vec<StereoFrame>,
    pub(super) reports: Vec<StripMixReport>,
    pub(super) input_meters: Vec<MeterReading>,
    pub(super) prepared: Vec<PreparedStrip>,
    pub(super) prepared_frames: Vec<StereoFrame>,
    pub(super) smart_mix_contributions: Vec<f32>,
    pub(super) smart_mix_gains: Vec<f32>,
    pub(super) active_ids: HashSet<StripId>,
}

impl MixerScratch {
    pub(super) fn begin(&mut self, frame_count: usize, strip_count: usize) {
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

    pub(super) fn clear_values(&mut self) {
        self.output.clear();
        self.reports.clear();
        self.input_meters.clear();
        self.prepared.clear();
        self.prepared_frames.clear();
        self.smart_mix_contributions.clear();
        self.smart_mix_gains.clear();
        self.active_ids.clear();
    }

    pub(super) fn release_capacity(&mut self) {
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

    pub(super) fn stats(&self) -> MixerScratchStats {
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
