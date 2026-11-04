use super::*;

mod support;
pub(in crate::playback) use support::{
    clone_mixer_controls_with_recovery, coverage_estimated_bytes, expected_audio_closure_roots,
    expected_engine_roots, record_analyzer_trace_arc, record_render_error_event,
    render_mixed_audio_guarded, soundfont_catalog_entry_string_bytes, stem_blocks_audio_bytes,
    stem_blocks_total_bytes, AnalyzerTrace, RenderErrorTrace, RenderFailureContext,
};
pub use support::{
    ArcReferenceDiagnostics, AudioRuntimeMemoryDiagnostics, MidiRuntimeMemoryDiagnostics,
    PlaybackComponentMemoryDiagnostics, PlaybackDebugMetrics, PlaybackLifecycleDiagnostics,
    PlaybackReferenceDiagnostics, RenderChurnDiagnostics, RenderChurnSnapshot,
    RenderErrorTraceEvent, RustystemRuntimeDiagnostics, SoundFontMetadataDiagnostics,
    VisualizerDebugMetrics,
};

impl PlaybackController {
    pub(super) fn reset_visualizer_to_silence(&self) {
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            let frame = analyzer.reset_to_silence();
            self.record_analyzer_trace(&frame);
        }
    }

    pub(super) fn record_analyzer_trace(&self, frame: &VisualizerFrame) {
        record_analyzer_trace_arc(self.analyzer_trace.as_ref(), frame);
    }

    pub fn visualizer_frame(&self) -> VisualizerFrame {
        self.visualizer_analyzer
            .lock()
            .map(|analyzer| analyzer.latest_frame())
            .unwrap_or_default()
    }

    pub fn take_render_error_events(&self) -> Vec<RenderErrorTraceEvent> {
        self.render_error_trace
            .as_ref()
            .and_then(|trace| trace.lock().ok().map(|mut trace| trace.take_recent()))
            .unwrap_or_default()
    }

    pub(super) fn record_render_lock_recovery(
        &self,
        source: &'static str,
        lock_name: &'static str,
    ) {
        record_render_error_event(
            self.render_error_trace.as_ref(),
            RenderErrorTraceEvent {
                source: source.to_owned(),
                failure_kind: "lock-recovery".to_owned(),
                detail: format!("recovered poisoned {lock_name} lock"),
                backtrace: None,
                frames_requested: 0,
                midi_source: self
                    .loaded_midi
                    .as_ref()
                    .map(|path| path.display().to_string()),
                soundfont_ids: self.loaded_soundfonts.clone(),
                midi_scan: self.last_midi_scan.clone(),
            },
        );
    }

    pub(super) fn clone_mixer_controls_for_render(
        &self,
        source: &'static str,
    ) -> MixerControlState {
        clone_mixer_controls_with_recovery(
            &self.mixer_controls,
            self.render_error_trace.as_ref(),
            source,
            self.loaded_midi.as_deref(),
        )
    }

    pub fn playback_memory_debug(&self) -> Option<PlaybackMemoryDebug> {
        self.engine
            .as_ref()
            .and_then(|engine| engine.lock().ok().map(|engine| engine.memory_debug()))
    }

    pub fn output_latency_breakdown(&self) -> OutputLatencyBreakdown {
        let diagnostics = self.audio.as_ref().map(AudioOutput::diagnostics);
        output_latency_breakdown(diagnostics.as_ref())
    }

    pub fn estimated_output_latency_frames(&self) -> u64 {
        self.output_latency_breakdown().total_frames
    }

    pub fn estimated_output_latency_seconds(&self) -> f64 {
        let sample_rate = self.audio_config.sample_rate.max(1) as f64;
        self.estimated_output_latency_frames() as f64 / sample_rate
    }

    pub fn audible_transport_seconds(&self) -> f64 {
        if let Some(decoded) = &self.decoded_audio {
            return decoded
                .lock()
                .map(|decoded| decoded.position_seconds())
                .unwrap_or(0.0);
        }
        let Some(engine) = &self.engine else {
            return 0.0;
        };
        match engine.lock() {
            Ok(engine) => (engine.position_seconds() - self.estimated_output_latency_seconds())
                .clamp(0.0, self.midi_transport.duration_seconds.max(0.0)),
            Err(_) => 0.0,
        }
    }

    pub fn update_latency_control(&mut self) {
        let Some(audio) = self.audio.as_ref() else {
            let _ = self
                .flux_guard
                .lock()
                .map(|mut guard| guard.observe(None, self.audio_config.sample_rate));
            return;
        };
        let state = audio.render_ahead_state();
        let snapshot = self
            .flux_guard
            .lock()
            .map(|mut guard| guard.observe(Some(state), self.audio_config.sample_rate))
            .unwrap_or_default();
        if !snapshot.decision.changed {
            return;
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.apply_render_ahead_target(snapshot.decision.target) {
                self.audio_error = Some(format!("audio render-ahead target failed: {error}"));
            }
        }
    }

    pub fn debug_metrics(&self) -> PlaybackDebugMetrics {
        let decoded_transport = self.decoded_audio.as_ref().and_then(|decoded| {
            decoded.lock().ok().map(|decoded| {
                (
                    decoded.state_label().to_owned(),
                    decoded.position_seconds(),
                    decoded.position_frame,
                    decoded.metadata.duration_seconds,
                )
            })
        });
        let (engine_state, transport_seconds, transport_tick, memory_debug) = self
            .engine
            .as_ref()
            .and_then(|engine| {
                engine.lock().ok().map(|engine| {
                    (
                        format!("{:?}", engine.state()),
                        engine.position_seconds(),
                        engine.position_ticks(),
                        Some(engine.memory_debug()),
                    )
                })
            })
            .unwrap_or_else(|| {
                decoded_transport
                    .as_ref()
                    .map(|(state, seconds, frame, _duration)| {
                        (state.clone(), *seconds, *frame, None)
                    })
                    .unwrap_or_else(|| ("unloaded".to_owned(), 0.0, 0, None))
            });
        let audio_diagnostics = self.audio.as_ref().map(AudioOutput::diagnostics);
        let flux_guard = self.flux_guard_snapshot();
        let audible_output = self.audible_output_view();
        let live_output = self.live_output_view();
        let snapshot = &audible_output.snapshot;
        let component_memory =
            self.component_memory_diagnostics(memory_debug.as_ref(), audio_diagnostics.as_ref());
        PlaybackDebugMetrics {
            engine_state,
            transport_seconds,
            transport_duration_seconds: decoded_transport
                .as_ref()
                .map(|(_, _, _, duration)| *duration)
                .unwrap_or(self.midi_transport.duration_seconds),
            transport_tick,
            loaded_soundfont_count: self.loaded_soundfonts.len(),
            requested_soundfont_count: self.last_soundfont_demand.requested_soundfont_count,
            loaded_provider_count: self.last_soundfont_demand.loaded_provider_count,
            pruned_soundfont_count: self.last_soundfont_demand.pruned_soundfont_count,
            loaded_midi_bytes: self
                .loaded_midi_bytes
                .as_ref()
                .map(Vec::len)
                .unwrap_or_default(),
            loaded_midi_capacity_bytes: self
                .loaded_midi_bytes
                .as_ref()
                .map(Vec::capacity)
                .unwrap_or_default(),
            midi_strip_count: self
                .midi_strips
                .lock()
                .map(|strips| strips.len())
                .unwrap_or_default(),
            midi_demand: self.last_demand_profile.clone(),
            subset_plans: SoundFontSubsetPlanAggregateDebug::from_plans(&self.last_subset_plans),
            loaded_subset_state: self.loaded_subset_state.clone(),
            subset_transition: self.subset_transition.clone(),
            soundfont_load: self.last_soundfont_load.clone(),
            output_peak: snapshot.output_meter.peak,
            output_rms: snapshot.output_meter.rms,
            meter_latency_frames: audible_output.effective_latency_frames,
            meter_latency_ms: audible_output.effective_latency_ms,
            meter_wrapper_queue_frames: audible_output.wrapper_queue_frames,
            meter_device_queue_frames: audible_output.device_queue_frames,
            live_frame_clock: live_output.rendered_frame_clock,
            audible_frame_clock: audible_output.rendered_frame_clock,
            active_strip_count: snapshot
                .strips
                .values()
                .filter(|strip| strip.audible || !strip.active_notes.is_empty())
                .count(),
            audio_status: self.audio_status_text(),
            audio_error: self.audio_error.clone(),
            audio_backend: self.audio_backend.name(),
            audio_config: self.audio_config,
            audio_diagnostics,
            flux_guard,
            memory_debug,
            render_churn: self
                .render_churn
                .lock()
                .map(|diagnostics| diagnostics.clone())
                .unwrap_or_default(),
            lifecycle: self.lifecycle,
            soundfont_cache: self.soundfont_cache.debug(),
            component_memory,
            references: self.reference_diagnostics(),
            visualizer: VisualizerDebugMetrics::from_frame(&self.visualizer_frame()),
        }
    }

    pub(super) fn reference_diagnostics(&self) -> PlaybackReferenceDiagnostics {
        PlaybackReferenceDiagnostics {
            engine: self.engine.as_ref().map(|engine| {
                ArcReferenceDiagnostics::from_arc(
                    engine,
                    expected_engine_roots(self.audio.is_some()),
                )
            }),
            mixer: ArcReferenceDiagnostics::from_arc(
                &self.mixer,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            mixer_controls: ArcReferenceDiagnostics::from_arc(
                &self.mixer_controls,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            latest_snapshot: ArcReferenceDiagnostics::from_arc(
                &self.latest_snapshot,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            midi_strips: ArcReferenceDiagnostics::from_arc(
                &self.midi_strips,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            render_scratch: ArcReferenceDiagnostics::from_arc(
                &self.render_scratch,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            render_churn: ArcReferenceDiagnostics::from_arc(
                &self.render_churn,
                expected_audio_closure_roots(self.audio.is_some()),
            ),
            audio_stream_open: self.audio.is_some(),
        }
    }

    pub(super) fn flux_guard_snapshot(&self) -> FluxGuardSnapshot {
        self.flux_guard
            .lock()
            .map(|guard| guard.snapshot())
            .unwrap_or_default()
    }

    pub(super) fn reset_flux_guard(&self) {
        if let Ok(mut guard) = self.flux_guard.lock() {
            guard.reset();
        }
    }

    pub(super) fn engine_state(&self) -> Option<PlaybackState> {
        self.engine
            .as_ref()
            .and_then(|engine| engine.lock().ok().map(|engine| engine.state()))
    }

    pub(super) fn component_memory_diagnostics(
        &self,
        memory_debug: Option<&PlaybackMemoryDebug>,
        audio_diagnostics: Option<&AudioDeviceDiagnostics>,
    ) -> PlaybackComponentMemoryDiagnostics {
        let soundfont_metadata = self.soundfont_metadata_diagnostics();
        let midi = self.midi_runtime_diagnostics(memory_debug);
        let rustystem = RustystemRuntimeDiagnostics::from_memory_debug(memory_debug);
        let audio = AudioRuntimeMemoryDiagnostics::from_audio_diagnostics(audio_diagnostics);
        let tracked_total_bytes = soundfont_metadata
            .estimated_bytes
            .saturating_add(midi.estimated_bytes)
            .saturating_add(rustystem.estimated_bytes)
            .saturating_add(audio.estimated_bytes);
        PlaybackComponentMemoryDiagnostics {
            soundfont_metadata,
            midi,
            rustystem,
            audio,
            tracked_total_bytes,
        }
    }

    pub(super) fn soundfont_metadata_diagnostics(&self) -> SoundFontMetadataDiagnostics {
        let catalog_vec_bytes = self.catalog.capacity() * mem::size_of::<SoundFontCatalogEntry>();
        let catalog_string_bytes = self
            .catalog
            .iter()
            .map(soundfont_catalog_entry_string_bytes)
            .sum::<usize>();
        let loaded_soundfont_id_bytes = self
            .loaded_soundfonts
            .capacity()
            .saturating_mul(mem::size_of::<String>())
            .saturating_add(
                self.loaded_soundfonts
                    .iter()
                    .map(String::capacity)
                    .sum::<usize>(),
            );
        let loaded_coverage_estimated_bytes = self
            .loaded_coverages
            .iter()
            .map(|(id, coverage)| {
                id.capacity()
                    .saturating_add(coverage_estimated_bytes(coverage))
            })
            .sum::<usize>()
            .saturating_add(
                self.loaded_coverages
                    .len()
                    .saturating_mul(mem::size_of::<(String, SoundFontCoverage)>()),
            );
        let estimated_bytes = catalog_vec_bytes
            .saturating_add(catalog_string_bytes)
            .saturating_add(loaded_soundfont_id_bytes)
            .saturating_add(loaded_coverage_estimated_bytes);
        SoundFontMetadataDiagnostics {
            catalog_entries: self.catalog.len(),
            catalog_estimated_bytes: catalog_vec_bytes.saturating_add(catalog_string_bytes),
            loaded_soundfont_ids: self.loaded_soundfonts.len(),
            loaded_soundfont_id_bytes,
            loaded_coverage_entries: self.loaded_coverages.len(),
            loaded_coverage_estimated_bytes,
            estimated_bytes,
        }
    }

    pub(super) fn midi_runtime_diagnostics(
        &self,
        memory_debug: Option<&PlaybackMemoryDebug>,
    ) -> MidiRuntimeMemoryDiagnostics {
        let strip_capacity = self
            .midi_strips
            .lock()
            .map(|strips| strips.capacity())
            .unwrap_or_default();
        let strip_count = self
            .midi_strips
            .lock()
            .map(|strips| strips.len())
            .unwrap_or_default();
        let strip_bytes = strip_capacity * mem::size_of::<MidiStripDescriptor>();
        let jump_point_bytes = self.midi_transport.jump_points.capacity() * mem::size_of::<u64>();
        let parsed = memory_debug.and_then(|memory| memory.midi_file);
        let parsed_estimated_bytes = parsed
            .map(|debug| debug.estimated_bytes)
            .unwrap_or_default();
        let raw_bytes = self
            .loaded_midi_bytes
            .as_ref()
            .map(Vec::len)
            .unwrap_or_default();
        let raw_capacity_bytes = self
            .loaded_midi_bytes
            .as_ref()
            .map(Vec::capacity)
            .unwrap_or_default();
        let estimated_bytes = raw_capacity_bytes
            .saturating_add(strip_bytes)
            .saturating_add(jump_point_bytes)
            .saturating_add(parsed_estimated_bytes);
        MidiRuntimeMemoryDiagnostics {
            raw_bytes,
            raw_capacity_bytes,
            strip_count,
            strip_capacity,
            strip_bytes,
            jump_point_count: self.midi_transport.jump_points.len(),
            jump_point_bytes,
            parsed_message_count: parsed.map(|debug| debug.message_count).unwrap_or_default(),
            parsed_sysex_events: parsed.map(|debug| debug.sysex_events).unwrap_or_default(),
            parsed_sysex_bytes: parsed.map(|debug| debug.sysex_bytes).unwrap_or_default(),
            parsed_estimated_bytes,
            estimated_bytes,
        }
    }
}
