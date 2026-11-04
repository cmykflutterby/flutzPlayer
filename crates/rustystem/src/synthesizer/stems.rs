use super::*;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct StemRenderAllocationDebug {
    pub output_block_count: usize,
    pub output_audio_bytes: usize,
    pub output_active_note_bytes: usize,
    pub output_vec_bytes: usize,
    pub internal_block_count: usize,
    pub internal_audio_bytes: usize,
    pub internal_active_note_bytes: usize,
    pub internal_vec_bytes: usize,
    pub residual_buffer_bytes: usize,
    pub effect_input_bytes: usize,
    pub effect_input_vec_bytes: usize,
    pub output_growth_bytes: usize,
    pub internal_growth_bytes: usize,
    pub residual_growth_bytes: usize,
    pub effect_input_growth_bytes: usize,
}

impl StemRenderAllocationDebug {
    pub fn total_bytes(self) -> usize {
        self.output_audio_bytes
            .saturating_add(self.output_active_note_bytes)
            .saturating_add(self.output_vec_bytes)
            .saturating_add(self.internal_audio_bytes)
            .saturating_add(self.internal_active_note_bytes)
            .saturating_add(self.internal_vec_bytes)
            .saturating_add(self.residual_buffer_bytes)
            .saturating_add(self.effect_input_bytes)
            .saturating_add(self.effect_input_vec_bytes)
    }

    pub fn total_growth_bytes(self) -> usize {
        self.output_growth_bytes
            .saturating_add(self.internal_growth_bytes)
            .saturating_add(self.residual_growth_bytes)
            .saturating_add(self.effect_input_growth_bytes)
    }

    pub fn add_assign(&mut self, other: &Self) {
        self.output_block_count = self
            .output_block_count
            .saturating_add(other.output_block_count);
        self.output_audio_bytes = self
            .output_audio_bytes
            .saturating_add(other.output_audio_bytes);
        self.output_active_note_bytes = self
            .output_active_note_bytes
            .saturating_add(other.output_active_note_bytes);
        self.output_vec_bytes = self.output_vec_bytes.saturating_add(other.output_vec_bytes);
        self.internal_block_count = self
            .internal_block_count
            .saturating_add(other.internal_block_count);
        self.internal_audio_bytes = self
            .internal_audio_bytes
            .saturating_add(other.internal_audio_bytes);
        self.internal_active_note_bytes = self
            .internal_active_note_bytes
            .saturating_add(other.internal_active_note_bytes);
        self.internal_vec_bytes = self
            .internal_vec_bytes
            .saturating_add(other.internal_vec_bytes);
        self.residual_buffer_bytes = self
            .residual_buffer_bytes
            .saturating_add(other.residual_buffer_bytes);
        self.effect_input_bytes = self
            .effect_input_bytes
            .saturating_add(other.effect_input_bytes);
        self.effect_input_vec_bytes = self
            .effect_input_vec_bytes
            .saturating_add(other.effect_input_vec_bytes);
        self.output_growth_bytes = self
            .output_growth_bytes
            .saturating_add(other.output_growth_bytes);
        self.internal_growth_bytes = self
            .internal_growth_bytes
            .saturating_add(other.internal_growth_bytes);
        self.residual_growth_bytes = self
            .residual_growth_bytes
            .saturating_add(other.residual_growth_bytes);
        self.effect_input_growth_bytes = self
            .effect_input_growth_bytes
            .saturating_add(other.effect_input_growth_bytes);
    }

    pub(super) fn add_output_blocks(&mut self, blocks: &Vec<StemRenderBlock>) {
        self.output_block_count = self.output_block_count.saturating_add(blocks.len());
        self.output_audio_bytes = self
            .output_audio_bytes
            .saturating_add(stem_render_blocks_audio_bytes(blocks));
        self.output_active_note_bytes = self
            .output_active_note_bytes
            .saturating_add(stem_render_blocks_active_note_bytes(blocks));
        self.output_vec_bytes = self
            .output_vec_bytes
            .saturating_add(blocks.capacity() * mem::size_of::<StemRenderBlock>());
    }

    pub(super) fn add_internal_blocks(&mut self, blocks: &Vec<StemRenderBlock>) {
        self.internal_block_count = self.internal_block_count.saturating_add(blocks.len());
        self.internal_audio_bytes = self
            .internal_audio_bytes
            .saturating_add(stem_render_blocks_audio_bytes(blocks));
        self.internal_active_note_bytes = self
            .internal_active_note_bytes
            .saturating_add(stem_render_blocks_active_note_bytes(blocks));
        self.internal_vec_bytes = self
            .internal_vec_bytes
            .saturating_add(blocks.capacity() * mem::size_of::<StemRenderBlock>());
    }

