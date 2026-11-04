use super::*;

#[derive(Debug, Clone, Default)]
pub struct PerfAudioOutputSnapshot {
    pub ring_retained_bytes: usize,
    pub callback_scratch_bytes: usize,
    pub producer_render_block_bytes: u64,
    pub write_calls: u64,
    pub write_bytes: u64,
    pub last_callback_additional_bytes: u64,
    pub largest_callback_additional_bytes: u64,
    pub last_callback_total_bytes: u64,
    pub largest_callback_total_bytes: u64,
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
    pub stream_id: u64,
    pub shared_strong_count: usize,
    pub shared_weak_count: usize,
    pub callback_shared_strong_count: usize,
    pub callback_shared_weak_count: usize,
}

impl From<&flutz_audio_sdl3::AudioDeviceDiagnostics> for PerfAudioOutputSnapshot {
    fn from(diagnostics: &flutz_audio_sdl3::AudioDeviceDiagnostics) -> Self {
        Self {
            ring_retained_bytes: diagnostics.ring_retained_bytes,
            callback_scratch_bytes: diagnostics.callback_scratch_bytes,
            producer_render_block_bytes: diagnostics.producer_render_block_bytes,
            write_calls: diagnostics.stats.write_calls,
            write_bytes: diagnostics.stats.write_bytes,
            last_callback_additional_bytes: diagnostics.stats.last_callback_additional_bytes,
            largest_callback_additional_bytes: diagnostics.stats.largest_callback_additional_bytes,
            last_callback_total_bytes: diagnostics.stats.last_callback_total_bytes,
            largest_callback_total_bytes: diagnostics.stats.largest_callback_total_bytes,
            init_calls: diagnostics.sdl_lifecycle.init_calls,
            quit_calls: diagnostics.sdl_lifecycle.quit_calls,
            stream_opens: diagnostics.sdl_lifecycle.stream_opens,
            stream_destroys: diagnostics.sdl_lifecycle.stream_destroys,
            stream_resumes: diagnostics.sdl_lifecycle.stream_resumes,
            stream_pauses: diagnostics.sdl_lifecycle.stream_pauses,
            callback_states_created: diagnostics.sdl_lifecycle.callback_states_created,
            callback_states_dropped: diagnostics.sdl_lifecycle.callback_states_dropped,
            producer_threads_started: diagnostics.sdl_lifecycle.producer_threads_started,
            producer_threads_finished: diagnostics.sdl_lifecycle.producer_threads_finished,
            stream_id: diagnostics.references.stream_id,
            shared_strong_count: diagnostics.references.shared_strong_count,
            shared_weak_count: diagnostics.references.shared_weak_count,
            callback_shared_strong_count: diagnostics.references.callback_shared_strong_count,
            callback_shared_weak_count: diagnostics.references.callback_shared_weak_count,
        }
    }
}

impl PerfAudioOutputSnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"ring_retained_bytes\":{},\"callback_scratch_bytes\":{},\"producer_render_block_bytes\":{},\"write_calls\":{},\"write_bytes\":{},\"last_callback_additional_bytes\":{},\"largest_callback_additional_bytes\":{},\"last_callback_total_bytes\":{},\"largest_callback_total_bytes\":{},\"init_calls\":{},\"quit_calls\":{},\"stream_opens\":{},\"stream_destroys\":{},\"stream_resumes\":{},\"stream_pauses\":{},\"callback_states_created\":{},\"callback_states_dropped\":{},\"producer_threads_started\":{},\"producer_threads_finished\":{},\"stream_id\":{},\"shared_strong_count\":{},\"shared_weak_count\":{},\"callback_shared_strong_count\":{},\"callback_shared_weak_count\":{}}}",
            self.ring_retained_bytes,
            self.callback_scratch_bytes,
            self.producer_render_block_bytes,
            self.write_calls,
            self.write_bytes,
            self.last_callback_additional_bytes,
            self.largest_callback_additional_bytes,
            self.last_callback_total_bytes,
            self.largest_callback_total_bytes,
            self.init_calls,
            self.quit_calls,
            self.stream_opens,
            self.stream_destroys,
            self.stream_resumes,
            self.stream_pauses,
            self.callback_states_created,
            self.callback_states_dropped,
            self.producer_threads_started,
            self.producer_threads_finished,
            self.stream_id,
            self.shared_strong_count,
            self.shared_weak_count,
            self.callback_shared_strong_count,
            self.callback_shared_weak_count
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfReferenceSnapshot {
    pub engine: Option<PerfArcReferenceSnapshot>,
    pub mixer: PerfArcReferenceSnapshot,
    pub mixer_controls: PerfArcReferenceSnapshot,
    pub latest_snapshot: PerfArcReferenceSnapshot,
    pub midi_strips: PerfArcReferenceSnapshot,
    pub render_scratch: PerfArcReferenceSnapshot,
    pub render_churn: PerfArcReferenceSnapshot,
    pub audio_stream_open: bool,
}

impl From<&PlaybackDebugMetrics> for PerfReferenceSnapshot {
    fn from(metrics: &PlaybackDebugMetrics) -> Self {
        let references = &metrics.references;
        Self {
            engine: references.engine.map(PerfArcReferenceSnapshot::from),
            mixer: PerfArcReferenceSnapshot::from(references.mixer),
            mixer_controls: PerfArcReferenceSnapshot::from(references.mixer_controls),
            latest_snapshot: PerfArcReferenceSnapshot::from(references.latest_snapshot),
            midi_strips: PerfArcReferenceSnapshot::from(references.midi_strips),
            render_scratch: PerfArcReferenceSnapshot::from(references.render_scratch),
            render_churn: PerfArcReferenceSnapshot::from(references.render_churn),
            audio_stream_open: references.audio_stream_open,
        }
    }
}

