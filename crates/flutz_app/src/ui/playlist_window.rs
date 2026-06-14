use eframe::egui;

use crate::app::FlutzDesktopApp;

pub(crate) fn draw_playlist_window_viewport(
    context: &egui::Context,
    class: egui::ViewportClass,
    app: &mut FlutzDesktopApp,
) {
    match class {
        egui::ViewportClass::Embedded => {
            egui::Window::new("Playlist")
                .id(egui::Id::new("playlist_window_embedded"))
                .resizable(true)
                .default_size(egui::vec2(720.0, 420.0))
                .show(context, |ui| {
                    app.draw_playlist_window_contents(ui, context);
                });
        }
        _ => {
            egui::CentralPanel::default().show(context, |ui| {
                app.draw_playlist_window_contents(ui, context);
            });
        }
    }
}
