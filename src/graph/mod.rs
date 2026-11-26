//! Turns any SurrealDB database into nodes and edges, without assuming a schema.
//!
//! - Records in relation tables (or with `in` and `out` fields) become edges.
//! - All other records become nodes.

mod canton_record;
mod canton_window;
mod explore;
mod model;
mod records;
mod result;
pub(crate) mod tables;

pub use model::{GraphData, GraphEdge, GraphNode};
pub use canton_record::{Lifecycle, archiving_event, lifecycle, transaction_of};
pub use canton_window::{Loaded as CantonWindowLoaded, Window as CantonWindow, load_window as load_canton_window};
pub use explore::{TABLE_NODE, neighbours, overview, record_count, table_records};
pub use result::{ResultGraph, load_result};

use surrealdb::types::Value;

use crate::db::Database;

/// What auto refresh watches for changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watch {
    /// The records the whole-database view draws: the first `per_table`
    /// (and one more, to see a table go over the limit) of each table.
    Tables { per_table: usize },
    /// A Canton ledger written by the syncer, or another indexer.
    Canton,
}

/// A cheap fingerprint of what `watch` covers: it changes when records are
/// added or removed, not when a record's fields change. Its cost doesn't grow
/// with the size of the database:
///
/// - for tables, only the ids of the records the view draws are read;
/// - for a Canton ledger, the syncer's `sync_state` (which it updates with
///   every batch) is read, or else the newest transaction's offset.
pub async fn change_marker(db: &Database, watch: Watch) -> surrealdb::Result<serde_json::Value> {
    let tables = tables::list(db).await?;
    let names: Vec<String> = tables.iter().map(|t| t.name.clone()).collect();
    let has = |name: &str| names.iter().any(|t| t == name);
    let marker: Value = match watch {
        Watch::Canton if has("sync_state") => db.query("SELECT * FROM sync_state").await?.take(0)?,
        Watch::Canton if has("transaction") => db
            .query("SELECT VALUE offset FROM transaction ORDER BY offset DESC LIMIT 1")
            .await?
            .take(0)?,
        Watch::Canton => Value::None,
        Watch::Tables { per_table } => {
            if tables.is_empty() {
                Value::None
            } else {
                let sql: String = (0..tables.len())
                    .map(|i| format!("RETURN crypto::sha256(<string> (SELECT VALUE id FROM type::table($t{i}) LIMIT $n));"))
                    .collect();
                let mut query = db.query(sql).bind(("n", per_table as i64 + 1));
                for (i, table) in tables.iter().enumerate() {
                    query = query.bind((format!("t{i}"), table.name.clone()));
                }
                let mut response = query.await?;
                let mut hashes = Vec::with_capacity(tables.len());
                for i in 0..tables.len() {
                    hashes.push(response.take::<Value>(i)?.into_json_value());
                }
                return Ok(serde_json::json!({ "tables": names, "ids": hashes }));
            }
        }
    };
    // A table added or removed counts as a change too.
    Ok(serde_json::json!({ "tables": names, "marker": marker.into_json_value() }))
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
