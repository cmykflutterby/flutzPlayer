use super::soundfont_cache::parse_soundfont_bytes;
use super::*;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug)]
pub struct MultiSoundFontPlayback {
    state: PlaybackState,
    config: PlaybackConfig,
    midi: Option<LoadedMidi>,
    instances: Vec<SynthInstance>,
    loop_settings: PlaybackLoopSettings,
    last_stem_render_allocations: StemRenderAllocationDebug,
}

impl MultiSoundFontPlayback {
    fn ensure_instances_loaded(&mut self) -> Result<()> {
        let midi_file = Arc::clone(
            self.midi
                .as_ref()
                .ok_or_else(|| FlutzError::InvalidInput("no MIDI file is loaded".to_owned()))?
                .midi_file(),
        );
        for instance in &mut self.instances {
            if instance.sequencer.get_midi_file().is_none() {
                instance.sequencer.load_midi(&midi_file);
            }
        }
        Ok(())
    }

    pub fn new(soundfonts: Vec<SoundFontBytes>, config: PlaybackConfig) -> Result<Self> {
        Self::new_with_loader(soundfonts, config, parse_soundfont_bytes)
    }

    pub fn new_with_cache(
        soundfonts: Vec<SoundFontBytes>,
        config: PlaybackConfig,
        cache: &mut SoundFontRuntimeCache,
    ) -> Result<Self> {
        let loaded_soundfonts = cache.load_soundfonts_parallel(soundfonts)?;
        Self::new_with_loaded_soundfonts(loaded_soundfonts, config)
    }

    pub fn new_with_loaded_soundfonts(
        soundfonts: Vec<LoadedSoundFont>,
        config: PlaybackConfig,
    ) -> Result<Self> {
        if soundfonts.is_empty() {
            return Err(FlutzError::InvalidInput(
                "at least one soundfont is required for playback".to_owned(),
            ));
        }

        let mut settings = SynthesizerSettings::new(config.sample_rate as i32);
        settings.block_size = config.block_frames;
        settings.maximum_polyphony = config.maximum_polyphony;
        settings.enable_reverb_and_chorus = config.enable_reverb_and_chorus;

        let mut instances = Vec::with_capacity(soundfonts.len());
        for soundfont in soundfonts {
            let synthesizer =
                Synthesizer::new(&soundfont.sound_font, &settings).map_err(|error| {
                    FlutzError::Runtime(format!(
                        "failed to create RustySynth instance for {}: {error:?}",
                        soundfont.internal_id
                    ))
                })?;
            let coverage = extract_coverage_from_sf2(&soundfont.sound_font);
            let mut sequencer = MidiFileSequencer::new(synthesizer);
            sequencer.set_reposition_preroll_seconds(config.reposition_preroll_ms as f64 / 1000.0);
            instances.push(SynthInstance {
                internal_id: soundfont.internal_id,
                display_name: soundfont.display_name,
                coverage,
                sequencer,
                left: Vec::new(),
                right: Vec::new(),
                stem_blocks: Vec::new(),
            });
        }

        Ok(Self {
            state: PlaybackState::Stopped,
            config,
            midi: None,
            instances,
            loop_settings: PlaybackLoopSettings::default(),
            last_stem_render_allocations: StemRenderAllocationDebug::default(),
        })
    }

