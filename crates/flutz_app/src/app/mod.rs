use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Instant,
};

use eframe::egui;
use flutz_core::{default_preset_set, FlutzError, Preset, PresetSet, Result, StripId};
use flutz_fmid::{
    read_fmid, write_fmid, FmidFile, LoopMode as FmidLoopMode, LoopRecord as FmidLoopRecord,
    MasterMixerRecord, MixerRecord, MixerSourceMode, MixerStripControls as FmidMixerStripControls,
    MixerStripIdentity as FmidMixerStripIdentity, MixerStripRecord as FmidMixerStripRecord,
    ProjectRecord as FmidProjectRecord, SmartMixRecord as FmidSmartMixRecord,
    SoundFontRowMuteRecord, SoundFontSlot,
};
use flutz_formats::{
    builtin_registry, read_flutz_wrapper, read_track_metadata_with_symphonia, write_flutz_wrapper,
    ContentKind, DecodedAudioStreamSource, FlutzAudioWrapper, FormatDescriptor,
    LoopMode as MediaLoopMode, LoopUnit, MasteringCapability, MediaLoop, MetadataField,
    NativeMetadata, SourceAudioBlock, TrackMetadata,
};
use flutz_media_controls::{
    create_media_controls_backend, should_publish_snapshot, MediaControlCommand,
    MediaControlsBackend, MediaPlaybackStatus, MediaTrackSnapshot,
};
use flutz_mixer::{
    effects::LimiterControls, AutoNormalization, MasterControls as MixerMasterControls,
    MixerSettings, MixerStripControls, SmartMixSettings,
};
use flutz_peq::{
    deserialize_preset_toml, load_preset_file, save_preset_file, Bandwidth, ChannelLayout,
    PeqBandConfig, PeqConfig, PeqPresetFile, PresetMetadata,
};
use flutz_synth::{PlaybackLoopMode, PlaybackLoopSettings, SoundFontCoverage};
use flutz_visualizer_core::VisualizerFrame;
#[cfg(windows)]
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use serde::{Deserialize, Serialize};

use crate::allocation_trace::{AllocationScope, AllocationScopeGuard};
use crate::perf_trace::{LatencyTrace, PerfTrace, PerfTraceRecord};
use crate::playback::{
    AudioBackend, AudioPlaybackStatus, MidiTransportMetadata, MixerControlState,
    PlaybackController, PlaybackDebugMetrics, RealtimeMixerSnapshot,
};
use crate::playlist::{
    persistence as playlist_persistence, PlaylistOrderMode, PlaylistRepeatMode, PlaylistState,
};
use crate::repaint_policy::{repaint_interval, RepaintInputs, UiWaker};
use crate::routing::compute_strip_routing;

mod content_loading;
mod decoded_peq;
mod diagnostics;
mod lifecycle;
mod media_controls;
mod metadata;
mod mixer_state;
mod persistence;
mod playlist_flow;
mod preferences;
mod transport;
use content_loading::{DecodedAudioLoadJobResult, LoadedContentState, PendingDecodedAudioLoad};
pub use decoded_peq::{
    runtime_decoded_peq_default_for_load, saved_decoded_peq_default_from_current,
};
use lifecycle::load_app_icon;
pub use metadata::{
    decoded_project_title, merge_imported_track_metadata, merge_metadata_field_lists,
    track_metadata_from_imported_fields, MetadataEditState,
};
use metadata::{
    draw_metadata_field_list, draw_metadata_multiline_field, draw_metadata_text_field,
    normalized_metadata_fields, read_os_track_metadata,
};
use mixer_state::{
    set_text_in_place, MixerEditSnapshot, PlaybackReloadResumeState, RealtimeStripKey,
};
pub use mixer_state::{
    MasterControls, MixerAssignmentMode, MixerStripUiState, SmartMixControls, SoundFontUiRow,
    FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER, MASTER_VOLUME_MAX_DB,
};
use playlist_flow::{PendingPlaylistPreload, ReadyPlaylistPreload};
pub use preferences::PeqConfigSource;
use preferences::{PeqEditState, PreferencesWarning, WindowViewPrefs};
pub use transport::LoopMode;

