use super::super::*;

#[derive(Debug)]
pub(in crate::playback) struct SoundFontDatReadTask {
    pub(in crate::playback) order: usize,
    pub(in crate::playback) soundfont_id: String,
    pub(in crate::playback) dat_path: PathBuf,
    pub(in crate::playback) index: DatArchiveIndex,
    pub(in crate::playback) record: DatEntryRecord,
}

#[derive(Debug)]
pub(in crate::playback) struct SoundFontDatPayload {
    pub(in crate::playback) bytes: Vec<u8>,
    pub(in crate::playback) sources: Vec<String>,
    pub(in crate::playback) entry_reads: usize,
    pub(in crate::playback) entry_read_bytes: usize,
}

#[derive(Debug)]
pub(in crate::playback) struct SoundFontSubsetDatPayload {
    pub(in crate::playback) metadata_bytes: Vec<u8>,
    pub(in crate::playback) sample_ranges: Vec<SoundFontSubsetSampleRange>,
    pub(in crate::playback) sources: Vec<String>,
    pub(in crate::playback) range_reads: usize,
    pub(in crate::playback) range_read_bytes: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontSubsetPlanDebug {
    pub soundfont_id: String,
    pub requested_role_count: usize,
    pub preset_ids: Vec<u32>,
    pub instrument_ids: Vec<u32>,
    pub sample_ids: Vec<u32>,
    pub logical_wave_range_count: usize,
    pub planned_range_count: usize,
    pub planned_byte_count: u64,
    pub used_index_json: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedSubsetState {
    pub soundfont_id: String,
    pub demand_signature: SoundFontDemandSignature,
    pub preset_ids: Vec<u32>,
    pub instrument_ids: Vec<u32>,
    pub sample_ids: Vec<u32>,
    pub planned_byte_count: u64,
    pub compact_loaded: bool,
}

impl LoadedSubsetState {
    pub(in crate::playback) fn from_plan(
        plan: &SoundFontSubsetPlanDebug,
        compact_loaded: bool,
    ) -> Self {
        Self {
            soundfont_id: plan.soundfont_id.clone(),
            demand_signature: SoundFontDemandSignature::from_subset_plan(plan),
            preset_ids: plan.preset_ids.clone(),
            instrument_ids: plan.instrument_ids.clone(),
            sample_ids: plan.sample_ids.clone(),
            planned_byte_count: plan.planned_byte_count,
            compact_loaded,
        }
    }

    pub(in crate::playback) fn sample_set(&self) -> BTreeSet<u32> {
        self.sample_ids.iter().copied().collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackSubsetTransitionDiagnostics {
    pub exact_subset_hits: usize,
    pub subset_contained_hits: usize,
    pub superset_growth_events: usize,
    pub font_set_changes: usize,
    pub missing_sample_count: usize,
    pub missing_planned_bytes: u64,
    pub compact_loaded_count: usize,
    pub full_fallback_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackSoundFontLoadDiagnostics {
    pub load_invocations: u64,
    pub requested_font_count: usize,
    pub dat_read_task_count: usize,
    pub full_dat_entry_reads: usize,
    pub range_dat_reads: usize,
    pub full_dat_entry_bytes: u64,
    pub range_dat_read_bytes: u64,
    pub subset_request_count: usize,
    pub subset_source_clone_bytes: u64,
    pub full_fallback_count: usize,
    pub non_subset_full_load_count: usize,
    pub json_resource_font_count: usize,
    pub index_json_plan_count: usize,
    pub load_duration_ms: u128,
}

pub(in crate::playback) fn compare_subset_transition(
    previous: &BTreeMap<String, LoadedSubsetState>,
    current: &BTreeMap<String, LoadedSubsetState>,
) -> PlaybackSubsetTransitionDiagnostics {
    let mut diagnostics = PlaybackSubsetTransitionDiagnostics::default();
    for (soundfont_id, current_state) in current {
        let Some(previous_state) = previous.get(soundfont_id) else {
            diagnostics.font_set_changes = diagnostics.font_set_changes.saturating_add(1);
            diagnostics.missing_sample_count = diagnostics
                .missing_sample_count
                .saturating_add(current_state.sample_ids.len());
            diagnostics.missing_planned_bytes = diagnostics
                .missing_planned_bytes
                .saturating_add(current_state.planned_byte_count);
            continue;
        };
        let previous_samples = previous_state.sample_set();
        let current_samples = current_state.sample_set();
        if previous_samples == current_samples {
            diagnostics.exact_subset_hits = diagnostics.exact_subset_hits.saturating_add(1);
        } else if current_samples.is_subset(&previous_samples) {
            diagnostics.subset_contained_hits = diagnostics.subset_contained_hits.saturating_add(1);
        } else if current_samples.is_superset(&previous_samples) {
            diagnostics.superset_growth_events =
                diagnostics.superset_growth_events.saturating_add(1);
            diagnostics.missing_sample_count = diagnostics
                .missing_sample_count
                .saturating_add(current_samples.difference(&previous_samples).count());
            diagnostics.missing_planned_bytes = diagnostics.missing_planned_bytes.saturating_add(
                current_state
                    .planned_byte_count
                    .saturating_sub(previous_state.planned_byte_count),
            );
        } else {
            diagnostics.font_set_changes = diagnostics.font_set_changes.saturating_add(1);
        }
    }
    diagnostics.compact_loaded_count = current
        .values()
        .filter(|state| state.compact_loaded)
        .count();
    diagnostics.full_fallback_count = current
        .values()
        .filter(|state| !state.compact_loaded)
        .count();
    diagnostics
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundFontSubsetPlanAggregateDebug {
    pub plan_count: usize,
    pub preset_count: usize,
    pub instrument_count: usize,
    pub sample_count: usize,
    pub logical_wave_range_count: usize,
    pub planned_range_count: usize,
    pub planned_byte_count: u64,
    pub index_json_plan_count: usize,
    pub signatures: Vec<SoundFontDemandSignature>,
}

impl SoundFontSubsetPlanAggregateDebug {
    pub(in crate::playback) fn from_plans(
        plans: &BTreeMap<String, SoundFontSubsetPlanDebug>,
    ) -> Self {
        Self {
            plan_count: plans.len(),
            preset_count: plans.values().map(|plan| plan.preset_ids.len()).sum(),
            instrument_count: plans.values().map(|plan| plan.instrument_ids.len()).sum(),
            sample_count: plans.values().map(|plan| plan.sample_ids.len()).sum(),
            logical_wave_range_count: plans
                .values()
                .map(|plan| plan.logical_wave_range_count)
                .sum(),
            planned_range_count: plans.values().map(|plan| plan.planned_range_count).sum(),
            planned_byte_count: plans.values().map(|plan| plan.planned_byte_count).sum(),
            index_json_plan_count: plans.values().filter(|plan| plan.used_index_json).count(),
            signatures: plans
                .values()
                .map(SoundFontDemandSignature::from_subset_plan)
                .collect(),
        }
    }
}

pub(in crate::playback) fn read_soundfont_dat_payloads(
    tasks: &[&SoundFontDatReadTask],
) -> Result<SoundFontDatPayload> {
    if tasks.is_empty() {
        return Ok(SoundFontDatPayload {
            bytes: Vec::new(),
            sources: Vec::new(),
            entry_reads: 0,
            entry_read_bytes: 0,
        });
    }

    let mut sorted_tasks = tasks.to_vec();
    sorted_tasks.sort_by_key(|task| task.order);
    let mut bytes = Vec::new();
    let mut sources = Vec::new();
    let mut entry_read_bytes = 0usize;
    for task in sorted_tasks {
        let source = task.dat_path.display().to_string();
        let payload = extract_entry_from_file(&task.dat_path, &task.index, &task.record)?;
        entry_read_bytes = entry_read_bytes.saturating_add(payload.len());
        bytes.extend_from_slice(&payload);
        if !sources.contains(&source) {
            sources.push(source);
        }
    }
    Ok(SoundFontDatPayload {
        bytes,
        sources,
        entry_reads: tasks.len(),
        entry_read_bytes,
    })
}

pub(in crate::playback) fn read_soundfont_subset_dat_payload(
    tasks: &[&SoundFontDatReadTask],
    index: &SoundFontIndexJson,
    plan: &SoundFontSubsetPlanDebug,
) -> Result<SoundFontSubsetDatPayload> {
    if tasks.is_empty() {
        return Err(FlutzError::InvalidInput(
            "soundfont DAT subset has no DAT entries".to_owned(),
        ));
    }
    let mut sorted_tasks = tasks.to_vec();
    sorted_tasks.sort_by_key(|task| task.order);
    let parts = sorted_tasks
        .iter()
        .map(|task| SplitDatEntryPart {
            path: task.dat_path.as_path(),
            index: &task.index,
            entry: &task.record,
        })
        .collect::<Vec<_>>();

    let metadata = read_soundfont_thin_metadata_from_files(&parts, index)?;
    let samples = read_soundfont_sample_ranges_from_files(&parts, index, &plan.sample_ids)?;
    let mut sample_ranges = Vec::with_capacity(samples.samples.len());
    for sample in samples.samples {
        sample_ranges.push(SoundFontSubsetSampleRange {
            sample_id: sample.sample_id as usize,
            samples: decode_smpl_i16(&sample.bytes)?,
        });
    }

    Ok(SoundFontSubsetDatPayload {
        metadata_bytes: metadata.bytes,
        sample_ranges,
        sources: soundfont_dat_sources(tasks),
        range_reads: metadata.read_count.saturating_add(samples.read_count),
        range_read_bytes: metadata.byte_count.saturating_add(samples.byte_count),
    })
}

fn decode_smpl_i16(bytes: &[u8]) -> Result<Vec<i16>> {
    if bytes.len() % 2 != 0 {
        return Err(FlutzError::InvalidInput(
            "soundfont sample range has odd byte length".to_owned(),
        ));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}

fn soundfont_dat_sources(tasks: &[&SoundFontDatReadTask]) -> Vec<String> {
    let mut sources = Vec::new();
    for task in tasks {
        let source = task.dat_path.display().to_string();
        if !sources.contains(&source) {
            sources.push(source);
        }
    }
    sources
}

pub(in crate::playback) fn soundfont_json_resources_present(
    resources: &SoundFontJsonResources,
) -> bool {
    resources.metadata.is_some()
        || resources.coverage.is_some()
        || resources.index.is_some()
        || resources.pack_report.is_some()
}

pub(in crate::playback) fn soundfont_json_resources_complete(
    resources: &SoundFontJsonResources,
) -> bool {
    resources.coverage.is_some() && resources.index.is_some()
}

pub(in crate::playback) fn merge_soundfont_json_resources(
    existing: &mut SoundFontJsonResources,
    incoming: &SoundFontJsonResources,
) {
    if existing.metadata.is_none() {
        existing.metadata = incoming.metadata.clone();
    }
    if existing.coverage.is_none() {
        existing.coverage = incoming.coverage.clone();
    }
    if existing.index.is_none() {
        existing.index = incoming.index.clone();
    }
    if existing.pack_report.is_none() {
        existing.pack_report = incoming.pack_report.clone();
    }
}

pub(in crate::playback) fn coverage_supports_midi_role(
    coverage: &SoundFontCoverage,
    role: &MidiStripDescriptor,
) -> bool {
    if role.is_percussion {
        coverage.provides_percussion()
    } else {
        coverage.provides_melodic(role.bank, role.program)
    }
}

pub(in crate::playback) fn subset_demand_signature(plan: &SoundFontSubsetPlanDebug) -> String {
    format!(
        "roles={};presets={};instruments={};samples={};ranges={}",
        plan.requested_role_count,
        join_u32_ids(&plan.preset_ids),
        join_u32_ids(&plan.instrument_ids),
        join_u32_ids(&plan.sample_ids),
        plan.logical_wave_range_count
    )
}

fn join_u32_ids(ids: &[u32]) -> String {
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

pub(in crate::playback) fn plan_soundfont_subsets(
    soundfont_ids: &[String],
    channel_roles: &[MidiStripDescriptor],
    json_resources: &BTreeMap<String, SoundFontJsonResources>,
    _read_tasks: &[SoundFontDatReadTask],
) -> Result<BTreeMap<String, SoundFontSubsetPlanDebug>> {
    let mut plans = BTreeMap::new();

    for soundfont_id in soundfont_ids {
        let Some(index) = json_resources
            .get(soundfont_id)
            .and_then(|resources| resources.index.as_ref())
        else {
            continue;
        };

        let mut preset_ids = BTreeSet::<u32>::new();
        let mut instrument_ids = BTreeSet::<u32>::new();
        let mut sample_ids = BTreeSet::<u32>::new();
        let mut wave_ranges = Vec::<DatEntryRange>::new();

        for role in channel_roles {
            for preset in index.presets.iter().filter(|preset| {
                if role.is_percussion {
                    preset.bank == 128
                } else {
                    preset.bank == role.bank && preset.program == role.program
                }
            }) {
                preset_ids.insert(preset.preset_id);
                for instrument_id in &preset.instrument_ids {
                    instrument_ids.insert(*instrument_id);
                }
            }
        }

        for instrument_id in &instrument_ids {
            if let Some(load_map) = index
                .instrument_load_map
                .iter()
                .find(|load_map| load_map.instrument_id == *instrument_id)
            {
                sample_ids.extend(load_map.sample_ids.iter().copied());
                wave_ranges.extend(load_map.wave_ranges.iter().filter_map(|range| {
                    (range.byte_length > 0).then_some(DatEntryRange {
                        offset: range.smpl_start_byte,
                        length: range.byte_length,
                    })
                }));
            }
        }

        if preset_ids.is_empty() && instrument_ids.is_empty() && sample_ids.is_empty() {
            continue;
        }

        let logical_ranges = coalesce_entry_ranges(&wave_ranges)?;
        let planned_range_count = logical_ranges.len();
        let planned_byte_count = logical_ranges.iter().map(|range| range.length).sum();

        plans.insert(
            soundfont_id.clone(),
            SoundFontSubsetPlanDebug {
                soundfont_id: soundfont_id.clone(),
                requested_role_count: channel_roles.len(),
                preset_ids: preset_ids.into_iter().collect(),
                instrument_ids: instrument_ids.into_iter().collect(),
                sample_ids: sample_ids.into_iter().collect(),
                logical_wave_range_count: wave_ranges.len(),
                planned_range_count,
                planned_byte_count,
                used_index_json: true,
            },
        );
    }

    Ok(plans)
}
