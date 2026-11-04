use super::*;

impl PlaybackController {
    pub fn new(
        data_dir: PathBuf,
        catalog: Vec<SoundFontCatalogEntry>,
        audio_backend: AudioBackend,
        debug_analyzer: bool,
        debug_render_errors: bool,
    ) -> Self {
        Self {
            data_dir,
            catalog,
            engine: None,
            mixer: Arc::new(Mutex::new(MixerEngine::new(MixerSettings::default()))),
            mixer_controls: Arc::new(Mutex::new(MixerControlState::default())),
            session_output_gain: Arc::new(Mutex::new(0.5)),
            latest_snapshot: Arc::new(Mutex::new(SnapshotHistory::default())),
            visualizer_analyzer: Arc::new(Mutex::new(VisualizerAnalyzer::new(
                VisualizerAnalyzerConfig::with_sample_rate(
                    AudioDeviceConfig::default().sample_rate,
                ),
            ))),
            analyzer_trace: debug_analyzer
                .then(AnalyzerTrace::open)
                .flatten()
                .map(|trace| Arc::new(Mutex::new(trace))),
            render_error_trace: debug_render_errors
                .then(RenderErrorTrace::open)
                .flatten()
                .map(|trace| Arc::new(Mutex::new(trace))),
            rendered_frame_clock: Arc::new(AtomicU64::new(0)),
            audio_backend,
            audio: None,
            audio_config: AudioDeviceConfig::default(),
            flux_guard: Mutex::new(FluxGuard::default()),
            audio_error: None,
            decoded_audio: None,
            loaded_midi: None,
            loaded_midi_bytes: None,
            loaded_soundfonts: Vec::new(),
            loaded_coverages: BTreeMap::new(),
            soundfont_cache: SoundFontRuntimeCache::default(),
            midi_transport: MidiTransportMetadata::default(),
            midi_strips: Arc::new(Mutex::new(Vec::new())),
            render_scratch: Arc::new(Mutex::new(PlaybackRenderScratch::default())),
            render_churn: Arc::new(Mutex::new(RenderChurnDiagnostics::default())),
            lifecycle: PlaybackLifecycleDiagnostics::default(),
            last_demand_profile: MidiDemandProfile::default(),
            last_midi_scan: PlaybackMidiScanDiagnostics::default(),
            last_soundfont_demand: SoundFontDemandDiagnostics::default(),
            loaded_subset_state: BTreeMap::new(),
            subset_transition: PlaybackSubsetTransitionDiagnostics::default(),
            last_subset_plans: BTreeMap::new(),
            last_soundfont_load: PlaybackSoundFontLoadDiagnostics::default(),
            ui_waker: None,
            last_prewarm: None,
        }
    }

    /// Installs the callback the audio thread uses to wake the UI when
    /// playback stops on its own; streams opened afterwards pick it up.
    pub fn set_ui_waker(&mut self, waker: UiWaker) {
        self.ui_waker = Some(waker);
    }