const PLAYLIST_PRELOAD_LEAD_TIME_SECONDS: f64 = 3.0;

static DECODED_AUDIO_LOAD_TOKEN: AtomicU64 = AtomicU64::new(0);
static PLAYLIST_PRELOAD_TOKEN: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum AppRunState {
    #[default]
    Idle,
    Playing,
    Paused,
}

#[derive(Debug, Clone)]
pub struct AppStartupConfig {
    pub data_dir: PathBuf,
    pub dat_summary: DatStartupSummary,
    pub audio_backend: AudioBackend,
    pub debug_memory: bool,
    pub debug_latency: bool,
    pub debug_analyzer: bool,
    pub debug_render_errors: bool,
    pub launch_open_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum DatStartupSummary {
    Available {
        dat_file_count: usize,
        dat_byte_count: u64,
        soundfonts: Vec<SoundFontCatalogEntry>,
    },
    Unavailable(String),
}

#[derive(Debug, Clone)]
pub struct SoundFontCatalogEntry {
    pub internal_id: String,
    pub display_name: String,
    pub source_format: String,
    pub storage_format: String,
    pub runtime_format: String,
    pub is_default: bool,
    pub total_size: u64,
    pub part_count: u64,
    pub coverage: Option<SoundFontCoverage>,
}

pub fn run_gui(config: AppStartupConfig) -> Result<()> {
    #[cfg(debug_assertions)]
    let min_inner_size = [960.0, 640.0];
    #[cfg(not(debug_assertions))]
    let min_inner_size = [320.0, 268.0];

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("flutzPlayer")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size(min_inner_size)
            .with_icon(load_app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "flutzPlayer",
        options,
        Box::new(|creation_context| Ok(Box::new(FlutzDesktopApp::new(creation_context, config)))),
    )
    .map_err(|error| FlutzError::Runtime(format!("failed to start GUI: {error}")))
}

#[cfg(windows)]
fn create_media_controls_backend_for_creation_context(
    creation_context: &eframe::CreationContext<'_>,
) -> Box<dyn MediaControlsBackend> {
    let Some(hwnd) =
        creation_context
            .window_handle()
            .ok()
            .and_then(|handle| match handle.as_raw() {
                RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
                _ => None,
            })
    else {
        return create_media_controls_backend();
    };
    flutz_media_controls::create_media_controls_backend_for_window(hwnd)
}

#[cfg(not(windows))]
fn create_media_controls_backend_for_creation_context(
    _creation_context: &eframe::CreationContext<'_>,
) -> Box<dyn MediaControlsBackend> {
    create_media_controls_backend()
}

pub struct FlutzDesktopApp {
    run_state: AppRunState,
    release_edit_mode: bool,
    release_editor_panels_collapsed: bool,
    speaker_volume_popup_open: bool,
    final_output_volume_percent: f32,
    release_loop_popup_open: bool,
    loaded_content: Option<LoadedContentState>,
    project_title: String,
    current_path: Option<String>,
    dirty: bool,
    transport_position: f32,
    loop_enabled: bool,
    loop_mode: LoopMode,
    loop_start_tick: u64,
    loop_end_tick: u64,
    loop_count: u32,
    master: MasterControls,
    smart_mix: SmartMixControls,
    mixer_fx_expanded: bool,
    soundfonts: Vec<SoundFontUiRow>,
    catalog_soundfonts: Vec<SoundFontCatalogEntry>,
    preset_set: &'static PresetSet,
    active_preset_id: String,
    selected_preset_id: String,
    default_preset_id: String,
    default_decoded_peq_preset: PeqPresetFile,
    decoded_peq_bypass_enabled: bool,
    coverage_cache: BTreeMap<String, SoundFontCoverage>,
    selected_soundfont: usize,
    mixer_assignment_mode: MixerAssignmentMode,
    data_dir: PathBuf,
    data_summary: String,
    playback: PlaybackController,
    media_controls: Box<dyn MediaControlsBackend>,
    last_media_controls_snapshot: Option<MediaTrackSnapshot>,
    last_media_controls_timeline_publish: Option<Instant>,
    perf_trace: PerfTrace,
    latency_trace: LatencyTrace,
    logged_unmatched_snapshot_strips: BTreeSet<RealtimeStripKey>,
    _memory_maintenance: Option<crate::memory_runtime::MemoryMaintenanceHandle>,
    realtime_snapshot_buffer: RealtimeMixerSnapshot,
    mixer_edit_buffer: MixerEditSnapshot,
    published_mixer_controls: MixerControlState,
    status: String,
    missing_preset_warning: Option<String>,
    preferences_status_active: bool,
    last_preferences_warning: Option<PreferencesWarning>,
    preferences_save_blocked: Option<PreferencesWarning>,
    playlist: Option<PlaylistState>,
    window_view_prefs: WindowViewPrefs,
    playlist_window_open: bool,
    playlist_file_picker_pending: bool,
    playlist_selected_indices: BTreeSet<usize>,
    playlist_drag_from: Option<usize>,
    metadata_popup_open: bool,
    metadata_edit_dirty: bool,
    metadata_edit_state: MetadataEditState,
    peq_edit_state: PeqEditState,
    pending_decoded_audio_load: Option<PendingDecodedAudioLoad>,
    pending_playlist_preload: Option<PendingPlaylistPreload>,
    ready_playlist_preload: Option<ReadyPlaylistPreload>,
    last_playback_active: bool,
}

impl FlutzDesktopApp {}

impl FlutzDesktopApp {
    pub fn draw_playlist_transport_cluster_hook(&mut self, ui: &mut egui::Ui) {
        let has_playlist = self.playlist.is_some();
        let prev_enabled = has_playlist && self.playlist_has_prev_track();
        let next_enabled = has_playlist && self.playlist_has_next_track();

        ui.add_space(8.0);
        if ui
            .add_enabled(prev_enabled, egui::Button::new("⏮"))
            .on_hover_text("Previous Track")
            .clicked()
        {
            self.prev_track();
        }
        if ui
            .add(egui::Button::new("≣").selected(self.playlist_window_open))
            .on_hover_text("Playlist")
            .clicked()
        {
            self.set_playlist_window_open(!self.playlist_window_open);
        }
        if ui
            .add_enabled(next_enabled, egui::Button::new("⏭"))
            .on_hover_text("Next Track")
            .clicked()
        {
            self.next_track();
        }
    }

