//! The part of the graph around one record: the record, the relations that
//! touch it, the records at their other ends, and so on, a few steps out.

use std::collections::BTreeSet;

use surrealdb::types::Value;

use super::model::GraphData;
use super::records;
use super::tables::{self, Table};
use crate::db::Database;

/// Collects the records and relations within `depth` steps of `start`
/// (a record id such as `person:alice`), following relations both ways.
/// Stops adding relations once there are `max_edges` of them, and then sets
/// `truncated` to the tables it had to cut.
pub async fn load(db: &Database, start: &str, depth: usize, max_edges: usize) -> surrealdb::Result<GraphData> {
    let relations: Vec<Table> = tables::list(db).await?.into_iter().filter(|t| t.is_relation).collect();

    let mut data = GraphData::default();
    let mut seen_nodes: BTreeSet<String> = BTreeSet::from([start.to_string()]);
    let mut seen_edges: BTreeSet<String> = BTreeSet::new();
    let mut frontier: Vec<String> = vec![start.to_string()];

    for _ in 0..depth {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for table in &relations {
            let room = max_edges.saturating_sub(data.edges.len());
            if room == 0 {
                if !data.truncated.contains(&table.name) {
                    data.truncated.push(table.name.clone());
                }
                continue;
            }
            let rows: Vec<Value> = db
                .query(
                    "LET $ids = $frontier.map(|$id| type::record($id)); \
                     SELECT * FROM type::table($table) WHERE in IN $ids OR out IN $ids LIMIT $limit",
                )
                .bind(("frontier", frontier.clone()))
                .bind(("table", table.name.clone()))
                .bind(("limit", room as i64 + 1))
                .await?
                .take(1)?;
            if rows.len() > room {
                data.truncated.push(table.name.clone());
            }
            let mut found = GraphData::default();
            for row in rows.into_iter().take(room) {
                records::add(&mut found, table, row);
            }
            for edge in found.edges {
                if !seen_edges.insert(edge.id.clone()) {
                    continue;
                }
                for end in [&edge.from, &edge.to] {
                    if seen_nodes.insert(end.clone()) {
                        next.push(end.clone());
                    }
                }
                data.edges.push(edge);
            }
        }
        frontier = next;
    }

    // Read the records themselves, start included.
    let ids: Vec<String> = seen_nodes.into_iter().collect();
    let rows: Vec<Value> = db
        .query("SELECT * FROM $ids.map(|$id| type::record($id))")
        .bind(("ids", ids))
        .await?
        .take(0)?;
    for row in rows {
        let table = match &row {
            Value::Object(o) => match o.get("id") {
                Some(Value::RecordId(id)) => id.table.to_string(),
                _ => continue,
            },
            _ => continue,
        };
        let table = Table { name: table, is_relation: false, definition: String::new() };
        records::add(&mut data, &table, row);
    }
    Ok(data)
}
