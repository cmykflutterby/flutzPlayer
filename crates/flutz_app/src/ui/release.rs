use eframe::egui;
use flutz_visualizer_egui::{paint_visualizer, VisualizerRendererConfig};

use crate::app::{
    AppRunState, FlutzDesktopApp, LoopMode, MixerAssignmentMode, MixerStripUiState, SoundFontUiRow,
    MASTER_VOLUME_MAX_DB,
};
use crate::ui::{apply_dracula_theme, DRACULA_PANEL_FILL, DRACULA_PANEL_STROKE};

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct UiRefreshPolicy {
    pub snapshots_per_second: u16,
}

const PLAYER_MIN_WIDTH: f32 = 320.0;
const PLAYER_TARGET_WIDTH: f32 = 480.0;
const EDITOR_MIN_WIDTH: f32 = 720.0;
const PLAYER_MIN_HEIGHT: f32 = 283.0;
const EDITOR_MIN_HEIGHT: f32 = 610.0;
const TOP_ACTION_HEIGHT: f32 = 30.0;
const TOP_ACTION_INNER_MARGIN: f32 = 10.0;
const HEADER_HEIGHT: f32 = 52.0;
const SPECTRUM_HEIGHT: f32 = 72.0;
const TRANSPORT_HEIGHT: f32 = 80.0;
const EDIT_PANEL_HEIGHT: f32 = 224.0;
const EDIT_PANEL_COLLAPSED_HEIGHT: f32 = 52.0;
const STATUS_HEIGHT: f32 = 22.0;
const PANEL_GAP: f32 = 6.0;
const MIXER_HEADER_HEIGHT: f32 = 34.0;
const MIXER_COLLAPSED_ROW_HEIGHT: f32 = 36.0;
const MIXER_EXPANDED_ROW_HEIGHT: f32 = 252.0;
const MIXER_FX_EXPANDED_ROW_HEIGHT: f32 = 642.0;
const CONTENT_SIDE_PADDING: f32 = 5.0;
const RELEASE_PANEL_FILL: egui::Color32 = DRACULA_PANEL_FILL;
const RELEASE_PANEL_STROKE: egui::Color32 = DRACULA_PANEL_STROKE;

#[derive(Clone, Copy)]
struct ReleaseViewportSync {
    initialized: bool,
    last_edit_mode: bool,
}

pub fn apply_theme(context: &egui::Context) {
    apply_dracula_theme(context);
}