    pub fn play(&mut self) -> Result<AudioPlaybackStatus> {
        self.prewarm_if_resuming_cold();
        if let Some(decoded) = self.decoded_audio.as_ref().map(Arc::clone) {
            {
                let mut decoded = decoded.lock().map_err(|_| {
                    FlutzError::Runtime("decoded audio lock is poisoned".to_owned())
                })?;
                decoded.play();
            }
            if let Err(error) = self.ensure_audio_stream() {
                let message = format!("audio unavailable: {error}");
                self.audio_error = Some(message.clone());
                if let Ok(mut decoded) = decoded.lock() {
                    decoded.pause();
                }
                return Ok(AudioPlaybackStatus::AudioUnavailable(message));
            }
            if let Some(audio) = &mut self.audio {
                if let Err(error) = audio.resume() {
                    self.audio = None;
                    let message = format!("audio resume failed: {error}");
                    self.audio_error = Some(message.clone());
                    if let Ok(mut decoded) = decoded.lock() {
                        decoded.pause();
                    }
                    return Ok(AudioPlaybackStatus::AudioUnavailable(message));
                }
            }
            self.update_latency_control();
            self.audio_error = None;
            memory_runtime::note_audio_activity();
            return Ok(AudioPlaybackStatus::Audible);
        }
        let engine = self.engine.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load a MIDI file before playing".to_owned())
        })?;
        {
            let mut engine = engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?;
            if engine.state() == PlaybackState::Paused {
                engine.resume();
            } else {
                engine.play()?;
            }
        }
        if let Err(error) = self.ensure_audio_stream() {
            let message = format!("audio unavailable: {error}");
            self.audio_error = Some(message.clone());
            if let Some(engine) = &self.engine {
                if let Ok(mut engine) = engine.lock() {
                    engine.pause();
                }
            }
            return Ok(AudioPlaybackStatus::AudioUnavailable(message));
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.resume() {
                self.audio = None;
                let message = format!("audio resume failed: {error}");
                self.audio_error = Some(message.clone());
                if let Some(engine) = &self.engine {
                    if let Ok(mut engine) = engine.lock() {
                        engine.pause();
                    }
                }
                return Ok(AudioPlaybackStatus::AudioUnavailable(message));
            }
        }
        self.update_latency_control();
        self.audio_error = None;
        memory_runtime::note_audio_activity();
        Ok(AudioPlaybackStatus::Audible)
    }

    pub fn pause(&mut self) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .pause();
        }
        if let Some(engine) = &self.engine {
            engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
                .pause();
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio = None;
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        memory_runtime::record_audio_working_set();
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        memory_runtime::decay_idle_reuse_preserving(false);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(decoded) = &self.decoded_audio {
            decoded
                .lock()
                .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
                .stop();
        }
        if let Some(engine) = &self.engine {
            engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
                .stop();
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio = None;
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        memory_runtime::record_audio_working_set();
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        memory_runtime::decay_idle_reuse_preserving(false);
        Ok(())
    }

    pub fn release_idle_resources(&mut self) -> Result<()> {
        self.stop()?;
        self.audio = None;
        self.audio_error = None;
        self.engine = None;
        self.decoded_audio = None;
        self.loaded_midi = None;
        self.loaded_midi_bytes = None;
        self.loaded_soundfonts.clear();
        self.loaded_coverages.clear();
        self.loaded_subset_state.clear();
        self.last_subset_plans.clear();
        self.subset_transition = PlaybackSubsetTransitionDiagnostics::default();
        self.midi_transport = MidiTransportMetadata::default();
        if let Ok(mut midi_strips) = self.midi_strips.lock() {
            midi_strips.clear();
        }
        self.soundfont_cache.clear();
        self.mixer
            .lock()
            .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?
            .reset_state_and_release_scratch();
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.render_scratch
            .lock()
            .map_err(|_| {
                FlutzError::Runtime("playback render scratch lock is poisoned".to_owned())
            })?
            .release_capacity();
        self.reset_visualizer_to_silence();
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        memory_runtime::aggressive_release();
        Ok(())
    }

    pub fn retry_audio(&mut self) -> Result<()> {
        self.audio = None;
        self.audio_error = None;
        self.reset_flux_guard();
        self.ensure_audio_stream()?;
        Ok(())
    }

    pub fn status_line(&self) -> String {
        if let Some(decoded) = &self.decoded_audio {
            return decoded
                .lock()
                .map(|decoded| {
                    format!(
                        "{}, {} Hz, {} channel(s), {}",
                        decoded.metadata.friendly_name,
                        decoded.metadata.sample_rate,
                        decoded.metadata.channels,
                        self.audio_status_text()
                    )
                })
                .unwrap_or_else(|_| "decoded audio unavailable".to_owned());
        }
        let Some(engine) = &self.engine else {
            return "no MIDI loaded".to_owned();
        };
        match engine.lock() {
            Ok(engine) => format!(
                "{} synth(s), {} Hz, {}",
                engine.instance_count(),
                engine.sample_rate(),
                self.audio_status_text()
            ),
            Err(_) => "playback engine unavailable".to_owned(),
        }
    }

    pub fn audio_status_text(&self) -> String {
        if let Some(error) = &self.audio_error {
            format!("audio unavailable ({error})")
        } else if let Some(audio) = &self.audio {
            let diagnostics = audio.diagnostics();
            let device = diagnostics
                .opened_device_name
                .as_deref()
                .unwrap_or("default playback");
            format!("audio ready via {} ({device})", audio.backend_name())
        } else {
            "audio closed".to_owned()
        }
    }

    pub fn playback_active(&self) -> bool {
        if self.engine_state() == Some(PlaybackState::Playing) {
            return true;
        }
        self.decoded_audio
            .as_ref()
            .and_then(|decoded| decoded.lock().ok().map(|decoded| decoded.is_playing()))
            .unwrap_or(false)
    }

    pub(super) fn ensure_audio_stream(&mut self) -> Result<()> {
        if self.audio.is_some() {
            return Ok(());
        }
        if let Some(decoded) = &self.decoded_audio {
            let audio_decoded = Arc::clone(decoded);
            let audio_session_output_gain = Arc::clone(&self.session_output_gain);
            let audio_snapshot = Arc::clone(&self.latest_snapshot);
            let audio_visualizer = Arc::clone(&self.visualizer_analyzer);
            let audio_analyzer_trace = self.analyzer_trace.as_ref().map(Arc::clone);
            let audio_frame_clock = Arc::clone(&self.rendered_frame_clock);
            let audio_backend = self.audio_backend;
            let audio_sample_rate = self.audio_config.sample_rate;
            let audio_ui_waker = self.ui_waker.clone();
            let mut audio_was_playing = false;
            let audio =
                AudioOutput::open_f32_stream(audio_backend, self.audio_config, move |output| {
                    let _allocation_scope =
                        AllocationScopeGuard::enter(AllocationScope::AudioCallback);
                    let output_gain = audio_session_output_gain
                        .lock()
                        .map(|gain| *gain)
                        .unwrap_or(1.0);
                    let (meter, playing_now) = match audio_decoded.lock() {
                        Ok(mut decoded) => {
                            let meter =
                                render_decoded_audio_stereo(&mut decoded, output, output_gain)
                                    .unwrap_or_else(|_| {
                                        output.fill(0.0);
                                        MeterReading::default()
                                    });
                            (meter, decoded.is_playing())
                        }
                        Err(_) => {
                            output.fill(0.0);
                            (MeterReading::default(), false)
                        }
                    };
                    if playing_now {
                        memory_runtime::note_audio_activity();
                    }
                    let rendered_frames = output.len() / 2;
                    let rendered_frame_end = audio_frame_clock
                        .fetch_add(rendered_frames as u64, Ordering::Relaxed)
                        .saturating_add(rendered_frames as u64);
                    if let Ok(mut snapshot_history) = audio_snapshot.lock() {
                        snapshot_history.record(
                            rendered_frame_end,
                            RealtimeMixerSnapshot {
                                output_meter: meter,
                                strips: BTreeMap::new(),
                            },
                            audio_sample_rate,
                        );
                    }
                    if let Ok(mut analyzer) = audio_visualizer.lock() {
                        let frame = analyzer.ingest_interleaved_stereo(
                            output,
                            rendered_frames as f32 / audio_sample_rate.max(1) as f32,
                        );
                        record_analyzer_trace_arc(audio_analyzer_trace.as_ref(), &frame);
                    }
                    wake_ui_on_playback_end(
                        &mut audio_was_playing,
                        playing_now,
                        audio_ui_waker.as_ref(),
                    );
                })
                .map_err(FlutzError::Runtime)?;
            self.audio = Some(audio);
            self.audio_error = None;
            return Ok(());
        }
        let engine = self.engine.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load a MIDI file before opening audio".to_owned())
        })?;
        let audio_engine = Arc::clone(engine);
        let audio_mixer = Arc::clone(&self.mixer);
        let audio_controls = Arc::clone(&self.mixer_controls);
        let audio_session_output_gain = Arc::clone(&self.session_output_gain);
        let audio_snapshot = Arc::clone(&self.latest_snapshot);
        let audio_visualizer = Arc::clone(&self.visualizer_analyzer);
        let audio_analyzer_trace = self.analyzer_trace.as_ref().map(Arc::clone);
        let audio_render_error_trace = self.render_error_trace.as_ref().map(Arc::clone);
        let audio_frame_clock = Arc::clone(&self.rendered_frame_clock);
        let audio_midi_strips = Arc::clone(&self.midi_strips);
        let audio_render_scratch = Arc::clone(&self.render_scratch);
        let audio_soundfont_ids = self.loaded_soundfonts.clone();
        let audio_render_churn = Arc::clone(&self.render_churn);
        let audio_backend = self.audio_backend;
        let audio_sample_rate = self.audio_config.sample_rate;
        let audio_midi_source = self.loaded_midi.clone();
        let audio_midi_scan = self.last_midi_scan.clone();
        let audio_ui_waker = self.ui_waker.clone();
        let mut audio_was_playing = false;
        let audio = AudioOutput::open_f32_stream(audio_backend, self.audio_config, move |output| {
            let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::AudioCallback);
            let Ok(mut engine) = audio_engine.lock() else {
                output.fill(0.0);
                return;
            };
            let Ok(mut mixer) = audio_mixer.lock() else {
                output.fill(0.0);
                return;
            };
            let Ok(midi_strips) = audio_midi_strips.lock() else {
                output.fill(0.0);
                return;
            };
            let controls = clone_mixer_controls_with_recovery(
                &audio_controls,
                audio_render_error_trace.as_ref(),
                "audio-callback",
                audio_midi_source.as_deref(),
            );
            let Ok(mut scratch) = audio_render_scratch.lock() else {
                output.fill(0.0);
                return;
            };
            let output_gain = audio_session_output_gain
                .lock()
                .map(|gain| *gain)
                .unwrap_or(1.0);
            match render_mixed_audio_guarded(
                &mut engine,
                &mut mixer,
                &mut scratch,
                &controls,
                output_gain,
                &audio_soundfont_ids,
                &midi_strips,
                output,
                audio_render_error_trace.as_ref(),
                RenderFailureContext {
                    source: "audio-callback",
                    frames_requested: output.len() / 2,
                    midi_source: audio_midi_source.as_deref(),
                    midi_scan: &audio_midi_scan,
                },
            ) {
                Some(report) => {
                    let rendered_frame_end = audio_frame_clock
                        .fetch_add(report.frames as u64, Ordering::Relaxed)
                        .saturating_add(report.frames as u64);
                    if let Ok(mut snapshot_history) = audio_snapshot.lock() {
                        snapshot_history.record(
                            rendered_frame_end,
                            report.snapshot,
                            audio_sample_rate,
                        );
                    }
                    if let Ok(mut analyzer) = audio_visualizer.lock() {
                        let frame = analyzer.ingest_interleaved_stereo(
                            output,
                            report.frames as f32 / audio_sample_rate.max(1) as f32,
                        );
                        record_analyzer_trace_arc(audio_analyzer_trace.as_ref(), &frame);
                    }
                    if let Ok(mut diagnostics) = audio_render_churn.lock() {
                        diagnostics.record(report.churn);
                    }
                }
                None => {
                    output.fill(0.0);
                    if let Ok(mut analyzer) = audio_visualizer.lock() {
                        let frame = analyzer.advance_silence(
                            output.len() / 2,
                            (output.len() / 2) as f32 / audio_sample_rate.max(1) as f32,
                        );
                        record_analyzer_trace_arc(audio_analyzer_trace.as_ref(), &frame);
                    }
                }
            }
            let playing_now = engine.state() == PlaybackState::Playing;
            if playing_now {
                memory_runtime::note_audio_activity();
            }
            drop(scratch);
            drop(midi_strips);
            drop(mixer);
            drop(engine);
            wake_ui_on_playback_end(&mut audio_was_playing, playing_now, audio_ui_waker.as_ref());
        })
        .map_err(FlutzError::Runtime)?;
        self.audio = Some(audio);
        self.audio_error = None;
        Ok(())
    }

    pub(super) fn record_engine_replacement(&mut self) {
        let Some(engine) = &self.engine else {
            return;
        };
        let Ok(engine) = engine.lock() else {
            return;
        };
        let memory = engine.memory_debug();
        self.lifecycle.engine_replacements = self.lifecycle.engine_replacements.saturating_add(1);
        self.lifecycle.cumulative_replaced_engine_estimated_bytes = self
            .lifecycle
            .cumulative_replaced_engine_estimated_bytes
            .saturating_add(memory.estimated_bytes as u64);
        self.lifecycle.last_replaced_engine_estimated_bytes = memory.estimated_bytes;
        self.lifecycle.last_replaced_engine_instances = memory.instance_count;
        self.lifecycle.last_replaced_stem_effects = memory.stem_effects;
        self.lifecycle.last_replaced_stem_effect_bytes = memory.stem_effect_bytes;
    }
}

impl Drop for PlaybackController {
    fn drop(&mut self) {
        if let Some(audio) = &mut self.audio {
            let _ = audio.pause();
        }
        self.audio = None;
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.reset_state_and_release_scratch();
        }
        self.soundfont_cache.clear();
        memory_runtime::purge_domain(MemoryDomain::FileLoad);
        memory_runtime::purge_domain(MemoryDomain::PlaybackLoad);
        memory_runtime::purge_domain(MemoryDomain::SoundFontDecode);
        memory_runtime::decay_idle_reuse_preserving(false);
    }
}

/// Wakes the UI once when the callback observes playback stop by itself, so
/// playlist advance doesn't wait for the next (possibly slow) repaint.
fn wake_ui_on_playback_end(was_playing: &mut bool, playing_now: bool, waker: Option<&UiWaker>) {
    if *was_playing && !playing_now {
        if let Some(waker) = waker {
            waker();
        }
    }
    *was_playing = playing_now;
}
