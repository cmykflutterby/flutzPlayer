use crate::{MediaControlCommand, MediaControlsBackend, MediaTrackSnapshot};

#[derive(Debug, Default)]
pub struct NoopMediaControlsBackend {
    last_snapshot: Option<MediaTrackSnapshot>,
}

impl NoopMediaControlsBackend {
    pub fn last_snapshot(&self) -> Option<&MediaTrackSnapshot> {
        self.last_snapshot.as_ref()
    }
}

impl MediaControlsBackend for NoopMediaControlsBackend {
    fn poll_command(&mut self) -> Option<MediaControlCommand> {
        None
    }

    fn publish_snapshot(&mut self, snapshot: &MediaTrackSnapshot) {
        self.last_snapshot = Some(snapshot.clone());
    }

    fn shutdown(&mut self) {}
}
