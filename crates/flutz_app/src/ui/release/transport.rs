use super::*;

pub(super) fn draw_project_header(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
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

pub(super) fn draw_spectrum_visualizer(ui: &mut egui::Ui, app: &FlutzDesktopApp) {
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

pub(super) fn draw_editor_controls(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    if app.active_mastering_capability() == MasteringCapability::DecodedAudioPeq {
        draw_decoded_audio_editor_controls(ui, app);
        return;
    }

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

pub(super) fn draw_decoded_audio_editor_controls(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    let row_width = ui.available_width();
    let row_height = ui.available_height();
    let (row_rect, _) =
        ui.allocate_exact_size(egui::vec2(row_width, row_height), egui::Sense::hover());
    let gap = DECODED_EDITOR_PANEL_GAP;
    let loop_rect = egui::Rect::from_min_size(
        row_rect.min,
        egui::vec2(row_width, DECODED_LOOP_PANEL_HEIGHT),
    );
    let peq_rect = egui::Rect::from_min_max(
        egui::pos2(row_rect.left(), loop_rect.bottom() + gap),
        row_rect.right_bottom(),
    );

    draw_decoded_loop_panel(ui, app, loop_rect);
    draw_decoded_peq_editor(ui, app, peq_rect);
}

pub(super) fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let minutes = total / 60;
    let seconds = total % 60;
    format!("{minutes}:{seconds:02}")
}

pub(super) fn draw_transport(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    let width = bounded_available_width(ui);
    let has_project = app.has_loaded_project();
    let total_seconds = app.transport_duration_seconds();
    let total_ticks = app.transport_tick_length();
    let current_seconds = app.transport_seconds();
    let current_tick = app.transport_tick();
    let unit_prefix = app.transport_unit_prefix();
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
                            egui::RichText::new(format!("{unit_prefix}{current_tick}")).monospace(),
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
                                egui::RichText::new(format!("{unit_prefix}{total_ticks}"))
                                    .monospace(),
                            ),
                        );
                    });
                });
            });
        });
}

pub(super) fn draw_transport_button(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    on_click: impl FnOnce(),
) {
    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
        on_click();
    }
}

pub(super) fn draw_transport_seek_bar(
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
