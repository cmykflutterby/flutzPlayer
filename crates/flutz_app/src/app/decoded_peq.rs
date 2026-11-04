use super::persistence::sanitize_file_stem;
use super::*;

pub(super) fn builtin_decoded_peq_preset_names() -> &'static [&'static str] {
    &["Default", "Flat"]
}

pub(super) fn builtin_decoded_peq_preset_text(name: &str) -> Option<&'static str> {
    match name {
        "Default" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/eq-presets/Default.fpeq"
        ))),
        "Flat" => Some(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/eq-presets/Flat.fpeq"
        ))),
        _ => None,
    }
}

pub(super) fn load_builtin_decoded_peq_preset(name: &str) -> PeqPresetFile {
    let text = builtin_decoded_peq_preset_text(name)
        .unwrap_or_else(|| panic!("missing built-in decoded PEQ preset {name}"));
    let mut preset = deserialize_preset_toml(text).unwrap_or_else(|error| {
        panic!("failed to parse built-in decoded PEQ preset {name}: {error}")
    });
    preset.metadata.name = Some(name.to_owned());
    preset.metadata.tags.clear();
    preset
}

pub(super) fn default_decoded_peq_preset(sample_rate_hz: u32, channel_count: u16) -> PeqPresetFile {
    PeqPresetFile {
        metadata: PresetMetadata {
            name: Some("Current Track Wrapper Settings".to_owned()),
            ..PresetMetadata::default()
        },
        config: default_decoded_peq_config(sample_rate_hz, channel_count),
        extra_fields: BTreeMap::new(),
    }
}

pub(super) fn normalize_runtime_decoded_peq_preset(
    mut preset: PeqPresetFile,
    sample_rate_hz: u32,
    channel_count: u16,
) -> PeqPresetFile {
    preset.metadata.tags.clear();
    preset.config = normalized_decoded_peq_config(preset.config, sample_rate_hz, channel_count);
    preset
}

pub(super) fn normalize_default_decoded_peq_preset(mut preset: PeqPresetFile) -> PeqPresetFile {
    preset.metadata.name = Some("Default".to_owned());
    preset.metadata.tags.clear();
    preset
}

pub fn saved_decoded_peq_default_from_current(preset: &PeqPresetFile) -> PeqPresetFile {
    normalize_default_decoded_peq_preset(preset.clone())
}

pub fn runtime_decoded_peq_default_for_load(
    preset: PeqPresetFile,
    sample_rate_hz: u32,
    channel_count: u16,
) -> PeqPresetFile {
    normalize_runtime_decoded_peq_preset(preset, sample_rate_hz, channel_count)
}

pub(super) fn effective_decoded_peq_config(
    mut config: PeqConfig,
    bypass_enabled: bool,
) -> PeqConfig {
    if bypass_enabled {
        config.wet_mix = 0.0;
    }
    config
}

pub(super) fn default_decoded_peq_config(sample_rate_hz: u32, channel_count: u16) -> PeqConfig {
    PeqConfig {
        sample_rate_hz: sample_rate_hz.max(1),
        channel_count: channel_count.max(1),
        channel_layout: ChannelLayout::Interleaved,
        output_gain_db: 0.0,
        wet_mix: 1.0,
        bands: vec![PeqBandConfig {
            enabled: false,
            frequency_hz: 1_000.0,
            gain_db: 0.0,
            bandwidth: Bandwidth::Q { value: 0.8 },
            attack_ms: 10.0,
            release_ms: 80.0,
            ..PeqBandConfig::default()
        }],
        extra_fields: BTreeMap::new(),
    }
}

