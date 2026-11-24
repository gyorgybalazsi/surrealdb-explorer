//! App configuration, kept in `config.toml` in the folder the app runs from.
//!
//! The file is created with defaults on first run. Every field is optional in
//! the file: missing ones take their default, so older files keep working.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::secret;

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
    pub mcp: McpConfig,
    pub analytics: AnalyticsConfig,
    pub memory: MemoryConfig,
}

/// The app's MCP server, through which Claude Code reads the database and
/// draws in the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub enabled: bool,
    /// Served at `http://127.0.0.1:<port>/mcp`. Changes apply at the next start.
    pub port: u16,
}

/// The watch on the app's own memory use (see `memory.rs`). Sizes are
/// resident memory in MB; 0 switches that check off. Changes apply at the
/// next start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// How often memory is sampled; 0 switches the watch off.
    pub check_seconds: u64,
    /// Warn, in the log and in the window, above this size.
    pub warn_above_mb: u64,
    /// Warn when memory grows by this much within a minute.
    pub growth_warn_mb: u64,
    /// Stop the app above this size, before the computer runs out of
    /// memory; 0 never stops. Left out, it is half the computer's memory, up
    /// to 8 GB.
    pub stop_above_mb: Option<u64>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        MemoryConfig { check_seconds: 5, warn_above_mb: 2048, growth_warn_mb: 1024, stop_above_mb: None }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        McpConfig { enabled: true, port: 8765 }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Address of the SurrealDB server, e.g. `ws://127.0.0.1:8000`. Empty:
    /// no database, e.g. to use only the analytics dashboard.
    pub server: String,
    /// User to sign in to the server with.
    pub username: String,
    /// Where that user is defined: on the server (root), on the namespace,
    /// or on the database.
    pub user_level: UserLevel,
    /// The password, or a 1Password reference to it (`op://…`).
    pub password: String,
    pub namespace: String,
    pub database: String,
    /// A read-only user (`ROLES VIEWER`) for the queries typed in the query
    /// panel or sent by Claude Code, so the server itself refuses any that
    /// would change data. Empty: those queries use the user above, and the
    /// app refuses the ones that look like writes.
    pub viewer_username: String,
    /// Where the viewer user is defined; usually on the database.
    pub viewer_user_level: UserLevel,
    /// The viewer's password, or a 1Password reference to it (`op://…`).
    pub viewer_password: String,
}

/// Where a server user is defined (`DEFINE USER … ON ROOT | NAMESPACE | DATABASE`).
/// A namespace or database user can only use its own namespace or database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserLevel {
    #[default]
    Root,
    Namespace,
    Database,
}

impl UserLevel {
    pub const ALL: [UserLevel; 3] = [UserLevel::Root, UserLevel::Namespace, UserLevel::Database];
}

impl std::fmt::Display for UserLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            UserLevel::Root => "Root user",
            UserLevel::Namespace => "Namespace user",
            UserLevel::Database => "Database user",
        })
    }
}

/// Leaves the passwords out (a 1Password reference is shown): settings are logged.
impl std::fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hidden = |p: &str| if secret::is_1password_reference(p) { p.trim().to_string() } else { "…".to_string() };
        f.debug_struct("DatabaseConfig")
            .field("server", &self.server)
            .field("username", &self.username)
            .field("user_level", &self.user_level)
            .field("password", &hidden(&self.password))
            .field("namespace", &self.namespace)
            .field("database", &self.database)
            .field("viewer_username", &self.viewer_username)
            .field("viewer_user_level", &self.viewer_user_level)
            .field("viewer_password", &hidden(&self.viewer_password))
            .finish()
    }
}

impl DatabaseConfig {
    /// Whether `other` reaches the same server as the same user (it may
    /// still use another namespace or database).
    pub fn same_place(&self, other: &DatabaseConfig) -> bool {
        self.server == other.server
            && self.username == other.username
            && self.password == other.password
            && self.user_level == other.user_level
            // A namespace or database user signs in to its own namespace or
            // database, so another one needs a new sign-in.
            && match self.user_level {
                UserLevel::Root => true,
                UserLevel::Namespace => self.namespace == other.namespace,
                UserLevel::Database => self.namespace == other.namespace && self.database == other.database,
            }
    }

    /// The connection settings of the read-only viewer user, if one is set:
    /// the same server, namespace and database, signed in as the viewer.
    pub fn viewer(&self) -> Option<DatabaseConfig> {
        let username = self.viewer_username.trim();
        (!username.is_empty()).then(|| DatabaseConfig {
            username: username.to_string(),
            user_level: self.viewer_user_level,
            password: self.viewer_password.clone(),
            viewer_username: String::new(),
            viewer_password: String::new(),
            ..self.clone()
        })
    }

