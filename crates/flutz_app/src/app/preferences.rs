use super::decoded_peq::{
    default_decoded_peq_preset, load_builtin_decoded_peq_preset,
    normalize_default_decoded_peq_preset,
};
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PeqEditState {
    pub(super) preset: PeqPresetFile,
    pub(super) source: PeqConfigSource,
    pub(super) preset_path: Option<PathBuf>,
    pub(super) dirty: bool,
}

impl Default for PeqEditState {
    fn default() -> Self {
        Self {
            preset: default_decoded_peq_preset(48_000, 2),
            source: PeqConfigSource::Inactive,
            preset_path: None,
            dirty: false,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PeqConfigSource {
    Inactive,
    Wrapper,
    Default,
    PresetFile,
    BuiltInPreset,
}

impl PeqConfigSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inactive => "No decoded audio",
            Self::Wrapper => "Embedded",
            Self::Default => "Default",
            Self::PresetFile => "Preset file",
            Self::BuiltInPreset => "Built-in preset",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct DecodedAudioPeqPreferences {
    #[serde(default)]
    pub(super) bypass: bool,
    #[serde(default = "default_preferences_default_peq_preset")]
    pub(super) default_preset: PeqPresetFile,
}

impl Default for DecodedAudioPeqPreferences {
    fn default() -> Self {
        Self {
            bypass: false,
            default_preset: default_preferences_default_peq_preset(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct WindowViewPrefs {
    #[serde(default)]
    pub(super) playlist_open: bool,
    #[serde(default)]
    pub(super) metadata_open: bool,
    #[serde(default)]
    pub(super) playlist_position: Option<[f32; 2]>,
    #[serde(default)]
    pub(super) playlist_size: Option<[f32; 2]>,
    #[serde(default)]
    pub(super) metadata_position: Option<[f32; 2]>,
    #[serde(default)]
    pub(super) metadata_size: Option<[f32; 2]>,
}

impl Default for WindowViewPrefs {
    fn default() -> Self {
        Self {
            playlist_open: false,
            metadata_open: false,
            playlist_position: None,
            playlist_size: None,
            metadata_position: None,
            metadata_size: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct PreferencesFile {
    #[serde(default = "default_preferences_default_preset_id")]
    pub(super) default_preset_id: String,
    #[serde(default)]
    pub(super) decoded_audio_peq: DecodedAudioPeqPreferences,
    #[serde(default)]
    pub(super) window: WindowViewPrefs,
}

pub(super) struct LoadedPreferences {
    pub(super) preferences: PreferencesFile,
    pub(super) warning: Option<PreferencesWarning>,
    pub(super) save_blocked: Option<PreferencesWarning>,
}

#[derive(Clone)]
pub(super) struct PreferencesWarning {
    pub(super) status: &'static str,
    pub(super) detail: String,
}

impl Default for PreferencesFile {
    fn default() -> Self {
        Self {
            default_preset_id: default_preferences_default_preset_id(),
            decoded_audio_peq: DecodedAudioPeqPreferences::default(),
            window: WindowViewPrefs::default(),
        }
    }
}

pub(super) fn default_preferences_default_preset_id() -> String {
    default_preset_set().default_preset().id.to_owned()
}

pub(super) fn default_preferences_default_peq_preset() -> PeqPresetFile {
    normalize_default_decoded_peq_preset(load_builtin_decoded_peq_preset("Default"))
}

impl FlutzDesktopApp {
    pub(super) fn load_or_create_preferences_from_data_dir(
        data_dir: &Path,
        preset_set: &'static PresetSet,
    ) -> LoadedPreferences {
        let path = data_dir.join(Self::preferences_file_name());
        let mut warnings = Vec::new();
        let writable = match flutz_fs_sdl3::ensure_writable_directory(data_dir) {
            Ok(()) => true,
            Err(error) => {
                warnings.push(Self::preferences_write_error(&path, &error));
                false
            }
        };
        let mut save_blocked = None;
        let (preferences, existing_text) = match fs::read_to_string(&path) {
            Ok(text) => {
                let preferences = match toml::from_str::<PreferencesFile>(&text) {
                    Ok(preferences) => preferences,
                    Err(error) => {
                        warnings.push(PreferencesWarning {
                            status: "Invalid preferences.ini; defaults loaded",
                            detail: format!(
                                "Could not parse preferences at {}: {error}\nDefault settings are being used.",
                                path.display()
                            ),
                        });
                        PreferencesFile::default()
                    }
                };
                (preferences, Some(text))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (PreferencesFile::default(), None)
            }
            Err(error) => {
                let warning = PreferencesWarning {
                    status: "Cannot read preferences.ini; fix access and restart",
                    detail: format!(
                        "Could not read preferences at {}: {error}\nDefault settings are being used. Existing preferences will not be overwritten; fix access and restart the app.",
                        path.display()
                    ),
                };
                warnings.push(warning.clone());
                save_blocked = Some(warning);
                (PreferencesFile::default(), None)
            }
        };
        let preferences = Self::normalize_preferences(preferences);
        let rendered = Self::render_preferences_file(&preferences, preset_set);
        if writable && save_blocked.is_none() && existing_text.as_deref() != Some(&rendered) {
            if let Err(error) = flutz_fs_sdl3::atomic_write(&path, rendered.as_bytes()) {
                warnings.push(Self::preferences_write_error(&path, &error));
            }
        }
        let warning = if warnings.is_empty() {
            None
        } else {
            let status = save_blocked
                .as_ref()
                .map_or(warnings[0].status, |warning| warning.status);
            Some(PreferencesWarning {
                status,
                detail: warnings
                    .into_iter()
                    .map(|warning| warning.detail)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            })
        };
        LoadedPreferences {
            preferences,
            warning,
            save_blocked,
        }
    }

    pub(super) fn preferences_file_name() -> &'static str {
        "preferences.ini"
    }

    pub(super) fn preferences_path(&self) -> PathBuf {
        self.data_dir.join(Self::preferences_file_name())
    }

    pub(super) fn render_preferences_file(
        preferences: &PreferencesFile,
        preset_set: &'static PresetSet,
    ) -> String {
        let mut output = String::new();
        writeln!(&mut output, "# flutzPlayer preferences").unwrap();
        writeln!(
            &mut output,
            "# Edit this file directly. TOML format is used so comments are preserved."
        )
        .unwrap();
        writeln!(&mut output).unwrap();
        writeln!(
            &mut output,
            "# default_preset_id selects the multifont preset used when opening plain MIDI files."
        )
        .unwrap();
        writeln!(&mut output, "# Available preset ids:").unwrap();
        for preset in preset_set.presets {
            writeln!(&mut output, "# - {} ({})", preset.id, preset.display_name).unwrap();
        }
        writeln!(&mut output, "# decoded_audio_peq.default_preset stores the optional default decoded-audio PEQ config.").unwrap();
        writeln!(
            &mut output,
            "# decoded_audio_peq.bypass persists the global Bypass EQ toggle."
        )
        .unwrap();
        writeln!(&mut output).unwrap();
        output.push_str(
            &toml::to_string_pretty(preferences).expect("preferences serialization should succeed"),
        );
        output
    }

    pub(super) fn normalize_preferences(mut preferences: PreferencesFile) -> PreferencesFile {
        preferences.decoded_audio_peq.default_preset =
            normalize_default_decoded_peq_preset(preferences.decoded_audio_peq.default_preset);
        preferences
    }

    pub(super) fn build_preferences(&self) -> PreferencesFile {
        PreferencesFile {
            default_preset_id: self.default_preset_id.clone(),
            decoded_audio_peq: DecodedAudioPeqPreferences {
                bypass: self.decoded_peq_bypass_enabled,
                default_preset: normalize_default_decoded_peq_preset(
                    self.default_decoded_peq_preset.clone(),
                ),
            },
            window: self.window_view_prefs.clone(),
        }
    }

    fn preferences_write_error(path: &Path, error: &std::io::Error) -> PreferencesWarning {
        #[cfg(target_os = "linux")]
        let (status, remediation) = (
            "Prefs not saved: use ~/Applications/flutzPlayer or --data-dir",
            "Move flutzPlayer to a user-owned location such as ~/Applications/flutzPlayer, or choose a writable directory with --data-dir.",
        );
        #[cfg(not(target_os = "linux"))]
        let (status, remediation) = (
            "Prefs not saved: use a writable --data-dir",
            "Move flutzPlayer to a user-owned location, or choose a writable directory with --data-dir.",
        );
        PreferencesWarning {
            status,
            detail: format!(
                "Could not save preferences to {}: {error}\nChanges will be kept only for this session. {remediation}",
                path.display()
            ),
        }
    }

    fn report_preferences_warning(&mut self, warning: PreferencesWarning) {
        if self
            .last_preferences_warning
            .as_ref()
            .map(|previous| previous.detail.as_str())
            != Some(warning.detail.as_str())
        {
            eprintln!("{}", warning.detail);
        }
        self.set_preferences_status(warning.status);
        self.last_preferences_warning = Some(warning);
    }

    pub(super) fn persist_window_view_prefs(&mut self) -> bool {
        if let Some(warning) = self.preferences_save_blocked.clone() {
            self.report_preferences_warning(warning);
            return false;
        }
        let path = self.preferences_path();
        let rendered = Self::render_preferences_file(&self.build_preferences(), self.preset_set);
        match flutz_fs_sdl3::atomic_write(&path, rendered.as_bytes()) {
            Ok(()) => {
                self.last_preferences_warning = None;
                if self.preferences_status_active {
                    self.set_status("Preferences saved");
                }
                true
            }
            Err(error) => {
                self.report_preferences_warning(Self::preferences_write_error(&path, &error));
                false
            }
        }
    }

    pub(super) fn set_playlist_window_open(&mut self, open: bool) {
        if self.playlist_window_open == open {
            return;
        }
        self.playlist_window_open = open;
        self.window_view_prefs.playlist_open = open;
        self.persist_window_view_prefs();
    }

    pub(super) fn set_metadata_window_open(&mut self, open: bool) {
        if self.metadata_popup_open == open {
            return;
        }
        self.metadata_popup_open = open;
        self.window_view_prefs.metadata_open = open;
        self.persist_window_view_prefs();
    }

    pub(super) fn update_playlist_window_geometry(
        &mut self,
        inner_rect: Option<egui::Rect>,
        outer_rect: Option<egui::Rect>,
    ) {
        let mut changed = false;

        if let Some(inner_rect) = inner_rect {
            let size = [inner_rect.width().round(), inner_rect.height().round()];
            if self.window_view_prefs.playlist_size != Some(size) {
                self.window_view_prefs.playlist_size = Some(size);
                changed = true;
            }
        }

        if let Some(outer_rect) = outer_rect {
            let pos = [outer_rect.min.x.round(), outer_rect.min.y.round()];
            if self.window_view_prefs.playlist_position != Some(pos) {
                self.window_view_prefs.playlist_position = Some(pos);
                changed = true;
            }
        }

        if changed {
            self.persist_window_view_prefs();
        }
    }

    pub(super) fn update_metadata_window_geometry(
        &mut self,
        inner_rect: Option<egui::Rect>,
        outer_rect: Option<egui::Rect>,
    ) {
        let mut changed = false;

        if let Some(inner_rect) = inner_rect {
            let size = [inner_rect.width().round(), inner_rect.height().round()];
            if self.window_view_prefs.metadata_size != Some(size) {
                self.window_view_prefs.metadata_size = Some(size);
                changed = true;
            }
        }

        if let Some(outer_rect) = outer_rect {
            let pos = [outer_rect.min.x.round(), outer_rect.min.y.round()];
            if self.window_view_prefs.metadata_position != Some(pos) {
                self.window_view_prefs.metadata_position = Some(pos);
                changed = true;
            }
        }

        if changed {
            self.persist_window_view_prefs();
        }
    }
}
