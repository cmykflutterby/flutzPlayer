use super::*;

pub(super) fn draw_decoded_peq_editor(
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
    panel_ui.set_min_height(content_rect.height());

    let collapsed = app.release_editor_panels_collapsed();
    panel_ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Parametric EQ").size(16.0));
        if ui
            .add_sized(
                [22.0, 20.0],
                egui::Button::new(if collapsed { "▶" } else { "▼" }),
            )
            .clicked()
        {
            app.toggle_release_editor_panels_collapsed();
        }
        ui.separator();
        ui.label(app.decoded_peq_config_source().label());
        ui.add(egui::Label::new(app.decoded_peq_preset_display_name()).truncate());
    });
    if collapsed {
        return;
    }

    let mut config = app.decoded_peq_config().clone();
    config.bands.sort_by(|left, right| {
        left.frequency_hz
            .partial_cmp(&right.frequency_hz)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let original = config.clone();

    panel_ui.add_space(6.0);
    draw_peq_section_label(&mut panel_ui, "Response");
    draw_peq_response_preview(&mut panel_ui, &config);

    panel_ui.add_space(6.0);
    panel_ui.horizontal(|ui| {
        ui.add_sized([96.0, 20.0], egui::Label::new("Preset/config:"));
        egui::ComboBox::from_id_salt("decoded_peq_preset_config")
            .width(260.0)
            .selected_text(app.decoded_peq_preset_display_name())
            .show_ui(ui, |ui| {
                for preset_name in app.builtin_decoded_peq_preset_names() {
                    if ui.selectable_label(false, *preset_name).clicked() {
                        app.apply_builtin_decoded_peq_preset(preset_name);
                    }
                }
            });
        ui.add(egui::Label::new(app.decoded_peq_preset_path_label()).truncate());
    });
    panel_ui.horizontal(|ui| {
        if ui.button("Load PEQ...").clicked() {
            app.load_decoded_peq_preset_dialog();
        }
        if ui.button("Save PEQ").clicked() {
            app.save_decoded_peq_preset();
        }
        if ui.button("Save PEQ As...").clicked() {
            app.save_decoded_peq_preset_as();
        }
        if ui
            .button("sD")
            .on_hover_text("Save current PEQ as the default in preferences.ini")
            .clicked()
        {
            app.save_decoded_peq_default();
        }
        if ui.button("Reset").clicked() {
            app.reset_decoded_peq_config();
        }
        let mut bypass = app.decoded_peq_bypass_enabled();
        if ui.checkbox(&mut bypass, "Bypass EQ").changed() {
            app.set_decoded_peq_bypass_enabled(bypass);
        }
    });

    panel_ui.add_space(6.0);
    draw_peq_section_label(&mut panel_ui, "Global");
    panel_ui.horizontal(|ui| {
        ui.add_sized([96.0, 20.0], egui::Label::new("Output gain"));
        ui.add(egui::Slider::new(&mut config.output_gain_db, -24.0..=24.0).show_value(false));
        ui.add_sized(
            [64.0, 20.0],
            egui::Label::new(format!("{:+.1} dB", config.output_gain_db)),
        );
        ui.add_sized([46.0, 20.0], egui::Label::new("Mix"));
        ui.add(egui::Slider::new(&mut config.wet_mix, 0.0..=1.0).show_value(false));
        ui.label(format!("{:.0}%", config.wet_mix * 100.0));
    });
    panel_ui.horizontal(|ui| {
        ui.label(format!(
            "Sample rate {} Hz   Channels {}   Layout {:?}   Processing {}",
            config.sample_rate_hz,
            config.channel_count,
            config.channel_layout,
            if app.decoded_peq_bypass_enabled() {
                "Bypassed"
            } else {
                "Active"
            }
        ));
    });

    panel_ui.add_space(8.0);
    panel_ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Bands").strong());
        if ui.add_sized([28.0, 22.0], egui::Button::new("+")).clicked() {
            config.bands.push(PeqBandConfig {
                frequency_hz: 1_000.0,
                gain_db: 0.0,
                attack_ms: 5.0,
                release_ms: 60.0,
                ..PeqBandConfig::default()
            });
        }
        ui.label(format!("{} band(s)", config.bands.len()));
    });

    let available_height = (content_rect.bottom() - panel_ui.cursor().top()).max(72.0);
    let band_height = available_height.max(PEQ_MIN_BAND_TABLE_HEIGHT);
    egui::ScrollArea::vertical()
        .max_height(band_height)
        .auto_shrink([false, false])
        .show(&mut panel_ui, |ui| {
            draw_peq_band_header(ui);
            let mut remove_index = None;
            let mut duplicate_index = None;
            for (index, band) in config.bands.iter_mut().enumerate() {
                draw_peq_band_row(ui, index, band, &mut duplicate_index, &mut remove_index);
            }
            if let Some(index) = duplicate_index {
                if let Some(band) = config.bands.get(index).cloned() {
                    config.bands.push(band);
                }
            }
            if let Some(index) = remove_index {
                if index < config.bands.len() {
                    config.bands.remove(index);
                }
            }
        });

    if config != original {
        app.apply_decoded_peq_config(config);
    }
}

pub(super) fn draw_peq_section_label(ui: &mut egui::Ui, label: &str) {
    ui.label(egui::RichText::new(label).strong());
}

pub(super) fn draw_peq_band_header(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_sized([26.0, 18.0], egui::Label::new("On"));
        ui.add_sized([78.0, 18.0], egui::Label::new("Type"));
        ui.add_sized([104.0, 18.0], egui::Label::new("Frequency"));
        ui.add_sized([84.0, 18.0], egui::Label::new("Gain"));
        ui.add_sized([72.0, 18.0], egui::Label::new("Q/Width"));
        ui.add_sized([76.0, 18.0], egui::Label::new("Attack"));
        ui.add_sized([76.0, 18.0], egui::Label::new("Rel"));
        ui.add_sized([54.0, 18.0], egui::Label::new("Dup/Del"));
    });
}

