use super::*;

impl FlutzDesktopApp {
    pub(super) fn save_fmid_to_path(&mut self, path: PathBuf) {
        match self.build_fmid_file() {
            Ok(fmid_file) => {
                let bytes = write_fmid(&fmid_file);
                match std::fs::write(&path, bytes) {
                    Ok(()) => {
                        self.current_path = Some(path.display().to_string());
                        if !self.metadata_edit_state.project_name.trim().is_empty() {
                            self.project_title = self.metadata_edit_state.project_name.clone();
                        }
                        self.dirty = false;
                        self.metadata_edit_dirty = false;
                        self.peq_edit_state.source = PeqConfigSource::Wrapper;
                        self.peq_edit_state.preset_path = None;
                        self.peq_edit_state.dirty = false;
                        self.set_status(format!("Saved {}", path.display()));
                    }
                    Err(error) => {
                        self.set_status(format!("failed to write {}: {error}", path.display()))
                    }
                }
            }
            Err(error) => self.set_status(format!("{error}")),
        }
    }

    pub(super) fn save_project_to_path(&mut self, path: PathBuf) {
        if self
            .loaded_content
            .as_ref()
            .is_some_and(|content| content.mastering == MasteringCapability::DecodedAudioPeq)
        {
            if let Err(error) = self.validate_decoded_wrapper_save_path(&path) {
                self.set_status(format!("{error}"));
                return;
            }
            self.save_decoded_wrapper_to_path(path);
        } else {
            self.save_fmid_to_path(path);
        }
    }

    pub(super) fn validate_decoded_wrapper_save_path(&self, path: &Path) -> Result<()> {
        let Some(content) = self
            .loaded_content
            .as_ref()
            .filter(|content| content.mastering == MasteringCapability::DecodedAudioPeq)
        else {
            return Ok(());
        };

        let wrapped_extension = content
            .wrapped_extension
            .as_deref()
            .unwrap_or("faudio")
            .trim_start_matches('.');
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            return Err(FlutzError::InvalidInput(format!(
                "decoded audio projects must be saved as .{wrapped_extension} wrapper files"
            )));
        };
        if !wrapped_extension.eq_ignore_ascii_case(extension) {
            return Err(FlutzError::InvalidInput(format!(
                "decoded audio projects must be saved as .{wrapped_extension} wrapper files"
            )));
        }

        if content
            .decoded_source_path
            .as_deref()
            .is_some_and(|source_path| paths_refer_to_same_file(path, source_path))
        {
            return Err(FlutzError::InvalidInput(format!(
                "cannot overwrite the active decoded source {}; save to a different .{wrapped_extension} wrapper path",
                path.display()
            )));
        }

