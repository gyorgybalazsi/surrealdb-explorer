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
    /// Open a database folder directly, or connect to a SurrealDB server.
    pub connection: Connection,
    /// Folder of the SurrealKV database, relative to the folder the app runs
    /// from (folder connection).
    pub path: String,
    /// Address of the SurrealDB server, e.g. `ws://127.0.0.1:8000` (server connection).
    pub server: String,
    /// Root user to sign in to the server with.
    pub username: String,
    /// The password, or a 1Password reference to it (`op://…`).
    pub password: String,
    pub namespace: String,
    pub database: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Connection {
    /// The app opens the folder itself; nothing else can use it meanwhile.
    Folder,
    /// The app is one client of a SurrealDB server, which other programs
    /// (Studio, an import job, …) can use at the same time.
    Server,
}

impl Connection {
    pub const ALL: [Connection; 2] = [Connection::Folder, Connection::Server];
}

impl std::fmt::Display for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Connection::Folder => "Database folder",
            Connection::Server => "SurrealDB server",
        })
    }
}

impl DatabaseConfig {
    /// Whether `other` reaches the same folder or server (it may still use
    /// another namespace or database).
    pub fn same_place(&self, other: &DatabaseConfig) -> bool {
        self.connection == other.connection
            && match self.connection {
                Connection::Folder => self.path == other.path,
                Connection::Server => {
                    self.server == other.server && self.username == other.username && self.password == other.password
                }
            }
    }

    /// Where the database is, for messages and the log (never the password).
    pub fn location(&self) -> String {
        match self.connection {
            Connection::Folder => format!("folder {}", self.path),
            Connection::Server => format!("server {}", self.server),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphConfig {
    /// Most records loaded per table, so a big table can't freeze the app.
    pub max_records_per_table: usize,
    /// Most records drawn for a query's result.
    pub max_result_records: usize,
    /// Also draw every relation between the records of a query's result,
    /// not only the relations the query returned.
    pub connect_results: bool,
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
    /// Running a suggested query also runs its context query, which shows
    /// how the answer connects to the records the question is about.
    pub show_connections: bool,
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
        AssistantConfig { mode: AssistantMode::Auto, api_key: String::new(), show_connections: true }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        DatabaseConfig {
            connection: Connection::Folder,
            path: "data".into(),
            server: "ws://127.0.0.1:8000".into(),
            username: "root".into(),
            password: String::new(),
            namespace: "main".into(),
            database: "main".into(),
        }
    }
}

impl Default for GraphConfig {
    fn default() -> Self {
        GraphConfig { max_records_per_table: 1000, max_result_records: 2000, connect_results: false }
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
        if d.namespace.trim().is_empty() || d.database.trim().is_empty() {
            return Err("namespace and database must not be empty".into());
        }
        match d.connection {
            Connection::Folder if d.path.trim().is_empty() => {
                return Err("the database folder must not be empty".into());
            }
            Connection::Server if d.server.trim().is_empty() || d.username.trim().is_empty() => {
                return Err("the server address and username must not be empty".into());
            }
            _ => {}
        }
        if self.graph.max_records_per_table == 0 || self.graph.max_result_records == 0 {
            return Err("the record limits must be at least 1".into());
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
