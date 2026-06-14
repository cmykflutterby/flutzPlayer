use eframe::egui;

use crate::app::{
    AppRunState, FlutzDesktopApp, LoopMode, MixerStripUiState, SoundFontUiRow, MASTER_VOLUME_MAX_DB,
};
use crate::ui::apply_dracula_theme;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct UiRefreshPolicy {
    pub snapshots_per_second: u16,
}

pub fn apply_theme(context: &egui::Context) {
    apply_dracula_theme(context);
}

pub fn draw_app(context: &egui::Context, app: &mut FlutzDesktopApp) {
    draw_menu_bar(context, app);
    draw_missing_preset_warning(context, app);

    egui::TopBottomPanel::top("transport_panel")
        .resizable(false)
        .show(context, |ui| {
            draw_project_header(ui, app);
            ui.add_space(6.0);
            draw_transport(ui, app);
            ui.add_space(6.0);
            draw_soundfont_manager(ui, app);
            ui.add_space(6.0);
            draw_global_controls(ui, app);
        });

    #[cfg(debug_assertions)]
    draw_debug_metrics_panel(context, app);

    egui::TopBottomPanel::bottom("status_bar")
        .resizable(false)
        .exact_height(28.0)
        .show(context, |ui| {
            ui.horizontal(|ui| {
                ui.label(app.status());
                ui.separator();
                ui.label(app.data_summary());
                ui.separator();
                ui.label(match app.run_state() {
                    AppRunState::Idle => "idle",
                    AppRunState::Playing => "playing",
                    AppRunState::Paused => "paused",
                });
            });
        });

    egui::CentralPanel::default().show(context, |ui| {
        draw_mixer(ui, app);
    });

    app.draw_playlist_viewport_host(context);
    app.draw_metadata_viewport_host(context);
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
                            metric(ui, "", String::new());
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
                if ui.button("View Diagnostics").clicked() {
                    app.set_status(
                        "Run flutz_soundfont_tools --diagnose-fmid --input <project.fmid> for CLI diagnostics",
                    );
                }
                if ui.button("Dismiss").clicked() {
                    app.dismiss_missing_preset_warning();
                }
            });
        });
}

fn draw_menu_bar(context: &egui::Context, app: &mut FlutzDesktopApp) {
    egui::TopBottomPanel::top("menu_bar")
        .resizable(false)
        .exact_height(30.0)
        .show(context, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open MIDI").clicked() {
                        app.open_midi_dialog();
                        ui.close();
                    }
                    if ui.button("Save").clicked() {
                        app.save_project();
                        ui.close();
                    }
                    if ui.button("Save As").clicked() {
                        app.save_project_as();
                        ui.close();
                    }
                });
            });
        });
}

fn draw_project_header(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    ui.horizontal(|ui| {
        if ui.button("♪").on_hover_text("Edit metadata").clicked() {
            app.toggle_metadata_popup();
        }
        ui.heading(app.project_title());
        if app.dirty() {
            ui.label("modified");
        }
        ui.separator();
        ui.label(app.current_path().unwrap_or("No file open"));
        ui.separator();
        ui.label(format!("Data: {}", app.data_dir().display()));
    });
}

fn draw_transport(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    ui.horizontal(|ui| {
        if ui.button("Play").clicked() {
            app.play();
        }
        if ui.button("Pause").clicked() {
            app.pause();
        }
        if ui.button("Stop").clicked() {
            app.stop();
        }
        if ui.button("Retry Audio").clicked() {
            app.retry_audio();
        }
        ui.add_space(8.0);
        ui.label("Seek");
        let mut seek_fraction = *app.transport_position();
        let seek_response = ui.add_sized(
            [420.0, 20.0],
            egui::Slider::new(&mut seek_fraction, 0.0..=1.0).show_value(false),
        );
        if seek_response.changed() {
            app.seek_transport_fraction(seek_fraction);
        }
        ui.label(format!("{:>5.1}%", *app.transport_position() * 100.0));
        app.draw_playlist_transport_cluster_hook(ui);
    });
}