        Ok(())
    }

    pub(super) fn can_save_project_to_existing_path(&self, path: &Path) -> bool {
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            return false;
        };
        if extension.eq_ignore_ascii_case("fmid") {
            return true;
        }
        self.loaded_content.as_ref().is_some_and(|content| {
            content.mastering == MasteringCapability::DecodedAudioPeq
                && content
                    .wrapped_extension
                    .as_deref()
                    .is_some_and(|wrapped| wrapped.eq_ignore_ascii_case(extension))
        })
    }

    pub(super) fn save_decoded_wrapper_to_path(&mut self, path: PathBuf) {
        match self.build_decoded_wrapper_file() {
            Ok(wrapper) => match write_flutz_wrapper(&wrapper) {
                Ok(bytes) => match std::fs::write(&path, bytes) {
                    Ok(()) => {
                        if let Some(content) = &mut self.loaded_content {
                            let decoded_source_path = content.decoded_source_path.clone();
                            let decoded_source_bytes = content.decoded_source_bytes.clone();
                            content.kind = ContentKind::DecodedAudioWrapper;
                            content.decoded_wrapper = Some(wrapper);
                            content.decoded_source_path = decoded_source_path;
                            content.decoded_source_bytes = decoded_source_bytes;
                        }
                        self.current_path = Some(path.display().to_string());
                        if !self.metadata_edit_state.project_name.trim().is_empty() {
                            self.project_title = self.metadata_edit_state.project_name.clone();
                        }
                        self.dirty = false;
                        self.metadata_edit_dirty = false;
                        self.set_status(format!("Saved {}", path.display()));
                    }
                    Err(error) => {
                        self.set_status(format!("failed to write {}: {error}", path.display()))
                    }
                },
                Err(error) => self.set_status(format!("{error}")),
            },
            Err(error) => self.set_status(format!("{error}")),
        }
    }

    pub(super) fn build_decoded_wrapper_file(&self) -> Result<FlutzAudioWrapper> {
        let source_path = self
            .loaded_content
            .as_ref()
            .and_then(|content| content.decoded_source_path.clone());
        let source_bytes = self
            .loaded_content
            .as_ref()
            .and_then(|content| content.decoded_source_bytes.clone());
        let mut wrapper = self
            .loaded_content
            .as_ref()
            .and_then(|content| content.decoded_wrapper.clone())
            .ok_or_else(|| {
                FlutzError::InvalidInput("load decoded audio before saving".to_owned())
            })?;
        if wrapper.source.bytes.is_empty() {
            if let Some(source_bytes) = source_bytes {
                wrapper.source.bytes = source_bytes.to_vec();
            } else {
                let source_path = source_path.ok_or_else(|| {
                    FlutzError::InvalidInput(
                        "decoded source bytes are unavailable for wrapper save".to_owned(),
                    )
                })?;
                wrapper.source.bytes = fs::read(&source_path).map_err(|error| {
                    FlutzError::Runtime(format!(
                        "failed to read decoded source {} for wrapper save: {error}",
                        source_path.display()
                    ))
                })?;
            }
        }
        wrapper.metadata = TrackMetadata {
            ..self
                .metadata_edit_state
                .to_track_metadata(self.project_title(), &wrapper.source.original_filename)
        };
        wrapper.native_metadata =
            normalized_metadata_fields(&self.metadata_edit_state.native_metadata);
        wrapper.loop_region = Some(MediaLoop {
            enabled: self.loop_enabled,
            mode: match self.loop_mode {
                LoopMode::None => MediaLoopMode::None,
                LoopMode::Infinite => MediaLoopMode::Infinite,
                LoopMode::Counted => MediaLoopMode::Counted,
            },
            unit: LoopUnit::SampleFrames {
                start: self.loop_start_tick,
                end: self.loop_end_tick,
            },
            loop_count: self.loop_count.max(1) as u64,
        });
        wrapper.peq = Some(self.peq_edit_state.preset.clone());
        Ok(wrapper)
    }

    pub(super) fn build_fmid_file(&self) -> Result<FmidFile> {
        let midi_bytes = self
            .playback
            .loaded_midi_bytes()
            .ok_or_else(|| FlutzError::InvalidInput("load a MIDI file before saving".to_owned()))?
            .to_vec();

        let project = self.metadata_edit_state.to_fmid_project(
            self.project_title(),
            self.playback
                .loaded_midi()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("embedded.mid"),
        );

        let loaded_soundfonts = self.playback.loaded_soundfont_ids();
        let soundfont_ids = if loaded_soundfonts.is_empty() {
            self.soundfonts
                .iter()
                .map(|font| font.internal_id.clone())
                .collect::<Vec<_>>()
        } else {
            loaded_soundfonts
        };

        let soundfonts = soundfont_ids
            .iter()
            .map(|id| SoundFontSlot {
                internal_id: id.clone(),
            })
            .collect::<Vec<_>>();

        let row_mutes = self
            .soundfonts
            .iter()
            .map(|font| SoundFontRowMuteRecord {
                soundfont_id: font.internal_id.clone(),
                muted: font.muted,
            })
            .collect::<Vec<_>>();

        let strips = self
            .soundfonts
            .iter()
            .flat_map(|font| {
                font.strips.iter().map(move |strip| FmidMixerStripRecord {
                    identity: FmidMixerStripIdentity {
                        soundfont_id: font.internal_id.clone(),
                        midi_channel: strip.channel as u64,
                        midi_program: strip.program as u64,
                        is_percussion: strip.is_percussion,
                    },
                    controls: FmidMixerStripControls {
                        volume: strip.volume as f64,
                        mute: strip.muted,
                        pan: strip.pan as f64,
                        gain_db: strip.gain_db as f64,
                        limiter_enabled: strip.limiter_enabled,
                        limiter_amount: strip.limiter_amount as f64,
                        limiter_release: LimiterControls::default().release,
                        reverb: strip.reverb as f64,
                        chorus: strip.chorus as f64,
                    },
                })
            })
            .collect::<Vec<_>>();

        let mixer = MixerRecord {
            master: MasterMixerRecord {
                volume_db: self.master.volume_db as f64,
                limiter_enabled: self.master.limiter_enabled,
                limiter_amount: self.master.limiter_amount as f64,
                limiter_release: LimiterControls::default().release,
                reverb: self.master.reverb as f64,
                chorus: self.master.chorus as f64,
                eq_low_db: self.master.eq_low as f64,
                eq_mid_db: self.master.eq_mid as f64,
                eq_high_db: self.master.eq_high as f64,
            },
            row_mutes,
            strips,
        };

        let loop_mode = if !self.loop_enabled {
            FmidLoopMode::None
        } else {
            match self.loop_mode {
                LoopMode::None => FmidLoopMode::None,
                LoopMode::Infinite => FmidLoopMode::Infinite,
                LoopMode::Counted => FmidLoopMode::Counted,
            }
        };

        let looping = FmidLoopRecord {
            enabled: self.loop_enabled,
            mode: loop_mode,
            start_tick: self.loop_start_tick,
            end_tick: self.loop_end_tick,
            loop_count: self.loop_count as u64,
        };

        let smart_mix = FmidSmartMixRecord {
            enabled: self.smart_mix.enabled,
            target_headroom: self.smart_mix.target_headroom_db as f64,
            attack: self.smart_mix.attack_ms as f64,
            release: self.smart_mix.release_ms as f64,
            lookahead: self.smart_mix.lookahead_ms as f64,
            auto_normalization_enabled: self.smart_mix.auto_normalize,
            auto_normalization_amount: self.smart_mix.normalization_amount as f64,
        };

        let mixer_source_mode = self
            .preset_set
            .find_preset(&self.active_preset_id)
            .map(|preset| MixerSourceMode::PresetDefault(preset.id.to_owned()))
            .unwrap_or(MixerSourceMode::Custom);

        Ok(FmidFile {
            midi_bytes,
            project,
            soundfonts,
            mixer,
            mixer_source_mode,
            looping,
            smart_mix,
            ..FmidFile::default()
        })
    }
}

pub(super) fn default_fmid_file_name(project_title: &str) -> String {
    let trimmed_title = project_title.trim();
    let stem = Path::new(trimmed_title)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(trimmed_title);
    format!("{}.fmid", stem.trim_end_matches('.'))
}

pub(super) fn default_wrapped_audio_file_name(project_title: &str, extension: &str) -> String {
    let trimmed_title = project_title.trim();
    let stem = Path::new(trimmed_title)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(trimmed_title);
    format!("{}.{}", stem.trim_end_matches('.'), extension)
}

pub(super) fn sanitize_file_stem(project_title: &str) -> String {
    let trimmed_title = project_title.trim();
    let stem = Path::new(trimmed_title)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(trimmed_title)
        .trim_end_matches('.');
    if stem.is_empty() {
        "peq-preset".to_owned()
    } else {
        stem.to_owned()
    }
}

pub(super) fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => {
            let left = left.as_os_str().to_string_lossy();
            let right = right.as_os_str().to_string_lossy();
            #[cfg(windows)]
            {
                left.eq_ignore_ascii_case(&right)
            }
            #[cfg(not(windows))]
            {
                left == right
            }
        }
    }
}
