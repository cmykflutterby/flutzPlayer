use eframe::egui;

use crate::app::FlutzDesktopApp;

pub(crate) fn draw_metadata_window_viewport(
    context: &egui::Context,
    class: egui::ViewportClass,
    app: &mut FlutzDesktopApp,
) {
    match class {
        egui::ViewportClass::Embedded => {
            egui::Window::new("Metadata")
                .id(egui::Id::new("metadata_window_embedded"))
                .resizable(true)
                .default_size(egui::vec2(920.0, 320.0))
                .show(context, |ui| {
                    app.draw_metadata_window_contents(ui);
                });
        }
        _ => {
            egui::CentralPanel::default().show(context, |ui| {
                app.draw_metadata_window_contents(ui);
            });
        }
    }
}
