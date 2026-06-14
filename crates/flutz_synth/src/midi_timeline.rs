use flutz_core::MidiTick;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct MidiTimelinePosition {
    pub tick: MidiTick,
}
