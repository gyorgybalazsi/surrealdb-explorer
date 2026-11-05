//! Logging to the terminal and to a daily log file, with `tracing`.

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

use crate::Error;
use crate::config::LoggingConfig;

/// Starts logging. Keep the returned guard alive for the whole run: dropping
/// it flushes and stops the log file writer.
///
/// The configured level applies to this app; other libraries (SurrealDB,
/// iced, …) log warnings and errors only. `RUST_LOG` overrides both, e.g.
/// `RUST_LOG=debug cargo run`.
pub fn init(config: &LoggingConfig) -> Result<WorkerGuard, Error> {
    std::fs::create_dir_all(&config.directory)?;
    let file = tracing_appender::rolling::daily(&config.directory, "surrealdb-explore.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!("warn,surrealdb_explore={}", config.level))
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false))
        .with(fmt::layer().with_ansi(false).with_writer(file_writer))
        .try_init()
        .map_err(|e| format!("could not start logging: {e}"))?;
    Ok(guard)
}