impl PerfReferenceSnapshot {
    pub(super) fn to_json(&self) -> String {
        let mut json = format!(
            "{{\"audio_stream_open\":{},\"mixer\":{},\"mixer_controls\":{},\"latest_snapshot\":{},\"midi_strips\":{},\"render_scratch\":{},\"render_churn\":{}",
            self.audio_stream_open,
            self.mixer.to_json(),
            self.mixer_controls.to_json(),
            self.latest_snapshot.to_json(),
            self.midi_strips.to_json(),
            self.render_scratch.to_json(),
            self.render_churn.to_json()
        );
        if let Some(engine) = self.engine {
            json.push_str(",\"engine\":");
            json.push_str(&engine.to_json());
        }
        json.push('}');
        json
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfArcReferenceSnapshot {
    pub strong_count: usize,
    pub weak_count: usize,
    pub expected_strong_roots: usize,
    pub excess_strong_count: usize,
}

impl From<crate::playback::ArcReferenceDiagnostics> for PerfArcReferenceSnapshot {
    fn from(value: crate::playback::ArcReferenceDiagnostics) -> Self {
        Self {
            strong_count: value.strong_count,
            weak_count: value.weak_count,
            expected_strong_roots: value.expected_strong_roots,
            excess_strong_count: value.excess_strong_count,
        }
    }
}

impl PerfArcReferenceSnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"strong\":{},\"weak\":{},\"expected_strong\":{},\"excess_strong\":{}}}",
            self.strong_count,
            self.weak_count,
            self.expected_strong_roots,
            self.excess_strong_count
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfComponentMemorySnapshot {
    pub tracked_total_bytes: usize,
    pub soundfont_metadata: PerfSoundFontMetadataMemorySnapshot,
    pub midi: PerfMidiMemorySnapshot,
    pub rustystem: PerfRustystemMemorySnapshot,
    pub audio: PerfAudioMemorySnapshot,
}

impl From<&PlaybackDebugMetrics> for PerfComponentMemorySnapshot {
    fn from(metrics: &PlaybackDebugMetrics) -> Self {
        let component = &metrics.component_memory;
        Self {
            tracked_total_bytes: component.tracked_total_bytes,
            soundfont_metadata: PerfSoundFontMetadataMemorySnapshot::from(
                component.soundfont_metadata,
            ),
            midi: PerfMidiMemorySnapshot::from(component.midi),
            rustystem: PerfRustystemMemorySnapshot::from(component.rustystem),
            audio: PerfAudioMemorySnapshot::from(component.audio),
        }
    }
}

impl PerfComponentMemorySnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"tracked_total_bytes\":{},\"soundfont_metadata\":{},\"midi\":{},\"rustystem\":{},\"audio\":{}}}",
            self.tracked_total_bytes,
            self.soundfont_metadata.to_json(),
            self.midi.to_json(),
            self.rustystem.to_json(),
            self.audio.to_json()
        )
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfSoundFontMetadataMemorySnapshot {
    pub catalog_entries: usize,
    pub catalog_estimated_bytes: usize,
    pub loaded_soundfont_ids: usize,
    pub loaded_soundfont_id_bytes: usize,
    pub loaded_coverage_entries: usize,
    pub loaded_coverage_estimated_bytes: usize,
    pub estimated_bytes: usize,
}

impl From<crate::playback::SoundFontMetadataDiagnostics> for PerfSoundFontMetadataMemorySnapshot {
    fn from(value: crate::playback::SoundFontMetadataDiagnostics) -> Self {
        Self {
            catalog_entries: value.catalog_entries,
            catalog_estimated_bytes: value.catalog_estimated_bytes,
            loaded_soundfont_ids: value.loaded_soundfont_ids,
            loaded_soundfont_id_bytes: value.loaded_soundfont_id_bytes,
            loaded_coverage_entries: value.loaded_coverage_entries,
            loaded_coverage_estimated_bytes: value.loaded_coverage_estimated_bytes,
            estimated_bytes: value.estimated_bytes,
        }
    }
}

impl PerfSoundFontMetadataMemorySnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"catalog_entries\":{},\"catalog_estimated_bytes\":{},\"loaded_soundfont_ids\":{},\"loaded_soundfont_id_bytes\":{},\"loaded_coverage_entries\":{},\"loaded_coverage_estimated_bytes\":{},\"estimated_bytes\":{}}}",
            self.catalog_entries,
            self.catalog_estimated_bytes,
            self.loaded_soundfont_ids,
            self.loaded_soundfont_id_bytes,
            self.loaded_coverage_entries,
            self.loaded_coverage_estimated_bytes,
            self.estimated_bytes
        )
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfMidiMemorySnapshot {
    pub raw_bytes: usize,
    pub raw_capacity_bytes: usize,
    pub strip_count: usize,
    pub strip_capacity: usize,
    pub strip_bytes: usize,
    pub jump_point_count: usize,
    pub jump_point_bytes: usize,
    pub parsed_message_count: usize,
    pub parsed_sysex_events: usize,
    pub parsed_sysex_bytes: usize,
    pub parsed_estimated_bytes: usize,
    pub estimated_bytes: usize,
}

