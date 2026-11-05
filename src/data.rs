//! Changing the whole database: loading the sample data, and erasing it.

use crate::Error;
use crate::config::DatabaseConfig;
use crate::db::Database;

/// The sample data, from `src/seed.surql`.
pub const SAMPLE_DATA: &str = include_str!("seed.surql");

/// Adds the sample people, company and `knows` relations. Deletes all
/// existing `knows` relations first; other data is kept.
pub async fn load_sample_data(db: &Database) -> Result<(), Error> {
    db.query(SAMPLE_DATA).await?.check()?;
    tracing::info!("sample data loaded");
    Ok(())
}

/// Removes everything in the configured database: all tables, records and
/// definitions. The empty database is then defined again, so the app can
/// keep using it.
pub async fn erase(db: &Database, config: &DatabaseConfig) -> Result<(), Error> {
    let name = escape_ident(&config.database);
    db.query(format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"))
        .await?
        .check()?;
    tracing::warn!(namespace = %config.namespace, database = %config.database, "database erased");
    Ok(())
}

/// Quotes a name for use as a SurrealQL identifier: `⟨name⟩`.
fn escape_ident(name: &str) -> String {
    format!("⟨{}⟩", name.replace('\\', "\\\\").replace('⟩', "\\⟩"))
}
