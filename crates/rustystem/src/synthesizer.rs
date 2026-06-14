#![allow(dead_code)]

use std::cmp;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::mem;
use std::sync::Arc;

use crate::array_math::ArrayMath;
use crate::channel::Channel;
use crate::chorus::Chorus;
use crate::error::SynthesizerError;
use crate::midi_interpretation::{decode_sysex_event, MidiChannelRole};
use crate::region_pair::RegionPair;
use crate::reverb::Reverb;
use crate::soundfont::SoundFont;
use crate::soundfont_math::SoundFontMath;
use crate::stem::{
    StemIdentity, StemRenderBlock, StemRenderMode, StemRenderRequest, StemRenderSet,
};
use crate::synthesizer_settings::SynthesizerSettings;
use crate::voice::Voice;
use crate::voice_collection::VoiceCollection;

/// An instance of the SoundFont synthesizer.
#[derive(Debug)]
#[non_exhaustive]
pub struct Synthesizer {
    pub(crate) sound_font: Arc<SoundFont>,
    pub(crate) sample_rate: i32,
    pub(crate) block_size: usize,
    pub(crate) maximum_polyphony: usize,

    preset_lookup: HashMap<i32, usize>,
    default_preset: usize,

    channels: Vec<Channel>,

    voices: VoiceCollection,

    block_left: Vec<f32>,
    block_right: Vec<f32>,
    block_stems: Vec<StemRenderBlock>,
    dry_stem_scratch: Vec<StemRenderBlock>,
    stem_effect_input_scratch: Vec<StemEffectInput>,
    residual_left: Vec<f32>,
    residual_right: Vec<f32>,
    stem_effects: HashMap<StemIdentity, StemEffects>,
    stem_effect_allocation_counters: StemEffectAllocationCounters,
    last_stem_render_allocations: StemRenderAllocationDebug,

    inverse_block_size: f32,

    block_read: usize,

    master_volume: f32,

    effects: Option<Effects>,
}

impl Synthesizer {
    /// The number of channels.
    pub const CHANNEL_COUNT: usize = 16;
    /// The default GM percussion channel.
    pub const PERCUSSION_CHANNEL: usize = 9;

    /// Initializes a new synthesizer using a specified SoundFont and settings.
    ///
    /// # Arguments
    ///
    /// * `sound_font` - The SoundFont instance.
    /// * `settings` - The settings for synthesis.
    pub fn new(
        sound_font: &Arc<SoundFont>,
        settings: &SynthesizerSettings,
    ) -> Result<Self, SynthesizerError> {
        settings.validate()?;

        let mut preset_lookup: HashMap<i32, usize> = HashMap::new();

        let mut min_preset_id = i32::MAX;
        let mut default_preset: usize = 0;
        for i in 0..sound_font.presets.len() {
            let preset = &sound_font.presets[i];

            // The preset ID is Int32, where the upper 16 bits represent the bank number
            // and the lower 16 bits represent the patch number.
            // This ID is used to search for presets by the combination of bank number
            // and patch number.
            let preset_id = (preset.bank_number << 16) | preset.patch_number;
            preset_lookup.insert(preset_id, i);

            // The preset with the minimum ID number will be default.
            // If the SoundFont is GM compatible, the piano will be chosen.
            if preset_id < min_preset_id {
                default_preset = i;
                min_preset_id = preset_id;
            }
        }

        let mut channels: Vec<Channel> = Vec::new();
        for i in 0..Synthesizer::CHANNEL_COUNT {
            channels.push(Channel::new(i == Synthesizer::PERCUSSION_CHANNEL));
        }

        let voices = VoiceCollection::new(settings);

        let block_left: Vec<f32> = vec![0_f32; settings.block_size];
        let block_right: Vec<f32> = vec![0_f32; settings.block_size];

        let inverse_block_size = 1_f32 / settings.block_size as f32;

        let block_read = settings.block_size;

        let master_volume = 0.5_f32;

        let effects = if settings.enable_reverb_and_chorus {
            Some(Effects::new(settings))
        } else {
            None
        };

        Ok(Self {
            sound_font: Arc::clone(sound_font),
            sample_rate: settings.sample_rate,
            block_size: settings.block_size,
            maximum_polyphony: settings.maximum_polyphony,
            preset_lookup,
            default_preset,
            channels,
            voices,
            block_left,
            block_right,
            block_stems: Vec::new(),
            dry_stem_scratch: Vec::new(),
            stem_effect_input_scratch: Vec::new(),
            residual_left: vec![0.0; settings.block_size],
            residual_right: vec![0.0; settings.block_size],
            stem_effects: HashMap::new(),
            stem_effect_allocation_counters: StemEffectAllocationCounters::default(),
            last_stem_render_allocations: StemRenderAllocationDebug::default(),
            inverse_block_size,
            block_read,
            master_volume,
            effects,
        })
    }