    pub fn draw_playlist_viewport_host(&mut self, context: &egui::Context) {
        if !self.playlist_window_open {
            return;
        }

        if self.playlist.is_none() {
            self.new_playlist();
        }

        let mut builder = egui::ViewportBuilder::default()
            .with_title("Playlist")
            .with_resizable(true)
            .with_min_inner_size(egui::vec2(480.0, 280.0));
        if let Some(size) = self.window_view_prefs.playlist_size {
            builder = builder.with_inner_size(egui::vec2(size[0], size[1]));
        } else {
            builder = builder.with_inner_size(egui::vec2(720.0, 420.0));
        }
        if let Some(position) = self.window_view_prefs.playlist_position {
            builder = builder.with_position(egui::pos2(position[0], position[1]));
        }

        let (close_requested, inner_rect, outer_rect) = context.show_viewport_immediate(
            egui::ViewportId::from_hash_of("playlist_viewport"),
            builder,
            |viewport_context, class| {
                crate::ui::playlist_window::draw_playlist_window_viewport(
                    viewport_context,
                    class,
                    self,
                );
                let info = viewport_context.input(|input| input.viewport().clone());
                (info.close_requested(), info.inner_rect, info.outer_rect)
            },
        );

        self.update_playlist_window_geometry(inner_rect, outer_rect);
        if close_requested {
            self.set_playlist_window_open(false);
        }
    }

