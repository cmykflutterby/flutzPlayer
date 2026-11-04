use super::*;

mod support;
pub(in crate::playback) use support::{
    compare_subset_transition, coverage_supports_midi_role, merge_soundfont_json_resources,
    plan_soundfont_subsets, read_soundfont_dat_payloads, read_soundfont_subset_dat_payload,
    soundfont_json_resources_complete, soundfont_json_resources_present, subset_demand_signature,
    SoundFontDatReadTask,
};
pub use support::{
    LoadedSubsetState, PlaybackSoundFontLoadDiagnostics, PlaybackSubsetTransitionDiagnostics,
    SoundFontSubsetPlanAggregateDebug, SoundFontSubsetPlanDebug,
};

impl PlaybackController {
    pub(super) fn resolve_soundfont_ids(
        &self,
        requested_soundfonts: &[String],
        channel_roles: &[MidiStripDescriptor],
    ) -> Result<Vec<String>> {
        let available = self
            .catalog
            .iter()
            .map(|font| font.internal_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut ids = requested_soundfonts
            .iter()
            .filter(|id| available.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();

        if !ids.is_empty() {
            ids = self.prune_soundfont_ids_by_coverage(ids, channel_roles);
        }

        if ids.is_empty() {
            if let Some(default) = self.catalog.iter().find(|font| font.is_default) {
                ids.push(default.internal_id.clone());
            } else if let Some(first) = self.catalog.first() {
                ids.push(first.internal_id.clone());
            }
        }

        if ids.is_empty() {
            return Err(FlutzError::InvalidInput(
                "no soundfonts are available in the DAT data folder".to_owned(),
            ));
        }

        Ok(ids)
    }

    pub(super) fn prune_soundfont_ids_by_coverage(
        &self,
        soundfont_ids: Vec<String>,
        channel_roles: &[MidiStripDescriptor],
    ) -> Vec<String> {
        if channel_roles.is_empty() {
            return soundfont_ids;
        }
        let coverage_by_id = self
            .catalog
            .iter()
            .map(|font| (font.internal_id.as_str(), font.coverage.as_ref()))
            .collect::<BTreeMap<_, _>>();
        let pruned = soundfont_ids
            .iter()
            .filter(|soundfont_id| {
                let Some(Some(coverage)) = coverage_by_id.get(soundfont_id.as_str()) else {
                    return true;
                };
                channel_roles
                    .iter()
                    .any(|role| coverage_supports_midi_role(coverage, role))
            })
            .cloned()
            .collect::<Vec<_>>();
        if pruned.is_empty() {
            soundfont_ids
        } else {
            pruned
        }
    }

    pub(super) fn load_soundfonts_for_playback(
        &mut self,
        soundfont_ids: &[String],
        channel_roles: &[MidiStripDescriptor],
    ) -> Result<Vec<LoadedSoundFont>> {
        let load_started = Instant::now();
        let load_invocation = self.last_soundfont_load.load_invocations.saturating_add(1);
        let wanted = soundfont_ids.iter().cloned().collect::<BTreeSet<_>>();
        let catalog = self
            .catalog
            .iter()
            .map(|font| (font.internal_id.as_str(), font))
            .collect::<BTreeMap<_, _>>();
        let (read_tasks, json_resources) = self.collect_soundfont_dat_resources(&wanted)?;
        let dat_read_task_count = read_tasks.len();
        let read_tasks_by_font = read_tasks.iter().fold(
            BTreeMap::<String, Vec<&SoundFontDatReadTask>>::new(),
            |mut tasks_by_font, task| {
                tasks_by_font
                    .entry(task.soundfont_id.clone())
                    .or_default()
                    .push(task);
                tasks_by_font
            },
        );

        let json_resource_font_count = json_resources
            .values()
            .filter(|resources| soundfont_json_resources_present(resources))
            .count();
        self.last_subset_plans =
            plan_soundfont_subsets(soundfont_ids, channel_roles, &json_resources, &read_tasks)?;
        let previous_subset_state = self.loaded_subset_state.clone();
        let mut current_subset_state = BTreeMap::<String, LoadedSubsetState>::new();

        let mut full_dat_entry_reads = 0usize;
        let mut full_dat_entry_bytes = 0u64;
        let mut range_dat_reads = 0usize;
        let mut range_dat_read_bytes = 0u64;

        let mut soundfonts = Vec::with_capacity(soundfont_ids.len());
        let mut subset_request_count = 0usize;
        let mut subset_source_clone_bytes = 0u64;
        let mut full_fallback_count = 0usize;
        let mut non_subset_full_load_count = 0usize;
        for id in soundfont_ids {
            let tasks = read_tasks_by_font.get(id).map(Vec::as_slice).unwrap_or(&[]);
            if tasks.is_empty() {
                return Err(FlutzError::InvalidInput(format!(
                    "soundfont payload not found in DAT files: {id}"
                )));
            }
            let Some(entry) = catalog.get(id.as_str()) else {
                return Err(FlutzError::InvalidInput(format!(
                    "soundfont is missing from startup catalog: {id}"
                )));
            };
            if entry.runtime_format != "sf2" || entry.storage_format != "sf2" {
                return Err(FlutzError::UnsupportedFormat(format!(
                    "soundfont {id} is {} stored as {}; playback requires sf2",
                    entry.runtime_format, entry.storage_format
                )));
            }
            if let Some(plan) = self.last_subset_plans.get(id) {
                let subset_payload = json_resources
                    .get(id)
                    .and_then(|resources| resources.index.as_ref())
                    .and_then(|index| read_soundfont_subset_dat_payload(tasks, index, plan).ok());

                subset_request_count = subset_request_count.saturating_add(1);
                if let Some(subset_payload) = subset_payload {
                    range_dat_reads = range_dat_reads.saturating_add(subset_payload.range_reads);
                    range_dat_read_bytes =
                        range_dat_read_bytes.saturating_add(subset_payload.range_read_bytes as u64);
                    let request = SoundFontSubsetBytes::from_dat_entry(
                        id.clone(),
                        entry.display_name.clone(),
                        subset_payload.metadata_bytes,
                        subset_payload.sources.clone(),
                        subset_demand_signature(plan),
                        SoundFontMetadataClosure::new(
                            plan.preset_ids.iter().map(|id| *id as usize).collect(),
                            plan.instrument_ids.iter().map(|id| *id as usize).collect(),
                            plan.sample_ids.iter().map(|id| *id as usize).collect(),
                        ),
                        subset_payload.sample_ranges,
                    );
                    if let Ok(sound_font) = self.soundfont_cache.load_subset_soundfont(request) {
                        current_subset_state
                            .insert(id.clone(), LoadedSubsetState::from_plan(plan, true));
                        soundfonts.push(LoadedSoundFont {
                            internal_id: id.clone(),
                            display_name: entry.display_name.clone(),
                            sound_font,
                        });
                        continue;
                    }
                }

                full_fallback_count = full_fallback_count.saturating_add(1);
                let payload = read_soundfont_dat_payloads(tasks)?;
                full_dat_entry_reads = full_dat_entry_reads.saturating_add(payload.entry_reads);
                full_dat_entry_bytes =
                    full_dat_entry_bytes.saturating_add(payload.entry_read_bytes as u64);
                subset_source_clone_bytes =
                    subset_source_clone_bytes.saturating_add(payload.bytes.len() as u64);
                let sound_font =
                    self.soundfont_cache
                        .load_soundfont(SoundFontBytes::from_dat_entry(
                            id.clone(),
                            entry.display_name.clone(),
                            payload.bytes,
                            payload.sources,
                        ))?;
                current_subset_state.insert(id.clone(), LoadedSubsetState::from_plan(plan, false));
                soundfonts.push(LoadedSoundFont {
                    internal_id: id.clone(),
                    display_name: entry.display_name.clone(),
                    sound_font,
                });
            } else {
                non_subset_full_load_count = non_subset_full_load_count.saturating_add(1);
                let payload = read_soundfont_dat_payloads(tasks)?;
                full_dat_entry_reads = full_dat_entry_reads.saturating_add(payload.entry_reads);
                full_dat_entry_bytes =
                    full_dat_entry_bytes.saturating_add(payload.entry_read_bytes as u64);
                let sound_font =
                    self.soundfont_cache
                        .load_soundfont(SoundFontBytes::from_dat_entry(
                            id.clone(),
                            entry.display_name.clone(),
                            payload.bytes,
                            payload.sources,
                        ))?;
                soundfonts.push(LoadedSoundFont {
                    internal_id: id.clone(),
                    display_name: entry.display_name.clone(),
                    sound_font,
                });
            }
        }

        self.loaded_subset_state = current_subset_state;
        self.subset_transition =
            compare_subset_transition(&previous_subset_state, &self.loaded_subset_state);
        self.last_soundfont_load = PlaybackSoundFontLoadDiagnostics {
            load_invocations: load_invocation,
            requested_font_count: soundfont_ids.len(),
            dat_read_task_count,
            full_dat_entry_reads,
            range_dat_reads,
            full_dat_entry_bytes,
            range_dat_read_bytes,
            subset_request_count,
            subset_source_clone_bytes,
            full_fallback_count,
            non_subset_full_load_count,
            json_resource_font_count,
            index_json_plan_count: self
                .last_subset_plans
                .values()
                .filter(|plan| plan.used_index_json)
                .count(),
            load_duration_ms: load_started.elapsed().as_millis(),
        };

        Ok(soundfonts)
    }

    pub(super) fn loaded_subset_state_satisfies(
        &mut self,
        soundfont_ids: &[String],
        channel_roles: &[MidiStripDescriptor],
    ) -> Result<bool> {
        let wanted = soundfont_ids.iter().cloned().collect::<BTreeSet<_>>();
        let (read_tasks, json_resources) = self.collect_soundfont_dat_resources(&wanted)?;
        let plans =
            plan_soundfont_subsets(soundfont_ids, channel_roles, &json_resources, &read_tasks)?;
        if plans.is_empty() {
            return Ok(true);
        }
        let previous_subset_state = self.loaded_subset_state.clone();
        let mut current_subset_state = BTreeMap::<String, LoadedSubsetState>::new();
        let mut satisfies = true;
        for (soundfont_id, plan) in &plans {
            let Some(previous) = previous_subset_state.get(soundfont_id) else {
                satisfies = false;
                break;
            };
            let current = LoadedSubsetState::from_plan(plan, previous.compact_loaded);
            if previous.compact_loaded && !current.sample_set().is_subset(&previous.sample_set()) {
                satisfies = false;
                break;
            }
            current_subset_state.insert(soundfont_id.clone(), current);
        }
        if satisfies {
            self.last_subset_plans = plans;
            self.subset_transition =
                compare_subset_transition(&previous_subset_state, &current_subset_state);
        }
        Ok(satisfies)
    }

    fn collect_soundfont_dat_resources(
        &self,
        wanted: &BTreeSet<String>,
    ) -> Result<(
        Vec<SoundFontDatReadTask>,
        BTreeMap<String, SoundFontJsonResources>,
    )> {
        let mut read_tasks = Vec::new();
        let mut json_resources = BTreeMap::<String, SoundFontJsonResources>::new();
        let mut next_order = 0usize;

        for dat_path in crate::collect_dat_paths(&self.data_dir)? {
            let index = parse_dat_index_file(&dat_path)?;
            for record in &index.entries {
                if record.entry.asset_type == "soundfont"
                    && wanted.contains(&record.entry.internal_id)
                {
                    read_tasks.push(SoundFontDatReadTask {
                        order: next_order,
                        soundfont_id: record.entry.internal_id.clone(),
                        dat_path: dat_path.clone(),
                        index: index.clone(),
                        record: record.clone(),
                    });
                    next_order = next_order.saturating_add(1);
                }
            }
            for soundfont_id in wanted {
                if json_resources
                    .get(soundfont_id)
                    .map(soundfont_json_resources_complete)
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Ok(resources) =
                    read_soundfont_json_resources_from_file(&dat_path, &index, soundfont_id)
                {
                    if soundfont_json_resources_present(&resources) {
                        json_resources
                            .entry(soundfont_id.clone())
                            .and_modify(|existing| {
                                merge_soundfont_json_resources(existing, &resources)
                            })
                            .or_insert(resources);
                    }
                }
            }
        }

        Ok((read_tasks, json_resources))
    }

    pub fn loaded_soundfont_ids(&self) -> Vec<String> {
        self.loaded_soundfonts.clone()
    }

    pub fn last_soundfont_subset_plans(&self) -> BTreeMap<String, SoundFontSubsetPlanDebug> {
        self.last_subset_plans.clone()
    }

    pub fn loaded_soundfont_coverages(&self) -> BTreeMap<String, SoundFontCoverage> {
        self.loaded_coverages.clone()
    }
}