    /// Processes a MIDI message.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel to which the message will be sent.
    /// * `command` - The type of the message.
    /// * `data1` - The first data part of the message.
    /// * `data2` - The second data part of the message.
    pub fn process_midi_message(&mut self, channel: i32, command: i32, data1: i32, data2: i32) {
        if !(0 <= channel && channel < self.channels.len() as i32) {
            return;
        }

        let channel_info = &mut self.channels[channel as usize];

        match command {
            0x80 => self.note_off(channel, data1),       // Note Off
            0x90 => self.note_on(channel, data1, data2), // Note On
            0xB0 => match data1 // Controller
            {
                0x00 => channel_info.set_bank(data2), // Bank Selection
                0x01 => channel_info.set_modulation_coarse(data2), // Modulation Coarse
                0x21 => channel_info.set_modulation_fine(data2), // Modulation Fine
                0x06 => channel_info.data_entry_coarse(data2), // Data Entry Coarse
                0x26 => channel_info.data_entry_fine(data2), // Data Entry Fine
                0x07 => channel_info.set_volume_coarse(data2), // Channel Volume Coarse
                0x27 => channel_info.set_volume_fine(data2), // Channel Volume Fine
                0x0A => channel_info.set_pan_coarse(data2), // Pan Coarse
                0x2A => channel_info.set_pan_fine(data2), // Pan Fine
                0x0B => channel_info.set_expression_coarse(data2), // Expression Coarse
                0x2B => channel_info.set_expression_fine(data2), // Expression Fine
                0x40 => channel_info.set_hold_pedal(data2), // Hold Pedal
                0x5B => channel_info.set_reverb_send(data2), // Reverb Send
                0x5D => channel_info.set_chorus_send(data2), // Chorus Send
                0x63 => channel_info.set_nrpn_coarse(data2), // NRPN Coarse
                0x62 => channel_info.set_nrpn_fine(data2), // NRPN Fine
                0x65 => channel_info.set_rpn_coarse(data2), // RPN Coarse
                0x64 => channel_info.set_rpn_fine(data2), // RPN Fine
                0x78 => self.note_off_all_channel(channel, true), // All Sound Off
                0x79 => self.reset_all_controllers_channel(channel), // Reset All Controllers
                0x7B => self.note_off_all_channel(channel, false), // All Note Off
                _ => (),
            },
            0xC0 => channel_info.set_patch(data1), // Program Change
            0xE0 => channel_info.set_pitch_bend(data1, data2), // Pitch Bend
            _ => (),
        }
    }

    pub fn process_sysex_message(&mut self, bytes: &[u8]) {
        let event = decode_sysex_event(bytes);
        if let Some((channel, role)) = event.channel_role {
            self.set_channel_role(channel, role);
        }
    }

    pub fn set_channel_role(&mut self, channel: u8, role: MidiChannelRole) {
        let Some(channel_info) = self.channels.get_mut(channel as usize) else {
            return;
        };
        channel_info.set_percussion_channel(role == MidiChannelRole::Percussion);
    }

    pub fn set_percussion_channels(&mut self, channels: &BTreeSet<u8>) {
        for (channel_index, channel_info) in self.channels.iter_mut().enumerate() {
            channel_info.set_percussion_channel(channels.contains(&(channel_index as u8)));
        }
    }

    pub fn is_percussion_channel(&self, channel: i32) -> bool {
        if !(0 <= channel && channel < self.channels.len() as i32) {
            return false;
        }
        self.channels[channel as usize].is_percussion_channel()
    }

    /// Stops a note.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel of the note.
    /// * `key` - The key of the note.
    pub fn note_off(&mut self, channel: i32, key: i32) {
        if !(0 <= channel && channel < self.channels.len() as i32) {
            return;
        }

        for voice in self.voices.get_active_voices().iter_mut() {
            if voice.channel() == channel && voice.key() == key {
                voice.end();
            }
        }
    }

    /// Starts a note.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel of the note.
    /// * `key` - The key of the note.
    /// * `velocity` - The velocity of the note.
    pub fn note_on(&mut self, channel: i32, key: i32, velocity: i32) {
        if velocity == 0 {
            self.note_off(channel, key);
            return;
        }

        if !(0 <= channel && channel < self.channels.len() as i32) {
            return;
        }

        let channel_info = &self.channels[channel as usize];

        let preset_id = (channel_info.get_bank_number() << 16) | channel_info.get_patch_number();

        let mut preset = self.default_preset;
        match self.preset_lookup.get(&preset_id) {
            Some(value) => preset = *value,
            None => {
                // Try fallback to the GM sound set.
                // Normally, the given patch number + the bank number 0 will work.
                // For drums (bank number >= 128), it seems to be better to select the standard set (128:0).
                let gm_preset_id = if channel_info.get_bank_number() < 128 {
                    channel_info.get_patch_number()
                } else {
                    128 << 16
                };

                // If no corresponding preset was found. Use the default one...
                if let Some(value) = self.preset_lookup.get(&gm_preset_id) {
                    preset = *value
                }
            }
        }

        let preset = &self.sound_font.presets[preset];
        for preset_region in preset.regions.iter() {
            if preset_region.contains(key, velocity) {
                let instrument = &self.sound_font.instruments[preset_region.instrument];
                for instrument_region in instrument.regions.iter() {
                    if instrument_region.contains(key, velocity) {
                        let region_pair = RegionPair::new(preset_region, instrument_region);

                        if let Some(value) = self.voices.request_new(instrument_region, channel) {
                            value.start(
                                &region_pair,
                                channel,
                                channel_info.get_bank_number(),
                                channel_info.get_patch_number(),
                                key,
                                velocity,
                            )
                        }
                    }
                }
            }
        }
    }

