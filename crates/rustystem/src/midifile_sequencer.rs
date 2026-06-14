#![allow(dead_code)]

use std::cmp;
use std::sync::Arc;

use crate::midifile::Message;
use crate::midifile::MidiFile;
use crate::stem::{StemRenderBlock, StemRenderRequest, StemRenderSet};
use crate::synthesizer::{StemRenderAllocationDebug, Synthesizer, SynthesizerMemoryDebug};

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum SequencerLoopMode {
    #[default]
    None,
    Infinite,
    Counted,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SequencerLoopSettings {
    pub enabled: bool,
    pub mode: SequencerLoopMode,
    pub start_tick: i32,
    pub end_tick: i32,
    pub loop_count: u32,
}

impl Default for SequencerLoopSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: SequencerLoopMode::None,
            start_tick: 0,
            end_tick: 0,
            loop_count: 1,
        }
    }
}

/// An instance of the MIDI file sequencer.
#[derive(Debug)]
#[non_exhaustive]
pub struct MidiFileSequencer {
    synthesizer: Synthesizer,

    speed: f64,

    midi_file: Option<Arc<MidiFile>>,
    loop_settings: SequencerLoopSettings,
    reposition_preroll_seconds: f64,

    block_wrote: usize,
    stem_scratch: Vec<StemRenderBlock>,

    current_time: f64,
    msg_index: usize,
    loops_completed: u32,
    last_stem_render_allocations: StemRenderAllocationDebug,
}

impl MidiFileSequencer {
    /// Initializes a new instance of the sequencer.
    ///
    /// # Arguments
    ///
    /// * `synthesizer` - The synthesizer to be handled by the sequencer.
    pub fn new(synthesizer: Synthesizer) -> Self {
        Self {
            synthesizer,
            speed: 1.0,
            midi_file: None,
            loop_settings: SequencerLoopSettings::default(),
            reposition_preroll_seconds: 0.0,
            block_wrote: 0,
            stem_scratch: Vec::new(),
            current_time: 0.0,
            msg_index: 0,
            loops_completed: 0,
            last_stem_render_allocations: StemRenderAllocationDebug::default(),
        }
    }

    /// Plays the MIDI file.
    ///
    /// # Arguments
    ///
    /// * `midi_file` - The MIDI file to be played.
    pub fn load_midi(&mut self, midi_file: &Arc<MidiFile>) {
        self.midi_file = Some(Arc::clone(midi_file));

        self.block_wrote = self.synthesizer.block_size;
        self.current_time = 0.0;
        self.msg_index = 0;
        self.loops_completed = 0;

        self.synthesizer.reset()
    }

    pub fn play(&mut self, midi_file: &Arc<MidiFile>) {
        self.load_midi(midi_file);
    }

    /// Stops playing.
    pub fn stop(&mut self) {
        self.midi_file = None;
        self.loops_completed = 0;
        self.synthesizer.reset();
    }

