#![allow(dead_code)]

use std::cmp;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::mem;
use std::sync::Arc;

use crate::array_math::ArrayMath;
use crate::channel::Channel;
use crate::chorus::Chorus;
use crate::error::SynthesizerError;
use crate::midi_interpretation::{decode_sysex_event, MidiChannelRole};
use crate::region_pair::RegionPair;
use crate::reverb::Reverb;
use crate::soundfont::SoundFont;
use crate::soundfont_math::SoundFontMath;
use crate::stem::{
    StemIdentity, StemRenderBlock, StemRenderMode, StemRenderRequest, StemRenderSet,
};
use crate::synthesizer_settings::SynthesizerSettings;
use crate::voice::Voice;
use crate::voice_collection::VoiceCollection;

mod core;
mod effects;
mod memory;
mod render;
mod stems;
pub use core::Synthesizer;
use effects::Effects;
pub use memory::SynthesizerMemoryDebug;
use memory::{
    f32_vec_bytes, stem_effect_inputs_total_bytes, stem_render_blocks_active_note_bytes,
    stem_render_blocks_audio_bytes, stem_render_blocks_total_bytes,
};
pub use stems::StemRenderAllocationDebug;
use stems::{
    prepare_stem_blocks, prepare_stem_effect_inputs, stem_block_has_signal,
    upsert_output_stem_block, StemEffectAllocationCounters, StemEffectInput, StemEffects,
};
