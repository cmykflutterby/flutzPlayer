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
    time::{Duration, Instant},
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
    builtin_registry, read_flutz_wrapper, write_flutz_wrapper, ContentKind,
    read_track_metadata_with_symphonia, DecodedAudioStreamSource, FlutzAudioWrapper,
    FormatDescriptor, LoopMode as MediaLoopMode, LoopUnit, MasteringCapability, MediaLoop,
    MetadataField, NativeMetadata, SourceAudioBlock, TrackMetadata,
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
use crate::routing::compute_strip_routing;

const IDLE_MEMORY_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5);
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
    perf_trace: PerfTrace,
    latency_trace: LatencyTrace,
    logged_unmatched_snapshot_strips: BTreeSet<RealtimeStripKey>,
    last_idle_memory_maintenance: Instant,
    status: String,
    missing_preset_warning: Option<String>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataEditState {
    pub project_name: String,
    pub source_filename: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub composer: String,
    pub conductor: String,
    pub genre: String,
    pub date: String,
    pub track_number: String,
    pub track_total: String,
    pub disc_number: String,
    pub disc_total: String,
    pub description: String,
    pub copyright: String,
    pub publisher: String,
    pub encoded_by: String,
    pub encoder: String,
    pub language: String,
    pub lyrics: String,
    pub url: String,
    pub notes: String,
    pub extra_fields: Vec<MetadataField>,
    pub native_metadata: NativeMetadata,
    pub supports_native_metadata: bool,
}

impl MetadataEditState {
    fn from_track_metadata(
        metadata: &TrackMetadata,
        fallback_project_name: String,
        fallback_source_filename: String,
    ) -> Self {
        Self {
            project_name: if metadata.project_name.trim().is_empty() {
                fallback_project_name
            } else {
                metadata.project_name.clone()
            },
            source_filename: if metadata.source_filename.trim().is_empty() {
                fallback_source_filename
            } else {
                metadata.source_filename.clone()
            },
            artist: metadata.artist.clone(),
            album: metadata.album.clone(),
            album_artist: metadata.album_artist.clone(),
            composer: metadata.composer.clone(),
            conductor: metadata.conductor.clone(),
            genre: metadata.genre.clone(),
            date: metadata.date.clone(),
            track_number: metadata.track_number.clone(),
            track_total: metadata.track_total.clone(),
            disc_number: metadata.disc_number.clone(),
            disc_total: metadata.disc_total.clone(),
            description: metadata.description.clone(),
            copyright: metadata.copyright.clone(),
            publisher: metadata.publisher.clone(),
            encoded_by: metadata.encoded_by.clone(),
            encoder: metadata.encoder.clone(),
            language: metadata.language.clone(),
            lyrics: metadata.lyrics.clone(),
            url: metadata.url.clone(),
            notes: metadata.notes.clone(),
            extra_fields: metadata.extra_fields.clone(),
            native_metadata: Vec::new(),
            supports_native_metadata: false,
        }
    }

