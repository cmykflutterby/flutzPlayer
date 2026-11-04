mod block;
pub mod effects;
mod engine;
pub mod master;
pub mod meters;
pub mod normalization;
mod runtime;
mod scratch;
pub mod smart_mix;
pub mod strip;
mod types;

pub use block::{AudioBlock, AudioBlockView, StereoFrame};
pub use engine::MixerEngine;
pub use scratch::{MixerAllocationStats, MixerScratchStats};
pub use types::{
    MixOutputReport, MixReport, MixerSettings, MixerStripInput, MixerStripInputView, StripMixReport,
};

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Range,
};

use flutz_core::{FlutzError, Result, StripId};

use crate::effects::{DelayEffectState, ThreeBandEqState};

pub use master::MasterControls;
pub use meters::MeterReading;
pub use normalization::AutoNormalization;
pub use smart_mix::SmartMixSettings;
pub use strip::{db_to_gain, MixerStripControls, MixerStripIdentity};
