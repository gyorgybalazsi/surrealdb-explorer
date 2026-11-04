//! Database connection.

use surrealdb::Surreal;

// For a SurrealKV database on disk
use surrealdb::engine::local::{Db, SurrealKv};

use crate::{Error, key};

/// The embedded database handle used throughout the program.
pub type Database = Surreal<Db>;

/// Opens the on-disk database in `./data` and selects the `main` namespace and database.
///
/// The database is encrypted at rest with the key from the macOS Keychain
/// (see [`crate::key`]). A database folder created without encryption cannot be opened.
pub async fn connect() -> Result<Database, Error> {
    key::install()?;

    // Create database connection using SurrealKV
    let db = Surreal::new::<SurrealKv>("data").await?;

    // Select a specific namespace / database
    db.use_ns("main").use_db("main").await?;

    Ok(db)
}