    fn from_fmid_project(
        project: &FmidProjectRecord,
        fallback_project_name: String,
        fallback_source_filename: String,
    ) -> Self {
        Self {
            project_name: if project.project_name.trim().is_empty() {
                fallback_project_name
            } else {
                project.project_name.clone()
            },
            source_filename: if project.source_midi_filename.trim().is_empty() {
                fallback_source_filename
            } else {
                project.source_midi_filename.clone()
            },
            artist: project.artist.clone(),
            album: project.album.clone(),
            album_artist: project.album_artist.clone(),
            composer: project.composer.clone(),
            conductor: project.conductor.clone(),
            genre: project.genre.clone(),
            date: project.date.clone(),
            track_number: project.track_number.clone(),
            track_total: project.track_total.clone(),
            disc_number: project.disc_number.clone(),
            disc_total: project.disc_total.clone(),
            description: project.description.clone(),
            copyright: project.copyright.clone(),
            publisher: project.publisher.clone(),
            encoded_by: project.encoded_by.clone(),
            encoder: project.encoder.clone(),
            language: project.language.clone(),
            lyrics: project.lyrics.clone(),
            url: project.url.clone(),
            notes: project.notes.clone(),
            extra_fields: project
                .extra_fields
                .iter()
                .map(|(key, value)| MetadataField {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
            native_metadata: Vec::new(),
            supports_native_metadata: false,
        }
    }

    fn with_native_metadata(mut self, native_metadata: NativeMetadata) -> Self {
        self.native_metadata = native_metadata;
        self.supports_native_metadata = true;
        self
    }

    fn to_track_metadata(
        &self,
        fallback_project_name: &str,
        fallback_source_filename: &str,
    ) -> TrackMetadata {
        TrackMetadata {
            project_name: first_non_empty(&self.project_name, fallback_project_name),
            source_filename: first_non_empty(&self.source_filename, fallback_source_filename),
            artist: self.artist.clone(),
            album: self.album.clone(),
            album_artist: self.album_artist.clone(),
            composer: self.composer.clone(),
            conductor: self.conductor.clone(),
            genre: self.genre.clone(),
            date: self.date.clone(),
            track_number: self.track_number.clone(),
            track_total: self.track_total.clone(),
            disc_number: self.disc_number.clone(),
            disc_total: self.disc_total.clone(),
            description: self.description.clone(),
            copyright: self.copyright.clone(),
            publisher: self.publisher.clone(),
            encoded_by: self.encoded_by.clone(),
            encoder: self.encoder.clone(),
            language: self.language.clone(),
            lyrics: self.lyrics.clone(),
            url: self.url.clone(),
            notes: self.notes.clone(),
            extra_fields: normalized_metadata_fields(&self.extra_fields),
        }
    }

    fn to_fmid_project(
        &self,
        fallback_project_name: &str,
        fallback_source_filename: &str,
    ) -> FmidProjectRecord {
        FmidProjectRecord {
            project_name: first_non_empty(&self.project_name, fallback_project_name),
            source_midi_filename: first_non_empty(&self.source_filename, fallback_source_filename),
            artist: self.artist.clone(),
            album: self.album.clone(),
            album_artist: self.album_artist.clone(),
            composer: self.composer.clone(),
            conductor: self.conductor.clone(),
            genre: self.genre.clone(),
            date: self.date.clone(),
            track_number: self.track_number.clone(),
            track_total: self.track_total.clone(),
            disc_number: self.disc_number.clone(),
            disc_total: self.disc_total.clone(),
            description: self.description.clone(),
            copyright: self.copyright.clone(),
            publisher: self.publisher.clone(),
            encoded_by: self.encoded_by.clone(),
            encoder: self.encoder.clone(),
            language: self.language.clone(),
            lyrics: self.lyrics.clone(),
            url: self.url.clone(),
            project_flags: 0,
            notes: self.notes.clone(),
            extra_fields: normalized_metadata_fields(&self.extra_fields)
                .into_iter()
                .map(|field| (field.key, field.value))
                .collect(),
        }
    }
}

fn first_non_empty(primary: &str, fallback: &str) -> String {
    if primary.trim().is_empty() {
        fallback.to_owned()
    } else {
        primary.to_owned()
    }
}

fn normalized_metadata_fields(fields: &[MetadataField]) -> Vec<MetadataField> {
    fields
        .iter()
        .filter_map(|field| {
            let key = field.key.trim();
            let value = field.value.trim();
            if key.is_empty() && value.is_empty() {
                None
            } else {
                Some(MetadataField {
                    key: key.to_owned(),
                    value: value.to_owned(),
                })
            }
        })
        .collect()
}

fn draw_metadata_text_field(ui: &mut egui::Ui, label: &str, value: &mut String) -> bool {
    ui.label(label);
    ui.text_edit_singleline(value).changed()
}

fn draw_metadata_multiline_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    rows: usize,
) -> bool {
    ui.label(label);
    ui.add(egui::TextEdit::multiline(value).desired_rows(rows))
        .changed()
}

fn draw_metadata_field_list(
    ui: &mut egui::Ui,
    heading: &str,
    add_label: &str,
    fields: &mut Vec<MetadataField>,
) -> bool {
    let mut changed = false;
    ui.label(heading);
    let mut remove_index = None;
    for (index, field) in fields.iter_mut().enumerate() {
        ui.push_id((heading, index), |ui| {
            ui.horizontal(|ui| {
                changed |= ui.text_edit_singleline(&mut field.key).changed();
                changed |= ui.text_edit_singleline(&mut field.value).changed();
                if ui.button("x").clicked() {
                    remove_index = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove_index {
        fields.remove(index);
        changed = true;
    }
    if ui.button(add_label).clicked() {
        fields.push(MetadataField::default());
        changed = true;
    }
    changed
}

#[derive(Debug, Clone, PartialEq)]
struct LoadedContentState {
    kind: ContentKind,
    format_id: String,
    friendly_name: String,
    mastering: MasteringCapability,
    wrapped_extension: Option<String>,
    decoded_wrapper: Option<FlutzAudioWrapper>,
    decoded_source_path: Option<PathBuf>,
    decoded_source_bytes: Option<Arc<[u8]>>,
}

struct PendingDecodedAudioLoad {
    token: u64,
    receiver: mpsc::Receiver<DecodedAudioLoadJobResult>,
    resume_after_load: bool,
    playlist_entry_load: bool,
}

struct PendingPlaylistPreload {
    token: u64,
    index: usize,
    path: PathBuf,
    receiver: mpsc::Receiver<DecodedAudioLoadJobResult>,
}

struct ReadyPlaylistPreload {
    token: u64,
    index: usize,
    path: PathBuf,
    result: DecodedAudioLoadJobResult,
}

struct DecodedAudioLoadJobResult {
    token: u64,
    path: PathBuf,
    path_display: String,
    descriptor: FormatDescriptor,
    is_wrapped: bool,
    wrapper: Option<FlutzAudioWrapper>,
    stream_source: Option<DecodedAudioStreamSource>,
    source_bytes: Option<Arc<[u8]>>,
    error: Option<String>,
    error_stage: &'static str,
}

fn prepare_decoded_audio_load_job(
    token: u64,
    path: PathBuf,
    descriptor: FormatDescriptor,
) -> DecodedAudioLoadJobResult {
    let path_display = path.display().to_string();
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    let is_wrapped = extension.as_deref().is_some_and(|ext| {
        descriptor
            .wrapped_extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(ext))
    });
    let mut result = DecodedAudioLoadJobResult {
        token,
        path: path.clone(),
        path_display,
        descriptor,
        is_wrapped,
        wrapper: None,
        stream_source: None,
        source_bytes: None,
        error: None,
        error_stage: "prepare",
    };

    if is_wrapped {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                result.error = Some(format!("failed to read {}: {error}", path.display()));
                result.error_stage = "read";
                return result;
            }
        };
        let mut wrapper = match read_flutz_wrapper(&bytes) {
            Ok(wrapper) => wrapper,
            Err(error) => {
                result.error = Some(format!("{error}"));
                result.error_stage = "parse";
                return result;
            }
        };
        let hint_extension = Path::new(&wrapper.source.original_filename)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_owned)
            .or_else(|| descriptor.extensions.first().map(|ext| (*ext).to_owned()));
        let source_bytes = Arc::<[u8]>::from(std::mem::take(&mut wrapper.source.bytes));
        result.stream_source = Some(DecodedAudioStreamSource::Bytes {
            bytes: Arc::clone(&source_bytes),
            hint_extension,
        });
        result.source_bytes = Some(source_bytes);
        result.wrapper = Some(wrapper);
        return result;
    }

    let source_filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("embedded.audio")
        .to_owned();
    let fallback_project_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(&source_filename)
        .to_owned();
    let (metadata, native_metadata) = read_track_metadata_with_symphonia(
        DecodedAudioStreamSource::Path(path.clone()),
        descriptor.id,
        &fallback_project_name,
        &source_filename,
    )
    .unwrap_or_else(|_| {
        (
            TrackMetadata {
                project_name: fallback_project_name.clone(),
                source_filename: source_filename.clone(),
                ..TrackMetadata::default()
            },
            Vec::new(),
        )
    });
    result.wrapper = Some(FlutzAudioWrapper {
        source: SourceAudioBlock {
            format_id: descriptor.id.to_owned(),
            original_filename: source_filename.clone(),
            media_type: format!("audio/{}", descriptor.id),
            bytes: Vec::new(),
        },
        metadata,
        native_metadata,
        ..FlutzAudioWrapper::default()
    });
    result.stream_source = Some(DecodedAudioStreamSource::Path(path));
    result
}

#[derive(Debug, Clone, PartialEq)]
struct PeqEditState {
    preset: PeqPresetFile,
    source: PeqConfigSource,
    preset_path: Option<PathBuf>,
    dirty: bool,
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
struct DecodedAudioPeqPreferences {
    #[serde(default)]
    bypass: bool,
    #[serde(default = "default_preferences_default_peq_preset")]
    default_preset: PeqPresetFile,
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
struct WindowViewPrefs {
    #[serde(default)]
    playlist_open: bool,
    #[serde(default)]
    metadata_open: bool,
    #[serde(default)]
    playlist_position: Option<[f32; 2]>,
    #[serde(default)]
    playlist_size: Option<[f32; 2]>,
    #[serde(default)]
    metadata_position: Option<[f32; 2]>,
    #[serde(default)]
    metadata_size: Option<[f32; 2]>,
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
struct PreferencesFile {
    #[serde(default = "default_preferences_default_preset_id")]
    default_preset_id: String,
    #[serde(default)]
    decoded_audio_peq: DecodedAudioPeqPreferences,
    #[serde(default)]
    window: WindowViewPrefs,
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

fn default_preferences_default_preset_id() -> String {
    default_preset_set().default_preset().id.to_owned()
}

fn default_preferences_default_peq_preset() -> PeqPresetFile {
    normalize_default_decoded_peq_preset(load_builtin_decoded_peq_preset("Default"))
}

impl FlutzDesktopApp {
    fn new(creation_context: &eframe::CreationContext<'_>, config: AppStartupConfig) -> Self {
        crate::ui::apply_theme(&creation_context.egui_ctx);
        let catalog_soundfonts = match &config.dat_summary {
            DatStartupSummary::Available { soundfonts, .. } => soundfonts.clone(),
            DatStartupSummary::Unavailable(_) => Vec::new(),
        };
        let data_summary = match &config.dat_summary {
            DatStartupSummary::Available {
                dat_file_count,
                dat_byte_count,
                soundfonts,
            } => format!(
                "{} DAT file(s), {} byte(s), {} soundfont(s)",
                dat_file_count,
                dat_byte_count,
                soundfonts.len()
            ),
            DatStartupSummary::Unavailable(message) => message.clone(),
        };
        let preset_set = default_preset_set();
        let preferences =
            Self::load_or_create_preferences_from_data_dir(&config.data_dir, preset_set);
        let default_decoded_peq_preset = preferences.decoded_audio_peq.default_preset.clone();
        let default_preset = preset_set
            .find_preset(&preferences.default_preset_id)
            .unwrap_or_else(|| preset_set.default_preset());
        let soundfonts = soundfont_rows_from_catalog(&catalog_soundfonts, default_preset.font_ids);
        let status = match &config.dat_summary {
            DatStartupSummary::Available { soundfonts, .. } if soundfonts.is_empty() => {
                "No bundled soundfonts found in data folder".to_owned()
            }
            DatStartupSummary::Available { .. } => "Ready".to_owned(),
            DatStartupSummary::Unavailable(message) => message.clone(),
        };
        let mut app = Self {
            run_state: AppRunState::Idle,
            release_edit_mode: false,
            release_editor_panels_collapsed: false,
            speaker_volume_popup_open: false,
            final_output_volume_percent: 0.0,
            release_loop_popup_open: false,
            loaded_content: None,
            project_title: "Untitled MIDI".to_owned(),
            current_path: None,
            dirty: false,
            transport_position: 0.0,
            loop_enabled: false,
            loop_mode: LoopMode::None,
            loop_start_tick: 0,
            loop_end_tick: 0,
            loop_count: 1,
            master: MasterControls::default(),
            smart_mix: SmartMixControls::default(),
            mixer_fx_expanded: false,
            soundfonts,
            catalog_soundfonts: catalog_soundfonts.clone(),
            preset_set,
            active_preset_id: default_preset.id.to_owned(),
            selected_preset_id: default_preset.id.to_owned(),
            default_preset_id: default_preset.id.to_owned(),
            default_decoded_peq_preset,
            decoded_peq_bypass_enabled: preferences.decoded_audio_peq.bypass,
            coverage_cache: BTreeMap::new(),
            selected_soundfont: 0,
            mixer_assignment_mode: MixerAssignmentMode::Manual,
            data_dir: config.data_dir.clone(),
            data_summary,
            playback: PlaybackController::new(
                config.data_dir,
                catalog_soundfonts,
                config.audio_backend,
                config.debug_analyzer,
                config.debug_render_errors,
            ),
            perf_trace: PerfTrace::new(config.debug_memory),
            latency_trace: LatencyTrace::new(config.debug_latency),
            logged_unmatched_snapshot_strips: BTreeSet::new(),
            last_idle_memory_maintenance: Instant::now(),
            status,
            missing_preset_warning: None,
            playlist: None,
            window_view_prefs: WindowViewPrefs {
                playlist_open: preferences.window.playlist_open,
                metadata_open: preferences.window.metadata_open,
                playlist_position: preferences.window.playlist_position,
                playlist_size: preferences.window.playlist_size,
                metadata_position: preferences.window.metadata_position,
                metadata_size: preferences.window.metadata_size,
            },
            playlist_window_open: preferences.window.playlist_open,
            playlist_file_picker_pending: false,
            playlist_selected_indices: BTreeSet::new(),
            playlist_drag_from: None,
            metadata_popup_open: preferences.window.metadata_open,
            metadata_edit_dirty: false,
            metadata_edit_state: MetadataEditState::default(),
            peq_edit_state: PeqEditState::default(),
            pending_decoded_audio_load: None,
            pending_playlist_preload: None,
            ready_playlist_preload: None,
            last_playback_active: false,
        };

        if let Some(path) = config.launch_open_path {
            let startup_path = path.clone();
            let startup_extension = startup_path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_ascii_lowercase());
            app.open_project_path_with_resume(path, true);
            if app.has_loaded_project() && app.run_state != AppRunState::Playing {
                if startup_extension.as_deref() != Some("fplist") {
                    app.regenerate_playlist_with_single_entry(startup_path, true);
                }
            }
        }

        app
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn load_or_create_preferences_from_data_dir(
        data_dir: &Path,
        preset_set: &'static PresetSet,
    ) -> PreferencesFile {
        let path = data_dir.join(Self::preferences_file_name());
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(preferences) = toml::from_str::<PreferencesFile>(&text) {
                let preferences = Self::normalize_preferences(preferences);
                let rendered = Self::render_preferences_file(&preferences, preset_set);
                if rendered != text {
                    let _ = fs::write(&path, rendered);
                }
                return preferences;
            }

            let preferences = Self::normalize_preferences(PreferencesFile::default());
            let _ = fs::write(
                &path,
                Self::render_preferences_file(&preferences, preset_set),
            );
            return preferences;
        }

        let preferences = Self::normalize_preferences(PreferencesFile::default());
        let _ = fs::write(
            &path,
            Self::render_preferences_file(&preferences, preset_set),
        );
        preferences
    }

    fn preferences_file_name() -> &'static str {
        "preferences.ini"
    }

    fn preferences_path(&self) -> PathBuf {
        self.data_dir.join(Self::preferences_file_name())
    }

    fn render_preferences_file(
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
        writeln!(&mut output, "# decoded_audio_peq.bypass persists the global Bypass EQ toggle.").unwrap();
        writeln!(&mut output).unwrap();
        output.push_str(
            &toml::to_string_pretty(preferences)
                .expect("preferences serialization should succeed"),
        );
        output
    }

    fn normalize_preferences(mut preferences: PreferencesFile) -> PreferencesFile {
        preferences.decoded_audio_peq.default_preset =
            normalize_default_decoded_peq_preset(preferences.decoded_audio_peq.default_preset);
        preferences
    }

    fn build_preferences(&self) -> PreferencesFile {
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

    fn persist_window_view_prefs(&self) {
        let path = self.preferences_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let preferences = self.build_preferences();
        let _ = fs::write(
            path,
            Self::render_preferences_file(&preferences, self.preset_set),
        );
    }

    fn set_playlist_window_open(&mut self, open: bool) {
        if self.playlist_window_open == open {
            return;
        }
        self.playlist_window_open = open;
        self.window_view_prefs.playlist_open = open;
        self.persist_window_view_prefs();
    }

    fn set_metadata_window_open(&mut self, open: bool) {
        if self.metadata_popup_open == open {
            return;
        }
        self.metadata_popup_open = open;
        self.window_view_prefs.metadata_open = open;
        self.persist_window_view_prefs();
    }

    fn update_playlist_window_geometry(
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

    fn update_metadata_window_geometry(
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

    fn tick_idle_memory_maintenance(&mut self) {
        if self.playback.playback_active()
            || self.last_idle_memory_maintenance.elapsed() < IDLE_MEMORY_MAINTENANCE_INTERVAL
        {
            return;
        }
        crate::memory_runtime::decay_all_idle_domains();
        self.last_idle_memory_maintenance = Instant::now();
    }
}

fn soundfont_rows_from_catalog(
    catalog_soundfonts: &[SoundFontCatalogEntry],
    ids: &[&str],
) -> Vec<SoundFontUiRow> {
    if ids.is_empty() {
        return vec![catalog_soundfonts
            .iter()
            .find(|font| font.is_default)
            .or_else(|| catalog_soundfonts.first())
            .map(SoundFontUiRow::from_catalog_entry)
            .unwrap_or_else(SoundFontUiRow::default_retro)];
    }

    ids.iter()
        .map(|id| {
            catalog_soundfonts
                .iter()
                .find(|entry| entry.internal_id == *id)
                .map(SoundFontUiRow::from_catalog_entry)
                .unwrap_or_else(|| SoundFontUiRow {
                    internal_id: (*id).to_owned(),
                    display_name: (*id).to_owned(),
                    is_default: false,
                    collapsed: true,
                    muted: false,
                    soloed: false,
                    strips: Vec::new(),
                })
        })
        .collect()
}

impl eframe::App for FlutzDesktopApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::AppUpdate);
        self.process_playlist_file_picker_pending();
        self.process_pending_decoded_audio_load();
        self.process_pending_playlist_preload();
        self.refresh_realtime_feedback();
        self.maybe_start_playlist_preload();
        let mixer_snapshot = self.mixer_edit_snapshot();
        {
            let _allocation_scope = AllocationScopeGuard::enter(AllocationScope::Ui);
            crate::ui::draw_app(context, self);
        }
        self.apply_mixer_edit_snapshot(mixer_snapshot);
        self.sync_playback_controls();
        self.handle_playback_end_transition();
        self.tick_idle_memory_maintenance();
        context.request_repaint_after(std::time::Duration::from_millis(16));
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct MixerEditSnapshot {
    master: MasterControls,
    smart_mix: SmartMixControls,
    rows: Vec<RowEditSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
struct RowEditSnapshot {
    internal_id: String,
    muted: bool,
    soloed: bool,
    strips: Vec<StripEditSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
struct StripEditSnapshot {
    channel: u8,
    bank: u16,
    program: u8,
    is_percussion: bool,
    volume: f32,
    muted: bool,
    soloed: bool,
    pan: f32,
    gain_db: f32,
    limiter_enabled: bool,
    limiter_amount: f32,
    reverb: f32,
    chorus: f32,
}

#[derive(Debug, Copy, Clone, PartialEq)]
struct PlaybackReloadResumeState {
    run_state: AppRunState,
    transport_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RealtimeStripKey {
    soundfont_id: String,
    channel: u8,
    bank: u16,
    program: u8,
    is_percussion: bool,
}

impl RealtimeStripKey {
    fn from_snapshot(snapshot: &crate::playback::RealtimeStripSnapshot) -> Self {
        Self {
            soundfont_id: snapshot.soundfont_id.clone(),
            channel: snapshot.midi_channel,
            bank: snapshot.midi_bank,
            program: snapshot.midi_program,
            is_percussion: snapshot.is_percussion,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LoopMode {
    None,
    Infinite,
    Counted,
}

// Mixer Final Volume Values
pub const MASTER_VOLUME_MAX_DB: f32 = 32.0; // 24
pub const FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER: f32 = 10.0; // 2

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum MixerAssignmentMode {
    #[default]
    Manual,
    Balance,
    Layer,
}

impl LoopMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Infinite => "inf",
            Self::Counted => "n",
        }
    }

    fn playback_mode(self) -> PlaybackLoopMode {
        match self {
            Self::None => PlaybackLoopMode::None,
            Self::Infinite => PlaybackLoopMode::Infinite,
            Self::Counted => PlaybackLoopMode::Counted,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MasterControls {
    pub volume_db: f32,
    pub limiter_enabled: bool,
    pub limiter_amount: f32,
    pub reverb: f32,
    pub chorus: f32,
    pub eq_low: f32,
    pub eq_mid: f32,
    pub eq_high: f32,
}

impl Default for MasterControls {
    fn default() -> Self {
        Self {
            volume_db: -6.0,
            limiter_enabled: false,
            limiter_amount: 0.25,
            reverb: 0.0,
            chorus: 0.0,
            eq_low: 0.0,
            eq_mid: 0.0,
            eq_high: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SmartMixControls {
    pub enabled: bool,
    pub auto_normalize: bool,
    pub target_headroom_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub lookahead_ms: f32,
    pub normalization_amount: f32,
}

impl Default for SmartMixControls {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_normalize: false,
            target_headroom_db: -6.0,
            attack_ms: 1.0,
            release_ms: 10.0,
            lookahead_ms: 500.0,
            normalization_amount: 25.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SoundFontUiRow {
    pub internal_id: String,
    pub display_name: String,
    pub is_default: bool,
    pub collapsed: bool,
    pub muted: bool,
    pub soloed: bool,
    pub strips: Vec<MixerStripUiState>,
}

impl SoundFontUiRow {
    fn from_catalog_entry(entry: &SoundFontCatalogEntry) -> Self {
        Self {
            internal_id: entry.internal_id.clone(),
            display_name: entry.display_name.clone(),
            is_default: entry.is_default,
            collapsed: true,
            muted: false,
            soloed: false,
            strips: Vec::new(),
        }
    }

    fn default_retro() -> Self {
        Self {
            internal_id: "retro_gm".to_owned(),
            display_name: "Retro GM".to_owned(),
            is_default: true,
            collapsed: true,
            muted: false,
            soloed: false,
            strips: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MixerStripUiState {
    pub strip_id: StripId,
    pub channel: u8,
    pub bank: u16,
    pub program: u8,
    pub is_percussion: bool,
    pub program_name: String,
    pub volume: f32,
    pub muted: bool,
    pub unsupported: bool,
    pub mute_policy_default: bool,
    pub volume_policy_default: bool,
    pub soloed: bool,
    pub pan: f32,
    pub gain_db: f32,
    pub limiter_enabled: bool,
    pub limiter_amount: f32,
    pub reverb: f32,
    pub chorus: f32,
    pub meter: f32,
    pub active: bool,
    pub current_note: String,
}

impl MixerStripUiState {
    #[allow(dead_code)]
    fn for_channel(soundfont_index: usize, channel_index: usize) -> Self {
        let channel = channel_index as u8;
        Self {
            strip_id: strip_id_for(soundfont_index, channel, 0, 0, channel == 9),
            channel,
            bank: 0,
            program: 0,
            is_percussion: channel == 9,
            program_name: if channel == 9 {
                "Percussion".to_owned()
            } else {
                "Acoustic Grand".to_owned()
            },
            volume: 1.0,
            muted: false,
            unsupported: false,
            mute_policy_default: true,
            volume_policy_default: true,
            soloed: false,
            pan: 0.0,
            gain_db: 0.0,
            limiter_enabled: false,
            limiter_amount: 0.25,
            reverb: 0.0,
            chorus: 0.0,
            meter: 0.0,
            active: false,
            current_note: "--".to_owned(),
        }
    }
}

fn coverage_supports_strip(
    coverage: Option<&SoundFontCoverage>,
    strip: &MixerStripUiState,
) -> bool {
    let Some(coverage) = coverage else {
        return false;
    };

    if strip.is_percussion {
        coverage.provides_percussion()
    } else {
        coverage.provides_melodic(strip.bank, strip.program)
    }
}

fn strip_id_for(
    soundfont_index: usize,
    channel: u8,
    bank: u16,
    program: u8,
    is_percussion: bool,
) -> StripId {
    let soundfont_slot = (soundfont_index as u64 + 1) * 100_000;
    let channel_slot = channel as u64 * 16_384;
    let bank_slot = bank as u64 * 128;
    let percussion_slot = if is_percussion { 50_000 } else { 0 };
    StripId(soundfont_slot + percussion_slot + channel_slot + bank_slot + program as u64 + 1)
}

fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = note as i16 / 12 - 1;
    format!("{}{}", NAMES[note as usize % 12], octave)
}

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

    pub fn run_state(&self) -> AppRunState {
        self.run_state
    }

    pub fn release_edit_mode(&self) -> bool {
        self.release_edit_mode
    }

    pub fn release_editor_panels_collapsed(&self) -> bool {
        self.release_editor_panels_collapsed
    }

    pub fn toggle_release_edit_mode(&mut self) {
        self.release_edit_mode = !self.release_edit_mode;
        self.speaker_volume_popup_open = false;
        self.release_loop_popup_open = false;
        self.status = if self.release_edit_mode {
            "Editor mode".to_owned()
        } else {
            "Player mode".to_owned()
        };
    }

    pub fn speaker_volume_popup_open(&self) -> bool {
        self.speaker_volume_popup_open
    }

    pub fn final_output_volume_percent(&self) -> f32 {
        self.final_output_volume_percent
    }

    pub fn final_output_volume_multiplier(&self) -> f32 {
        let normalized = (self.final_output_volume_percent / 100.0).clamp(-1.0, 1.0);
        if normalized <= 0.0 {
            1.0 + normalized
        } else {
            1.0 + (FINAL_OUTPUT_MAX_VOLUME_MULTIPLIER - 1.0) * normalized
        }
    }

    pub fn toggle_speaker_volume_popup(&mut self) {
        self.speaker_volume_popup_open = !self.speaker_volume_popup_open;
    }

    pub fn set_final_output_volume_percent(&mut self, percent: f32) {
        let clamped = percent.clamp(-100.0, 100.0);
        if (self.final_output_volume_percent - clamped).abs() <= f32::EPSILON {
            return;
        }

        self.final_output_volume_percent = clamped;
        self.sync_playback_output_volume();
    }

    pub fn close_speaker_volume_popup(&mut self) {
        self.speaker_volume_popup_open = false;
    }

    pub fn release_loop_popup_open(&self) -> bool {
        self.release_loop_popup_open
    }

    pub fn toggle_release_loop_popup(&mut self) {
        self.release_loop_popup_open = !self.release_loop_popup_open;
    }

    pub fn close_release_loop_popup(&mut self) {
        self.release_loop_popup_open = false;
    }

    pub fn toggle_release_editor_panels_collapsed(&mut self) {
        self.release_editor_panels_collapsed = !self.release_editor_panels_collapsed;
    }

    pub fn set_run_state(&mut self, run_state: AppRunState) {
        self.run_state = run_state;
        self.status = match run_state {
            AppRunState::Idle => "Stopped".to_owned(),
            AppRunState::Playing => "Playing".to_owned(),
            AppRunState::Paused => "Paused".to_owned(),
        };
    }

    pub fn project_title(&self) -> &str {
        if self.current_path.is_some() && !self.metadata_edit_state.project_name.trim().is_empty() {
            return &self.metadata_edit_state.project_name;
        }
        &self.project_title
    }

    pub fn current_path(&self) -> Option<&str> {
        self.current_path.as_deref()
    }

    pub fn has_loaded_project(&self) -> bool {
        self.current_path.is_some()
    }

    pub fn active_mastering_capability(&self) -> MasteringCapability {
        self.loaded_content
            .as_ref()
            .map(|content| content.mastering)
            .unwrap_or(MasteringCapability::None)
    }

    pub fn active_content_kind(&self) -> Option<ContentKind> {
        self.loaded_content.as_ref().map(|content| content.kind)
    }

    pub fn transport_unit_label(&self) -> &'static str {
        match self.active_content_kind() {
            Some(ContentKind::DecodedAudio | ContentKind::DecodedAudioWrapper) => "sample-frames",
            Some(ContentKind::Midi | ContentKind::Fmid) => "ticks",
            None => "ticks",
        }
    }

    pub fn transport_unit_prefix(&self) -> &'static str {
        match self.active_content_kind() {
            Some(ContentKind::DecodedAudio | ContentKind::DecodedAudioWrapper) => "f:",
            Some(ContentKind::Midi | ContentKind::Fmid) => "t:",
            None => "t:",
        }
    }

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

    pub fn open_midi_dialog(&mut self) {
        let trace = self.begin_user_trace("file.open_dialog");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Song files", &Self::open_dialog_extensions())
            .pick_file()
        else {
            self.set_status("Open canceled");
            self.finish_user_trace(trace, "file.open_dialog", "canceled");
            return;
        };

        if !self.prepare_playlist_for_open_path(&path) {
            self.finish_user_trace(trace, "file.open_dialog", "canceled");
            return;
        }

        let path_display = path.display().to_string();
        self.finish_user_trace(trace, "file.open_dialog", "selected");
        self.open_project_path(path.clone());

        if self
            .current_path
            .as_deref()
            .is_some_and(|current| current == path_display)
            && Self::is_supported_playlist_entry_path(&path)
        {
            self.regenerate_playlist_with_single_entry(path, true);
        }
    }

    pub fn open_project_path(&mut self, path: PathBuf) {
        self.open_project_path_with_resume(path, false);
    }

    fn open_project_path_with_resume(&mut self, path: PathBuf, resume_after_load: bool) {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());
        match extension.as_deref() {
            Some("fplist") => self.load_fplist_path(path),
            Some(ext) => {
                let registry = builtin_registry();
                match registry.find_by_extension(ext).copied() {
                    Some(descriptor) if descriptor.content_kind == ContentKind::Fmid => {
                        self.load_fmid_path(path);
                        if resume_after_load && self.has_loaded_project() {
                            self.play();
                        }
                    }
                    Some(descriptor) if descriptor.content_kind == ContentKind::Midi => {
                        self.load_midi_path(path);
                        if resume_after_load && self.has_loaded_project() {
                            self.play();
                        }
                    }
                    Some(descriptor)
                        if descriptor.mastering == MasteringCapability::DecodedAudioPeq =>
                    {
                        self.load_decoded_audio_path_with_resume(
                            path,
                            descriptor,
                            resume_after_load,
                            false,
                        )
                    }
                    _ => self.set_status("Unsupported file type"),
                }
            }
            None => {
                self.load_midi_path(path);
                if resume_after_load && self.has_loaded_project() {
                    self.play();
                }
            }
        }
    }

    fn regenerate_playlist_with_single_entry(&mut self, path: PathBuf, align_repeat_to_loop: bool) {
        if !Self::is_supported_playlist_entry_path(&path) {
            return;
        }

        let mut playlist = PlaylistState::default();
        playlist.shuffle_seed = 0xC0FFEE_u64;
        playlist.add_entry(path);

        if align_repeat_to_loop {
            let repeat_mode = if self.loop_enabled {
                PlaylistRepeatMode::Track
            } else {
                PlaylistRepeatMode::Off
            };
            playlist.repeat_mode = repeat_mode;
            playlist.loop_enabled = repeat_mode == PlaylistRepeatMode::Playlist;
        }

        playlist.dirty = false;
        self.playlist = Some(playlist);
        self.playlist_selected_indices.clear();
        self.playlist_drag_from = None;
    }

    fn prepare_playlist_for_open_path(&mut self, path: &Path) -> bool {
        if !Self::is_supported_playlist_entry_path(path) {
            return true;
        }

        let (entry_count, playlist_dirty) = self
            .playlist
            .as_ref()
            .map(|playlist| (playlist.entries.len(), playlist.dirty))
            .unwrap_or((0, false));

        if entry_count > 1 && playlist_dirty {
            let choice = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Unsaved Playlist")
                .set_description(
                    "The current playlist has unsaved changes. Save before opening a new file?",
                )
                .set_buttons(rfd::MessageButtons::YesNoCancel)
                .show();

            match choice {
                rfd::MessageDialogResult::Yes => {
                    self.save_playlist();
                    if self
                        .playlist
                        .as_ref()
                        .is_some_and(|playlist| playlist.dirty)
                    {
                        self.set_status("Open canceled: playlist was not saved");
                        return false;
                    }
                }
                rfd::MessageDialogResult::No => {}
                _ => {
                    self.set_status("Open canceled");
                    return false;
                }
            }
        }

        self.regenerate_playlist_with_single_entry(path.to_path_buf(), false);
        true
    }

    fn sync_loop_button_from_playlist_repeat_mode(&mut self, mark_project_dirty: bool) {
        let Some(repeat_mode) = self.playlist.as_ref().map(|playlist| playlist.repeat_mode) else {
            return;
        };

        let enabled = repeat_mode == PlaylistRepeatMode::Track;
        let previous = self.loop_state_tuple();
        self.loop_enabled = enabled;
        if enabled && matches!(self.loop_mode, LoopMode::None) {
            self.loop_mode = LoopMode::Infinite;
        }
        if enabled && self.loop_end_tick == 0 {
            self.loop_end_tick = self.transport_tick_length();
        }
        self.normalize_loop_state(false);
        if mark_project_dirty {
            let _ = self.commit_loop_state_change(previous);
        } else if let Err(error) = self
            .playback
            .set_loop_settings(self.current_loop_settings())
        {
            self.set_status(format!("{error}"));
        }
    }

    pub fn get_next_track_file_path(&mut self) -> Option<PathBuf> {
        let mut preview = self.playlist.clone()?;
        let next_index = preview.next_track_for_mode_by(|entry| {
            Self::is_supported_playlist_entry_path(&entry.file_path)
        })?;
        preview
            .entries
            .get(next_index)
            .map(|entry| entry.file_path.clone())
    }

    pub fn next_track(&mut self) {
        let previous_state = self.run_state;
        let autoplay = previous_state == AppRunState::Playing;
        let loaded = self.navigate_playlist(true, autoplay);
        if loaded && previous_state == AppRunState::Paused {
            self.run_state = AppRunState::Paused;
            self.set_status("Paused");
        }
    }

    pub fn prev_track(&mut self) {
        let previous_state = self.run_state;
        let autoplay = previous_state == AppRunState::Playing;
        let loaded = self.navigate_playlist(false, autoplay);
        if loaded && previous_state == AppRunState::Paused {
            self.run_state = AppRunState::Paused;
            self.set_status("Paused");
        }
    }

    fn navigate_playlist(&mut self, forward: bool, autoplay: bool) -> bool {
        let Some(playlist) = self.playlist.as_mut() else {
            self.set_status("No playlist loaded");
            return false;
        };

        if playlist.is_empty() {
            self.set_status("Playlist is empty");
            return false;
        }

        let target_index = if forward {
            playlist.next_track_for_mode_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
        } else {
            playlist.prev_track_for_mode_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
        };

        let Some(index) = target_index else {
            self.log_playlist_skip_reason();
            return false;
        };

        self.load_playlist_track_by_index(index, true, autoplay)
    }

    fn load_playlist_track_by_index(
        &mut self,
        index: usize,
        push_history: bool,
        autoplay: bool,
    ) -> bool {
        let Some(playlist) = self.playlist.as_mut() else {
            return false;
        };
        if index >= playlist.entries.len() {
            return false;
        }
        let path = playlist.entries[index].file_path.clone();
        playlist.set_current_with_history(index, push_history);
        let loaded = if let Some(preloaded) = self.take_ready_playlist_preload(index, &path) {
            self.commit_decoded_audio_load(preloaded, autoplay, true)
        } else {
            self.clear_playlist_preload();
            self.load_playlist_entry_path(path, autoplay)
        };
        if loaded {
            // Playlist repeat mode is authoritative for track looping when a track
            // is selected from a playlist (.fplist load, next/prev, double-click).
            self.sync_loop_button_from_playlist_repeat_mode(false);
        }
        loaded
    }

    fn playlist_has_prev_track(&mut self) -> bool {
        let Some(mut preview) = self.playlist.clone() else {
            return false;
        };
        preview
            .prev_track_for_mode_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
            .is_some()
    }

    fn playlist_has_next_track(&mut self) -> bool {
        let Some(mut preview) = self.playlist.clone() else {
            return false;
        };
        preview
            .next_track_for_mode_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
            .is_some()
    }

    fn log_playlist_skip_reason(&mut self) {
        let Some(playlist) = self.playlist.as_mut() else {
            self.set_status("No valid playlist entries remain");
            return;
        };
        let mut first_missing = None;
        let mut first_unsupported = None;
        for entry in &mut playlist.entries {
            entry.refresh_status();
            if !entry.file_exists && first_missing.is_none() {
                first_missing = Some(entry.display_name.clone());
            } else if entry.file_exists
                && !Self::is_supported_playlist_entry_path(&entry.file_path)
                && first_unsupported.is_none()
            {
                first_unsupported = Some(entry.display_name.clone());
            }
        }

        if let Some(name) = first_missing {
            self.set_status(format!("Skipped missing file: {name}"));
        } else if let Some(name) = first_unsupported {
            self.set_status(format!("Skipped unsupported file: {name}"));
        } else {
            self.set_status("No valid playlist entries remain");
        }
    }

    pub fn save_project(&mut self) {
        let trace = self.begin_user_trace("file.save");
        let path = self
            .current_path
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| self.can_save_project_to_existing_path(path));

        if let Some(path) = path {
            self.save_project_to_path(path);
        } else {
            self.save_project_as();
        }
        self.finish_user_trace(trace, "file.save", "completed");
    }

    fn load_playlist_entry_path(&mut self, path: PathBuf, autoplay: bool) -> bool {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());

        match extension.as_deref() {
            Some(ext) => {
                let registry = builtin_registry();
                let Some(descriptor) = registry.find_by_extension(ext).copied() else {
                    self.set_status("Skipped unsupported file: unsupported playlist entry type");
                    return false;
                };
                match descriptor.content_kind {
                    ContentKind::Fmid => {
                        self.load_fmid_path(path);
                        if autoplay {
                            self.play();
                        }
                    }
                    ContentKind::Midi => {
                        self.load_midi_path(path);
                        if autoplay {
                            self.play();
                        }
                    }
                    _ if descriptor.mastering == MasteringCapability::DecodedAudioPeq => {
                        self.load_decoded_audio_path_with_resume(path, descriptor, autoplay, true);
                    }
                    _ => {
                        self.set_status(
                            "Skipped unsupported file: unsupported playlist entry type",
                        );
                        return false;
                    }
                }
                true
            }
            _ => {
                self.set_status("Skipped unsupported file: unsupported playlist entry type");
                false
            }
        }
    }

    fn is_supported_playlist_entry_path(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| builtin_registry().find_by_extension(ext).is_some())
    }

    pub fn new_playlist(&mut self) {
        self.clear_playlist_preload();
        let mut playlist = PlaylistState::default();
        playlist.shuffle_seed = 0xC0FFEE_u64;
        self.playlist = Some(playlist);
        self.playlist_selected_indices.clear();
        self.playlist_drag_from = None;
        self.set_playlist_window_open(true);
        self.set_status("New playlist");
    }

    pub fn load_fplist_path(&mut self, path: PathBuf) {
        self.clear_playlist_preload();
        match playlist_persistence::load_playlist(&path) {
            Ok(mut playlist) => {
                if playlist.entries.is_empty() {
                    self.playlist = Some(playlist);
                    self.set_playlist_window_open(true);
                    self.sync_loop_button_from_playlist_repeat_mode(false);
                    self.set_status("Loaded empty playlist");
                    return;
                }

                let first_valid = playlist.first_valid_track_by(|entry| {
                    Self::is_supported_playlist_entry_path(&entry.file_path)
                });
                self.playlist = Some(playlist);
                self.set_playlist_window_open(true);
                self.sync_loop_button_from_playlist_repeat_mode(false);
                if let Some(index) = first_valid {
                    let loaded = self.load_playlist_track_by_index(index, false, true);
                    if loaded {
                        self.set_status(format!("Loaded playlist {}", path.display()));
                    } else {
                        self.set_status("Loaded playlist, but first valid track failed to load");
                    }
                } else {
                    self.set_status("Loaded playlist, but no valid tracks were found");
                }
            }
            Err(error) => self.set_status(format!("{error}")),
        }
    }

    pub fn save_playlist(&mut self) {
        let Some(playlist) = self.playlist.as_mut() else {
            self.set_status("No playlist loaded");
            return;
        };
        let Some(path) = playlist.file_path.clone() else {
            self.save_playlist_as();
            return;
        };

        if let Some(parent) = path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                self.set_status(format!("failed to create {}: {error}", parent.display()));
                return;
            }
        }

        match playlist_persistence::save_playlist(&path, playlist) {
            Ok(()) => {
                playlist.dirty = false;
                self.set_status(format!("Saved playlist {}", path.display()));
            }
            Err(error) => self.set_status(format!("{error}")),
        }
    }

    pub fn save_playlist_as(&mut self) {
        if self.playlist.is_none() {
            self.new_playlist();
        }

        let Some(path) = rfd::FileDialog::new()
            .add_filter("flutzPlayer Playlist", &["fplist"])
            .set_file_name("playlist.fplist")
            .save_file()
        else {
            self.set_status("Playlist Save As canceled");
            return;
        };

        if let Some(playlist) = self.playlist.as_mut() {
            playlist.file_path = Some(path);
        }
        self.save_playlist();
    }

    fn remove_selected_playlist_entries(&mut self) {
        self.clear_playlist_preload();
        let Some(playlist) = self.playlist.as_mut() else {
            return;
        };
        if self.playlist_selected_indices.is_empty() {
            return;
        }

        let indices = self
            .playlist_selected_indices
            .iter()
            .copied()
            .collect::<Vec<_>>();
        playlist.remove_indices(&indices);
        self.playlist_selected_indices.clear();
    }

    fn process_playlist_file_picker_pending(&mut self) {
        if !self.playlist_file_picker_pending {
            return;
        }
        self.playlist_file_picker_pending = false;

        let Some(paths) = rfd::FileDialog::new()
            .add_filter("Playlist Tracks", &Self::playlist_dialog_extensions())
            .pick_files()
        else {
            return;
        };

        if self.playlist.is_none() {
            self.new_playlist();
        }
        self.clear_playlist_preload();
        if let Some(playlist) = self.playlist.as_mut() {
            playlist.add_entries(paths);
        }
    }

    fn handle_dropped_playlist_files(&mut self, context: &egui::Context) {
        let dropped_paths = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect::<Vec<_>>()
        });

        if dropped_paths.is_empty() {
            return;
        }

        let filtered = dropped_paths
            .into_iter()
            .filter(|path| Self::is_supported_playlist_entry_path(path))
            .collect::<Vec<_>>();
        if filtered.is_empty() {
            return;
        }

        if self.playlist.is_none() {
            self.new_playlist();
        }
        self.clear_playlist_preload();
        if let Some(playlist) = self.playlist.as_mut() {
            playlist.add_entries(filtered);
            self.set_playlist_window_open(true);
        }
    }

    fn handle_playback_end_transition(&mut self) {
        let currently_active = self.playback.playback_active();
        if self.last_playback_active
            && !currently_active
            && self.run_state == AppRunState::Playing
            && !self.loop_enabled
        {
            if !self.on_track_end() {
                self.run_state = AppRunState::Idle;
                self.set_status("Stopped");
            }
        }
        self.last_playback_active = currently_active;
    }

    fn process_pending_playlist_preload(&mut self) {
        let Some(pending) = &self.pending_playlist_preload else {
            return;
        };
        let token = pending.token;
        let index = pending.index;
        let path = pending.path.clone();
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_playlist_preload = None;
                return;
            }
        };
        self.pending_playlist_preload = None;
        if token != PLAYLIST_PRELOAD_TOKEN.load(Ordering::Relaxed) || result.error.is_some() {
            return;
        }
        self.ready_playlist_preload = Some(ReadyPlaylistPreload {
            token,
            index,
            path,
            result,
        });
    }

    fn maybe_start_playlist_preload(&mut self) {
        if self.run_state != AppRunState::Playing || !self.playback.playback_active() {
            return;
        }
        if self.loop_enabled
            || self
                .playlist
                .as_ref()
                .is_some_and(|playlist| playlist.repeat_mode == PlaylistRepeatMode::Track)
        {
            self.clear_playlist_preload();
            return;
        }
        let duration = self.transport_duration_seconds();
        let position = self.transport_seconds();
        if duration <= 0.0 || duration - position > PLAYLIST_PRELOAD_LEAD_TIME_SECONDS {
            return;
        }
        let Some((index, path, descriptor)) = self.next_playlist_preload_candidate() else {
            self.clear_playlist_preload();
            return;
        };
        if self
            .ready_playlist_preload
            .as_ref()
            .is_some_and(|preload| preload.index == index && preload.path == path)
            || self
                .pending_playlist_preload
                .as_ref()
                .is_some_and(|preload| preload.index == index && preload.path == path)
        {
            return;
        }
        if descriptor.mastering != MasteringCapability::DecodedAudioPeq {
            self.clear_playlist_preload();
            return;
        }
        let token = PLAYLIST_PRELOAD_TOKEN.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = mpsc::channel();
        let path_for_worker = path.clone();
        let spawn_result = thread::Builder::new()
            .name("flutz-playlist-preload".to_owned())
            .spawn(move || {
                let result = prepare_decoded_audio_load_job(token, path_for_worker, descriptor);
                let _ = sender.send(result);
            });
        if spawn_result.is_ok() {
            self.pending_playlist_preload = Some(PendingPlaylistPreload {
                token,
                index,
                path,
                receiver,
            });
        }
    }

    fn next_playlist_preload_candidate(&self) -> Option<(usize, PathBuf, FormatDescriptor)> {
        let mut preview = self.playlist.clone()?;
        let index = preview.next_track_for_mode_by(|entry| {
            Self::is_supported_playlist_entry_path(&entry.file_path)
        })?;
        let path = preview.entries.get(index)?.file_path.clone();
        let extension = path.extension().and_then(|ext| ext.to_str())?;
        let descriptor = builtin_registry().find_by_extension(extension).copied()?;
        Some((index, path, descriptor))
    }

    fn clear_playlist_preload(&mut self) {
        self.pending_playlist_preload = None;
        self.ready_playlist_preload = None;
        PLAYLIST_PRELOAD_TOKEN.fetch_add(1, Ordering::Relaxed);
    }

    fn take_ready_playlist_preload(
        &mut self,
        index: usize,
        path: &Path,
    ) -> Option<DecodedAudioLoadJobResult> {
        let preload = self.ready_playlist_preload.take()?;
        if preload.index == index
            && preload.path == path
            && preload.token == PLAYLIST_PRELOAD_TOKEN.load(Ordering::Relaxed)
        {
            self.pending_playlist_preload = None;
            Some(preload.result)
        } else {
            self.ready_playlist_preload = Some(preload);
            None
        }
    }

    pub fn on_track_end(&mut self) -> bool {
        let repeat_track = self
            .playlist
            .as_ref()
            .is_some_and(|playlist| playlist.repeat_mode == PlaylistRepeatMode::Track);
        if repeat_track {
            self.play();
            return true;
        }

        if self.navigate_playlist(true, true) {
            return true;
        }

        let wrap = self.playlist.as_ref().is_some_and(|playlist| {
            playlist.loop_enabled || playlist.repeat_mode == PlaylistRepeatMode::Playlist
        });
        if !wrap {
            return false;
        }

        let first_index = self.playlist.as_mut().and_then(|playlist| {
            playlist.first_valid_track_by(|entry| {
                Self::is_supported_playlist_entry_path(&entry.file_path)
            })
        });
        if let Some(index) = first_index {
            return self.load_playlist_track_by_index(index, false, true);
        }

        false
    }

    pub fn save_project_as(&mut self) {
        let trace = self.begin_user_trace("file.save_as");
        let (filter_name, extensions, default_name) = self.save_as_dialog_spec();
        let Some(path) = rfd::FileDialog::new()
            .add_filter(&filter_name, &extensions)
            .set_file_name(&default_name)
            .save_file()
        else {
            self.set_status("Save As canceled");
            self.finish_user_trace(trace, "file.save_as", "canceled");
            return;
        };
        self.save_project_to_path(path);
        self.finish_user_trace(trace, "file.save_as", "completed");
    }

    pub fn load_midi_path(&mut self, path: PathBuf) {
        let trace = self.begin_user_trace("file.load_midi");
        let path_display = path.display().to_string();
        self.log_midi_mapping_scan_start("file.load_midi", &path_display);
        let preset = self
            .preset_set
            .find_preset(&self.default_preset_id)
            .unwrap_or_else(|| self.preset_set.default_preset());
        let selected_soundfonts = preset
            .font_ids
            .iter()
            .map(|font_id| (*font_id).to_owned())
            .collect::<Vec<_>>();
        match self.playback.load_midi_file(&path, &selected_soundfonts) {
            Ok(message) => {
                self.project_title = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| "Loaded MIDI".to_owned());
                self.loaded_content = Some(LoadedContentState {
                    kind: ContentKind::Midi,
                    format_id: "midi".to_owned(),
                    friendly_name: "MIDI sequence".to_owned(),
                    mastering: MasteringCapability::MidiMastering,
                    wrapped_extension: Some("fmid".to_owned()),
                    decoded_wrapper: None,
                    decoded_source_path: None,
                    decoded_source_bytes: None,
                });
                self.current_path = Some(path.display().to_string());
                self.transport_position = 0.0;
                self.run_state = AppRunState::Idle;
                self.active_preset_id = preset.id.to_owned();
                self.selected_preset_id = preset.id.to_owned();
                self.soundfonts =
                    soundfont_rows_from_catalog(&self.catalog_soundfonts, preset.font_ids);
                self.apply_metadata_for_plain_midi(&path);
                self.reset_plain_midi_defaults();
                let transport_metadata = self.playback.midi_transport_metadata();
                let has_non_default_loop =
                    Self::midi_has_non_default_loop_config(&transport_metadata);
                self.apply_midi_transport_metadata(transport_metadata);
                self.apply_loop_defaults_after_load(has_non_default_loop);
                self.sync_coverage_cache_from_playback();
                self.reset_strip_layout();
                self.sync_playback_controls();
                self.sync_playback_loop_settings();
                self.log_midi_mapping_scan_result("file.load_midi");
                self.peq_edit_state = PeqEditState::default();
                self.set_status(message);
                self.finish_user_trace(trace, "file.load_midi", "ok");
            }
            Err(error) => {
                self.set_status(format!("{error}"));
                self.finish_user_trace(trace, "file.load_midi", "error");
            }
        }
    }

    pub fn load_fmid_path(&mut self, path: PathBuf) {
        let trace = self.begin_user_trace("file.load_fmid");
        let path_display = path.display().to_string();
        self.log_midi_mapping_scan_start("file.load_fmid", &path_display);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.set_status(format!("failed to read {}: {error}", path.display()));
                self.finish_user_trace(trace, "file.load_fmid", "read_error");
                return;
            }
        };

        let fmid = match read_fmid(&bytes) {
            Ok(file) => file,
            Err(error) => {
                self.set_status(format!("{error}"));
                self.finish_user_trace(trace, "file.load_fmid", "parse_error");
                return;
            }
        };

        let (requested_soundfonts, preset_mode_message) = match &fmid.mixer_source_mode {
            MixerSourceMode::Custom => (
                fmid.soundfonts
                    .iter()
                    .map(|slot| slot.internal_id.clone())
                    .collect::<Vec<_>>(),
                None,
            ),
            MixerSourceMode::PresetDefault(preset_id) => {
                let preset = self
                    .preset_set
                    .find_preset(preset_id)
                    .unwrap_or_else(|| self.preset_set.default_preset());
                let message = if preset.id == preset_id {
                    None
                } else {
                    Some(format!(
                        "Preset {preset_id} was unavailable; loaded default preset {}",
                        preset.display_name
                    ))
                };
                self.missing_preset_warning = message.clone();
                (
                    preset
                        .font_ids
                        .iter()
                        .map(|font_id| (*font_id).to_owned())
                        .collect::<Vec<_>>(),
                    message,
                )
            }
        };
        self.soundfonts = self.soundfont_rows_for_ids(&requested_soundfonts);

        match self.playback.load_midi_bytes(
            fmid.midi_bytes.clone(),
            fmid.project.source_midi_filename.clone(),
            &requested_soundfonts,
        ) {
            Ok(message) => {
                self.project_title = if fmid.project.project_name.is_empty() {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| "Loaded FMID".to_owned())
                } else {
                    fmid.project.project_name.clone()
                };
                self.loaded_content = Some(LoadedContentState {
                    kind: ContentKind::Fmid,
                    format_id: "fmid".to_owned(),
                    friendly_name: "Flutz MIDI project".to_owned(),
                    mastering: MasteringCapability::MidiMastering,
                    wrapped_extension: Some("fmid".to_owned()),
                    decoded_wrapper: None,
                    decoded_source_path: None,
                    decoded_source_bytes: None,
                });
                self.current_path = Some(path.display().to_string());
                self.transport_position = 0.0;
                self.run_state = AppRunState::Idle;

                self.apply_fmid_global_settings(&fmid);
                self.apply_metadata_for_fmid(&fmid, &path);
                let has_non_default_loop = Self::fmid_has_non_default_loop_config(&fmid);
                self.apply_loop_defaults_after_load(has_non_default_loop);
                self.sync_coverage_cache_from_playback();
                self.reset_strip_layout();
                self.apply_fmid_mixer_source_mode(&fmid);
                self.sync_playback_controls();
                self.sync_playback_loop_settings();

                let loaded_soundfonts = self.playback.loaded_soundfont_ids();
                if !loaded_soundfonts.is_empty() {
                    self.soundfonts = self.soundfont_rows_for_ids(&loaded_soundfonts);
                    self.sync_coverage_cache_from_playback();
                    self.reset_strip_layout();
                    self.apply_fmid_mixer_source_mode(&fmid);
                    self.sync_playback_controls();
                    self.sync_playback_loop_settings();
                }

                self.dirty = false;
                self.log_midi_mapping_scan_result("file.load_fmid");
                self.peq_edit_state = PeqEditState::default();
                self.set_status(
                    preset_mode_message.unwrap_or_else(|| format!("Loaded FMID: {message}")),
                );
                self.finish_user_trace(trace, "file.load_fmid", "ok");
            }
            Err(error) => {
                self.set_status(format!("{error}"));
                self.finish_user_trace(trace, "file.load_fmid", "error");
            }
        }
    }

    pub fn load_decoded_audio_path(&mut self, path: PathBuf, descriptor: FormatDescriptor) {
        self.load_decoded_audio_path_with_resume(path, descriptor, false, false);
    }

    fn load_decoded_audio_path_with_resume(
        &mut self,
        path: PathBuf,
        descriptor: FormatDescriptor,
        resume_after_load: bool,
        playlist_entry_load: bool,
    ) {
        let trace = self.begin_user_trace("file.load_decoded_audio");
        let token = DECODED_AUDIO_LOAD_TOKEN.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = mpsc::channel();
        self.pending_decoded_audio_load = Some(PendingDecodedAudioLoad {
            token,
            receiver,
            resume_after_load,
            playlist_entry_load,
        });
        let path_for_status = path.display().to_string();
        let spawn_result = thread::Builder::new()
            .name("flutz-decoded-load".to_owned())
            .spawn(move || {
                let result = prepare_decoded_audio_load_job(token, path, descriptor);
                let _ = sender.send(result);
            });
        if let Err(error) = spawn_result {
            self.pending_decoded_audio_load = None;
            self.set_status(format!("failed to start decoded audio load: {error}"));
            self.finish_user_trace(trace, "file.load_decoded_audio", "spawn_error");
            return;
        }
        self.set_status(format!("Loading decoded audio {}", path_for_status));
        self.finish_user_trace(trace, "file.load_decoded_audio", "scheduled");
    }

    fn process_pending_decoded_audio_load(&mut self) {
        let Some(pending) = &self.pending_decoded_audio_load else {
            return;
        };
        let token = pending.token;
        let resume_after_load = pending.resume_after_load;
        let playlist_entry_load = pending.playlist_entry_load;
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_decoded_audio_load = None;
                self.set_status("decoded audio load worker exited before reporting a result");
                return;
            }
        };
        self.pending_decoded_audio_load = None;
        if result.token != token || result.token != DECODED_AUDIO_LOAD_TOKEN.load(Ordering::Relaxed)
        {
            return;
        }
        let _ = self.commit_decoded_audio_load(result, resume_after_load, playlist_entry_load);
    }

