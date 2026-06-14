use eframe::egui;

pub(crate) const DRACULA_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(40, 42, 54);
pub(crate) const DRACULA_PANEL_FILL: egui::Color32 = egui::Color32::from_rgb(33, 34, 44);
pub(crate) const DRACULA_PANEL_STROKE: egui::Color32 = egui::Color32::from_rgb(98, 114, 164);
const DRACULA_FOREGROUND: egui::Color32 = egui::Color32::from_rgb(248, 248, 242);
const DRACULA_COMMENT: egui::Color32 = egui::Color32::from_rgb(98, 114, 164);
const DRACULA_CYAN: egui::Color32 = egui::Color32::from_rgb(139, 233, 253);
const DRACULA_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 250, 123);
const DRACULA_LIGHT_GREEN: egui::Color32 = egui::Color32::from_rgb(74, 180, 150);
const DRACULA_RED: egui::Color32 = egui::Color32::from_rgb(255, 85, 85);
const DRACULA_SELECTION: egui::Color32 = egui::Color32::from_rgb(68, 71, 90);

pub(crate) fn apply_dracula_theme(context: &egui::Context) {
    apply_preferred_fonts(context);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(DRACULA_FOREGROUND);
    visuals.panel_fill = DRACULA_PANEL_FILL;
    visuals.window_fill = DRACULA_BACKGROUND;
    visuals.faint_bg_color = DRACULA_SELECTION;
    visuals.extreme_bg_color = egui::Color32::from_rgb(24, 25, 33);
    visuals.code_bg_color = egui::Color32::from_rgb(30, 31, 41);
    visuals.hyperlink_color = DRACULA_CYAN;
    visuals.warn_fg_color = DRACULA_CYAN;
    visuals.error_fg_color = DRACULA_RED;
    visuals.selection.bg_fill = DRACULA_LIGHT_GREEN;
    visuals.widgets.noninteractive.bg_fill = DRACULA_BACKGROUND;
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(52, 55, 70);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(61, 64, 82);
    visuals.widgets.hovered.bg_stroke.color = DRACULA_CYAN;
    visuals.widgets.hovered.fg_stroke.color = DRACULA_FOREGROUND;
    visuals.widgets.active.bg_fill = DRACULA_LIGHT_GREEN;
    visuals.widgets.active.bg_stroke.color = DRACULA_LIGHT_GREEN;
    visuals.widgets.active.fg_stroke.color = DRACULA_FOREGROUND;
    visuals.widgets.open.bg_fill = DRACULA_LIGHT_GREEN;
    visuals.widgets.open.bg_stroke.color = DRACULA_LIGHT_GREEN;
    visuals.widgets.open.fg_stroke.color = DRACULA_FOREGROUND;
    visuals.window_stroke.color = DRACULA_PANEL_STROKE;
    visuals.popup_shadow.color = egui::Color32::from_black_alpha(96);

    visuals.widgets.active.weak_bg_fill = DRACULA_GREEN;
    visuals.widgets.hovered.weak_bg_fill = DRACULA_COMMENT;
    visuals.widgets.inactive.weak_bg_fill = DRACULA_COMMENT;
    context.set_visuals(visuals);
}

fn apply_preferred_fonts(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let mono_font = "jetbrains_mono_variable";
    let symbol_font = "noto_sans_symbols2";
    let math_font = "noto_sans_math";

    fonts.font_data.insert(
        mono_font.to_owned(),
        egui::FontData::from_static(damascene_fonts::JETBRAINS_MONO_VARIABLE).into(),
    );
    fonts.font_data.insert(
        symbol_font.to_owned(),
        egui::FontData::from_static(damascene_fonts::NOTO_SANS_SYMBOLS2_REGULAR).into(),
    );
    fonts.font_data.insert(
        math_font.to_owned(),
        egui::FontData::from_static(damascene_fonts::NOTO_SANS_MATH_REGULAR).into(),
    );

    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        list.insert(0, mono_font.to_owned());
        list.push(symbol_font.to_owned());
        list.push(math_font.to_owned());
    }

    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        list.push(symbol_font.to_owned());
        list.push(math_font.to_owned());
    }

    context.set_fonts(fonts);
}
