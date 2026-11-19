//! Changing the whole database: loading the sample data, and erasing it.

use crate::Error;
use surrealdb::types::Value;

use crate::config::{DatabaseConfig, UserLevel};
use crate::db::Database;

/// The sample data, from `src/seed.surql`.
pub const SAMPLE_DATA: &str = include_str!("seed.surql");

/// Adds the sample people and their `knows` relations. Deletes all
/// existing `knows` relations first; other data is kept.
pub async fn load_sample_data(db: &Database) -> Result<(), Error> {
    db.query(SAMPLE_DATA).await?.check()?;
    tracing::info!("sample data loaded");
    Ok(())
}

/// The sample Canton event graph, from `src/canton_sample.surql`.
pub const CANTON_SAMPLE: &str = include_str!("canton_sample.surql");

/// Adds a small Canton event graph: parties, transactions, created and
/// exercised events and their relations, as the Canton toolbox indexes them.
/// Recreates the relations that start at the sample's records; other data is kept.
pub async fn load_canton_sample(db: &Database) -> Result<(), Error> {
    db.query(CANTON_SAMPLE).await?.check()?;
    tracing::info!("Canton sample loaded");
    Ok(())
}

/// Removes everything in the configured database: all tables, records and
/// definitions. The empty database is then defined again, so the app can
/// keep using it.
///
/// A database user (as the Canton syncer's) can't remove its own database,
/// so for one each table, function, parameter, analyzer and sequence is
/// removed instead. Its users and access methods stay: the app signs in
/// with them.
pub async fn erase(db: &Database, config: &DatabaseConfig) -> Result<(), Error> {
    let statements = match config.user_level {
        UserLevel::Database => remove_definitions(db).await?,
        _ => {
            let name = escape_ident(&config.database);
            format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")
        }
    };
    if !statements.is_empty() {
        db.query(statements).await?.check()?;
    }
    tracing::warn!(namespace = %config.namespace, database = %config.database, "database erased");
    Ok(())
}

/// `REMOVE …` statements for what the database defines, except users and
/// access methods.
async fn remove_definitions(db: &Database) -> Result<String, Error> {
    let info = db.query("INFO FOR DB").await?.take::<Value>(0)?.into_json_value();
    let names = |kind: &str| -> Vec<String> {
        info.get(kind).and_then(|v| v.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default()
    };
    let mut sql = String::new();
    for table in names("tables") {
        sql.push_str(&format!("REMOVE TABLE IF EXISTS {};\n", escape_ident(&table)));
    }
    for function in names("functions") {
        // Function names have `::` between their parts, each quoted on its own.
        let path: Vec<String> = function.trim_start_matches("fn::").split("::").map(escape_ident).collect();
        sql.push_str(&format!("REMOVE FUNCTION IF EXISTS fn::{};\n", path.join("::")));
    }
    for param in names("params") {
        sql.push_str(&format!("REMOVE PARAM IF EXISTS ${};\n", escape_ident(&param)));
    }
    for analyzer in names("analyzers") {
        sql.push_str(&format!("REMOVE ANALYZER IF EXISTS {};\n", escape_ident(&analyzer)));
    }
    for sequence in names("sequences") {
        sql.push_str(&format!("REMOVE SEQUENCE IF EXISTS {};\n", escape_ident(&sequence)));
    }
    Ok(sql)
}

/// Quotes a name for use as a SurrealQL identifier: `⟨name⟩`.
fn escape_ident(name: &str) -> String {
    format!("⟨{}⟩", name.replace('\\', "\\\\").replace('⟩', "\\⟩"))
}