impl From<crate::playback::MidiRuntimeMemoryDiagnostics> for PerfMidiMemorySnapshot {
    fn from(value: crate::playback::MidiRuntimeMemoryDiagnostics) -> Self {
        Self {
            raw_bytes: value.raw_bytes,
            raw_capacity_bytes: value.raw_capacity_bytes,
            strip_count: value.strip_count,
            strip_capacity: value.strip_capacity,
            strip_bytes: value.strip_bytes,
            jump_point_count: value.jump_point_count,
            jump_point_bytes: value.jump_point_bytes,
            parsed_message_count: value.parsed_message_count,
            parsed_sysex_events: value.parsed_sysex_events,
            parsed_sysex_bytes: value.parsed_sysex_bytes,
            parsed_estimated_bytes: value.parsed_estimated_bytes,
            estimated_bytes: value.estimated_bytes,
        }
    }
}

impl PerfMidiMemorySnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"raw_bytes\":{},\"raw_capacity_bytes\":{},\"strip_count\":{},\"strip_capacity\":{},\"strip_bytes\":{},\"jump_point_count\":{},\"jump_point_bytes\":{},\"parsed_message_count\":{},\"parsed_sysex_events\":{},\"parsed_sysex_bytes\":{},\"parsed_estimated_bytes\":{},\"estimated_bytes\":{}}}",
            self.raw_bytes,
            self.raw_capacity_bytes,
            self.strip_count,
            self.strip_capacity,
            self.strip_bytes,
            self.jump_point_count,
            self.jump_point_bytes,
            self.parsed_message_count,
            self.parsed_sysex_events,
            self.parsed_sysex_bytes,
            self.parsed_estimated_bytes,
            self.estimated_bytes
        )
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfRustystemMemorySnapshot {
    pub instance_count: usize,
    pub soundfont_wave_bytes: usize,
    pub voice_buffer_bytes: usize,
    pub block_buffer_bytes: usize,
    pub effects_bytes: usize,
    pub preset_lookup_bytes: usize,
    pub channel_bytes: usize,
    pub stem_effect_map_bytes: usize,
    pub block_stem_vec_bytes: usize,
    pub estimated_bytes: usize,
}

impl From<crate::playback::RustystemRuntimeDiagnostics> for PerfRustystemMemorySnapshot {
    fn from(value: crate::playback::RustystemRuntimeDiagnostics) -> Self {
        Self {
            instance_count: value.instance_count,
            soundfont_wave_bytes: value.soundfont_wave_bytes,
            voice_buffer_bytes: value.voice_buffer_bytes,
            block_buffer_bytes: value.block_buffer_bytes,
            effects_bytes: value.effects_bytes,
            preset_lookup_bytes: value.preset_lookup_bytes,
            channel_bytes: value.channel_bytes,
            stem_effect_map_bytes: value.stem_effect_map_bytes,
            block_stem_vec_bytes: value.block_stem_vec_bytes,
            estimated_bytes: value.estimated_bytes,
        }
    }
}

impl PerfRustystemMemorySnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"instance_count\":{},\"soundfont_wave_bytes\":{},\"voice_buffer_bytes\":{},\"block_buffer_bytes\":{},\"effects_bytes\":{},\"preset_lookup_bytes\":{},\"channel_bytes\":{},\"stem_effect_map_bytes\":{},\"block_stem_vec_bytes\":{},\"estimated_bytes\":{}}}",
            self.instance_count,
            self.soundfont_wave_bytes,
            self.voice_buffer_bytes,
            self.block_buffer_bytes,
            self.effects_bytes,
            self.preset_lookup_bytes,
            self.channel_bytes,
            self.stem_effect_map_bytes,
            self.block_stem_vec_bytes,
            self.estimated_bytes
        )
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct PerfAudioMemorySnapshot {
    pub stream_open: bool,
    pub ring_retained_bytes: usize,
    pub callback_scratch_bytes: usize,
    pub producer_render_block_bytes: u64,
    pub estimated_bytes: usize,
}

impl From<crate::playback::AudioRuntimeMemoryDiagnostics> for PerfAudioMemorySnapshot {
    fn from(value: crate::playback::AudioRuntimeMemoryDiagnostics) -> Self {
        Self {
            stream_open: value.stream_open,
            ring_retained_bytes: value.ring_retained_bytes,
            callback_scratch_bytes: value.callback_scratch_bytes,
            producer_render_block_bytes: value.producer_render_block_bytes,
            estimated_bytes: value.estimated_bytes,
        }
    }
}

impl PerfAudioMemorySnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"stream_open\":{},\"ring_retained_bytes\":{},\"callback_scratch_bytes\":{},\"producer_render_block_bytes\":{},\"estimated_bytes\":{}}}",
            self.stream_open,
            self.ring_retained_bytes,
            self.callback_scratch_bytes,
            self.producer_render_block_bytes,
            self.estimated_bytes
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfSoundFontCacheSnapshot {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub decoded_bytes: u64,
    pub resident_bytes: usize,
    pub evictions: u64,
    pub total_strong_count: usize,
    pub max_strong_count: usize,
    pub entry_refs: Vec<PerfSoundFontCacheEntrySnapshot>,
}

impl From<&SoundFontRuntimeCacheDebug> for PerfSoundFontCacheSnapshot {
    fn from(debug: &SoundFontRuntimeCacheDebug) -> Self {
        Self {
            entries: debug.entries,
            hits: debug.hits,
            misses: debug.misses,
            decoded_bytes: debug.decoded_bytes,
            resident_bytes: debug.resident_bytes,
            evictions: debug.evictions,
            total_strong_count: debug.total_strong_count,
            max_strong_count: debug.max_strong_count,
            entry_refs: debug
                .entry_refs
                .iter()
                .map(PerfSoundFontCacheEntrySnapshot::from)
                .collect(),
        }
    }
}

