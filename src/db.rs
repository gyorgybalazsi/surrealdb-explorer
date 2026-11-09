//! Database connection: a database folder opened by the app itself, or a
//! SurrealDB server the app connects to as one client among others.

use surrealdb::Surreal;
use surrealdb::engine::any::{self, Any};
use surrealdb::opt::auth::Root;

use crate::Error;
use crate::config::{Connection, DatabaseConfig};
use crate::secret;

/// The database handle used throughout the program, for either connection.
pub type Database = Surreal<Any>;

/// Opens the configured folder or connects to the configured server, and
/// selects the configured namespace and database.
pub async fn connect(config: &DatabaseConfig) -> Result<Database, Error> {
    let opened = match config.connection {
        Connection::Folder => any::connect(format!("surrealkv://{}", config.path)).await.map_err(Error::from),
        Connection::Server => connect_to_server(config).await,
    };
    let db = opened.map_err(|e| {
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
    db.signin(Root { username: config.username.trim().to_string(), password })
        .await
        .map_err(|e| format!("could not sign in to {} as {}: {e}", config.server, config.username))?;
    Ok(db)
}

/// Shuts the connection down. For a folder this releases it, so it can be
/// opened again in this process; just dropping the handle keeps it locked.
/// Other copies of the handle stop working.
pub async fn close(db: Database) -> Result<(), Error> {
    db.shutdown().await?;
    drop(db);
    tracing::info!("database closed");
    Ok(())
}
