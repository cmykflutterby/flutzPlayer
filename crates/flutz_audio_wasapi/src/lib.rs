use std::{
    sync::atomic::{AtomicU64, Ordering},
    sync::{mpsc, Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

pub use flutz_audio_sdl3::{
    AudioCallbackCounters, AudioCallbackStats, AudioDeviceConfig, AudioDeviceDiagnostics,
    AudioDeviceState, AudioFormatDiagnostics, AudioMemoryHooks, AudioReferenceDiagnostics,
    AudioRingBuffer, AudioSdlLifecycleStats, RenderAheadState, RenderAheadTarget, UnderrunReport,
};
use wasapi::{
    deinitialize, initialize_mta, DeviceEnumerator, Direction, SampleType, ShareMode, StreamMode,
    WaveFormat,
};

static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);
static COM_INIT_CALLS: AtomicU64 = AtomicU64::new(0);
static COM_UNINIT_CALLS: AtomicU64 = AtomicU64::new(0);
static STREAM_OPENS: AtomicU64 = AtomicU64::new(0);
static STREAM_DESTROYS: AtomicU64 = AtomicU64::new(0);
static STREAM_RESUMES: AtomicU64 = AtomicU64::new(0);
static STREAM_PAUSES: AtomicU64 = AtomicU64::new(0);
static CALLBACK_STATES_CREATED: AtomicU64 = AtomicU64::new(0);
static CALLBACK_STATES_DROPPED: AtomicU64 = AtomicU64::new(0);
static PRODUCER_THREADS_STARTED: AtomicU64 = AtomicU64::new(0);
static PRODUCER_THREADS_FINISHED: AtomicU64 = AtomicU64::new(0);

pub struct WasapiAudioOutput {
    producer: Option<JoinHandle<()>>,
    shared: Arc<SharedWasapiState>,
    config: AudioDeviceConfig,
}

impl WasapiAudioOutput {
    pub fn open_f32_stream(
        config: AudioDeviceConfig,
        renderer: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> Result<Self, String> {
        Self::open_f32_stream_with_memory_hooks(config, AudioMemoryHooks::default(), renderer)
    }

    pub fn open_f32_stream_with_memory_hooks(
        config: AudioDeviceConfig,
        memory_hooks: AudioMemoryHooks,
        renderer: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> Result<Self, String> {
        if config.channels != 2 {
            return Err("WASAPI output currently expects stereo f32 audio".to_owned());
        }

        let stream_id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(SharedWasapiState {
            stream_id,
            lifecycle: Mutex::new(WasapiLifecycle::default()),
            lifecycle_changed: Condvar::new(),
            state: Mutex::new(AudioDeviceState::Open),
            diagnostics: Mutex::new(WasapiRuntimeDiagnostics::default()),
            render_ahead: Mutex::new(AdaptiveRenderAheadRuntime::default_for_capacity(
                config.ring_buffer.capacity_frames,
            )),
            stats: AudioCallbackCounters::default(),
        });
        CALLBACK_STATES_CREATED.fetch_add(1, Ordering::Relaxed);

        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let producer_shared = Arc::clone(&shared);
        let producer = thread::spawn(move || {
            run_wasapi_thread(
                producer_shared,
                config,
                memory_hooks,
                renderer,
                ready_sender,
            );
        });

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                producer: Some(producer),
                shared,
                config,
            }),
            Ok(Err(error)) => {
                signal_shutdown(&shared);
                let _ = producer.join();
                CALLBACK_STATES_DROPPED.fetch_add(1, Ordering::Relaxed);
                Err(error)
            }
            Err(error) => {
                signal_shutdown(&shared);
                let _ = producer.join();
                CALLBACK_STATES_DROPPED.fetch_add(1, Ordering::Relaxed);
                Err(format!(
                    "WASAPI backend did not report startup status: {error}"
                ))
            }
        }
    }

    pub fn resume(&mut self) -> Result<(), String> {
        set_running(&self.shared, true);
        STREAM_RESUMES.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), String> {
        set_running(&self.shared, false);
        STREAM_PAUSES.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn state(&self) -> AudioDeviceState {
        self.shared
            .state
            .lock()
            .map(|state| *state)
            .unwrap_or(AudioDeviceState::Closed)
    }

    pub fn config(&self) -> AudioDeviceConfig {
        self.config
    }

    pub fn stats(&self) -> AudioCallbackStats {
        self.shared.stats.snapshot()
    }

    pub fn underrun_report(&self) -> UnderrunReport {
        let stats = self.stats();
        UnderrunReport::from_counts(
            stats.underrun_count,
            stats.frames_requested,
            stats.frames_delivered,
        )
    }

    pub fn diagnostics(&self) -> AudioDeviceDiagnostics {
        let stats = self.stats();
        let runtime = self
            .shared
            .diagnostics
            .lock()
            .map(|diagnostics| diagnostics.clone())
            .unwrap_or_default();
        let shutdown_started = self
            .shared
            .lifecycle
            .lock()
            .map(|lifecycle| lifecycle.shutdown)
            .unwrap_or(true);
        let mut diagnostics = AudioDeviceDiagnostics::from_config(
            self.config,
            self.state(),
            runtime.ring_capacity_frames,
            runtime.ring_available_frames,
            runtime.ring_retained_bytes,
            runtime.render_block_bytes as usize,
            shutdown_started,
            stats,
            wasapi_lifecycle_snapshot(),
            AudioReferenceDiagnostics {
                stream_id: self.shared.stream_id,
                shared_strong_count: Arc::strong_count(&self.shared),
                shared_weak_count: Arc::weak_count(&self.shared),
                callback_shared_strong_count: Arc::strong_count(&self.shared),
                callback_shared_weak_count: Arc::weak_count(&self.shared),
            },
        );
        diagnostics.opened_device_name = runtime.opened_device_name;
        diagnostics.stream_input_format = Some(AudioFormatDiagnostics::f32_stereo(
            self.config.sample_rate,
            self.config.channels,
        ));
        diagnostics.stream_output_format = runtime.output_format;
        diagnostics.device_buffer_frames = runtime.buffer_frames as usize;
        diagnostics.device_available_frames = runtime.available_frames as usize;
        diagnostics
    }

    pub fn render_ahead_state(&self) -> RenderAheadState {
        let stats = self.stats();
        let runtime = self
            .shared
            .diagnostics
            .lock()
            .map(|diagnostics| diagnostics.clone())
            .unwrap_or_default();
        let target = self
            .shared
            .render_ahead
            .lock()
            .map(|runtime| runtime.target)
            .unwrap_or_default();
        RenderAheadState {
            effective_target_frames: target.target_frames,
            current_buffered_frames: runtime.ring_available_frames.min(u32::MAX as usize) as u32,
            available_frames: runtime.available_frames,
            largest_callback_frames: stats.largest_callback_frames.min(u64::from(u32::MAX)) as u32,
            underrun_count: stats.underrun_count,
            queue_error_count: stats.queue_error_count,
        }
    }

    pub fn apply_render_ahead_target(&mut self, target: RenderAheadTarget) -> Result<(), String> {
        let target = AdaptiveRenderAheadRuntime::clamp_target(
            target,
            self.config.ring_buffer.capacity_frames,
        );
        let mut runtime = self
            .shared
            .render_ahead
            .lock()
            .map_err(|_| "WASAPI render-ahead lock is poisoned".to_owned())?;
        runtime.target = target;
        Ok(())
    }
}

impl Drop for WasapiAudioOutput {
    fn drop(&mut self) {
        signal_shutdown(&self.shared);
        if let Some(producer) = self.producer.take() {
            let _ = producer.join();
        }
        CALLBACK_STATES_DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
struct SharedWasapiState {
    stream_id: u64,
    lifecycle: Mutex<WasapiLifecycle>,
    lifecycle_changed: Condvar,
    state: Mutex<AudioDeviceState>,
    diagnostics: Mutex<WasapiRuntimeDiagnostics>,
    render_ahead: Mutex<AdaptiveRenderAheadRuntime>,
    stats: AudioCallbackCounters,
}

#[derive(Debug, Default)]
struct WasapiLifecycle {
    running: bool,
    shutdown: bool,
}

#[derive(Debug, Clone, Default)]
struct WasapiRuntimeDiagnostics {
    opened_device_name: Option<String>,
    output_format: Option<AudioFormatDiagnostics>,
    buffer_frames: u32,
    available_frames: u32,
    ring_capacity_frames: usize,
    ring_available_frames: usize,
    ring_retained_bytes: usize,
    render_block_bytes: u64,
}

#[derive(Debug, Copy, Clone, Default)]
struct AdaptiveRenderAheadRuntime {
    target: RenderAheadTarget,
}

impl AdaptiveRenderAheadRuntime {
    fn default_for_capacity(capacity_frames: usize) -> Self {
        Self {
            target: RenderAheadTarget {
                target_frames: capacity_frames.min(u32::MAX as usize) as u32,
                low_water_frames: (capacity_frames / 2).min(u32::MAX as usize) as u32,
                high_water_frames: capacity_frames.min(u32::MAX as usize) as u32,
            },
        }
    }

    fn clamp_target(target: RenderAheadTarget, capacity_frames: usize) -> RenderAheadTarget {
        let capacity = capacity_frames.min(u32::MAX as usize) as u32;
        let block_floor = 1;
        let target_frames = target
            .target_frames
            .clamp(block_floor, capacity.max(block_floor));
        let low_water_frames = target.low_water_frames.min(target_frames);
        let high_water_frames = target
            .high_water_frames
            .max(target_frames)
            .min(capacity.max(target_frames));
        RenderAheadTarget {
            target_frames,
            low_water_frames,
            high_water_frames,
        }
    }
}

struct ProducerThreadGuard;

impl Drop for ProducerThreadGuard {
    fn drop(&mut self) {
        PRODUCER_THREADS_FINISHED.fetch_add(1, Ordering::Relaxed);
    }
}

struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        deinitialize();
        COM_UNINIT_CALLS.fetch_add(1, Ordering::Relaxed);
    }
}

fn run_wasapi_thread(
    shared: Arc<SharedWasapiState>,
    config: AudioDeviceConfig,
    memory_hooks: AudioMemoryHooks,
    mut renderer: impl FnMut(&mut [f32]) + Send + 'static,
    ready_sender: mpsc::SyncSender<Result<(), String>>,
) {
    PRODUCER_THREADS_STARTED.fetch_add(1, Ordering::Relaxed);
    let _producer_thread_guard = ProducerThreadGuard;
    let _memory_guard = memory_hooks.producer.as_ref().map(|bind| bind());
    let startup = initialize_wasapi_stream(&shared, config);
    let Ok(stream) = startup else {
        let error = startup
            .err()
            .unwrap_or_else(|| "WASAPI startup failed".to_owned());
        let _ = ready_sender.send(Err(error));
        set_state(&shared, AudioDeviceState::Closed);
        return;
    };
    let _ = ready_sender.send(Ok(()));

    let channels = config.channels as usize;
    let block_frames = config.internal_block_frames.max(1) as usize;
    let mut ring = AudioRingBuffer::new(config.ring_buffer, channels);
    let mut render_block = vec![0.0; block_frames * channels];
    let mut device_block = vec![0.0; block_frames * channels];
    let mut byte_buffer = vec![0u8; render_block.len() * std::mem::size_of::<f32>()];
    shared
        .stats
        .record_producer_render_block_bytes(byte_buffer.capacity() as u64);
    if let Ok(mut diagnostics) = shared.diagnostics.lock() {
        diagnostics.render_block_bytes = byte_buffer.capacity() as u64;
        diagnostics.ring_capacity_frames = ring.capacity_frames();
        diagnostics.ring_available_frames = ring.available_frames();
        diagnostics.ring_retained_bytes = ring.retained_bytes();
    }

    let mut started = false;
    loop {
        let mut lifecycle = match shared.lifecycle.lock() {
            Ok(lifecycle) => lifecycle,
            Err(_) => break,
        };
        while !lifecycle.running && !lifecycle.shutdown {
            if started {
                let _ = stream.audio_client.stop_stream();
                let _ = stream.audio_client.reset_stream();
                ring.clear();
                update_ring_diagnostics(&shared, &ring);
                started = false;
                set_state(&shared, AudioDeviceState::Open);
            }
            lifecycle = match shared.lifecycle_changed.wait(lifecycle) {
                Ok(lifecycle) => lifecycle,
                Err(_) => return,
            };
        }
        if lifecycle.shutdown {
            break;
        }
        drop(lifecycle);

        if !started {
            if stream.audio_client.start_stream().is_err() {
                shared.stats.record_callback(0, 0, 0, 0, 0, false, 0, 0, 0);
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            started = true;
            set_state(&shared, AudioDeviceState::Running);
        }

        fill_render_ahead_ring(
            &shared,
            &mut ring,
            channels,
            block_frames,
            &mut render_block,
            &mut renderer,
        );

        let available_frames = stream
            .audio_client
            .get_available_space_in_frames()
            .unwrap_or(0);
        if let Ok(mut diagnostics) = shared.diagnostics.lock() {
            diagnostics.available_frames = available_frames;
        }
        if available_frames == 0 {
            thread::sleep(stream.sleep_period);
            continue;
        }

        let frames = (available_frames as usize).min(block_frames).max(1);
        let samples = frames * channels;
        let render = &mut device_block[..samples];
        render.fill(0.0);
        let read_samples = ring.read(render);
        let delivered_frames = read_samples / channels;
        let missing_frames = frames.saturating_sub(delivered_frames);
        update_ring_diagnostics(&shared, &ring);
        encode_f32_bytes(render, &mut byte_buffer);
        let bytes = &byte_buffer[..samples * std::mem::size_of::<f32>()];
        let queued = stream
            .render_client
            .write_to_device(frames, bytes, None)
            .is_ok();
        shared.stats.record_callback(
            frames as u64,
            bytes.len() as u64,
            bytes.len() as u64,
            if queued { delivered_frames as u64 } else { 0 },
            missing_frames as u64,
            queued,
            1,
            bytes.len() as u64,
            ring.available_frames() as u64,
        );
    }

    if started {
        let _ = stream.audio_client.stop_stream();
        let _ = stream.audio_client.reset_stream();
    }
    set_state(&shared, AudioDeviceState::Closed);
    STREAM_DESTROYS.fetch_add(1, Ordering::Relaxed);
}

fn fill_render_ahead_ring(
    shared: &SharedWasapiState,
    ring: &mut AudioRingBuffer,
    channels: usize,
    block_frames: usize,
    render_block: &mut [f32],
    renderer: &mut impl FnMut(&mut [f32]),
) {
    let high_water_frames = shared
        .render_ahead
        .lock()
        .map(|runtime| runtime.target.high_water_frames as usize)
        .unwrap_or_else(|_| ring.capacity_frames());

    while ring.free_frames() >= block_frames && ring.available_frames() < high_water_frames {
        let samples = block_frames * channels;
        let render = &mut render_block[..samples];
        render.fill(0.0);
        renderer(render);
        let written_samples = ring.write(render);
        shared
            .stats
            .record_producer_rendered((written_samples / channels) as u64);
        if written_samples == 0 {
            break;
        }
    }
    update_ring_diagnostics(shared, ring);
}

fn update_ring_diagnostics(shared: &SharedWasapiState, ring: &AudioRingBuffer) {
    if let Ok(mut diagnostics) = shared.diagnostics.lock() {
        diagnostics.ring_capacity_frames = ring.capacity_frames();
        diagnostics.ring_available_frames = ring.available_frames();
        diagnostics.ring_retained_bytes = ring.retained_bytes();
    }
}

struct WasapiStream {
    audio_client: wasapi::AudioClient,
    render_client: wasapi::AudioRenderClient,
    sleep_period: Duration,
    _com: ComGuard,
}

fn initialize_wasapi_stream(
    shared: &SharedWasapiState,
    config: AudioDeviceConfig,
) -> Result<WasapiStream, String> {
    initialize_mta()
        .ok()
        .map_err(|error| format!("WASAPI COM init failed: {error}"))?;
    COM_INIT_CALLS.fetch_add(1, Ordering::Relaxed);
    let com = ComGuard;

    let channels = config.channels as usize;
    let enumerator = DeviceEnumerator::new()
        .map_err(|error| format!("WASAPI device enumerator failed: {error}"))?;
    let device = enumerator
        .get_default_device(&Direction::Render)
        .map_err(|error| format!("WASAPI default render device failed: {error}"))?;
    let opened_device_name = device.get_friendlyname().ok();
    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|error| format!("WASAPI audio client activation failed: {error}"))?;
    let desired_format = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        config.sample_rate as usize,
        channels,
        None,
    );
    let needs_convert = match audio_client.is_supported(&desired_format, &ShareMode::Shared) {
        Ok(None) => false,
        Ok(Some(_)) | Err(_) => true,
    };
    let (default_period, _) = audio_client
        .get_device_period()
        .map_err(|error| format!("WASAPI device period query failed: {error}"))?;
    let mode = StreamMode::PollingShared {
        autoconvert: needs_convert,
        buffer_duration_hns: default_period,
    };
    audio_client
        .initialize_client(&desired_format, &Direction::Render, &mode)
        .map_err(|error| format!("WASAPI client initialize failed: {error}"))?;
    let render_client = audio_client
        .get_audiorenderclient()
        .map_err(|error| format!("WASAPI render client query failed: {error}"))?;
    let buffer_frames = audio_client
        .get_buffer_size()
        .map_err(|error| format!("WASAPI buffer size query failed: {error}"))?;
    let sleep_period = Duration::from_millis(
        (500 * buffer_frames as u64 / config.sample_rate.max(1) as u64).clamp(1, 20),
    );

    if let Ok(mut diagnostics) = shared.diagnostics.lock() {
        diagnostics.opened_device_name = opened_device_name;
        diagnostics.output_format = Some(AudioFormatDiagnostics::f32_stereo(
            config.sample_rate,
            config.channels,
        ));
        diagnostics.buffer_frames = buffer_frames;
        diagnostics.available_frames = buffer_frames;
    }
    set_state(shared, AudioDeviceState::Open);
    STREAM_OPENS.fetch_add(1, Ordering::Relaxed);

    Ok(WasapiStream {
        audio_client,
        render_client,
        sleep_period,
        _com: com,
    })
}

