//! Turns any SurrealDB database into nodes and edges, without assuming a schema.
//!
//! - Records in relation tables (or with `in` and `out` fields) become edges.
//! - All other records become nodes.

mod model;
pub mod neighbourhood;
mod records;
pub(crate) mod tables;

pub use model::{GraphData, GraphEdge, GraphNode};

use surrealdb::types::Value;

use crate::db::Database;

/// Loads every table in the database as graph data, at most
/// `max_per_table` records per table.
pub async fn load(db: &Database, max_per_table: usize) -> surrealdb::Result<GraphData> {
    let started = std::time::Instant::now();
    let mut data = GraphData::default();
    for table in tables::list(db).await? {
        let mut rows: Vec<Value> = db
            .query("SELECT * FROM type::table($table) LIMIT $limit")
            .bind(("table", table.name.clone()))
            .bind(("limit", max_per_table as i64 + 1))
            .await?
            .take(0)?;
        if rows.len() > max_per_table {
            rows.truncate(max_per_table);
            tracing::warn!(table = %table.name, max_per_table, "table has more records than the limit; showing the first ones");
            data.truncated.push(table.name.clone());
        }
        for row in rows {
            records::add(&mut data, &table, row);
        }
    }
    tracing::info!(
        records = data.nodes.len(),
        relations = data.edges.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "graph loaded"
    );
    Ok(data)
}