    pub fn draw_metadata_viewport_host(&mut self, context: &egui::Context) {
        if !self.metadata_popup_open {
            return;
        }

        let mut builder = egui::ViewportBuilder::default()
            .with_title("Metadata")
            .with_resizable(true)
            .with_min_inner_size(egui::vec2(720.0, 240.0));
        if let Some(size) = self.window_view_prefs.metadata_size {
            builder = builder.with_inner_size(egui::vec2(size[0], size[1]));
        } else {
            builder = builder.with_inner_size(egui::vec2(920.0, 320.0));
        }
        if let Some(position) = self.window_view_prefs.metadata_position {
            builder = builder.with_position(egui::pos2(position[0], position[1]));
        }

        let (close_requested, inner_rect, outer_rect) = context.show_viewport_immediate(
            egui::ViewportId::from_hash_of("metadata_viewport"),
            builder,
            |viewport_context, class| {
                crate::ui::metadata_popup::draw_metadata_window_viewport(
                    viewport_context,
                    class,
                    self,
                );
                let info = viewport_context.input(|input| input.viewport().clone());
                (info.close_requested(), info.inner_rect, info.outer_rect)
            },
        );

        self.update_metadata_window_geometry(inner_rect, outer_rect);
        if close_requested {
            self.set_metadata_window_open(false);
        }
    }

    pub(crate) fn draw_playlist_window_contents(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
    ) {
        self.handle_dropped_playlist_files(context);
        let mut repeat_mode_changed = false;

        ui.horizontal(|ui| {
            if ui.button("Add Track").clicked() {
                self.playlist_file_picker_pending = true;
            }

            let can_save = self
                .playlist
                .as_ref()
                .and_then(|playlist| playlist.file_path.as_ref())
                .is_some();
            if ui
                .add_enabled(can_save, egui::Button::new("Save"))
                .clicked()
            {
                self.save_playlist();
            }

            if ui.button("Save As").clicked() {
                self.save_playlist_as();
            }

            if ui.button("Clear").clicked() {
                self.new_playlist();
            }

            let can_remove = !self.playlist_selected_indices.is_empty();
            if ui
                .add_enabled(can_remove, egui::Button::new("Remove"))
                .clicked()
            {
                self.remove_selected_playlist_entries();
            }

            if let Some(playlist) = self.playlist.as_mut() {
                let mut loop_enabled = playlist.loop_enabled;
                if ui
                    .add(egui::Button::new("Loop Playlist").selected(loop_enabled))
                    .clicked()
                {
                    loop_enabled = !loop_enabled;
                    playlist.loop_enabled = loop_enabled;
                    if loop_enabled {
                        playlist.set_repeat_mode(PlaylistRepeatMode::Playlist);
                        repeat_mode_changed = true;
                    } else if playlist.repeat_mode == PlaylistRepeatMode::Playlist {
                        playlist.set_repeat_mode(PlaylistRepeatMode::Off);
                        repeat_mode_changed = true;
                    }
                    playlist.dirty = true;
                }

                ui.separator();
                let mut order_mode = playlist.order_mode;
                egui::ComboBox::from_id_salt("playlist_order_mode")
                    .selected_text(match order_mode {
                        PlaylistOrderMode::Sequential => "Sequential",
                        PlaylistOrderMode::Shuffle => "Shuffle",
                        PlaylistOrderMode::Random => "Random",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut order_mode,
                            PlaylistOrderMode::Sequential,
                            "Sequential",
                        );
                        ui.selectable_value(&mut order_mode, PlaylistOrderMode::Shuffle, "Shuffle");
                        ui.selectable_value(&mut order_mode, PlaylistOrderMode::Random, "Random");
                    });
                if order_mode != playlist.order_mode {
                    playlist.set_order_mode(order_mode);
                }

                let mut repeat_mode = playlist.repeat_mode;
                egui::ComboBox::from_id_salt("playlist_repeat_mode")
                    .selected_text(match repeat_mode {
                        PlaylistRepeatMode::Off => "Repeat Off",
                        PlaylistRepeatMode::Track => "Repeat Track",
                        PlaylistRepeatMode::Playlist => "Repeat Playlist",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut repeat_mode,
                            PlaylistRepeatMode::Off,
                            "Repeat Off",
                        );
                        ui.selectable_value(
                            &mut repeat_mode,
                            PlaylistRepeatMode::Track,
                            "Repeat Track",
                        );
                        ui.selectable_value(
                            &mut repeat_mode,
                            PlaylistRepeatMode::Playlist,
                            "Repeat Playlist",
                        );
                    });
                if repeat_mode != playlist.repeat_mode {
                    playlist.set_repeat_mode(repeat_mode);
                    playlist.loop_enabled = repeat_mode == PlaylistRepeatMode::Playlist;
                    repeat_mode_changed = true;
                }
            }
        });

