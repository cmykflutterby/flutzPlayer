use super::super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioPlaybackStatus {
    Audible,
    AudioUnavailable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudioTransportMetadata {
    pub path: PathBuf,
    pub format_id: String,
    pub friendly_name: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub frame_length: u64,
    pub duration_seconds: f64,
    pub content_kind: ContentKind,
    pub mastering: MasteringCapability,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedRenderProbeReport {
    pub frames: usize,
    pub samples: usize,
    pub peak: f32,
    pub rms: f32,
    pub peq_generation: u64,
    pub scratch_growth_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedStreamCacheDebug {
    pub streaming: bool,
    pub cache_start_frame: u64,
    pub cache_frames: usize,
    pub cache_capacity_frames: usize,
    pub source_channels: usize,
    pub cached_sample_capacity: usize,
    pub full_sample_len: usize,
    pub full_sample_capacity: usize,
    pub request_generation: u64,
    pub filled_generation: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
enum DecodedPlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

const DECODED_STREAM_PLAYAHEAD_SECONDS: usize = 10;

#[derive(Debug)]
pub(in crate::playback) struct DecodedAudioPlaybackState {
    pub(in crate::playback) metadata: DecodedAudioTransportMetadata,
    samples: Option<Vec<f32>>,
    stream: Option<DecodedStreamCache>,
    peq: PeqProcessor,
    peq_input: Vec<f32>,
    pub(in crate::playback) position_frame: u64,
    state: DecodedPlaybackState,
    loop_settings: PlaybackLoopSettings,
    counted_loops_remaining: u32,
}

#[derive(Debug)]
struct DecodedStreamCache {
    shared: Arc<(Mutex<DecodedStreamCacheShared>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Debug)]
struct DecodedStreamCacheShared {
    cache_start_frame: u64,
    cache_frames: usize,
    cache_capacity_frames: usize,
    source_channels: usize,
    samples: Vec<f32>,
    request_start_frame: u64,
    request_generation: u64,
    filled_generation: u64,
    shutdown: bool,
    last_error: Option<String>,
}

impl Drop for DecodedStreamCache {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.shared;
        if let Ok(mut shared) = lock.lock() {
            shared.shutdown = true;
            shared.request_generation = shared.request_generation.saturating_add(1);
            condvar.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl DecodedAudioPlaybackState {
    pub(in crate::playback) fn new(
        metadata: DecodedAudioTransportMetadata,
        samples: Vec<f32>,
        peq: Option<PeqPresetFile>,
        preallocated_frames: usize,
    ) -> Result<Self> {
        let peq_config = decoded_peq_config(peq, metadata.sample_rate, 2);
        let peq = PeqProcessor::from_config(peq_config).map_err(|error| {
            FlutzError::InvalidInput(format!("invalid decoded audio PEQ config: {error}"))
        })?;
        Ok(Self {
            metadata,
            samples: Some(samples),
            stream: None,
            peq,
            peq_input: vec![0.0; preallocated_frames.saturating_mul(2)],
            position_frame: 0,
            state: DecodedPlaybackState::Stopped,
            loop_settings: PlaybackLoopSettings::default(),
            counted_loops_remaining: 0,
        })
    }

    pub(in crate::playback) fn new_streaming(
        metadata: DecodedAudioTransportMetadata,
        session: DecodedAudioStreamSession,
        peq: Option<PeqPresetFile>,
        preallocated_frames: usize,
    ) -> Result<Self> {
        let peq_config = decoded_peq_config(peq, metadata.sample_rate, 2);
        let peq = PeqProcessor::from_config(peq_config).map_err(|error| {
            FlutzError::InvalidInput(format!("invalid decoded audio PEQ config: {error}"))
        })?;
        let source_channels = metadata.channels.max(1);
        let cache_capacity_frames = (metadata.sample_rate as usize)
            .saturating_mul(DECODED_STREAM_PLAYAHEAD_SECONDS)
            .max(preallocated_frames.saturating_mul(4));
        let shared = Arc::new((
            Mutex::new(DecodedStreamCacheShared {
                cache_start_frame: 0,
                cache_frames: 0,
                cache_capacity_frames,
                source_channels,
                samples: Vec::with_capacity(cache_capacity_frames.saturating_mul(source_channels)),
                request_start_frame: 0,
                request_generation: 1,
                filled_generation: 0,
                shutdown: false,
                last_error: None,
            }),
            Condvar::new(),
        ));
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("flutz-decoded-stream".to_owned())
            .spawn(move || decoded_stream_worker(session, worker_shared))
            .map_err(|error| {
                FlutzError::Runtime(format!("failed to start decoded stream worker: {error}"))
            })?;
        Ok(Self {
            metadata,
            samples: None,
            stream: Some(DecodedStreamCache {
                shared,
                worker: Some(worker),
            }),
            peq,
            peq_input: vec![0.0; preallocated_frames.saturating_mul(2)],
            position_frame: 0,
            state: DecodedPlaybackState::Stopped,
            loop_settings: PlaybackLoopSettings::default(),
            counted_loops_remaining: 0,
        })
    }

    pub(in crate::playback) fn play(&mut self) {
        if self.position_frame >= self.metadata.frame_length {
            self.position_frame = 0;
        }
        if matches!(self.state, DecodedPlaybackState::Stopped) {
            self.counted_loops_remaining = self.loop_settings.loop_count.max(1);
        }
        self.request_stream_cache_fill(self.position_frame);
        self.state = DecodedPlaybackState::Playing;
    }

    pub(in crate::playback) fn pause(&mut self) {
        if matches!(self.state, DecodedPlaybackState::Playing) {
            self.state = DecodedPlaybackState::Paused;
        }
    }

    pub(in crate::playback) fn stop(&mut self) {
        self.state = DecodedPlaybackState::Stopped;
        self.position_frame = 0;
        self.counted_loops_remaining = self.loop_settings.loop_count.max(1);
    }

    pub(in crate::playback) fn is_playing(&self) -> bool {
        matches!(self.state, DecodedPlaybackState::Playing)
    }

    /// Pre-touches the decoded samples (full buffer or stream cache) and PEQ scratch.
    pub(in crate::playback) fn touch_resident_pages(&self) -> u64 {
        let mut pages = touch_pages(&self.peq_input);
        if let Some(samples) = &self.samples {
            pages += touch_pages(samples);
        }
        if let Some(stream) = &self.stream {
            if let Ok(shared) = stream.shared.0.lock() {
                pages += touch_pages(&shared.samples);
            }
        }
        pages
    }

    pub(in crate::playback) fn state_label(&self) -> &'static str {
        match self.state {
            DecodedPlaybackState::Stopped => "Stopped",
            DecodedPlaybackState::Playing => "Playing",
            DecodedPlaybackState::Paused => "Paused",
        }
    }

    pub(in crate::playback) fn position_seconds(&self) -> f64 {
        if self.metadata.sample_rate == 0 {
            return 0.0;
        }
        self.position_frame as f64 / self.metadata.sample_rate as f64
    }

    pub(in crate::playback) fn transport_fraction(&self) -> f32 {
        if self.metadata.frame_length == 0 {
            return 0.0;
        }
        (self.position_frame as f64 / self.metadata.frame_length as f64).clamp(0.0, 1.0) as f32
    }

    pub(in crate::playback) fn seek_fraction(&mut self, fraction: f32) {
        let target = (self.metadata.frame_length as f64 * f64::from(fraction.clamp(0.0, 1.0)))
            .round() as u64;
        self.seek_frame(target);
    }

    pub(in crate::playback) fn seek_seconds(&mut self, seconds: f64) {
        let target = (seconds.max(0.0) * self.metadata.sample_rate.max(1) as f64).round() as u64;
        self.seek_frame(target);
    }

    pub(in crate::playback) fn seek_frame(&mut self, frame: u64) {
        self.position_frame = frame.min(self.metadata.frame_length);
        self.counted_loops_remaining = self.loop_settings.loop_count.max(1);
        self.request_stream_cache_fill(self.position_frame);
    }

    pub(in crate::playback) fn set_loop_enabled(&mut self, enabled: bool) {
        self.loop_settings.enabled = enabled;
        if enabled && matches!(self.loop_settings.mode, PlaybackLoopMode::None) {
            self.loop_settings.mode = PlaybackLoopMode::Infinite;
        }
    }

    pub(in crate::playback) fn set_loop_settings(&mut self, mut settings: PlaybackLoopSettings) {
        settings.start_tick = settings.start_tick.min(self.metadata.frame_length);
        settings.end_tick = settings.end_tick.min(self.metadata.frame_length);
        settings.loop_count = settings.loop_count.max(1);
        if settings.end_tick <= settings.start_tick {
            settings.enabled = false;
        }
        self.loop_settings = settings;
        self.counted_loops_remaining = self.loop_settings.loop_count;
    }

    pub(in crate::playback) fn next_source_frame(&mut self) -> Option<u64> {
        if !self.is_playing() || self.metadata.frame_length == 0 {
            return None;
        }
        if self.position_frame >= self.effective_play_end_frame() && !self.apply_loop_boundary() {
            self.state = DecodedPlaybackState::Stopped;
            return None;
        }
        let frame = self.position_frame;
        self.position_frame = self.position_frame.saturating_add(1);
        Some(frame)
    }

    fn effective_play_end_frame(&self) -> u64 {
        if self.loop_settings.enabled && self.loop_settings.end_tick > self.loop_settings.start_tick
        {
            self.loop_settings.end_tick.min(self.metadata.frame_length)
        } else {
            self.metadata.frame_length
        }
    }

    fn apply_loop_boundary(&mut self) -> bool {
        if !self.loop_settings.enabled
            || self.loop_settings.end_tick <= self.loop_settings.start_tick
        {
            return false;
        }
        match self.loop_settings.mode {
            PlaybackLoopMode::Infinite => {
                self.position_frame = self.loop_settings.start_tick;
                self.request_stream_cache_fill(self.position_frame);
                true
            }
            PlaybackLoopMode::Counted if self.counted_loops_remaining > 0 => {
                self.counted_loops_remaining = self.counted_loops_remaining.saturating_sub(1);
                self.position_frame = self.loop_settings.start_tick;
                self.request_stream_cache_fill(self.position_frame);
                true
            }
            _ => false,
        }
    }

    pub(in crate::playback) fn retained_scratch_bytes(&self) -> usize {
        let stream_bytes = self
            .stream
            .as_ref()
            .and_then(|stream| {
                stream.shared.0.lock().ok().map(|shared| {
                    shared
                        .samples
                        .capacity()
                        .saturating_mul(std::mem::size_of::<f32>())
                })
            })
            .unwrap_or_default();
        let full_sample_bytes = self
            .samples
            .as_ref()
            .map(|samples| {
                samples
                    .capacity()
                    .saturating_mul(std::mem::size_of::<f32>())
            })
            .unwrap_or_default();
        self.peq_input
            .capacity()
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_add(self.peq.metrics().retained_state_bytes)
            .saturating_add(stream_bytes)
            .saturating_add(full_sample_bytes)
    }

    pub(in crate::playback) fn peq_generation(&self) -> u64 {
        self.peq.metrics().active_config_generation
    }

    pub(in crate::playback) fn set_peq_prepared_config(
        &mut self,
        prepared: PreparedConfig,
    ) -> Result<u64> {
        self.peq.set_prepared_config(prepared).map_err(|error| {
            FlutzError::InvalidInput(format!("invalid decoded audio PEQ config: {error}"))
        })?;
        Ok(self
            .peq
            .metrics()
            .pending_config_generation
            .unwrap_or_else(|| self.peq.metrics().active_config_generation))
    }

    pub(in crate::playback) fn stream_cache_debug(&self) -> Option<DecodedStreamCacheDebug> {
        let Some(stream) = &self.stream else {
            return Some(DecodedStreamCacheDebug {
                streaming: false,
                cache_start_frame: 0,
                cache_frames: 0,
                cache_capacity_frames: 0,
                source_channels: self.metadata.channels,
                cached_sample_capacity: 0,
                full_sample_len: self.samples.as_ref().map(Vec::len).unwrap_or_default(),
                full_sample_capacity: self.samples.as_ref().map(Vec::capacity).unwrap_or_default(),
                request_generation: 0,
                filled_generation: 0,
                last_error: None,
            });
        };
        let shared = stream.shared.0.lock().ok()?;
        Some(DecodedStreamCacheDebug {
            streaming: true,
            cache_start_frame: shared.cache_start_frame,
            cache_frames: shared.cache_frames,
            cache_capacity_frames: shared.cache_capacity_frames,
            source_channels: shared.source_channels,
            cached_sample_capacity: shared.samples.capacity(),
            full_sample_len: 0,
            full_sample_capacity: 0,
            request_generation: shared.request_generation,
            filled_generation: shared.filled_generation,
            last_error: shared.last_error.clone(),
        })
    }

    pub(in crate::playback) fn wait_for_stream_cache(&self, timeout: Duration) -> bool {
        let Some(stream) = &self.stream else {
            return true;
        };
        let target_frame = self.position_frame;
        let (lock, condvar) = &*stream.shared;
        let Ok(shared) = lock.lock() else {
            return false;
        };
        if frame_in_cache(&shared, target_frame) {
            return true;
        }
        match condvar.wait_timeout_while(shared, timeout, |shared| {
            !shared.shutdown && shared.last_error.is_none() && !frame_in_cache(shared, target_frame)
        }) {
            Ok((shared, _)) => frame_in_cache(&shared, target_frame),
            Err(_) => false,
        }
    }

    fn request_stream_cache_fill(&self, frame: u64) {
        let Some(stream) = &self.stream else {
            return;
        };
        let (lock, condvar) = &*stream.shared;
        if let Ok(mut shared) = lock.lock() {
            if frame_in_cache(&shared, frame) && frame_in_cache(&shared, frame.saturating_add(1)) {
                return;
            }
            if shared.request_start_frame == frame
                && shared.request_generation != shared.filled_generation
            {
                return;
            }
            shared.request_start_frame = frame;
            shared.request_generation = shared.request_generation.saturating_add(1);
            condvar.notify_all();
        }
    }

    fn request_stream_cache_prefetch(&self, rendered_frames: usize) {
        let Some(stream) = &self.stream else {
            return;
        };
        let threshold_frames = rendered_frames
            .saturating_mul(4)
            .max((self.metadata.sample_rate as usize / 2).max(1));
        let (lock, condvar) = &*stream.shared;
        if let Ok(mut shared) = lock.lock() {
            let cache_end = shared
                .cache_start_frame
                .saturating_add(shared.cache_frames as u64);
            let position = self.position_frame.min(self.metadata.frame_length);
            let remaining = cache_end.saturating_sub(position);
            if position >= shared.cache_start_frame
                && position < cache_end
                && remaining > threshold_frames as u64
            {
                return;
            }
            if shared.request_start_frame == position
                && shared.request_generation != shared.filled_generation
            {
                return;
            }
            shared.request_start_frame = position;
            shared.request_generation = shared.request_generation.saturating_add(1);
            condvar.notify_all();
        }
    }
}

pub(in crate::playback) fn render_decoded_audio_stereo(
    decoded: &mut DecodedAudioPlaybackState,
    output: &mut [f32],
    output_gain: f32,
) -> Result<MeterReading> {
    if decoded.peq_input.len() < output.len() {
        decoded.peq_input.resize(output.len(), 0.0);
    }
    let source_channels = decoded.metadata.channels.max(1);
    let output_frames = output.len() / 2;
    for frame_index in 0..output_frames {
        let output_index = frame_index * 2;
        let Some(source_frame) = decoded.next_source_frame() else {
            decoded.peq_input[output_index] = 0.0;
            decoded.peq_input[output_index + 1] = 0.0;
            continue;
        };
        let (left, right) = decoded_source_stereo_frame(decoded, source_frame, source_channels);
        decoded.peq_input[output_index] = left;
        decoded.peq_input[output_index + 1] = right;
    }
    decoded
        .peq
        .process_interleaved(&decoded.peq_input[..output.len()], output)
        .map_err(|error| FlutzError::Runtime(format!("decoded audio PEQ failed: {error}")))?;
    for sample in output.iter_mut() {
        *sample *= output_gain;
    }
    decoded.request_stream_cache_prefetch(output_frames);
    Ok(MeterReading::from_interleaved(output))
}

fn decoded_source_stereo_frame(
    decoded: &DecodedAudioPlaybackState,
    source_frame: u64,
    source_channels: usize,
) -> (f32, f32) {
    if let Some(samples) = decoded.samples.as_ref() {
        let source_index = source_frame as usize * source_channels;
        if source_index >= samples.len() {
            return (0.0, 0.0);
        }
        let left = samples[source_index];
        let right = if source_channels == 1 {
            left
        } else {
            samples.get(source_index + 1).copied().unwrap_or(left)
        };
        return (left, right);
    }

    let Some(stream) = decoded.stream.as_ref() else {
        return (0.0, 0.0);
    };
    let (lock, condvar) = &*stream.shared;
    let Ok(mut shared) = lock.lock() else {
        return (0.0, 0.0);
    };
    if !frame_in_cache(&shared, source_frame) {
        if shared.request_start_frame != source_frame
            || shared.request_generation == shared.filled_generation
        {
            shared.request_start_frame = source_frame;
            shared.request_generation = shared.request_generation.saturating_add(1);
            condvar.notify_all();
        }
        return (0.0, 0.0);
    }
    let frame_offset = source_frame.saturating_sub(shared.cache_start_frame) as usize;
    let source_index = frame_offset.saturating_mul(shared.source_channels.max(1));
    let left = shared.samples.get(source_index).copied().unwrap_or(0.0);
    let right = if shared.source_channels == 1 {
        left
    } else {
        shared
            .samples
            .get(source_index + 1)
            .copied()
            .unwrap_or(left)
    };
    (left, right)
}

fn frame_in_cache(shared: &DecodedStreamCacheShared, frame: u64) -> bool {
    let cache_end = shared
        .cache_start_frame
        .saturating_add(shared.cache_frames as u64);
    frame >= shared.cache_start_frame && frame < cache_end
}

fn decoded_stream_worker(
    mut session: DecodedAudioStreamSession,
    shared: Arc<(Mutex<DecodedStreamCacheShared>, Condvar)>,
) {
    let mut decode_buffer = Vec::<f32>::new();
    loop {
        let (request_start, request_generation, cache_capacity_frames) = {
            let (lock, condvar) = &*shared;
            let mut state = match lock.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            while !state.shutdown && state.request_generation == state.filled_generation {
                state = condvar
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if state.shutdown {
                return;
            }
            (
                state.request_start_frame,
                state.request_generation,
                state.cache_capacity_frames,
            )
        };

        let result = (|| -> Result<usize> {
            session.seek_frame(request_start)?;
            let window = session.decode_next_frames(cache_capacity_frames, &mut decode_buffer)?;
            Ok(window.frames_decoded)
        })();

        let (lock, condvar) = &*shared;
        let mut state = match lock.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.shutdown {
            return;
        }
        match result {
            Ok(frames) => {
                state.samples.clear();
                state.samples.extend_from_slice(&decode_buffer);
                state.cache_start_frame = request_start;
                state.cache_frames = frames;
                state.last_error = None;
            }
            Err(error) => {
                state.samples.clear();
                state.cache_start_frame = request_start;
                state.cache_frames = 0;
                state.last_error = Some(format!("{error}"));
            }
        }
        state.filled_generation = request_generation;
        condvar.notify_all();
    }
}

fn decoded_peq_config(peq: Option<PeqPresetFile>, sample_rate: u32, channels: u16) -> PeqConfig {
    let mut config = peq.map(|preset| preset.config).unwrap_or_default();
    config.sample_rate_hz = sample_rate.max(1);
    config.channel_count = channels.max(1);
    config.channel_layout = ChannelLayout::Interleaved;
    config
}
