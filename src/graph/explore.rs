//! Exploring a database too big to draw at once: an overview of its tables,
//! one table's records, and the neighbours of one record.

use surrealdb::types::Value;

use super::model::{GraphData, GraphEdge, GraphNode};
use super::result::{ResultGraph, load_result};
use super::tables;
use crate::db::Database;

/// How many records the database has outside relation tables (those are
/// drawn as nodes, so they decide whether everything can be drawn).
pub async fn record_count(db: &Database) -> surrealdb::Result<usize> {
    let tables = tables::list(db).await?;
    let records: Vec<_> = tables.iter().filter(|t| !t.is_relation).collect();
    if records.is_empty() {
        return Ok(0);
    }
    let sql: String = (0..records.len())
        .map(|i| format!("SELECT VALUE count() FROM type::table($t{i}) GROUP ALL;"))
        .collect();
    let mut query = db.query(sql);
    for (i, t) in records.iter().enumerate() {
        query = query.bind((format!("t{i}"), t.name.clone()));
    }
    let mut response = query.await?;
    let mut total = 0;
    for i in 0..records.len() {
        let n: Vec<i64> = response.take(i)?;
        total += n.first().copied().unwrap_or(0).max(0) as usize;
    }
    Ok(total)
}

/// The prefix of an overview node's id; the rest is the table name.
pub const TABLE_NODE: &str = "⟨table⟩ ";

/// The tables as a graph: one node per table (its data holds `name` and
/// `records`), and one relation per relation table and pair of tables it
/// links, labelled with the relation table and how many relations it holds.
pub async fn overview(db: &Database) -> surrealdb::Result<GraphData> {
    let tables = tables::list(db).await?;
    let mut data = GraphData::default();
    for t in &tables {
        let n: Vec<i64> = db
            .query("SELECT VALUE count() FROM type::table($t) GROUP ALL")
            .bind(("t", t.name.clone()))
            .await?
            .take(0)?;
        let n = n.first().copied().unwrap_or(0);
        if t.is_relation {
            let pairs: Vec<Value> = db
                .query(
                    "SELECT record::tb(in) AS from, record::tb(out) AS to, count() AS n \
                     FROM type::table($t) GROUP BY from, to",
                )
                .bind(("t", t.name.clone()))
                .await?
                .take(0)?;
            for pair in pairs {
                let json = pair.into_json_value();
                let (Some(from), Some(to)) = (json["from"].as_str(), json["to"].as_str()) else { continue };
                let count = json["n"].as_i64().unwrap_or(0);
                data.edges.push(GraphEdge {
                    id: format!("{TABLE_NODE}{}:{from}->{to}", t.name),
                    from: format!("{TABLE_NODE}{from}"),
                    to: format!("{TABLE_NODE}{to}"),
                    label: format!("{} ({count})", t.name),
                    data: serde_json::json!({ "relation": t.name, "from": from, "to": to, "relations": count }),
                });
            }
        } else {
            data.nodes.push(GraphNode {
                id: format!("{TABLE_NODE}{}", t.name),
                table: t.name.clone(),
                data: serde_json::json!({ "name": format!("{} ({n})", t.name), "table": t.name, "records": n }),
            });
        }
    }
    // Relations whose ends are in tables that aren't there (or are relation tables) can't be drawn.
    let names: std::collections::HashSet<String> = data.nodes.iter().map(|n| n.id.clone()).collect();
    data.edges.retain(|e| names.contains(&e.from) && names.contains(&e.to));
    Ok(data)
}

/// Up to `limit` records of one table, with the relations between them.
pub async fn table_records(db: &Database, table: &str, limit: usize) -> surrealdb::Result<ResultGraph> {
    let ids: Vec<Value> = db
        .query("SELECT VALUE <string> id FROM type::table($t) LIMIT $n")
        .bind(("t", table.to_string()))
        .bind(("n", limit as i64))
        .await?
        .take(0)?;
    let ids: Vec<String> = ids.into_iter().filter_map(|v| v.into_json_value().as_str().map(String::from)).collect();
    load_result(db, &ids, &[], limit, true).await
}

/// A record, its relations both ways, and the records at their other ends:
/// at most `limit` records.
pub async fn neighbours(db: &Database, id: &str, limit: usize) -> surrealdb::Result<ResultGraph> {
    let edges: Value = db
        .query(
            "LET $r = type::record($id); \
             RETURN array::flatten([(SELECT VALUE ->? FROM ONLY $r), (SELECT VALUE <-? FROM ONLY $r)]).map(|$e| <string> $e);",
        )
        .bind(("id", id.to_string()))
        .await?
        .take(1)?;
    let mut ids = vec![id.to_string()];
    if let serde_json::Value::Array(edges) = edges.into_json_value() {
        ids.extend(edges.into_iter().filter_map(|e| e.as_str().map(String::from)));
    }
    load_result(db, &ids, &[], limit, false).await
}
