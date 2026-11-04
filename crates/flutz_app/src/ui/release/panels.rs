use super::*;

pub(super) fn draw_panel_shell(ui: &mut egui::Ui, rect: egui::Rect) -> egui::Rect {
    ui.painter().rect_filled(rect, 4.0, RELEASE_PANEL_FILL);
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, RELEASE_PANEL_STROKE),
        egui::StrokeKind::Inside,
    );
    rect.shrink2(egui::vec2(14.0, 12.0))
}

pub(super) fn draw_soundfont_manager(
    ui: &mut egui::Ui,
    app: &mut FlutzDesktopApp,
    panel_rect: egui::Rect,
) {
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

    draw_release_loop_controls(&mut panel_ui, app);
}

pub(super) fn draw_release_loop_controls(ui: &mut egui::Ui, app: &mut FlutzDesktopApp) {
    ui.horizontal(|ui| {
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
        ui.label(app.transport_unit_label());
    });

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(72.0);
        ui.label("Start");
        let unit_prefix = app.transport_unit_prefix();
        let mut loop_start_tick = app.loop_start_tick_value();
        if ui
            .add(
                egui::DragValue::new(&mut loop_start_tick)
                    .speed(1.0)
                    .prefix(unit_prefix),
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
                    .prefix(unit_prefix),
            )
            .changed()
        {
            app.set_loop_end_tick(loop_end_tick);
        }
    });
}

pub(super) fn draw_master_panel(
    ui: &mut egui::Ui,
    app: &mut FlutzDesktopApp,
    panel_rect: egui::Rect,
) {
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

pub(super) fn draw_master_slider_row(
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

pub(super) fn draw_master_limiter_row(ui: &mut egui::Ui, master: &mut crate::app::MasterControls) {
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

pub(super) fn draw_decoded_loop_panel(
    ui: &mut egui::Ui,
    app: &mut FlutzDesktopApp,
    panel_rect: egui::Rect,
) {
    let content_rect = draw_panel_shell(ui, panel_rect.shrink2(egui::vec2(0.0, 2.0)));
    let mut panel_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    panel_ui.set_width(content_rect.width());

    panel_ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Loop").size(16.0));
        ui.separator();
        ui.label(app.transport_unit_label());
    });
    panel_ui.add_space(4.0);
    draw_release_loop_controls(&mut panel_ui, app);
}
