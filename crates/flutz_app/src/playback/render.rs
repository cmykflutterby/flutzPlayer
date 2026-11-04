use super::*;

mod core;
pub(in crate::playback) use core::{
    render_mixed_audio, MixedAudioRenderReport, PlaybackRenderScratch, SnapshotHistory,
};
pub use core::{MixerControlState, RealtimeMixerSnapshot, RealtimeStripSnapshot};

impl PlaybackController {
    pub fn set_mixer_controls(&mut self, controls: MixerControlState) -> Result<()> {
        let mut shared = match self.mixer_controls.lock() {
            Ok(shared) => shared,
            Err(poisoned) => {
                self.record_render_lock_recovery("set_mixer_controls", "mixer_controls");
                poisoned.into_inner()
            }
        };
        *shared = controls;
        Ok(())
    }

    pub fn set_session_output_gain(&mut self, gain: f32) -> Result<()> {
        let mut shared = self
            .session_output_gain
            .lock()
            .map_err(|_| FlutzError::Runtime("session output gain lock is poisoned".to_owned()))?;
        *shared = gain.clamp(0.0, FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER);
        Ok(())
    }

    pub fn latest_snapshot(&self) -> RealtimeMixerSnapshot {
        self.audible_output_view().snapshot
    }

    pub fn live_output_view(&self) -> OutputViewSnapshot {
        let rendered_frames = self.rendered_frame_clock.load(Ordering::Relaxed);
        let snapshot = self
            .latest_snapshot
            .lock()
            .map(|snapshot| snapshot.live_snapshot())
            .unwrap_or_default();
        OutputViewSnapshot {
            kind: OutputViewKind::Live,
            rendered_frame_clock: rendered_frames,
            effective_latency_frames: 0,
            effective_latency_ms: 0.0,
            wrapper_queue_frames: 0,
            device_queue_frames: 0,
            snapshot,
        }
    }

    pub fn audible_output_view(&self) -> OutputViewSnapshot {
        let rendered_frames = self.rendered_frame_clock.load(Ordering::Relaxed);
        let latency = self.output_latency_breakdown();
        let sample_rate = self.audio_config.sample_rate.max(1) as f64;
        let snapshot = self
            .latest_snapshot
            .lock()
            .map(|snapshot| snapshot.delayed_snapshot(rendered_frames, latency.total_frames))
            .unwrap_or_default();
        OutputViewSnapshot {
            kind: OutputViewKind::Audible,
            rendered_frame_clock: rendered_frames,
            effective_latency_frames: latency.total_frames,
            effective_latency_ms: latency.total_frames as f64 * 1000.0 / sample_rate,
            wrapper_queue_frames: latency.wrapper_queue_frames,
            device_queue_frames: latency.device_queue_frames,
            snapshot,
        }
    }

    /// Same snapshot as `audible_output_view().snapshot`, copied into a caller-owned buffer.
    pub fn copy_audible_snapshot_into(&self, target: &mut RealtimeMixerSnapshot) {
        let rendered_frames = self.rendered_frame_clock.load(Ordering::Relaxed);
        let latency = self.output_latency_breakdown();
        match self.latest_snapshot.lock() {
            Ok(history) => {
                history.copy_delayed_snapshot_into(rendered_frames, latency.total_frames, target)
            }
            Err(_) => *target = RealtimeMixerSnapshot::default(),
        }
    }

    pub fn render_probe(&mut self, frames: usize) -> Result<RenderProbeReport> {
        let reports = self.render_probe_sequence(&[frames], true)?;
        reports
            .into_iter()
            .next()
            .ok_or_else(|| FlutzError::Runtime("render probe produced no report".to_owned()))
    }

    pub fn render_probe_sequence(
        &mut self,
        frame_sequence: &[usize],
        stop_after: bool,
    ) -> Result<Vec<RenderProbeReport>> {
        let reports = self.render_probe_sequence_with_stems(frame_sequence, stop_after)?;
        Ok(reports.into_iter().map(|report| report.report).collect())
    }

