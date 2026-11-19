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

/// Shuts the connection down. Other copies of the handle stop working.
pub async fn close(db: Database) -> Result<(), Error> {
    db.shutdown().await?;
    drop(db);
    tracing::info!("database closed");
    Ok(())
}