    fn new_with_loader(
        soundfonts: Vec<SoundFontBytes>,
        config: PlaybackConfig,
        mut load_soundfont: impl FnMut(SoundFontBytes) -> Result<Arc<SoundFont>>,
    ) -> Result<Self> {
        if soundfonts.is_empty() {
            return Err(FlutzError::InvalidInput(
                "at least one soundfont is required for playback".to_owned(),
            ));
        }

        let mut settings = SynthesizerSettings::new(config.sample_rate as i32);
        settings.block_size = config.block_frames;
        settings.maximum_polyphony = config.maximum_polyphony;
        settings.enable_reverb_and_chorus = config.enable_reverb_and_chorus;

        let mut instances = Vec::with_capacity(soundfonts.len());
        for soundfont in soundfonts {
            let internal_id = soundfont.internal_id.clone();
            let display_name = soundfont.display_name.clone();
            let sound_font = load_soundfont(soundfont)?;
            let synthesizer = Synthesizer::new(&sound_font, &settings).map_err(|error| {
                FlutzError::Runtime(format!(
                    "failed to create RustySynth instance for {internal_id}: {error:?}"
                ))
            })?;
            let coverage = extract_coverage_from_sf2(&sound_font);
            let mut sequencer = MidiFileSequencer::new(synthesizer);
            sequencer.set_reposition_preroll_seconds(config.reposition_preroll_ms as f64 / 1000.0);
            instances.push(SynthInstance {
                internal_id,
                display_name,
                coverage,
                sequencer,
                left: Vec::new(),
                right: Vec::new(),
                stem_blocks: Vec::new(),
            });
        }

        Ok(Self {
            state: PlaybackState::Stopped,
            config,
            midi: None,
            instances,
            loop_settings: PlaybackLoopSettings::default(),
            last_stem_render_allocations: StemRenderAllocationDebug::default(),
        })
    }

    pub fn load_midi_bytes(&mut self, bytes: &[u8]) -> Result<LoadedMidi> {
        self.load_midi_bytes_with_loop_type(bytes, MidiFileLoopType::LoopPoint(0))
    }

    pub fn load_midi_bytes_with_loop_type(
        &mut self,
        bytes: &[u8],
        loop_type: MidiFileLoopType,
    ) -> Result<LoadedMidi> {
        let midi = validate_midi_bytes_with_loop_type(bytes, loop_type)?;
        self.midi = Some(midi.clone());
        self.stop();
        self.set_loop_settings(self.loop_settings);
        Ok(midi)
    }

    pub fn play(&mut self) -> Result<()> {
        self.ensure_instances_loaded()?;
        let loop_settings = self.loop_settings.as_sequencer_settings();
        for instance in &mut self.instances {
            instance.sequencer.set_loop_settings(loop_settings);
        }
        self.state = PlaybackState::Playing;
        Ok(())
    }

