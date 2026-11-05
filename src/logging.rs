//! Logging to the terminal and to a daily log file, with `tracing`.

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, Registry, reload};

use crate::Error;
use crate::config::LoggingConfig;

/// Changes the log level while the app runs.
#[derive(Clone)]
pub struct LogControl {
    filter: reload::Handle<EnvFilter, Registry>,
    /// `RUST_LOG` was set; it wins over the configured level.
    from_env: bool,
}

impl LogControl {
    /// Switches to `level` for this app (libraries stay at warnings). Returns
    /// `false` if `RUST_LOG` is set, which keeps overriding the level.
    pub fn set_level(&self, level: &str) -> Result<bool, Error> {
        if self.from_env {
            return Ok(false);
        }
        self.filter
            .reload(app_filter(level))
            .map_err(|e| format!("could not change the log level: {e}"))?;
        tracing::info!(level, "log level changed");
        Ok(true)
    }
}

/// Starts logging. Keep the returned guard alive for the whole run: dropping
/// it flushes and stops the log file writer.
///
/// The configured level applies to this app; other libraries (SurrealDB,
/// iced, …) log warnings and errors only. `RUST_LOG` overrides both, e.g.
/// `RUST_LOG=debug cargo run`.
pub fn init(config: &LoggingConfig) -> Result<(WorkerGuard, LogControl), Error> {
    std::fs::create_dir_all(&config.directory)?;
    let file = tracing_appender::rolling::daily(&config.directory, "surrealdb-explore.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file);

    let env_filter = EnvFilter::try_from_default_env().ok();
    let from_env = env_filter.is_some();
    let (filter, handle) =
        reload::Layer::new(env_filter.unwrap_or_else(|| app_filter(&config.level)));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false))
        .with(fmt::layer().with_ansi(false).with_writer(file_writer))
        .try_init()
        .map_err(|e| format!("could not start logging: {e}"))?;
    Ok((guard, LogControl { filter: handle, from_env }))
}

fn app_filter(level: &str) -> EnvFilter {
    EnvFilter::new(format!("warn,surrealdb_explore={level}"))
}