    /// Stops all the notes in the specified channel.
    ///
    /// # Arguments
    ///
    /// * `immediate` - If `true`, notes will stop immediately without the release sound.
    pub fn note_off_all(&mut self, immediate: bool) {
        if immediate {
            self.voices.clear();
        } else {
            for voice in self.voices.get_active_voices().iter_mut() {
                voice.end();
            }
        }
    }

    /// Stops all the notes in the specified channel.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel in which the notes will be stopped.
    /// * `immediate` - If `true`, notes will stop immediately without the release sound.
    pub fn note_off_all_channel(&mut self, channel: i32, immediate: bool) {
        if immediate {
            for voice in self.voices.get_active_voices().iter_mut() {
                if voice.channel() == channel {
                    voice.kill();
                }
            }
        } else {
            for voice in self.voices.get_active_voices().iter_mut() {
                if voice.channel() == channel {
                    voice.end();
                }
            }
        }
    }

    /// Resets all the controllers.
    pub fn reset_all_controllers(&mut self) {
        for channel in &mut self.channels {
            channel.reset_all_controllers();
        }
    }

    /// Resets all the controllers of the specified channel.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel to be reset.
    pub fn reset_all_controllers_channel(&mut self, channel: i32) {
        if !(0 <= channel && channel < self.channels.len() as i32) {
            return;
        }

        self.channels[channel as usize].reset_all_controllers();
    }

    /// Resets the synthesizer.
    pub fn reset(&mut self) {
        self.voices.clear();

        for channel in &mut self.channels {
            channel.reset();
        }

        if let Some(effects) = self.effects.as_mut() {
            effects.reverb.mute();
            effects.chorus.mute();
        }

        self.clear_stem_render_cache();

        self.block_read = self.block_size;
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

        let mut wrote = 0;
        while wrote < left_length {
            if self.block_read == self.block_size {
                self.render_block();
                self.block_read = 0;
            }

            let src_rem = self.block_size - self.block_read;
            let dst_rem = left_length - wrote;
            let rem = cmp::min(src_rem, dst_rem);

            for t in 0..rem {
                left[wrote + t] = self.block_left[self.block_read + t];
                right[wrote + t] = self.block_right[self.block_read + t];
            }

            self.block_read += rem;
            wrote += rem;
        }
    }

    pub fn render_stems(&mut self, request: &StemRenderRequest, frames: usize) -> StemRenderSet {
        let mut blocks = Vec::new();
        self.render_stems_into(request, frames, &mut blocks);
        StemRenderSet::new(frames, blocks).expect("rendered stem blocks should be valid")
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
        let mut wrote = 0;
        while wrote < frames {
            if self.block_read == self.block_size {
                self.render_block_with_stems(request);
                allocations.add_assign(&self.last_stem_render_allocations);
                self.block_read = 0;
            }

            let src_rem = self.block_size - self.block_read;
            let dst_rem = frames - wrote;
            let rem = cmp::min(src_rem, dst_rem);

            for block in &self.block_stems {
                let index = upsert_output_stem_block(blocks, &mut active, block, frames);

                blocks[index].left[wrote..wrote + rem]
                    .copy_from_slice(&block.left[self.block_read..self.block_read + rem]);
                blocks[index].right[wrote..wrote + rem]
                    .copy_from_slice(&block.right[self.block_read..self.block_read + rem]);
            }

            self.block_read += rem;
            wrote += rem;
        }

        let mut index = 0;
        blocks.retain(|_| {
            let keep = active.get(index).copied().unwrap_or(false);
            index += 1;
            keep
        });
        allocations.add_output_blocks(&blocks);
        allocations.output_growth_bytes =
            stem_render_blocks_total_bytes(blocks).saturating_sub(output_bytes_before);
        self.last_stem_render_allocations = allocations;
    }

    fn render_block(&mut self) {
        self.render_block_internal(None);
    }

    fn render_block_with_stems(&mut self, request: &StemRenderRequest) {
        self.render_block_internal(Some(request));
    }

