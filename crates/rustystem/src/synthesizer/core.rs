use super::*;

#[derive(Debug)]
#[non_exhaustive]
pub struct Synthesizer {
    pub(crate) sound_font: Arc<SoundFont>,
    pub(crate) sample_rate: i32,
    pub(crate) block_size: usize,
    pub(crate) maximum_polyphony: usize,

    pub(super) preset_lookup: HashMap<i32, usize>,
    pub(super) default_preset: usize,

    pub(super) channels: Vec<Channel>,

    pub(super) voices: VoiceCollection,

    pub(super) block_left: Vec<f32>,
    pub(super) block_right: Vec<f32>,
    pub(super) block_stems: Vec<StemRenderBlock>,
    pub(super) dry_stem_scratch: Vec<StemRenderBlock>,
    pub(super) stem_effect_input_scratch: Vec<StemEffectInput>,
    pub(super) residual_left: Vec<f32>,
    pub(super) residual_right: Vec<f32>,
    pub(super) stem_effects: HashMap<StemIdentity, StemEffects>,
    pub(super) stem_effect_allocation_counters: StemEffectAllocationCounters,
    pub(super) last_stem_render_allocations: StemRenderAllocationDebug,

    pub(super) inverse_block_size: f32,

    pub(super) block_read: usize,

    pub(super) master_volume: f32,

    pub(super) effects: Option<Effects>,
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
}

impl Synthesizer {
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
