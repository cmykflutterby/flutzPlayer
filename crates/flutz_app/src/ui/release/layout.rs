use super::*;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct UiRefreshPolicy {
    pub snapshots_per_second: u16,
}

pub(super) const PLAYER_MIN_WIDTH: f32 = 320.0;
pub(super) const PLAYER_TARGET_WIDTH: f32 = 480.0;
pub(super) const EDITOR_MIN_WIDTH: f32 = 720.0;
pub(super) const PLAYER_MIN_HEIGHT: f32 = 283.0;
pub(super) const EDITOR_MIN_HEIGHT: f32 = 610.0;
pub(super) const TOP_ACTION_HEIGHT: f32 = 30.0;
pub(super) const TOP_ACTION_INNER_MARGIN: f32 = 10.0;
pub(super) const HEADER_HEIGHT: f32 = 52.0;
pub(super) const SPECTRUM_HEIGHT: f32 = 72.0;
pub(super) const TRANSPORT_HEIGHT: f32 = 80.0;
pub(super) const EDIT_PANEL_HEIGHT: f32 = 224.0;
pub(super) const EDIT_PANEL_COLLAPSED_HEIGHT: f32 = 52.0;
pub(super) const DECODED_LOOP_PANEL_HEIGHT: f32 = 92.0;
pub(super) const DECODED_EDITOR_PANEL_HEIGHT: f32 = 544.0;
pub(super) const DECODED_EDITOR_PANEL_GAP: f32 = 8.0;
pub(super) const DECODED_EDITOR_COLLAPSED_PANEL_HEIGHT: f32 =
    DECODED_LOOP_PANEL_HEIGHT + DECODED_EDITOR_PANEL_GAP + EDIT_PANEL_COLLAPSED_HEIGHT;
pub(super) const PEQ_MIN_BAND_TABLE_HEIGHT: f32 = 124.0;
pub(super) const STATUS_HEIGHT: f32 = 22.0;
pub(super) const DECODED_STATUS_BAR_RESERVE: f32 = STATUS_HEIGHT + 8.0;
pub(super) const PANEL_GAP: f32 = 6.0;
pub(super) const MIXER_HEADER_HEIGHT: f32 = 34.0;
pub(super) const MIXER_COLLAPSED_ROW_HEIGHT: f32 = 36.0;
pub(super) const MIXER_EXPANDED_ROW_HEIGHT: f32 = 252.0;
pub(super) const MIXER_FX_EXPANDED_ROW_HEIGHT: f32 = 642.0;
pub(super) const CONTENT_SIDE_PADDING: f32 = 5.0;
pub(super) const RELEASE_PANEL_FILL: egui::Color32 = DRACULA_PANEL_FILL;
pub(super) const RELEASE_PANEL_STROKE: egui::Color32 = DRACULA_PANEL_STROKE;

#[derive(Clone, Copy)]
pub(super) struct ReleaseViewportSync {
    initialized: bool,
    last_edit_mode: bool,
}

pub(super) fn sync_release_viewport(context: &egui::Context, app: &mut FlutzDesktopApp) {
    let edit_mode = app.release_edit_mode();
    let sync_id = egui::Id::new("release_viewport_sync");
    let mut sync_state = context
        .data_mut(|data| data.get_temp::<ReleaseViewportSync>(sync_id))
        .unwrap_or(ReleaseViewportSync {
            initialized: false,
            last_edit_mode: edit_mode,
        });
    let min_width = if edit_mode {
        EDITOR_MIN_WIDTH
    } else {
        PLAYER_MIN_WIDTH
    };
    let min_height = if edit_mode {
        editor_min_height(app)
    } else {
        PLAYER_MIN_HEIGHT
    };
    context.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
        min_width, min_height,
    )));

    let current_size = context.screen_rect().size();
    let entered_player_mode = sync_state.initialized && sync_state.last_edit_mode && !edit_mode;
    let initial_player_layout = !sync_state.initialized && !edit_mode;

    if edit_mode {
        let target_height = editor_target_height(context, app);
        if current_size.x < EDITOR_MIN_WIDTH || (current_size.y - target_height).abs() > 1.0 {
            context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                current_size.x.max(EDITOR_MIN_WIDTH),
                target_height,
            )));
        }
    } else if (current_size.y - PLAYER_MIN_HEIGHT).abs() > 1.0
        || entered_player_mode
        || initial_player_layout
    {
        let target_width = if entered_player_mode || initial_player_layout {
            current_size
                .x
                .min(PLAYER_TARGET_WIDTH)
                .max(PLAYER_MIN_WIDTH)
        } else {
            current_size.x.max(PLAYER_MIN_WIDTH)
        };
        context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            target_width,
            PLAYER_MIN_HEIGHT,
        )));
    }

    sync_state.initialized = true;
    sync_state.last_edit_mode = edit_mode;
    context.data_mut(|data| data.insert_temp(sync_id, sync_state));
}