impl PerfSoundFontCacheSnapshot {
    pub(super) fn to_json(&self) -> String {
        let entry_refs = self
            .entry_refs
            .iter()
            .map(PerfSoundFontCacheEntrySnapshot::to_json)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"entries\":{},\"hits\":{},\"misses\":{},\"decoded_bytes\":{},\"resident_bytes\":{},\"evictions\":{},\"total_strong_count\":{},\"max_strong_count\":{},\"entry_refs\":[{}]}}",
            self.entries,
            self.hits,
            self.misses,
            self.decoded_bytes,
            self.resident_bytes,
            self.evictions,
            self.total_strong_count,
            self.max_strong_count,
            entry_refs
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfSoundFontCacheEntrySnapshot {
    pub key: String,
    pub strong_count: usize,
    pub weak_count: usize,
    pub resident_bytes: usize,
}

impl From<&SoundFontRuntimeCacheEntryDebug> for PerfSoundFontCacheEntrySnapshot {
    fn from(debug: &SoundFontRuntimeCacheEntryDebug) -> Self {
        Self {
            key: debug.key.clone(),
            strong_count: debug.strong_count,
            weak_count: debug.weak_count,
            resident_bytes: debug.resident_bytes,
        }
    }
}

impl PerfSoundFontCacheEntrySnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"key\":\"{}\",\"strong\":{},\"weak\":{},\"resident_bytes\":{}}}",
            escape_json(&self.key),
            self.strong_count,
            self.weak_count,
            self.resident_bytes
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfLifecycleSnapshot {
    pub engine_replacements: u64,
    pub cumulative_replaced_engine_estimated_bytes: u64,
    pub last_replaced_engine_estimated_bytes: usize,
    pub last_replaced_engine_instances: usize,
    pub last_replaced_stem_effects: usize,
    pub last_replaced_stem_effect_bytes: usize,
}

impl From<&PlaybackDebugMetrics> for PerfLifecycleSnapshot {
    fn from(metrics: &PlaybackDebugMetrics) -> Self {
        Self {
            engine_replacements: metrics.lifecycle.engine_replacements,
            cumulative_replaced_engine_estimated_bytes: metrics
                .lifecycle
                .cumulative_replaced_engine_estimated_bytes,
            last_replaced_engine_estimated_bytes: metrics
                .lifecycle
                .last_replaced_engine_estimated_bytes,
            last_replaced_engine_instances: metrics.lifecycle.last_replaced_engine_instances,
            last_replaced_stem_effects: metrics.lifecycle.last_replaced_stem_effects,
            last_replaced_stem_effect_bytes: metrics.lifecycle.last_replaced_stem_effect_bytes,
        }
    }
}

impl PerfLifecycleSnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"engine_replacements\":{},\"cumulative_replaced_engine_estimated_bytes\":{},\"last_replaced_engine_estimated_bytes\":{},\"last_replaced_engine_instances\":{},\"last_replaced_stem_effects\":{},\"last_replaced_stem_effect_bytes\":{}}}",
            self.engine_replacements,
            self.cumulative_replaced_engine_estimated_bytes,
            self.last_replaced_engine_estimated_bytes,
            self.last_replaced_engine_instances,
            self.last_replaced_stem_effects,
            self.last_replaced_stem_effect_bytes
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct PerfRenderChurnSnapshot {
    pub render_calls: u64,
    pub cumulative_allocation_events: u64,
    pub cumulative_deallocation_events: u64,
    pub cumulative_transient_alloc_bytes: u64,
    pub cumulative_transient_dealloc_bytes: u64,
    pub cumulative_transient_growth_bytes: u64,
    pub cumulative_retained_capacity_growth_bytes: u64,
    pub cumulative_temp_container_alloc_bytes: u64,
    pub max_transient_alloc_bytes: usize,
    pub max_transient_growth_bytes: usize,
    pub max_retained_capacity_growth_bytes: usize,
    pub max_temp_container_alloc_bytes: usize,
    pub last_allocation_events: usize,
    pub last_deallocation_events: usize,
    pub last_frames: usize,
    pub last_rendered_blocks: usize,
    pub last_routed_blocks: usize,
    pub last_mixer_inputs: usize,
    pub last_zero_fill_blocks: usize,
    pub last_visual_strips: usize,
    pub last_stem_audio_bytes: usize,
    pub last_stem_output_blocks: usize,
    pub last_stem_output_audio_bytes: usize,
    pub last_stem_output_active_note_bytes: usize,
    pub last_stem_output_vec_bytes: usize,
    pub last_stem_internal_blocks: usize,
    pub last_stem_internal_audio_bytes: usize,
    pub last_stem_internal_active_note_bytes: usize,
    pub last_stem_internal_vec_bytes: usize,
    pub last_stem_residual_buffer_bytes: usize,
    pub last_stem_effect_input_bytes: usize,
    pub last_stem_effect_input_vec_bytes: usize,
    pub last_stem_output_growth_bytes: usize,
    pub last_stem_internal_growth_bytes: usize,
    pub last_stem_residual_growth_bytes: usize,
    pub last_stem_effect_input_growth_bytes: usize,
    pub last_stem_playback_growth_bytes: usize,
    pub last_stem_total_growth_bytes: usize,
    pub last_stem_tracked_alloc_bytes: usize,
    pub last_stem_untracked_alloc_bytes: usize,
    pub last_zero_fill_audio_bytes: usize,
    pub last_mixer_frame_bytes: usize,
    pub last_mixer_internal_alloc_bytes: usize,
    pub last_mixer_internal_dealloc_bytes: usize,
    pub last_mixer_scratch_growth_bytes: usize,
    pub last_mixer_output_bytes: usize,
    pub last_mixer_prepared_frame_bytes: usize,
    pub last_mixer_report_bytes: usize,
    pub last_vec_overhead_bytes: usize,
    pub last_retained_container_capacity_bytes: usize,
    pub last_temp_container_alloc_bytes: usize,
    pub last_retained_capacity_growth_bytes: usize,
    pub last_transient_alloc_bytes: usize,
    pub last_transient_dealloc_bytes: usize,
    pub last_transient_growth_bytes: usize,
}

impl From<&PlaybackDebugMetrics> for PerfRenderChurnSnapshot {
    fn from(metrics: &PlaybackDebugMetrics) -> Self {
        let churn = &metrics.render_churn;
        Self {
            render_calls: churn.render_calls,
            cumulative_allocation_events: churn.cumulative_allocation_events,
            cumulative_deallocation_events: churn.cumulative_deallocation_events,
            cumulative_transient_alloc_bytes: churn.cumulative_transient_alloc_bytes,
            cumulative_transient_dealloc_bytes: churn.cumulative_transient_dealloc_bytes,
            cumulative_transient_growth_bytes: churn.cumulative_transient_growth_bytes,
            cumulative_retained_capacity_growth_bytes: churn
                .cumulative_retained_capacity_growth_bytes,
            cumulative_temp_container_alloc_bytes: churn.cumulative_temp_container_alloc_bytes,
            max_transient_alloc_bytes: churn.max_transient_alloc_bytes,
            max_transient_growth_bytes: churn.max_transient_growth_bytes,
            max_retained_capacity_growth_bytes: churn.max_retained_capacity_growth_bytes,
            max_temp_container_alloc_bytes: churn.max_temp_container_alloc_bytes,
            last_allocation_events: churn.last.allocation_events,
            last_deallocation_events: churn.last.deallocation_events,
            last_frames: churn.last.frames,
            last_rendered_blocks: churn.last.rendered_blocks,
            last_routed_blocks: churn.last.routed_blocks,
            last_mixer_inputs: churn.last.mixer_inputs,
            last_zero_fill_blocks: churn.last.zero_fill_blocks,
            last_visual_strips: churn.last.visual_strips,
            last_stem_audio_bytes: churn.last.stem_audio_bytes,
            last_stem_output_blocks: churn.last.stem_output_blocks,
            last_stem_output_audio_bytes: churn.last.stem_output_audio_bytes,
            last_stem_output_active_note_bytes: churn.last.stem_output_active_note_bytes,
            last_stem_output_vec_bytes: churn.last.stem_output_vec_bytes,
            last_stem_internal_blocks: churn.last.stem_internal_blocks,
            last_stem_internal_audio_bytes: churn.last.stem_internal_audio_bytes,
            last_stem_internal_active_note_bytes: churn.last.stem_internal_active_note_bytes,
            last_stem_internal_vec_bytes: churn.last.stem_internal_vec_bytes,
            last_stem_residual_buffer_bytes: churn.last.stem_residual_buffer_bytes,
            last_stem_effect_input_bytes: churn.last.stem_effect_input_bytes,
            last_stem_effect_input_vec_bytes: churn.last.stem_effect_input_vec_bytes,
            last_stem_output_growth_bytes: churn.last.stem_output_growth_bytes,
            last_stem_internal_growth_bytes: churn.last.stem_internal_growth_bytes,
            last_stem_residual_growth_bytes: churn.last.stem_residual_growth_bytes,
            last_stem_effect_input_growth_bytes: churn.last.stem_effect_input_growth_bytes,
            last_stem_playback_growth_bytes: churn.last.stem_playback_growth_bytes,
            last_stem_total_growth_bytes: churn.last.stem_total_growth_bytes,
            last_stem_tracked_alloc_bytes: churn.last.stem_tracked_alloc_bytes,
            last_stem_untracked_alloc_bytes: churn.last.stem_untracked_alloc_bytes,
            last_zero_fill_audio_bytes: churn.last.zero_fill_audio_bytes,
            last_mixer_frame_bytes: churn.last.mixer_frame_bytes,
            last_mixer_internal_alloc_bytes: churn.last.mixer_internal_alloc_bytes,
            last_mixer_internal_dealloc_bytes: churn.last.mixer_internal_dealloc_bytes,
            last_mixer_scratch_growth_bytes: churn.last.mixer_scratch_growth_bytes,
            last_mixer_output_bytes: churn.last.mixer_output_bytes,
            last_mixer_prepared_frame_bytes: churn.last.mixer_prepared_frame_bytes,
            last_mixer_report_bytes: churn.last.mixer_report_bytes,
            last_vec_overhead_bytes: churn.last.vec_overhead_bytes,
            last_retained_container_capacity_bytes: churn.last.retained_container_capacity_bytes,
            last_temp_container_alloc_bytes: churn.last.temp_container_alloc_bytes,
            last_retained_capacity_growth_bytes: churn.last.retained_capacity_growth_bytes,
            last_transient_alloc_bytes: churn.last.transient_alloc_bytes,
            last_transient_dealloc_bytes: churn.last.transient_dealloc_bytes,
            last_transient_growth_bytes: churn.last.transient_growth_bytes,
        }
    }
}

impl PerfRenderChurnSnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"render_calls\":{},\"cumulative_allocation_events\":{},\"cumulative_deallocation_events\":{},\"cumulative_transient_alloc_bytes\":{},\"cumulative_transient_dealloc_bytes\":{},\"cumulative_transient_growth_bytes\":{},\"cumulative_retained_capacity_growth_bytes\":{},\"cumulative_temp_container_alloc_bytes\":{},\"max_transient_alloc_bytes\":{},\"max_transient_growth_bytes\":{},\"max_retained_capacity_growth_bytes\":{},\"max_temp_container_alloc_bytes\":{},\"last_allocation_events\":{},\"last_deallocation_events\":{},\"last_frames\":{},\"last_rendered_blocks\":{},\"last_routed_blocks\":{},\"last_mixer_inputs\":{},\"last_zero_fill_blocks\":{},\"last_visual_strips\":{},\"last_stem_audio_bytes\":{},\"last_stem_output_blocks\":{},\"last_stem_output_audio_bytes\":{},\"last_stem_output_active_note_bytes\":{},\"last_stem_output_vec_bytes\":{},\"last_stem_internal_blocks\":{},\"last_stem_internal_audio_bytes\":{},\"last_stem_internal_active_note_bytes\":{},\"last_stem_internal_vec_bytes\":{},\"last_stem_residual_buffer_bytes\":{},\"last_stem_effect_input_bytes\":{},\"last_stem_effect_input_vec_bytes\":{},\"last_stem_output_growth_bytes\":{},\"last_stem_internal_growth_bytes\":{},\"last_stem_residual_growth_bytes\":{},\"last_stem_effect_input_growth_bytes\":{},\"last_stem_playback_growth_bytes\":{},\"last_stem_total_growth_bytes\":{},\"last_stem_tracked_alloc_bytes\":{},\"last_stem_untracked_alloc_bytes\":{},\"last_zero_fill_audio_bytes\":{},\"last_mixer_frame_bytes\":{},\"last_mixer_internal_alloc_bytes\":{},\"last_mixer_internal_dealloc_bytes\":{},\"last_mixer_scratch_growth_bytes\":{},\"last_mixer_output_bytes\":{},\"last_mixer_prepared_frame_bytes\":{},\"last_mixer_report_bytes\":{},\"last_vec_overhead_bytes\":{},\"last_retained_container_capacity_bytes\":{},\"last_temp_container_alloc_bytes\":{},\"last_retained_capacity_growth_bytes\":{},\"last_transient_alloc_bytes\":{},\"last_transient_dealloc_bytes\":{},\"last_transient_growth_bytes\":{}}}",
            self.render_calls,
            self.cumulative_allocation_events,
            self.cumulative_deallocation_events,
            self.cumulative_transient_alloc_bytes,
            self.cumulative_transient_dealloc_bytes,
            self.cumulative_transient_growth_bytes,
            self.cumulative_retained_capacity_growth_bytes,
            self.cumulative_temp_container_alloc_bytes,
            self.max_transient_alloc_bytes,
            self.max_transient_growth_bytes,
            self.max_retained_capacity_growth_bytes,
            self.max_temp_container_alloc_bytes,
            self.last_allocation_events,
            self.last_deallocation_events,
            self.last_frames,
            self.last_rendered_blocks,
            self.last_routed_blocks,
            self.last_mixer_inputs,
            self.last_zero_fill_blocks,
            self.last_visual_strips,
            self.last_stem_audio_bytes,
            self.last_stem_output_blocks,
            self.last_stem_output_audio_bytes,
            self.last_stem_output_active_note_bytes,
            self.last_stem_output_vec_bytes,
            self.last_stem_internal_blocks,
            self.last_stem_internal_audio_bytes,
            self.last_stem_internal_active_note_bytes,
            self.last_stem_internal_vec_bytes,
            self.last_stem_residual_buffer_bytes,
            self.last_stem_effect_input_bytes,
            self.last_stem_effect_input_vec_bytes,
            self.last_stem_output_growth_bytes,
            self.last_stem_internal_growth_bytes,
            self.last_stem_residual_growth_bytes,
            self.last_stem_effect_input_growth_bytes,
            self.last_stem_playback_growth_bytes,
            self.last_stem_total_growth_bytes,
            self.last_stem_tracked_alloc_bytes,
            self.last_stem_untracked_alloc_bytes,
            self.last_zero_fill_audio_bytes,
            self.last_mixer_frame_bytes,
            self.last_mixer_internal_alloc_bytes,
            self.last_mixer_internal_dealloc_bytes,
            self.last_mixer_scratch_growth_bytes,
            self.last_mixer_output_bytes,
            self.last_mixer_prepared_frame_bytes,
            self.last_mixer_report_bytes,
            self.last_vec_overhead_bytes,
            self.last_retained_container_capacity_bytes,
            self.last_temp_container_alloc_bytes,
            self.last_retained_capacity_growth_bytes,
            self.last_transient_alloc_bytes,
            self.last_transient_dealloc_bytes,
            self.last_transient_growth_bytes
        )
    }
}