pub(super) fn draw_peq_band_row(
    ui: &mut egui::Ui,
    index: usize,
    band: &mut PeqBandConfig,
    duplicate_index: &mut Option<usize>,
    remove_index: &mut Option<usize>,
) {
    ui.horizontal(|ui| {
        ui.add_sized([26.0, 22.0], egui::Checkbox::new(&mut band.enabled, ""));
        egui::ComboBox::from_id_salt(("peq_filter", index))
            .width(78.0)
            .selected_text(match band.filter_type {
                PeqFilterType::Bell => "Bell",
                PeqFilterType::LowShelf => "LowShelf",
                PeqFilterType::HighShelf => "HighShelf",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut band.filter_type, PeqFilterType::Bell, "Bell");
                ui.selectable_value(&mut band.filter_type, PeqFilterType::LowShelf, "LowShelf");
                ui.selectable_value(&mut band.filter_type, PeqFilterType::HighShelf, "HighShelf");
            });
        ui.add_sized(
            [104.0, 22.0],
            egui::DragValue::new(&mut band.frequency_hz)
                .range(20.0..=24_000.0)
                .speed(1.0)
                .suffix(" Hz"),
        );
        ui.add_sized(
            [84.0, 22.0],
            egui::DragValue::new(&mut band.gain_db)
                .range(-24.0..=24.0)
                .speed(0.1)
                .suffix(" dB"),
        );
        let mut q = match band.bandwidth {
            Bandwidth::Q { value } | Bandwidth::Octaves { value } => value,
        };
        if ui
            .add_sized(
                [72.0, 22.0],
                egui::DragValue::new(&mut q).range(0.05..=12.0).speed(0.05),
            )
            .changed()
        {
            band.bandwidth = Bandwidth::Q { value: q };
        }
        ui.add_sized(
            [76.0, 22.0],
            egui::DragValue::new(&mut band.attack_ms)
                .range(0.0..=5_000.0)
                .speed(1.0)
                .suffix(" ms"),
        );
        ui.add_sized(
            [76.0, 22.0],
            egui::DragValue::new(&mut band.release_ms)
                .range(0.0..=5_000.0)
                .speed(1.0)
                .suffix(" ms"),
        );
        if ui.button("+").clicked() {
            *duplicate_index = Some(index);
        }
        if ui.button("-").clicked() {
            *remove_index = Some(index);
        }
    });
}

pub(super) fn draw_peq_response_preview(ui: &mut egui::Ui, config: &flutz_peq::PeqConfig) {
    let width = ui.available_width().max(120.0);
    let height = 104.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    let label_color = egui::Color32::from_rgb(188, 190, 198);
    let font = egui::FontId::monospace(11.0);
    let graph_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 44.0, rect.top() + 4.0),
        egui::pos2(rect.right() - 4.0, rect.bottom() - 20.0),
    );
    painter.rect_stroke(
        graph_rect,
        2.0,
        egui::Stroke::new(1.0, RELEASE_PANEL_STROKE),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [graph_rect.left_center(), graph_rect.right_center()],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(108, 112, 122)),
    );
    for (label, t) in [
        ("+12 dB", 0.0_f32),
        ("+6 dB", 0.25),
        ("0 dB", 0.5),
        ("-6 dB", 0.75),
        ("-12 dB", 1.0),
    ] {
        let y = egui::lerp(graph_rect.top()..=graph_rect.bottom(), t);
        painter.text(
            egui::pos2(rect.left() + 2.0, y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            label_color,
        );
    }
    for (label, t) in [
        ("20", 0.0_f32),
        ("50", 0.1326),
        ("100", 0.2330),
        ("500", 0.4663),
        ("1k", 0.5663),
        ("5k", 0.7997),
        ("10k", 0.9003),
        ("20k Hz", 1.0),
    ] {
        let x = egui::lerp(graph_rect.left()..=graph_rect.right(), t);
        painter.text(
            egui::pos2(x, rect.bottom() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            font.clone(),
            label_color,
        );
    }
    let mut points = Vec::with_capacity(96);
    for index in 0..96 {
        let t = index as f32 / 95.0;
        let hz = 20.0_f32 * (1_000.0_f32).powf(t);
        let mut gain = config.output_gain_db;
        for band in &config.bands {
            if !band.enabled {
                continue;
            }
            let q = match band.bandwidth {
                Bandwidth::Q { value } | Bandwidth::Octaves { value } => value.max(0.05),
            };
            let distance = (hz / band.frequency_hz.max(20.0)).log2().abs();
            let influence = match band.filter_type {
                PeqFilterType::Bell => (1.0 - distance * q).clamp(0.0, 1.0),
                PeqFilterType::LowShelf => {
                    (1.0 - (hz / band.frequency_hz.max(20.0)).log2()).clamp(0.0, 1.0)
                }
                PeqFilterType::HighShelf => {
                    (1.0 + (hz / band.frequency_hz.max(20.0)).log2()).clamp(0.0, 1.0)
                }
            };
            gain += band.gain_db * influence * config.wet_mix;
        }
        let x = egui::lerp(graph_rect.left()..=graph_rect.right(), t);
        let y = egui::lerp(
            graph_rect.bottom()..=graph_rect.top(),
            ((gain + 12.0) / 24.0).clamp(0.0, 1.0),
        );
        points.push(egui::pos2(x, y));
    }
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(2.0, egui::Color32::from_rgb(139, 233, 253)),
    ));
}
