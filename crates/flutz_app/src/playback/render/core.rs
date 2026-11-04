use super::super::*;

pub(in crate::playback) struct MixedAudioRenderReport {
    pub(in crate::playback) output_meter: MeterReading,
    pub(in crate::playback) snapshot: RealtimeMixerSnapshot,
    pub(in crate::playback) frames: usize,
    pub(in crate::playback) churn: RenderChurnSnapshot,
    pub(in crate::playback) strip_mix_diagnostics: Vec<RenderProbeStripMixDiagnostics>,
}

#[derive(Debug, Clone)]
pub struct MixerControlState {
    pub settings: MixerSettings,
    pub strip_controls: BTreeMap<StripId, MixerStripControls>,
}

impl Default for MixerControlState {
    fn default() -> Self {
        Self {
            settings: MixerSettings::default(),
            strip_controls: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RealtimeMixerSnapshot {
    pub output_meter: MeterReading,
    pub strips: BTreeMap<StripId, RealtimeStripSnapshot>,
}

#[derive(Debug, Clone, Default)]
pub(in crate::playback) struct SnapshotHistory {
    latest: RealtimeMixerSnapshot,
    frames: VecDeque<SnapshotFrame>,
}

#[derive(Debug, Clone)]
struct SnapshotFrame {
    rendered_frame_end: u64,
    snapshot: RealtimeMixerSnapshot,
}

impl SnapshotHistory {
    pub(in crate::playback) fn clear(&mut self) {
        self.latest = RealtimeMixerSnapshot::default();
        self.frames.clear();
    }

    pub(in crate::playback) fn record(
        &mut self,
        rendered_frame_end: u64,
        snapshot: RealtimeMixerSnapshot,
        sample_rate: u32,
    ) {
        self.latest = snapshot.clone();
        self.frames.push_back(SnapshotFrame {
            rendered_frame_end,
            snapshot,
        });
        let retention_frames = sample_rate.max(1) as u64 * 4;
        let cutoff = rendered_frame_end.saturating_sub(retention_frames);
        while self
            .frames
            .front()
            .is_some_and(|entry| entry.rendered_frame_end < cutoff)
        {
            self.frames.pop_front();
        }
    }

    pub(in crate::playback) fn live_snapshot(&self) -> RealtimeMixerSnapshot {
        self.latest.clone()
    }

    pub(in crate::playback) fn delayed_snapshot(
        &self,
        rendered_frames: u64,
        latency_frames: u64,
    ) -> RealtimeMixerSnapshot {
        let mut snapshot = RealtimeMixerSnapshot::default();
        self.copy_delayed_snapshot_into(rendered_frames, latency_frames, &mut snapshot);
        snapshot
    }

    pub(in crate::playback) fn copy_delayed_snapshot_into(
        &self,
        rendered_frames: u64,
        latency_frames: u64,
        target: &mut RealtimeMixerSnapshot,
    ) {
        let target_frame = rendered_frames.saturating_sub(latency_frames);
        for entry in self.frames.iter().rev() {
            if entry.rendered_frame_end <= target_frame {
                target.copy_from(&entry.snapshot);
                return;
            }
        }
        if self.frames.is_empty() {
            target.copy_from(&self.latest);
        } else {
            target.output_meter = MeterReading::default();
            target.strips.clear();
        }
    }
}

impl RealtimeMixerSnapshot {
    /// Copies `source` into `self`, reusing existing strip entries and their string/note
    /// buffers so per-frame UI reads don't reallocate.
    pub fn copy_from(&mut self, source: &RealtimeMixerSnapshot) {
        self.output_meter = source.output_meter;
        self.strips
            .retain(|strip_id, _| source.strips.contains_key(strip_id));
        for (strip_id, strip) in &source.strips {
            match self.strips.get_mut(strip_id) {
                Some(existing) => existing.copy_from(strip),
                None => {
                    self.strips.insert(*strip_id, strip.clone());
                }
            }
        }
    }
}

impl RealtimeStripSnapshot {
    fn copy_from(&mut self, source: &RealtimeStripSnapshot) {
        self.soundfont_id.clone_from(&source.soundfont_id);
        self.midi_channel = source.midi_channel;
        self.midi_bank = source.midi_bank;
        self.midi_program = source.midi_program;
        self.is_percussion = source.is_percussion;
        match (&mut self.display_name, &source.display_name) {
            (Some(name), Some(source_name)) => name.clone_from(source_name),
            (name, source_name) => name.clone_from(source_name),
        }
        self.meter = source.meter;
        self.audible = source.audible;
        self.active_notes.clone_from(&source.active_notes);
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeStripSnapshot {
    pub soundfont_id: String,
    pub midi_channel: u8,
    pub midi_bank: u16,
    pub midi_program: u8,
    pub is_percussion: bool,
    pub display_name: Option<String>,
    pub meter: MeterReading,
    pub audible: bool,
    pub active_notes: Vec<u8>,
}

pub(in crate::playback) fn render_mixed_audio(
    engine: &mut MultiSoundFontPlayback,
    mixer: &mut MixerEngine,
    scratch: &mut PlaybackRenderScratch,
    controls: &MixerControlState,
    final_output_gain: f32,
    soundfont_ids: &[String],
    midi_strips: &[MidiStripDescriptor],
    output: &mut [f32],
) -> Result<MixedAudioRenderReport> {
    let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::PlaybackRender);
    mixer.set_settings(controls.settings);
    output.fill(0.0);
    let frames = output.len() / 2;
    if frames == 0 {
        return Ok(MixedAudioRenderReport {
            output_meter: MeterReading::from_interleaved(output),
            snapshot: RealtimeMixerSnapshot::default(),
            frames: 0,
            churn: RenderChurnSnapshot::default(),
            strip_mix_diagnostics: Vec::new(),
        });
    }

    let soundfont_gain = 1.0 / engine.instance_count().max(1) as f32;
    scratch.begin_render();
    let playback_stem_bytes_before = stem_blocks_total_bytes(&scratch.blocks);
    {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::RenderStems);
        engine.render_channel_program_stem_blocks_into(frames, &mut scratch.blocks);
    }
    let stem_playback_growth_bytes =
        stem_blocks_total_bytes(&scratch.blocks).saturating_sub(playback_stem_bytes_before);
    let stem_allocations = engine.last_stem_render_allocations();
    let stem_audio_bytes = stem_blocks_audio_bytes(&scratch.blocks);
    let stem_tracked_alloc_bytes = stem_allocations.total_bytes();
    let stem_total_growth_bytes = stem_allocations
        .total_growth_bytes()
        .saturating_add(stem_playback_growth_bytes);
    let stem_untracked_alloc_bytes = stem_tracked_alloc_bytes.saturating_sub(stem_audio_bytes);
    scratch.soundfont_slots.extend(
        soundfont_ids
            .iter()
            .enumerate()
            .map(|(index, soundfont_id)| (soundfont_id.clone(), index)),
    );
    for (block_index, block) in scratch.blocks.iter().enumerate() {
        let next_slot = scratch.soundfont_slots.len();
        let soundfont_slot = *scratch
            .soundfont_slots
            .entry(block.identity.soundfont_id.clone())
            .or_insert(next_slot);
        let strip_id = stem_strip_id(soundfont_slot, &block.identity);
        let is_global_residual = is_global_residual_stem(&block.identity);
        scratch.rendered_routes.push(RoutedStemRoute {
            block_index,
            soundfont_slot,
            strip_id,
            is_global_residual,
        });
    }
    let rendered_blocks = scratch.rendered_routes.len();
    scratch
        .rendered_strip_ids
        .extend(scratch.rendered_routes.iter().map(|route| route.strip_id));

    for (soundfont_slot, soundfont_id) in soundfont_ids.iter().enumerate() {
        for descriptor in midi_strips {
            let identity = StemIdentity::channel_bank_program(
                soundfont_id.clone(),
                descriptor.channel,
                Some(descriptor.bank),
                descriptor.program,
                descriptor.is_percussion,
            );
            let strip_id = stem_strip_id(soundfont_slot, &identity);
            let controls = controls
                .strip_controls
                .get(&strip_id)
                .copied()
                .unwrap_or_default();
            scratch
                .visual_controls_by_soundfont
                .entry(soundfont_slot)
                .or_default()
                .push(controls);
            scratch.visual_control_ids.insert(strip_id);
            if !scratch.rendered_strip_ids.contains(&strip_id) {
                scratch.visual_metadata.insert(
                    strip_id,
                    RealtimeStripSnapshot {
                        soundfont_id: identity.soundfont_id.clone(),
                        midi_channel: identity.midi_channel.unwrap_or(0),
                        midi_bank: identity.midi_bank.unwrap_or(0),
                        midi_program: identity.midi_program.unwrap_or(0),
                        is_percussion: identity.is_percussion,
                        display_name: None,
                        meter: MeterReading::default(),
                        audible: false,
                        active_notes: Vec::new(),
                    },
                );
                scratch.silence_candidates.push(SilenceStemRoute {
                    identity,
                    strip_id,
                    controls,
                });
            }
        }
    }

    for route in &scratch.rendered_routes {
        if !route.is_global_residual && scratch.visual_control_ids.insert(route.strip_id) {
            scratch
                .visual_controls_by_soundfont
                .entry(route.soundfont_slot)
                .or_default()
                .push(
                    controls
                        .strip_controls
                        .get(&route.strip_id)
                        .copied()
                        .unwrap_or_default(),
                );
        }

        let identity = route.identity(&scratch.blocks);
        let block_view = route.as_mixer_view(&scratch.blocks, soundfont_gain);
        let meter = block_view.meter();
        scratch.visual_metadata.insert(
            route.strip_id,
            RealtimeStripSnapshot {
                soundfont_id: identity.soundfont_id.clone(),
                midi_channel: identity.midi_channel.unwrap_or(0),
                midi_bank: identity.midi_bank.unwrap_or(0),
                midi_program: identity.midi_program.unwrap_or(0),
                is_percussion: identity.is_percussion,
                display_name: route.display_name(&scratch.blocks).map(str::to_owned),
                meter,
                audible: meter.peak > f32::EPSILON,
                active_notes: route.active_notes(&scratch.blocks).to_vec(),
            },
        );
    }

    let zero_fill_blocks = scratch.silence_candidates.len();
    let zero_fill_audio_bytes = 0;
    let mixer_frame_bytes = 0usize;
    for (route_index, route) in scratch.rendered_routes.iter().enumerate() {
        let identity = route.identity(&scratch.blocks);
        let mixer_identity = MixerStripIdentity {
            strip_id: route.strip_id,
            soundfont_id: SoundFontId::new(identity.soundfont_id.clone()),
            midi_channel: identity.midi_channel.unwrap_or(0),
            midi_program: identity.midi_program.unwrap_or(0),
            is_percussion: identity.is_percussion,
        };
        scratch.mixer_input_routes.push(MixerInputRoute {
            rendered_route_index: Some(route_index),
            identity: mixer_identity,
            controls: if route.is_global_residual {
                residual_controls_for_soundfont(
                    &scratch.visual_controls_by_soundfont,
                    route.soundfont_slot,
                )
            } else {
                controls
                    .strip_controls
                    .get(&route.strip_id)
                    .copied()
                    .unwrap_or_default()
            },
            automatic_processing: !route.is_global_residual,
        });
    }

    for candidate in &scratch.silence_candidates {
        if !candidate.controls.solo
            && !mixer.strip_has_effect_tail(candidate.strip_id, candidate.controls)
        {
            continue;
        }
        scratch.mixer_input_routes.push(MixerInputRoute {
            rendered_route_index: None,
            identity: MixerStripIdentity {
                strip_id: candidate.strip_id,
                soundfont_id: SoundFontId::new(candidate.identity.soundfont_id.clone()),
                midi_channel: candidate.identity.midi_channel.unwrap_or(0),
                midi_program: candidate.identity.midi_program.unwrap_or(0),
                is_percussion: candidate.identity.is_percussion,
            },
            controls: candidate.controls,
            automatic_processing: true,
        });
    }

    let visual_strips = scratch.visual_metadata.len();
    let routed_blocks = scratch.mixer_input_routes.len();
    let report = {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::RenderMixer);
        mixer.mix_generated_views_interleaved(routed_blocks, output, |index| {
            let route = &scratch.mixer_input_routes[index];
            let block = route
                .rendered_route_index
                .map(|route_index| {
                    scratch.rendered_routes[route_index]
                        .as_mixer_view(&scratch.blocks, soundfont_gain)
                })
                .unwrap_or(AudioBlockView::Silence {
                    frame_count: frames,
                });
            MixerStripInputView {
                identity: route.identity.clone(),
                controls: route.controls,
                automatic_processing: route.automatic_processing,
                block,
            }
        })?
    };
    let strip_mix_diagnostics = build_strip_mix_diagnostics(
        &report.strips,
        &scratch.mixer_input_routes,
        &scratch.rendered_routes,
        &scratch.blocks,
    );
    let final_output_gain = final_output_gain.clamp(0.0, FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER);
    if (final_output_gain - 1.0).abs() > f32::EPSILON {
        for sample in output.iter_mut() {
            *sample *= final_output_gain;
        }
    }
    let output_meter = MeterReading::from_interleaved(output);
    let mixer_internal_alloc_bytes = report.allocation_stats.allocated_bytes;
    let mixer_internal_dealloc_bytes = report.allocation_stats.deallocated_bytes;
    let mixer_scratch_growth_bytes = report.allocation_stats.scratch_growth_bytes;
    let mixer_output_bytes = report.allocation_stats.output_bytes;
    let mixer_prepared_frame_bytes = report.allocation_stats.prepared_rendered_frame_bytes;
    let mixer_report_bytes = report.allocation_stats.reports_bytes;
    let processed_mixer_inputs = report.allocation_stats.processed_inputs;
    let mixer_allocation_events = report.allocation_stats.allocation_events;
    let mixer_deallocation_events = report.allocation_stats.deallocation_events;
    let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::RenderSnapshot);
    let mut snapshot = RealtimeMixerSnapshot {
        output_meter,
        strips: BTreeMap::new(),
    };
    snapshot.strips.append(&mut scratch.visual_metadata);
    for strip in &report.strips {
        if let Some(visual) = snapshot.strips.get_mut(&strip.identity.strip_id) {
            visual.audible = strip.audible && visual.meter.peak > f32::EPSILON;
        }
    }
    let routed_block_overhead = scratch
        .rendered_routes
        .capacity()
        .saturating_mul(mem::size_of::<RoutedStemRoute>());
    let mixer_input_overhead = scratch
        .mixer_input_routes
        .capacity()
        .saturating_mul(mem::size_of::<MixerInputRoute>());
    let vec_overhead_bytes = routed_block_overhead.saturating_add(mixer_input_overhead);
    let retained_container_capacity_bytes = vec_overhead_bytes;
    let temp_container_alloc_bytes = 0;
    let transient_alloc_bytes = stem_audio_bytes
        .saturating_add(stem_untracked_alloc_bytes)
        .saturating_add(zero_fill_audio_bytes)
        .saturating_add(mixer_frame_bytes)
        .saturating_add(mixer_internal_alloc_bytes)
        .saturating_add(vec_overhead_bytes);
    let transient_dealloc_bytes = transient_alloc_bytes;
    let retained_capacity_growth_bytes =
        stem_total_growth_bytes.saturating_add(mixer_scratch_growth_bytes);
    let transient_growth_bytes = retained_capacity_growth_bytes;
    let allocation_events = rendered_blocks
        .saturating_mul(2)
        .saturating_add(routed_blocks)
        .saturating_add(2)
        .saturating_add(mixer_allocation_events);
    let deallocation_events = allocation_events
        .saturating_sub(mixer_allocation_events)
        .saturating_add(mixer_deallocation_events);
    Ok(MixedAudioRenderReport {
        output_meter,
        snapshot,
        frames,
        churn: RenderChurnSnapshot {
            allocation_events,
            deallocation_events,
            frames,
            rendered_blocks,
            routed_blocks,
            mixer_inputs: processed_mixer_inputs,
            zero_fill_blocks,
            visual_strips,
            stem_audio_bytes,
            stem_output_blocks: stem_allocations.output_block_count,
            stem_output_audio_bytes: stem_allocations.output_audio_bytes,
            stem_output_active_note_bytes: stem_allocations.output_active_note_bytes,
            stem_output_vec_bytes: stem_allocations.output_vec_bytes,
            stem_internal_blocks: stem_allocations.internal_block_count,
            stem_internal_audio_bytes: stem_allocations.internal_audio_bytes,
            stem_internal_active_note_bytes: stem_allocations.internal_active_note_bytes,
            stem_internal_vec_bytes: stem_allocations.internal_vec_bytes,
            stem_residual_buffer_bytes: stem_allocations.residual_buffer_bytes,
            stem_effect_input_bytes: stem_allocations.effect_input_bytes,
            stem_effect_input_vec_bytes: stem_allocations.effect_input_vec_bytes,
            stem_output_growth_bytes: stem_allocations.output_growth_bytes,
            stem_internal_growth_bytes: stem_allocations.internal_growth_bytes,
            stem_residual_growth_bytes: stem_allocations.residual_growth_bytes,
            stem_effect_input_growth_bytes: stem_allocations.effect_input_growth_bytes,
            stem_playback_growth_bytes,
            stem_total_growth_bytes,
            stem_tracked_alloc_bytes,
            stem_untracked_alloc_bytes,
            zero_fill_audio_bytes,
            mixer_frame_bytes,
            mixer_internal_alloc_bytes,
            mixer_internal_dealloc_bytes,
            mixer_scratch_growth_bytes,
            mixer_output_bytes,
            mixer_prepared_frame_bytes,
            mixer_report_bytes,
            vec_overhead_bytes,
            retained_container_capacity_bytes,
            temp_container_alloc_bytes,
            retained_capacity_growth_bytes,
            transient_alloc_bytes,
            transient_dealloc_bytes,
            transient_growth_bytes,
        },
        strip_mix_diagnostics,
    })
}

fn build_strip_mix_diagnostics(
    strip_reports: &[StripMixReport],
    mixer_input_routes: &[MixerInputRoute],
    rendered_routes: &[RoutedStemRoute],
    blocks: &[StemRenderBlock],
) -> Vec<RenderProbeStripMixDiagnostics> {
    strip_reports
        .iter()
        .zip(mixer_input_routes)
        .map(|(report, route)| {
            let (left_pan_gain, right_pan_gain) = route.controls.pan_gains();
            let session_gain = route.controls.session_gain(report.solo_active);
            let (display_name, active_notes) = route
                .rendered_route_index
                .map(|index| {
                    let rendered_route = &rendered_routes[index];
                    (
                        rendered_route.display_name(blocks).map(str::to_owned),
                        rendered_route.active_notes(blocks).to_vec(),
                    )
                })
                .unwrap_or((None, Vec::new()));
            RenderProbeStripMixDiagnostics {
                strip_id: report.identity.strip_id,
                soundfont_id: report.identity.soundfont_id.as_str().to_owned(),
                display_name,
                midi_channel: report.identity.midi_channel,
                midi_program: report.identity.midi_program,
                is_percussion: report.identity.is_percussion,
                input_meter: report.input_meter,
                estimated_post_peak: report.input_meter.peak * report.applied_gain,
                estimated_post_rms: report.input_meter.rms * report.applied_gain,
                applied_gain: report.applied_gain,
                session_gain,
                smart_mix_gain: report.smart_mix_gain,
                lookahead_gain: report.lookahead_gain,
                normalization_gain: report.normalization_gain,
                left_pan_gain,
                right_pan_gain,
                automatic_processing: route.automatic_processing,
                mute: route.controls.mute,
                solo: route.controls.solo,
                audible: report.audible,
                active_notes,
            }
        })
        .collect()
}

#[derive(Debug, Default)]
pub(in crate::playback) struct PlaybackRenderScratch {
    pub(in crate::playback) blocks: Vec<StemRenderBlock>,
    rendered_routes: Vec<RoutedStemRoute>,
    mixer_input_routes: Vec<MixerInputRoute>,
    rendered_strip_ids: BTreeSet<StripId>,
    soundfont_slots: BTreeMap<String, usize>,
    visual_metadata: BTreeMap<StripId, RealtimeStripSnapshot>,
    visual_control_ids: BTreeSet<StripId>,
    visual_controls_by_soundfont: BTreeMap<usize, Vec<MixerStripControls>>,
    silence_candidates: Vec<SilenceStemRoute>,
}

impl PlaybackRenderScratch {
    fn begin_render(&mut self) {
        self.rendered_routes.clear();
        self.mixer_input_routes.clear();
        self.rendered_strip_ids.clear();
        self.soundfont_slots.clear();
        self.visual_metadata.clear();
        self.visual_control_ids.clear();
        for controls in self.visual_controls_by_soundfont.values_mut() {
            controls.clear();
        }
        self.visual_controls_by_soundfont.clear();
        self.silence_candidates.clear();
    }