#[derive(Debug, Clone)]
pub struct PerfMemorySnapshot {
    pub midi_bytes: usize,
    pub midi_capacity_bytes: usize,
    pub instance_count: usize,
    pub soundfont_wave_bytes: usize,
    pub soundfont_metadata_items: usize,
    pub total_voices: usize,
    pub active_voices: usize,
    pub voice_buffer_bytes: usize,
    pub block_buffer_bytes: usize,
    pub retained_stem_blocks: usize,
    pub retained_stem_block_bytes: usize,
    pub stem_effects: usize,
    pub stem_effect_bytes: usize,
    pub stem_effect_allocations: u64,
    pub stem_effect_deallocations: u64,
    pub stem_effect_allocated_bytes: u64,
    pub stem_effect_deallocated_bytes: u64,
    pub stem_effect_cache_clears: u64,
    pub stem_effect_cache_released_bytes: u64,
    pub last_stem_render_allocations: StemRenderAllocationDebug,
    pub effects_bytes: usize,
    pub preset_lookup_bytes: usize,
    pub channel_bytes: usize,
    pub stem_effect_map_bytes: usize,
    pub block_stem_vec_bytes: usize,
    pub estimated_bytes: usize,
    pub instances: Vec<PerfMemoryInstanceSnapshot>,
}