    fn commit_decoded_audio_load(
        &mut self,
        result: DecodedAudioLoadJobResult,
        resume_after_load: bool,
        playlist_entry_load: bool,
    ) -> bool {
        let DecodedAudioLoadJobResult {
            path,
            path_display,
            descriptor,
            is_wrapped,
            wrapper,
            stream_source,
            source_bytes,
            error,
            error_stage,
            ..
        } = result;
        if let Some(error) = error {
            self.set_status(format!("decoded audio {error_stage} failed: {error}"));
            return false;
        }
        let Some(wrapper) = wrapper else {
            self.set_status("decoded audio load did not produce wrapper metadata");
            return false;
        };
        let Some(stream_source) = stream_source else {
            self.set_status("decoded audio load did not produce a stream source");
            return false;
        };

        match self.playback.load_decoded_audio_stream(
            path.clone(),
            descriptor.id,
            descriptor.friendly_name,
            stream_source,
            wrapper.peq.clone(),
        ) {
            Ok(message) => {
                self.loaded_content = Some(LoadedContentState {
                    kind: if is_wrapped {
                        ContentKind::DecodedAudioWrapper
                    } else {
                        ContentKind::DecodedAudio
                    },
                    format_id: descriptor.id.to_owned(),
                    friendly_name: descriptor.friendly_name.to_owned(),
                    mastering: descriptor.mastering,
                    wrapped_extension: descriptor
                        .wrapped_extensions
                        .first()
                        .map(|extension| (*extension).to_owned()),
                    decoded_wrapper: Some(wrapper.clone()),
                    decoded_source_path: (!is_wrapped).then(|| path.clone()),
                    decoded_source_bytes: source_bytes,
                });
                self.project_title = if wrapper.metadata.project_name.trim().is_empty() {
                    path.file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Loaded Audio")
                        .to_owned()
                } else {
                    wrapper.metadata.project_name.clone()
                };
                self.current_path = Some(path_display);
                self.transport_position = 0.0;
                self.run_state = AppRunState::Idle;
                self.apply_metadata_for_decoded_audio(&wrapper, &path);
                self.apply_decoded_audio_defaults(wrapper.loop_region);
                self.reset_decoded_peq_edit_state(wrapper.peq.clone());
                self.sync_playback_loop_settings();
                if playlist_entry_load {
                    self.sync_loop_button_from_playlist_repeat_mode(false);
                }
                self.reset_strip_layout();
                self.sync_playback_controls();
                self.dirty = !is_wrapped;
                self.set_status(message);
                if resume_after_load {
                    self.play();
                }
                true
            }
            Err(error) => {
                self.set_status(format!("{error}"));
                false
            }
        }
    }

