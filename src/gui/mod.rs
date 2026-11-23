//! The iced desktop app: window, sidebar, settings, query panel, graph canvas and layout.
//!
//! It is in the library (not only the binary) so tests can boot [`App`] and
//! drive it, e.g. with `iced_test`.

mod app;
mod canton_draw;
mod canvas;
#[cfg(feature = "dashboard")]
mod dashboard;
pub(crate) mod icon;
mod layout;
mod model;
mod query_panel;
mod settings;
mod sidebar;

use std::path::PathBuf;

pub use app::{App, CantonFilterChange, Message, Selection};
use crate::config::{CONFIG_FILE, Config};
use crate::logging::LogControl;
use crate::saved_queries::QUERIES_FILE;

/// Where the app keeps its settings and saved queries.
#[derive(Debug, Clone)]
pub struct Files {
    /// The settings, `config.toml`.
    pub config: PathBuf,
    /// The saved queries, `queries.toml`.
    pub queries: PathBuf,
}

impl Default for Files {
    /// `config.toml` and `queries.toml` in the folder the app runs from.
    fn default() -> Self {
        Files { config: PathBuf::from(CONFIG_FILE), queries: PathBuf::from(QUERIES_FILE) }
    }
}

/// Opens the window and runs until it is closed. `startup_error` is shown in
/// the sidebar, e.g. when the config file was invalid and defaults are used.
/// `log` lets saved settings change the log level (`None` if logging is off).
pub fn run(config: Config, startup_error: Option<String>, log: Option<LogControl>) -> iced::Result {
    program(config, Files::default(), startup_error, log).run()
}

/// The app as an iced program: what [`run`] opens in a window, and what a
/// test can run headless in `iced_test`'s `Emulator`.
pub fn program(
    config: Config,
    files: Files,
    startup_error: Option<String>,
    log: Option<LogControl>,
) -> iced::application::Application<impl iced::Program<State = App, Message = Message, Theme = iced::Theme>> {
    iced::application(
        move || {
            let mut app = App::boot(config.clone(), files.clone(), startup_error.clone(), log.clone());
            let task = app.first_task();
            (app, task)
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
}