fn draw_soundfont_manager(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("Preset: {}", app.active_preset_label()));
        let mut selected_preset = app.selected_preset_id().to_owned();
        egui::ComboBox::from_id_salt("preset_picker")
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
        if ui.button("Apply Preset").clicked() {
            app.apply_selected_preset();
        }
        ui.separator();
        ui.label(format!("Order: {}", app.loaded_preset_font_order()));
        ui.separator();

        ui.label("SoundFonts Catalog:");
        ui.label(format!("{} available", app.catalog_soundfonts().len()));

        let catalog_entries = app.catalog_soundfonts().to_vec();
        let mut selected_index = app
            .selected_soundfont_index()
            .min(catalog_entries.len().saturating_sub(1));
        let selected_name = catalog_entries
            .get(selected_index)
            .map(|entry| entry.display_name.clone())
            .unwrap_or_else(|| "None".to_owned());

        egui::ComboBox::from_id_salt("soundfont_picker")
            .selected_text(selected_name)
            .show_ui(ui, |ui| {
                for (index, entry) in catalog_entries.iter().enumerate() {
                    let label = if entry.is_default {
                        format!("{} [default]", entry.display_name)
                    } else {
                        entry.display_name.clone()
                    };
                    ui.selectable_value(&mut selected_index, index, label);
                }
            });

        app.set_selected_soundfont_index(selected_index);

        if ui.button("Add").clicked() {
            app.add_selected_soundfont();
        }

        if ui.button("Remove").clicked() {
            app.remove_selected_soundfont();
        }

        let loaded_names = app
            .soundfonts()
            .iter()
            .map(|row| row.display_name.clone())
            .collect::<Vec<_>>();
        ui.label(format!("Loaded: {}", loaded_names.join(" | ")));

        if ui.button("Clear Solos").clicked() {
            for font in app.soundfonts() {
                font.soloed = false;
                for strip in &mut font.strips {
                    strip.soloed = false;
                }
            }
        }
    });
}

fn draw_global_controls(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    ui.vertical(|ui| {
        ui.group(|ui| {
            ui.set_min_width(420.0);
            ui.label("Loop");
            ui.horizontal(|ui| {
                let mut loop_enabled = app.loop_enabled_value();
                let loop_response = ui.checkbox(&mut loop_enabled, "Enabled");
                if loop_response.changed() {
                    app.set_loop_enabled(loop_enabled);
                }
                let mut loop_mode = app.loop_mode_value();
                for mode in [LoopMode::None, LoopMode::Infinite, LoopMode::Counted] {
                    if ui.radio_value(&mut loop_mode, mode, mode.label()).changed() {
                        app.set_loop_mode(loop_mode);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Start");
                let mut loop_start_tick = app.loop_start_tick_value();
                if ui
                    .add(egui::DragValue::new(&mut loop_start_tick).speed(16.0))
                    .changed()
                {
                    app.set_loop_start_tick(loop_start_tick);
                }
                ui.label("End");
                let mut loop_end_tick = app.loop_end_tick_value();
                if ui
                    .add(egui::DragValue::new(&mut loop_end_tick).speed(16.0))
                    .changed()
                {
                    app.set_loop_end_tick(loop_end_tick);
                }
                ui.label("Count");
                let mut loop_count = app.loop_count_value();
                if ui
                    .add(egui::DragValue::new(&mut loop_count).range(1..=999))
                    .changed()
                {
                    app.set_loop_count(loop_count);
                }
            });
        });

        ui.group(|ui| {
            ui.set_min_width(420.0);
            ui.label("Smart Mix");
            let smart_mix = app.smart_mix();
            ui.horizontal(|ui| {
                ui.checkbox(&mut smart_mix.enabled, "Enabled");
                ui.checkbox(&mut smart_mix.auto_normalize, "Auto normalize");
            });
            ui.add(
                egui::Slider::new(&mut smart_mix.target_headroom_db, -24.0..=0.0)
                    .text("Headroom dB"),
            );
            ui.add(
                egui::Slider::new(&mut smart_mix.normalization_amount, 0.0..=100.0)
                    .text("Normalize amt"),
            );
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut smart_mix.attack_ms, 1.0..=2000.0).text("Attack"));
                ui.add(egui::Slider::new(&mut smart_mix.release_ms, 10.0..=2000.0).text("Release"));
                ui.add(
                    egui::Slider::new(&mut smart_mix.lookahead_ms, 100.0..=2000.0)
                        .text("Lookahead"),
                );
            });
        });

        ui.group(|ui| {
            ui.set_min_width(420.0);
            ui.label("Master");
            let master = app.master();
            ui.horizontal(|ui| {
                ui.add(
                    egui::Slider::new(&mut master.volume_db, -60.0..=MASTER_VOLUME_MAX_DB)
                        .text("Volume dB"),
                );
                ui.checkbox(&mut master.limiter_enabled, "Limiter");
                ui.add(egui::Slider::new(&mut master.limiter_amount, 0.0..=0.95).text("Limit amt"));
            });
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut master.reverb, 0.0..=100.0).text("Reverb"));
                ui.add(egui::Slider::new(&mut master.chorus, 0.0..=100.0).text("Chorus"));
            });
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut master.eq_low, -24.0..=24.0).text("Low"));
                ui.add(egui::Slider::new(&mut master.eq_mid, -24.0..=24.0).text("Mid"));
                ui.add(egui::Slider::new(&mut master.eq_high, -24.0..=24.0).text("High"));
            });
        });
    });
}