impl PerfMemorySnapshot {
    pub(super) fn from_playback(
        memory: &PlaybackMemoryDebug,
        midi_bytes: usize,
        midi_capacity_bytes: usize,
    ) -> Self {
        Self {
            midi_bytes,
            midi_capacity_bytes,
            instance_count: memory.instance_count,
            soundfont_wave_bytes: memory.soundfont_wave_bytes,
            soundfont_metadata_items: memory.soundfont_metadata_items,
            total_voices: memory.total_voices,
            active_voices: memory.active_voices,
            voice_buffer_bytes: memory.voice_buffer_bytes,
            block_buffer_bytes: memory.block_buffer_bytes,
            retained_stem_blocks: memory.retained_stem_blocks,
            retained_stem_block_bytes: memory.retained_stem_block_bytes,
            stem_effects: memory.stem_effects,
            stem_effect_bytes: memory.stem_effect_bytes,
            stem_effect_allocations: memory.stem_effect_allocations,
            stem_effect_deallocations: memory.stem_effect_deallocations,
            stem_effect_allocated_bytes: memory.stem_effect_allocated_bytes,
            stem_effect_deallocated_bytes: memory.stem_effect_deallocated_bytes,
            stem_effect_cache_clears: memory.stem_effect_cache_clears,
            stem_effect_cache_released_bytes: memory.stem_effect_cache_released_bytes,
            last_stem_render_allocations: memory.last_stem_render_allocations,
            effects_bytes: memory.effects_bytes,
            preset_lookup_bytes: memory.preset_lookup_bytes,
            channel_bytes: memory.channel_bytes,
            stem_effect_map_bytes: memory.stem_effect_map_bytes,
            block_stem_vec_bytes: memory.block_stem_vec_bytes,
            estimated_bytes: memory.estimated_bytes,
            instances: memory
                .instances
                .iter()
                .map(PerfMemoryInstanceSnapshot::from)
                .collect(),
        }
    }