fn encode_f32_bytes(samples: &[f32], output: &mut [u8]) {
    for (sample, bytes) in samples.iter().zip(output.chunks_exact_mut(4)) {
        bytes.copy_from_slice(&sample.to_le_bytes());
    }
}

fn set_running(shared: &SharedWasapiState, running: bool) {
    if let Ok(mut lifecycle) = shared.lifecycle.lock() {
        lifecycle.running = running;
        shared.lifecycle_changed.notify_all();
    }
}

fn signal_shutdown(shared: &SharedWasapiState) {
    if let Ok(mut lifecycle) = shared.lifecycle.lock() {
        lifecycle.running = false;
        lifecycle.shutdown = true;
        shared.lifecycle_changed.notify_all();
    }
}

fn set_state(shared: &SharedWasapiState, state: AudioDeviceState) {
    if let Ok(mut current) = shared.state.lock() {
        *current = state;
    }
}

fn wasapi_lifecycle_snapshot() -> AudioSdlLifecycleStats {
    AudioSdlLifecycleStats {
        init_calls: COM_INIT_CALLS.load(Ordering::Relaxed),
        quit_calls: COM_UNINIT_CALLS.load(Ordering::Relaxed),
        stream_opens: STREAM_OPENS.load(Ordering::Relaxed),
        stream_destroys: STREAM_DESTROYS.load(Ordering::Relaxed),
        stream_resumes: STREAM_RESUMES.load(Ordering::Relaxed),
        stream_pauses: STREAM_PAUSES.load(Ordering::Relaxed),
        callback_states_created: CALLBACK_STATES_CREATED.load(Ordering::Relaxed),
        callback_states_dropped: CALLBACK_STATES_DROPPED.load(Ordering::Relaxed),
        producer_threads_started: PRODUCER_THREADS_STARTED.load(Ordering::Relaxed),
        producer_threads_finished: PRODUCER_THREADS_FINISHED.load(Ordering::Relaxed),
    }
}