pub fn draw_app(context: &egui::Context, app: &mut FlutzDesktopApp) {
    sync_release_viewport(context, app);
    draw_top_action_bar(context, app);
    draw_missing_preset_warning(context, app);
    let editor_panel_height = editor_panel_height(app);

    let top_height = HEADER_HEIGHT
        + SPECTRUM_HEIGHT
        + TRANSPORT_HEIGHT
        + PANEL_GAP * 2.0
        + if app.release_edit_mode() {
            editor_panel_height + PANEL_GAP
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

    if app.release_edit_mode() {
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

fn sync_release_viewport(context: &egui::Context, app: &mut FlutzDesktopApp) {
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

fn editor_panel_height(app: &FlutzDesktopApp) -> f32 {
    if app.release_editor_panels_collapsed() {
        EDIT_PANEL_COLLAPSED_HEIGHT
    } else {
        EDIT_PANEL_HEIGHT
    }
}

fn editor_top_stack_height(app: &FlutzDesktopApp) -> f32 {
    (TOP_ACTION_HEIGHT + TOP_ACTION_INNER_MARGIN * 2.0)
        + HEADER_HEIGHT
        + SPECTRUM_HEIGHT
        + TRANSPORT_HEIGHT
        + editor_panel_height(app)
        + STATUS_HEIGHT
        + PANEL_GAP * 4.0
}

fn editor_min_height(app: &FlutzDesktopApp) -> f32 {
    (editor_top_stack_height(app) + MIXER_HEADER_HEIGHT + MIXER_COLLAPSED_ROW_HEIGHT)
        .max(EDITOR_MIN_HEIGHT)
}

fn editor_target_height(context: &egui::Context, app: &mut FlutzDesktopApp) -> f32 {
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

fn draw_fixed_height(ui: &mut egui::Ui, height: f32, add_contents: impl FnOnce(&mut egui::Ui)) {
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

fn bounded_available_width(ui: &egui::Ui) -> f32 {
    ui.available_width()
        .min(ui.available_rect_before_wrap().width())
        .min(ui.max_rect().width())
        .max(0.0)
}

fn draw_status_bar(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    const LEFT_PAD: f32 = 20.0;
    const RIGHT_PAD: f32 = 8.0;
    const SLOT_TOP_PAD: f32 = 1.0;
    const SLOT_HEIGHT: f32 = 20.0;
    const RUN_WIDTH: f32 = 60.0;
    const MODE_WIDTH: f32 = 40.0;
    const BACKEND_WIDTH: f32 = 40.0;
    const LATENCY_WIDTH: f32 = 58.0;
    const UNAVAILABLE_WIDTH: f32 = 148.0;
    const RETRY_WIDTH: f32 = 80.0;
    const GAP: f32 = 10.0;

    let row_rect = egui::Rect::from_min_size(
        ui.max_rect().min,
        egui::vec2(ui.max_rect().width(), STATUS_HEIGHT),
    );
    ui.allocate_rect(row_rect, egui::Sense::hover());

    let mut cursor_x = row_rect.left() + LEFT_PAD;
    let slot_top = row_rect.top() + SLOT_TOP_PAD;
    let separator_top = row_rect.top() + 4.0;
    let separator_bottom = row_rect.bottom() - 4.0;
    let separator_color = egui::Color32::from_rgb(82, 88, 94);

    paint_status_slot(
        ui,
        egui::Rect::from_min_size(
            egui::pos2(cursor_x, slot_top),
            egui::vec2(RUN_WIDTH, SLOT_HEIGHT),
        ),
        match app.run_state() {
            AppRunState::Idle => "○ Idle".to_owned(),
            AppRunState::Playing => "● Playing".to_owned(),
            AppRunState::Paused => "⏸ Paused".to_owned(),
        },
        false,
    );
    cursor_x += RUN_WIDTH + GAP;
    draw_status_separator(
        ui,
        cursor_x,
        separator_top,
        separator_bottom,
        separator_color,
    );
    cursor_x += GAP;

    paint_status_slot(
        ui,
        egui::Rect::from_min_size(
            egui::pos2(cursor_x, slot_top),
            egui::vec2(MODE_WIDTH, SLOT_HEIGHT),
        ),
        if app.release_edit_mode() {
            "Editor".to_owned()
        } else {
            "Player".to_owned()
        },
        true,
    );
    cursor_x += MODE_WIDTH + GAP;
    draw_status_separator(
        ui,
        cursor_x,
        separator_top,
        separator_bottom,
        separator_color,
    );
    cursor_x += GAP;

    let audio_status = app.audio_status();
    if audio_status.to_ascii_lowercase().contains("unavailable") {
        paint_status_slot(
            ui,
            egui::Rect::from_min_size(
                egui::pos2(cursor_x, slot_top),
                egui::vec2(UNAVAILABLE_WIDTH, SLOT_HEIGHT),
            ),
            format!("⚠ {} unavailable", app.audio_backend_label()),
            false,
        );
        cursor_x += UNAVAILABLE_WIDTH + 6.0;
        let retry_rect = egui::Rect::from_min_size(
            egui::pos2(cursor_x, row_rect.top() + 3.0),
            egui::vec2(RETRY_WIDTH, 22.0),
        );
        if ui
            .put(retry_rect, egui::Button::new("Retry Audio"))
            .clicked()
        {
            app.retry_audio();
        }
        cursor_x += RETRY_WIDTH + GAP;
    } else {
        paint_status_slot(
            ui,
            egui::Rect::from_min_size(
                egui::pos2(cursor_x, slot_top),
                egui::vec2(BACKEND_WIDTH, SLOT_HEIGHT),
            ),
            app.audio_backend_label().to_owned(),
            false,
        );

        cursor_x += BACKEND_WIDTH + GAP;
        draw_status_separator(
            ui,
            cursor_x,
            separator_top,
            separator_bottom,
            separator_color,
        );
        cursor_x += GAP;

        paint_status_slot(
            ui,
            egui::Rect::from_min_size(
                egui::pos2(cursor_x, slot_top),
                egui::vec2(LATENCY_WIDTH, SLOT_HEIGHT),
            ),
            format!("{:.1} ms", app.measured_output_latency_ms()),
            false,
        );
        cursor_x += LATENCY_WIDTH + GAP;
    }

    draw_status_separator(
        ui,
        cursor_x,
        separator_top,
        separator_bottom,
        separator_color,
    );
    cursor_x += GAP;

    let status_rect = egui::Rect::from_min_max(
        egui::pos2(cursor_x, slot_top),
        egui::pos2(
            (row_rect.right() - RIGHT_PAD).max(cursor_x),
            slot_top + SLOT_HEIGHT,
        ),
    );
    paint_status_slot(ui, status_rect, app.status().to_owned(), false);
}

fn paint_status_slot(ui: &mut egui::Ui, rect: egui::Rect, text: String, strong: bool) {
    ui.allocate_rect(rect, egui::Sense::hover());
    if rect.width() <= 0.0 {
        return;
    }

    let mut font_id = egui::TextStyle::Body.resolve(ui.style());
    font_id.size = 11.0;
    let color = ui.visuals().text_color();
    let text = elide_text_to_width(ui, text, font_id.clone(), color, rect.width());
    let galley = ui.fonts_mut(|fonts| {
        let mut job = egui::text::LayoutJob::single_section(
            text,
            egui::TextFormat {
                font_id,
                color,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
        if strong {
            job.sections[0].format.font_id.size = 10.0;
        }
        job.wrap.max_width = f32::INFINITY;
        fonts.layout_job(job)
    });

    ui.painter().with_clip_rect(rect).galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
}

fn elide_text_to_width(
    ui: &egui::Ui,
    text: String,
    font_id: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> String {
    ui.fonts_mut(|fonts| {
        if fonts
            .layout_no_wrap(text.clone(), font_id.clone(), color)
            .size()
            .x
            <= width
        {
            return text;
        }

        let ellipsis = "...";
        if fonts
            .layout_no_wrap(ellipsis.to_owned(), font_id.clone(), color)
            .size()
            .x
            > width
        {
            return String::new();
        }

        let mut low = 0;
        let mut high = text.chars().count();
        while low < high {
            let mid = (low + high + 1) / 2;
            let candidate = format!("{}{}", text.chars().take(mid).collect::<String>(), ellipsis);
            if fonts
                .layout_no_wrap(candidate, font_id.clone(), color)
                .size()
                .x
                <= width
            {
                low = mid;
            } else {
                high = mid - 1;
            }
        }

        format!("{}{}", text.chars().take(low).collect::<String>(), ellipsis)
    })
}

fn draw_status_separator(ui: &egui::Ui, x: f32, top: f32, bottom: f32, color: egui::Color32) {
    ui.painter().line_segment(
        [egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(1.0, color),
    );
}

fn draw_top_action_bar(context: &egui::Context, app: &mut FlutzDesktopApp) {
    let mut speaker_rect = None;
    let mut speaker_clicked = false;

    egui::TopBottomPanel::top("top_action_bar")
        .resizable(false)
        .exact_height(TOP_ACTION_HEIGHT)
        .frame(
            egui::Frame::side_top_panel(&context.style())
                .fill(RELEASE_PANEL_FILL)
                .inner_margin(TOP_ACTION_INNER_MARGIN),
        )
        .show(context, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open").clicked() {
                    app.open_midi_dialog();
                }

                if ui
                    .add_enabled(app.has_loaded_project(), egui::Button::new("Save"))
                    .clicked()
                {
                    app.save_project();
                }
                if ui
                    .add_enabled(app.has_loaded_project(), egui::Button::new("Save As"))
                    .clicked()
                {
                    app.save_project_as();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let response = ui.add_sized([28.0, 22.0], egui::Button::new("🔊"));
                    speaker_rect = Some(response.rect);
                    if response.clicked() {
                        app.toggle_speaker_volume_popup();
                        speaker_clicked = true;
                    }

                    let edit_response = ui.add(
                        egui::Button::new(if app.release_edit_mode() {
                            "Edit Mode"
                        } else {
                            "Edit Mode"
                        })
                        .selected(app.release_edit_mode()),
                    );
                    if edit_response.clicked() {
                        app.toggle_release_edit_mode();
                    }
                });
            });
        });

    if app.speaker_volume_popup_open() {
        let anchor = speaker_rect
            .map(|rect| rect.left_bottom() + egui::vec2(-18.0, 2.0))
            .unwrap_or_else(|| egui::pos2(0.0, 30.0));
        let popup = egui::Area::new("speaker_volume_popup".into())
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .show(context, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(58.0);
                    ui.vertical_centered(|ui| {
                        ui.small("Out");
                        let mut final_output_volume = app.final_output_volume_percent();
                        ui.add_sized(
                            [18.0, 96.0],
                            egui::Slider::new(&mut final_output_volume, -100.0..=100.0)
                                .vertical()
                                .show_value(false),
                        );
                        app.set_final_output_volume_percent(final_output_volume);
                        ui.add(
                            egui::DragValue::new(&mut final_output_volume)
                                .range(-100.0..=100.0)
                                .speed(1.0)
                                .suffix(" %"),
                        );
                        app.set_final_output_volume_percent(final_output_volume);
                        ui.small(format!("{:.2}x", app.final_output_volume_multiplier()));
                    });
                });
            });

        if context.input(|input| input.pointer.any_click()) && !speaker_clicked {
            let pointer_pos = context.input(|input| input.pointer.interact_pos());
            let clicked_popup = pointer_pos
                .map(|pos| popup.response.rect.contains(pos))
                .unwrap_or(false);
            let clicked_button = pointer_pos
                .and_then(|pos| speaker_rect.map(|rect| rect.contains(pos)))
                .unwrap_or(false);
            if !clicked_popup && !clicked_button {
                app.close_speaker_volume_popup();
            }
        }
    }
}

#[cfg(debug_assertions)]
fn draw_debug_metrics_panel(context: &egui::Context, app: &mut FlutzDesktopApp) {
    egui::TopBottomPanel::bottom("debug_metrics_panel")
        .resizable(true)
        .default_height(32.0)
        .show(context, |ui| {
            egui::CollapsingHeader::new("Debug Metrics")
                .id_salt("debug_metrics_expander")
                .default_open(false)
                .show(ui, |ui| {
                    let metrics = app.debug_metrics();
                    let diagnostics = metrics.audio_diagnostics.as_ref();
                    let stats = diagnostics.map(|diagnostics| diagnostics.stats);
                    egui::Grid::new("debug_metrics_grid")
                        .num_columns(4)
                        .spacing([18.0, 4.0])
                        .striped(true)
                        .show(ui, |ui| {
                            metric(ui, "engine", metrics.engine_state);
                            metric(
                                ui,
                                "time",
                                format!(
                                    "{:.2}/{:.2}s",
                                    metrics.transport_seconds, metrics.transport_duration_seconds
                                ),
                            );
                            ui.end_row();

                            metric(ui, "tick", metrics.transport_tick.to_string());
                            metric(ui, "audio", metrics.audio_status);
                            ui.end_row();

                            metric(ui, "soundfonts", metrics.loaded_soundfont_count.to_string());
                            metric(ui, "midi strips", metrics.midi_strip_count.to_string());
                            ui.end_row();

                            metric(ui, "active strips", metrics.active_strip_count.to_string());
                            metric(
                                ui,
                                "output peak/rms",
                                format!("{:.3}/{:.3}", metrics.output_peak, metrics.output_rms),
                            );
                            ui.end_row();

                            metric(
                                ui,
                                "visualizer bands",
                                metrics.visualizer.band_count.to_string(),
                            );
                            metric(
                                ui,
                                "visualizer dominant",
                                metrics
                                    .visualizer
                                    .dominant_band_index
                                    .map(|index| {
                                        format!(
                                            "{} @ {:.0} Hz ({:.3})",
                                            index,
                                            metrics.visualizer.dominant_center_hz,
                                            metrics.visualizer.dominant_live_level
                                        )
                                    })
                                    .unwrap_or_else(|| "none".to_owned()),
                            );
                            ui.end_row();

                            metric(
                                ui,
                                "visualizer peak square",
                                format!("{:.3}", metrics.visualizer.highest_peak_square_level),
                            );
                            metric(
                                ui,
                                "visualizer peak/rms",
                                format!(
                                    "{:.3}/{:.3}",
                                    metrics.visualizer.aggregate_peak,
                                    metrics.visualizer.aggregate_rms
                                ),
                            );
                            ui.end_row();

                            metric(
                                ui,
                                "requested format",
                                format!(
                                    "{} ch @ {} Hz",
                                    metrics.audio_config.channels, metrics.audio_config.sample_rate
                                ),
                            );
                            metric(
                                ui,
                                "block frames",
                                metrics.audio_config.internal_block_frames.to_string(),
                            );
                            ui.end_row();

                            metric(
                                ui,
                                "ring frames",
                                diagnostics
                                    .map(|diagnostics| {
                                        format!(
                                            "{}/{}",
                                            diagnostics.ring_available_frames,
                                            diagnostics.ring_capacity_frames
                                        )
                                    })
                                    .unwrap_or_else(|| "closed".to_owned()),
                            );
                            metric(
                                ui,
                                "device",
                                diagnostics
                                    .and_then(|diagnostics| diagnostics.opened_device_name.clone())
                                    .unwrap_or_else(|| "none".to_owned()),
                            );
                            ui.end_row();

                            metric(
                                ui,
                                "callbacks",
                                stats
                                    .map(|stats| stats.callback_count.to_string())
                                    .unwrap_or_else(|| "0".to_owned()),
                            );
                            metric(
                                ui,
                                "last/largest frames",
                                stats
                                    .map(|stats| {
                                        format!(
                                            "{}/{}",
                                            stats.last_callback_frames,
                                            stats.largest_callback_frames
                                        )
                                    })
                                    .unwrap_or_else(|| "0/0".to_owned()),
                            );
                            ui.end_row();

                            metric(
                                ui,
                                "delivered/requested",
                                stats
                                    .map(|stats| {
                                        format!(
                                            "{}/{}",
                                            stats.frames_delivered, stats.frames_requested
                                        )
                                    })
                                    .unwrap_or_else(|| "0/0".to_owned()),
                            );
                            metric(
                                ui,
                                "underruns/queue errors",
                                stats
                                    .map(|stats| {
                                        format!(
                                            "{}/{}",
                                            stats.underrun_count, stats.queue_error_count
                                        )
                                    })
                                    .unwrap_or_else(|| "0/0".to_owned()),
                            );
                            ui.end_row();

                            metric(
                                ui,
                                "producer frames",
                                stats
                                    .map(|stats| stats.producer_rendered_frames.to_string())
                                    .unwrap_or_else(|| "0".to_owned()),
                            );
                            metric(
                                ui,
                                "meter latency",
                                format!(
                                    "{} frames / {:.1} ms (queue {}, device {})",
                                    metrics.meter_latency_frames,
                                    metrics.meter_latency_ms,
                                    metrics.meter_wrapper_queue_frames,
                                    metrics.meter_device_queue_frames
                                ),
                            );
                            ui.end_row();

                            metric(
                                ui,
                                "flux target",
                                format!(
                                    "{} ({}-{})",
                                    metrics.flux_guard.decision.target.target_frames,
                                    metrics.flux_guard.decision.target.low_water_frames,
                                    metrics.flux_guard.decision.target.high_water_frames
                                ),
                            );
                            metric(
                                ui,
                                "flux state",
                                format!(
                                    "{} / {}",
                                    metrics.flux_guard.decision.state.current_buffered_frames,
                                    metrics.flux_guard.decision.reason.as_str()
                                ),
                            );
                            ui.end_row();

                            metric(
                                ui,
                                "audio error",
                                metrics.audio_error.unwrap_or_else(|| "none".to_owned()),
                            );
                            ui.end_row();
                        });
                    ui.separator();
                    draw_perf_trace(ui, app);
                });
        });
}

#[cfg(debug_assertions)]
fn draw_perf_trace(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("Performance Trace");
        ui.monospace(app.perf_trace_status());
        if ui.button("Export JSONL").clicked() {
            app.export_perf_trace_log();
        }
        if ui.button("Start Log").clicked() {
            app.start_perf_trace_logging();
        }
        if ui.button("Stop Log").clicked() {
            app.stop_perf_trace_logging();
        }
        if ui.button("Clear").clicked() {
            app.clear_perf_trace();
        }
    });

    let issues = app.perf_trace_issues();
    if !issues.is_empty() {
        ui.label("Detected quality events");
        for record in issues.iter().rev().take(6) {
            ui.monospace(record.summary());
        }
    }

    ui.label("Recent trace events");
    egui::ScrollArea::vertical()
        .id_salt("perf_trace_timeline")
        .max_height(96.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for record in app.perf_trace_records().iter().rev().take(8).rev() {
                ui.monospace(record.summary());
            }
        });
}