pub(super) fn editor_panel_height(app: &FlutzDesktopApp) -> f32 {
    if app.release_editor_panels_collapsed()
        && app.active_mastering_capability() == MasteringCapability::DecodedAudioPeq
    {
        DECODED_EDITOR_COLLAPSED_PANEL_HEIGHT
    } else if app.release_editor_panels_collapsed() {
        EDIT_PANEL_COLLAPSED_HEIGHT
    } else if app.active_mastering_capability() == MasteringCapability::DecodedAudioPeq {
        DECODED_EDITOR_PANEL_HEIGHT
    } else {
        EDIT_PANEL_HEIGHT
    }
}

pub(super) fn active_editor_panel_height(context: &egui::Context, app: &FlutzDesktopApp) -> f32 {
    if app.release_editor_panels_collapsed()
        || app.active_mastering_capability() != MasteringCapability::DecodedAudioPeq
    {
        return editor_panel_height(app);
    }

    let viewport_height = context.screen_rect().height();
    let fixed_height = TOP_ACTION_HEIGHT
        + HEADER_HEIGHT
        + SPECTRUM_HEIGHT
        + TRANSPORT_HEIGHT
        + STATUS_HEIGHT
        + PANEL_GAP * 4.0;
    (viewport_height - fixed_height).clamp(
        DECODED_EDITOR_COLLAPSED_PANEL_HEIGHT,
        DECODED_EDITOR_PANEL_HEIGHT,
    )
}

pub(super) fn editor_top_stack_height(app: &FlutzDesktopApp) -> f32 {
    TOP_ACTION_HEIGHT
        + HEADER_HEIGHT
        + SPECTRUM_HEIGHT
        + TRANSPORT_HEIGHT
        + editor_panel_height(app)
        + STATUS_HEIGHT
        + PANEL_GAP * 4.0
}

pub(super) fn editor_min_height(app: &FlutzDesktopApp) -> f32 {
    if app.active_mastering_capability() == MasteringCapability::DecodedAudioPeq {
        return (editor_top_stack_height(app) + DECODED_STATUS_BAR_RESERVE).max(PLAYER_MIN_HEIGHT);
    }

    (editor_top_stack_height(app) + MIXER_HEADER_HEIGHT + MIXER_COLLAPSED_ROW_HEIGHT)
        .max(EDITOR_MIN_HEIGHT)
}

pub(super) fn editor_target_height(context: &egui::Context, app: &mut FlutzDesktopApp) -> f32 {
    if app.active_mastering_capability() == MasteringCapability::DecodedAudioPeq {
        return editor_min_height(app);
    }

    let fx_expanded = app.mixer_fx_expanded();
    let row_count = app.soundfont_rows().len().max(1) as f32;
    let mixer_rows_height = app
        .soundfont_rows()
        .iter()
        .map(|font| {
            if font.collapsed {
                MIXER_COLLAPSED_ROW_HEIGHT
            } else if fx_expanded {
                MIXER_FX_EXPANDED_ROW_HEIGHT
            } else {
                MIXER_EXPANDED_ROW_HEIGHT
            }
        })
        .sum::<f32>()
        .max(MIXER_COLLAPSED_ROW_HEIGHT * row_count);

    let target_height = (editor_top_stack_height(app) + MIXER_HEADER_HEIGHT + mixer_rows_height)
        .max(editor_min_height(app));
    context
        .input(|input| input.viewport().monitor_size)
        .map(|monitor_size| target_height.min((monitor_size.y - 64.0).max(editor_min_height(app))))
        .unwrap_or(target_height)
}

pub(super) fn draw_fixed_height(
    ui: &mut egui::Ui,
    height: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let width = bounded_available_width(ui);
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            ui.set_max_width(width);
            add_contents(ui);
        },
    );
}

pub(super) fn bounded_available_width(ui: &egui::Ui) -> f32 {
    ui.available_width()
        .min(ui.available_rect_before_wrap().width())
        .min(ui.max_rect().width())
        .max(0.0)
}
