pub mod allocation_trace;
pub mod app;
mod dat_startup;
mod launch;
#[cfg(target_os = "linux")]
mod linux_desktop_integration;
pub mod memory_runtime;
pub mod perf_trace;
pub mod playback;
pub mod playlist;
pub mod project;
pub mod repaint_policy;
pub mod routing;
pub mod ui;
pub(crate) use dat_startup::collect_dat_paths;
pub use dat_startup::dat_startup_summary_for_data_dir;
#[cfg(windows)]
pub use launch::windows_file_association_specs;
#[cfg(windows)]
mod windows_file_association;

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::IsTerminal,
    path::{Path, PathBuf},
};

use flutz_core::{default_preset_set, FlutzError, Result};
use flutz_dat::{
    assets::DAT_ENTRY_FLAG_DEFAULT,
    read::{parse_dat_index, parse_dat_index_file, read_soundfont_coverage_json_from_file},
    soundfont_json::SoundFontCoverageJson,
};
use flutz_formats::builtin_registry;
use flutz_synth::{
    BankProgram, MelodicCoverage, PercussionCoverage, PercussionKeyRange, SoundFontCoverage,
    SoundFontCoverageMetadata,
};

pub fn run() -> Result<()> {
    launch::run()
}
