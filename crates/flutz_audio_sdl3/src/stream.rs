use std::{
    cell::RefCell,
    ffi::{c_void, CStr},
    ptr::NonNull,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use sdl3_sys::everything::{
    SDL_AudioSpec, SDL_AudioStream, SDL_DestroyAudioStream, SDL_GetAudioDeviceName,
    SDL_GetAudioStreamDevice, SDL_GetAudioStreamFormat, SDL_GetError, SDL_Init,
    SDL_OpenAudioDeviceStream, SDL_PauseAudioStreamDevice, SDL_PutAudioStreamData,
    SDL_QuitSubSystem, SDL_ResumeAudioStreamDevice, SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK,
    SDL_AUDIO_F32, SDL_INIT_AUDIO,
};

use crate::{
    AudioCallbackCounters, AudioCallbackStats, AudioDeviceConfig, AudioDeviceDiagnostics,
    AudioDeviceState, AudioFormatDiagnostics, AudioMemoryHooks, AudioReferenceDiagnostics,
    AudioRingBuffer, AudioSdlLifecycleStats, RenderAheadState, RenderAheadTarget, UnderrunReport,
};

static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);
static SDL_INIT_CALLS: AtomicU64 = AtomicU64::new(0);
static SDL_QUIT_CALLS: AtomicU64 = AtomicU64::new(0);
static SDL_STREAM_OPENS: AtomicU64 = AtomicU64::new(0);
static SDL_STREAM_DESTROYS: AtomicU64 = AtomicU64::new(0);
static SDL_STREAM_RESUMES: AtomicU64 = AtomicU64::new(0);
static SDL_STREAM_PAUSES: AtomicU64 = AtomicU64::new(0);
static CALLBACK_STATES_CREATED: AtomicU64 = AtomicU64::new(0);
static CALLBACK_STATES_DROPPED: AtomicU64 = AtomicU64::new(0);
static PRODUCER_THREADS_STARTED: AtomicU64 = AtomicU64::new(0);
static PRODUCER_THREADS_FINISHED: AtomicU64 = AtomicU64::new(0);

pub struct SdlAudioOutput {
    stream: NonNull<SDL_AudioStream>,
    callback_state: NonNull<CallbackState>,
    producer: Option<JoinHandle<()>>,
    shared: Arc<SharedAudioState>,
    config: AudioDeviceConfig,
    opened_device_id: u32,
    opened_device_name: Option<String>,
    stream_input_format: Option<AudioFormatDiagnostics>,
    stream_output_format: Option<AudioFormatDiagnostics>,
    state: AudioDeviceState,
}

