//! Desktop viewer for a SurrealDB database as an interactive graph, built with iced.
//!
//! Run with `cargo run`. Settings are in `config.toml` (created on first run,
//! also editable in the app), logs go to the terminal and to `logs/`.
//! The app keeps the database open while running, so the examples can't run
//! at the same time.

mod gui;

use surrealdb_explore::config::Config;
use surrealdb_explore::logging;

fn main() -> iced::Result {
    let (config, created, startup_error) = match Config::load_or_create() {
        Ok((config, created)) => (config, created, None),
        Err(e) => {
            let message = format!("Could not read {}: {e}. Using defaults.", Config::path().display());
            (Config::default(), false, Some(message))
        }
    };

    // Keep the guard until the app exits, so the log file is flushed.
    let _log_guard = match logging::init(&config.logging) {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("Logging is off: {e}");
            None
        }
    };

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "surrealdb-explore starting");
    if created {
        tracing::info!(path = %Config::path().display(), "created a default config file");
    }
    if let Some(e) = &startup_error {
        tracing::error!("{e}");
    }

    let result = gui::run(config, startup_error);
    tracing::info!("surrealdb-explore exiting");
    result
}