    pub(super) fn to_json(&self) -> String {
        let mut json = format!(
            "{{\"midi_bytes\":{},\"midi_capacity_bytes\":{},\"instances\":{},\"soundfont_wave_bytes\":{},\"soundfont_metadata_items\":{},\"total_voices\":{},\"active_voices\":{},\"voice_buffer_bytes\":{},\"block_buffer_bytes\":{},\"retained_stem_blocks\":{},\"retained_stem_block_bytes\":{},\"stem_effects\":{},\"stem_effect_bytes\":{},\"stem_effect_allocations\":{},\"stem_effect_deallocations\":{},\"stem_effect_allocated_bytes\":{},\"stem_effect_deallocated_bytes\":{},\"stem_effect_cache_clears\":{},\"stem_effect_cache_released_bytes\":{},\"last_stem_render_allocations\":{},\"effects_bytes\":{},\"preset_lookup_bytes\":{},\"channel_bytes\":{},\"stem_effect_map_bytes\":{},\"block_stem_vec_bytes\":{},\"estimated_bytes\":{}",
            self.midi_bytes,
            self.midi_capacity_bytes,
            self.instance_count,
            self.soundfont_wave_bytes,
            self.soundfont_metadata_items,
            self.total_voices,
            self.active_voices,
            self.voice_buffer_bytes,
            self.block_buffer_bytes,
            self.retained_stem_blocks,
            self.retained_stem_block_bytes,
            self.stem_effects,
            self.stem_effect_bytes,
            self.stem_effect_allocations,
            self.stem_effect_deallocations,
            self.stem_effect_allocated_bytes,
            self.stem_effect_deallocated_bytes,
            self.stem_effect_cache_clears,
            self.stem_effect_cache_released_bytes,
            stem_render_allocations_json(self.last_stem_render_allocations),
            self.effects_bytes,
            self.preset_lookup_bytes,
            self.channel_bytes,
            self.stem_effect_map_bytes,
            self.block_stem_vec_bytes,
            self.estimated_bytes
        );
        if !self.instances.is_empty() {
            json.push_str(",\"instance_details\":[");
            for (index, instance) in self.instances.iter().enumerate() {
                if index > 0 {
                    json.push(',');
                }
                json.push_str(&instance.to_json());
            }
            json.push(']');
        }
        json.push('}');
        json
    }
}

#[derive(Debug, Clone)]
pub struct PerfMemoryInstanceSnapshot {
    pub internal_id: String,
    pub display_name: String,
    pub soundfont_wave_bytes: usize,
    pub soundfont_sample_headers: usize,
    pub soundfont_presets: usize,
    pub soundfont_instruments: usize,
    pub total_voices: usize,
    pub active_voices: usize,
    pub voice_buffer_bytes: usize,
    pub retained_stem_blocks: usize,
    pub retained_stem_block_bytes: usize,
    pub stem_effects: usize,
    pub stem_effect_bytes: usize,
    pub stem_effect_allocations: u64,
    pub stem_effect_deallocations: u64,
    pub stem_effect_allocated_bytes: u64,
    pub stem_effect_deallocated_bytes: u64,
    pub stem_effect_cache_clears: u64,
    pub stem_effect_cache_released_bytes: u64,
    pub last_stem_render_allocations: StemRenderAllocationDebug,
    pub effects_bytes: usize,
    pub preset_lookup_bytes: usize,
    pub channel_bytes: usize,
    pub stem_effect_map_bytes: usize,
    pub block_stem_vec_bytes: usize,
    pub estimated_bytes: usize,
}