impl SdlAudioOutput {
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
            return Err("SDL output currently expects stereo f32 audio".to_owned());
        }

        SDL_INIT_CALLS.fetch_add(1, Ordering::Relaxed);
        if !unsafe { SDL_Init(SDL_INIT_AUDIO) } {
            return Err(format!("SDL audio init failed: {}", sdl_error()));
        }

        let spec = SDL_AudioSpec {
            format: SDL_AUDIO_F32,
            channels: config.channels as i32,
            freq: config.sample_rate as i32,
        };
        let channels = config.channels as usize;
        let ring = AudioRingBuffer::new(config.ring_buffer, channels);
        let ring_capacity_frames = ring.capacity_frames();
        let callback_scratch_frames = ring
            .capacity_frames()
            .max(config.internal_block_frames as usize);
        let stream_id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(SharedAudioState {
            stream_id,
            ring: Mutex::new(ring),
            lifecycle: Mutex::new(ProducerLifecycle::default()),
            lifecycle_changed: Condvar::new(),
            render_ahead: Mutex::new(AdaptiveRenderAheadRuntime::default_for_capacity(
                ring_capacity_frames,
            )),
            stats: AudioCallbackCounters::default(),
        });
        let callback_state = Box::new(CallbackState {
            channels,
            scratch: vec![0.0; callback_scratch_frames * channels],
            shared: Arc::clone(&shared),
            memory_hooks: memory_hooks.clone(),
        });
        CALLBACK_STATES_CREATED.fetch_add(1, Ordering::Relaxed);
        let callback_state = Box::into_raw(callback_state);

        let stream = unsafe {
            SDL_OpenAudioDeviceStream(
                SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK,
                &spec,
                Some(audio_stream_callback),
                callback_state.cast::<c_void>(),
            )
        };

        let Some(stream) = NonNull::new(stream) else {
            unsafe {
                drop(Box::from_raw(callback_state));
                SDL_QuitSubSystem(SDL_INIT_AUDIO);
            }
            CALLBACK_STATES_DROPPED.fetch_add(1, Ordering::Relaxed);
            SDL_QUIT_CALLS.fetch_add(1, Ordering::Relaxed);
            return Err(format!("SDL audio stream open failed: {}", sdl_error()));
        };
        SDL_STREAM_OPENS.fetch_add(1, Ordering::Relaxed);

        let callback_state = NonNull::new(callback_state)
            .ok_or_else(|| "SDL callback state pointer was unexpectedly null".to_owned())?;
        let opened_device_id = unsafe { SDL_GetAudioStreamDevice(stream.as_ptr()) }.0;
        let opened_device_name = device_name(opened_device_id);
        let (stream_input_format, stream_output_format) = stream_formats(stream.as_ptr());
        let producer = spawn_producer(Arc::clone(&shared), config, memory_hooks, renderer);

        Ok(Self {
            stream,
            callback_state,
            producer: Some(producer),
            shared,
            config,
            opened_device_id,
            opened_device_name,
            stream_input_format,
            stream_output_format,
            state: AudioDeviceState::Open,
        })
    }

    pub fn resume(&mut self) -> Result<(), String> {
        self.set_producer_running(true);
        if unsafe { SDL_ResumeAudioStreamDevice(self.stream.as_ptr()) } {
            SDL_STREAM_RESUMES.fetch_add(1, Ordering::Relaxed);
            self.state = AudioDeviceState::Running;
            Ok(())
        } else {
            self.set_producer_running(false);
            Err(format!("SDL audio resume failed: {}", sdl_error()))
        }
    }

    pub fn pause(&mut self) -> Result<(), String> {
        self.set_producer_running(false);
        if unsafe { SDL_PauseAudioStreamDevice(self.stream.as_ptr()) } {
            SDL_STREAM_PAUSES.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut ring) = self.shared.ring.lock() {
                ring.clear();
            }
            self.state = AudioDeviceState::Open;
            Ok(())
        } else {
            Err(format!("SDL audio pause failed: {}", sdl_error()))
        }
    }

    pub fn state(&self) -> AudioDeviceState {
        self.state
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
        let (ring_capacity_frames, ring_available_frames, ring_retained_bytes) = self
            .shared
            .ring
            .lock()
            .map(|ring| {
                (
                    ring.capacity_frames(),
                    ring.available_frames(),
                    ring.retained_bytes(),
                )
            })
            .unwrap_or_default();
        let callback_scratch_bytes =
            unsafe { self.callback_state.as_ref().scratch.capacity() * std::mem::size_of::<f32>() };
        let callback_shared_counts = unsafe {
            let shared = &self.callback_state.as_ref().shared;
            (Arc::strong_count(shared), Arc::weak_count(shared))
        };
        let shutdown_started = self
            .shared
            .lifecycle
            .lock()
            .map(|lifecycle| lifecycle.shutdown)
            .unwrap_or(true);
        let mut diagnostics = AudioDeviceDiagnostics::from_config(
            self.config,
            self.state,
            ring_capacity_frames,
            ring_available_frames,
            ring_retained_bytes,
            callback_scratch_bytes,
            shutdown_started,
            stats,
            sdl_lifecycle_snapshot(),
            AudioReferenceDiagnostics {
                stream_id: self.shared.stream_id,
                shared_strong_count: Arc::strong_count(&self.shared),
                shared_weak_count: Arc::weak_count(&self.shared),
                callback_shared_strong_count: callback_shared_counts.0,
                callback_shared_weak_count: callback_shared_counts.1,
            },
        );
        diagnostics.opened_device_id = self.opened_device_id;
        diagnostics.opened_device_name = self.opened_device_name.clone();
        diagnostics.stream_input_format = self.stream_input_format.clone();
        diagnostics.stream_output_format = self.stream_output_format.clone();
        diagnostics
    }

    pub fn render_ahead_state(&self) -> RenderAheadState {
        let stats = self.stats();
        let (ring_available_frames, ring_capacity_frames) = self
            .shared
            .ring
            .lock()
            .map(|ring| (ring.available_frames(), ring.capacity_frames()))
            .unwrap_or_default();
        let target = self
            .shared
            .render_ahead
            .lock()
            .map(|runtime| runtime.target)
            .unwrap_or_default();
        RenderAheadState {
            effective_target_frames: target.target_frames,
            current_buffered_frames: ring_available_frames.min(u32::MAX as usize) as u32,
            available_frames: ring_available_frames.min(u32::MAX as usize) as u32,
            largest_callback_frames: stats.largest_callback_frames.min(u64::from(u32::MAX)) as u32,
            underrun_count: stats.underrun_count,
            queue_error_count: stats.queue_error_count,
        }
        .with_capacity_fallback(ring_capacity_frames)
    }

    pub fn apply_render_ahead_target(&mut self, target: RenderAheadTarget) -> Result<(), String> {
        let capacity_frames = self
            .shared
            .ring
            .lock()
            .map(|ring| ring.capacity_frames())
            .map_err(|_| "SDL ring lock is poisoned".to_owned())?;
        let target = AdaptiveRenderAheadRuntime::clamp_target(target, capacity_frames);
        let mut runtime = self
            .shared
            .render_ahead
            .lock()
            .map_err(|_| "SDL render-ahead lock is poisoned".to_owned())?;
        runtime.target = target;
        Ok(())
    }

    fn set_producer_running(&self, running: bool) {
        if let Ok(mut lifecycle) = self.shared.lifecycle.lock() {
            lifecycle.running = running;
            self.shared.lifecycle_changed.notify_all();
        }
    }
}

