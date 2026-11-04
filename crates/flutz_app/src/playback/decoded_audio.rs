use super::*;

mod state;
pub(in crate::playback) use state::{render_decoded_audio_stereo, DecodedAudioPlaybackState};
pub use state::{
    AudioPlaybackStatus, DecodedAudioTransportMetadata, DecodedRenderProbeReport,
    DecodedStreamCacheDebug,
};

impl PlaybackController {
    pub fn load_decoded_audio_buffer(
        &mut self,
        path: PathBuf,
        format_id: impl Into<String>,
        friendly_name: impl Into<String>,
        buffer: DecodedAudioBuffer,
        peq: Option<PeqPresetFile>,
    ) -> Result<String> {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::PlaybackLoad);
        if buffer.summary.channels == 0 || buffer.summary.sample_rate == 0 {
            return Err(FlutzError::UnsupportedFormat(
                "decoded audio stream has no usable sample rate or channel layout".to_owned(),
            ));
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        memory_runtime::record_audio_working_set();
        self.audio = None;
        self.audio_error = None;
        self.engine = None;
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

        self.audio_config = AudioDeviceConfig {
            sample_rate: buffer.summary.sample_rate,
            channels: 2,
            internal_block_frames: PlaybackConfig::default().block_frames as u16,
            ..AudioDeviceConfig::default()
        };
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            *analyzer = VisualizerAnalyzer::new(VisualizerAnalyzerConfig::with_sample_rate(
                self.audio_config.sample_rate,
            ));
        }

