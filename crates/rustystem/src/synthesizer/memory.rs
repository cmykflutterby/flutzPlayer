use super::*;

impl Synthesizer {
    pub fn memory_debug(&self) -> SynthesizerMemoryDebug {
        let soundfont_wave_bytes = self.sound_font.wave_data.len() * mem::size_of::<i16>();
        let soundfont_metadata_items = self.sound_font.sample_headers.len()
            + self.sound_font.presets.len()
            + self.sound_font.instruments.len();
        let block_buffer_bytes = f32_vec_bytes(&self.block_left) + f32_vec_bytes(&self.block_right);
        let retained_stem_block_bytes = self
            .block_stems
            .iter()
            .map(stem_render_block_buffer_bytes)
            .sum();
        let stem_effect_bytes = self
            .stem_effects
            .values()
            .map(StemEffects::memory_debug_bytes)
            .sum();
        let effects_bytes = self
            .effects
            .as_ref()
            .map(Effects::memory_debug_bytes)
            .unwrap_or_default();
        let voice_buffer_bytes = self.voices.voice_buffer_bytes();
        let preset_lookup_bytes =
            self.preset_lookup.capacity() * (mem::size_of::<i32>() + mem::size_of::<usize>());
        let channel_bytes = self.channels.capacity() * mem::size_of::<Channel>();
        let stem_effect_map_bytes = self.stem_effects.capacity()
            * (mem::size_of::<StemIdentity>() + mem::size_of::<StemEffects>());
        let block_stem_vec_bytes = self.block_stems.capacity() * mem::size_of::<StemRenderBlock>();
        let volume_envelope = self.voices.volume_envelope_debug();
        SynthesizerMemoryDebug {
            soundfont_wave_bytes,
            soundfont_sample_headers: self.sound_font.sample_headers.len(),
            soundfont_presets: self.sound_font.presets.len(),
            soundfont_instruments: self.sound_font.instruments.len(),
            soundfont_metadata_items,
            total_voices: self.voices.voice_count(),
            active_voices: self.voices.active_voice_count,
            max_active_voices: self.voices.max_active_voice_count,
            total_voice_requests: self.voices.total_voice_requests,
            exclusive_class_reuses: self.voices.exclusive_class_reuses,
            free_voice_allocations: self.voices.free_voice_allocations,
            contention_steals: self.voices.contention_steals,
            env_delay_voices: volume_envelope.delay_voices,
            env_attack_voices: volume_envelope.attack_voices,
            env_hold_voices: volume_envelope.hold_voices,
            env_decay_voices: volume_envelope.decay_voices,
            env_release_voices: volume_envelope.release_voices,
            env_value_sum: volume_envelope.value_sum,
            env_value_avg: volume_envelope.average_value(self.voices.active_voice_count),
            voice_buffer_bytes,
            block_buffer_bytes,
            retained_stem_blocks: self.block_stems.len(),
            retained_stem_block_bytes,
            stem_effects: self.stem_effects.len(),
            stem_effect_bytes,
            stem_effect_allocations: self.stem_effect_allocation_counters.allocations,
            stem_effect_deallocations: self.stem_effect_allocation_counters.deallocations,
            stem_effect_allocated_bytes: self.stem_effect_allocation_counters.allocated_bytes,
            stem_effect_deallocated_bytes: self.stem_effect_allocation_counters.deallocated_bytes,
            stem_effect_cache_clears: self.stem_effect_allocation_counters.cache_clears,
            stem_effect_cache_released_bytes: self
                .stem_effect_allocation_counters
                .cache_released_bytes,
            last_stem_render_allocations: self.last_stem_render_allocations,
            effects_bytes,
            preset_lookup_bytes,
            channel_bytes,
            stem_effect_map_bytes,
            block_stem_vec_bytes,
            estimated_bytes: soundfont_wave_bytes
                + voice_buffer_bytes
                + block_buffer_bytes
                + retained_stem_block_bytes
                + stem_effect_bytes
                + effects_bytes
                + preset_lookup_bytes
                + channel_bytes
                + stem_effect_map_bytes
                + block_stem_vec_bytes,
        }
    }

    pub fn last_stem_render_allocations(&self) -> StemRenderAllocationDebug {
        self.last_stem_render_allocations
    }
}

#[derive(Debug, Clone, Default)]
pub struct SynthesizerMemoryDebug {
    pub soundfont_wave_bytes: usize,
    pub soundfont_sample_headers: usize,
    pub soundfont_presets: usize,
    pub soundfont_instruments: usize,
    pub soundfont_metadata_items: usize,
    pub total_voices: usize,
    pub active_voices: usize,
    pub max_active_voices: usize,
    pub total_voice_requests: u64,
    pub exclusive_class_reuses: u64,
    pub free_voice_allocations: u64,
    pub contention_steals: u64,
    pub env_delay_voices: usize,
    pub env_attack_voices: usize,
    pub env_hold_voices: usize,
    pub env_decay_voices: usize,
    pub env_release_voices: usize,
    pub env_value_sum: f32,
    pub env_value_avg: f32,
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
}

pub(super) fn f32_vec_bytes(values: &Vec<f32>) -> usize {
    values.capacity() * mem::size_of::<f32>()
}

pub(super) fn stem_render_block_buffer_bytes(block: &StemRenderBlock) -> usize {
    f32_vec_bytes(&block.left)
        + f32_vec_bytes(&block.right)
        + block.active_notes.capacity() * mem::size_of::<u8>()
}

pub(super) fn stem_render_blocks_audio_bytes(blocks: &[StemRenderBlock]) -> usize {
    blocks
        .iter()
        .map(|block| f32_vec_bytes(&block.left) + f32_vec_bytes(&block.right))
        .sum()
}

pub(super) fn stem_render_blocks_active_note_bytes(blocks: &[StemRenderBlock]) -> usize {
    blocks
        .iter()
        .map(|block| block.active_notes.capacity() * mem::size_of::<u8>())
        .sum()
}

pub(super) fn stem_render_blocks_total_bytes(blocks: &Vec<StemRenderBlock>) -> usize {
    blocks
        .capacity()
        .saturating_mul(mem::size_of::<StemRenderBlock>())
        .saturating_add(stem_render_blocks_audio_bytes(blocks))
        .saturating_add(stem_render_blocks_active_note_bytes(blocks))
}

pub(super) fn stem_effect_inputs_total_bytes(inputs: &Vec<StemEffectInput>) -> usize {
    inputs
        .capacity()
        .saturating_mul(mem::size_of::<StemEffectInput>())
        .saturating_add(inputs.iter().map(StemEffectInput::memory_debug_bytes).sum())
}