impl Drop for SdlAudioOutput {
    fn drop(&mut self) {
        if let Ok(mut lifecycle) = self.shared.lifecycle.lock() {
            lifecycle.running = false;
            lifecycle.shutdown = true;
            self.shared.lifecycle_changed.notify_all();
        }
        unsafe {
            let _ = SDL_PauseAudioStreamDevice(self.stream.as_ptr());
            SDL_DestroyAudioStream(self.stream.as_ptr());
        }
        SDL_STREAM_PAUSES.fetch_add(1, Ordering::Relaxed);
        SDL_STREAM_DESTROYS.fetch_add(1, Ordering::Relaxed);
        if let Some(producer) = self.producer.take() {
            let _ = producer.join();
        }
        unsafe {
            drop(Box::from_raw(self.callback_state.as_ptr()));
            SDL_QuitSubSystem(SDL_INIT_AUDIO);
        }
        CALLBACK_STATES_DROPPED.fetch_add(1, Ordering::Relaxed);
        SDL_QUIT_CALLS.fetch_add(1, Ordering::Relaxed);
        self.state = AudioDeviceState::Closed;
    }
}

fn sdl_lifecycle_snapshot() -> AudioSdlLifecycleStats {
    AudioSdlLifecycleStats {
        init_calls: SDL_INIT_CALLS.load(Ordering::Relaxed),
        quit_calls: SDL_QUIT_CALLS.load(Ordering::Relaxed),
        stream_opens: SDL_STREAM_OPENS.load(Ordering::Relaxed),
        stream_destroys: SDL_STREAM_DESTROYS.load(Ordering::Relaxed),
        stream_resumes: SDL_STREAM_RESUMES.load(Ordering::Relaxed),
        stream_pauses: SDL_STREAM_PAUSES.load(Ordering::Relaxed),
        callback_states_created: CALLBACK_STATES_CREATED.load(Ordering::Relaxed),
        callback_states_dropped: CALLBACK_STATES_DROPPED.load(Ordering::Relaxed),
        producer_threads_started: PRODUCER_THREADS_STARTED.load(Ordering::Relaxed),
        producer_threads_finished: PRODUCER_THREADS_FINISHED.load(Ordering::Relaxed),
    }
}