    pub fn play(&mut self) {
        let trace = self.begin_user_trace("transport.play");
        let mut outcome = "error";
        match self.playback.play() {
            Ok(AudioPlaybackStatus::Audible) => {
                self.run_state = AppRunState::Playing;
                self.set_status("Playing");
                outcome = "audible";
            }
            Ok(AudioPlaybackStatus::AudioUnavailable(message)) => {
                self.run_state = AppRunState::Paused;
                self.set_status(format!(
                    "{message}. Connect an output device and use Retry Audio."
                ));
                outcome = "audio_unavailable";
            }
            Err(error) => self.set_status(format!("{error}")),
        }
        self.finish_user_trace(trace, "transport.play", outcome);
    }

    pub fn retry_audio(&mut self) {
        let trace = self.begin_user_trace("transport.retry_audio");
        let mut outcome = "error";
        match self.playback.retry_audio() {
            Ok(()) => {
                self.set_status("Audio output ready");
                outcome = "ok";
            }
            Err(error) => self.set_status(format!(
                "Audio still unavailable: {error}. Connect an output device and retry."
            )),
        }
        self.finish_user_trace(trace, "transport.retry_audio", outcome);
    }

    pub fn pause(&mut self) {
        let trace = self.begin_user_trace("transport.pause");
        let mut outcome = "error";
        match self.playback.pause() {
            Ok(()) => {
                self.run_state = AppRunState::Paused;
                self.set_status("Paused");
                outcome = "ok";
            }
            Err(error) => self.set_status(format!("{error}")),
        }
        self.finish_user_trace(trace, "transport.pause", outcome);
    }

