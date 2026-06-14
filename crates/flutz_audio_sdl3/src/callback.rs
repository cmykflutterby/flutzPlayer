use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct AudioCallbackStats {
    pub frames_requested: u64,
    pub frames_delivered: u64,
    pub underrun_count: u64,
    pub callback_count: u64,
    pub queue_error_count: u64,
    pub last_callback_frames: u64,
    pub largest_callback_frames: u64,
    pub last_callback_additional_bytes: u64,
    pub largest_callback_additional_bytes: u64,
    pub last_callback_total_bytes: u64,
    pub largest_callback_total_bytes: u64,
    pub last_ring_available_frames: u64,
    pub producer_rendered_frames: u64,
    pub producer_render_block_bytes: u64,
    pub write_calls: u64,
    pub write_bytes: u64,
}

#[derive(Debug, Default)]
pub struct AudioCallbackCounters {
    frames_requested: AtomicU64,
    frames_delivered: AtomicU64,
    underrun_count: AtomicU64,
    callback_count: AtomicU64,
    queue_error_count: AtomicU64,
    last_callback_frames: AtomicU64,
    largest_callback_frames: AtomicU64,
    last_callback_additional_bytes: AtomicU64,
    largest_callback_additional_bytes: AtomicU64,
    last_callback_total_bytes: AtomicU64,
    largest_callback_total_bytes: AtomicU64,
    last_ring_available_frames: AtomicU64,
    producer_rendered_frames: AtomicU64,
    producer_render_block_bytes: AtomicU64,
    write_calls: AtomicU64,
    write_bytes: AtomicU64,
}

impl AudioCallbackCounters {
    pub fn record_callback(
        &self,
        frames_requested: u64,
        additional_bytes: u64,
        total_bytes: u64,
        frames_delivered: u64,
        missing_frames: u64,
        queued: bool,
        write_calls: u64,
        write_bytes: u64,
        ring_available_frames: u64,
    ) {
        self.frames_requested
            .fetch_add(frames_requested, Ordering::Relaxed);
        if queued {
            self.frames_delivered
                .fetch_add(frames_delivered, Ordering::Relaxed);
        } else {
            self.queue_error_count.fetch_add(1, Ordering::Relaxed);
        }
        if missing_frames > 0 {
            self.underrun_count.fetch_add(1, Ordering::Relaxed);
        }
        self.callback_count.fetch_add(1, Ordering::Relaxed);
        self.last_callback_frames
            .store(frames_requested, Ordering::Relaxed);
        self.largest_callback_frames
            .fetch_max(frames_requested, Ordering::Relaxed);
        self.last_callback_additional_bytes
            .store(additional_bytes, Ordering::Relaxed);
        self.largest_callback_additional_bytes
            .fetch_max(additional_bytes, Ordering::Relaxed);
        self.last_callback_total_bytes
            .store(total_bytes, Ordering::Relaxed);
        self.largest_callback_total_bytes
            .fetch_max(total_bytes, Ordering::Relaxed);
        self.write_calls.fetch_add(write_calls, Ordering::Relaxed);
        self.write_bytes.fetch_add(write_bytes, Ordering::Relaxed);
        self.last_ring_available_frames
            .store(ring_available_frames, Ordering::Relaxed);
    }

    pub fn record_producer_render_block_bytes(&self, bytes: u64) {
        self.producer_render_block_bytes
            .store(bytes, Ordering::Relaxed);
    }

    pub fn record_producer_rendered(&self, frames: u64) {
        self.producer_rendered_frames
            .fetch_add(frames, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> AudioCallbackStats {
        AudioCallbackStats {
            frames_requested: self.frames_requested.load(Ordering::Relaxed),
            frames_delivered: self.frames_delivered.load(Ordering::Relaxed),
            underrun_count: self.underrun_count.load(Ordering::Relaxed),
            callback_count: self.callback_count.load(Ordering::Relaxed),
            queue_error_count: self.queue_error_count.load(Ordering::Relaxed),
            last_callback_frames: self.last_callback_frames.load(Ordering::Relaxed),
            largest_callback_frames: self.largest_callback_frames.load(Ordering::Relaxed),
            last_callback_additional_bytes: self
                .last_callback_additional_bytes
                .load(Ordering::Relaxed),
            largest_callback_additional_bytes: self
                .largest_callback_additional_bytes
                .load(Ordering::Relaxed),
            last_callback_total_bytes: self.last_callback_total_bytes.load(Ordering::Relaxed),
            largest_callback_total_bytes: self.largest_callback_total_bytes.load(Ordering::Relaxed),
            last_ring_available_frames: self.last_ring_available_frames.load(Ordering::Relaxed),
            producer_rendered_frames: self.producer_rendered_frames.load(Ordering::Relaxed),
            producer_render_block_bytes: self.producer_render_block_bytes.load(Ordering::Relaxed),
            write_calls: self.write_calls.load(Ordering::Relaxed),
            write_bytes: self.write_bytes.load(Ordering::Relaxed),
        }
    }
}
