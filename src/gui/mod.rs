//! The iced desktop app: window, sidebar, settings, query panel, graph canvas and layout.

mod app;
mod canton_draw;
mod canvas;
mod icon;
mod layout;
mod model;
mod query_panel;
mod settings;
mod sidebar;

use app::App;
use surrealdb_explorer::config::Config;
use surrealdb_explorer::logging::LogControl;

/// Opens the window and runs until it is closed. `startup_error` is shown in
/// the sidebar, e.g. when the config file was invalid and defaults are used.
/// `log` lets saved settings change the log level (`None` if logging is off).
pub fn run(config: Config, startup_error: Option<String>, log: Option<LogControl>) -> iced::Result {
    iced::application(
        move || {
            // The event loop exists by now, so macOS has an application to set it on.
            icon::set_dock_icon();
            App::boot(config.clone(), startup_error.clone(), log.clone())
        },
        App::update,
        App::view,
    )
    .title("SurrealDB Explorer")
    .subscription(App::subscription)
    .theme(App::theme)
    .window(iced::window::Settings {
        size: iced::Size::new(1200.0, 800.0),
        icon: icon::window_icon(),
        ..Default::default()
    })
    .antialiasing(true)
    .run()
}
