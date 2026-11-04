use super::*;

mod controls;
mod feedback;
mod presets;
mod types;
pub(super) use types::{
    coverage_supports_strip, non_empty_media_metadata, set_text_in_place,
    soundfont_rows_from_catalog, strip_id_for, write_note_name, MixerEditSnapshot,
    PlaybackReloadResumeState, RealtimeStripKey, RowEditSnapshot, StripEditSnapshot,
};
pub use types::{
    MasterControls, MixerAssignmentMode, MixerStripUiState, SmartMixControls, SoundFontUiRow,
    FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER, MASTER_VOLUME_MAX_DB,
};

impl FlutzDesktopApp {}
