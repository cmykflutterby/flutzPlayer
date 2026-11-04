//! Streaming parametric EQ primitives for decoded-audio mastering.
//!
//! This crate currently requires `std` for preset file I/O and owned runtime state.
//! It does not install a custom global allocator and relies on the process allocator.
//!
//! `PeqProcessor` is a single-owner processing object with no internal locking.
//! Prepare replacement configs off the realtime path with `PreparedConfig::from_config`
//! and hand them to `set_prepared_config`; the swap becomes active on the next
//! `process_*` call, which is the intended chunk boundary for live playback updates.

mod biquad;
mod config;
mod preset_io;
mod processor;

pub use config::{
    Bandwidth, ChannelLayout, PeqBandConfig, PeqConfig, PeqError, PeqFilterType, PeqPresetFile,
    PresetMetadata, Result,
};
pub use preset_io::{
    deserialize_preset_toml, load_preset_file, save_preset_file, serialize_preset_toml,
};
pub use processor::{
    PeqProcessor, PreparedConfig, ProcessSummary, ProcessorMetrics, ReleaseDisposition,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, error::Error, f32::consts::PI, fmt, fs, mem, path::Path};