    /// Renders the waveform.
    ///
    /// # Arguments
    ///
    /// * `left` - The buffer of the left channel to store the rendered waveform.
    /// * `right` - The buffer of the right channel to store the rendered waveform.
    ///
    /// # Remarks
    ///
    /// The output buffers for the left and right must be the same length.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        if left.len() != right.len() {
            panic!("The output buffers for the left and right must be the same length.");
        }

        let left_length = left.len();
        let mut wrote: usize = 0;
        while wrote < left_length {
            if self.block_wrote == self.synthesizer.block_size {
                self.process_events();
                self.block_wrote = 0;
                self.current_time += self.speed * self.synthesizer.block_size as f64
                    / self.synthesizer.sample_rate as f64;
            }

            let src_rem = self.synthesizer.block_size - self.block_wrote;
            let dst_rem = left_length - wrote;
            let rem = cmp::min(src_rem, dst_rem);

            self.synthesizer.render(
                &mut left[wrote..wrote + rem],
                &mut right[wrote..wrote + rem],
            );

            self.block_wrote += rem;
            wrote += rem;
        }
    }

    pub fn render_stems(&mut self, request: &StemRenderRequest, frames: usize) -> StemRenderSet {
        let mut blocks = Vec::new();
        self.render_stems_into(request, frames, &mut blocks);
        StemRenderSet::new(frames, blocks).expect("sequenced stem blocks should be valid")
    }

    pub fn render_stems_into(
        &mut self,
        request: &StemRenderRequest,
        frames: usize,
        blocks: &mut Vec<StemRenderBlock>,
    ) {
        let output_bytes_before = stem_render_blocks_total_bytes(blocks);
        let mut active = vec![false; blocks.len()];
        for block in blocks.iter_mut() {
            block.active_notes.clear();
            block.left.resize(frames, 0.0);
            block.right.resize(frames, 0.0);
            block.left.fill(0.0);
            block.right.fill(0.0);
        }
        let mut allocations = StemRenderAllocationDebug::default();
        let mut wrote: usize = 0;
        while wrote < frames {
            if self.block_wrote == self.synthesizer.block_size {
                self.process_events();
                self.block_wrote = 0;
                self.current_time += self.speed * self.synthesizer.block_size as f64
                    / self.synthesizer.sample_rate as f64;
            }

            let src_rem = self.synthesizer.block_size - self.block_wrote;
            let dst_rem = frames - wrote;
            let rem = cmp::min(src_rem, dst_rem);

            self.synthesizer
                .render_stems_into(request, rem, &mut self.stem_scratch);
            allocations.add_assign(&self.synthesizer.last_stem_render_allocations());
            for block in &self.stem_scratch {
                let index = upsert_sequenced_stem_block(blocks, &mut active, block, frames);

                blocks[index].left[wrote..wrote + rem].copy_from_slice(&block.left[..]);
                blocks[index].right[wrote..wrote + rem].copy_from_slice(&block.right[..]);
            }

            self.block_wrote += rem;
            wrote += rem;
        }

        let mut index = 0;
        blocks.retain(|_| {
            let keep = active.get(index).copied().unwrap_or(false);
            index += 1;
            keep
        });
        allocations.output_block_count =
            allocations.output_block_count.saturating_add(blocks.len());
        allocations.output_audio_bytes = allocations
            .output_audio_bytes
            .saturating_add(stem_render_blocks_audio_bytes(&blocks));
        allocations.output_active_note_bytes = allocations
            .output_active_note_bytes
            .saturating_add(stem_render_blocks_active_note_bytes(&blocks));
        allocations.output_vec_bytes = allocations
            .output_vec_bytes
            .saturating_add(blocks.capacity() * std::mem::size_of::<StemRenderBlock>());
        allocations.output_growth_bytes = allocations.output_growth_bytes.saturating_add(
            stem_render_blocks_total_bytes(blocks).saturating_sub(output_bytes_before),
        );
        self.last_stem_render_allocations = allocations;
    }

    fn process_events(&mut self) {
        let midi_file = match self.midi_file.as_ref() {
            Some(value) => Arc::clone(value),
            None => return,
        };

        while self.msg_index < midi_file.messages.len() {
            let time = midi_file.times[self.msg_index];
            let msg = midi_file.messages[self.msg_index].clone();

            if time <= self.current_time {
                self.process_message(msg, &midi_file);
                self.msg_index += 1;
            } else {
                break;
            }
        }

        self.process_loop_boundary(&midi_file);
    }

    fn process_message(&mut self, message: Message, _midi_file: &MidiFile) {
        match message {
            Message::Normal {
                status,
                data1,
                data2,
            } => {
                let channel = status & 0x0F;
                let command = status & 0xF0;
                self.synthesizer.process_midi_message(
                    channel as i32,
                    command as i32,
                    data1 as i32,
                    data2 as i32,
                );
            }
            Message::SysEx { bytes } => self.synthesizer.process_sysex_message(&bytes),
            _ => (),
        }
    }

    fn active_loop_settings(&self, midi_file: &MidiFile) -> Option<SequencerLoopSettings> {
        let mut settings = self.loop_settings;
        let tick_length = midi_file.get_tick_length().max(0);
        settings.start_tick = settings.start_tick.clamp(0, tick_length);
        settings.end_tick = settings.end_tick.clamp(0, tick_length);
        settings.loop_count = settings.loop_count.max(1);

        if !settings.enabled
            || matches!(settings.mode, SequencerLoopMode::None)
            || settings.end_tick <= settings.start_tick
        {
            return None;
        }

        Some(settings)
    }

    fn process_loop_boundary(&mut self, midi_file: &MidiFile) {
        let Some(settings) = self.active_loop_settings(midi_file) else {
            return;
        };

        let current_tick = midi_file.get_tick_at_time(self.current_time);
        if current_tick < settings.end_tick {
            return;
        }

        match settings.mode {
            SequencerLoopMode::Infinite => self.jump_to_tick(midi_file, settings.start_tick),
            SequencerLoopMode::Counted => {
                if self.loops_completed < settings.loop_count {
                    self.loops_completed = self.loops_completed.saturating_add(1);
                    self.jump_to_tick(midi_file, settings.start_tick);
                }
            }
            SequencerLoopMode::None => {}
        }
    }

    fn jump_to_tick(&mut self, midi_file: &MidiFile, target_tick: i32) {
        self.rebuild_to_tick_with_preroll(midi_file, target_tick, false);
    }

    fn rebuild_to_tick_with_preroll(
        &mut self,
        midi_file: &MidiFile,
        target_tick: i32,
        reset_loop_counter: bool,
    ) {
        let clamped_target = target_tick.clamp(0, midi_file.get_tick_length());
        let target_time = midi_file.get_time_at_tick(clamped_target);
        let preroll_seconds = self.reposition_preroll_seconds.max(0.0);
        if preroll_seconds <= 0.0 || target_time <= 0.0 {
            self.rebuild_to_tick(midi_file, clamped_target, reset_loop_counter);
            return;
        }

        let preroll_time = (target_time - preroll_seconds).max(0.0);
        let preroll_tick = midi_file.get_tick_at_time(preroll_time);
        if preroll_tick >= clamped_target {
            self.rebuild_to_tick(midi_file, clamped_target, reset_loop_counter);
            return;
        }

        self.rebuild_to_tick(midi_file, preroll_tick, reset_loop_counter);
        self.current_time = preroll_time;
        self.render_silently_until_time(target_time);
        self.current_time = target_time;
    }

    fn rebuild_to_tick(
        &mut self,
        midi_file: &MidiFile,
        target_tick: i32,
        reset_loop_counter: bool,
    ) {
        let clamped_target = target_tick.clamp(0, midi_file.get_tick_length());
        self.synthesizer.reset();
        self.current_time = 0.0;
        self.msg_index = 0;
        self.block_wrote = self.synthesizer.block_size;
        if reset_loop_counter {
            self.loops_completed = 0;
        }

        while self.msg_index < midi_file.messages.len() {
            let tick = midi_file.ticks[self.msg_index];
            if tick > clamped_target {
                break;
            }
            let msg = midi_file.messages[self.msg_index].clone();
            self.process_message(msg, midi_file);
            self.msg_index += 1;
        }

        self.current_time = midi_file.get_time_at_tick(clamped_target);
        self.block_wrote = self.synthesizer.block_size;
    }

    fn render_silently_until_time(&mut self, target_time: f64) {
        if self.speed <= 0.0 || target_time <= self.current_time {
            return;
        }

        let sample_rate = self.synthesizer.sample_rate.max(1) as f64;
        let frames = ((target_time - self.current_time) * sample_rate / self.speed).ceil() as usize;
        let mut remaining = frames;
        let mut left = Vec::new();
        let mut right = Vec::new();
        while remaining > 0 {
            let chunk = remaining.min(self.synthesizer.block_size.max(1));
            left.resize(chunk, 0.0);
            right.resize(chunk, 0.0);
            left.fill(0.0);
            right.fill(0.0);
            self.render(&mut left, &mut right);
            remaining -= chunk;
        }
    }

    /// Seeks the sequencer to a target position in seconds.
    pub fn seek_to_seconds(&mut self, target_seconds: f64) {
        let midi_file = match self.midi_file.as_ref() {
            Some(value) => Arc::clone(value),
            None => return,
        };

        let clamped_target = target_seconds.clamp(0.0, midi_file.get_length());
        let target_tick = midi_file.get_tick_at_time(clamped_target);
        self.rebuild_to_tick_with_preroll(&midi_file, target_tick, true);
        self.current_time = clamped_target;

        if let Some(settings) = self.active_loop_settings(&midi_file) {
            if matches!(settings.mode, SequencerLoopMode::Counted)
                && (target_tick < settings.start_tick || target_tick >= settings.end_tick)
            {
                self.loop_settings.enabled = false;
            }
        }
    }

    /// Seeks the sequencer to a target MIDI tick.
    pub fn seek_to_tick(&mut self, target_tick: i32) {
        let midi_file = match self.midi_file.as_ref() {
            Some(value) => Arc::clone(value),
            None => return,
        };

        let clamped_target = target_tick.clamp(0, midi_file.get_tick_length());
        self.rebuild_to_tick_with_preroll(&midi_file, clamped_target, true);

        if let Some(settings) = self.active_loop_settings(&midi_file) {
            if matches!(settings.mode, SequencerLoopMode::Counted)
                && (clamped_target < settings.start_tick || clamped_target >= settings.end_tick)
            {
                self.loop_settings.enabled = false;
            }
        }
    }

    /// Gets the synthesizer handled by the sequencer.
    pub fn get_synthesizer(&self) -> &Synthesizer {
        &self.synthesizer
    }

    pub fn memory_debug(&self) -> SynthesizerMemoryDebug {
        let mut debug = self.synthesizer.memory_debug();
        debug.last_stem_render_allocations = self.last_stem_render_allocations;
        debug
    }

    pub fn last_stem_render_allocations(&self) -> StemRenderAllocationDebug {
        self.last_stem_render_allocations
    }

    /// Gets the currently playing MIDI file.
    pub fn get_midi_file(&self) -> Option<&MidiFile> {
        match &self.midi_file {
            None => None,
            Some(value) => Some(value),
        }
    }

    /// Gets the current playback position in seconds.
    pub fn get_position(&self) -> f64 {
        self.current_time
    }

    /// Gets the current playback position in MIDI ticks.
    pub fn get_tick_position(&self) -> i32 {
        match &self.midi_file {
            None => 0,
            Some(value) => value.get_tick_at_time(self.current_time),
        }
    }

    /// Gets a value that indicates whether the current playback position is at the end of the sequence.
    ///
    /// # Remarks
    ///
    /// If the `play` method has not yet been called, this value will be `true`.
    /// This value will never be `true` if loop playback is enabled.
    pub fn end_of_sequence(&self) -> bool {
        match &self.midi_file {
            None => true,
            Some(value) => {
                if let Some(settings) = self.active_loop_settings(value) {
                    if matches!(settings.mode, SequencerLoopMode::Infinite)
                        || (matches!(settings.mode, SequencerLoopMode::Counted)
                            && self.loops_completed < settings.loop_count)
                    {
                        return false;
                    }
                }

                self.msg_index == value.messages.len()
            }
        }
    }

    /// Gets the current playback speed.
    ///
    /// # Remarks
    ///
    /// The default value is 1.
    /// The tempo will be multiplied by this value during playback.
    pub fn get_speed(&self) -> f64 {
        self.speed
    }

    /// Sets the playback speed.
    ///
    /// # Remarks
    ///
    /// The value must be non-negative.
    pub fn set_speed(&mut self, value: f64) {
        if value < 0.0 {
            panic!("The playback speed must be a non-negative value.");
        }

        self.speed = value;
    }

    pub fn set_reposition_preroll_seconds(&mut self, seconds: f64) {
        self.reposition_preroll_seconds = seconds.max(0.0);
    }

    /// Enables or disables loop playback for the currently loaded MIDI file.
    pub fn set_play_loop(&mut self, enabled: bool) {
        self.loop_settings.enabled = enabled;
        if enabled && matches!(self.loop_settings.mode, SequencerLoopMode::None) {
            self.loop_settings.mode = SequencerLoopMode::Infinite;
        }
        if !enabled {
            self.loops_completed = 0;
        }
    }

    pub fn set_loop_settings(&mut self, settings: SequencerLoopSettings) -> bool {
        let should_stop = settings.enabled
            && matches!(settings.mode, SequencerLoopMode::Counted)
            && self.loops_completed >= settings.loop_count.max(1);
        self.loop_settings = settings;
        if !self.loop_settings.enabled || matches!(self.loop_settings.mode, SequencerLoopMode::None)
        {
            self.loops_completed = 0;
        }
        if should_stop {
            self.loop_settings.enabled = false;
        }
        should_stop
    }
}

fn stem_render_blocks_audio_bytes(blocks: &[StemRenderBlock]) -> usize {
    blocks
        .iter()
        .map(|block| {
            block
                .left
                .capacity()
                .saturating_add(block.right.capacity())
                .saturating_mul(std::mem::size_of::<f32>())
        })
        .sum()
}

fn stem_render_blocks_active_note_bytes(blocks: &[StemRenderBlock]) -> usize {
    blocks
        .iter()
        .map(|block| block.active_notes.capacity() * std::mem::size_of::<u8>())
        .sum()
}

fn stem_render_blocks_total_bytes(blocks: &Vec<StemRenderBlock>) -> usize {
    blocks
        .capacity()
        .saturating_mul(std::mem::size_of::<StemRenderBlock>())
        .saturating_add(stem_render_blocks_audio_bytes(blocks))
        .saturating_add(stem_render_blocks_active_note_bytes(blocks))
}

fn upsert_sequenced_stem_block(
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