fn draw_mixer(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    ui.horizontal(|ui| {
        ui.heading("Mixer");
        ui.label(app.playback_summary());
        ui.separator();
        ui.label(app.audio_status());
    });
    ui.separator();

    let fx_expanded = app.mixer_fx_expanded();
    let mut toggle_fx_expansion = false;

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for font in app.soundfonts() {
                toggle_fx_expansion |= draw_soundfont_row(ui, font, fx_expanded);
                ui.add_space(8.0);
            }
        });

    if toggle_fx_expansion {
        app.set_mixer_fx_expanded(!fx_expanded);
    }
}

fn draw_soundfont_row(ui: &mut egui::Ui, font: &mut SoundFontUiRow, fx_expanded: bool) -> bool {
    let mut toggle_fx_expansion = false;
    ui.horizontal(|ui| {
        let collapse_label = if font.collapsed { "+" } else { "-" };
        if ui.button(collapse_label).clicked() {
            font.collapsed = !font.collapsed;
        }
        ui.heading(format!(
            "{}{}",
            font.display_name,
            if font.is_default { " [default]" } else { "" }
        ));
        ui.label(&font.internal_id);
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
                    HEADER_ROW_HEIGHT,
                    &format!("Ch {}", strip.channel),
                );
                let strip_label = if strip.unsupported {
                    "Unsupported".to_owned()
                } else {
                    format!("{} P{}", strip.program_name, strip.program)
                };
                fixed_centered_small(ui, CONTENT_WIDTH, ID_ROW_HEIGHT, &strip_label);
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
                    fixed_value_text(ui, 24.0, 16.0, &format!("{:.2}", strip.volume));
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
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.small(text);
        },
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
                (&mut strip.pan, -1.0..=1.0, "Pan"),
                Some((&mut strip.gain_db, -24.0..=24.0, "Gain")),
            );
            draw_fx_slider_pair(ui, (&mut strip.limiter_amount, 0.0..=1.0, "Limit"), None);
            draw_fx_slider_pair(
                ui,
                (&mut strip.reverb, 0.0..=100.0, "Rev"),
                Some((&mut strip.chorus, 0.0..=100.0, "Chor")),
            );
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
        "Pan" => format!("{value:.2}"),
        "Gain" => format!("{value:.0}"),
        "Limit" | "Rev" | "Chor" => format!("{value:.0}"),
        _ => format!("{value:.1}"),
    }
}
