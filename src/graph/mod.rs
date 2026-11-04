//! Turns any SurrealDB database into nodes and edges, without assuming a schema.
//!
//! - Records in relation tables (or with `in` and `out` fields) become edges.
//! - All other records become nodes.

mod model;
mod records;
mod tables;

pub use model::{GraphData, GraphEdge, GraphNode};

use surrealdb::types::Value;

use crate::db::Database;

/// Most records loaded per table, so a big table can't freeze the browser.
pub const MAX_PER_TABLE: usize = 1000;

/// Loads every table in the database as graph data.
pub async fn load(db: &Database) -> surrealdb::Result<GraphData> {
    let mut data = GraphData::default();
    for table in tables::list(db).await? {
        let mut rows: Vec<Value> = db
            .query("SELECT * FROM type::table($table) LIMIT $limit")
            .bind(("table", table.name.clone()))
            .bind(("limit", MAX_PER_TABLE as i64 + 1))
            .await?
            .take(0)?;
        if rows.len() > MAX_PER_TABLE {
            rows.truncate(MAX_PER_TABLE);
            data.truncated.push(table.name.clone());
        }
        for row in rows {
            records::add(&mut data, &table, row);
        }
    }
    Ok(data)
}