#[cfg(debug_assertions)]
fn metric(ui: &mut egui::Ui, label: &str, value: String) {
    ui.strong(label);
    ui.monospace(value);
}

fn draw_missing_preset_warning(context: &egui::Context, app: &mut FlutzDesktopApp) {
    let Some(message) = app.missing_preset_warning().map(str::to_owned) else {
        return;
    };
    egui::Window::new("Missing Preset")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(context, |ui| {
            ui.label(message);
            ui.label("Default preset settings were loaded instead.");
            ui.horizontal(|ui| {
                if ui.button("Dismiss").clicked() {
                    app.dismiss_missing_preset_warning();
                }
            });
        });
}

fn draw_project_header(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    let width = bounded_available_width(ui);
    let has_project = app.has_loaded_project();
    let title = if has_project {
        format!(
            "♪ {}{}",
            app.project_title(),
            if app.dirty() { " ·" } else { "" }
        )
    } else {
        "♪ No file open".to_owned()
    };
    let path = if has_project {
        app.current_path().unwrap_or("No file open").to_owned()
    } else {
        "No file open".to_owned()
    };
    let duration = if has_project {
        app.transport_duration_seconds()
    } else {
        0.0
    };

    ui.allocate_ui_with_layout(
        egui::vec2(width, HEADER_HEIGHT),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_sized([26.0, 24.0], egui::Button::new("♪"))
                    .on_hover_text("Edit metadata")
                    .clicked()
                {
                    app.toggle_metadata_popup();
                }
                ui.add_space(4.0);
                let duration_width = if duration > 0.0 { 72.0 } else { 0.0 };
                let title_width = (width - duration_width - 38.0).max(0.0);
                ui.add_sized(
                    [title_width, 28.0],
                    egui::Label::new(egui::RichText::new(title).size(22.0))
                        .truncate()
                        .halign(egui::Align::Min),
                );
                if duration > 0.0 {
                    ui.add_space(8.0);
                    ui.add_sized(
                        [duration_width, 18.0],
                        egui::Label::new(format!("{} total", format_time(duration))).truncate(),
                    );
                }
            });
            ui.add_sized(
                [width, 18.0],
                egui::Label::new(egui::RichText::new(&path).size(14.0))
                    .truncate()
                    .halign(egui::Align::Min),
            );
        },
    );
}

