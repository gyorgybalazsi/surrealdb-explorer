//! The iced desktop app: window, sidebar, settings, query panel, graph canvas and layout.
//!
//! It is in the library (not only the binary) so tests can boot [`App`] and
//! drive it, e.g. with `iced_test`.

mod app;
mod canton_draw;
mod canvas;
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
    iced::application(
        move || App::boot(config.clone(), Files::default(), startup_error.clone(), log.clone()),
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