    pub fn pause(&mut self) {
        if self.state == PlaybackState::Playing {
            self.state = PlaybackState::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == PlaybackState::Paused {
            self.state = PlaybackState::Playing;
        }
    }

    pub fn stop(&mut self) {
        for instance in &mut self.instances {
            instance.sequencer.stop();
        }
        self.last_stem_render_allocations = StemRenderAllocationDebug::default();
        self.state = PlaybackState::Stopped;
    }

    pub fn render_interleaved_stereo(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let frames = output.len() / 2;
        if frames == 0 {
            return;
        }

        let blocks = self.render_soundfont_blocks(frames);
        let gain = 1.0 / blocks.len().max(1) as f32;
        for block in blocks {
            for frame in 0..frames {
                output[frame * 2] += block.left[frame] * gain;
                output[frame * 2 + 1] += block.right[frame] * gain;
            }
        }
    }

    pub fn render_soundfont_blocks(&mut self, frames: usize) -> Vec<StemRenderBlock> {
        if self.state != PlaybackState::Playing || frames == 0 {
            self.last_stem_render_allocations = StemRenderAllocationDebug::default();
            return Vec::new();
        }

        let mut ended = true;
        let mut blocks = Vec::with_capacity(self.instances.len());
        for instance in &mut self.instances {
            instance.render(frames);
            ended = ended && instance.sequencer.end_of_sequence();
            blocks.push(StemRenderBlock::whole_soundfont_named(
                instance.internal_id.clone(),
                instance.display_name.clone(),
                instance.left.clone(),
                instance.right.clone(),
            ));
        }

        if ended {
            self.stop();
        }
        blocks
    }

    pub fn render_channel_program_stem_blocks(&mut self, frames: usize) -> Vec<StemRenderBlock> {
        let mut blocks = Vec::new();
        self.render_channel_program_stem_blocks_into(frames, &mut blocks);
        blocks
    }

    pub fn render_channel_program_stem_blocks_into(
        &mut self,
        frames: usize,
        blocks: &mut Vec<StemRenderBlock>,
    ) {
        let mut active = vec![false; blocks.len()];
        for block in blocks.iter_mut() {
            block.active_notes.clear();
            block.left.resize(frames, 0.0);
            block.right.resize(frames, 0.0);
            block.left.fill(0.0);
            block.right.fill(0.0);
        }
        if self.state != PlaybackState::Playing || frames == 0 {
            blocks.clear();
            self.last_stem_render_allocations = StemRenderAllocationDebug::default();
            return;
        }

        let mut ended = true;
        let mut stem_allocations = StemRenderAllocationDebug::default();
        for instance in &mut self.instances {
            for block in instance.render_channel_program_stems(frames) {
                upsert_stem_block(blocks, &mut active, block, frames);
            }
            stem_allocations.add_assign(&instance.sequencer.last_stem_render_allocations());
            ended = ended && instance.sequencer.end_of_sequence();
        }
        self.last_stem_render_allocations = stem_allocations;

        let mut index = 0;
        blocks.retain(|_| {
            let keep = active.get(index).copied().unwrap_or(false);
            index += 1;
            keep
        });

        if ended {
            self.stop();
        }
    }

    pub fn state(&self) -> PlaybackState {
        self.state
    }

    pub fn position_seconds(&self) -> f64 {
        self.instances
            .iter()
            .map(|instance| instance.sequencer.get_position())
            .fold(0.0, f64::max)
    }

    pub fn position_ticks(&self) -> u64 {
        self.instances
            .iter()
            .map(|instance| instance.sequencer.get_tick_position().max(0) as u64)
            .max()
            .unwrap_or(0)
    }

    pub fn duration_seconds(&self) -> f64 {
        self.midi
            .as_ref()
            .map(LoadedMidi::duration_seconds)
            .unwrap_or(0.0)
    }

    pub fn seek_to_fraction(&mut self, fraction: f64) -> Result<()> {
        self.ensure_instances_loaded()?;
        let duration = self.duration_seconds();
        if duration <= 0.0 {
            return Ok(());
        }

        let clamped_fraction = fraction.clamp(0.0, 1.0);
        let target_seconds = duration * clamped_fraction;
        for instance in &mut self.instances {
            instance.sequencer.seek_to_seconds(target_seconds);
        }
        Ok(())
    }

    pub fn seek_to_tick(&mut self, tick: u64) -> Result<()> {
        self.ensure_instances_loaded()?;
        let clamped_tick = tick.min(i32::MAX as u64) as i32;
        for instance in &mut self.instances {
            instance.sequencer.seek_to_tick(clamped_tick);
        }
        Ok(())
    }

    pub fn seek_to_seconds(&mut self, seconds: f64) -> Result<()> {
        self.ensure_instances_loaded()?;
        for instance in &mut self.instances {
            instance.sequencer.seek_to_seconds(seconds);
        }
        Ok(())
    }

    pub fn set_loop_enabled(&mut self, enabled: bool) {
        self.loop_settings.enabled = enabled;
        if enabled && matches!(self.loop_settings.mode, PlaybackLoopMode::None) {
            self.loop_settings.mode = PlaybackLoopMode::Infinite;
        }
        self.set_loop_settings(self.loop_settings);
    }

    pub fn set_loop_settings(&mut self, settings: PlaybackLoopSettings) {
        self.loop_settings = settings;
        let runtime_settings = settings.as_sequencer_settings();
        let mut should_stop = false;
        for instance in &mut self.instances {
            should_stop |= instance.sequencer.set_loop_settings(runtime_settings);
        }
        if should_stop {
            self.stop();
        }
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    pub fn soundfont_coverages(&self) -> Vec<(String, SoundFontCoverage)> {
        self.instances
            .iter()
            .map(|instance| (instance.internal_id.clone(), instance.coverage.clone()))
            .collect()
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn memory_debug(&self) -> PlaybackMemoryDebug {
        let instances = self
            .instances
            .iter()
            .map(SynthInstance::memory_debug)
            .collect::<Vec<_>>();
        PlaybackMemoryDebug::from_instances(
            instances,
            self.midi.as_ref().map(LoadedMidi::memory_debug),
        )
    }

    pub fn last_stem_render_allocations(&self) -> StemRenderAllocationDebug {
        self.last_stem_render_allocations
    }
}

#[derive(Debug, Clone, Default)]
pub struct PlaybackMemoryDebug {
    pub instance_count: usize,
    pub soundfont_wave_bytes: usize,
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
    pub midi_file: Option<MidiFileMemoryDebug>,
    pub instances: Vec<SynthInstanceMemoryDebug>,
}

impl PlaybackMemoryDebug {
    fn from_instances(
        instances: Vec<SynthInstanceMemoryDebug>,
        midi_file: Option<MidiFileMemoryDebug>,
    ) -> Self {
        Self {
            instance_count: instances.len(),
            soundfont_wave_bytes: instances
                .iter()
                .map(|instance| instance.synth.soundfont_wave_bytes)
                .sum(),
            soundfont_metadata_items: instances
                .iter()
                .map(|instance| instance.synth.soundfont_metadata_items)
                .sum(),
            total_voices: instances
                .iter()
                .map(|instance| instance.synth.total_voices)
                .sum(),
            active_voices: instances
                .iter()
                .map(|instance| instance.synth.active_voices)
                .sum(),
            max_active_voices: instances
                .iter()
                .map(|instance| instance.synth.max_active_voices)
                .sum(),
            total_voice_requests: instances
                .iter()
                .map(|instance| instance.synth.total_voice_requests)
                .sum(),
            exclusive_class_reuses: instances
                .iter()
                .map(|instance| instance.synth.exclusive_class_reuses)
                .sum(),
            free_voice_allocations: instances
                .iter()
                .map(|instance| instance.synth.free_voice_allocations)
                .sum(),
            contention_steals: instances
                .iter()
                .map(|instance| instance.synth.contention_steals)
                .sum(),
            env_delay_voices: instances
                .iter()
                .map(|instance| instance.synth.env_delay_voices)
                .sum(),
            env_attack_voices: instances
                .iter()
                .map(|instance| instance.synth.env_attack_voices)
                .sum(),
            env_hold_voices: instances
                .iter()
                .map(|instance| instance.synth.env_hold_voices)
                .sum(),
            env_decay_voices: instances
                .iter()
                .map(|instance| instance.synth.env_decay_voices)
                .sum(),
            env_release_voices: instances
                .iter()
                .map(|instance| instance.synth.env_release_voices)
                .sum(),
            env_value_sum: instances
                .iter()
                .map(|instance| instance.synth.env_value_sum)
                .sum(),
            env_value_avg: {
                let active_voices: usize = instances
                    .iter()
                    .map(|instance| instance.synth.active_voices)
                    .sum();
                let value_sum: f32 = instances
                    .iter()
                    .map(|instance| instance.synth.env_value_sum)
                    .sum();
                if active_voices > 0 {
                    value_sum / active_voices as f32
                } else {
                    0.0
                }
            },
            voice_buffer_bytes: instances
                .iter()
                .map(|instance| instance.synth.voice_buffer_bytes)
                .sum(),
            block_buffer_bytes: instances
                .iter()
                .map(|instance| instance.synth.block_buffer_bytes)
                .sum(),
            retained_stem_blocks: instances
                .iter()
                .map(|instance| instance.synth.retained_stem_blocks)
                .sum(),
            retained_stem_block_bytes: instances
                .iter()
                .map(|instance| instance.synth.retained_stem_block_bytes)
                .sum(),
            stem_effects: instances
                .iter()
                .map(|instance| instance.synth.stem_effects)
                .sum(),
            stem_effect_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_bytes)
                .sum(),
            stem_effect_allocations: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_allocations)
                .sum(),
            stem_effect_deallocations: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_deallocations)
                .sum(),
            stem_effect_allocated_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_allocated_bytes)
                .sum(),
            stem_effect_deallocated_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_deallocated_bytes)
                .sum(),
            stem_effect_cache_clears: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_cache_clears)
                .sum(),
            stem_effect_cache_released_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_cache_released_bytes)
                .sum(),
            last_stem_render_allocations: instances.iter().fold(
                StemRenderAllocationDebug::default(),
                |mut allocations, instance| {
                    allocations.add_assign(&instance.synth.last_stem_render_allocations);
                    allocations
                },
            ),
            effects_bytes: instances
                .iter()
                .map(|instance| instance.synth.effects_bytes)
                .sum(),
            preset_lookup_bytes: instances
                .iter()
                .map(|instance| instance.synth.preset_lookup_bytes)
                .sum(),
            channel_bytes: instances
                .iter()
                .map(|instance| instance.synth.channel_bytes)
                .sum(),
            stem_effect_map_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_map_bytes)
                .sum(),
            block_stem_vec_bytes: instances
                .iter()
                .map(|instance| instance.synth.block_stem_vec_bytes)
                .sum(),
            estimated_bytes: instances
                .iter()
                .map(|instance| instance.synth.estimated_bytes)
                .sum::<usize>()
                .saturating_add(
                    midi_file
                        .map(|debug| debug.estimated_bytes)
                        .unwrap_or_default(),
                ),
            midi_file,
            instances,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SynthInstanceMemoryDebug {
    pub internal_id: String,
    pub display_name: String,
    pub synth: SynthesizerMemoryDebug,
}