fn draw_spectrum_visualizer(ui: &mut egui::Ui, app: &FlutzDesktopApp) {
    let height = 72.0;
    let (_, row_rect) = ui.allocate_space(egui::vec2(bounded_available_width(ui), height));
    let clip_rect = ui.clip_rect();
    let rect = egui::Rect::from_min_max(
        egui::pos2(clip_rect.left() + CONTENT_SIDE_PADDING, row_rect.top()),
        egui::pos2(clip_rect.right() - CONTENT_SIDE_PADDING, row_rect.bottom()),
    );
    let frame = app.visualizer_frame();
    paint_visualizer(ui, rect, &frame, &VisualizerRendererConfig::default());
}

fn draw_editor_controls(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    let row_width = ui.available_width();
    let row_height = ui.available_height().max(editor_panel_height(app));
    let (row_rect, _) =
        ui.allocate_exact_size(egui::vec2(row_width, row_height), egui::Sense::hover());
    let gap = 8.0;
    let usable_width = (row_rect.width() - gap).max(0.0);
    let sound_min = 320.0;
    let master_min = 360.0;
    let sound_width = if usable_width > sound_min + master_min {
        (usable_width * 0.44).clamp(sound_min, usable_width - master_min)
    } else {
        (usable_width * 0.44).max(0.0)
    };
    let sound_rect =
        egui::Rect::from_min_size(row_rect.min, egui::vec2(sound_width, row_height - 4.0));
    let master_rect_width = usable_width - gap - sound_width;
    let master_rect = egui::Rect::from_min_size(
        egui::pos2(sound_rect.right() + gap, row_rect.top()),
        egui::vec2(master_rect_width, row_height - 4.0),
    );

    draw_soundfont_manager(ui, app, sound_rect);
    draw_master_panel(ui, app, master_rect);
}

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let minutes = total / 60;
    let seconds = total % 60;
    format!("{minutes}:{seconds:02}")
}