pub(super) fn normalized_decoded_peq_config(
    mut config: PeqConfig,
    sample_rate_hz: u32,
    channel_count: u16,
) -> PeqConfig {
    config.sample_rate_hz = sample_rate_hz.max(1);
    config.channel_count = channel_count.max(1);
    config.channel_layout = ChannelLayout::Interleaved;
    config.output_gain_db = config.output_gain_db.clamp(-24.0, 24.0);
    config.wet_mix = config.wet_mix.clamp(0.0, 1.0);
    let nyquist = config.sample_rate_hz as f32 * 0.5;
    for band in &mut config.bands {
        band.frequency_hz = band.frequency_hz.clamp(20.0, (nyquist - 1.0).max(20.0));
        band.gain_db = band.gain_db.clamp(-24.0, 24.0);
        band.attack_ms = band.attack_ms.max(0.0);
        band.release_ms = band.release_ms.max(0.0);
        match &mut band.bandwidth {
            Bandwidth::Q { value } | Bandwidth::Octaves { value } => {
                *value = value.clamp(0.05, 12.0);
            }
        }
    }
    config.bands.sort_by(|left, right| {
        peq_band_sort_frequency(left)
            .partial_cmp(&peq_band_sort_frequency(right))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    config
}

pub(super) fn peq_band_sort_frequency(band: &PeqBandConfig) -> f32 {
    let width = match band.bandwidth {
        Bandwidth::Q { value } => value.max(0.05),
        Bandwidth::Octaves { value } => value.max(0.05),
    };
    match band.filter_type {
        flutz_peq::PeqFilterType::LowShelf => 0.0,
        flutz_peq::PeqFilterType::HighShelf => band.frequency_hz,
        flutz_peq::PeqFilterType::Bell => (band.frequency_hz / width.sqrt()).max(0.0),
    }
}

impl FlutzDesktopApp {
    pub fn decoded_peq_config(&self) -> &PeqConfig {
        &self.peq_edit_state.preset.config
    }

    pub fn decoded_peq_config_source(&self) -> PeqConfigSource {
        self.peq_edit_state.source
    }

    pub fn decoded_peq_bypass_enabled(&self) -> bool {
        self.decoded_peq_bypass_enabled
    }

    pub fn decoded_peq_preset_display_name(&self) -> String {
        let base = match self.peq_edit_state.source {
            PeqConfigSource::Inactive => "No decoded audio".to_owned(),
            PeqConfigSource::Wrapper => "Embedded".to_owned(),
            PeqConfigSource::Default => "Default".to_owned(),
            PeqConfigSource::PresetFile => self
                .peq_edit_state
                .preset
                .metadata
                .name
                .clone()
                .or_else(|| {
                    self.peq_edit_state.preset_path.as_ref().and_then(|path| {
                        path.file_stem()
                            .and_then(|stem| stem.to_str())
                            .map(str::to_owned)
                    })
                })
                .unwrap_or_else(|| "Preset".to_owned()),
            PeqConfigSource::BuiltInPreset => self
                .peq_edit_state
                .preset
                .metadata
                .name
                .clone()
                .unwrap_or_else(|| "Built-in".to_owned()),
        };
        if self.peq_edit_state.dirty {
            format!("{base}*")
        } else {
            base
        }
    }

    pub fn builtin_decoded_peq_preset_names(&self) -> &'static [&'static str] {
        builtin_decoded_peq_preset_names()
    }

    pub fn decoded_peq_preset_path_label(&self) -> String {
        match self.peq_edit_state.source {
            PeqConfigSource::Inactive => "No decoded audio loaded".to_owned(),
            PeqConfigSource::Wrapper => "Embedded in current media file".to_owned(),
            PeqConfigSource::Default => "Stored in preferences.ini".to_owned(),
            PeqConfigSource::PresetFile => self
                .peq_edit_state
                .preset_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Preset file".to_owned()),
            PeqConfigSource::BuiltInPreset => "Bundled with flutzPlayer".to_owned(),
        }
    }

    pub fn apply_decoded_peq_config(&mut self, config: PeqConfig) {
        let source = self.peq_edit_state.source;
        let preset_path = self.peq_edit_state.preset_path.clone();
        self.apply_decoded_peq_config_with_source(config, source, preset_path, true);
    }

    pub fn reset_decoded_peq_config(&mut self) {
        self.apply_decoded_peq_config_with_source(
            self.default_decoded_peq_preset.config.clone(),
            PeqConfigSource::Default,
            None,
            false,
        );
    }

    pub fn apply_builtin_decoded_peq_preset(&mut self, preset_name: &str) {
        if self.active_mastering_capability() != MasteringCapability::DecodedAudioPeq {
            self.set_status("Load decoded audio before selecting a PEQ preset");
            return;
        }
        let preset = load_builtin_decoded_peq_preset(preset_name);
        self.apply_decoded_peq_config_with_source(
            preset.config,
            PeqConfigSource::BuiltInPreset,
            None,
            true,
        );
        self.peq_edit_state.preset.metadata = preset.metadata;
        self.peq_edit_state.preset.extra_fields = preset.extra_fields;
        self.set_status(format!("Applied built-in PEQ preset {preset_name}"));
    }

    pub fn save_decoded_peq_default(&mut self) {
        let preset = saved_decoded_peq_default_from_current(&self.peq_edit_state.preset);
        self.default_decoded_peq_preset = preset.clone();
        let saved = self.persist_window_view_prefs();
        if self.peq_edit_state.source == PeqConfigSource::Default {
            self.peq_edit_state.preset = preset;
            self.peq_edit_state.dirty = false;
        }
        if saved {
            self.set_status("Saved decoded audio PEQ default to preferences.ini");
        }
    }

    pub fn set_decoded_peq_bypass_enabled(&mut self, enabled: bool) {
        if self.decoded_peq_bypass_enabled == enabled {
            return;
        }
        self.decoded_peq_bypass_enabled = enabled;
        let saved = self.persist_window_view_prefs();
        if self.active_mastering_capability() == MasteringCapability::DecodedAudioPeq {
            let config = self.peq_edit_state.preset.config.clone();
            if let Err(error) = self.apply_decoded_peq_runtime_config(&config) {
                self.set_status(format!("{error}"));
                return;
            }
        }
        if saved {
            self.set_status(if enabled {
                "Bypass EQ enabled"
            } else {
                "Bypass EQ disabled"
            });
        }
    }

    pub fn load_decoded_peq_preset_dialog(&mut self) {
        if self.active_mastering_capability() != MasteringCapability::DecodedAudioPeq {
            self.set_status("Load decoded audio before loading a PEQ preset");
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PEQ preset", &["fpeq", "toml"])
            .pick_file()
        else {
            self.set_status("Load PEQ canceled");
            return;
        };
        match load_preset_file(&path) {
            Ok(preset) => {
                self.apply_decoded_peq_config_with_source(
                    preset.config,
                    PeqConfigSource::PresetFile,
                    Some(path.clone()),
                    false,
                );
                self.peq_edit_state.preset.metadata = preset.metadata;
                self.peq_edit_state.preset.extra_fields = preset.extra_fields;
                self.peq_edit_state.dirty = false;
                self.set_status(format!("Loaded PEQ preset {}", path.display()));
            }
            Err(error) => self.set_status(format!("{error}")),
        }
    }

    pub fn save_decoded_peq_preset(&mut self) {
        if let Some(path) = self.peq_edit_state.preset_path.clone() {
            self.save_decoded_peq_preset_to_path(path);
        } else {
            self.save_decoded_peq_preset_as();
        }
    }

    pub fn save_decoded_peq_preset_as(&mut self) {
        if self.active_mastering_capability() != MasteringCapability::DecodedAudioPeq {
            self.set_status("Load decoded audio before saving a PEQ preset");
            return;
        }
        let default_name = format!("{}.fpeq", sanitize_file_stem(self.project_title()));
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PEQ preset", &["fpeq", "toml"])
            .set_file_name(default_name)
            .save_file()
        else {
            self.set_status("Save PEQ canceled");
            return;
        };
        self.save_decoded_peq_preset_to_path(path);
    }

    pub(super) fn save_decoded_peq_preset_to_path(&mut self, path: PathBuf) {
        let mut preset = self.peq_edit_state.preset.clone();
        if preset.metadata.name.is_none() {
            preset.metadata.name = Some(self.project_title().to_owned());
        }
        match save_preset_file(&path, &preset) {
            Ok(()) => {
                self.peq_edit_state.preset = preset;
                self.peq_edit_state.preset_path = Some(path.clone());
                if self.peq_edit_state.source == PeqConfigSource::PresetFile {
                    self.peq_edit_state.dirty = false;
                }
                self.set_status(format!("Saved PEQ preset {}", path.display()));
            }
            Err(error) => self.set_status(format!("{error}")),
        }
    }

    pub(super) fn apply_decoded_audio_defaults(&mut self, loop_region: Option<MediaLoop>) {
        self.soundfonts.clear();
        self.coverage_cache.clear();
        self.selected_soundfont = 0;
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        self.master = MasterControls::default();
        self.smart_mix = SmartMixControls::default();

        if let Some(loop_region) = loop_region {
            self.loop_enabled = loop_region.enabled;
            self.loop_mode = match loop_region.mode {
                MediaLoopMode::None => LoopMode::None,
                MediaLoopMode::Infinite => LoopMode::Infinite,
                MediaLoopMode::Counted => LoopMode::Counted,
            };
            if let LoopUnit::SampleFrames { start, end } = loop_region.unit {
                self.loop_start_tick = start;
                self.loop_end_tick = end;
            } else {
                self.loop_start_tick = 0;
                self.loop_end_tick = self.transport_tick_length();
            }
            self.loop_count = loop_region.loop_count.max(1).min(u32::MAX as u64) as u32;
            self.normalize_loop_state(false);
            return;
        }

        let frame_length = self.transport_tick_length();
        let enable_by_default = self.should_enable_loop_by_default(false);
        self.loop_mode = if enable_by_default {
            LoopMode::Infinite
        } else {
            LoopMode::None
        };
        self.loop_enabled = enable_by_default && frame_length > 0;
        self.loop_start_tick = 0;
        self.loop_end_tick = if self.loop_enabled { frame_length } else { 0 };
        self.loop_count = 1;
        self.normalize_loop_state(false);
    }

    pub(super) fn reset_decoded_peq_edit_state(&mut self, peq: Option<PeqPresetFile>) {
        let sample_rate = self
            .playback
            .decoded_transport_metadata()
            .map(|metadata| metadata.sample_rate)
            .unwrap_or(48_000);
        let (preset, source) = match peq {
            Some(mut preset) => {
                preset = normalize_runtime_decoded_peq_preset(preset, sample_rate, 2);
                (preset, PeqConfigSource::Wrapper)
            }
            None => {
                let preset = normalize_runtime_decoded_peq_preset(
                    self.default_decoded_peq_preset.clone(),
                    sample_rate,
                    2,
                );
                (preset, PeqConfigSource::Default)
            }
        };
        self.peq_edit_state = PeqEditState {
            preset: preset.clone(),
            source,
            preset_path: None,
            dirty: false,
        };
        if let Err(error) = self.apply_decoded_peq_runtime_config(&preset.config) {
            self.set_status(format!("{error}"));
        }
    }

    pub(super) fn apply_decoded_peq_runtime_config(&mut self, config: &PeqConfig) -> Result<()> {
        let runtime_config =
            effective_decoded_peq_config(config.clone(), self.decoded_peq_bypass_enabled);
        self.playback
            .set_decoded_peq_config(runtime_config)
            .map(|_| ())
    }

    pub(super) fn apply_decoded_peq_config_with_source(
        &mut self,
        config: PeqConfig,
        source: PeqConfigSource,
        preset_path: Option<PathBuf>,
        mark_dirty: bool,
    ) {
        if self.active_mastering_capability() != MasteringCapability::DecodedAudioPeq {
            self.set_status("Load decoded audio before editing PEQ");
            return;
        }
        let sample_rate = self
            .playback
            .decoded_transport_metadata()
            .map(|metadata| metadata.sample_rate)
            .unwrap_or(config.sample_rate_hz);
        let config = normalized_decoded_peq_config(config, sample_rate, 2);
        if let Err(error) = self.apply_decoded_peq_runtime_config(&config) {
            self.set_status(format!("{error}"));
            return;
        }
        let changed = self.peq_edit_state.preset.config != config;
        self.peq_edit_state.preset.config = config;
        self.peq_edit_state.source = source;
        self.peq_edit_state.preset_path = preset_path;
        self.peq_edit_state.dirty = mark_dirty && changed;
        if let Some(content) = &mut self.loaded_content {
            if let Some(wrapper) = &mut content.decoded_wrapper {
                wrapper.peq = Some(self.peq_edit_state.preset.clone());
            }
        }
        if changed && mark_dirty {
            self.mark_dirty();
        }
    }
}