    /// Where the database is, for messages and the log (never the password).
    pub fn location(&self) -> String {
        format!("server {}", self.server)
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
    /// Redraw the graph by itself when another program adds or removes
    /// records, such as an indexer streaming into the server.
    pub auto_refresh: bool,
    /// How often auto refresh checks the database, in seconds.
    pub refresh_seconds: u64,
    /// A database with more records than this opens on an overview of its
    /// tables instead of drawing them all.
    pub overview_above: usize,
    /// How many transactions the Canton view loads at a time.
    pub canton_transactions: usize,
    /// How many transactions the Canton view puts side by side.
    pub canton_columns: usize,
    /// In the Canton view, scroll back to the top whenever a filter changes.
    pub fit_on_filter: bool,
    /// In the Canton view, scroll down to the newest transactions whenever
    /// auto refresh brings in new ones (for watching a ledger live).
    pub canton_follow_newest: bool,
    /// How the graph is drawn.
    pub visualization: Visualization,
}

/// How the graph is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visualization {
    /// Any database: records as dots, spread out by a force-directed layout.
    #[default]
    Standard,
    /// A Canton ledger's event graph, drawn like rust-canton-toolbox's
    /// event-graph-ui: transactions in columns, their events as trees under
    /// them, and parties above. See `canton`.
    Canton,
}

impl Visualization {
    pub const ALL: [Visualization; 2] = [Visualization::Standard, Visualization::Canton];
}

impl std::fmt::Display for Visualization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Visualization::Standard => "Standard",
            Visualization::Canton => "Canton event graph",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// One of `LOG_LEVELS`. The `RUST_LOG` environment variable overrides it.
    pub level: String,
    /// Folder for the daily log files.
    pub directory: String,
}

/// The Dashboard, over the tables the canton-analytics daemon writes. Set
/// on its own, so the explorer can read a local folder while testing and a
/// bucket in production.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalyticsConfig {
    /// Where the tables are: a folder (relative to the folder the app runs
    /// from), or an `s3://`, `az://` or `gs://` URL.
    pub location: String,
    /// Object store options as `key=value`, e.g. `aws_region=eu-central-1`.
    /// Credentials usually come from the cloud's environment variables.
    pub storage_options: Vec<String>,
    /// The table last shown.
    pub selected: Option<String>,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        AnalyticsConfig { location: "tables".into(), storage_options: Vec::new(), selected: None }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database: DatabaseConfig::default(),
            graph: GraphConfig::default(),
            logging: LoggingConfig::default(),
            mcp: McpConfig::default(),
            analytics: AnalyticsConfig::default(),
            memory: MemoryConfig::default(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        DatabaseConfig {
            server: "ws://127.0.0.1:8000".into(),
            username: "root".into(),
            user_level: UserLevel::Root,
            password: String::new(),
            namespace: "main".into(),
            database: "main".into(),
            viewer_username: String::new(),
            viewer_user_level: UserLevel::Database,
            viewer_password: String::new(),
        }
    }
}

impl Default for GraphConfig {
    fn default() -> Self {
        GraphConfig {
            max_records_per_table: 1000,
            max_result_records: 2000,
            connect_results: false,
            auto_refresh: false,
            refresh_seconds: 2,
            overview_above: 2000,
            canton_transactions: 50,
            canton_columns: 3,
            fit_on_filter: true,
            canton_follow_newest: false,
            visualization: Visualization::Standard,
        }
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
        // An empty server address means no database (dashboard only).
        if !d.server.trim().is_empty() && d.username.trim().is_empty() {
            return Err("a server needs a username".into());
        }
        if self.graph.max_records_per_table == 0 || self.graph.max_result_records == 0 {
            return Err("the record limits must be at least 1".into());
        }
        if self.graph.canton_columns == 0 {
            return Err("canton_columns must be at least 1".into());
        }
        if self.graph.canton_transactions == 0 {
            return Err("canton_transactions must be at least 1".into());
        }
        if self.graph.overview_above == 0 {
            return Err("overview_above must be at least 1".into());
        }
        if self.graph.refresh_seconds == 0 {
            return Err("refresh_seconds must be at least 1".into());
        }
        if !LOG_LEVELS.contains(&self.logging.level.as_str()) {
            return Err(format!("log level must be one of: {}", LOG_LEVELS.join(", ")).into());
        }
        Ok(())
    }
}