    fn render_block_internal(&mut self, stem_request: Option<&StemRenderRequest>) {
        self.voices
            .process(&self.sound_font.wave_data, &self.channels);

        self.block_left.fill(0_f32);
        self.block_right.fill(0_f32);
        let internal_bytes_before = stem_render_blocks_total_bytes(&self.block_stems)
            .saturating_add(stem_render_blocks_total_bytes(&self.dry_stem_scratch));
        let effect_input_bytes_before =
            stem_effect_inputs_total_bytes(&self.stem_effect_input_scratch);
        let residual_bytes_before =
            f32_vec_bytes(&self.residual_left).saturating_add(f32_vec_bytes(&self.residual_right));
        self.block_stems.clear();
        prepare_stem_blocks(&mut self.dry_stem_scratch, self.block_size);
        prepare_stem_effect_inputs(&mut self.stem_effect_input_scratch);
        self.last_stem_render_allocations = StemRenderAllocationDebug::default();

        let wants_grouped_stems = stem_request
            .map(|request| request.requires_voice_grouping())
            .unwrap_or(false);
        for voice in self.voices.get_active_voices().iter_mut() {
            let previous_gain_left = self.master_volume * voice.previous_mix_gain_left;
            let current_gain_left = self.master_volume * voice.current_mix_gain_left;
            Synthesizer::write_block(
                previous_gain_left,
                current_gain_left,
                voice.block(),
                &mut self.block_left[..],
                self.inverse_block_size,
            );
            let previous_gain_right = self.master_volume * voice.previous_mix_gain_right;
            let current_gain_right = self.master_volume * voice.current_mix_gain_right;
            Synthesizer::write_block(
                previous_gain_right,
                current_gain_right,
                voice.block(),
                &mut self.block_right[..],
                self.inverse_block_size,
            );

            if wants_grouped_stems {
                if let Some(request) = stem_request {
                    Self::write_voice_stems(
                        request,
                        &self.channels,
                        voice,
                        previous_gain_left,
                        current_gain_left,
                        previous_gain_right,
                        current_gain_right,
                        self.inverse_block_size,
                        self.block_size,
                        &mut self.dry_stem_scratch,
                    );
                }
            }
        }

        let mut reverb_input_gain = 0.0;
        if let Some(effects) = self.effects.as_mut() {
            let chorus = &mut effects.chorus;
            let chorus_input_left = &mut effects.chorus_input_left[..];
            let chorus_input_right = &mut effects.chorus_input_right[..];
            let chorus_output_left = &mut effects.chorus_output_left[..];
            let chorus_output_right = &mut effects.chorus_output_right[..];
            chorus_input_left.fill(0_f32);
            chorus_input_right.fill(0_f32);
            for voice in self.voices.get_active_voices().iter_mut() {
                let previous_gain_left = voice.previous_chorus_send * voice.previous_mix_gain_left;
                let current_gain_left = voice.current_chorus_send * voice.current_mix_gain_left;
                Synthesizer::write_block(
                    previous_gain_left,
                    current_gain_left,
                    voice.block(),
                    chorus_input_left,
                    self.inverse_block_size,
                );
                let previous_gain_right =
                    voice.previous_chorus_send * voice.previous_mix_gain_right;
                let current_gain_right = voice.current_chorus_send * voice.current_mix_gain_right;
                Synthesizer::write_block(
                    previous_gain_right,
                    current_gain_right,
                    voice.block(),
                    chorus_input_right,
                    self.inverse_block_size,
                );
            }
            chorus.process(
                chorus_input_left,
                chorus_input_right,
                chorus_output_left,
                chorus_output_right,
            );
            ArrayMath::multiply_add(
                self.master_volume,
                chorus_output_left,
                &mut self.block_left[..],
            );
            ArrayMath::multiply_add(
                self.master_volume,
                chorus_output_right,
                &mut self.block_right[..],
            );

            let reverb = &mut effects.reverb;
            reverb_input_gain = reverb.get_input_gain();
            let reverb_input = &mut effects.reverb_input[..];
            let reverb_output_left = &mut effects.reverb_output_left[..];
            let reverb_output_right = &mut effects.reverb_output_right[..];
            reverb_input.fill(0_f32);
            for voice in self.voices.get_active_voices().iter_mut() {
                let previous_gain = reverb.get_input_gain()
                    * voice.previous_reverb_send
                    * (voice.previous_mix_gain_left + voice.previous_mix_gain_right);
                let current_gain = reverb.get_input_gain()
                    * voice.current_reverb_send
                    * (voice.current_mix_gain_left + voice.current_mix_gain_right);
                Synthesizer::write_block(
                    previous_gain,
                    current_gain,
                    voice.block(),
                    &mut reverb_input[..],
                    self.inverse_block_size,
                );
            }

            reverb.process(reverb_input, reverb_output_left, reverb_output_right);
            ArrayMath::multiply_add(
                self.master_volume,
                reverb_output_left,
                &mut self.block_left[..],
            );
            ArrayMath::multiply_add(
                self.master_volume,
                reverb_output_right,
                &mut self.block_right[..],
            );
        }

        if wants_grouped_stems && self.effects.is_some() {
            if let Some(request) = stem_request {
                for voice in self.voices.get_active_voices().iter_mut() {
                    Self::write_voice_effect_inputs(
                        request,
                        &self.channels,
                        voice,
                        reverb_input_gain,
                        self.inverse_block_size,
                        self.block_size,
                        &mut self.stem_effect_input_scratch,
                    );
                }
                self.stem_effect_input_scratch
                    .retain(|input| !input.active_notes.is_empty());
                let mut allocations = self.last_stem_render_allocations;
                allocations.add_effect_inputs(&self.stem_effect_input_scratch);
                allocations.effect_input_growth_bytes =
                    stem_effect_inputs_total_bytes(&self.stem_effect_input_scratch)
                        .saturating_sub(effect_input_bytes_before);
                self.last_stem_render_allocations = allocations;
                self.add_stem_effect_outputs();
            }
        }

        if let Some(request) = stem_request {
            if request.requires_voice_grouping() {
                let mut allocations = self.last_stem_render_allocations;
                self.dry_stem_scratch.retain(stem_block_has_signal);
                std::mem::swap(&mut self.block_stems, &mut self.dry_stem_scratch);
                allocations.residual_buffer_bytes = Self::reconcile_stems_with_direct_block(
                    request,
                    &self.block_left,
                    &self.block_right,
                    &mut self.block_stems,
                    &mut self.residual_left,
                    &mut self.residual_right,
                );
                allocations.residual_growth_bytes = f32_vec_bytes(&self.residual_left)
                    .saturating_add(f32_vec_bytes(&self.residual_right))
                    .saturating_sub(residual_bytes_before);
                allocations.add_internal_blocks(&self.block_stems);
                allocations.internal_growth_bytes =
                    stem_render_blocks_total_bytes(&self.block_stems)
                        .saturating_add(stem_render_blocks_total_bytes(&self.dry_stem_scratch))
                        .saturating_sub(internal_bytes_before);
                self.last_stem_render_allocations = allocations;
            } else {
                self.block_stems.push(StemRenderBlock::whole_soundfont(
                    request.soundfont_id.clone(),
                    self.block_left.clone(),
                    self.block_right.clone(),
                ));
                let mut allocations = StemRenderAllocationDebug::default();
                allocations.add_internal_blocks(&self.block_stems);
                allocations.internal_growth_bytes =
                    stem_render_blocks_total_bytes(&self.block_stems)
                        .saturating_add(stem_render_blocks_total_bytes(&self.dry_stem_scratch))
                        .saturating_sub(internal_bytes_before);
                self.last_stem_render_allocations = allocations;
            }
        }
    }

