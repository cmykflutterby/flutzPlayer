use super::*;

mod support;
pub(in crate::playback) use support::{
    analyze_midi_file, json_option_string, json_string, json_string_array, loop_type_label,
    midi_scan_json, midi_scan_trace_json, MidiDemandProfile,
};
pub use support::{
    BankProgramDemand, MidiStripDescriptor, MidiTransportMetadata, PlaybackMidiChannelRoleChange,
    PlaybackMidiScanDiagnostics, PlaybackMidiSysexEvent, RenderProbeReport,
    RenderProbeStripMixDiagnostics, RenderStemProbeReport, SoundFontDemandDiagnostics,
    SoundFontDemandSignature,
};

impl PlaybackController {
    pub fn load_midi_file(
        &mut self,
        path: &Path,
        requested_soundfonts: &[String],
    ) -> Result<String> {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::FileLoad);
        let midi_bytes = fs::read(path).map_err(|error| {
            FlutzError::Runtime(format!(
                "failed to read MIDI file {}: {error}",
                path.display()
            ))
        })?;
        self.load_midi_data(midi_bytes, path.to_owned(), requested_soundfonts)
    }

    pub fn load_midi_bytes(
        &mut self,
        midi_bytes: Vec<u8>,
        source_name: impl Into<String>,
        requested_soundfonts: &[String],
    ) -> Result<String> {
        self.load_midi_data(
            midi_bytes,
            PathBuf::from(source_name.into()),
            requested_soundfonts,
        )
    }

    fn load_midi_data(
        &mut self,
        midi_bytes: Vec<u8>,
        source_path: PathBuf,
        requested_soundfonts: &[String],
    ) -> Result<String> {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::PlaybackLoad);
        self.decoded_audio = None;
        let scan = analyze_midi_file(&midi_bytes)?;
        let detected_loop_type = scan.detected_loop_type;
        self.last_demand_profile = scan.demand_profile.clone();
        self.last_midi_scan = scan.diagnostics.clone();
        let soundfont_ids =
            self.resolve_soundfont_ids(requested_soundfonts, &scan.channel_roles)?;
        self.last_soundfont_demand = self.soundfont_demand_diagnostics(
            requested_soundfonts,
            &soundfont_ids,
            &scan.demand_profile,
        );
        let transition = self.capture_render_layer_transition(&midi_bytes, &soundfont_ids);
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        memory_runtime::record_audio_working_set();

        let config = PlaybackConfig::default();
        self.audio_config = AudioDeviceConfig {
            sample_rate: config.sample_rate,
            channels: 2,
            internal_block_frames: config.block_frames as u16,
            ..AudioDeviceConfig::default()
        };
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            *analyzer = VisualizerAnalyzer::new(VisualizerAnalyzerConfig::with_sample_rate(
                self.audio_config.sample_rate,
            ));
        }

        let can_reuse_loaded_engine = self.loaded_soundfonts == soundfont_ids
            && self.engine.is_some()
            && self.loaded_subset_state_satisfies(&soundfont_ids, &scan.channel_roles)?;
        let mut fallback_note = None::<String>;
        let loaded_midi = if can_reuse_loaded_engine {
            if let Some(engine) = &self.engine {
                engine
                    .lock()
                    .map_err(|_| {
                        FlutzError::Runtime("playback engine lock is poisoned".to_owned())
                    })?
                    .load_midi_bytes_with_loop_type(&midi_bytes, detected_loop_type)?
            } else {
                unreachable!("can_reuse_loaded_engine requires an existing playback engine")
            }
        } else {
            self.audio = None;
            self.audio_error = None;
            let mut attempts = vec![soundfont_ids.clone()];
            if requested_soundfonts.is_empty() {
                for candidate in self.catalog.iter().map(|font| font.internal_id.clone()) {
                    if !soundfont_ids.contains(&candidate) {
                        attempts.push(vec![candidate]);
                    }
                }
            }

            let mut load_errors = Vec::new();
            let mut loaded = None;
            for candidate_ids in attempts {
                match self.load_soundfonts_for_playback(&candidate_ids, &scan.channel_roles) {
                    Ok(soundfonts) => {
                        match MultiSoundFontPlayback::new_with_loaded_soundfonts(soundfonts, config)
                        {
                            Ok(mut engine) => {
                                let loaded_midi = engine.load_midi_bytes_with_loop_type(
                                    &midi_bytes,
                                    detected_loop_type,
                                )?;
                                self.last_soundfont_demand = self.soundfont_demand_diagnostics(
                                    requested_soundfonts,
                                    &candidate_ids,
                                    &scan.demand_profile,
                                );
                                loaded = Some((candidate_ids, loaded_midi, engine));
                                break;
                            }
                            Err(error) => {
                                load_errors.push(format!("{} => {error}", candidate_ids.join(",")));
                            }
                        }
                    }
                    Err(error) => {
                        load_errors.push(format!("{} => {error}", candidate_ids.join(",")));
                    }
                }
            }

            let Some((resolved_ids, loaded_midi, engine)) = loaded else {
                let details = if load_errors.is_empty() {
                    "no candidate soundfonts could be loaded".to_owned()
                } else {
                    load_errors.join("; ")
                };
                return Err(FlutzError::InvalidInput(format!(
                    "failed to initialize playback soundfont set: {details}"
                )));
            };

            if resolved_ids != soundfont_ids {
                fallback_note = Some(format!(
                    "Fallback soundfont selection: requested [{}], loaded [{}]",
                    soundfont_ids.join(", "),
                    resolved_ids.join(", ")
                ));
            }

            self.record_engine_replacement();
            self.loaded_soundfonts = resolved_ids;
            self.loaded_coverages = engine.soundfont_coverages().into_iter().collect();
            self.engine = Some(Arc::new(Mutex::new(engine)));
            self.soundfont_cache.release_unused();
            loaded_midi
        };

        let analyzed_strips = loaded_midi
            .channel_program_roles()
            .into_iter()
            .map(|role| MidiStripDescriptor {
                channel: role.channel,
                bank: role.bank,
                program: role.program,
                is_percussion: role.is_percussion,
            })
            .collect::<Vec<_>>();

        self.loaded_midi = Some(source_path);
        self.loaded_midi_bytes = Some(midi_bytes);
        self.midi_transport = MidiTransportMetadata::from_loaded_midi(&loaded_midi);
        if self.loaded_soundfonts.is_empty() {
            self.loaded_soundfonts = soundfont_ids;
        }
        if self.loaded_coverages.is_empty() {
            if let Some(engine) = &self.engine {
                self.loaded_coverages = engine
                    .lock()
                    .map_err(|_| {
                        FlutzError::Runtime("playback engine lock is poisoned".to_owned())
                    })?
                    .soundfont_coverages()
                    .into_iter()
                    .collect();
            }
        }
        *self.midi_strips.lock().map_err(|_| {
            FlutzError::Runtime("MIDI strip analysis lock is poisoned".to_owned())
        })? = analyzed_strips;
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        *self.render_churn.lock().map_err(|_| {
            FlutzError::Runtime("render churn diagnostics lock is poisoned".to_owned())
        })? = RenderChurnDiagnostics::default();
        self.render_scratch
            .lock()
            .map_err(|_| {
                FlutzError::Runtime("playback render scratch lock is poisoned".to_owned())
            })?
            .release_capacity();
        memory_runtime::decay_idle_reuse_preserving(false);
        memory_runtime::release_after_content_change();
        *self.latest_snapshot.lock().map_err(|_| {
            FlutzError::Runtime("realtime mixer snapshot lock is poisoned".to_owned())
        })? = SnapshotHistory::default();
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        self.finish_render_layer_transition(transition)?;

        let mut summary = format!(
            "Loaded {} byte MIDI with {} RustySynth instance(s), {:.2}s, {} ticks",
            loaded_midi.byte_len,
            self.loaded_soundfonts.len(),
            self.midi_transport.duration_seconds,
            self.midi_transport.tick_length,
        );
        if !self.midi_transport.jump_points.is_empty() {
            summary.push_str("\nJump points (tick): ");
            summary.push_str(
                &self
                    .midi_transport
                    .jump_points
                    .iter()
                    .map(|tick| tick.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !matches!(detected_loop_type, MidiFileLoopType::LoopPoint(_)) {
            summary.push_str("\nAuto-detected loop style: ");
            summary.push_str(loop_type_label(detected_loop_type));
        }
        let interpretation = loaded_midi.midi_interpretation();
        if interpretation.sysex_event_count > 0 {
            summary.push_str("\nMIDI SysEx: ");
            summary.push_str(&format!(
                "{} event(s), {} recognized, percussion channels [{}]",
                interpretation.sysex_event_count,
                interpretation.recognized_sysex_event_count,
                interpretation
                    .percussion_channels
                    .iter()
                    .map(|channel| channel.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(note) = fallback_note {
            summary.push_str("\n");
            summary.push_str(&note);
        }

        self.record_midi_scan_trace();

        Ok(summary)
    }

    pub fn midi_scan_diagnostics(&self) -> PlaybackMidiScanDiagnostics {
        self.last_midi_scan.clone()
    }

    fn record_midi_scan_trace(&self) {
        if let Some(trace) = self.render_error_trace.as_ref() {
            if let Ok(mut trace) = trace.lock() {
                trace.record_midi_scan(self.loaded_midi.as_deref(), &self.last_midi_scan);
            }
        }
    }

    fn soundfont_demand_diagnostics(
        &self,
        requested_soundfonts: &[String],
        resolved_soundfonts: &[String],
        demand_profile: &MidiDemandProfile,
    ) -> SoundFontDemandDiagnostics {
        let available = self
            .catalog
            .iter()
            .map(|font| font.internal_id.as_str())
            .collect::<BTreeSet<_>>();
        let requested_available_count = requested_soundfonts
            .iter()
            .filter(|id| available.contains(id.as_str()))
            .count();
        let requested_soundfont_count = if requested_soundfonts.is_empty() {
            resolved_soundfonts.len()
        } else {
            requested_available_count
        };
        let resolved_requested_count = resolved_soundfonts
            .iter()
            .filter(|id| requested_soundfonts.is_empty() || requested_soundfonts.contains(id))
            .count();

        SoundFontDemandDiagnostics {
            requested_soundfont_count,
            loaded_provider_count: resolved_soundfonts.len(),
            pruned_soundfont_count: requested_soundfont_count
                .saturating_sub(resolved_requested_count),
            demand_profile: demand_profile.clone(),
        }
    }

    pub fn transport_fraction(&self) -> f32 {
        if let Some(decoded) = &self.decoded_audio {
            return decoded
                .lock()
                .map(|decoded| decoded.transport_fraction())
                .unwrap_or(0.0);
        }
        let Some(engine) = &self.engine else {
            return 0.0;
        };
        if self.midi_transport.duration_seconds <= 0.0 {
            return 0.0;
        }
        match engine.lock() {
            Ok(engine) => (engine.position_seconds() / self.midi_transport.duration_seconds)
                .clamp(0.0, 1.0) as f32,
            Err(_) => 0.0,
        }
    }

    pub fn transport_tick(&self) -> u64 {
        if let Some(decoded) = &self.decoded_audio {
            return decoded
                .lock()
                .map(|decoded| decoded.position_frame)
                .unwrap_or(0);
        }
        let Some(engine) = &self.engine else {
            return 0;
        };
        match engine.lock() {
            Ok(engine) => engine.position_ticks(),
            Err(_) => 0,
        }
    }

    pub fn seek_transport_fraction(&mut self, fraction: f32) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            let was_playing = self.playback_active();
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .seek_fraction(fraction);
            self.reset_audible_output_after_reposition(was_playing);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        let was_playing = self.engine_state() == Some(PlaybackState::Playing);
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .seek_to_fraction(fraction as f64)?;
        self.reset_audible_output_after_reposition(was_playing);
        Ok(())
    }

    pub fn seek_transport_tick(&mut self, tick: u64) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            let was_playing = self.playback_active();
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .seek_frame(tick);
            self.reset_audible_output_after_reposition(was_playing);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        let was_playing = self.engine_state() == Some(PlaybackState::Playing);
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .seek_to_tick(tick)?;
        self.reset_audible_output_after_reposition(was_playing);
        Ok(())
    }

    pub fn seek_transport_seconds(&mut self, seconds: f64) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            let was_playing = self.playback_active();
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .seek_seconds(seconds);
            self.reset_audible_output_after_reposition(was_playing);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        let was_playing = self.engine_state() == Some(PlaybackState::Playing);
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .seek_to_seconds(seconds)?;
        self.reset_audible_output_after_reposition(was_playing);
        Ok(())
    }

    pub fn set_loop_enabled(&mut self, enabled: bool) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .set_loop_enabled(enabled);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .set_loop_enabled(enabled);
        Ok(())
    }

    pub fn set_loop_settings(&mut self, settings: PlaybackLoopSettings) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .set_loop_settings(settings);
            return Ok(());
        }
        let Some(engine) = &self.engine else {
            return Ok(());
        };
        engine
            .lock()
            .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
            .set_loop_settings(settings);
        Ok(())
    }

    pub fn midi_transport_metadata(&self) -> MidiTransportMetadata {
        self.midi_transport.clone()
    }

    pub fn loaded_midi(&self) -> Option<&Path> {
        self.loaded_midi.as_deref()
    }

    pub fn loaded_midi_bytes(&self) -> Option<&[u8]> {
        self.loaded_midi_bytes.as_deref()
    }

    pub fn midi_strip_layout(&self) -> Vec<(u8, u16, u8, bool)> {
        self.midi_strips
            .lock()
            .map(|strips| {
                strips
                    .iter()
                    .map(|s| (s.channel, s.bank, s.program, s.is_percussion))
                    .collect()
            })
            .unwrap_or_default()
    }
}
