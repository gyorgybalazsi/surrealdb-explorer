//! Database connection.

use surrealdb::Surreal;

// For a SurrealKV database on disk
use surrealdb::engine::local::{Db, SurrealKv};

use crate::config::DatabaseConfig;
use crate::{Error, key};

/// The embedded database handle used throughout the program.
pub type Database = Surreal<Db>;

/// Opens the SurrealKV database at the configured path and selects the
/// configured namespace and database.
///
/// With `encryption` on, the database is encrypted at rest with the key from
/// the macOS Keychain (see [`crate::key`]). A database folder must be opened
/// the way it was created: encrypted folders need the key, and unencrypted
/// ones can't be opened with it.
pub async fn connect(config: &DatabaseConfig) -> Result<Database, Error> {
    if config.encryption {
        key::install(&config.keychain_service)?;
    }

    let db = Surreal::new::<SurrealKv>(config.path.as_str()).await.map_err(|e| {
        tracing::error!(path = %config.path, error = %e, "could not open the database");
        e
    })?;
    db.use_ns(&config.namespace).use_db(&config.database).await?;

    tracing::info!(
        path = %config.path,
        namespace = %config.namespace,
        database = %config.database,
        encryption = config.encryption,
        "database opened"
    );
    Ok(db)
}
