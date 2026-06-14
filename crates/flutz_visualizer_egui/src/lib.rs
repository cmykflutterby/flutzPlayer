use egui::{Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use flutz_visualizer_core::{VisualizerFrame, VIS_BAND_COUNT_TARGET};

pub const VIS_COLUMN_MIN_PX: f32 = 0.0;
pub const VIS_PEAK_MIN_PX: f32 = 0.0;
pub const VIS_PEAK_SQUARE_SIZE_PX: f32 = 4.0;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VisualizerStylePreset {
    Dracula,
}

impl Default for VisualizerStylePreset {
    fn default() -> Self {
        Self::Dracula
    }
}

#[derive(Debug, Clone)]
pub struct VisualizerRendererConfig {
    pub background_color: Color32,
    pub border_color: Color32,
    pub grid_color: Color32,
    pub column_bottom_color: Color32,
    pub column_top_color: Color32,
    pub peak_square_color: Color32,
    pub column_min_px: f32,
    pub peak_min_px: f32,
    pub peak_square_size_px: f32,
    pub spacing_px: f32,
    pub corner_radius_px: f32,
    pub style_preset: VisualizerStylePreset,
    pub draw_grid: bool,
}

impl Default for VisualizerRendererConfig {
    fn default() -> Self {
        Self {
            background_color: Color32::from_rgb(14, 17, 20),
            border_color: Color32::from_rgb(44, 50, 56),
            grid_color: Color32::from_rgba_premultiplied(68, 76, 84, 70),
            column_bottom_color: Color32::from_rgb(74, 180, 150),
            column_top_color: Color32::from_rgb(160, 244, 210),
            peak_square_color: Color32::from_rgb(246, 214, 106),
            column_min_px: VIS_COLUMN_MIN_PX,
            peak_min_px: VIS_PEAK_MIN_PX,
            peak_square_size_px: VIS_PEAK_SQUARE_SIZE_PX,
            spacing_px: 3.0,
            corner_radius_px: 3.0,
            style_preset: VisualizerStylePreset::Dracula,
            draw_grid: true,
        }
    }
}

pub fn visualizer_ui(
    ui: &mut Ui,
    desired_size: Vec2,
    frame: &VisualizerFrame,
    config: &VisualizerRendererConfig,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::hover());
    paint_visualizer(ui, rect, frame, config);
    response
}

pub fn paint_visualizer(
    ui: &Ui,
    rect: Rect,
    frame: &VisualizerFrame,
    config: &VisualizerRendererConfig,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, config.corner_radius_px, config.background_color);
    painter.rect_stroke(
        rect,
        config.corner_radius_px,
        Stroke::new(1.0, config.border_color),
        StrokeKind::Inside,
    );

    let content = rect.shrink2(egui::vec2(8.0, 7.0));
    if content.width() <= 0.0 || content.height() <= 0.0 {
        return;
    }

    if config.draw_grid {
        for fraction in [0.25, 0.5, 0.75] {
            let y = content.bottom() - content.height() * fraction;
            painter.line_segment(
                [
                    egui::pos2(content.left(), y),
                    egui::pos2(content.right(), y),
                ],
                Stroke::new(1.0, config.grid_color),
            );
        }
    }

    let band_count = frame.band_count().max(VIS_BAND_COUNT_TARGET);
    let spacing = config.spacing_px.max(0.0);
    let bar_width = ((content.width() - spacing * (band_count.saturating_sub(1) as f32))
        / band_count as f32)
        .max(1.0);
    let baseline = content.bottom();
    let max_height = content.height();

    for index in 0..band_count {
        let x = content.left() + index as f32 * (bar_width + spacing);
        let Some(band) = frame.bands.get(index) else {
            continue;
        };
        let column_height = level_to_pixels(
            band.state.column_level_norm,
            max_height,
            config.column_min_px,
        );
        if column_height > 0.0 {
            let column_rect = Rect::from_min_max(
                egui::pos2(x, baseline - column_height),
                egui::pos2(x + bar_width, baseline),
            );
            painter.rect_filled(
                column_rect,
                (bar_width * 0.35).min(2.0),
                column_color(index, band_count, config),
            );
        }

        let peak_height = level_to_pixels(
            band.state.peak_square_level_norm,
            max_height,
            config.peak_min_px,
        );
        if peak_height > 0.0 || config.peak_min_px > 0.0 {
            let size = config.peak_square_size_px.max(1.0).min(bar_width.max(1.0));
            let peak_center_x = x + bar_width * 0.5;
            let peak_y =
                (baseline - peak_height - size * 0.5).clamp(content.top(), content.bottom() - size);
            let peak_rect = Rect::from_min_size(
                egui::pos2(peak_center_x - size * 0.5, peak_y),
                egui::vec2(size, size),
            );
            painter.rect_filled(peak_rect, 1.0, config.peak_square_color);
        }
    }
}

fn level_to_pixels(level: f32, max_height: f32, min_px: f32) -> f32 {
    let height = level.clamp(0.0, 1.0) * max_height;
    if height <= 0.0 {
        min_px.max(0.0).min(max_height)
    } else {
        height.max(min_px).min(max_height)
    }
}

fn column_color(index: usize, band_count: usize, config: &VisualizerRendererConfig) -> Color32 {
    let fraction = if band_count <= 1 {
        0.0
    } else {
        index as f32 / (band_count - 1) as f32
    };
    lerp_color(
        config.column_bottom_color,
        config.column_top_color,
        fraction * 0.55,
    )
}

fn lerp_color(start: Color32, end: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        (start.r() as f32 + (end.r() as f32 - start.r() as f32) * t).round() as u8,
        (start.g() as f32 + (end.g() as f32 - start.g() as f32) * t).round() as u8,
        (start.b() as f32 + (end.b() as f32 - start.b() as f32) * t).round() as u8,
    )
}