fn draw_transport(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    let width = bounded_available_width(ui);
    let has_project = app.has_loaded_project();
    let total_seconds = app.transport_duration_seconds();
    let total_ticks = app.transport_tick_length();
    let current_seconds = app.transport_seconds();
    let current_tick = app.transport_tick();
    let audio_unavailable = app
        .audio_status()
        .to_ascii_lowercase()
        .contains("unavailable");

    egui::Frame::default()
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            let content_width = (width - 16.0).max(0.0);
            ui.set_width(content_width);
            ui.set_max_width(content_width);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.horizontal(|ui| {
                    draw_transport_button(ui, has_project, "▶ Play", || app.play());
                    draw_transport_button(ui, has_project, "⏸ Pause", || app.pause());
                    draw_transport_button(ui, has_project, "■ Stop", || app.stop());

                    ui.add_space(12.0);
                    let mut loop_enabled = app.loop_enabled_value();
                    let loop_response =
                        ui.add_enabled(has_project, egui::Button::new("⟲").selected(loop_enabled));
                    if loop_response.clicked() {
                        loop_enabled = !loop_enabled;
                        app.set_loop_enabled(loop_enabled);
                    }
                    if loop_enabled {
                        match app.loop_mode_value() {
                            LoopMode::Infinite => {
                                ui.label("∞");
                            }
                            LoopMode::Counted => {
                                ui.label(format!("×{}", app.loop_count_value()));
                            }
                            LoopMode::None => {}
                        }
                    }

                    app.draw_playlist_transport_cluster_hook(ui);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if audio_unavailable {
                            if ui.button("Retry").clicked() {
                                app.retry_audio();
                            }
                            ui.label("⚠ Audio unavailable");
                        }
                    });
                });

                ui.horizontal(|ui| {
                    ui.add_sized(
                        [44.0, 18.0],
                        egui::Label::new(
                            egui::RichText::new(format_time(current_seconds)).monospace(),
                        ),
                    );
                    draw_transport_seek_bar(ui, app, has_project, total_ticks);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_sized(
                            [52.0, 18.0],
                            egui::Label::new(
                                egui::RichText::new(format_time(total_seconds)).monospace(),
                            ),
                        );
                    });
                });

                ui.horizontal(|ui| {
                    ui.add_sized(
                        [44.0, 18.0],
                        egui::Label::new(
                            egui::RichText::new(format!("t:{current_tick}")).monospace(),
                        ),
                    );
                    let line_width = (bounded_available_width(ui) - 76.0).max(16.0);
                    let (line_rect, _) =
                        ui.allocate_exact_size(egui::vec2(line_width, 6.0), egui::Sense::hover());
                    ui.painter().line_segment(
                        [line_rect.left_center(), line_rect.right_center()],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(128, 134, 140)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_sized(
                            [72.0, 18.0],
                            egui::Label::new(
                                egui::RichText::new(format!("t:{total_ticks}")).monospace(),
                            ),
                        );
                    });
                });
            });
        });
}

fn draw_transport_button(ui: &mut egui::Ui, enabled: bool, label: &str, on_click: impl FnOnce()) {
    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
        on_click();
    }
}

fn draw_transport_seek_bar(
    ui: &mut egui::Ui,
    app: &mut FlutzDesktopApp,
    enabled: bool,
    total_ticks: u64,
) {
    let desired_width = (bounded_available_width(ui) - 52.0).max(24.0);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(desired_width, 18.0),
        if enabled {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::hover()
        },
    );

    let rail_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    let painter = ui.painter();
    painter.line_segment(
        [rail_rect.left_center(), rail_rect.right_center()],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(182, 184, 188)),
    );
    painter.line_segment(
        [
            rect.left_center() + egui::vec2(0.0, -8.0),
            rect.left_center() + egui::vec2(0.0, 8.0),
        ],
        egui::Stroke::new(1.5, egui::Color32::from_rgb(182, 184, 188)),
    );
    painter.line_segment(
        [
            rect.right_center() + egui::vec2(0.0, -8.0),
            rect.right_center() + egui::vec2(0.0, 8.0),
        ],
        egui::Stroke::new(1.5, egui::Color32::from_rgb(182, 184, 188)),
    );

    if app.loop_enabled_value() && total_ticks > 0 {
        let start_tick = app.loop_start_tick_value().min(total_ticks);
        let end_tick = app.loop_end_tick_value().min(total_ticks).max(start_tick);
        let start_x = egui::lerp(
            rect.left()..=rect.right(),
            start_tick as f32 / total_ticks as f32,
        );
        let end_x = egui::lerp(
            rect.left()..=rect.right(),
            end_tick as f32 / total_ticks as f32,
        );
        let loop_rect = egui::Rect::from_min_max(
            egui::pos2(start_x, rail_rect.top() - 1.0),
            egui::pos2(end_x, rail_rect.bottom() + 1.0),
        );
        painter.rect_filled(
            loop_rect,
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 170, 190, 72),
        );
    }

    let fraction = (*app.transport_position()).clamp(0.0, 1.0);
    let handle_x = egui::lerp(rect.left()..=rect.right(), fraction);
    painter.circle_filled(
        egui::pos2(handle_x, rect.center().y),
        5.0,
        egui::Color32::from_rgb(244, 244, 244),
    );
    painter.circle_stroke(
        egui::pos2(handle_x, rect.center().y),
        5.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 54, 60)),
    );

    if enabled && (response.dragged() || response.clicked()) {
        if let Some(pointer_pos) = response.interact_pointer_pos() {
            let new_fraction = ((pointer_pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            app.seek_transport_fraction(new_fraction);
        }
    }
}

fn draw_panel_shell(ui: &mut egui::Ui, rect: egui::Rect) -> egui::Rect {
    ui.painter().rect_filled(rect, 4.0, RELEASE_PANEL_FILL);
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, RELEASE_PANEL_STROKE),
        egui::StrokeKind::Inside,
    );
    rect.shrink2(egui::vec2(14.0, 12.0))
}