    fn reconcile_stems_with_direct_block(
        request: &StemRenderRequest,
        direct_left: &[f32],
        direct_right: &[f32],
        stems: &mut Vec<StemRenderBlock>,
        residual_left: &mut Vec<f32>,
        residual_right: &mut Vec<f32>,
    ) -> usize {
        if direct_left.is_empty() || direct_right.is_empty() {
            return 0;
        }

        residual_left.resize(direct_left.len(), 0.0);
        residual_right.resize(direct_right.len(), 0.0);
        residual_left.fill(0.0);
        residual_right.fill(0.0);
        let residual_buffer_bytes = f32_vec_bytes(&residual_left) + f32_vec_bytes(&residual_right);
        let mut has_residual = false;
        for frame in 0..direct_left.len() {
            let mut stem_left = 0.0;
            let mut stem_right = 0.0;
            for stem in stems.iter() {
                stem_left += stem.left[frame];
                stem_right += stem.right[frame];
            }

            residual_left[frame] = direct_left[frame] - stem_left;
            residual_right[frame] = direct_right[frame] - stem_right;
            has_residual |= residual_left[frame].abs() > f32::EPSILON
                || residual_right[frame].abs() > f32::EPSILON;
        }

        if !has_residual {
            return residual_buffer_bytes;
        }

        if let Some(stem) = stems.iter_mut().find(|stem| {
            stem.identity == StemIdentity::global_effects(request.soundfont_id.clone())
        }) {
            for frame in 0..direct_left.len() {
                stem.left[frame] += residual_left[frame];
                stem.right[frame] += residual_right[frame];
            }
        } else {
            stems.push(StemRenderBlock::whole_soundfont_named(
                request.soundfont_id.clone(),
                "Global effects",
                residual_left.clone(),
                residual_right.clone(),
            ));
        }
        residual_buffer_bytes
    }

    fn write_voice_stems(
        request: &StemRenderRequest,
        channels: &[Channel],
        voice: &Voice,
        previous_gain_left: f32,
        current_gain_left: f32,
        previous_gain_right: f32,
        current_gain_right: f32,
        inverse_block_size: f32,
        block_size: usize,
        stems: &mut Vec<StemRenderBlock>,
    ) {
        for mode in &request.modes {
            let Some(identity) = Self::voice_stem_identity(request, mode, channels, voice) else {
                continue;
            };

            let index = match stems.iter().position(|stem| stem.identity == identity) {
                Some(index) => index,
                None => {
                    stems.push(StemRenderBlock {
                        identity,
                        display_name: None,
                        active_notes: Vec::new(),
                        left: vec![0.0; block_size],
                        right: vec![0.0; block_size],
                    });
                    stems.len() - 1
                }
            };

            let note = voice.key() as u8;
            if !stems[index].active_notes.contains(&note) {
                stems[index].active_notes.push(note);
            }

            Self::write_block(
                previous_gain_left,
                current_gain_left,
                voice.block(),
                &mut stems[index].left[..],
                inverse_block_size,
            );
            Self::write_block(
                previous_gain_right,
                current_gain_right,
                voice.block(),
                &mut stems[index].right[..],
                inverse_block_size,
            );
        }
    }

    fn write_voice_effect_inputs(
        request: &StemRenderRequest,
        channels: &[Channel],
        voice: &Voice,
        reverb_input_gain: f32,
        inverse_block_size: f32,
        block_size: usize,
        inputs: &mut Vec<StemEffectInput>,
    ) {
        for mode in &request.modes {
            let Some(identity) = Self::voice_stem_identity(request, mode, channels, voice) else {
                continue;
            };

            let index = match inputs.iter().position(|input| input.identity == identity) {
                Some(index) => index,
                None => {
                    inputs.push(StemEffectInput::new(identity, block_size));
                    inputs.len() - 1
                }
            };

            let note = voice.key() as u8;
            if !inputs[index].active_notes.contains(&note) {
                inputs[index].active_notes.push(note);
            }

            let previous_chorus_left = voice.previous_chorus_send * voice.previous_mix_gain_left;
            let current_chorus_left = voice.current_chorus_send * voice.current_mix_gain_left;
            Self::write_block(
                previous_chorus_left,
                current_chorus_left,
                voice.block(),
                &mut inputs[index].chorus_input_left[..],
                inverse_block_size,
            );

            let previous_chorus_right = voice.previous_chorus_send * voice.previous_mix_gain_right;
            let current_chorus_right = voice.current_chorus_send * voice.current_mix_gain_right;
            Self::write_block(
                previous_chorus_right,
                current_chorus_right,
                voice.block(),
                &mut inputs[index].chorus_input_right[..],
                inverse_block_size,
            );

            let previous_reverb = reverb_input_gain
                * voice.previous_reverb_send
                * (voice.previous_mix_gain_left + voice.previous_mix_gain_right);
            let current_reverb = reverb_input_gain
                * voice.current_reverb_send
                * (voice.current_mix_gain_left + voice.current_mix_gain_right);
            Self::write_block(
                previous_reverb,
                current_reverb,
                voice.block(),
                &mut inputs[index].reverb_input[..],
                inverse_block_size,
            );
        }
    }

