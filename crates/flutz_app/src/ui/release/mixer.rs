use super::*;

pub(super) fn draw_mixer(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
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

pub(super) fn clear_all_solos(app: &mut FlutzDesktopApp) {
    for font in app.soundfonts() {
        font.soloed = false;
        for strip in &mut font.strips {
            strip.soloed = false;
        }
    }
}

pub(super) fn clear_all_mutes(app: &mut FlutzDesktopApp) {
    for font in app.soundfonts() {
        font.muted = false;
        for strip in &mut font.strips {
            strip.muted = false;
        }
    }
}

pub(super) fn draw_soundfont_row(
    ui: &mut egui::Ui,
    font: &mut SoundFontUiRow,
    fx_expanded: bool,
) -> bool {
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

pub(super) fn draw_strip(
    ui: &mut egui::Ui,
    strip: &mut MixerStripUiState,
    fx_expanded: bool,
) -> bool {
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

pub(super) fn strip_display_label(strip: &MixerStripUiState) -> String {
    let program_prefix = if strip.is_percussion { 'P' } else { 'I' };
    format!("Ch {}, {}-{}", strip.channel, program_prefix, strip.program)
}

pub(super) fn draw_volume_lane(
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

pub(super) fn draw_mute_solo_fx_row(
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

pub(super) fn fixed_centered_checkbox(
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

pub(super) fn fixed_centered_small(ui: &mut egui::Ui, width: f32, height: f32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(9.0),
        ui.visuals().weak_text_color(),
    );
}

pub(super) fn fixed_value_text(ui: &mut egui::Ui, width: f32, height: f32, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.label(egui::RichText::new(text).size(9.0).monospace());
        },
    );
}

pub(super) fn draw_fx_slider_grid(ui: &mut egui::Ui, strip: &mut MixerStripUiState, width: f32) {
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

pub(super) fn draw_horizontal_pan_slider(ui: &mut egui::Ui, value: &mut f32) {
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

pub(super) fn draw_fx_slider_pair(
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

pub(super) fn draw_vertical_fx_slider(
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

pub(super) fn slider_value_text(label: &str, value: f32) -> String {
    match label {
        "db" | "Lmt" | "Rev" | "Chor" => signed_value_text(value, 0),
        _ => signed_value_text(value, 1),
    }
}

pub(super) fn signed_value_text(value: f32, decimals: usize) -> String {
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
