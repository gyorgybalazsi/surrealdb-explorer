//! Discovering the tables in the database.

use surrealdb::types::Value;

use crate::db::Database;

pub struct Table {
    pub name: String,
    /// Defined with `TYPE RELATION`, so its records are edges.
    pub is_relation: bool,
}

/// Lists all tables in the current database, sorted by name.
pub async fn list(db: &Database) -> surrealdb::Result<Vec<Table>> {
    let info: Value = db.query("INFO FOR DB").await?.take(0)?;
    let Value::Object(info) = info else {
        return Ok(Vec::new());
    };
    let Some(Value::Object(definitions)) = info.get("tables") else {
        return Ok(Vec::new());
    };

    let mut tables: Vec<Table> = definitions
        .iter()
        .map(|(name, definition)| Table {
            name: name.clone(),
            is_relation: matches!(definition, Value::String(d) if d.contains("TYPE RELATION")),
        })
        .collect();
    tables.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tables)
}