    fn voice_stem_identity(
        request: &StemRenderRequest,
        mode: &StemRenderMode,
        channels: &[Channel],
        voice: &Voice,
    ) -> Option<StemIdentity> {
        let channel = voice.channel() as u8;
        let is_percussion = channels
            .get(channel as usize)
            .map(Channel::is_percussion_channel)
            .unwrap_or(false);
        match mode {
            StemRenderMode::WholeSoundFont => None,
            StemRenderMode::MidiChannel => Some(StemIdentity::midi_channel(
                request.soundfont_id.clone(),
                channel,
            )),
            StemRenderMode::MidiProgram => {
                if is_percussion {
                    Some(StemIdentity::percussion_channel(
                        request.soundfont_id.clone(),
                        channel,
                    ))
                } else {
                    Some(StemIdentity::midi_program(
                        request.soundfont_id.clone(),
                        voice.program() as u8,
                    ))
                }
            }
            StemRenderMode::Percussion => {
                if is_percussion {
                    Some(StemIdentity::percussion_channel(
                        request.soundfont_id.clone(),
                        channel,
                    ))
                } else {
                    None
                }
            }
            StemRenderMode::ChannelProgram => Some(
                StemIdentity::channel_program(
                    request.soundfont_id.clone(),
                    channel,
                    voice.program() as u8,
                    is_percussion,
                )
                .with_bank(if is_percussion {
                    Some(128)
                } else {
                    u16::try_from(voice.bank()).ok()
                }),
            ),
        }
    }

    fn add_stem_effect_outputs(&mut self) {
        for effects in self.stem_effects.values_mut() {
            effects.clear_inputs();
        }

        for input in &self.stem_effect_input_scratch {
            if !self.stem_effects.contains_key(&input.identity) {
                let effects = StemEffects::new(self.sample_rate, self.block_size);
                let bytes = effects.memory_debug_bytes();
                self.stem_effect_allocation_counters.allocations = self
                    .stem_effect_allocation_counters
                    .allocations
                    .saturating_add(1);
                self.stem_effect_allocation_counters.allocated_bytes = self
                    .stem_effect_allocation_counters
                    .allocated_bytes
                    .saturating_add(bytes as u64);
                self.stem_effects.insert(input.identity.clone(), effects);
            }
            let effects = self
                .stem_effects
                .get_mut(&input.identity)
                .expect("stem effects identity should exist after insertion");
            effects.reverb_input.copy_from_slice(&input.reverb_input);
            effects
                .chorus_input_left
                .copy_from_slice(&input.chorus_input_left);
            effects
                .chorus_input_right
                .copy_from_slice(&input.chorus_input_right);

            if let Some(stem) = self
                .dry_stem_scratch
                .iter_mut()
                .find(|stem| stem.identity == input.identity)
            {
                for note in &input.active_notes {
                    if !stem.active_notes.contains(&note) {
                        stem.active_notes.push(*note);
                    }
                }
            }
        }

        let identities = self.stem_effects.keys().cloned().collect::<Vec<_>>();
        for identity in identities {
            let effects = self
                .stem_effects
                .get_mut(&identity)
                .expect("stem effects identity should exist");
            effects.chorus.process(
                &effects.chorus_input_left,
                &effects.chorus_input_right,
                &mut effects.chorus_output_left,
                &mut effects.chorus_output_right,
            );
            effects.reverb.process(
                &effects.reverb_input,
                &mut effects.reverb_output_left,
                &mut effects.reverb_output_right,
            );

            let has_wet_output = effects
                .chorus_output_left
                .iter()
                .chain(effects.chorus_output_right.iter())
                .chain(effects.reverb_output_left.iter())
                .chain(effects.reverb_output_right.iter())
                .any(|sample| sample.abs() >= SoundFontMath::NON_AUDIBLE);
            if !has_wet_output {
                continue;
            }

            let index = match self
                .dry_stem_scratch
                .iter()
                .position(|stem| stem.identity == identity)
            {
                Some(index) => index,
                None => {
                    self.dry_stem_scratch.push(StemRenderBlock {
                        identity,
                        display_name: None,
                        active_notes: Vec::new(),
                        left: vec![0.0; self.block_size],
                        right: vec![0.0; self.block_size],
                    });
                    self.dry_stem_scratch.len() - 1
                }
            };

            ArrayMath::multiply_add(
                self.master_volume,
                &effects.chorus_output_left,
                &mut self.dry_stem_scratch[index].left[..],
            );
            ArrayMath::multiply_add(
                self.master_volume,
                &effects.chorus_output_right,
                &mut self.dry_stem_scratch[index].right[..],
            );
            ArrayMath::multiply_add(
                self.master_volume,
                &effects.reverb_output_left,
                &mut self.dry_stem_scratch[index].left[..],
            );
            ArrayMath::multiply_add(
                self.master_volume,
                &effects.reverb_output_right,
                &mut self.dry_stem_scratch[index].right[..],
            );
        }
    }