        let metadata = DecodedAudioTransportMetadata {
            path,
            format_id: format_id.into(),
            friendly_name: friendly_name.into(),
            sample_rate: buffer.summary.sample_rate,
            channels: buffer.summary.channels,
            frame_length: buffer.summary.frames_decoded,
            duration_seconds: if buffer.summary.sample_rate == 0 {
                0.0
            } else {
                buffer.summary.frames_decoded as f64 / buffer.summary.sample_rate as f64
            },
            content_kind: ContentKind::DecodedAudio,
            mastering: MasteringCapability::DecodedAudioPeq,
        };
        let summary = format!(
            "Loaded {} decoded audio, {:.2}s, {} Hz, {} channel(s)",
            metadata.friendly_name,
            metadata.duration_seconds,
            metadata.sample_rate,
            metadata.channels,
        );
        self.decoded_audio = Some(Arc::new(Mutex::new(DecodedAudioPlaybackState::new(
            metadata,
            buffer.samples,
            peq,
            PlaybackConfig::default().block_frames,
        )?)));
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
        self.soundfont_cache.release_unused();
        self.render_scratch
            .lock()
            .map_err(|_| {
                FlutzError::Runtime("playback render scratch lock is poisoned".to_owned())
            })?
            .release_capacity();
        memory_runtime::decay_idle_reuse_preserving(false);
        memory_runtime::release_after_content_change();
        Ok(summary)
    }

    pub fn load_decoded_audio_stream(
        &mut self,
        path: PathBuf,
        format_id: impl Into<String>,
        friendly_name: impl Into<String>,
        source: DecodedAudioStreamSource,
        peq: Option<PeqPresetFile>,
    ) -> Result<String> {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::PlaybackLoad);
        let format_id = format_id.into();
        let friendly_name = friendly_name.into();
        let session = DecodedAudioStreamSession::open(source, &format_id, Default::default())?;
        let stream_metadata = session.metadata().clone();
        if stream_metadata.channels == 0 || stream_metadata.sample_rate == 0 {
            return Err(FlutzError::UnsupportedFormat(
                "decoded audio stream has no usable sample rate or channel layout".to_owned(),
            ));
        }
        if let Some(audio) = &mut self.audio {
            if let Err(error) = audio.pause() {
                self.audio_error = Some(format!("audio pause failed: {error}"));
            }
        }
        memory_runtime::record_audio_working_set();
        self.audio = None;
        self.audio_error = None;
        self.engine = None;
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

        self.audio_config = AudioDeviceConfig {
            sample_rate: stream_metadata.sample_rate,
            channels: 2,
            internal_block_frames: PlaybackConfig::default().block_frames as u16,
            ..AudioDeviceConfig::default()
        };
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            *analyzer = VisualizerAnalyzer::new(VisualizerAnalyzerConfig::with_sample_rate(
                self.audio_config.sample_rate,
            ));
        }

        let frame_length = stream_metadata.frame_length.unwrap_or(0);
        let duration_seconds = stream_metadata
            .duration_seconds
            .unwrap_or_else(|| frame_length as f64 / stream_metadata.sample_rate.max(1) as f64);
        let metadata = DecodedAudioTransportMetadata {
            path,
            format_id,
            friendly_name,
            sample_rate: stream_metadata.sample_rate,
            channels: stream_metadata.channels,
            frame_length,
            duration_seconds,
            content_kind: ContentKind::DecodedAudio,
            mastering: MasteringCapability::DecodedAudioPeq,
        };
        let summary = format!(
            "Loaded {} decoded audio stream, {:.2}s, {} Hz, {} channel(s)",
            metadata.friendly_name,
            metadata.duration_seconds,
            metadata.sample_rate,
            metadata.channels,
        );
        self.decoded_audio = Some(Arc::new(Mutex::new(
            DecodedAudioPlaybackState::new_streaming(
                metadata,
                session,
                peq,
                PlaybackConfig::default().block_frames,
            )?,
        )));
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
        self.soundfont_cache.release_unused();
        self.render_scratch
            .lock()
            .map_err(|_| {
                FlutzError::Runtime("playback render scratch lock is poisoned".to_owned())
            })?
            .release_capacity();
        memory_runtime::decay_idle_reuse_preserving(false);
        memory_runtime::release_after_content_change();
        Ok(summary)
    }

    pub fn decoded_transport_metadata(&self) -> Option<DecodedAudioTransportMetadata> {
        self.decoded_audio
            .as_ref()
            .and_then(|decoded| decoded.lock().ok().map(|decoded| decoded.metadata.clone()))
    }

    pub fn decoded_stream_cache_debug(&self) -> Option<DecodedStreamCacheDebug> {
        self.decoded_audio.as_ref().and_then(|decoded| {
            decoded
                .lock()
                .ok()
                .and_then(|decoded| decoded.stream_cache_debug())
        })
    }

    pub fn wait_for_decoded_stream_cache(&self, timeout: Duration) -> bool {
        self.decoded_audio
            .as_ref()
            .and_then(|decoded| {
                decoded
                    .lock()
                    .ok()
                    .map(|decoded| decoded.wait_for_stream_cache(timeout))
            })
            .unwrap_or(false)
    }

    pub fn set_decoded_peq_config(&mut self, config: PeqConfig) -> Result<u64> {
        let prepared = PeqProcessor::prepare_config(config).map_err(|error| {
            FlutzError::InvalidInput(format!("invalid decoded audio PEQ config: {error}"))
        })?;
        let decoded = self.decoded_audio.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load decoded audio before editing PEQ".to_owned())
        })?;
        decoded
            .lock()
            .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?
            .set_peq_prepared_config(prepared)
    }

    pub fn decoded_render_probe(&mut self, frames: usize) -> Result<DecodedRenderProbeReport> {
        let decoded = self.decoded_audio.as_ref().ok_or_else(|| {
            FlutzError::InvalidInput("load decoded audio before probing render".to_owned())
        })?;
        let mut output = vec![0.0f32; frames.saturating_mul(2)];
        let output_gain = self
            .session_output_gain
            .lock()
            .map(|gain| *gain)
            .unwrap_or(1.0);
        let mut decoded = decoded
            .lock()
            .map_err(|_| FlutzError::Runtime("decoded audio lock is poisoned".to_owned()))?;
        decoded.play();
        let retained_before = decoded.retained_scratch_bytes();
        let meter = render_decoded_audio_stereo(&mut decoded, &mut output, output_gain)?;
        let retained_after = decoded.retained_scratch_bytes();
        let peq_generation = decoded.peq_generation();
        drop(decoded);
        if let Ok(mut analyzer) = self.visualizer_analyzer.lock() {
            let frame = analyzer.ingest_interleaved_stereo(
                &output,
                frames as f32 / self.audio_config.sample_rate.max(1) as f32,
            );
            self.record_analyzer_trace(&frame);
        }
        Ok(DecodedRenderProbeReport {
            frames,
            samples: output.len(),
            peak: meter.peak,
            rms: meter.rms,
            peq_generation,
            scratch_growth_bytes: retained_after.saturating_sub(retained_before),
        })
    }
}