    pub(in crate::playback) fn release_capacity(&mut self) {
        self.blocks.clear();
        self.blocks.shrink_to_fit();
        self.rendered_routes.clear();
        self.rendered_routes.shrink_to_fit();
        self.mixer_input_routes.clear();
        self.mixer_input_routes.shrink_to_fit();
        self.rendered_strip_ids.clear();
        self.soundfont_slots.clear();
        self.visual_metadata.clear();
        self.visual_control_ids.clear();
        self.visual_controls_by_soundfont.clear();
        self.silence_candidates.clear();
        self.silence_candidates.shrink_to_fit();
    }
}

#[derive(Debug)]
struct RoutedStemRoute {
    block_index: usize,
    soundfont_slot: usize,
    strip_id: StripId,
    is_global_residual: bool,
}

#[derive(Debug)]
struct MixerInputRoute {
    rendered_route_index: Option<usize>,
    identity: MixerStripIdentity,
    controls: MixerStripControls,
    automatic_processing: bool,
}

#[derive(Debug)]
struct SilenceStemRoute {
    identity: StemIdentity,
    strip_id: StripId,
    controls: MixerStripControls,
}

impl RoutedStemRoute {
    fn block<'a>(&self, blocks: &'a [StemRenderBlock]) -> &'a StemRenderBlock {
        &blocks[self.block_index]
    }

    fn identity<'a>(&self, blocks: &'a [StemRenderBlock]) -> &'a StemIdentity {
        &self.block(blocks).identity
    }

    fn display_name<'a>(&self, blocks: &'a [StemRenderBlock]) -> Option<&'a str> {
        self.block(blocks).display_name.as_deref()
    }

    fn active_notes<'a>(&self, blocks: &'a [StemRenderBlock]) -> &'a [u8] {
        &self.block(blocks).active_notes
    }

    fn as_mixer_view<'a>(
        &self,
        blocks: &'a [StemRenderBlock],
        soundfont_gain: f32,
    ) -> AudioBlockView<'a> {
        let block = self.block(blocks);
        AudioBlockView::Split {
            left: &block.left,
            right: &block.right,
            gain: soundfont_gain,
        }
    }
}

