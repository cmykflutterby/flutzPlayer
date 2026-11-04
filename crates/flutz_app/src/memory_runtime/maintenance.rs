use std::{
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
};

use super::*;

pub const MEMORY_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5);
pub const MEMORY_MAINTENANCE_THREAD_NAME: &str = "flutz-memory-maint";

/// Decays non-reuse-critical arenas on a fixed interval, independent of the UI frame rate.
/// Reuse-critical (audio) arenas are left alone while audio is in use; once audio has been
/// idle for `IDLE_DEEP_RELEASE_AFTER` every arena is purged once (see `idle`).
/// Dropping the handle stops and joins the thread.
#[derive(Debug)]
pub struct MemoryMaintenanceHandle {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl MemoryMaintenanceHandle {
    pub fn spawn(interval: Duration) -> std::io::Result<Self> {
        let (stop, stop_signal) = mpsc::channel::<()>();
        let runtime = global();
        let thread = thread::Builder::new()
            .name(MEMORY_MAINTENANCE_THREAD_NAME.to_owned())
            .spawn(move || {
                let _domain = runtime.bind_current_thread(MemoryDomain::App);
                while let Err(RecvTimeoutError::Timeout) = stop_signal.recv_timeout(interval) {
                    runtime.decay_all_idle_domains();
                    super::idle::service_idle_release_with(
                        &runtime,
                        super::idle::IDLE_DEEP_RELEASE_AFTER,
                    );
                }
            })?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for MemoryMaintenanceHandle {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