struct CallbackState {
    channels: usize,
    scratch: Vec<f32>,
    shared: Arc<SharedAudioState>,
    memory_hooks: AudioMemoryHooks,
}

struct SharedAudioState {
    stream_id: u64,
    ring: Mutex<AudioRingBuffer>,
    lifecycle: Mutex<ProducerLifecycle>,
    lifecycle_changed: Condvar,
    render_ahead: Mutex<AdaptiveRenderAheadRuntime>,
    stats: AudioCallbackCounters,
}

#[derive(Debug, Default)]
struct ProducerLifecycle {
    running: bool,
    shutdown: bool,
}

#[derive(Debug, Copy, Clone)]
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

trait RenderAheadStateCapacityFallback {
    fn with_capacity_fallback(self, capacity_frames: usize) -> Self;
}

impl RenderAheadStateCapacityFallback for RenderAheadState {
    fn with_capacity_fallback(mut self, capacity_frames: usize) -> Self {
        if self.effective_target_frames == 0 {
            self.effective_target_frames = capacity_frames.min(u32::MAX as usize) as u32;
        }
        self
    }
}

unsafe extern "C" fn audio_stream_callback(
    userdata: *mut c_void,
    stream: *mut SDL_AudioStream,
    additional_amount: i32,
    total_amount: i32,
) {
    if userdata.is_null() || stream.is_null() || additional_amount <= 0 {
        return;
    }

    let state = &mut *(userdata.cast::<CallbackState>());
    bind_callback_thread_once(&state.memory_hooks);
    let bytes_per_sample = std::mem::size_of::<f32>();
    let requested_samples = additional_amount as usize / bytes_per_sample;
    let frames = requested_samples.div_ceil(state.channels).max(1);
    let scratch_frames = (state.scratch.len() / state.channels).max(1);
    let mut remaining_frames = frames;
    let mut delivered_frames = 0usize;
    let mut missing_frames = 0usize;
    let mut queued_all = true;
    let mut last_ring_available_frames = 0usize;
    let mut put_calls = 0u64;
    let mut put_bytes = 0u64;

    while remaining_frames > 0 {
        let chunk_frames = remaining_frames.min(scratch_frames);
        let chunk_samples = chunk_frames * state.channels;
        let output = &mut state.scratch[..chunk_samples];
        output.fill(0.0);

        let read_samples = if let Ok(mut ring) = state.shared.ring.try_lock() {
            let read_samples = ring.read(output);
            last_ring_available_frames = ring.available_frames();
            read_samples
        } else {
            0
        };
        let read_frames = read_samples / state.channels;
        delivered_frames += read_frames;
        missing_frames += chunk_frames.saturating_sub(read_frames);

        let byte_count = output.len() * bytes_per_sample;
        if !SDL_PutAudioStreamData(stream, output.as_ptr().cast::<c_void>(), byte_count as i32) {
            queued_all = false;
        }
        put_calls = put_calls.saturating_add(1);
        put_bytes = put_bytes.saturating_add(byte_count as u64);
        remaining_frames -= chunk_frames;
    }

    state.shared.stats.record_callback(
        frames as u64,
        additional_amount.max(0) as u64,
        total_amount.max(0) as u64,
        delivered_frames as u64,
        missing_frames as u64,
        queued_all,
        put_calls,
        put_bytes,
        last_ring_available_frames as u64,
    );
}

