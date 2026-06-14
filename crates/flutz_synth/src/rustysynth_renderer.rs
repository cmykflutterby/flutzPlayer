#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RustySynthRendererDescriptor {
    pub renderer_name: String,
}

pub const RUSTYSTEM_CRATE_VERSION: &str = "0.1.0";
pub const RUSTYSYNTH_CRATE_VERSION: &str = RUSTYSTEM_CRATE_VERSION;

pub fn rustystem_soundfont_type_name() -> &'static str {
    std::any::type_name::<rustystem::SoundFont>()
}

pub fn rustysynth_soundfont_type_name() -> &'static str {
    rustystem_soundfont_type_name()
}