#[derive(Debug)]
struct SynthInstance {
    internal_id: String,
    display_name: String,
    coverage: SoundFontCoverage,
    sequencer: MidiFileSequencer,
    left: Vec<f32>,
    right: Vec<f32>,
    stem_blocks: Vec<StemRenderBlock>,
}

impl SynthInstance {
    fn render(&mut self, frames: usize) {
        self.left.resize(frames, 0.0);
        self.right.resize(frames, 0.0);
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.sequencer.render(&mut self.left, &mut self.right);
    }

    fn render_channel_program_stems(&mut self, frames: usize) -> &[StemRenderBlock] {
        let request = StemRenderRequest::channel_program(self.internal_id.clone());
        self.sequencer
            .render_stems_into(&request, frames, &mut self.stem_blocks);
        for block in &mut self.stem_blocks {
            if block.display_name.is_none() && block.identity.mode().requires_voice_grouping() {
                let channel = block.identity.midi_channel.unwrap_or(0);
                let program = block.identity.midi_program.unwrap_or(0);
                block.display_name = Some(format!("{} Ch {channel} P{program}", self.display_name));
            } else if block.display_name.as_deref() == Some("Global effects") {
                block.display_name = Some(format!("{} Global effects", self.display_name));
            }
        }
        &self.stem_blocks
    }

    fn memory_debug(&self) -> SynthInstanceMemoryDebug {
        SynthInstanceMemoryDebug {
            internal_id: self.internal_id.clone(),
            display_name: self.display_name.clone(),
            synth: self.sequencer.memory_debug(),
        }
    }
}

fn upsert_stem_block(
    blocks: &mut Vec<StemRenderBlock>,
    active: &mut Vec<bool>,
    source: &StemRenderBlock,
    frames: usize,
) {
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
    target.display_name.clone_from(&source.display_name);
    target.active_notes.clear();
    target
        .active_notes
        .extend(source.active_notes.iter().copied());
    target.left.resize(frames, 0.0);
    target.right.resize(frames, 0.0);
    target.left.copy_from_slice(&source.left);
    target.right.copy_from_slice(&source.right);
    active[index] = true;
}