    pub fn stop(&mut self) {
        let trace = self.begin_user_trace("transport.stop");
        let mut outcome = "error";
        match self.playback.stop() {
            Ok(()) => {
                self.run_state = AppRunState::Idle;
                self.transport_position = 0.0;
                self.set_status("Stopped");
                outcome = "ok";
            }
            Err(error) => self.set_status(format!("{error}")),
        }
        self.finish_user_trace(trace, "transport.stop", outcome);
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub fn data_summary(&self) -> &str {
        &self.data_summary
    }

    pub fn playback_summary(&self) -> String {
        self.playback.status_line()
    }

    pub fn audio_status(&self) -> String {
        self.playback.audio_status_text()
    }

    pub fn audio_backend_label(&self) -> &'static str {
        match self.debug_metrics().audio_backend {
            "wasapi" => "WASAPI",
            _ => "SDL3",
        }
    }

    pub fn measured_output_latency_ms(&self) -> f64 {
        self.playback.estimated_output_latency_seconds() * 1000.0
    }

    pub fn transport_seconds(&self) -> f64 {
        self.debug_metrics().transport_seconds
    }

    pub fn transport_duration_seconds(&self) -> f64 {
        self.debug_metrics().transport_duration_seconds
    }

    pub fn transport_tick(&self) -> u64 {
        self.playback.transport_tick()
    }

    pub fn transport_tick_length(&self) -> u64 {
        self.playback
            .decoded_transport_metadata()
            .map(|metadata| metadata.frame_length)
            .unwrap_or_else(|| self.playback.midi_transport_metadata().tick_length)
    }

    pub fn debug_metrics(&self) -> PlaybackDebugMetrics {
        self.playback.debug_metrics()
    }

    pub fn visualizer_frame(&self) -> VisualizerFrame {
        self.playback.visualizer_frame()
    }

    pub fn perf_trace_status(&self) -> String {
        self.perf_trace.status_line()
    }

    pub fn perf_trace_records(&self) -> Vec<PerfTraceRecord> {
        self.perf_trace.records()
    }

    pub fn perf_trace_issues(&self) -> Vec<PerfTraceRecord> {
        self.perf_trace.issues()
    }

    pub fn start_perf_trace_logging(&mut self) {
        match self.perf_trace.start_logging_default() {
            Ok(path) => self.set_status(format!("Performance trace logging to {}", path.display())),
            Err(error) => self.set_status(error),
        }
    }

    pub fn stop_perf_trace_logging(&mut self) {
        self.perf_trace.stop_logging();
        self.set_status("Performance trace logging stopped");
    }

    pub fn export_perf_trace_log(&mut self) {
        match self.perf_trace.export_jsonl_default() {
            Ok(path) => {
                self.set_status(format!("Performance trace exported to {}", path.display()))
            }
            Err(error) => self.set_status(error),
        }
    }

    pub fn clear_perf_trace(&mut self) {
        self.perf_trace.clear();
        self.set_status("Performance trace cleared");
    }

    pub fn catalog_soundfonts(&self) -> &[SoundFontCatalogEntry] {
        &self.catalog_soundfonts
    }

    pub fn loaded_preset_font_order(&self) -> String {
        self.soundfonts
            .iter()
            .map(|font| font.display_name.clone())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    pub fn preset_set(&self) -> &'static PresetSet {
        self.preset_set
    }

