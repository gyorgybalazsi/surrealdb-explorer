//! Database connection.

use surrealdb::Surreal;

// For a SurrealKV database on disk
use surrealdb::engine::local::{Db, SurrealKv};

/// The embedded database handle used throughout the program.
pub type Database = Surreal<Db>;

/// Opens the on-disk database in `./data` and selects the `main` namespace and database.
pub async fn connect() -> surrealdb::Result<Database> {
    // Create database connection using SurrealKV
    let db = Surreal::new::<SurrealKv>("data").await?;

    // Select a specific namespace / database
    db.use_ns("main").use_db("main").await?;

    Ok(db)
}
