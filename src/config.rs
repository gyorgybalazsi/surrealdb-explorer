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
    pub assistant: AssistantConfig,
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

/// Asking questions in plain English, answered by Claude with SurrealQL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantConfig {
    /// Which Claude model writes the queries.
    pub mode: AssistantMode,
    /// 1Password reference to the Claude API key, read with the `op` tool,
    /// e.g. `op://Personal/my-key/credential`. The `ANTHROPIC_API_KEY`
    /// environment variable, if set, is used instead.
    pub api_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssistantMode {
    /// Haiku first; Opus when Haiku is unsure or its query fails.
    Auto,
    Haiku,
    Opus,
}

impl AssistantMode {
    pub const ALL: [AssistantMode; 3] = [AssistantMode::Auto, AssistantMode::Haiku, AssistantMode::Opus];
}

impl std::fmt::Display for AssistantMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AssistantMode::Auto => "Auto (Haiku, Opus when needed)",
            AssistantMode::Haiku => "Always Haiku",
            AssistantMode::Opus => "Always Opus",
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database: DatabaseConfig::default(),
            graph: GraphConfig::default(),
            logging: LoggingConfig::default(),
            assistant: AssistantConfig::default(),
        }
    }
}

impl Default for AssistantConfig {
    fn default() -> Self {
        AssistantConfig { mode: AssistantMode::Auto, api_key: String::new() }
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
            "# surrealdb-explorer configuration. Edit here or in the app's Settings.\n\
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
        let key = self.assistant.api_key.trim();
        if !key.is_empty() && !key.starts_with("op://") {
            return Err("the API key setting must be a 1Password reference starting with op:// \
                        (never the key itself)"
                .into());
        }
        Ok(())
    }
}
