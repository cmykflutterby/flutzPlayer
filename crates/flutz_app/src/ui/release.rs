use eframe::egui;
use flutz_formats::MasteringCapability;
use flutz_peq::{Bandwidth, PeqBandConfig, PeqFilterType};
use flutz_visualizer_egui::{paint_visualizer, VisualizerRendererConfig};

use crate::app::{
    AppRunState, FlutzDesktopApp, LoopMode, MixerAssignmentMode, MixerStripUiState, SoundFontUiRow,
    MASTER_VOLUME_MAX_DB,
};
use crate::ui::{apply_dracula_theme, DRACULA_PANEL_FILL, DRACULA_PANEL_STROKE};

mod layout;
mod mixer;
mod panels;
mod peq;
mod status;
mod transport;
pub use layout::UiRefreshPolicy;
use layout::*;
use mixer::*;
use panels::*;
use peq::*;
use status::*;
use transport::*;

pub fn apply_theme(context: &egui::Context) {
    apply_dracula_theme(context);
}

pub fn draw_app(context: &egui::Context, app: &mut FlutzDesktopApp) {
    sync_release_viewport(context, app);
    draw_top_action_bar(context, app);
    draw_missing_preset_warning(context, app);
    let editor_panel_height = active_editor_panel_height(context, app);

    let top_height = HEADER_HEIGHT
        + SPECTRUM_HEIGHT
        + TRANSPORT_HEIGHT
        + PANEL_GAP * 2.0
        + if app.release_edit_mode() {
            editor_panel_height + PANEL_GAP * 2.0
        } else {
            0.0
        };

    egui::TopBottomPanel::top("player_top_panel")
        .resizable(false)
        .exact_height(top_height)
        .show(context, |ui| {
            draw_fixed_height(ui, HEADER_HEIGHT, |ui| draw_project_header(ui, app));
            ui.add_space(PANEL_GAP);
            draw_fixed_height(ui, SPECTRUM_HEIGHT, |ui| draw_spectrum_visualizer(ui, app));
            ui.add_space(PANEL_GAP);
            draw_fixed_height(ui, TRANSPORT_HEIGHT, |ui| draw_transport(ui, app));
            if app.release_edit_mode() {
                ui.add_space(PANEL_GAP);
                draw_fixed_height(ui, editor_panel_height, |ui| draw_editor_controls(ui, app));
                ui.add_space(PANEL_GAP);
            }
        });

    #[cfg(debug_assertions)]
    draw_debug_metrics_panel(context, app);

    egui::TopBottomPanel::bottom("status_bar")
        .resizable(false)
        .exact_height(STATUS_HEIGHT)
        .frame(
            egui::Frame::side_top_panel(&context.style())
                .fill(RELEASE_PANEL_FILL)
                .inner_margin(0.0),
        )
        .show(context, |ui| {
            draw_status_bar(ui, app);
        });

    if app.release_edit_mode()
        && app.active_mastering_capability() != MasteringCapability::DecodedAudioPeq
    {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&context.style())
                    .fill(RELEASE_PANEL_FILL)
                    .inner_margin(0.0),
            )
            .show(context, |ui| {
                draw_mixer(ui, app);
            });
    }

    app.draw_playlist_viewport_host(context);
    app.draw_metadata_viewport_host(context);
}