    fn write_block(
        previous_gain: f32,
        current_gain: f32,
        source: &[f32],
        destination: &mut [f32],
        inverse_block_size: f32,
    ) {
        if SoundFontMath::max(previous_gain, current_gain) < SoundFontMath::NON_AUDIBLE {
            return;
        }

        if (current_gain - previous_gain).abs() < 1.0E-3_f32 {
            ArrayMath::multiply_add(current_gain, source, destination);
        } else {
            let step = inverse_block_size * (current_gain - previous_gain);
            ArrayMath::multiply_add_slope(previous_gain, step, source, destination);
        }
    }

    /// Gets the SoundFont used as the audio source.
    pub fn get_sound_font(&self) -> &SoundFont {
        &self.sound_font
    }

    /// Gets the sample rate for synthesis.
    pub fn get_sample_rate(&self) -> i32 {
        self.sample_rate
    }

    /// Gets the block size for rendering waveform.
    pub fn get_block_size(&self) -> usize {
        self.block_size
    }

    /// Gets the number of maximum polyphony.
    pub fn get_maximum_polyphony(&self) -> usize {
        self.maximum_polyphony
    }

    /// Gets the value indicating whether reverb and chorus are enabled.
    pub fn get_enable_reverb_and_chorus(&self) -> bool {
        self.effects.is_some()
    }

    /// Gets the master volume.
    pub fn get_master_volume(&self) -> f32 {
        self.master_volume
    }

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

    /// Sets the master volume.
    ///
    /// # Arguments
    ///
    /// * `value` - The new value of the master volume.
    pub fn set_master_volume(&mut self, value: f32) {
        self.master_volume = value;
    }