fn residual_controls_for_soundfont(
    visual_controls_by_soundfont: &BTreeMap<usize, Vec<MixerStripControls>>,
    soundfont_slot: usize,
) -> MixerStripControls {
    let Some(visible_controls) = visual_controls_by_soundfont.get(&soundfont_slot) else {
        return MixerStripControls::default();
    };

    if visible_controls.is_empty() {
        return MixerStripControls::default();
    }

    let muted = visible_controls.iter().any(|controls| controls.mute);
    let solo_count = visible_controls
        .iter()
        .filter(|controls| controls.solo)
        .count();
    let partial_solo = solo_count > 0 && solo_count < visible_controls.len();
    let row_solo = solo_count == visible_controls.len();

    MixerStripControls {
        mute: muted || partial_solo,
        solo: row_solo,
        ..MixerStripControls::default()
    }
}

fn stem_strip_id(font_index: usize, identity: &StemIdentity) -> StripId {
    let soundfont_slot = (font_index as u64 + 1) * 100_000;
    let channel_slot = identity.midi_channel.unwrap_or(16) as u64 * 16_384;
    let bank_slot = identity.midi_bank.unwrap_or(0) as u64 * 128;
    let program_slot = identity.midi_program.unwrap_or(127) as u64;
    let percussion_slot = if identity.is_percussion { 50_000 } else { 0 };
    StripId(soundfont_slot + percussion_slot + channel_slot + bank_slot + program_slot + 1)
}

fn is_global_residual_stem(identity: &StemIdentity) -> bool {
    identity.midi_channel.is_none() && identity.midi_program.is_none() && !identity.is_percussion
}
