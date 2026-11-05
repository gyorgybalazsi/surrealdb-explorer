//! App configuration, kept in `config.toml` in the folder the app runs from.
//!
//! The file is created with defaults on first run. Every field is optional in
//! the file: missing ones take their default, so older files keep working.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;

/// Where the configuration is read from and saved to.
pub const CONFIG_FILE: &str = "config.toml";

/// Log levels offered in the settings, from least to most detailed.
pub const LOG_LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub database: DatabaseConfig,
    pub graph: GraphConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Folder of the SurrealKV database, relative to the folder the app runs from.
    pub path: String,
    pub namespace: String,
    pub database: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphConfig {
    /// Most records loaded per table, so a big table can't freeze the app.
    pub max_records_per_table: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// One of `LOG_LEVELS`. The `RUST_LOG` environment variable overrides it.
    pub level: String,
    /// Folder for the daily log files.
    pub directory: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database: DatabaseConfig::default(),
            graph: GraphConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        DatabaseConfig {
            path: "data".into(),
            namespace: "main".into(),
            database: "main".into(),
        }
    }
}

impl Default for GraphConfig {
    fn default() -> Self {
        GraphConfig { max_records_per_table: 1000 }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig { level: "info".into(), directory: "logs".into() }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        PathBuf::from(CONFIG_FILE)
    }

    /// Reads `config.toml`, or creates it with defaults if it doesn't exist.
    /// Returns the config and whether the file was just created.
    pub fn load_or_create() -> Result<(Config, bool), Error> {
        Self::load_or_create_at(&Self::path())
    }

    pub fn load_or_create_at(path: &Path) -> Result<(Config, bool), Error> {
        if !path.exists() {
            let config = Config::default();
            config.save_to(path)?;
            return Ok((config, true));
        }
        let text = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&text)
            .map_err(|e| format!("{} is not valid: {e}", path.display()))?;
        config.validate()?;
        Ok((config, false))
    }

    pub fn save(&self) -> Result<(), Error> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), Error> {
        self.validate()?;
        let text = format!(
            "# surrealdb-explore configuration. Edit here or in the app's Settings.\n\
             # Saving in the app applies changes at once; edits here apply at the next start.\n\n{}",
            toml::to_string_pretty(self)?
        );
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Checks values the app can't work with.
    pub fn validate(&self) -> Result<(), Error> {
        let d = &self.database;
        if d.path.trim().is_empty() || d.namespace.trim().is_empty() || d.database.trim().is_empty() {
            return Err("database path, namespace and database must not be empty".into());
        }
        if self.graph.max_records_per_table == 0 {
            return Err("max records per table must be at least 1".into());
        }
        if !LOG_LEVELS.contains(&self.logging.level.as_str()) {
            return Err(format!("log level must be one of: {}", LOG_LEVELS.join(", ")).into());
        }
        Ok(())
    }
}
