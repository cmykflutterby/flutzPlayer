use super::*;

pub(super) fn draw_status_bar(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
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

    if app.preferences_status_active() {
        let audio_unavailable = app
            .audio_status()
            .to_ascii_lowercase()
            .contains("unavailable");
        let retry_space = if audio_unavailable {
            RETRY_WIDTH + GAP
        } else {
            0.0
        };
        let status_rect = egui::Rect::from_min_max(
            egui::pos2(row_rect.left() + RIGHT_PAD, row_rect.top() + SLOT_TOP_PAD),
            egui::pos2(
                (row_rect.right() - RIGHT_PAD - retry_space).max(row_rect.left() + RIGHT_PAD),
                row_rect.top() + SLOT_TOP_PAD + SLOT_HEIGHT,
            ),
        );
        paint_status_slot(ui, status_rect, app.status().to_owned(), false);
        if audio_unavailable {
            let retry_rect = egui::Rect::from_min_size(
                egui::pos2(
                    row_rect.right() - RIGHT_PAD - RETRY_WIDTH,
                    row_rect.top() + 3.0,
                ),
                egui::vec2(RETRY_WIDTH, 22.0),
            );
            if ui
                .put(retry_rect, egui::Button::new("Retry Audio"))
                .clicked()
            {
                app.retry_audio();
            }
        }
        return;
    }

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

pub(super) fn paint_status_slot(ui: &mut egui::Ui, rect: egui::Rect, text: String, strong: bool) {
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

pub(super) fn elide_text_to_width(
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

pub(super) fn draw_status_separator(
    ui: &egui::Ui,
    x: f32,
    top: f32,
    bottom: f32,
    color: egui::Color32,
) {
    ui.painter().line_segment(
        [egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(1.0, color),
    );
}

pub(super) fn draw_top_action_bar(context: &egui::Context, app: &mut FlutzDesktopApp) {
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
pub(super) fn draw_debug_metrics_panel(context: &egui::Context, app: &mut FlutzDesktopApp) {
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
pub(super) fn draw_perf_trace(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
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
pub(super) fn metric(ui: &mut egui::Ui, label: &str, value: String) {
    ui.strong(label);
    ui.monospace(value);
}

pub(super) fn draw_missing_preset_warning(context: &egui::Context, app: &mut FlutzDesktopApp) {
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
