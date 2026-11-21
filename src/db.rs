//! Database connection: the app is one client of a SurrealDB server, which
//! other programs (Studio, an import job, …) can use at the same time.

use surrealdb::Surreal;
use surrealdb::engine::any::{self, Any};
use surrealdb::opt::auth::{Database as DatabaseUser, Namespace as NamespaceUser, Root};

use crate::Error;
use crate::config::{DatabaseConfig, UserLevel};
use crate::secret;

/// The database handle used throughout the program.
pub type Database = Surreal<Any>;

/// Connects and signs in to the configured server, and selects the
/// configured namespace and database.
pub async fn connect(config: &DatabaseConfig) -> Result<Database, Error> {
    let db = connect_to_server(config).await.map_err(|e| {
        tracing::error!(location = %config.location(), error = %e, "could not open the database");
        e
    })?;
    db.use_ns(&config.namespace).use_db(&config.database).await?;

    tracing::info!(
        location = %config.location(),
        namespace = %config.namespace,
        database = %config.database,
        "database opened"
    );
    Ok(db)
}

async fn connect_to_server(config: &DatabaseConfig) -> Result<Database, Error> {
    let password = if secret::is_1password_reference(&config.password) {
        let reference = config.password.clone();
        tokio::task::spawn_blocking(move || secret::read_1password(&reference)).await??
    } else {
        config.password.clone()
    };
    let db = any::connect(config.server.trim()).await?;
    let username = config.username.trim().to_string();
    let signed_in = match config.user_level {
        UserLevel::Root => db.signin(Root { username, password }).await,
        UserLevel::Namespace => {
            db.signin(NamespaceUser { namespace: config.namespace.clone(), username, password }).await
        }
        UserLevel::Database => {
            db.signin(DatabaseUser {
                namespace: config.namespace.clone(),
                database: config.database.clone(),
                username,
                password,
            })
            .await
        }
    };
    signed_in.map_err(|e| {
        format!(
            "could not sign in to {} as {} ({}): {e}",
            config.server,
            config.username,
            config.user_level.to_string().to_lowercase()
        )
    })?;
    Ok(db)
}

/// Where the queries a person types (in the query panel) or Claude Code sends
/// (over MCP) run.
#[derive(Clone, Debug)]
pub enum Queries {
    /// No viewer user is set: they run on the main connection, and the app
    /// refuses the ones that look like writes before sending them.
    Main,
    /// Signed in as the read-only viewer user: the server refuses writes.
    Viewer { db: Database, username: String },
    /// A viewer user is set but couldn't sign in. Queries don't run at all,
    /// rather than falling back to a user that can write.
    Failed(String),
}

impl Queries {
    /// Signs in as the viewer user `config` names, if any.
    pub async fn connect(config: &DatabaseConfig) -> Queries {
        let Some(viewer) = config.viewer() else { return Queries::Main };
        match connect(&viewer).await {
            Ok(db) => Queries::Viewer { db, username: viewer.username },
            Err(e) => Queries::Failed(format!("could not sign in as the read-only viewer user: {e}")),
        }
    }

    /// The connection to run a user's query on: the viewer's, or `main` when
    /// none is set.
    pub fn pick(&self, main: &Database) -> Result<Database, String> {
        match self {
            Queries::Main => Ok(main.clone()),
            Queries::Viewer { db, .. } => Ok(db.clone()),
            Queries::Failed(e) => Err(e.clone()),
        }
    }

    /// Whether the server enforces read-only (a viewer user is signed in).
    pub fn enforced(&self) -> bool {
        matches!(self, Queries::Viewer { .. })
    }

    /// Closes the viewer's connection, if there is one.
    pub async fn close(self) {
        if let Queries::Viewer { db, .. } = self
            && let Err(e) = db.shutdown().await
        {
            tracing::warn!(error = %e, "could not close the viewer connection");
        }
    }
}

/// Shuts the connection down. Other copies of the handle stop working.
pub async fn close(db: Database) -> Result<(), Error> {
    db.shutdown().await?;
    drop(db);
    tracing::info!("database closed");
    Ok(())
}
