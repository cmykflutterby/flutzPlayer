use flutz_core::MidiTick;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SeekRequest {
    pub target_tick: MidiTick,
    pub rebuild_state: bool,
    pub all_notes_off_first: bool,
}

impl SeekRequest {
    pub fn new(target_tick: MidiTick) -> Self {
        Self {
            target_tick,
            rebuild_state: true,
            all_notes_off_first: true,
        }
    }
}
