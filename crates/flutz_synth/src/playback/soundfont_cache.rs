use super::*;

#[derive(Debug, Clone)]
pub struct SoundFontBytes {
    pub(super) internal_id: String,
    pub(super) display_name: String,
    pub(super) bytes: Vec<u8>,
    pub(super) source: SoundFontDataSource,
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

    /// Sample data of every cached SoundFont a live engine still references.
    pub fn live_wave_data(&self) -> impl Iterator<Item = &[i16]> + '_ {
        let full = self
            .entries
            .values()
            .filter(|soundfont| Arc::strong_count(soundfont) > 1)
            .map(|soundfont| soundfont.get_wave_data());
        let subset = self
            .subset_entries
            .values()
            .filter(|entry| Arc::strong_count(&entry.sound_font) > 1)
            .map(|entry| entry.sound_font.get_wave_data());
        full.chain(subset)
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

pub(super) fn parse_soundfont_bytes(soundfont: SoundFontBytes) -> Result<Arc<SoundFont>> {
    let internal_id = soundfont.internal_id.clone();
    let source_description = soundfont.source_description();
    let mut cursor = Cursor::new(soundfont.bytes);
    Ok(Arc::new(SoundFont::new(&mut cursor).map_err(|error| {
        FlutzError::InvalidInput(format!(
            "failed to load DAT-sourced soundfont {internal_id} ({source_description}): {error:?}; DAT soundfont bytes are expected to be RustyStem/RustySynth-compatible, so revalidate DAT packing/unpacking byte identity for this entry"
        ))
    })?))
}

pub(super) fn build_subset_soundfont(
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
pub(super) struct ParsedSoundFont {
    index: usize,
    key: String,
    internal_id: String,
    display_name: String,
    sound_font: Arc<SoundFont>,
}

#[derive(Debug)]
pub(super) struct ParsedSubsetSoundFont {
    index: usize,
    key: String,
    source_key: String,
    sample_ids: BTreeSet<usize>,
    internal_id: String,
    display_name: String,
    sound_font: Arc<SoundFont>,
}

pub(super) fn parse_soundfonts_parallel(
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

pub(super) fn parse_subset_soundfonts_parallel(
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