        if repeat_mode_changed {
            self.sync_loop_button_from_playlist_repeat_mode(true);
        }

        ui.separator();

        let mut drop_reorder = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let playlist_len = self.playlist.as_ref().map(|p| p.entries.len()).unwrap_or(0);
            for index in 0..playlist_len {
                let (status_icon, file_type, is_current, display_name, path_text) = {
                    let playlist = self.playlist.as_mut().expect("playlist initialized");
                    let entry = &mut playlist.entries[index];
                    entry.refresh_status();
                    let status_icon = match entry.status {
                        crate::playlist::PlaylistEntryStatus::CurrentlyPlaying => "►",
                        crate::playlist::PlaylistEntryStatus::Valid => "✓",
                        crate::playlist::PlaylistEntryStatus::Missing => "✗",
                    };
                    let file_type = entry
                        .file_path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext.to_ascii_lowercase())
                        .unwrap_or_else(|| "other".to_owned());
                    (
                        status_icon.to_owned(),
                        file_type,
                        Some(index) == playlist.current_index,
                        entry.display_name.clone(),
                        entry.file_path.display().to_string(),
                    )
                };

                ui.horizontal(|ui| {
                    let drag = ui
                        .add(egui::Label::new("⇅").sense(egui::Sense::drag()))
                        .on_hover_text("Drag to reorder");
                    if drag.drag_started() {
                        self.playlist_drag_from = Some(index);
                    }
                    if drag.hovered() && ui.input(|input| input.pointer.any_released()) {
                        if let Some(from_index) = self.playlist_drag_from.take() {
                            if from_index != index {
                                drop_reorder = Some((from_index, index));
                            }
                        }
                    }

                    ui.label(status_icon);

                    let selected = self.playlist_selected_indices.contains(&index);
                    let label = if is_current {
                        format!("{display_name}  [{file_type}]  (current)")
                    } else {
                        format!("{display_name}  [{file_type}]")
                    };
                    let response = ui.selectable_label(selected, label);
                    response.clone().on_hover_text(path_text);

                    if response.clicked() {
                        if ui.input(|input| input.modifiers.ctrl || input.modifiers.command) {
                            if selected {
                                self.playlist_selected_indices.remove(&index);
                            } else {
                                self.playlist_selected_indices.insert(index);
                            }
                        } else {
                            self.playlist_selected_indices.clear();
                            self.playlist_selected_indices.insert(index);
                        }
                    }

                    if response.double_clicked() {
                        let autoplay = self.run_state == AppRunState::Playing;
                        self.load_playlist_track_by_index(index, false, autoplay);
                    }
                });
            }
        });

        if let Some((from_index, to_index)) = drop_reorder {
            if let Some(playlist) = self.playlist.as_mut() {
                playlist.move_entry(from_index, to_index);
            }
        }
    }

    pub(crate) fn draw_metadata_window_contents(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let button_row_height = ui.spacing().interact_size.y + ui.spacing().item_spacing.y * 3.0;
        let scroll_height = (ui.available_height() - button_row_height).max(0.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(scroll_height)
            .show(ui, |ui| {
                ui.columns(2, |columns| {
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Project Title",
                        &mut self.metadata_edit_state.project_name,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Source Filename",
                        &mut self.metadata_edit_state.source_filename,
                    );
                    columns[0].separator();
                    columns[0].label("Tagged Metadata");
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Artist",
                        &mut self.metadata_edit_state.artist,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Album",
                        &mut self.metadata_edit_state.album,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Album Artist",
                        &mut self.metadata_edit_state.album_artist,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Composer",
                        &mut self.metadata_edit_state.composer,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Conductor",
                        &mut self.metadata_edit_state.conductor,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Genre",
                        &mut self.metadata_edit_state.genre,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Date",
                        &mut self.metadata_edit_state.date,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Track Number",
                        &mut self.metadata_edit_state.track_number,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[0],
                        "Track Total",
                        &mut self.metadata_edit_state.track_total,
                    );

                    changed |= draw_metadata_text_field(
                        &mut columns[1],
                        "Disc Number",
                        &mut self.metadata_edit_state.disc_number,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[1],
                        "Disc Total",
                        &mut self.metadata_edit_state.disc_total,
                    );
                    changed |= draw_metadata_multiline_field(
                        &mut columns[1],
                        "Description",
                        &mut self.metadata_edit_state.description,
                        3,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[1],
                        "Copyright",
                        &mut self.metadata_edit_state.copyright,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[1],
                        "Publisher",
                        &mut self.metadata_edit_state.publisher,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[1],
                        "Encoded By",
                        &mut self.metadata_edit_state.encoded_by,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[1],
                        "Encoder",
                        &mut self.metadata_edit_state.encoder,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[1],
                        "Language",
                        &mut self.metadata_edit_state.language,
                    );
                    changed |= draw_metadata_multiline_field(
                        &mut columns[1],
                        "Lyrics",
                        &mut self.metadata_edit_state.lyrics,
                        4,
                    );
                    changed |= draw_metadata_text_field(
                        &mut columns[1],
                        "URL",
                        &mut self.metadata_edit_state.url,
                    );
                    changed |= draw_metadata_multiline_field(
                        &mut columns[1],
                        "Notes",
                        &mut self.metadata_edit_state.notes,
                        6,
                    );
                });

                ui.separator();
                changed |= draw_metadata_field_list(
                    ui,
                    "Additional Fields",
                    "Add Field",
                    &mut self.metadata_edit_state.extra_fields,
                );

                if self.metadata_edit_state.supports_native_metadata {
                    ui.separator();
                    changed |= draw_metadata_field_list(
                        ui,
                        "Native Audio Tags",
                        "Add Native Tag",
                        &mut self.metadata_edit_state.native_metadata,
                    );
                }
            });

        if changed {
            self.project_title = self.metadata_edit_state.project_name.clone();
            self.metadata_edit_dirty = true;
            self.mark_dirty();
        }

        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                self.project_title = self.metadata_edit_state.project_name.clone();
                self.metadata_edit_dirty = true;
                self.mark_dirty();
            }
            if ui.button("Close").clicked() {
                self.set_metadata_window_open(false);
            }
        });
    }

    pub fn toggle_metadata_popup(&mut self) {
        self.set_metadata_window_open(!self.metadata_popup_open);
    }

    fn sync_playback_output_volume(&mut self) {
        if let Err(error) = self
            .playback
            .set_session_output_gain(self.final_output_volume_multiplier())
        {
            self.set_status(format!("{error}"));
        }
    }
}
