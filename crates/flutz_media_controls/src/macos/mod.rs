use crate::{MediaControlsBackend, NoopMediaControlsBackend};

pub fn create_media_controls_backend() -> Box<dyn MediaControlsBackend> {
    Box::new(NoopMediaControlsBackend::default())
}
