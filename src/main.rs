//! Desktop viewer for a SurrealDB database as an interactive graph, built with iced.
//!
//! Run with `cargo run`. Settings are in `config.toml` (created on first run,
//! also editable in the app), logs go to the terminal and to `logs/`.
//! The app keeps the database open while running, so only one copy can run
//! at a time. Sample data can be loaded from the app (Settings → Data).

mod gui;

use surrealdb_explorer::config::Config;
use surrealdb_explorer::logging;

fn main() -> iced::Result {
    let (config, created, startup_error) = match Config::load_or_create() {
        Ok((config, created)) => (config, created, None),
        Err(e) => {
            let message = format!("Could not read {}: {e}. Using defaults.", Config::path().display());
            (Config::default(), false, Some(message))
        }
    };

    // Keep the guard until the app exits, so the log file is flushed.
    let (_log_guard, log_control) = match logging::init(&config.logging) {
        Ok((guard, control)) => (Some(guard), Some(control)),
        Err(e) => {
            eprintln!("Logging is off: {e}");
            (None, None)
        }
    };

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "surrealdb-explorer starting");
    if created {
        tracing::info!(path = %Config::path().display(), "created a default config file");
    }
    if let Some(e) = &startup_error {
        tracing::error!("{e}");
    }

    let result = gui::run(config, startup_error, log_control);
    tracing::info!("surrealdb-explorer exiting");
    result
}