    pub(super) fn add_effect_inputs(&mut self, inputs: &Vec<StemEffectInput>) {
        self.effect_input_bytes = self
            .effect_input_bytes
            .saturating_add(inputs.iter().map(StemEffectInput::memory_debug_bytes).sum());
        self.effect_input_vec_bytes = self
            .effect_input_vec_bytes
            .saturating_add(inputs.capacity() * mem::size_of::<StemEffectInput>());
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct StemEffectAllocationCounters {
    pub(super) allocations: u64,
    pub(super) deallocations: u64,
    pub(super) allocated_bytes: u64,
    pub(super) deallocated_bytes: u64,
    pub(super) cache_clears: u64,
    pub(super) cache_released_bytes: u64,
}

pub(super) fn prepare_stem_blocks(blocks: &mut [StemRenderBlock], block_size: usize) {
    for block in blocks {
        block.active_notes.clear();
        block.left.resize(block_size, 0.0);
        block.right.resize(block_size, 0.0);
        block.left.fill(0.0);
        block.right.fill(0.0);
    }
}

pub(super) fn stem_block_has_signal(block: &StemRenderBlock) -> bool {
    !block.active_notes.is_empty()
        || block
            .left
            .iter()
            .chain(block.right.iter())
            .any(|sample| sample.abs() >= SoundFontMath::NON_AUDIBLE)
}

pub(super) fn prepare_stem_effect_inputs(inputs: &mut [StemEffectInput]) {
    for input in inputs {
        input.active_notes.clear();
        input.reverb_input.fill(0.0);
        input.chorus_input_left.fill(0.0);
        input.chorus_input_right.fill(0.0);
    }
}

pub(super) fn upsert_output_stem_block(
    blocks: &mut Vec<StemRenderBlock>,
    active: &mut Vec<bool>,
    source: &StemRenderBlock,
    frames: usize,
) -> usize {
    let index = match blocks
        .iter()
        .position(|existing| existing.identity == source.identity)
    {
        Some(index) => index,
        None => {
            blocks.push(StemRenderBlock {
                identity: source.identity.clone(),
                display_name: source.display_name.clone(),
                active_notes: Vec::new(),
                left: Vec::new(),
                right: Vec::new(),
            });
            active.push(false);
            blocks.len() - 1
        }
    };

    let target = &mut blocks[index];
    target.display_name = source.display_name.clone();
    target.left.resize(frames, 0.0);
    target.right.resize(frames, 0.0);
    for note in &source.active_notes {
        if !target.active_notes.contains(note) {
            target.active_notes.push(*note);
        }
    }
    active[index] = true;
    index
}

#[derive(Debug)]
pub(super) struct StemEffectInput {
    pub(super) identity: StemIdentity,
    pub(super) active_notes: Vec<u8>,
    pub(super) reverb_input: Vec<f32>,
    pub(super) chorus_input_left: Vec<f32>,
    pub(super) chorus_input_right: Vec<f32>,
}

impl StemEffectInput {
    pub(super) fn new(identity: StemIdentity, block_size: usize) -> Self {
        Self {
            identity,
            active_notes: Vec::new(),
            reverb_input: vec![0.0; block_size],
            chorus_input_left: vec![0.0; block_size],
            chorus_input_right: vec![0.0; block_size],
        }
    }

    pub(super) fn memory_debug_bytes(&self) -> usize {
        f32_vec_bytes(&self.reverb_input)
            + f32_vec_bytes(&self.chorus_input_left)
            + f32_vec_bytes(&self.chorus_input_right)
            + self.active_notes.capacity() * mem::size_of::<u8>()
    }
}

#[derive(Debug)]
pub(super) struct StemEffects {
    pub(super) reverb: Reverb,
    pub(super) reverb_input: Vec<f32>,
    pub(super) reverb_output_left: Vec<f32>,
    pub(super) reverb_output_right: Vec<f32>,

    pub(super) chorus: Chorus,
    pub(super) chorus_input_left: Vec<f32>,
    pub(super) chorus_input_right: Vec<f32>,
    pub(super) chorus_output_left: Vec<f32>,
    pub(super) chorus_output_right: Vec<f32>,
}

impl StemEffects {
    pub(super) fn new(sample_rate: i32, block_size: usize) -> Self {
        Self {
            reverb: Reverb::new(sample_rate),
            reverb_input: vec![0.0; block_size],
            reverb_output_left: vec![0.0; block_size],
            reverb_output_right: vec![0.0; block_size],
            chorus: Chorus::new(sample_rate, 0.002, 0.0019, 0.4),
            chorus_input_left: vec![0.0; block_size],
            chorus_input_right: vec![0.0; block_size],
            chorus_output_left: vec![0.0; block_size],
            chorus_output_right: vec![0.0; block_size],
        }
    }

    pub(super) fn clear_inputs(&mut self) {
        self.reverb_input.fill(0.0);
        self.chorus_input_left.fill(0.0);
        self.chorus_input_right.fill(0.0);
    }

    pub(super) fn memory_debug_bytes(&self) -> usize {
        f32_vec_bytes(&self.reverb_input)
            + f32_vec_bytes(&self.reverb_output_left)
            + f32_vec_bytes(&self.reverb_output_right)
            + f32_vec_bytes(&self.chorus_input_left)
            + f32_vec_bytes(&self.chorus_input_right)
            + f32_vec_bytes(&self.chorus_output_left)
            + f32_vec_bytes(&self.chorus_output_right)
    }
}
