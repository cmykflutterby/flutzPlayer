mod theme;
pub(crate) mod metadata_popup;
pub(crate) mod playlist_window;

#[cfg(debug_assertions)]
mod debug;
#[cfg(not(debug_assertions))]
mod release;

pub(crate) use theme::apply_dracula_theme;
#[cfg(not(debug_assertions))]
pub(crate) use theme::{DRACULA_PANEL_FILL, DRACULA_PANEL_STROKE};

#[cfg(debug_assertions)]
pub use debug::{apply_theme, draw_app, UiRefreshPolicy};
#[cfg(not(debug_assertions))]
pub use release::{apply_theme, draw_app, UiRefreshPolicy};