    fn clear_stem_render_cache(&mut self) {
        let stem_effect_bytes = self
            .stem_effects
            .values()
            .map(StemEffects::memory_debug_bytes)
            .sum::<usize>();
        let stem_effect_count = self.stem_effects.len() as u64;
        if stem_effect_count > 0 || stem_effect_bytes > 0 {
            self.stem_effect_allocation_counters.deallocations = self
                .stem_effect_allocation_counters
                .deallocations
                .saturating_add(stem_effect_count);
            self.stem_effect_allocation_counters.deallocated_bytes = self
                .stem_effect_allocation_counters
                .deallocated_bytes
                .saturating_add(stem_effect_bytes as u64);
            self.stem_effect_allocation_counters.cache_clears = self
                .stem_effect_allocation_counters
                .cache_clears
                .saturating_add(1);
            self.stem_effect_allocation_counters.cache_released_bytes = self
                .stem_effect_allocation_counters
                .cache_released_bytes
                .saturating_add(stem_effect_bytes as u64);
        }
        self.stem_effects.clear();
        self.stem_effects.shrink_to_fit();
        self.block_stems.clear();
        self.block_stems.shrink_to_fit();
        self.dry_stem_scratch.clear();
        self.dry_stem_scratch.shrink_to_fit();
        self.stem_effect_input_scratch.clear();
        self.stem_effect_input_scratch.shrink_to_fit();
        self.residual_left.clear();
        self.residual_left.shrink_to_fit();
        self.residual_right.clear();
        self.residual_right.shrink_to_fit();
    }
}

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

    fn add_output_blocks(&mut self, blocks: &Vec<StemRenderBlock>) {
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

    fn add_internal_blocks(&mut self, blocks: &Vec<StemRenderBlock>) {
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

    fn add_effect_inputs(&mut self, inputs: &Vec<StemEffectInput>) {
        self.effect_input_bytes = self
            .effect_input_bytes
            .saturating_add(inputs.iter().map(StemEffectInput::memory_debug_bytes).sum());
        self.effect_input_vec_bytes = self
            .effect_input_vec_bytes
            .saturating_add(inputs.capacity() * mem::size_of::<StemEffectInput>());
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

#[derive(Debug, Clone, Default)]
struct StemEffectAllocationCounters {
    allocations: u64,
    deallocations: u64,
    allocated_bytes: u64,
    deallocated_bytes: u64,
    cache_clears: u64,
    cache_released_bytes: u64,
}

fn f32_vec_bytes(values: &Vec<f32>) -> usize {
    values.capacity() * mem::size_of::<f32>()
}

fn stem_render_block_buffer_bytes(block: &StemRenderBlock) -> usize {
    f32_vec_bytes(&block.left)
        + f32_vec_bytes(&block.right)
        + block.active_notes.capacity() * mem::size_of::<u8>()
}

fn stem_render_blocks_audio_bytes(blocks: &[StemRenderBlock]) -> usize {
    blocks
        .iter()
        .map(|block| f32_vec_bytes(&block.left) + f32_vec_bytes(&block.right))
        .sum()
}

fn stem_render_blocks_active_note_bytes(blocks: &[StemRenderBlock]) -> usize {
    blocks
        .iter()
        .map(|block| block.active_notes.capacity() * mem::size_of::<u8>())
        .sum()
}

fn stem_render_blocks_total_bytes(blocks: &Vec<StemRenderBlock>) -> usize {
    blocks
        .capacity()
        .saturating_mul(mem::size_of::<StemRenderBlock>())
        .saturating_add(stem_render_blocks_audio_bytes(blocks))
        .saturating_add(stem_render_blocks_active_note_bytes(blocks))
}

fn stem_effect_inputs_total_bytes(inputs: &Vec<StemEffectInput>) -> usize {
    inputs
        .capacity()
        .saturating_mul(mem::size_of::<StemEffectInput>())
        .saturating_add(inputs.iter().map(StemEffectInput::memory_debug_bytes).sum())
}

fn prepare_stem_blocks(blocks: &mut [StemRenderBlock], block_size: usize) {
    for block in blocks {
        block.active_notes.clear();
        block.left.resize(block_size, 0.0);
        block.right.resize(block_size, 0.0);
        block.left.fill(0.0);
        block.right.fill(0.0);
    }
}

fn stem_block_has_signal(block: &StemRenderBlock) -> bool {
    !block.active_notes.is_empty()
        || block
            .left
            .iter()
            .chain(block.right.iter())
            .any(|sample| sample.abs() >= SoundFontMath::NON_AUDIBLE)
}

fn prepare_stem_effect_inputs(inputs: &mut [StemEffectInput]) {
    for input in inputs {
        input.active_notes.clear();
        input.reverb_input.fill(0.0);
        input.chorus_input_left.fill(0.0);
        input.chorus_input_right.fill(0.0);
    }
}

fn upsert_output_stem_block(
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
struct Effects {
    reverb: Reverb,
    reverb_input: Vec<f32>,
    reverb_output_left: Vec<f32>,
    reverb_output_right: Vec<f32>,

    chorus: Chorus,
    chorus_input_left: Vec<f32>,
    chorus_input_right: Vec<f32>,
    chorus_output_left: Vec<f32>,
    chorus_output_right: Vec<f32>,
}

impl Effects {
    fn new(settings: &SynthesizerSettings) -> Effects {
        Self {
            reverb: Reverb::new(settings.sample_rate),
            reverb_input: vec![0_f32; settings.block_size],
            reverb_output_left: vec![0_f32; settings.block_size],
            reverb_output_right: vec![0_f32; settings.block_size],
            chorus: Chorus::new(settings.sample_rate, 0.002, 0.0019, 0.4),
            chorus_input_left: vec![0_f32; settings.block_size],
            chorus_input_right: vec![0_f32; settings.block_size],
            chorus_output_left: vec![0_f32; settings.block_size],
            chorus_output_right: vec![0_f32; settings.block_size],
        }
    }

    fn memory_debug_bytes(&self) -> usize {
        f32_vec_bytes(&self.reverb_input)
            + f32_vec_bytes(&self.reverb_output_left)
            + f32_vec_bytes(&self.reverb_output_right)
            + f32_vec_bytes(&self.chorus_input_left)
            + f32_vec_bytes(&self.chorus_input_right)
            + f32_vec_bytes(&self.chorus_output_left)
            + f32_vec_bytes(&self.chorus_output_right)
    }
}

#[derive(Debug)]
struct StemEffectInput {
    identity: StemIdentity,
    active_notes: Vec<u8>,
    reverb_input: Vec<f32>,
    chorus_input_left: Vec<f32>,
    chorus_input_right: Vec<f32>,
}

impl StemEffectInput {
    fn new(identity: StemIdentity, block_size: usize) -> Self {
        Self {
            identity,
            active_notes: Vec::new(),
            reverb_input: vec![0.0; block_size],
            chorus_input_left: vec![0.0; block_size],
            chorus_input_right: vec![0.0; block_size],
        }
    }

    fn memory_debug_bytes(&self) -> usize {
        f32_vec_bytes(&self.reverb_input)
            + f32_vec_bytes(&self.chorus_input_left)
            + f32_vec_bytes(&self.chorus_input_right)
            + self.active_notes.capacity() * mem::size_of::<u8>()
    }
}

#[derive(Debug)]
struct StemEffects {
    reverb: Reverb,
    reverb_input: Vec<f32>,
    reverb_output_left: Vec<f32>,
    reverb_output_right: Vec<f32>,

    chorus: Chorus,
    chorus_input_left: Vec<f32>,
    chorus_input_right: Vec<f32>,
    chorus_output_left: Vec<f32>,
    chorus_output_right: Vec<f32>,
}

impl StemEffects {
    fn new(sample_rate: i32, block_size: usize) -> Self {
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

    fn clear_inputs(&mut self) {
        self.reverb_input.fill(0.0);
        self.chorus_input_left.fill(0.0);
        self.chorus_input_right.fill(0.0);
    }

    fn memory_debug_bytes(&self) -> usize {
        f32_vec_bytes(&self.reverb_input)
            + f32_vec_bytes(&self.reverb_output_left)
            + f32_vec_bytes(&self.reverb_output_right)
            + f32_vec_bytes(&self.chorus_input_left)
            + f32_vec_bytes(&self.chorus_input_right)
            + f32_vec_bytes(&self.chorus_output_left)
            + f32_vec_bytes(&self.chorus_output_right)
    }
}