fn spawn_producer(
    shared: Arc<SharedAudioState>,
    config: AudioDeviceConfig,
    memory_hooks: AudioMemoryHooks,
    mut renderer: impl FnMut(&mut [f32]) + Send + 'static,
) -> JoinHandle<()> {
    thread::spawn(move || {
        PRODUCER_THREADS_STARTED.fetch_add(1, Ordering::Relaxed);
        let _producer_thread_guard = ProducerThreadGuard;
        let _memory_guard = memory_hooks.producer.as_ref().map(|bind| bind());
        let channels = config.channels as usize;
        let block_frames = config.internal_block_frames.max(1) as usize;
        let mut render_block = vec![0.0; block_frames * channels];
        shared.stats.record_producer_render_block_bytes(
            (render_block.capacity() * std::mem::size_of::<f32>()) as u64,
        );

        loop {
            let mut lifecycle = match shared.lifecycle.lock() {
                Ok(lifecycle) => lifecycle,
                Err(_) => return,
            };
            while !lifecycle.running && !lifecycle.shutdown {
                lifecycle = match shared.lifecycle_changed.wait(lifecycle) {
                    Ok(lifecycle) => lifecycle,
                    Err(_) => return,
                };
            }
            if lifecycle.shutdown {
                return;
            }
            drop(lifecycle);

            let should_render = shared
                .ring
                .lock()
                .map(|ring| {
                    let available_frames = ring.available_frames();
                    let free_frames = ring.free_frames();
                    let high_water_frames = shared
                        .render_ahead
                        .lock()
                        .map(|runtime| runtime.target.high_water_frames as usize)
                        .unwrap_or_else(|_| ring.capacity_frames());
                    free_frames >= block_frames && available_frames < high_water_frames
                })
                .unwrap_or(false);
            if !should_render {
                thread::sleep(Duration::from_millis(1));
                continue;
            }

            render_block.fill(0.0);
            renderer(&mut render_block);
            if let Ok(mut ring) = shared.ring.lock() {
                let written_samples = ring.write(&render_block);
                shared
                    .stats
                    .record_producer_rendered((written_samples / channels) as u64);
            } else {
                return;
            }
        }
    })
}

thread_local! {
    static CALLBACK_MEMORY_GUARD: RefCell<Option<Box<dyn Send>>> = RefCell::new(None);
}

fn bind_callback_thread_once(memory_hooks: &AudioMemoryHooks) {
    let Some(bind) = memory_hooks.callback.as_ref() else {
        return;
    };
    CALLBACK_MEMORY_GUARD.with(|guard| {
        let mut guard = guard.borrow_mut();
        if guard.is_none() {
            *guard = Some(bind());
        }
    });
}

struct ProducerThreadGuard;

impl Drop for ProducerThreadGuard {
    fn drop(&mut self) {
        PRODUCER_THREADS_FINISHED.fetch_add(1, Ordering::Relaxed);
    }
}

fn stream_formats(
    stream: *mut SDL_AudioStream,
) -> (
    Option<AudioFormatDiagnostics>,
    Option<AudioFormatDiagnostics>,
) {
    let mut input = SDL_AudioSpec {
        format: SDL_AUDIO_F32,
        channels: 0,
        freq: 0,
    };
    let mut output = input;
    if unsafe { SDL_GetAudioStreamFormat(stream, &mut input, &mut output) } {
        (format_diagnostics(input), format_diagnostics(output))
    } else {
        (None, None)
    }
}

fn format_diagnostics(spec: SDL_AudioSpec) -> Option<AudioFormatDiagnostics> {
    let format = if spec.format == SDL_AUDIO_F32 {
        "f32"
    } else {
        "other"
    };
    Some(AudioFormatDiagnostics {
        format,
        channels: spec.channels as u16,
        sample_rate: spec.freq as u32,
    })
}

fn device_name(opened_device_id: u32) -> Option<String> {
    let pointer = unsafe {
        SDL_GetAudioDeviceName(sdl3_sys::everything::SDL_AudioDeviceID(opened_device_id))
    };
    if pointer.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn sdl_error() -> String {
    let pointer = SDL_GetError();
    if pointer.is_null() {
        return "unknown SDL error".to_owned();
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned()
}