impl From<&SynthInstanceMemoryDebug> for PerfMemoryInstanceSnapshot {
    fn from(instance: &SynthInstanceMemoryDebug) -> Self {
        Self {
            internal_id: instance.internal_id.clone(),
            display_name: instance.display_name.clone(),
            soundfont_wave_bytes: instance.synth.soundfont_wave_bytes,
            soundfont_sample_headers: instance.synth.soundfont_sample_headers,
            soundfont_presets: instance.synth.soundfont_presets,
            soundfont_instruments: instance.synth.soundfont_instruments,
            total_voices: instance.synth.total_voices,
            active_voices: instance.synth.active_voices,
            voice_buffer_bytes: instance.synth.voice_buffer_bytes,
            retained_stem_blocks: instance.synth.retained_stem_blocks,
            retained_stem_block_bytes: instance.synth.retained_stem_block_bytes,
            stem_effects: instance.synth.stem_effects,
            stem_effect_bytes: instance.synth.stem_effect_bytes,
            stem_effect_allocations: instance.synth.stem_effect_allocations,
            stem_effect_deallocations: instance.synth.stem_effect_deallocations,
            stem_effect_allocated_bytes: instance.synth.stem_effect_allocated_bytes,
            stem_effect_deallocated_bytes: instance.synth.stem_effect_deallocated_bytes,
            stem_effect_cache_clears: instance.synth.stem_effect_cache_clears,
            stem_effect_cache_released_bytes: instance.synth.stem_effect_cache_released_bytes,
            last_stem_render_allocations: instance.synth.last_stem_render_allocations,
            effects_bytes: instance.synth.effects_bytes,
            preset_lookup_bytes: instance.synth.preset_lookup_bytes,
            channel_bytes: instance.synth.channel_bytes,
            stem_effect_map_bytes: instance.synth.stem_effect_map_bytes,
            block_stem_vec_bytes: instance.synth.block_stem_vec_bytes,
            estimated_bytes: instance.synth.estimated_bytes,
        }
    }
}

impl PerfMemoryInstanceSnapshot {
    pub(super) fn to_json(&self) -> String {
        format!(
            "{{\"id\":\"{}\",\"display\":\"{}\",\"soundfont_wave_bytes\":{},\"soundfont_sample_headers\":{},\"soundfont_presets\":{},\"soundfont_instruments\":{},\"total_voices\":{},\"active_voices\":{},\"voice_buffer_bytes\":{},\"retained_stem_blocks\":{},\"retained_stem_block_bytes\":{},\"stem_effects\":{},\"stem_effect_bytes\":{},\"stem_effect_allocations\":{},\"stem_effect_deallocations\":{},\"stem_effect_allocated_bytes\":{},\"stem_effect_deallocated_bytes\":{},\"stem_effect_cache_clears\":{},\"stem_effect_cache_released_bytes\":{},\"last_stem_render_allocations\":{},\"effects_bytes\":{},\"preset_lookup_bytes\":{},\"channel_bytes\":{},\"stem_effect_map_bytes\":{},\"block_stem_vec_bytes\":{},\"estimated_bytes\":{}}}",
            escape_json(&self.internal_id),
            escape_json(&self.display_name),
            self.soundfont_wave_bytes,
            self.soundfont_sample_headers,
            self.soundfont_presets,
            self.soundfont_instruments,
            self.total_voices,
            self.active_voices,
            self.voice_buffer_bytes,
            self.retained_stem_blocks,
            self.retained_stem_block_bytes,
            self.stem_effects,
            self.stem_effect_bytes,
            self.stem_effect_allocations,
            self.stem_effect_deallocations,
            self.stem_effect_allocated_bytes,
            self.stem_effect_deallocated_bytes,
            self.stem_effect_cache_clears,
            self.stem_effect_cache_released_bytes,
            stem_render_allocations_json(self.last_stem_render_allocations),
            self.effects_bytes,
            self.preset_lookup_bytes,
            self.channel_bytes,
            self.stem_effect_map_bytes,
            self.block_stem_vec_bytes,
            self.estimated_bytes
        )
    }
}

fn stem_render_allocations_json(allocations: StemRenderAllocationDebug) -> String {
    format!(
        "{{\"output_block_count\":{},\"output_audio_bytes\":{},\"output_active_note_bytes\":{},\"output_vec_bytes\":{},\"internal_block_count\":{},\"internal_audio_bytes\":{},\"internal_active_note_bytes\":{},\"internal_vec_bytes\":{},\"residual_buffer_bytes\":{},\"effect_input_bytes\":{},\"effect_input_vec_bytes\":{},\"output_growth_bytes\":{},\"internal_growth_bytes\":{},\"residual_growth_bytes\":{},\"effect_input_growth_bytes\":{},\"total_bytes\":{},\"total_growth_bytes\":{}}}",
        allocations.output_block_count,
        allocations.output_audio_bytes,
        allocations.output_active_note_bytes,
        allocations.output_vec_bytes,
        allocations.internal_block_count,
        allocations.internal_audio_bytes,
        allocations.internal_active_note_bytes,
        allocations.internal_vec_bytes,
        allocations.residual_buffer_bytes,
        allocations.effect_input_bytes,
        allocations.effect_input_vec_bytes,
        allocations.output_growth_bytes,
        allocations.internal_growth_bytes,
        allocations.residual_growth_bytes,
        allocations.effect_input_growth_bytes,
        allocations.total_bytes(),
        allocations.total_growth_bytes()
    )
}
