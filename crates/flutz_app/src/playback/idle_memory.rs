use super::*;

const TOUCH_PAGE_BYTES: usize = 4096;

/// What `prewarm_for_resume` pre-touched before audio resumed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlaybackPrewarmReport {
    pub idle_seconds: f64,
    pub arenas_were_cold: bool,
    pub soundfont_pages: u64,
    pub decoded_pages: u64,
    pub arena_bytes_warmed: u64,
    pub elapsed_ms: f64,
}

/// Reads one element per page so pages trimmed from the working set fault in now,
/// not in the audio callback. Returns the number of pages touched.
pub(in crate::playback) fn touch_pages<T: Copy>(values: &[T]) -> u64 {
    let stride = (TOUCH_PAGE_BYTES / std::mem::size_of::<T>().max(1)).max(1);
    let mut pages = 0u64;
    for index in (0..values.len()).step_by(stride) {
        std::hint::black_box(values[index]);
        pages += 1;
    }
    pages
}

impl PlaybackController {
    /// Deep idle release: drops the audio device stream and every playback scratch
    /// buffer, trims the SoundFont cache to the loaded engine, and purges all arenas.
    /// Loaded content, transport position, and pause state are kept, so `play()` resumes
    /// in place. Returns false (and does nothing) while playback is active.
    pub fn release_idle_memory(&mut self) -> Result<bool> {
        if self.playback_active() {
            return Ok(false);
        }
        memory_runtime::record_audio_working_set();
        if let Some(audio) = &mut self.audio {
            let _ = audio.pause();
        }
        self.audio = None;
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
        self.soundfont_cache.release_unused();
        memory_runtime::release_idle_now();
        Ok(true)
    }

    /// Pre-touches loaded sample data and re-commits the audio arenas' working set.
    /// `play()` calls this when audio was idle for `RESUME_PREWARM_AFTER` or the arenas
    /// were released; it doesn't move the transport.
    pub fn prewarm_for_resume(&mut self) -> PlaybackPrewarmReport {
        let started = Instant::now();
        let idle_seconds = memory_runtime::audio_idle_duration().as_secs_f64();
        let arenas_were_cold = memory_runtime::audio_arenas_cold();
        let soundfont_pages = self
            .soundfont_cache
            .live_wave_data()
            .map(touch_pages)
            .sum::<u64>();
        let decoded_pages = self
            .decoded_audio
            .as_ref()
            .and_then(|decoded| {
                decoded
                    .lock()
                    .ok()
                    .map(|decoded| decoded.touch_resident_pages())
            })
            .unwrap_or(0);
        let arena_bytes_warmed = memory_runtime::prewarm_reuse_critical_domains();
        let report = PlaybackPrewarmReport {
            idle_seconds,
            arenas_were_cold,
            soundfont_pages,
            decoded_pages,
            arena_bytes_warmed,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        };
        self.last_prewarm = Some(report);
        report
    }

    pub fn last_prewarm_report(&self) -> Option<PlaybackPrewarmReport> {
        self.last_prewarm
    }

    pub(super) fn prewarm_if_resuming_cold(&mut self) {
        if self.engine.is_none() && self.decoded_audio.is_none() {
            return;
        }
        if self.playback_active() {
            return;
        }
        if memory_runtime::should_prewarm_on_resume(
            memory_runtime::audio_idle_duration(),
            memory_runtime::audio_arenas_cold(),
        ) {
            self.prewarm_for_resume();
        }
    }
}