fn draw_soundfont_manager(ui: &mut egui::Ui, app: &mut FlutzDesktopApp, panel_rect: egui::Rect) {
    let content_rect = draw_panel_shell(ui, panel_rect);
    let mut panel_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    panel_ui.set_width(content_rect.width());
    panel_ui.set_min_height(content_rect.height());
    let collapsed = app.release_editor_panels_collapsed();
    panel_ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Render").size(16.0));
        if ui
            .add_sized(
                [22.0, 20.0],
                egui::Button::new(if collapsed { "▶" } else { "▼" }),
            )
            .clicked()
        {
            app.toggle_release_editor_panels_collapsed();
        }
    });
    if collapsed {
        return;
    }
    panel_ui.add_space(6.0);

    panel_ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_sized([64.0, 22.0], egui::Label::new("Preset"));
        let apply_width = 92.0;
        let combo_width = (ui.available_width() - apply_width - 8.0).max(96.0);
        let mut selected_preset = app.selected_preset_id().to_owned();
        egui::ComboBox::from_id_salt("preset_picker")
            .width(combo_width)
            .selected_text(
                app.preset_set()
                    .find_preset(&selected_preset)
                    .map(|preset| preset.display_name)
                    .unwrap_or("Unknown preset"),
            )
            .show_ui(ui, |ui| {
                for preset in app.preset_set().presets {
                    ui.selectable_value(
                        &mut selected_preset,
                        preset.id.to_owned(),
                        preset.display_name,
                    );
                }
            });
        app.set_selected_preset_id(selected_preset);
        if ui
            .add_sized([apply_width, 22.0], egui::Button::new("Apply Preset"))
            .clicked()
        {
            app.apply_selected_preset();
        }
    });

    panel_ui.horizontal(|ui| {
        ui.add_space(72.0);
        ui.add(egui::Label::new(format!("Active: {}", app.active_preset_label())).truncate());
    });

    panel_ui.add_space(12.0);
    let catalog_entries = app.catalog_soundfonts().to_vec();
    let mut selected_index = app
        .selected_soundfont_index()
        .min(catalog_entries.len().saturating_sub(1));
    let selected_name = catalog_entries
        .get(selected_index)
        .map(|entry| entry.display_name.clone())
        .unwrap_or_else(|| "None".to_owned());

    panel_ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_sized([64.0, 22.0], egui::Label::new("Synth"));
        let add_width = 52.0;
        let remove_width = 28.0;
        let available_width = ui.available_width();
        let reserved_width = add_width + remove_width + 16.0;
        let combo_width = (available_width * 0.5)
            .min((available_width - reserved_width).max(96.0))
            .max(96.0);
        egui::ComboBox::from_id_salt("soundfont_picker")
            .width(combo_width)
            .selected_text(selected_name)
            .show_ui(ui, |ui| {
                for (index, entry) in catalog_entries.iter().enumerate() {
                    ui.selectable_value(&mut selected_index, index, entry.display_name.clone());
                }
            });

        if ui
            .add_sized([add_width, 22.0], egui::Button::new("+ Add"))
            .clicked()
        {
            app.add_selected_soundfont();
        }
        if ui
            .add_sized([remove_width, 22.0], egui::Button::new("×"))
            .clicked()
        {
            app.remove_selected_soundfont();
        }
    });

    app.set_selected_soundfont_index(selected_index);

    panel_ui.add_space(12.0);

    panel_ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_sized([64.0, 20.0], egui::Label::new("Loop"));
        let mut loop_mode = app.loop_mode_value();
        if ui
            .radio_value(&mut loop_mode, LoopMode::Infinite, "Inf")
            .changed()
        {
            app.set_loop_mode(loop_mode);
        }
        if ui
            .radio_value(&mut loop_mode, LoopMode::Counted, "Count")
            .changed()
        {
            app.set_loop_mode(loop_mode);
        }
        let mut loop_count = app.loop_count_value();
        ui.add_enabled_ui(loop_mode == LoopMode::Counted, |ui| {
            if ui
                .add(
                    egui::DragValue::new(&mut loop_count)
                        .range(1..=999)
                        .speed(1.0)
                        .prefix("×"),
                )
                .changed()
            {
                app.set_loop_count(loop_count);
            }
        });
        if ui
            .radio_value(&mut loop_mode, LoopMode::None, "Off")
            .changed()
        {
            app.set_loop_mode(loop_mode);
        }
    });

    panel_ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(72.0);
        ui.label("Start");
        let mut loop_start_tick = app.loop_start_tick_value();
        if ui
            .add(
                egui::DragValue::new(&mut loop_start_tick)
                    .speed(1.0)
                    .prefix("t:"),
            )
            .changed()
        {
            app.set_loop_start_tick(loop_start_tick);
        }
        ui.label("End");
        let mut loop_end_tick = app.loop_end_tick_value();
        if ui
            .add(
                egui::DragValue::new(&mut loop_end_tick)
                    .speed(1.0)
                    .prefix("t:"),
            )
            .changed()
        {
            app.set_loop_end_tick(loop_end_tick);
        }
    });
}

fn draw_master_panel(ui: &mut egui::Ui, app: &mut FlutzDesktopApp, panel_rect: egui::Rect) {
    let content_rect = draw_panel_shell(ui, panel_rect);
    let mut panel_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    panel_ui.set_width(content_rect.width());
    panel_ui.set_min_height(content_rect.height());
    panel_ui.spacing_mut().item_spacing.y = 3.0;

    let collapsed = app.release_editor_panels_collapsed();
    panel_ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Master").size(16.0));
        if ui
            .add_sized(
                [22.0, 20.0],
                egui::Button::new(if collapsed { "▶" } else { "▼" }),
            )
            .clicked()
        {
            app.toggle_release_editor_panels_collapsed();
        }
    });
    if collapsed {
        return;
    }
    panel_ui.add_space(4.0);

    let master = app.master();
    draw_master_slider_row(
        &mut panel_ui,
        "Volume",
        &mut master.volume_db,
        -60.0..=MASTER_VOLUME_MAX_DB,
        " dB",
    );
    draw_master_slider_row(
        &mut panel_ui,
        "Reverb",
        &mut master.reverb,
        0.0..=100.0,
        " %",
    );
    draw_master_slider_row(
        &mut panel_ui,
        "Chorus",
        &mut master.chorus,
        0.0..=100.0,
        " %",
    );
    draw_master_slider_row(
        &mut panel_ui,
        "EQ Low",
        &mut master.eq_low,
        -24.0..=24.0,
        " dB",
    );
    draw_master_slider_row(
        &mut panel_ui,
        "EQ Mid",
        &mut master.eq_mid,
        -24.0..=24.0,
        " dB",
    );
    draw_master_slider_row(
        &mut panel_ui,
        "EQ High",
        &mut master.eq_high,
        -24.0..=24.0,
        " dB",
    );
    panel_ui.add_space(8.0);
    draw_master_limiter_row(&mut panel_ui, master);
}

fn draw_master_slider_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) {
    let row_width = ui.available_width();
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(row_width, 22.0), egui::Sense::hover());
    let label_rect = egui::Rect::from_min_size(
        row_rect.left_top() + egui::vec2(4.0, 1.0),
        egui::vec2(86.0, 20.0),
    );
    let value_rect = egui::Rect::from_min_size(
        egui::pos2(row_rect.right() - 96.0, row_rect.top() + 1.0),
        egui::vec2(96.0, 20.0),
    );
    let slider_rect = egui::Rect::from_min_max(
        egui::pos2(label_rect.right() + 18.0, row_rect.top() + 2.0),
        egui::pos2(value_rect.left() - 18.0, row_rect.bottom() - 2.0),
    );

    ui.put(label_rect, egui::Label::new(label));
    if slider_rect.width() >= 64.0 {
        let mut slider_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(slider_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        slider_ui.spacing_mut().slider_width = slider_rect.width();
        slider_ui.add_sized(
            slider_rect.size(),
            egui::Slider::new(value, range.clone()).show_value(false),
        );
    }
    ui.put(
        value_rect,
        egui::DragValue::new(value)
            .range(range)
            .speed(0.25)
            .suffix(suffix),
    );
}

