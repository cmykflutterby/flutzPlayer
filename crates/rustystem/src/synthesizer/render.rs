use super::*;

impl Synthesizer {
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
}
