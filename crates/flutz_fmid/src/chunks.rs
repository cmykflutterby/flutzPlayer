#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ChunkId(pub [u8; 4]);

impl ChunkId {
    pub const MIDI: Self = Self(*b"MIDI");
    pub const PROJ: Self = Self(*b"PROJ");
    pub const FONT: Self = Self(*b"FONT");
    pub const MIXR: Self = Self(*b"MIXR");
    pub const LOOP: Self = Self(*b"LOOP");
    pub const SMIX: Self = Self(*b"SMIX");
    pub const NOTE: Self = Self(*b"NOTE");
    pub const MSRC: Self = Self(*b"MSRC");

    pub fn is_known(id: [u8; 4]) -> bool {
        id == Self::MIDI.0
            || id == Self::PROJ.0
            || id == Self::FONT.0
            || id == Self::MIXR.0
            || id == Self::LOOP.0
            || id == Self::SMIX.0
            || id == Self::NOTE.0
            || id == Self::MSRC.0
    }

    pub fn is_required(id: [u8; 4]) -> bool {
        id == Self::MIDI.0
            || id == Self::PROJ.0
            || id == Self::FONT.0
            || id == Self::MIXR.0
            || id == Self::LOOP.0
            || id == Self::SMIX.0
    }
}
