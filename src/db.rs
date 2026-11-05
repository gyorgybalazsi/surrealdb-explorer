//! Database connection.

use surrealdb::Surreal;

// For a SurrealKV database on disk
use surrealdb::engine::local::{Db, SurrealKv};

use crate::Error;
use crate::config::DatabaseConfig;

/// The embedded database handle used throughout the program.
pub type Database = Surreal<Db>;

/// Opens the SurrealKV database at the configured path and selects the
/// configured namespace and database.
pub async fn connect(config: &DatabaseConfig) -> Result<Database, Error> {
    let db = Surreal::new::<SurrealKv>(config.path.as_str()).await.map_err(|e| {
        tracing::error!(path = %config.path, error = %e, "could not open the database");
        e
    })?;
    db.use_ns(&config.namespace).use_db(&config.database).await?;

    tracing::info!(
        path = %config.path,
        namespace = %config.namespace,
        database = %config.database,
        "database opened"
    );
    Ok(db)
}

/// Shuts the database down and releases its folder, so it can be opened
/// again in this process. Just dropping the handle keeps the folder locked.
/// Other copies of the handle stop working.
pub async fn close(db: Database) -> Result<(), Error> {
    db.shutdown().await?;
    drop(db);
    tracing::info!("database closed");
    Ok(())
}
