//! The iced desktop app: window, sidebar, settings, query panel, graph canvas and layout.

mod app;
mod canvas;
mod layout;
mod model;
mod query_panel;
mod settings;
mod sidebar;

use app::App;
use surrealdb_explore::config::Config;

/// Opens the window and runs until it is closed. `startup_error` is shown in
/// the sidebar, e.g. when the config file was invalid and defaults are used.
pub fn run(config: Config, startup_error: Option<String>) -> iced::Result {
    iced::application(
        move || App::boot(config.clone(), startup_error.clone()),
        App::update,
        App::view,
    )
    .title("SurrealDB graph")
    .subscription(App::subscription)
    .theme(App::theme)
    .window_size((1200.0, 800.0))
    .antialiasing(true)
    .run()
}
