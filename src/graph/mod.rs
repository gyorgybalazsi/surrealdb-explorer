//! Turns any SurrealDB database into nodes and edges, without assuming a schema.
//!
//! - Records in relation tables (or with `in` and `out` fields) become edges.
//! - All other records become nodes.

mod canton_window;
mod explore;
mod model;
mod records;
mod result;
pub(crate) mod tables;

pub use model::{GraphData, GraphEdge, GraphNode};
pub use canton_window::{Loaded as CantonWindowLoaded, Window as CantonWindow, load_window as load_canton_window};
pub use explore::{TABLE_NODE, neighbours, overview, record_count, table_records};
pub use result::{ResultGraph, load_result};

use surrealdb::types::Value;

use crate::db::Database;

/// How many records each table has, in table order. A cheap check of
/// whether the graph is out of date: it changes when records are added or
/// removed, not when a record's fields change.
pub async fn record_counts(db: &Database) -> surrealdb::Result<Vec<(String, i64)>> {
    let tables = tables::list(db).await?;
    if tables.is_empty() {
        return Ok(Vec::new());
    }
    let sql: String = (0..tables.len())
        .map(|i| format!("SELECT VALUE count() FROM type::table($t{i}) GROUP ALL;"))
        .collect();
    let mut query = db.query(sql);
    for (i, table) in tables.iter().enumerate() {
        query = query.bind((format!("t{i}"), table.name.clone()));
    }
    let mut response = query.await?;
    let mut counts = Vec::with_capacity(tables.len());
    for (i, table) in tables.into_iter().enumerate() {
        let n: Vec<i64> = response.take(i)?;
        counts.push((table.name, n.first().copied().unwrap_or(0)));
    }
    Ok(counts)
}

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
