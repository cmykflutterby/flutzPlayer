use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    mem,
    sync::Arc,
    thread,
};

use flutz_core::{FlutzError, Result};
use rustystem::{
    extract_coverage_from_sf2, MidiChannelProgramRole, MidiFile, MidiFileLoopType,
    MidiFileMemoryDebug, MidiFileSequencer, MidiInterpretation, SequencerLoopMode,
    SequencerLoopSettings, SoundFont, SoundFontCoverage, SoundFontMetadata,
    SoundFontMetadataClosure, StemRenderAllocationDebug, StemRenderBlock, StemRenderRequest,
    Synthesizer, SynthesizerMemoryDebug, SynthesizerSettings,
};

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum PlaybackLoopMode {
    #[default]
    None,
    Infinite,
    Counted,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PlaybackLoopSettings {
    pub enabled: bool,
    pub mode: PlaybackLoopMode,
    pub start_tick: u64,
    pub end_tick: u64,
    pub loop_count: u32,
}

impl Default for PlaybackLoopSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: PlaybackLoopMode::None,
            start_tick: 0,
            end_tick: 0,
            loop_count: 1,
        }
    }
}

impl PlaybackLoopSettings {
    fn as_sequencer_settings(self) -> SequencerLoopSettings {
        SequencerLoopSettings {
            enabled: self.enabled,
            mode: match self.mode {
                PlaybackLoopMode::None => SequencerLoopMode::None,
                PlaybackLoopMode::Infinite => SequencerLoopMode::Infinite,
                PlaybackLoopMode::Counted => SequencerLoopMode::Counted,
            },
            start_tick: self.start_tick.min(i32::MAX as u64) as i32,
            end_tick: self.end_tick.min(i32::MAX as u64) as i32,
            loop_count: self.loop_count.max(1),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PlaybackConfig {
    pub sample_rate: u32,
    pub block_frames: usize,
    pub maximum_polyphony: usize,
    pub enable_reverb_and_chorus: bool,
    pub reposition_preroll_ms: u32,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            block_frames: 512,
            maximum_polyphony: 128,
            enable_reverb_and_chorus: true,
            reposition_preroll_ms: 750,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SoundFontBytes {
    internal_id: String,
    display_name: String,
    bytes: Vec<u8>,
    source: SoundFontDataSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundFontDataSource {
    DatEntry { dat_files: Vec<String> },
}

impl SoundFontBytes {
    pub fn from_dat_entry(
        internal_id: impl Into<String>,
        display_name: impl Into<String>,
        bytes: Vec<u8>,
        dat_files: Vec<String>,
    ) -> Self {
        Self {
            internal_id: internal_id.into(),
            display_name: display_name.into(),
            bytes,
            source: SoundFontDataSource::DatEntry { dat_files },
        }
    }

    pub fn internal_id(&self) -> &str {
        &self.internal_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn source(&self) -> &SoundFontDataSource {
        &self.source
    }

    fn cache_key(&self) -> String {
        format!(
            "full:{}",
            source_cache_key(&self.internal_id, self.bytes.len(), &self.source)
        )
    }

    fn source_description(&self) -> String {
        source_description(&self.source)
    }
}

fn source_cache_key(internal_id: &str, byte_len: usize, source: &SoundFontDataSource) -> String {
    match source {
        SoundFontDataSource::DatEntry { dat_files } => {
            format!("dat:{internal_id}:{byte_len}:{}", dat_files.join("|"))
        }
    }
}

fn source_description(source: &SoundFontDataSource) -> String {
    match source {
        SoundFontDataSource::DatEntry { dat_files } => {
            format!("DAT entry from {}", dat_files.join(", "))
        }
    }
}

fn closure_signature(closure: &SoundFontMetadataClosure) -> String {
    format!(
        "p={};i={};s={}",
        join_usize_ids(&closure.preset_ids),
        join_usize_ids(&closure.instrument_ids),
        join_usize_ids(&closure.sample_ids)
    )
}

fn join_usize_ids(ids: &[usize]) -> String {
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn sample_range_key(source_key: &str, sample_id: usize) -> String {
    format!("sample-range:{source_key}:{sample_id}")
}

#[derive(Debug, Clone)]
pub struct SoundFontSubsetSampleRange {
    pub sample_id: usize,
    pub samples: Vec<i16>,
}

#[derive(Debug, Clone)]
pub struct SoundFontSubsetBytes {
    internal_id: String,
    display_name: String,
    source_bytes: Vec<u8>,
    source: SoundFontDataSource,
    demand_signature: String,
    closure: SoundFontMetadataClosure,
    sample_ranges: Vec<SoundFontSubsetSampleRange>,
}

impl SoundFontSubsetBytes {
    pub fn from_dat_entry(
        internal_id: impl Into<String>,
        display_name: impl Into<String>,
        source_bytes: Vec<u8>,
        dat_files: Vec<String>,
        demand_signature: impl Into<String>,
        closure: SoundFontMetadataClosure,
        sample_ranges: Vec<SoundFontSubsetSampleRange>,
    ) -> Self {
        Self {
            internal_id: internal_id.into(),
            display_name: display_name.into(),
            source_bytes,
            source: SoundFontDataSource::DatEntry { dat_files },
            demand_signature: demand_signature.into(),
            closure,
            sample_ranges,
        }
    }

    pub fn internal_id(&self) -> &str {
        &self.internal_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    fn source_key(&self) -> String {
        source_cache_key(&self.internal_id, self.source_bytes.len(), &self.source)
    }

    fn metadata_key(&self) -> String {
        format!("metadata:{}", self.source_key())
    }

    fn subset_key(&self) -> String {
        format!(
            "subset:{}:{}:{}",
            self.source_key(),
            self.demand_signature,
            closure_signature(&self.closure)
        )
    }

    fn source_description(&self) -> String {
        source_description(&self.source)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SoundFontRuntimeCacheDebug {
    pub entries: usize,
    pub full_entries: usize,
    pub metadata_entries: usize,
    pub subset_entries: usize,
    pub sample_range_entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub full_hits: u64,
    pub full_misses: u64,
    pub metadata_hits: u64,
    pub metadata_misses: u64,
    pub subset_hits: u64,
    pub subset_contained_hits: u64,
    pub subset_misses: u64,
    pub sample_range_hits: u64,
    pub sample_range_misses: u64,
    pub decoded_bytes: u64,
    pub resident_bytes: usize,
    pub evictions: u64,
    pub total_strong_count: usize,
    pub max_strong_count: usize,
    pub entry_refs: Vec<SoundFontRuntimeCacheEntryDebug>,
}

#[derive(Debug, Clone, Default)]
pub struct SoundFontRuntimeCacheEntryDebug {
    pub key: String,
    pub layer: &'static str,
    pub strong_count: usize,
    pub weak_count: usize,
    pub resident_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct LoadedSoundFont {
    pub internal_id: String,
    pub display_name: String,
    pub sound_font: Arc<SoundFont>,
}

#[derive(Debug, Default)]
pub struct SoundFontRuntimeCache {
    entries: BTreeMap<String, Arc<SoundFont>>,
    metadata_entries: BTreeMap<String, Arc<SoundFontMetadata>>,
    subset_entries: BTreeMap<String, SoundFontSubsetCacheEntry>,
    sample_range_entries: BTreeMap<String, Arc<Vec<i16>>>,
    debug: SoundFontRuntimeCacheDebug,
}

#[derive(Debug, Clone)]
struct SoundFontSubsetCacheEntry {
    source_key: String,
    sample_ids: BTreeSet<usize>,
    sound_font: Arc<SoundFont>,
}

impl SoundFontRuntimeCache {
    pub fn load_soundfont(&mut self, soundfont: SoundFontBytes) -> Result<Arc<SoundFont>> {
        let key = soundfont.cache_key();
        if let Some(cached) = self.entries.get(&key) {
            self.debug.hits = self.debug.hits.saturating_add(1);
            self.debug.full_hits = self.debug.full_hits.saturating_add(1);
            return Ok(Arc::clone(cached));
        }

        self.debug.misses = self.debug.misses.saturating_add(1);
        self.debug.full_misses = self.debug.full_misses.saturating_add(1);
        self.debug.decoded_bytes = self
            .debug
            .decoded_bytes
            .saturating_add(soundfont.bytes.len() as u64);
        let internal_id = soundfont.internal_id.clone();
        let source_description = soundfont.source_description();
        let mut cursor = Cursor::new(soundfont.bytes);
        let sound_font = Arc::new(SoundFont::new(&mut cursor).map_err(|error| {
            FlutzError::InvalidInput(format!(
                "failed to load DAT-sourced soundfont {internal_id} ({source_description}): {error:?}; DAT soundfont bytes are expected to be RustyStem/RustySynth-compatible, so revalidate DAT packing/unpacking byte identity for this entry"
            ))
        })?);
        self.entries.insert(key, Arc::clone(&sound_font));
        self.refresh_resident_bytes();
        Ok(sound_font)
    }

    pub fn load_soundfonts_parallel(
        &mut self,
        soundfonts: Vec<SoundFontBytes>,
    ) -> Result<Vec<LoadedSoundFont>> {
        let mut loaded = vec![None; soundfonts.len()];
        let mut parse_tasks = Vec::new();

        for (index, soundfont) in soundfonts.into_iter().enumerate() {
            let key = soundfont.cache_key();
            let internal_id = soundfont.internal_id.clone();
            let display_name = soundfont.display_name.clone();
            if let Some(cached) = self.entries.get(&key) {
                self.debug.hits = self.debug.hits.saturating_add(1);
                self.debug.full_hits = self.debug.full_hits.saturating_add(1);
                loaded[index] = Some(LoadedSoundFont {
                    internal_id,
                    display_name,
                    sound_font: Arc::clone(cached),
                });
            } else {
                self.debug.misses = self.debug.misses.saturating_add(1);
                self.debug.full_misses = self.debug.full_misses.saturating_add(1);
                self.debug.decoded_bytes = self
                    .debug
                    .decoded_bytes
                    .saturating_add(soundfont.bytes.len() as u64);
                parse_tasks.push((index, key, soundfont));
            }
        }

        for parsed in parse_soundfonts_parallel(parse_tasks)? {
            self.entries
                .insert(parsed.key, Arc::clone(&parsed.sound_font));
            loaded[parsed.index] = Some(LoadedSoundFont {
                internal_id: parsed.internal_id,
                display_name: parsed.display_name,
                sound_font: parsed.sound_font,
            });
        }

        self.refresh_resident_bytes();
        loaded
            .into_iter()
            .map(|entry| {
                entry.ok_or_else(|| {
                    FlutzError::Runtime("soundfont load result was incomplete".to_owned())
                })
            })
            .collect()
    }

    pub fn load_subset_soundfont(
        &mut self,
        request: SoundFontSubsetBytes,
    ) -> Result<Arc<SoundFont>> {
        let mut loaded = self.load_subset_soundfonts_parallel(vec![request])?;
        loaded
            .pop()
            .map(|loaded| loaded.sound_font)
            .ok_or_else(|| FlutzError::Runtime("subset soundfont load result was empty".to_owned()))
    }

    pub fn load_subset_soundfonts_parallel(
        &mut self,
        requests: Vec<SoundFontSubsetBytes>,
    ) -> Result<Vec<LoadedSoundFont>> {
        let mut loaded = vec![None; requests.len()];
        let mut build_tasks = Vec::new();

        for (index, request) in requests.into_iter().enumerate() {
            let subset_key = request.subset_key();
            let source_key = request.source_key();
            let sample_ids = request
                .closure
                .sample_ids
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            if let Some(cached) = self.subset_entries.get(&subset_key) {
                self.debug.hits = self.debug.hits.saturating_add(1);
                self.debug.subset_hits = self.debug.subset_hits.saturating_add(1);
                loaded[index] = Some(LoadedSoundFont {
                    internal_id: request.internal_id,
                    display_name: request.display_name,
                    sound_font: Arc::clone(&cached.sound_font),
                });
                continue;
            }
            if let Some(cached) = self.find_containing_subset(&source_key, &sample_ids) {
                self.debug.hits = self.debug.hits.saturating_add(1);
                self.debug.subset_contained_hits =
                    self.debug.subset_contained_hits.saturating_add(1);
                loaded[index] = Some(LoadedSoundFont {
                    internal_id: request.internal_id,
                    display_name: request.display_name,
                    sound_font: cached,
                });
                continue;
            }

            self.debug.misses = self.debug.misses.saturating_add(1);
            self.debug.subset_misses = self.debug.subset_misses.saturating_add(1);
            self.ensure_metadata_cached(&request)?;
            let compact_wave_data = self.compact_wave_data_from_ranges(&request, &source_key)?;
            self.debug.decoded_bytes = self
                .debug
                .decoded_bytes
                .saturating_add(request.source_bytes.len() as u64);
            build_tasks.push((
                index,
                subset_key,
                source_key,
                sample_ids,
                request,
                compact_wave_data,
            ));
        }

        for parsed in parse_subset_soundfonts_parallel(build_tasks)? {
            self.subset_entries.insert(
                parsed.key.clone(),
                SoundFontSubsetCacheEntry {
                    source_key: parsed.source_key,
                    sample_ids: parsed.sample_ids,
                    sound_font: Arc::clone(&parsed.sound_font),
                },
            );
            loaded[parsed.index] = Some(LoadedSoundFont {
                internal_id: parsed.internal_id,
                display_name: parsed.display_name,
                sound_font: parsed.sound_font,
            });
        }

        self.refresh_resident_bytes();
        loaded
            .into_iter()
            .map(|entry| {
                entry.ok_or_else(|| {
                    FlutzError::Runtime("subset soundfont load result was incomplete".to_owned())
                })
            })
            .collect()
    }

    fn find_containing_subset(
        &self,
        source_key: &str,
        sample_ids: &BTreeSet<usize>,
    ) -> Option<Arc<SoundFont>> {
        self.subset_entries
            .values()
            .find(|entry| entry.source_key == source_key && sample_ids.is_subset(&entry.sample_ids))
            .map(|entry| Arc::clone(&entry.sound_font))
    }

    fn ensure_metadata_cached(&mut self, request: &SoundFontSubsetBytes) -> Result<()> {
        let key = request.metadata_key();
        if self.metadata_entries.contains_key(&key) {
            self.debug.metadata_hits = self.debug.metadata_hits.saturating_add(1);
            return Ok(());
        }
        self.debug.metadata_misses = self.debug.metadata_misses.saturating_add(1);
        let mut cursor = Cursor::new(&request.source_bytes);
        let metadata = SoundFont::metadata_only(&mut cursor).map_err(|error| {
            FlutzError::InvalidInput(format!(
                "failed to load subset metadata for {} ({}): {error:?}",
                request.internal_id,
                request.source_description()
            ))
        })?;
        self.metadata_entries.insert(key, Arc::new(metadata));
        Ok(())
    }

    fn compact_wave_data_from_ranges(
        &mut self,
        request: &SoundFontSubsetBytes,
        source_key: &str,
    ) -> Result<Option<Vec<i16>>> {
        if request.sample_ranges.is_empty() {
            return Ok(None);
        }
        let incoming = request
            .sample_ranges
            .iter()
            .map(|range| (range.sample_id, range))
            .collect::<BTreeMap<_, _>>();
        let mut wave_data = Vec::new();
        for sample_id in &request.closure.sample_ids {
            let key = sample_range_key(source_key, *sample_id);
            let samples = if let Some(cached) = self.sample_range_entries.get(&key) {
                self.debug.sample_range_hits = self.debug.sample_range_hits.saturating_add(1);
                Arc::clone(cached)
            } else {
                let Some(range) = incoming.get(sample_id) else {
                    return Ok(None);
                };
                self.debug.sample_range_misses = self.debug.sample_range_misses.saturating_add(1);
                let samples = Arc::new(range.samples.clone());
                self.sample_range_entries.insert(key, Arc::clone(&samples));
                samples
            };
            wave_data.extend_from_slice(&samples);
        }
        Ok(Some(wave_data))
    }

    pub fn release_unused(&mut self) {
        let before = self.entries.len();
        self.entries
            .retain(|_, soundfont| Arc::strong_count(soundfont) > 1);
        let subset_before = self.subset_entries.len();
        self.subset_entries
            .retain(|_, entry| Arc::strong_count(&entry.sound_font) > 1);
        let active_source_keys = self
            .subset_entries
            .values()
            .map(|entry| entry.source_key.clone())
            .collect::<BTreeSet<_>>();
        let active_sample_range_keys = self
            .subset_entries
            .values()
            .flat_map(|entry| {
                entry
                    .sample_ids
                    .iter()
                    .map(|sample_id| sample_range_key(&entry.source_key, *sample_id))
            })
            .collect::<BTreeSet<_>>();
        let metadata_before = self.metadata_entries.len();
        self.metadata_entries.retain(|key, metadata| {
            Arc::strong_count(metadata) > 1
                || key
                    .strip_prefix("metadata:")
                    .map(|source_key| active_source_keys.contains(source_key))
                    .unwrap_or(false)
        });
        let sample_range_before = self.sample_range_entries.len();
        self.sample_range_entries.retain(|key, samples| {
            Arc::strong_count(samples) > 1 || active_sample_range_keys.contains(key)
        });
        let removed = before
            .saturating_sub(self.entries.len())
            .saturating_add(subset_before.saturating_sub(self.subset_entries.len()))
            .saturating_add(metadata_before.saturating_sub(self.metadata_entries.len()))
            .saturating_add(sample_range_before.saturating_sub(self.sample_range_entries.len()));
        if removed > 0 {
            self.debug.evictions = self.debug.evictions.saturating_add(removed as u64);
            self.refresh_resident_bytes();
        }
    }

    pub fn clear(&mut self) {
        let removed = self
            .entries
            .len()
            .saturating_add(self.subset_entries.len())
            .saturating_add(self.metadata_entries.len())
            .saturating_add(self.sample_range_entries.len());
        self.entries.clear();
        self.metadata_entries.clear();
        self.subset_entries.clear();
        self.sample_range_entries.clear();
        if removed > 0 {
            self.debug.evictions = self.debug.evictions.saturating_add(removed as u64);
        }
        self.refresh_resident_bytes();
    }

    pub fn debug(&self) -> SoundFontRuntimeCacheDebug {
        let mut debug = self.debug.clone();
        debug.full_entries = self.entries.len();
        debug.metadata_entries = self.metadata_entries.len();
        debug.subset_entries = self.subset_entries.len();
        debug.sample_range_entries = self.sample_range_entries.len();
        debug.entries = debug
            .full_entries
            .saturating_add(debug.metadata_entries)
            .saturating_add(debug.subset_entries)
            .saturating_add(debug.sample_range_entries);
        let mut entry_refs = self
            .entries
            .iter()
            .map(|(key, soundfont)| SoundFontRuntimeCacheEntryDebug {
                key: key.clone(),
                layer: "full",
                strong_count: Arc::strong_count(soundfont),
                weak_count: Arc::weak_count(soundfont),
                resident_bytes: soundfont.get_wave_data().len() * mem::size_of::<i16>()
                    + soundfont.get_sample_headers().len() * mem::size_of::<usize>()
                    + soundfont.get_presets().len() * mem::size_of::<usize>(),
            })
            .collect::<Vec<_>>();
        entry_refs.extend(self.subset_entries.iter().map(|(key, entry)| {
            SoundFontRuntimeCacheEntryDebug {
                key: key.clone(),
                layer: "subset",
                strong_count: Arc::strong_count(&entry.sound_font),
                weak_count: Arc::weak_count(&entry.sound_font),
                resident_bytes: entry.sound_font.get_wave_data().len() * mem::size_of::<i16>()
                    + entry.sound_font.get_sample_headers().len() * mem::size_of::<usize>()
                    + entry.sound_font.get_presets().len() * mem::size_of::<usize>(),
            }
        }));
        entry_refs.extend(self.metadata_entries.iter().map(|(key, metadata)| {
            SoundFontRuntimeCacheEntryDebug {
                key: key.clone(),
                layer: "metadata",
                strong_count: Arc::strong_count(metadata),
                weak_count: Arc::weak_count(metadata),
                resident_bytes: metadata.get_sample_headers().len() * mem::size_of::<usize>()
                    + metadata.get_presets().len() * mem::size_of::<usize>()
                    + metadata.get_instruments().len() * mem::size_of::<usize>(),
            }
        }));
        entry_refs.extend(self.sample_range_entries.iter().map(|(key, samples)| {
            SoundFontRuntimeCacheEntryDebug {
                key: key.clone(),
                layer: "sample-range",
                strong_count: Arc::strong_count(samples),
                weak_count: Arc::weak_count(samples),
                resident_bytes: samples.len() * mem::size_of::<i16>(),
            }
        }));
        debug.entry_refs = entry_refs;
        debug.total_strong_count = debug
            .entry_refs
            .iter()
            .map(|entry| entry.strong_count)
            .sum();
        debug.max_strong_count = debug
            .entry_refs
            .iter()
            .map(|entry| entry.strong_count)
            .max()
            .unwrap_or_default();
        debug
    }

    fn refresh_resident_bytes(&mut self) {
        self.debug.full_entries = self.entries.len();
        self.debug.metadata_entries = self.metadata_entries.len();
        self.debug.subset_entries = self.subset_entries.len();
        self.debug.sample_range_entries = self.sample_range_entries.len();
        self.debug.entries = self
            .debug
            .full_entries
            .saturating_add(self.debug.metadata_entries)
            .saturating_add(self.debug.subset_entries)
            .saturating_add(self.debug.sample_range_entries);
        let full_bytes = self
            .entries
            .values()
            .map(|soundfont| {
                soundfont.get_wave_data().len() * mem::size_of::<i16>()
                    + soundfont.get_sample_headers().len() * mem::size_of::<usize>()
                    + soundfont.get_presets().len() * mem::size_of::<usize>()
            })
            .sum::<usize>();
        let subset_bytes = self
            .subset_entries
            .values()
            .map(|entry| {
                entry.sound_font.get_wave_data().len() * mem::size_of::<i16>()
                    + entry.sound_font.get_sample_headers().len() * mem::size_of::<usize>()
                    + entry.sound_font.get_presets().len() * mem::size_of::<usize>()
            })
            .sum::<usize>();
        let metadata_bytes = self
            .metadata_entries
            .values()
            .map(|metadata| {
                metadata.get_sample_headers().len() * mem::size_of::<usize>()
                    + metadata.get_presets().len() * mem::size_of::<usize>()
                    + metadata.get_instruments().len() * mem::size_of::<usize>()
            })
            .sum::<usize>();
        let sample_range_bytes = self
            .sample_range_entries
            .values()
            .map(|samples| samples.len() * mem::size_of::<i16>())
            .sum::<usize>();
        self.debug.resident_bytes = full_bytes
            .saturating_add(subset_bytes)
            .saturating_add(metadata_bytes)
            .saturating_add(sample_range_bytes);
    }
}

#[derive(Debug, Clone)]
pub struct LoadedMidi {
    pub byte_len: usize,
    midi_file: Arc<MidiFile>,
}

impl LoadedMidi {
    pub fn midi_file(&self) -> &Arc<MidiFile> {
        &self.midi_file
    }

    pub fn duration_seconds(&self) -> f64 {
        self.midi_file.get_length()
    }

    pub fn tick_length(&self) -> u64 {
        self.midi_file.get_tick_length().max(0) as u64
    }

    pub fn midi_interpretation(&self) -> &MidiInterpretation {
        self.midi_file.get_interpretation()
    }

    pub fn memory_debug(&self) -> MidiFileMemoryDebug {
        self.midi_file.memory_debug()
    }

    pub fn is_percussion_channel(&self, channel: u8) -> bool {
        self.midi_interpretation().is_percussion_channel(channel)
    }

    pub fn channel_program_roles(&self) -> Vec<MidiChannelProgramRole> {
        self.midi_file.get_channel_program_roles()
    }

    pub fn loop_start_ticks(&self) -> Vec<u64> {
        self.midi_file
            .get_loop_start_ticks()
            .into_iter()
            .map(|tick| tick.max(0) as u64)
            .collect()
    }

    pub fn loop_end_ticks(&self) -> Vec<u64> {
        self.midi_file
            .get_loop_end_ticks()
            .into_iter()
            .map(|tick| tick.max(0) as u64)
            .collect()
    }
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug)]
pub struct MultiSoundFontPlayback {
    state: PlaybackState,
    config: PlaybackConfig,
    midi: Option<LoadedMidi>,
    instances: Vec<SynthInstance>,
    loop_settings: PlaybackLoopSettings,
    last_stem_render_allocations: StemRenderAllocationDebug,
}

impl MultiSoundFontPlayback {
    fn ensure_instances_loaded(&mut self) -> Result<()> {
        let midi_file = Arc::clone(
            self.midi
                .as_ref()
                .ok_or_else(|| FlutzError::InvalidInput("no MIDI file is loaded".to_owned()))?
                .midi_file(),
        );
        for instance in &mut self.instances {
            if instance.sequencer.get_midi_file().is_none() {
                instance.sequencer.load_midi(&midi_file);
            }
        }
        Ok(())
    }

    pub fn new(soundfonts: Vec<SoundFontBytes>, config: PlaybackConfig) -> Result<Self> {
        Self::new_with_loader(soundfonts, config, parse_soundfont_bytes)
    }

    pub fn new_with_cache(
        soundfonts: Vec<SoundFontBytes>,
        config: PlaybackConfig,
        cache: &mut SoundFontRuntimeCache,
    ) -> Result<Self> {
        let loaded_soundfonts = cache.load_soundfonts_parallel(soundfonts)?;
        Self::new_with_loaded_soundfonts(loaded_soundfonts, config)
    }

    pub fn new_with_loaded_soundfonts(
        soundfonts: Vec<LoadedSoundFont>,
        config: PlaybackConfig,
    ) -> Result<Self> {
        if soundfonts.is_empty() {
            return Err(FlutzError::InvalidInput(
                "at least one soundfont is required for playback".to_owned(),
            ));
        }

        let mut settings = SynthesizerSettings::new(config.sample_rate as i32);
        settings.block_size = config.block_frames;
        settings.maximum_polyphony = config.maximum_polyphony;
        settings.enable_reverb_and_chorus = config.enable_reverb_and_chorus;

        let mut instances = Vec::with_capacity(soundfonts.len());
        for soundfont in soundfonts {
            let synthesizer =
                Synthesizer::new(&soundfont.sound_font, &settings).map_err(|error| {
                    FlutzError::Runtime(format!(
                        "failed to create RustySynth instance for {}: {error:?}",
                        soundfont.internal_id
                    ))
                })?;
            let coverage = extract_coverage_from_sf2(&soundfont.sound_font);
            let mut sequencer = MidiFileSequencer::new(synthesizer);
            sequencer.set_reposition_preroll_seconds(config.reposition_preroll_ms as f64 / 1000.0);
            instances.push(SynthInstance {
                internal_id: soundfont.internal_id,
                display_name: soundfont.display_name,
                coverage,
                sequencer,
                left: Vec::new(),
                right: Vec::new(),
                stem_blocks: Vec::new(),
            });
        }

        Ok(Self {
            state: PlaybackState::Stopped,
            config,
            midi: None,
            instances,
            loop_settings: PlaybackLoopSettings::default(),
            last_stem_render_allocations: StemRenderAllocationDebug::default(),
        })
    }

    fn new_with_loader(
        soundfonts: Vec<SoundFontBytes>,
        config: PlaybackConfig,
        mut load_soundfont: impl FnMut(SoundFontBytes) -> Result<Arc<SoundFont>>,
    ) -> Result<Self> {
        if soundfonts.is_empty() {
            return Err(FlutzError::InvalidInput(
                "at least one soundfont is required for playback".to_owned(),
            ));
        }

        let mut settings = SynthesizerSettings::new(config.sample_rate as i32);
        settings.block_size = config.block_frames;
        settings.maximum_polyphony = config.maximum_polyphony;
        settings.enable_reverb_and_chorus = config.enable_reverb_and_chorus;

        let mut instances = Vec::with_capacity(soundfonts.len());
        for soundfont in soundfonts {
            let internal_id = soundfont.internal_id.clone();
            let display_name = soundfont.display_name.clone();
            let sound_font = load_soundfont(soundfont)?;
            let synthesizer = Synthesizer::new(&sound_font, &settings).map_err(|error| {
                FlutzError::Runtime(format!(
                    "failed to create RustySynth instance for {internal_id}: {error:?}"
                ))
            })?;
            let coverage = extract_coverage_from_sf2(&sound_font);
            let mut sequencer = MidiFileSequencer::new(synthesizer);
            sequencer.set_reposition_preroll_seconds(config.reposition_preroll_ms as f64 / 1000.0);
            instances.push(SynthInstance {
                internal_id,
                display_name,
                coverage,
                sequencer,
                left: Vec::new(),
                right: Vec::new(),
                stem_blocks: Vec::new(),
            });
        }

        Ok(Self {
            state: PlaybackState::Stopped,
            config,
            midi: None,
            instances,
            loop_settings: PlaybackLoopSettings::default(),
            last_stem_render_allocations: StemRenderAllocationDebug::default(),
        })
    }

    pub fn load_midi_bytes(&mut self, bytes: &[u8]) -> Result<LoadedMidi> {
        self.load_midi_bytes_with_loop_type(bytes, MidiFileLoopType::LoopPoint(0))
    }

    pub fn load_midi_bytes_with_loop_type(
        &mut self,
        bytes: &[u8],
        loop_type: MidiFileLoopType,
    ) -> Result<LoadedMidi> {
        let midi = validate_midi_bytes_with_loop_type(bytes, loop_type)?;
        self.midi = Some(midi.clone());
        self.stop();
        self.set_loop_settings(self.loop_settings);
        Ok(midi)
    }

    pub fn play(&mut self) -> Result<()> {
        self.ensure_instances_loaded()?;
        let loop_settings = self.loop_settings.as_sequencer_settings();
        for instance in &mut self.instances {
            instance.sequencer.set_loop_settings(loop_settings);
        }
        self.state = PlaybackState::Playing;
        Ok(())
    }

    pub fn pause(&mut self) {
        if self.state == PlaybackState::Playing {
            self.state = PlaybackState::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == PlaybackState::Paused {
            self.state = PlaybackState::Playing;
        }
    }

    pub fn stop(&mut self) {
        for instance in &mut self.instances {
            instance.sequencer.stop();
        }
        self.last_stem_render_allocations = StemRenderAllocationDebug::default();
        self.state = PlaybackState::Stopped;
    }

    pub fn render_interleaved_stereo(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let frames = output.len() / 2;
        if frames == 0 {
            return;
        }

        let blocks = self.render_soundfont_blocks(frames);
        let gain = 1.0 / blocks.len().max(1) as f32;
        for block in blocks {
            for frame in 0..frames {
                output[frame * 2] += block.left[frame] * gain;
                output[frame * 2 + 1] += block.right[frame] * gain;
            }
        }
    }

    pub fn render_soundfont_blocks(&mut self, frames: usize) -> Vec<StemRenderBlock> {
        if self.state != PlaybackState::Playing || frames == 0 {
            self.last_stem_render_allocations = StemRenderAllocationDebug::default();
            return Vec::new();
        }

        let mut ended = true;
        let mut blocks = Vec::with_capacity(self.instances.len());
        for instance in &mut self.instances {
            instance.render(frames);
            ended = ended && instance.sequencer.end_of_sequence();
            blocks.push(StemRenderBlock::whole_soundfont_named(
                instance.internal_id.clone(),
                instance.display_name.clone(),
                instance.left.clone(),
                instance.right.clone(),
            ));
        }

        if ended {
            self.stop();
        }
        blocks
    }

    pub fn render_channel_program_stem_blocks(&mut self, frames: usize) -> Vec<StemRenderBlock> {
        let mut blocks = Vec::new();
        self.render_channel_program_stem_blocks_into(frames, &mut blocks);
        blocks
    }

    pub fn render_channel_program_stem_blocks_into(
        &mut self,
        frames: usize,
        blocks: &mut Vec<StemRenderBlock>,
    ) {
        let mut active = vec![false; blocks.len()];
        for block in blocks.iter_mut() {
            block.active_notes.clear();
            block.left.resize(frames, 0.0);
            block.right.resize(frames, 0.0);
            block.left.fill(0.0);
            block.right.fill(0.0);
        }
        if self.state != PlaybackState::Playing || frames == 0 {
            blocks.clear();
            self.last_stem_render_allocations = StemRenderAllocationDebug::default();
            return;
        }

        let mut ended = true;
        let mut stem_allocations = StemRenderAllocationDebug::default();
        for instance in &mut self.instances {
            for block in instance.render_channel_program_stems(frames) {
                upsert_stem_block(blocks, &mut active, block, frames);
            }
            stem_allocations.add_assign(&instance.sequencer.last_stem_render_allocations());
            ended = ended && instance.sequencer.end_of_sequence();
        }
        self.last_stem_render_allocations = stem_allocations;

        let mut index = 0;
        blocks.retain(|_| {
            let keep = active.get(index).copied().unwrap_or(false);
            index += 1;
            keep
        });

        if ended {
            self.stop();
        }
    }

    pub fn state(&self) -> PlaybackState {
        self.state
    }

    pub fn position_seconds(&self) -> f64 {
        self.instances
            .iter()
            .map(|instance| instance.sequencer.get_position())
            .fold(0.0, f64::max)
    }

    pub fn position_ticks(&self) -> u64 {
        self.instances
            .iter()
            .map(|instance| instance.sequencer.get_tick_position().max(0) as u64)
            .max()
            .unwrap_or(0)
    }

    pub fn duration_seconds(&self) -> f64 {
        self.midi
            .as_ref()
            .map(LoadedMidi::duration_seconds)
            .unwrap_or(0.0)
    }

    pub fn seek_to_fraction(&mut self, fraction: f64) -> Result<()> {
        self.ensure_instances_loaded()?;
        let duration = self.duration_seconds();
        if duration <= 0.0 {
            return Ok(());
        }

        let clamped_fraction = fraction.clamp(0.0, 1.0);
        let target_seconds = duration * clamped_fraction;
        for instance in &mut self.instances {
            instance.sequencer.seek_to_seconds(target_seconds);
        }
        Ok(())
    }

    pub fn seek_to_tick(&mut self, tick: u64) -> Result<()> {
        self.ensure_instances_loaded()?;
        let clamped_tick = tick.min(i32::MAX as u64) as i32;
        for instance in &mut self.instances {
            instance.sequencer.seek_to_tick(clamped_tick);
        }
        Ok(())
    }

    pub fn seek_to_seconds(&mut self, seconds: f64) -> Result<()> {
        self.ensure_instances_loaded()?;
        for instance in &mut self.instances {
            instance.sequencer.seek_to_seconds(seconds);
        }
        Ok(())
    }

    pub fn set_loop_enabled(&mut self, enabled: bool) {
        self.loop_settings.enabled = enabled;
        if enabled && matches!(self.loop_settings.mode, PlaybackLoopMode::None) {
            self.loop_settings.mode = PlaybackLoopMode::Infinite;
        }
        self.set_loop_settings(self.loop_settings);
    }

    pub fn set_loop_settings(&mut self, settings: PlaybackLoopSettings) {
        self.loop_settings = settings;
        let runtime_settings = settings.as_sequencer_settings();
        let mut should_stop = false;
        for instance in &mut self.instances {
            should_stop |= instance.sequencer.set_loop_settings(runtime_settings);
        }
        if should_stop {
            self.stop();
        }
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    pub fn soundfont_coverages(&self) -> Vec<(String, SoundFontCoverage)> {
        self.instances
            .iter()
            .map(|instance| (instance.internal_id.clone(), instance.coverage.clone()))
            .collect()
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn memory_debug(&self) -> PlaybackMemoryDebug {
        let instances = self
            .instances
            .iter()
            .map(SynthInstance::memory_debug)
            .collect::<Vec<_>>();
        PlaybackMemoryDebug::from_instances(
            instances,
            self.midi.as_ref().map(LoadedMidi::memory_debug),
        )
    }

    pub fn last_stem_render_allocations(&self) -> StemRenderAllocationDebug {
        self.last_stem_render_allocations
    }
}

#[derive(Debug, Clone, Default)]
pub struct PlaybackMemoryDebug {
    pub instance_count: usize,
    pub soundfont_wave_bytes: usize,
    pub soundfont_metadata_items: usize,
    pub total_voices: usize,
    pub active_voices: usize,
    pub max_active_voices: usize,
    pub total_voice_requests: u64,
    pub exclusive_class_reuses: u64,
    pub free_voice_allocations: u64,
    pub contention_steals: u64,
    pub env_delay_voices: usize,
    pub env_attack_voices: usize,
    pub env_hold_voices: usize,
    pub env_decay_voices: usize,
    pub env_release_voices: usize,
    pub env_value_sum: f32,
    pub env_value_avg: f32,
    pub voice_buffer_bytes: usize,
    pub block_buffer_bytes: usize,
    pub retained_stem_blocks: usize,
    pub retained_stem_block_bytes: usize,
    pub stem_effects: usize,
    pub stem_effect_bytes: usize,
    pub stem_effect_allocations: u64,
    pub stem_effect_deallocations: u64,
    pub stem_effect_allocated_bytes: u64,
    pub stem_effect_deallocated_bytes: u64,
    pub stem_effect_cache_clears: u64,
    pub stem_effect_cache_released_bytes: u64,
    pub last_stem_render_allocations: StemRenderAllocationDebug,
    pub effects_bytes: usize,
    pub preset_lookup_bytes: usize,
    pub channel_bytes: usize,
    pub stem_effect_map_bytes: usize,
    pub block_stem_vec_bytes: usize,
    pub estimated_bytes: usize,
    pub midi_file: Option<MidiFileMemoryDebug>,
    pub instances: Vec<SynthInstanceMemoryDebug>,
}

impl PlaybackMemoryDebug {
    fn from_instances(
        instances: Vec<SynthInstanceMemoryDebug>,
        midi_file: Option<MidiFileMemoryDebug>,
    ) -> Self {
        Self {
            instance_count: instances.len(),
            soundfont_wave_bytes: instances
                .iter()
                .map(|instance| instance.synth.soundfont_wave_bytes)
                .sum(),
            soundfont_metadata_items: instances
                .iter()
                .map(|instance| instance.synth.soundfont_metadata_items)
                .sum(),
            total_voices: instances
                .iter()
                .map(|instance| instance.synth.total_voices)
                .sum(),
            active_voices: instances
                .iter()
                .map(|instance| instance.synth.active_voices)
                .sum(),
            max_active_voices: instances
                .iter()
                .map(|instance| instance.synth.max_active_voices)
                .sum(),
            total_voice_requests: instances
                .iter()
                .map(|instance| instance.synth.total_voice_requests)
                .sum(),
            exclusive_class_reuses: instances
                .iter()
                .map(|instance| instance.synth.exclusive_class_reuses)
                .sum(),
            free_voice_allocations: instances
                .iter()
                .map(|instance| instance.synth.free_voice_allocations)
                .sum(),
            contention_steals: instances
                .iter()
                .map(|instance| instance.synth.contention_steals)
                .sum(),
            env_delay_voices: instances
                .iter()
                .map(|instance| instance.synth.env_delay_voices)
                .sum(),
            env_attack_voices: instances
                .iter()
                .map(|instance| instance.synth.env_attack_voices)
                .sum(),
            env_hold_voices: instances
                .iter()
                .map(|instance| instance.synth.env_hold_voices)
                .sum(),
            env_decay_voices: instances
                .iter()
                .map(|instance| instance.synth.env_decay_voices)
                .sum(),
            env_release_voices: instances
                .iter()
                .map(|instance| instance.synth.env_release_voices)
                .sum(),
            env_value_sum: instances
                .iter()
                .map(|instance| instance.synth.env_value_sum)
                .sum(),
            env_value_avg: {
                let active_voices: usize = instances
                    .iter()
                    .map(|instance| instance.synth.active_voices)
                    .sum();
                let value_sum: f32 = instances
                    .iter()
                    .map(|instance| instance.synth.env_value_sum)
                    .sum();
                if active_voices > 0 {
                    value_sum / active_voices as f32
                } else {
                    0.0
                }
            },
            voice_buffer_bytes: instances
                .iter()
                .map(|instance| instance.synth.voice_buffer_bytes)
                .sum(),
            block_buffer_bytes: instances
                .iter()
                .map(|instance| instance.synth.block_buffer_bytes)
                .sum(),
            retained_stem_blocks: instances
                .iter()
                .map(|instance| instance.synth.retained_stem_blocks)
                .sum(),
            retained_stem_block_bytes: instances
                .iter()
                .map(|instance| instance.synth.retained_stem_block_bytes)
                .sum(),
            stem_effects: instances
                .iter()
                .map(|instance| instance.synth.stem_effects)
                .sum(),
            stem_effect_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_bytes)
                .sum(),
            stem_effect_allocations: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_allocations)
                .sum(),
            stem_effect_deallocations: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_deallocations)
                .sum(),
            stem_effect_allocated_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_allocated_bytes)
                .sum(),
            stem_effect_deallocated_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_deallocated_bytes)
                .sum(),
            stem_effect_cache_clears: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_cache_clears)
                .sum(),
            stem_effect_cache_released_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_cache_released_bytes)
                .sum(),
            last_stem_render_allocations: instances.iter().fold(
                StemRenderAllocationDebug::default(),
                |mut allocations, instance| {
                    allocations.add_assign(&instance.synth.last_stem_render_allocations);
                    allocations
                },
            ),
            effects_bytes: instances
                .iter()
                .map(|instance| instance.synth.effects_bytes)
                .sum(),
            preset_lookup_bytes: instances
                .iter()
                .map(|instance| instance.synth.preset_lookup_bytes)
                .sum(),
            channel_bytes: instances
                .iter()
                .map(|instance| instance.synth.channel_bytes)
                .sum(),
            stem_effect_map_bytes: instances
                .iter()
                .map(|instance| instance.synth.stem_effect_map_bytes)
                .sum(),
            block_stem_vec_bytes: instances
                .iter()
                .map(|instance| instance.synth.block_stem_vec_bytes)
                .sum(),
            estimated_bytes: instances
                .iter()
                .map(|instance| instance.synth.estimated_bytes)
                .sum::<usize>()
                .saturating_add(
                    midi_file
                        .map(|debug| debug.estimated_bytes)
                        .unwrap_or_default(),
                ),
            midi_file,
            instances,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SynthInstanceMemoryDebug {
    pub internal_id: String,
    pub display_name: String,
    pub synth: SynthesizerMemoryDebug,
}

#[derive(Debug)]
struct SynthInstance {
    internal_id: String,
    display_name: String,
    coverage: SoundFontCoverage,
    sequencer: MidiFileSequencer,
    left: Vec<f32>,
    right: Vec<f32>,
    stem_blocks: Vec<StemRenderBlock>,
}

impl SynthInstance {
    fn render(&mut self, frames: usize) {
        self.left.resize(frames, 0.0);
        self.right.resize(frames, 0.0);
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.sequencer.render(&mut self.left, &mut self.right);
    }

    fn render_channel_program_stems(&mut self, frames: usize) -> &[StemRenderBlock] {
        let request = StemRenderRequest::channel_program(self.internal_id.clone());
        self.sequencer
            .render_stems_into(&request, frames, &mut self.stem_blocks);
        for block in &mut self.stem_blocks {
            if block.display_name.is_none() && block.identity.mode().requires_voice_grouping() {
                let channel = block.identity.midi_channel.unwrap_or(0);
                let program = block.identity.midi_program.unwrap_or(0);
                block.display_name = Some(format!("{} Ch {channel} P{program}", self.display_name));
            } else if block.display_name.as_deref() == Some("Global effects") {
                block.display_name = Some(format!("{} Global effects", self.display_name));
            }
        }
        &self.stem_blocks
    }

    fn memory_debug(&self) -> SynthInstanceMemoryDebug {
        SynthInstanceMemoryDebug {
            internal_id: self.internal_id.clone(),
            display_name: self.display_name.clone(),
            synth: self.sequencer.memory_debug(),
        }
    }
}

fn upsert_stem_block(
    blocks: &mut Vec<StemRenderBlock>,
    active: &mut Vec<bool>,
    source: &StemRenderBlock,
    frames: usize,
) {
    let index = match blocks
        .iter()
        .position(|existing| existing.identity == source.identity)
    {
        Some(index) => index,
        None => {
            blocks.push(StemRenderBlock {
                identity: source.identity.clone(),
                display_name: source.display_name.clone(),
                active_notes: Vec::new(),
                left: Vec::new(),
                right: Vec::new(),
            });
            active.push(false);
            blocks.len() - 1
        }
    };

    let target = &mut blocks[index];
    target.display_name.clone_from(&source.display_name);
    target.active_notes.clear();
    target
        .active_notes
        .extend(source.active_notes.iter().copied());
    target.left.resize(frames, 0.0);
    target.right.resize(frames, 0.0);
    target.left.copy_from_slice(&source.left);
    target.right.copy_from_slice(&source.right);
    active[index] = true;
}

pub fn validate_midi_bytes(bytes: &[u8]) -> Result<LoadedMidi> {
    validate_midi_bytes_with_loop_type(bytes, MidiFileLoopType::LoopPoint(0))
}

pub fn validate_midi_bytes_with_loop_type(
    bytes: &[u8],
    loop_type: MidiFileLoopType,
) -> Result<LoadedMidi> {
    if !bytes.starts_with(b"MThd") {
        return Err(FlutzError::InvalidInput(
            "missing standard MIDI MThd header".to_owned(),
        ));
    }
    let mut cursor = Cursor::new(bytes);
    let midi_file = MidiFile::new_with_loop_type(&mut cursor, loop_type)
        .map_err(|error| FlutzError::InvalidInput(format!("invalid MIDI file: {error:?}")))?;
    Ok(LoadedMidi {
        byte_len: bytes.len(),
        midi_file: Arc::new(midi_file),
    })
}

fn parse_soundfont_bytes(soundfont: SoundFontBytes) -> Result<Arc<SoundFont>> {
    let internal_id = soundfont.internal_id.clone();
    let source_description = soundfont.source_description();
    let mut cursor = Cursor::new(soundfont.bytes);
    Ok(Arc::new(SoundFont::new(&mut cursor).map_err(|error| {
        FlutzError::InvalidInput(format!(
            "failed to load DAT-sourced soundfont {internal_id} ({source_description}): {error:?}; DAT soundfont bytes are expected to be RustyStem/RustySynth-compatible, so revalidate DAT packing/unpacking byte identity for this entry"
        ))
    })?))
}

fn build_subset_soundfont(
    request: SoundFontSubsetBytes,
    compact_wave_data: Option<Vec<i16>>,
) -> Result<Arc<SoundFont>> {
    let internal_id = request.internal_id.clone();
    let source_description = request.source_description();
    if let Some(wave_data) = compact_wave_data {
        let mut cursor = Cursor::new(&request.source_bytes);
        let metadata = SoundFont::metadata_only(&mut cursor).map_err(|error| {
            FlutzError::InvalidInput(format!(
                "failed to load subset metadata for {internal_id} ({source_description}): {error:?}; subset assembly requires RustyStem/RustySynth-compatible SF2 metadata bytes"
            ))
        })?;
        let compact = metadata
            .compact_from_closure_and_wave_data(&request.closure, wave_data)
            .map_err(|error| {
                FlutzError::InvalidInput(format!(
                    "failed to assemble subset soundfont {internal_id} ({source_description}) from metadata and sample ranges: {error:?}"
                ))
            })?;
        return Ok(Arc::new(compact));
    }

    let mut cursor = Cursor::new(&request.source_bytes);
    let source = SoundFont::new(&mut cursor).map_err(|error| {
        FlutzError::InvalidInput(format!(
            "failed to load subset source soundfont {internal_id} ({source_description}): {error:?}; subset assembly requires RustyStem/RustySynth-compatible SF2 source bytes"
        ))
    })?;
    let compact = source
        .compact_from_closure(&request.closure)
        .map_err(|error| {
            FlutzError::InvalidInput(format!(
            "failed to assemble subset soundfont {internal_id} ({source_description}): {error:?}"
        ))
        })?;
    Ok(Arc::new(compact))
}

#[derive(Debug)]
struct ParsedSoundFont {
    index: usize,
    key: String,
    internal_id: String,
    display_name: String,
    sound_font: Arc<SoundFont>,
}

#[derive(Debug)]
struct ParsedSubsetSoundFont {
    index: usize,
    key: String,
    source_key: String,
    sample_ids: BTreeSet<usize>,
    internal_id: String,
    display_name: String,
    sound_font: Arc<SoundFont>,
}

fn parse_soundfonts_parallel(
    tasks: Vec<(usize, String, SoundFontBytes)>,
) -> Result<Vec<ParsedSoundFont>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }

    let worker_count = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, 4)
        .min(tasks.len());
    let mut task_groups = (0..worker_count)
        .map(|_| Vec::new())
        .collect::<Vec<Vec<(usize, String, SoundFontBytes)>>>();
    for (index, task) in tasks.into_iter().enumerate() {
        task_groups[index % worker_count].push(task);
    }

    let mut workers = Vec::with_capacity(worker_count);
    for task_group in task_groups {
        workers.push(thread::spawn(move || {
            let mut parsed = Vec::with_capacity(task_group.len());
            for (index, key, soundfont) in task_group {
                let internal_id = soundfont.internal_id.clone();
                let display_name = soundfont.display_name.clone();
                let sound_font = parse_soundfont_bytes(soundfont)?;
                parsed.push(ParsedSoundFont {
                    index,
                    key,
                    internal_id,
                    display_name,
                    sound_font,
                });
            }
            Ok::<_, FlutzError>(parsed)
        }));
    }

    let mut parsed = Vec::new();
    for worker in workers {
        match worker.join() {
            Ok(Ok(mut worker_parsed)) => parsed.append(&mut worker_parsed),
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(FlutzError::Runtime(
                    "soundfont parse worker thread panicked".to_owned(),
                ));
            }
        }
    }
    parsed.sort_by_key(|soundfont| soundfont.index);
    Ok(parsed)
}

fn parse_subset_soundfonts_parallel(
    tasks: Vec<(
        usize,
        String,
        String,
        BTreeSet<usize>,
        SoundFontSubsetBytes,
        Option<Vec<i16>>,
    )>,
) -> Result<Vec<ParsedSubsetSoundFont>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }

    let worker_count = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, 4)
        .min(tasks.len());
    let mut task_groups = (0..worker_count).map(|_| Vec::new()).collect::<Vec<
        Vec<(
            usize,
            String,
            String,
            BTreeSet<usize>,
            SoundFontSubsetBytes,
            Option<Vec<i16>>,
        )>,
    >>();
    for (index, task) in tasks.into_iter().enumerate() {
        task_groups[index % worker_count].push(task);
    }

    let mut workers = Vec::with_capacity(worker_count);
    for group in task_groups {
        workers.push(thread::spawn(move || {
            let mut parsed = Vec::new();
            for (index, key, source_key, sample_ids, request, compact_wave_data) in group {
                let internal_id = request.internal_id.clone();
                let display_name = request.display_name.clone();
                let sound_font = build_subset_soundfont(request, compact_wave_data)?;
                parsed.push(ParsedSubsetSoundFont {
                    index,
                    key,
                    source_key,
                    sample_ids,
                    internal_id,
                    display_name,
                    sound_font,
                });
            }
            Ok::<_, FlutzError>(parsed)
        }));
    }

    let mut parsed = Vec::new();
    for worker in workers {
        match worker.join() {
            Ok(Ok(mut worker_parsed)) => parsed.append(&mut worker_parsed),
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(FlutzError::Runtime(
                    "subset soundfont parse worker thread panicked".to_owned(),
                ));
            }
        }
    }
    parsed.sort_by_key(|parsed| parsed.index);
    Ok(parsed)
}