    pub fn active_preset(&self) -> &'static Preset {
        self.preset_set
            .find_preset(&self.active_preset_id)
            .unwrap_or_else(|| self.preset_set.default_preset())
    }

    pub fn active_preset_label(&self) -> String {
        self.preset_set
            .find_preset(&self.active_preset_id)
            .map(|preset| preset.display_name.to_owned())
            .unwrap_or_else(|| "Custom Mode".to_owned())
    }

    pub fn selected_preset_id(&self) -> &str {
        &self.selected_preset_id
    }

    pub fn set_selected_preset_id(&mut self, preset_id: impl Into<String>) {
        let preset_id = preset_id.into();
        if self.preset_set.find_preset(&preset_id).is_some() {
            self.selected_preset_id = preset_id;
        }
    }

    pub fn apply_selected_preset(&mut self) {
        let trace = self.begin_user_trace("preset.apply");
        if self.selected_preset_id == self.active_preset_id {
            self.set_status("Preset already active");
            self.finish_user_trace(trace, "preset.apply", "already_active");
            return;
        }

        let Some(preset) = self.preset_set.find_preset(&self.selected_preset_id) else {
            let fallback = self.preset_set.default_preset();
            self.selected_preset_id = fallback.id.to_owned();
            self.set_status(format!(
                "Preset unavailable; selected fallback preset {}",
                fallback.display_name
            ));
            self.finish_user_trace(trace, "preset.apply", "fallback_selected");
            return;
        };

        self.apply_preset(preset, true);
        self.finish_user_trace(trace, "preset.apply", "completed");
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn transport_position(&mut self) -> &mut f32 {
        &mut self.transport_position
    }

    pub fn seek_transport_fraction(&mut self, fraction: f32) {
        let trace = self.begin_user_trace("transport.seek");
        let clamped = fraction.clamp(0.0, 1.0);
        self.transport_position = clamped;
        let mut outcome = "ok";
        if let Err(error) = self.playback.seek_transport_fraction(clamped) {
            self.set_status(format!("{error}"));
            outcome = "error";
        } else {
            let target_tick = (self.transport_tick_length() as f32 * clamped).round() as u64;
            self.handle_counted_loop_seek(target_tick);
        }
        self.finish_user_trace(trace, "transport.seek", outcome);
    }

    pub fn seek_transport_tick(&mut self, tick: u64) {
        let trace = self.begin_user_trace("transport.seek_tick");
        let clamped = tick.min(self.transport_tick_length());
        let mut outcome = "ok";
        if let Err(error) = self.playback.seek_transport_tick(clamped) {
            self.set_status(format!("{error}"));
            outcome = "error";
        } else {
            self.handle_counted_loop_seek(clamped);
        }
        self.playback.update_latency_control();
        self.transport_position = self.playback.transport_fraction();
        self.finish_user_trace(trace, "transport.seek_tick", outcome);
    }

    pub fn loop_enabled_value(&self) -> bool {
        self.loop_enabled
    }

    pub fn set_loop_enabled(&mut self, enabled: bool) {
        let trace = self.begin_user_trace("transport.loop_enabled");
        if let Some(playlist) = self.playlist.as_mut() {
            let desired_repeat_mode = if enabled {
                PlaylistRepeatMode::Track
            } else {
                PlaylistRepeatMode::Off
            };

            if playlist.repeat_mode != desired_repeat_mode {
                playlist.set_repeat_mode(desired_repeat_mode);
                if desired_repeat_mode != PlaylistRepeatMode::Playlist {
                    playlist.loop_enabled = false;
                }
                playlist.dirty = true;
            }
        }

        let previous = self.loop_state_tuple();
        self.loop_enabled = enabled;
        if enabled && matches!(self.loop_mode, LoopMode::None) {
            self.loop_mode = LoopMode::Infinite;
        }
        if enabled && self.loop_end_tick == 0 {
            self.loop_end_tick = self.transport_tick_length();
        }
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_enabled", outcome);
    }

    pub fn loop_mode_value(&self) -> LoopMode {
        self.loop_mode
    }

    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        let trace = self.begin_user_trace("transport.loop_mode");
        let previous = self.loop_state_tuple();
        self.loop_mode = mode;
        self.loop_enabled = !matches!(mode, LoopMode::None);
        if self.loop_enabled && self.loop_end_tick == 0 {
            self.loop_end_tick = self.transport_tick_length();
        }
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_mode", outcome);
    }

    pub fn loop_start_tick_value(&self) -> u64 {
        self.loop_start_tick
    }

    pub fn set_loop_start_tick(&mut self, tick: u64) {
        let trace = self.begin_user_trace("transport.loop_start_tick");
        let previous = self.loop_state_tuple();
        self.loop_start_tick = tick;
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_start_tick", outcome);
    }

    pub fn loop_end_tick_value(&self) -> u64 {
        self.loop_end_tick
    }

    pub fn set_loop_end_tick(&mut self, tick: u64) {
        let trace = self.begin_user_trace("transport.loop_end_tick");
        let previous = self.loop_state_tuple();
        self.loop_end_tick = tick;
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_end_tick", outcome);
    }

    pub fn loop_count_value(&self) -> u32 {
        self.loop_count
    }

    pub fn set_loop_count(&mut self, count: u32) {
        let trace = self.begin_user_trace("transport.loop_count");
        let previous = self.loop_state_tuple();
        self.loop_count = count.max(1);
        self.normalize_loop_state(true);
        let outcome = self.commit_loop_state_change(previous);
        self.finish_user_trace(trace, "transport.loop_count", outcome);
    }

    pub fn master(&mut self) -> &mut MasterControls {
        &mut self.master
    }

    pub fn smart_mix(&mut self) -> &mut SmartMixControls {
        &mut self.smart_mix
    }

    pub fn soundfont_rows(&self) -> &[SoundFontUiRow] {
        &self.soundfonts
    }

    pub fn soundfonts(&mut self) -> &mut Vec<SoundFontUiRow> {
        &mut self.soundfonts
    }

    pub fn mixer_fx_expanded(&self) -> bool {
        self.mixer_fx_expanded
    }

    pub fn set_mixer_fx_expanded(&mut self, expanded: bool) {
        self.mixer_fx_expanded = expanded;
    }

    pub fn selected_soundfont(&mut self) -> &mut usize {
        &mut self.selected_soundfont
    }

    pub fn selected_soundfont_index(&self) -> usize {
        self.selected_soundfont
    }

    pub fn set_selected_soundfont_index(&mut self, index: usize) {
        self.selected_soundfont = index.min(self.catalog_soundfonts.len().saturating_sub(1));
    }

    pub fn mixer_assignment_mode(&self) -> MixerAssignmentMode {
        self.mixer_assignment_mode
    }

    pub fn all_mixer_rows_collapsed(&self) -> bool {
        !self.soundfonts.is_empty() && self.soundfonts.iter().all(|font| font.collapsed)
    }

    pub fn toggle_all_mixer_rows_collapsed(&mut self) {
        let collapsed = !self.all_mixer_rows_collapsed();
        for font in &mut self.soundfonts {
            font.collapsed = collapsed;
        }
    }

    pub fn apply_balanced_mixer_assignment(&mut self) {
        self.apply_mixer_assignment_mode(MixerAssignmentMode::Balance);
    }

    pub fn apply_layered_mixer_assignment(&mut self) {
        self.apply_mixer_assignment_mode(MixerAssignmentMode::Layer);
    }

    pub fn add_selected_soundfont(&mut self) {
        let trace = self.begin_user_trace("soundfont.add");
        let Some(selected) = self
            .catalog_soundfonts
            .get(self.selected_soundfont)
            .map(|entry| entry.internal_id.clone())
        else {
            self.set_status("No catalog soundfont selected");
            self.finish_user_trace(trace, "soundfont.add", "none_selected");
            return;
        };

        if self
            .soundfonts
            .iter()
            .any(|font| font.internal_id == selected)
        {
            self.set_status("Soundfont is already loaded");
            self.finish_user_trace(trace, "soundfont.add", "already_loaded");
            return;
        }

        let mut requested = self
            .soundfonts
            .iter()
            .map(|font| font.internal_id.clone())
            .collect::<Vec<_>>();
        requested.push(selected);
        self.convert_to_custom_mode();
        self.apply_soundfont_set_change(requested, "Added soundfont");
        self.finish_user_trace(trace, "soundfont.add", "completed");
    }

    pub fn remove_selected_soundfont(&mut self) {
        let trace = self.begin_user_trace("soundfont.remove");
        let Some(selected) = self
            .catalog_soundfonts
            .get(self.selected_soundfont)
            .map(|entry| entry.internal_id.clone())
        else {
            self.set_status("No catalog soundfont selected");
            self.finish_user_trace(trace, "soundfont.remove", "none_selected");
            return;
        };

        if !self
            .soundfonts
            .iter()
            .any(|font| font.internal_id == selected)
        {
            self.set_status("Selected catalog soundfont is not loaded");
            self.finish_user_trace(trace, "soundfont.remove", "not_loaded");
            return;
        }

        let requested = self
            .soundfonts
            .iter()
            .map(|font| font.internal_id.clone())
            .filter(|id| id != &selected)
            .collect::<Vec<_>>();

        if requested.is_empty() {
            self.set_status("Cannot remove the last loaded soundfont");
            self.finish_user_trace(trace, "soundfont.remove", "last_soundfont");
            return;
        }

        self.convert_to_custom_mode();
        self.apply_soundfont_set_change(requested, "Removed soundfont");
        self.finish_user_trace(trace, "soundfont.remove", "completed");
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn missing_preset_warning(&self) -> Option<&str> {
        self.missing_preset_warning.as_deref()
    }

    pub fn dismiss_missing_preset_warning(&mut self) {
        self.missing_preset_warning = None;
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub fn mark_project_dirty(&mut self) {
        self.mark_dirty();
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
        let mut preset = self.peq_edit_state.preset.clone();
        preset.config = self.peq_edit_state.preset.config.clone();
        self.default_decoded_peq_preset = normalize_default_decoded_peq_preset(preset);
        self.persist_window_view_prefs();
        if self.peq_edit_state.source == PeqConfigSource::Default {
            self.peq_edit_state.dirty = false;
        }
        self.set_status("Saved decoded audio PEQ default to preferences.ini");
    }

    pub fn set_decoded_peq_bypass_enabled(&mut self, enabled: bool) {
        if self.decoded_peq_bypass_enabled == enabled {
            return;
        }
        self.decoded_peq_bypass_enabled = enabled;
        self.persist_window_view_prefs();
        if self.active_mastering_capability() == MasteringCapability::DecodedAudioPeq {
            let config = self.peq_edit_state.preset.config.clone();
            if let Err(error) = self.apply_decoded_peq_runtime_config(&config) {
                self.set_status(format!("{error}"));
                return;
            }
        }
        self.set_status(if enabled {
            "Bypass EQ enabled"
        } else {
            "Bypass EQ disabled"
        });
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

    fn save_decoded_peq_preset_to_path(&mut self, path: PathBuf) {
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

    fn save_fmid_to_path(&mut self, path: PathBuf) {
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

    fn save_project_to_path(&mut self, path: PathBuf) {
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

    fn validate_decoded_wrapper_save_path(&self, path: &Path) -> Result<()> {
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

    fn can_save_project_to_existing_path(&self, path: &Path) -> bool {
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

    fn save_decoded_wrapper_to_path(&mut self, path: PathBuf) {
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

    fn build_decoded_wrapper_file(&self) -> Result<FlutzAudioWrapper> {
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
            ..self.metadata_edit_state.to_track_metadata(
                self.project_title(),
                &wrapper.source.original_filename,
            )
        };
        wrapper.native_metadata = normalized_metadata_fields(&self.metadata_edit_state.native_metadata);
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

    fn build_fmid_file(&self) -> Result<FmidFile> {
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

    fn reset_strip_layout(&mut self) {
        let layout = self.playback.midi_strip_layout();
        self.logged_unmatched_snapshot_strips.clear();
        let active_preset = self.preset_set.find_preset(&self.active_preset_id);
        let font_ids = self
            .soundfonts
            .iter()
            .map(|font| font.internal_id.clone())
            .collect::<Vec<_>>();
        for (font_index, font) in self.soundfonts.iter_mut().enumerate() {
            font.muted = false;
            font.soloed = false;
            font.strips = layout
                .iter()
                .map(|&(channel, bank, program, is_percussion)| {
                    let routing_decision = active_preset.and_then(|preset| {
                        compute_strip_routing(
                            preset,
                            &font_ids,
                            &self.coverage_cache,
                            program,
                            bank,
                            is_percussion,
                        )
                        .into_iter()
                        .find(|decision| decision.font_index == font_index)
                    });
                    let strip_id = strip_id_for(font_index, channel, bank, program, is_percussion);
                    MixerStripUiState {
                        strip_id,
                        channel,
                        bank,
                        program,
                        is_percussion,
                        program_name: if is_percussion {
                            "Percussion".to_owned()
                        } else {
                            format!("Program {}", program)
                        },
                        volume: routing_decision
                            .map(|decision| decision.volume)
                            .unwrap_or(1.0),
                        muted: routing_decision
                            .map(|decision| decision.muted)
                            .unwrap_or(false),
                        unsupported: routing_decision
                            .map(|decision| decision.unsupported)
                            .unwrap_or(false),
                        mute_policy_default: routing_decision.is_some(),
                        volume_policy_default: routing_decision.is_some(),
                        soloed: false,
                        pan: 0.0,
                        gain_db: 0.0,
                        limiter_enabled: false,
                        limiter_amount: 0.25,
                        reverb: 0.0,
                        chorus: 0.0,
                        meter: 0.0,
                        active: false,
                        current_note: "--".to_owned(),
                    }
                })
                .collect();
        }
    }

    fn apply_preset(&mut self, preset: &'static Preset, mark_dirty: bool) {
        let requested_soundfonts = preset
            .font_ids
            .iter()
            .map(|font_id| (*font_id).to_owned())
            .collect::<Vec<_>>();

        let midi_bytes = self
            .playback
            .loaded_midi_bytes()
            .map(|bytes| bytes.to_vec());
        let source_name = self
            .playback
            .loaded_midi()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "embedded.mid".to_owned());
        let resume_state = self.capture_playback_reload_resume_state();

        if let Some(midi_bytes) = midi_bytes {
            match self
                .playback
                .load_midi_bytes(midi_bytes, source_name, &requested_soundfonts)
            {
                Ok(message) => {
                    self.active_preset_id = preset.id.to_owned();
                    self.selected_preset_id = preset.id.to_owned();
                    self.mixer_assignment_mode = MixerAssignmentMode::Manual;
                    let loaded_ids = self.playback.loaded_soundfont_ids();
                    self.soundfonts = self.soundfont_rows_for_ids(&loaded_ids);
                    self.sync_coverage_cache_from_playback();
                    self.reset_strip_layout();
                    self.sync_playback_controls();
                    self.sync_playback_loop_settings();
                    self.restore_playback_reload_resume_state(resume_state);
                    if mark_dirty {
                        self.mark_dirty();
                    }
                    self.set_status(format!("Preset {} applied: {message}", preset.display_name));
                }
                Err(error) => self.set_status(format!("{error}")),
            }
            return;
        }

        self.active_preset_id = preset.id.to_owned();
        self.selected_preset_id = preset.id.to_owned();
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        self.soundfonts = self.soundfont_rows_for_ids(&requested_soundfonts);
        self.sync_coverage_cache_from_playback();
        self.reset_strip_layout();
        self.sync_playback_controls();
        if mark_dirty {
            self.mark_dirty();
        }
        self.set_status(format!("Preset {} applied", preset.display_name));
    }

    fn convert_to_custom_mode(&mut self) {
        self.active_preset_id = "custom".to_owned();
    }

    fn convert_manual_mixer_edit_to_custom_mode(&mut self) {
        self.convert_to_custom_mode();
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        self.mark_dirty();
    }

    fn apply_mixer_assignment_mode(&mut self, mode: MixerAssignmentMode) {
        self.sync_coverage_cache_from_playback();
        self.convert_to_custom_mode();
        self.mixer_assignment_mode = mode;

        let mut top_provider_by_strip = BTreeMap::new();
        for (font_index, font) in self.soundfonts.iter().enumerate() {
            for strip in &font.strips {
                if coverage_supports_strip(self.coverage_cache.get(&font.internal_id), strip) {
                    top_provider_by_strip.insert(
                        (
                            strip.channel,
                            strip.bank,
                            strip.program,
                            strip.is_percussion,
                        ),
                        font_index,
                    );
                }
            }
        }

        for (font_index, font) in self.soundfonts.iter_mut().enumerate() {
            font.muted = false;
            font.soloed = false;
            for strip in &mut font.strips {
                strip.volume = 1.0;
                strip.volume_policy_default = false;
                strip.mute_policy_default = false;
                strip.soloed = false;
                strip.muted = match mode {
                    MixerAssignmentMode::Manual => strip.muted,
                    MixerAssignmentMode::Balance => false,
                    MixerAssignmentMode::Layer => {
                        let key = (
                            strip.channel,
                            strip.bank,
                            strip.program,
                            strip.is_percussion,
                        );
                        match top_provider_by_strip.get(&key) {
                            Some(provider_index) => *provider_index != font_index,
                            None => true,
                        }
                    }
                };
            }
        }

        self.sync_playback_controls();
        self.mark_dirty();
        self.set_status(match mode {
            MixerAssignmentMode::Manual => "Mixer -> Custom".to_owned(),
            MixerAssignmentMode::Balance => "Mixer -> Balanced".to_owned(),
            MixerAssignmentMode::Layer => "Mixer -> Layered".to_owned(),
        });
    }

    fn capture_playback_reload_resume_state(&self) -> PlaybackReloadResumeState {
        PlaybackReloadResumeState {
            run_state: self.run_state,
            transport_seconds: self.playback.audible_transport_seconds(),
        }
    }

    fn restore_playback_reload_resume_state(&mut self, resume_state: PlaybackReloadResumeState) {
        if resume_state.transport_seconds > 0.0 {
            if let Err(error) = self
                .playback
                .seek_transport_seconds(resume_state.transport_seconds)
            {
                self.set_status(format!("{error}"));
            }
        }
        self.transport_position = self.playback.transport_fraction();
        match resume_state.run_state {
            AppRunState::Playing => self.play(),
            AppRunState::Paused => {
                self.run_state = AppRunState::Paused;
                let _ = self.playback.pause();
            }
            AppRunState::Idle => self.run_state = AppRunState::Idle,
        }
    }

    fn mixer_edit_snapshot(&self) -> MixerEditSnapshot {
        MixerEditSnapshot {
            master: self.master.clone(),
            smart_mix: self.smart_mix.clone(),
            rows: self
                .soundfonts
                .iter()
                .map(|row| RowEditSnapshot {
                    internal_id: row.internal_id.clone(),
                    muted: row.muted,
                    soloed: row.soloed,
                    strips: row
                        .strips
                        .iter()
                        .map(|strip| StripEditSnapshot {
                            channel: strip.channel,
                            bank: strip.bank,
                            program: strip.program,
                            is_percussion: strip.is_percussion,
                            volume: strip.volume,
                            muted: strip.muted,
                            soloed: strip.soloed,
                            pan: strip.pan,
                            gain_db: strip.gain_db,
                            limiter_enabled: strip.limiter_enabled,
                            limiter_amount: strip.limiter_amount,
                            reverb: strip.reverb,
                            chorus: strip.chorus,
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn apply_mixer_edit_snapshot(&mut self, snapshot: MixerEditSnapshot) {
        let mut mixer_changed =
            self.master != snapshot.master || self.smart_mix != snapshot.smart_mix;
        let rows_by_id = snapshot
            .rows
            .iter()
            .map(|row| (row.internal_id.as_str(), row))
            .collect::<BTreeMap<_, _>>();

        for row in &mut self.soundfonts {
            let Some(previous_row) = rows_by_id.get(row.internal_id.as_str()) else {
                continue;
            };
            if row.muted != previous_row.muted {
                mixer_changed = true;
            }
            if row.soloed != previous_row.soloed {
                mixer_changed = true;
            }

            let previous_strips = previous_row
                .strips
                .iter()
                .map(|strip| {
                    (
                        (
                            strip.channel,
                            strip.bank,
                            strip.program,
                            strip.is_percussion,
                        ),
                        strip,
                    )
                })
                .collect::<BTreeMap<_, _>>();
            for strip in &mut row.strips {
                let Some(previous_strip) = previous_strips.get(&(
                    strip.channel,
                    strip.bank,
                    strip.program,
                    strip.is_percussion,
                )) else {
                    continue;
                };
                if strip.volume != previous_strip.volume {
                    strip.volume_policy_default = false;
                    mixer_changed = true;
                }
                if strip.muted != previous_strip.muted {
                    strip.mute_policy_default = false;
                    mixer_changed = true;
                }
                if strip.soloed != previous_strip.soloed {
                    mixer_changed = true;
                }
                if strip.pan != previous_strip.pan
                    || strip.gain_db != previous_strip.gain_db
                    || strip.limiter_enabled != previous_strip.limiter_enabled
                    || strip.limiter_amount != previous_strip.limiter_amount
                    || strip.reverb != previous_strip.reverb
                    || strip.chorus != previous_strip.chorus
                {
                    strip.mute_policy_default = false;
                    strip.volume_policy_default = false;
                    mixer_changed = true;
                }
            }
        }

        if mixer_changed {
            self.convert_manual_mixer_edit_to_custom_mode();
            self.record_user_trace_point("mixer.edit", "changed");
        }
    }

    fn apply_soundfont_set_change(&mut self, requested_soundfonts: Vec<String>, action: &str) {
        let previous_rows = self.soundfonts.clone();
        let resume_state = self.capture_playback_reload_resume_state();
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;

        let midi_bytes = self
            .playback
            .loaded_midi_bytes()
            .map(|bytes| bytes.to_vec());
        let source_name = self
            .playback
            .loaded_midi()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "embedded.mid".to_owned());

        if let Some(midi_bytes) = midi_bytes {
            match self
                .playback
                .load_midi_bytes(midi_bytes, source_name, &requested_soundfonts)
            {
                Ok(message) => {
                    let loaded_ids = self.playback.loaded_soundfont_ids();
                    self.soundfonts = self.soundfont_rows_for_ids(&loaded_ids);
                    self.sync_coverage_cache_from_playback();
                    self.selected_soundfont = self
                        .catalog_soundfonts
                        .iter()
                        .position(|entry| {
                            self.soundfonts
                                .first()
                                .map(|font| font.internal_id.as_str())
                                .is_some_and(|id| id == entry.internal_id)
                        })
                        .unwrap_or(0);
                    self.reset_strip_layout();
                    self.restore_soundfont_row_controls(&previous_rows);
                    self.sync_playback_controls();
                    self.sync_playback_loop_settings();
                    self.restore_playback_reload_resume_state(resume_state);
                    self.mark_dirty();
                    self.set_status(format!("{action}: {message}"));
                }
                Err(error) => self.set_status(format!("{error}")),
            }
            return;
        }

        self.soundfonts = self.soundfont_rows_for_ids(&requested_soundfonts);
        self.sync_coverage_cache_from_playback();
        self.selected_soundfont = self
            .catalog_soundfonts
            .iter()
            .position(|entry| {
                self.soundfonts
                    .first()
                    .map(|font| font.internal_id.as_str())
                    .is_some_and(|id| id == entry.internal_id)
            })
            .unwrap_or(0);
        self.reset_strip_layout();
        self.restore_soundfont_row_controls(&previous_rows);
        self.sync_playback_controls();
        self.mark_dirty();
        self.set_status(action);
    }

    fn restore_soundfont_row_controls(&mut self, previous_rows: &[SoundFontUiRow]) {
        let previous_by_id = previous_rows
            .iter()
            .map(|row| (row.internal_id.clone(), row))
            .collect::<BTreeMap<_, _>>();

        for row in &mut self.soundfonts {
            let Some(previous_row) = previous_by_id.get(&row.internal_id) else {
                continue;
            };

            row.collapsed = previous_row.collapsed;
            row.muted = previous_row.muted;

            let previous_strip_map = previous_row
                .strips
                .iter()
                .map(|strip| {
                    (
                        (
                            strip.channel,
                            strip.bank,
                            strip.program,
                            strip.is_percussion,
                        ),
                        (
                            strip.volume,
                            strip.muted,
                            strip.pan,
                            strip.gain_db,
                            strip.limiter_enabled,
                            strip.limiter_amount,
                            strip.reverb,
                            strip.chorus,
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>();

            for strip in &mut row.strips {
                let Some(saved) = previous_strip_map.get(&(
                    strip.channel,
                    strip.bank,
                    strip.program,
                    strip.is_percussion,
                )) else {
                    continue;
                };

                strip.volume = saved.0;
                strip.muted = saved.1;
                strip.unsupported = false;
                strip.mute_policy_default = false;
                strip.volume_policy_default = false;
                strip.pan = saved.2;
                strip.gain_db = saved.3;
                strip.limiter_enabled = saved.4;
                strip.limiter_amount = saved.5;
                strip.reverb = saved.6;
                strip.chorus = saved.7;
            }
        }
    }

    fn soundfont_rows_for_ids(&self, ids: &[String]) -> Vec<SoundFontUiRow> {
        if ids.is_empty() {
            return vec![self
                .catalog_soundfonts
                .iter()
                .find(|font| font.is_default)
                .or_else(|| self.catalog_soundfonts.first())
                .map(SoundFontUiRow::from_catalog_entry)
                .unwrap_or_else(SoundFontUiRow::default_retro)];
        }

        ids.iter()
            .map(|id| {
                self.catalog_soundfonts
                    .iter()
                    .find(|entry| entry.internal_id == *id)
                    .map(SoundFontUiRow::from_catalog_entry)
                    .unwrap_or_else(|| SoundFontUiRow {
                        internal_id: id.clone(),
                        display_name: id.clone(),
                        is_default: false,
                        collapsed: true,
                        muted: false,
                        soloed: false,
                        strips: Vec::new(),
                    })
            })
            .collect()
    }

    fn apply_fmid_global_settings(&mut self, fmid: &FmidFile) {
        self.loop_enabled = fmid.looping.enabled;
        self.loop_mode = match fmid.looping.mode {
            FmidLoopMode::None => LoopMode::None,
            FmidLoopMode::Infinite => LoopMode::Infinite,
            FmidLoopMode::Counted => LoopMode::Counted,
        };
        self.loop_start_tick = fmid.looping.start_tick;
        self.loop_end_tick = fmid.looping.end_tick;
        self.loop_count = fmid.looping.loop_count as u32;
        self.normalize_loop_state(false);

        self.master = MasterControls {
            volume_db: fmid.mixer.master.volume_db as f32,
            limiter_enabled: fmid.mixer.master.limiter_enabled,
            limiter_amount: fmid.mixer.master.limiter_amount as f32,
            reverb: fmid.mixer.master.reverb as f32,
            chorus: fmid.mixer.master.chorus as f32,
            eq_low: fmid.mixer.master.eq_low_db as f32,
            eq_mid: fmid.mixer.master.eq_mid_db as f32,
            eq_high: fmid.mixer.master.eq_high_db as f32,
        };

        self.smart_mix = SmartMixControls {
            enabled: fmid.smart_mix.enabled,
            auto_normalize: fmid.smart_mix.auto_normalization_enabled,
            target_headroom_db: fmid.smart_mix.target_headroom as f32,
            attack_ms: fmid.smart_mix.attack as f32,
            release_ms: fmid.smart_mix.release as f32,
            lookahead_ms: fmid.smart_mix.lookahead as f32,
            normalization_amount: fmid.smart_mix.auto_normalization_amount as f32,
        };
    }

    fn apply_fmid_mixer_settings(&mut self, fmid: &FmidFile) {
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        let row_mutes = fmid
            .mixer
            .row_mutes
            .iter()
            .map(|row| (row.soundfont_id.as_str(), row.muted))
            .collect::<std::collections::BTreeMap<_, _>>();

        for font in &mut self.soundfonts {
            font.muted = row_mutes
                .get(font.internal_id.as_str())
                .copied()
                .unwrap_or(false);
            font.soloed = false;
            for strip in &mut font.strips {
                strip.soloed = false;
                strip.muted = false;
            }
        }

        for saved_strip in &fmid.mixer.strips {
            let Some(font) = self
                .soundfonts
                .iter_mut()
                .find(|font| font.internal_id == saved_strip.identity.soundfont_id)
            else {
                continue;
            };

            let Some(strip) = font.strips.iter_mut().find(|strip| {
                strip.channel as u64 == saved_strip.identity.midi_channel
                    && strip.program as u64 == saved_strip.identity.midi_program
                    && strip.is_percussion == saved_strip.identity.is_percussion
            }) else {
                continue;
            };

            strip.volume = saved_strip.controls.volume as f32;
            strip.muted = saved_strip.controls.mute;
            strip.pan = saved_strip.controls.pan as f32;
            strip.gain_db = saved_strip.controls.gain_db as f32;
            strip.limiter_enabled = saved_strip.controls.limiter_enabled;
            strip.limiter_amount = saved_strip.controls.limiter_amount as f32;
            strip.reverb = saved_strip.controls.reverb as f32;
            strip.chorus = saved_strip.controls.chorus as f32;
        }
    }

    fn apply_fmid_mixer_source_mode(&mut self, fmid: &FmidFile) {
        match &fmid.mixer_source_mode {
            MixerSourceMode::Custom => {
                self.active_preset_id = "custom".to_owned();
                self.apply_fmid_mixer_settings(fmid);
            }
            MixerSourceMode::PresetDefault(preset_id) => {
                let preset = self
                    .preset_set
                    .find_preset(preset_id)
                    .unwrap_or_else(|| self.preset_set.default_preset());
                self.active_preset_id = preset.id.to_owned();
                self.selected_preset_id = preset.id.to_owned();
            }
        }
    }

    fn apply_metadata_for_plain_midi(&mut self, path: &Path) {
        let project_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Loaded MIDI")
            .to_owned();
        let source_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("embedded.mid")
            .to_owned();
        self.metadata_edit_state = MetadataEditState {
            project_name: project_name.clone(),
            source_filename: source_name,
            ..MetadataEditState::default()
        };
        self.project_title = project_name;
        self.metadata_edit_dirty = false;
    }

    fn apply_metadata_for_fmid(&mut self, fmid: &FmidFile, path: &Path) {
        self.metadata_edit_state = MetadataEditState::from_fmid_project(
            &fmid.project,
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Loaded FMID")
                .to_owned(),
            "embedded.mid".to_owned(),
        );
        self.project_title = self.metadata_edit_state.project_name.clone();
        self.metadata_edit_dirty = false;
    }

    fn apply_metadata_for_decoded_audio(&mut self, wrapper: &FlutzAudioWrapper, path: &Path) {
        let project_name = if wrapper.metadata.project_name.trim().is_empty() {
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Loaded Audio")
                .to_owned()
        } else {
            wrapper.metadata.project_name.clone()
        };
        let source_filename = if wrapper.metadata.source_filename.trim().is_empty() {
            wrapper.source.original_filename.clone()
        } else {
            wrapper.metadata.source_filename.clone()
        };
        self.metadata_edit_state = MetadataEditState::from_track_metadata(
            &wrapper.metadata,
            project_name.clone(),
            source_filename,
        )
        .with_native_metadata(wrapper.native_metadata.clone());
        self.project_title = project_name;
        self.metadata_edit_dirty = false;
    }

    fn apply_decoded_audio_defaults(&mut self, loop_region: Option<MediaLoop>) {
        self.soundfonts.clear();
        self.coverage_cache.clear();
        self.selected_soundfont = 0;
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        self.master = MasterControls::default();
        self.smart_mix = SmartMixControls::default();
        self.final_output_volume_percent = 0.0;

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

    fn reset_decoded_peq_edit_state(&mut self, peq: Option<PeqPresetFile>) {
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

    fn apply_decoded_peq_runtime_config(&mut self, config: &PeqConfig) -> Result<()> {
        let runtime_config = effective_decoded_peq_config(config.clone(), self.decoded_peq_bypass_enabled);
        self.playback.set_decoded_peq_config(runtime_config).map(|_| ())
    }

    fn apply_decoded_peq_config_with_source(
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

    fn midi_has_non_default_loop_config(metadata: &MidiTransportMetadata) -> bool {
        metadata.jump_start_tick.is_some()
            || metadata.jump_end_tick.is_some()
            || !metadata.jump_points.is_empty()
    }

    fn fmid_has_non_default_loop_config(fmid: &FmidFile) -> bool {
        fmid.looping.enabled
            || !matches!(fmid.looping.mode, FmidLoopMode::None)
            || fmid.looping.start_tick > 0
            || fmid.looping.end_tick > 0
            || fmid.looping.loop_count > 1
    }

    fn playlist_has_multiple_entries(&self) -> bool {
        self.playlist
            .as_ref()
            .is_some_and(|playlist| playlist.entries.len() > 1)
    }

    fn should_enable_loop_by_default(&self, has_non_default_loop_config: bool) -> bool {
        if has_non_default_loop_config {
            return true;
        }
        !self.playlist_has_multiple_entries()
    }

    fn apply_loop_defaults_after_load(&mut self, has_non_default_loop_config: bool) {
        let enable_by_default = self.should_enable_loop_by_default(has_non_default_loop_config);
        if has_non_default_loop_config {
            self.normalize_loop_state(false);
            self.sync_playback_loop_settings();
            return;
        }

        if enable_by_default {
            let tick_length = self.transport_tick_length();
            self.loop_mode = LoopMode::Infinite;
            self.loop_enabled = tick_length > 0;
            self.loop_start_tick = 0;
            self.loop_end_tick = tick_length;
            self.loop_count = 1;
        } else {
            self.loop_mode = LoopMode::None;
            self.loop_enabled = false;
            self.loop_start_tick = 0;
            self.loop_end_tick = 0;
            self.loop_count = 1;
        }

        self.normalize_loop_state(false);
        self.sync_playback_loop_settings();
    }

    fn reset_plain_midi_defaults(&mut self) {
        self.dirty = false;
        self.mixer_assignment_mode = MixerAssignmentMode::Manual;
        self.final_output_volume_percent = 0.0;
        self.loop_enabled = false;
        self.loop_mode = LoopMode::None;
        self.loop_start_tick = 0;
        self.loop_end_tick = 0;
        self.loop_count = 1;
        self.master = MasterControls::default();
        self.smart_mix = SmartMixControls::default();
        for font in &mut self.soundfonts {
            font.muted = false;
            font.soloed = false;
            for strip in &mut font.strips {
                strip.volume = 1.0;
                strip.muted = false;
                strip.unsupported = false;
                strip.mute_policy_default = false;
                strip.volume_policy_default = false;
                strip.soloed = false;
                strip.pan = 0.0;
                strip.gain_db = 0.0;
                strip.limiter_enabled = false;
                strip.limiter_amount = 0.25;
                strip.reverb = 0.0;
                strip.chorus = 0.0;
                strip.meter = 0.0;
                strip.active = false;
                strip.current_note = "--".to_owned();
            }
        }
        self.sync_playback_output_volume();
    }

    fn refresh_realtime_feedback(&mut self) {
        for font in &mut self.soundfonts {
            for strip in &mut font.strips {
                strip.meter *= 0.82;
                strip.active = false;
                strip.current_note = "--".to_owned();
            }
        }

        self.transport_position = self.playback.transport_fraction();

        let output_view = self.playback.audible_output_view();
        self.apply_realtime_snapshot(output_view.snapshot);
        self.perf_trace.observe_frame();
        if self.perf_trace.is_periodic_sample_due() || self.latency_trace.is_periodic_sample_due() {
            let metrics = self.debug_metrics();
            if self.perf_trace.is_enabled() {
                self.perf_trace.sample_frame(&metrics);
            }
            if self.latency_trace.is_enabled() {
                self.latency_trace.sample_frame(&metrics);
            }
        }
    }

    fn begin_user_trace(&mut self, label: &str) -> u64 {
        if !self.perf_trace.is_enabled() {
            return 0;
        }
        let metrics = self.debug_metrics();
        self.perf_trace.begin_user_event(label, &metrics)
    }

    fn finish_user_trace(&mut self, operation_id: u64, label: &str, outcome: &str) {
        if !self.perf_trace.is_enabled() || operation_id == 0 {
            return;
        }
        let metrics = self.debug_metrics();
        self.perf_trace
            .finish_user_event(operation_id, label, outcome, &metrics);
    }

    fn record_user_trace_point(&mut self, label: &str, outcome: &str) {
        if !self.perf_trace.is_enabled() {
            return;
        }
        let metrics = self.debug_metrics();
        self.perf_trace.record_user_event(label, outcome, &metrics);
    }

    fn open_dialog_extensions() -> Vec<String> {
        let mut extensions = Self::playlist_dialog_extensions();
        extensions.push("fplist".to_owned());
        extensions.sort();
        extensions.dedup();
        extensions
    }

    fn playlist_dialog_extensions() -> Vec<String> {
        let mut extensions = Vec::new();
        for descriptor in builtin_registry().descriptors() {
            extensions.extend(
                descriptor
                    .extensions
                    .iter()
                    .map(|extension| (*extension).to_owned()),
            );
            extensions.extend(
                descriptor
                    .wrapped_extensions
                    .iter()
                    .map(|extension| (*extension).to_owned()),
            );
        }
        extensions.sort();
        extensions.dedup();
        extensions
    }

    fn save_as_dialog_spec(&self) -> (String, Vec<String>, String) {
        if let Some(content) = self
            .loaded_content
            .as_ref()
            .filter(|content| content.mastering == MasteringCapability::DecodedAudioPeq)
        {
            let extension = content
                .wrapped_extension
                .as_deref()
                .unwrap_or("faudio")
                .trim_start_matches('.');
            let default_name = default_wrapped_audio_file_name(&self.project_title, extension);
            return (
                format!("Flutz {} wrapper", content.friendly_name),
                vec![extension.to_owned()],
                default_name,
            );
        }

        (
            "FMID files".to_owned(),
            vec!["fmid".to_owned()],
            default_fmid_file_name(&self.project_title),
        )
    }

    fn record_diagnostic_trace(&mut self, label: &str, outcome: &str, details: &[(&str, String)]) {
        self.perf_trace.record_diagnostic(label, outcome, details);
    }

    fn log_midi_mapping_scan_start(&mut self, source: &str, path: &str) {
        self.record_diagnostic_trace(
            "midi.mapping.scan",
            "start",
            &[("source", source.to_owned()), ("path", path.to_owned())],
        );
    }

    fn log_midi_mapping_scan_result(&mut self, source: &str) {
        let strips = self
            .playback
            .midi_strip_layout()
            .into_iter()
            .map(|(channel, bank, program, is_percussion)| {
                format!("ch={channel} bank={bank} prog={program} perc={is_percussion}")
            })
            .collect::<Vec<_>>();
        let loaded_soundfonts = self
            .soundfonts
            .iter()
            .map(|font| font.internal_id.clone())
            .collect::<Vec<_>>();
        let per_font_strip_counts = self
            .soundfonts
            .iter()
            .map(|font| format!("{}={}", font.internal_id, font.strips.len()))
            .collect::<Vec<_>>();

        self.record_diagnostic_trace(
            "midi.mapping.scan",
            "result",
            &[
                ("source", source.to_owned()),
                ("soundfont_count", loaded_soundfonts.len().to_string()),
                ("soundfonts", loaded_soundfonts.join(",")),
                ("strip_count", strips.len().to_string()),
                ("strips", strips.join(";")),
                ("per_font_strip_counts", per_font_strip_counts.join(",")),
            ],
        );
    }

    fn sync_coverage_cache_from_playback(&mut self) {
        self.coverage_cache = self
            .catalog_soundfonts
            .iter()
            .filter_map(|font| {
                font.coverage
                    .clone()
                    .map(|coverage| (font.internal_id.clone(), coverage))
            })
            .collect();
        self.coverage_cache
            .extend(self.playback.loaded_soundfont_coverages());
    }

    fn apply_realtime_snapshot(&mut self, snapshot: RealtimeMixerSnapshot) {
        for visual in snapshot.strips.values() {
            let Some(font_index) = self
                .soundfonts
                .iter()
                .position(|font| font.internal_id == visual.soundfont_id)
            else {
                continue;
            };
            let strip_id = strip_id_for(
                font_index,
                visual.midi_channel,
                visual.midi_bank,
                visual.midi_program,
                visual.is_percussion,
            );
            let Some(strip_index) = self.soundfonts[font_index]
                .strips
                .iter()
                .position(|strip| strip.strip_id == strip_id)
            else {
                if visual.display_name.as_deref() != Some("Global effects") {
                    let key = RealtimeStripKey::from_snapshot(visual);
                    if self.logged_unmatched_snapshot_strips.insert(key) {
                        self.record_diagnostic_trace(
                            "midi.mapping.unmatched_realtime_strip",
                            "detected",
                            &[
                                ("soundfont", visual.soundfont_id.clone()),
                                ("channel", visual.midi_channel.to_string()),
                                ("bank", visual.midi_bank.to_string()),
                                ("program", visual.midi_program.to_string()),
                                ("is_percussion", visual.is_percussion.to_string()),
                                ("expected_strip_id", strip_id.0.to_string()),
                            ],
                        );
                    }
                }
                continue;
            };
            let strip = &mut self.soundfonts[font_index].strips[strip_index];
            strip.meter = visual.meter.peak.clamp(0.0, 1.0);
            strip.active = visual.audible || !visual.active_notes.is_empty();
            if let Some(display_name) = &visual.display_name {
                strip.program_name = display_name.clone();
            }
            strip.current_note = visual
                .active_notes
                .last()
                .map(|note| note_name(*note))
                .unwrap_or_else(|| "--".to_owned());
        }
    }

    fn apply_midi_transport_metadata(&mut self, metadata: MidiTransportMetadata) {
        if let Some(start_tick) = metadata.jump_start_tick {
            self.loop_start_tick = start_tick;
        }
        if let Some(end_tick) = metadata.jump_end_tick {
            self.loop_end_tick = end_tick;
        } else if metadata.tick_length > 0 {
            self.loop_end_tick = metadata.tick_length;
        }

        if self.loop_start_tick > 0 || self.loop_end_tick > 0 {
            self.loop_mode = LoopMode::Infinite;
            self.loop_enabled = true;
        }
        self.normalize_loop_state(false);

        if !metadata.jump_points.is_empty() {
            self.status = format!(
                "{} jump point(s) parsed: {}",
                metadata.jump_points.len(),
                metadata
                    .jump_points
                    .iter()
                    .map(|tick| tick.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    fn sync_playback_controls(&mut self) {
        let mut state = MixerControlState {
            settings: self.current_mixer_settings(),
            strip_controls: std::collections::BTreeMap::new(),
        };
        for font in &self.soundfonts {
            for strip in &font.strips {
                state.strip_controls.insert(
                    strip.strip_id,
                    MixerStripControls {
                        volume: strip.volume as f64,
                        mute: font.muted || strip.muted,
                        solo: font.soloed || strip.soloed,
                        pan: strip.pan as f64,
                        gain_db: strip.gain_db as f64,
                        limiter: LimiterControls {
                            enabled: strip.limiter_enabled,
                            amount: strip.limiter_amount as f64,
                            release: LimiterControls::default().release,
                        },
                        reverb: strip.reverb as f64,
                        chorus: strip.chorus as f64,
                    },
                );
            }
        }
        if let Err(error) = self.playback.set_mixer_controls(state) {
            self.set_status(format!("{error}"));
        }
    }

    fn current_loop_settings(&self) -> PlaybackLoopSettings {
        PlaybackLoopSettings {
            enabled: self.loop_enabled,
            mode: self.loop_mode.playback_mode(),
            start_tick: self.loop_start_tick,
            end_tick: self.loop_end_tick,
            loop_count: self.loop_count.max(1),
        }
    }

    fn loop_state_tuple(&self) -> (bool, LoopMode, u64, u64, u32) {
        (
            self.loop_enabled,
            self.loop_mode,
            self.loop_start_tick,
            self.loop_end_tick,
            self.loop_count,
        )
    }

    fn normalize_loop_state(&mut self, emit_status: bool) {
        let tick_length = self.transport_tick_length();
        self.loop_start_tick = self.loop_start_tick.min(tick_length);
        self.loop_end_tick = self.loop_end_tick.min(tick_length);
        self.loop_count = self.loop_count.max(1);

        if matches!(self.loop_mode, LoopMode::None) {
            self.loop_enabled = false;
            return;
        }

        if self.loop_enabled && self.loop_end_tick <= self.loop_start_tick {
            self.loop_enabled = false;
            if emit_status {
                self.set_status("Loop disabled: start tick must be less than end tick");
            }
        }
    }

    fn sync_playback_loop_settings(&mut self) {
        if let Err(error) = self
            .playback
            .set_loop_settings(self.current_loop_settings())
        {
            self.set_status(format!("{error}"));
        }
    }

    fn commit_loop_state_change(
        &mut self,
        previous: (bool, LoopMode, u64, u64, u32),
    ) -> &'static str {
        let changed = previous != self.loop_state_tuple();
        let mut outcome = "ok";
        if let Err(error) = self
            .playback
            .set_loop_settings(self.current_loop_settings())
        {
            self.set_status(format!("{error}"));
            outcome = "error";
        }
        if changed {
            self.mark_dirty();
        }
        outcome
    }

    fn handle_counted_loop_seek(&mut self, target_tick: u64) {
        if !self.loop_enabled || !matches!(self.loop_mode, LoopMode::Counted) {
            return;
        }
        if target_tick >= self.loop_start_tick && target_tick < self.loop_end_tick {
            return;
        }

        self.loop_enabled = false;
        self.sync_playback_loop_settings();
        self.mark_dirty();
        self.set_status("Loop disabled after seeking outside counted loop range");
    }

    fn sync_playback_output_volume(&mut self) {
        if let Err(error) = self
            .playback
            .set_session_output_gain(self.final_output_volume_multiplier())
        {
            self.set_status(format!("{error}"));
        }
    }

    fn current_mixer_settings(&self) -> MixerSettings {
        let target_peak = 10.0_f32.powf(self.smart_mix.target_headroom_db / 20.0);
        MixerSettings {
            master: MixerMasterControls {
                volume_db: self.master.volume_db as f64,
                limiter: LimiterControls {
                    enabled: self.master.limiter_enabled,
                    amount: self.master.limiter_amount as f64,
                    release: LimiterControls::default().release,
                },
                reverb: self.master.reverb as f64,
                chorus: self.master.chorus as f64,
                eq_low_db: self.master.eq_low as f64,
                eq_mid_db: self.master.eq_mid as f64,
                eq_high_db: self.master.eq_high as f64,
            },
            smart_mix: SmartMixSettings {
                enabled: self.smart_mix.enabled,
                target_headroom: (1.0 - target_peak.clamp(0.05, 1.0)) as f64,
                attack: (self.smart_mix.attack_ms / 2_000.0).clamp(0.0, 1.0) as f64,
                release: (self.smart_mix.release_ms / 2_000.0).clamp(0.0, 1.0) as f64,
                lookahead: (self.smart_mix.lookahead_ms / 2_000.0).clamp(0.0, 1.0) as f64,
            },
            auto_normalization: AutoNormalization {
                enabled: self.smart_mix.auto_normalize,
                amount: (self.smart_mix.normalization_amount / 100.0).clamp(0.0, 1.0) as f64,
            },
        }
    }
}

fn load_app_icon() -> egui::IconData {
    let png_bytes = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/flutzplayer-icon.png"
    ));
    let image = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
        .expect("flutzPlayer icon PNG must decode")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

fn default_fmid_file_name(project_title: &str) -> String {
    let trimmed_title = project_title.trim();
    let stem = Path::new(trimmed_title)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(trimmed_title);
    format!("{}.fmid", stem.trim_end_matches('.'))
}

fn default_wrapped_audio_file_name(project_title: &str, extension: &str) -> String {
    let trimmed_title = project_title.trim();
    let stem = Path::new(trimmed_title)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(trimmed_title);
    format!("{}.{}", stem.trim_end_matches('.'), extension)
}

fn sanitize_file_stem(project_title: &str) -> String {
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

fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
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

fn builtin_decoded_peq_preset_names() -> &'static [&'static str] {
    &["Default", "Flat"]
}

fn builtin_decoded_peq_preset_text(name: &str) -> Option<&'static str> {
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

fn load_builtin_decoded_peq_preset(name: &str) -> PeqPresetFile {
    let text = builtin_decoded_peq_preset_text(name)
        .unwrap_or_else(|| panic!("missing built-in decoded PEQ preset {name}"));
    let mut preset = deserialize_preset_toml(text)
        .unwrap_or_else(|error| panic!("failed to parse built-in decoded PEQ preset {name}: {error}"));
    preset.metadata.name = Some(name.to_owned());
    preset.metadata.tags.clear();
    preset
}

fn default_decoded_peq_preset(sample_rate_hz: u32, channel_count: u16) -> PeqPresetFile {
    PeqPresetFile {
        metadata: PresetMetadata {
            name: Some("Current Track Wrapper Settings".to_owned()),
            ..PresetMetadata::default()
        },
        config: default_decoded_peq_config(sample_rate_hz, channel_count),
        extra_fields: BTreeMap::new(),
    }
}

fn normalize_runtime_decoded_peq_preset(
    mut preset: PeqPresetFile,
    sample_rate_hz: u32,
    channel_count: u16,
) -> PeqPresetFile {
    preset.metadata.tags.clear();
    preset.config = normalized_decoded_peq_config(preset.config, sample_rate_hz, channel_count);
    preset
}

fn normalize_default_decoded_peq_preset(mut preset: PeqPresetFile) -> PeqPresetFile {
    preset.metadata.name = Some("Default".to_owned());
    preset.metadata.tags.clear();
    preset
}

fn effective_decoded_peq_config(mut config: PeqConfig, bypass_enabled: bool) -> PeqConfig {
    if bypass_enabled {
        config.wet_mix = 0.0;
    }
    config
}

fn default_decoded_peq_config(sample_rate_hz: u32, channel_count: u16) -> PeqConfig {
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

fn normalized_decoded_peq_config(
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

fn peq_band_sort_frequency(band: &PeqBandConfig) -> f32 {
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