fn draw_master_limiter_row(ui: &mut egui::Ui, master: &mut crate::app::MasterControls) {
    let row_width = ui.available_width();
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(row_width, 24.0), egui::Sense::hover());
    let checkbox_rect = egui::Rect::from_min_size(
        row_rect.left_top() + egui::vec2(4.0, 2.0),
        egui::vec2(22.0, 20.0),
    );
    let limiter_rect = egui::Rect::from_min_size(
        egui::pos2(checkbox_rect.right() + 8.0, row_rect.top() + 2.0),
        egui::vec2(70.0, 20.0),
    );
    let amount_rect = egui::Rect::from_min_size(
        egui::pos2(limiter_rect.right() + 12.0, row_rect.top() + 2.0),
        egui::vec2(70.0, 20.0),
    );
    let value_rect = egui::Rect::from_min_size(
        egui::pos2(row_rect.right() - 74.0, row_rect.top() + 2.0),
        egui::vec2(74.0, 20.0),
    );
    let slider_rect = egui::Rect::from_min_max(
        egui::pos2(amount_rect.right() + 16.0, row_rect.top() + 3.0),
        egui::pos2(value_rect.left() - 18.0, row_rect.bottom() - 3.0),
    );

    ui.put(
        checkbox_rect,
        egui::Checkbox::new(&mut master.limiter_enabled, ""),
    );
    ui.put(limiter_rect, egui::Label::new("Limiter"));
    ui.put(amount_rect, egui::Label::new("Amount"));
    ui.add_enabled_ui(master.limiter_enabled, |ui| {
        if slider_rect.width() >= 64.0 {
            let mut slider_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(slider_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            slider_ui.spacing_mut().slider_width = slider_rect.width();
            slider_ui.add_sized(
                slider_rect.size(),
                egui::Slider::new(&mut master.limiter_amount, 0.0..=0.95).show_value(false),
            );
        }
        ui.put(
            value_rect,
            egui::DragValue::new(&mut master.limiter_amount).speed(0.01),
        );
    });
}

fn draw_mixer(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    let fx_expanded = app.mixer_fx_expanded();
    let mut toggle_fx_expansion = false;

    egui::Frame::default()
        .fill(RELEASE_PANEL_FILL)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());

            ui.horizontal(|ui| {
                ui.heading("Mixer");
                if ui
                    .add_sized(
                        [22.0, 20.0],
                        egui::Button::new(if app.all_mixer_rows_collapsed() {
                            "▶"
                        } else {
                            "▼"
                        }),
                    )
                    .clicked()
                {
                    app.toggle_all_mixer_rows_collapsed();
                }
                ui.separator();
                if ui.button("CS").clicked() {
                    clear_all_solos(app);
                }
                if ui.button("CM").clicked() {
                    clear_all_mutes(app);
                }
                if ui
                    .add(
                        egui::Button::new("BAL")
                            .selected(app.mixer_assignment_mode() == MixerAssignmentMode::Balance),
                    )
                    .clicked()
                {
                    app.apply_balanced_mixer_assignment();
                }
                if ui
                    .add(
                        egui::Button::new("LAY")
                            .selected(app.mixer_assignment_mode() == MixerAssignmentMode::Layer),
                    )
                    .clicked()
                {
                    app.apply_layered_mixer_assignment();
                }
                ui.separator();
                ui.label(app.playback_summary());
            });
            ui.separator();

            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for font in app.soundfonts() {
                        toggle_fx_expansion |= draw_soundfont_row(ui, font, fx_expanded);
                        ui.add_space(8.0);
                    }
                });
        });

    if toggle_fx_expansion {
        app.set_mixer_fx_expanded(!fx_expanded);
    }
}

fn clear_all_solos(app: &mut FlutzDesktopApp) {
    for font in app.soundfonts() {
        font.soloed = false;
        for strip in &mut font.strips {
            strip.soloed = false;
        }
    }
}

fn clear_all_mutes(app: &mut FlutzDesktopApp) {
    for font in app.soundfonts() {
        font.muted = false;
        for strip in &mut font.strips {
            strip.muted = false;
        }
    }
}

fn draw_soundfont_row(ui: &mut egui::Ui, font: &mut SoundFontUiRow, fx_expanded: bool) -> bool {
    let mut toggle_fx_expansion = false;
    ui.horizontal(|ui| {
        let collapse_label = if font.collapsed { "+" } else { "-" };
        if ui.button(collapse_label).clicked() {
            font.collapsed = !font.collapsed;
        }
        ui.heading(&font.display_name);
        ui.checkbox(&mut font.muted, "Mute row");
        ui.checkbox(&mut font.soloed, "Solo row");
        let fx_label = if fx_expanded { "FX v" } else { "FX >" };
        if ui.button(fx_label).clicked() {
            toggle_fx_expansion = true;
        }
    });

    if font.collapsed {
        return toggle_fx_expansion;
    }

    ui.horizontal_top(|ui| {
        for strip in &mut font.strips {
            toggle_fx_expansion |= draw_strip(ui, strip, fx_expanded);
        }
    });

    toggle_fx_expansion
}