    pub fn render_probe_sequence_with_stems(
        &mut self,
        frame_sequence: &[usize],
        stop_after: bool,
    ) -> Result<Vec<RenderStemProbeReport>> {
        let engine = self.engine.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load a MIDI file before rendering".to_owned())
        })?;
        let max_frames = frame_sequence.iter().copied().max().unwrap_or(0);
        let mut output = vec![0.0f32; max_frames.saturating_mul(2)];
        let mut reports = Vec::with_capacity(frame_sequence.len());
        {
            let controls = self.clone_mixer_controls_for_render("render_probe_sequence");
            let output_gain = self
                .session_output_gain
                .lock()
                .map(|gain| *gain)
                .unwrap_or(1.0);
            let mut mixer = self
                .mixer
                .lock()
                .map_err(|_| FlutzError::Runtime("mixer lock is poisoned".to_owned()))?;
            let mut engine = engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?;
            let midi_strips = self.midi_strips.lock().map_err(|_| {
                FlutzError::Runtime("MIDI strip analysis lock is poisoned".to_owned())
            })?;
            let mut scratch = self.render_scratch.lock().map_err(|_| {
                FlutzError::Runtime("playback render scratch lock is poisoned".to_owned())
            })?;
            engine.play()?;
            for frames in frame_sequence.iter().copied() {
                let samples = frames.saturating_mul(2);
                if output.len() < samples {
                    output.resize(samples, 0.0);
                }
                let output = &mut output[..samples];
                let report = render_mixed_audio_guarded(
                    &mut engine,
                    &mut mixer,
                    &mut scratch,
                    &controls,
                    output_gain,
                    &self.loaded_soundfonts,
                    &midi_strips,
                    output,
                    self.render_error_trace.as_ref(),
                    RenderFailureContext {
                        source: "probe-render",
                        frames_requested: frames,
                        midi_source: self.loaded_midi.as_deref(),
                        midi_scan: &self.last_midi_scan,
                    },
                );
                let recovered_error_count = usize::from(report.is_none());
                let peak = report
                    .as_ref()
                    .map(|report| report.output_meter.peak)
                    .unwrap_or(0.0);
                let stems = scratch.blocks.clone();
                let mix_diagnostics = report
                    .as_ref()
                    .map(|report| report.strip_mix_diagnostics.clone())
                    .unwrap_or_default();
                if let Some(report) = report {
                    if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
                        snapshot_history.record(
                            self.rendered_frame_clock
                                .load(Ordering::Relaxed)
                                .saturating_add(report.frames as u64),
                            report.snapshot.clone(),
                            engine.sample_rate(),
                        );
                    }
                    if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
                        let frame = analyzer.ingest_interleaved_stereo(
                            output,
                            report.frames as f32 / self.audio_config.sample_rate.max(1) as f32,
                        );
                        self.record_analyzer_trace(&frame);
                    }
                    self.rendered_frame_clock
                        .fetch_add(report.frames as u64, Ordering::Relaxed);
                } else if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
                    let frame = analyzer.advance_silence(
                        output.len() / 2,
                        (output.len() / 2) as f32 / self.audio_config.sample_rate.max(1) as f32,
                    );
                    self.record_analyzer_trace(&frame);
                }
                reports.push(RenderStemProbeReport {
                    report: RenderProbeReport {
                        frames,
                        samples: output.len(),
                        peak,
                        soundfont_count: self.loaded_soundfonts.len(),
                        midi_strip_count: midi_strips.len(),
                        recovered_error_count,
                    },
                    stems,
                    mix_diagnostics,
                });
            }
            if stop_after {
                engine.stop();
                mixer.reset_state_and_release_scratch();
                memory_runtime::decay_idle_reuse_preserving(false);
            }
        }
        Ok(reports)
    }

    pub(super) fn capture_render_layer_transition(
        &self,
        midi_bytes: &[u8],
        resolved_soundfonts: &[String],
    ) -> RenderLayerTransition {
        let same_midi = self
            .loaded_midi_bytes
            .as_deref()
            .is_some_and(|loaded| loaded == midi_bytes);
        let soundfont_set_changed = self.loaded_soundfonts != resolved_soundfonts;
        let Some(engine) = &self.engine else {
            return RenderLayerTransition::default();
        };
        if !same_midi || !soundfont_set_changed {
            return RenderLayerTransition::default();
        }
        match engine.lock() {
            Ok(engine) => RenderLayerTransition {
                warmup_tick: Some(engine.position_ticks()),
            },
            Err(_) => RenderLayerTransition::default(),
        }
    }

    pub(super) fn finish_render_layer_transition(
        &mut self,
        transition: RenderLayerTransition,
    ) -> Result<()> {
        let Some(warmup_tick) = transition.warmup_tick else {
            return Ok(());
        };
        if let Some(engine) = &self.engine {
            engine
                .lock()
                .map_err(|_| FlutzError::Runtime("playback engine lock is poisoned".to_owned()))?
                .seek_to_tick(warmup_tick)?;
        }
        self.reset_audible_output_after_reposition(false);
        Ok(())
    }

    pub(super) fn reset_audible_output_after_reposition(&mut self, resume_audio: bool) {
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio = None;
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.reset_state_and_release_scratch();
        }
        memory_runtime::decay_idle_reuse_preserving(!matches!(
            self.engine_state(),
            Some(PlaybackState::Stopped) | None
        ));
        if let Ok(mut snapshot_history) = self.latest_snapshot.lock() {
            snapshot_history.clear();
        }
        self.rendered_frame_clock.store(0, Ordering::Relaxed);
        self.reset_flux_guard();
        if resume_audio {
            if let Some(audio) = &mut self.audio {
                if let Err(error) = audio.resume() {
                    self.audio = None;
                    self.audio_error = Some(format!("audio resume failed: {error}"));
                }
            }
        }
    }
}