fn draw_strip(ui: &mut egui::Ui, strip: &mut MixerStripUiState, fx_expanded: bool) -> bool {
    const STRIP_WIDTH: f32 = 76.0;
    const CONTENT_WIDTH: f32 = 68.0;
    const HEADER_ROW_HEIGHT: f32 = 16.0;
    const ID_ROW_HEIGHT: f32 = 18.0;
    const LABEL_BLOCK_HEIGHT: f32 = HEADER_ROW_HEIGHT + ID_ROW_HEIGHT;
    const CONTROL_HEIGHT: f32 = 128.0;
    const BUTTON_ROW_HEIGHT: f32 = 20.0;
    const COLLAPSED_HEIGHT: f32 = 205.0;
    const EXPANDED_HEIGHT: f32 = 595.0;

    let mut toggle_fx_expansion = false;
    let strip_height = if fx_expanded {
        EXPANDED_HEIGHT
    } else {
        COLLAPSED_HEIGHT
    };
    let (strip_rect, _) =
        ui.allocate_exact_size(egui::vec2(STRIP_WIDTH, strip_height), egui::Sense::hover());

    let mut strip_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(strip_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    egui::Frame::group(strip_ui.style())
        .inner_margin(egui::Margin::same(4))
        .show(&mut strip_ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.set_width(CONTENT_WIDTH);
                ui.set_max_width(CONTENT_WIDTH);

                fixed_centered_small(
                    ui,
                    CONTENT_WIDTH,
                    LABEL_BLOCK_HEIGHT,
                    &strip_display_label(strip),
                );
                ui.add_space(4.0);

                let meter_color = if strip.active {
                    egui::Color32::from_rgb(80, 210, 120)
                } else {
                    egui::Color32::from_rgb(14, 16, 18)
                };
                draw_volume_lane(ui, strip, meter_color, CONTENT_WIDTH, CONTROL_HEIGHT);
                ui.add_space(4.0);

                let fx_label = if fx_expanded { "FX v" } else { "FX >" };
                if draw_mute_solo_fx_row(ui, strip, CONTENT_WIDTH, BUTTON_ROW_HEIGHT, fx_label) {
                    toggle_fx_expansion = true;
                }

                if fx_expanded {
                    ui.add_space(6.0);
                    fixed_centered_checkbox(
                        ui,
                        CONTENT_WIDTH,
                        20.0,
                        &mut strip.limiter_enabled,
                        "Limiter",
                    );
                    ui.add_space(4.0);
                    draw_fx_slider_grid(ui, strip, CONTENT_WIDTH);
                }
            });
        });
    toggle_fx_expansion
}

fn strip_display_label(strip: &MixerStripUiState) -> String {
    let program_prefix = if strip.is_percussion { 'P' } else { 'I' };
    format!("Ch {}, {}-{}", strip.channel, program_prefix, strip.program)
}

fn draw_volume_lane(
    ui: &mut egui::Ui,
    strip: &mut MixerStripUiState,
    meter_color: egui::Color32,
    width: f32,
    height: f32,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, height), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 3.0, egui::Color32::from_rgb(12, 13, 15));
            let filled = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - rect.height() * strip.meter),
                rect.right_bottom(),
            );
            ui.painter().rect_filled(filled, 3.0, meter_color);

            ui.add_space(4.0);
            ui.allocate_ui_with_layout(
                egui::vec2(24.0, height),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add_sized(
                        [10.0, height - 16.0],
                        egui::Slider::new(&mut strip.volume, 0.0..=2.5)
                            .vertical()
                            .show_value(false),
                    );
                    fixed_value_text(ui, 24.0, 16.0, &signed_value_text(strip.volume, 2));
                },
            );
        },
    );
}

fn draw_mute_solo_fx_row(
    ui: &mut egui::Ui,
    strip: &mut MixerStripUiState,
    width: f32,
    height: f32,
    fx_label: &str,
) -> bool {
    let mut fx_clicked = false;
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(5.0);
            let mute_response =
                ui.add_sized([16.0, 18.0], egui::Button::new("M").selected(strip.muted));
            if mute_response.clicked() {
                strip.muted = !strip.muted;
            }
            let solo_response = ui
                .add_enabled_ui(!strip.unsupported, |ui| {
                    ui.add_sized([16.0, 18.0], egui::Button::new("S").selected(strip.soloed))
                })
                .inner;
            if solo_response.clicked() {
                strip.soloed = !strip.soloed;
            }
            if ui
                .add_sized([26.0, 18.0], egui::Button::new(fx_label))
                .clicked()
            {
                fx_clicked = true;
            }
        },
    );
    fx_clicked
}

fn fixed_centered_checkbox(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    value: &mut bool,
    text: &str,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.checkbox(value, text);
        },
    );
}

fn fixed_centered_small(ui: &mut egui::Ui, width: f32, height: f32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(9.0),
        ui.visuals().weak_text_color(),
    );
}

fn fixed_value_text(ui: &mut egui::Ui, width: f32, height: f32, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.label(egui::RichText::new(text).size(9.0).monospace());
        },
    );
}

fn draw_fx_slider_grid(ui: &mut egui::Ui, strip: &mut MixerStripUiState, width: f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 345.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            draw_fx_slider_pair(
                ui,
                (&mut strip.limiter_amount, 0.0..=1.0, "Lmt"),
                Some((&mut strip.gain_db, -24.0..=24.0, "db")),
            );
            draw_horizontal_pan_slider(ui, &mut strip.pan);
            draw_fx_slider_pair(
                ui,
                (&mut strip.reverb, 0.0..=100.0, "Rev"),
                Some((&mut strip.chorus, 0.0..=100.0, "Chor")),
            );
        },
    );
}

fn draw_horizontal_pan_slider(ui: &mut egui::Ui, value: &mut f32) {
    ui.allocate_ui_with_layout(
        egui::vec2(58.0, 114.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(34.0);
            fixed_value_text(ui, 58.0, 14.0, "Pan");

            let slider_width = 56.0;
            ui.spacing_mut().slider_width = slider_width;
            ui.add_sized(
                [slider_width, 24.0],
                egui::Slider::new(value, -1.0..=1.0).show_value(false),
            );

            fixed_value_text(ui, 58.0, 14.0, &signed_value_text(*value, 2));
        },
    );
}

fn draw_fx_slider_pair(
    ui: &mut egui::Ui,
    left: (&mut f32, std::ops::RangeInclusive<f32>, &str),
    right: Option<(&mut f32, std::ops::RangeInclusive<f32>, &str)>,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(58.0, 114.0),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            draw_vertical_fx_slider(ui, left.0, left.1, left.2);
            if let Some(right) = right {
                ui.add_space(6.0);
                draw_vertical_fx_slider(ui, right.0, right.1, right.2);
            }
        },
    );
}

fn draw_vertical_fx_slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(24.0, 112.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_sized(
                [10.0, 76.0],
                egui::Slider::new(value, range).vertical().show_value(false),
            );
            fixed_value_text(ui, 24.0, 14.0, label);
            fixed_value_text(ui, 24.0, 14.0, &slider_value_text(label, *value));
        },
    );
}

fn slider_value_text(label: &str, value: f32) -> String {
    match label {
        "db" | "Lmt" | "Rev" | "Chor" => signed_value_text(value, 0),
        _ => signed_value_text(value, 1),
    }
}

fn signed_value_text(value: f32, decimals: usize) -> String {
    let zero_threshold = match decimals {
        0 => 0.5,
        1 => 0.05,
        2 => 0.005,
        _ => 0.05,
    };
    let value = if value.abs() < zero_threshold {
        0.0
    } else {
        value
    };
    match decimals {
        0 => format!("{value:+.0}"),
        1 => format!("{value:+.1}"),
        2 => format!("{value:+.2}"),
        _ => format!("{value:+.1}"),
    }
}
