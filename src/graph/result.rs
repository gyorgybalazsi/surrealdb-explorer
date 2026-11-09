//! Building a graph from a query's result: exactly the records and relations
//! it returned, read from the database by id, plus the records at both ends
//! of each relation.

use std::collections::{HashMap, HashSet};

use surrealdb::types::{ToSql, Value};

use super::model::GraphData;
use super::records;
use super::tables::{self, Table};
use crate::db::Database;

/// A query result as a graph.
#[derive(Debug, Clone, Default)]
pub struct ResultGraph {
    pub data: GraphData,
    /// Records the query itself returned (the answer) that are drawn.
    pub answer: HashSet<String>,
    /// Relations the query or its context query returned that are drawn.
    pub context_edges: HashSet<String>,
    /// Records and relations the queries returned.
    pub found: usize,
    /// How many of those weren't drawn because of the limit.
    pub left_out: usize,
}

/// Builds the graph for `answer` (ids the query returned) and `context` (ids
/// its context query returned). At most `limit` records are drawn, answers
/// first. With `connect`, all relations between the drawn records are added
/// too, not only the ones the queries returned.
pub async fn load_result(
    db: &Database,
    answer: &[String],
    context: &[String],
    limit: usize,
    connect: bool,
) -> surrealdb::Result<ResultGraph> {
    let started = std::time::Instant::now();
    let tables: HashMap<String, Table> =
        tables::list(db).await?.into_iter().map(|t| (t.name.clone(), t)).collect();

    // Only strings that name a record of an existing table (results can hold
    // other text with a colon, like times and links), each once.
    let mut seen = HashSet::new();
    let wanted: Vec<String> = answer
        .iter()
        .chain(context)
        .filter(|id| id.split_once(':').is_some_and(|(table, _)| tables.contains_key(table)))
        .filter(|id| seen.insert(id.to_string()))
        .cloned()
        .collect();
    let found = wanted.len();
    let taken: Vec<String> = wanted.into_iter().take(limit).collect();

    let mut data = GraphData::default();
    add_rows(&mut data, &tables, fetch(db, &taken).await?);

    // Relations bring the records at both ends, as long as there's room.
    let have: HashSet<String> = data.nodes.iter().map(|n| n.id.clone()).collect();
    let mut ends: Vec<String> = Vec::new();
    for e in &data.edges {
        for end in [&e.from, &e.to] {
            if !have.contains(end) && !ends.contains(end) {
                ends.push(end.clone());
            }
        }
    }
    let room = limit.saturating_sub(data.nodes.len());
    ends.truncate(room);
    add_rows(&mut data, &tables, fetch(db, &ends).await?);

    let answer_ids: HashSet<&String> = answer.iter().collect();
    let drawn_answer: HashSet<String> = data
        .nodes
        .iter()
        .map(|n| &n.id)
        .chain(data.edges.iter().map(|e| &e.id))
        .filter(|id| answer_ids.contains(id))
        .cloned()
        .collect();
    let context_edges: HashSet<String> = data.edges.iter().map(|e| e.id.clone()).collect();

    if connect && !data.nodes.is_empty() {
        let nodes: Vec<String> = data.nodes.iter().map(|n| n.id.clone()).collect();
        let rows: Vec<Value> = db
            .query(
                "LET $nodes = $ids.map(|$i| type::record($i));
                 SELECT * FROM array::flatten((SELECT VALUE ->? FROM $nodes)) WHERE out IN $nodes;",
            )
            .bind(("ids", nodes))
            .await?
            .take(1)?;
        let rows = rows.into_iter().filter(|row| !context_edges.contains(&id_of(row))).collect();
        add_rows(&mut data, &tables, rows);
    }

    tracing::info!(
        found,
        drawn_records = data.nodes.len(),
        drawn_relations = data.edges.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "query result loaded as a graph"
    );
    Ok(ResultGraph { data, answer: drawn_answer, context_edges, found, left_out: found.saturating_sub(taken.len()) })
}

/// Reads records by id; ids that don't exist are skipped.
async fn fetch(db: &Database, ids: &[String]) -> surrealdb::Result<Vec<Value>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    db.query("SELECT * FROM $ids.map(|$i| type::record($i))")
        .bind(("ids", ids.to_vec()))
        .await?
        .take(0)
}

fn add_rows(data: &mut GraphData, tables: &HashMap<String, Table>, rows: Vec<Value>) {
    for row in rows {
        let id = id_of(&row);
        let Some(table) = id.split_once(':').and_then(|(t, _)| tables.get(t)) else { continue };
        records::add(data, table, row);
    }
}

fn id_of(row: &Value) -> String {
    match row {
        Value::Object(o) => match o.get("id") {
            Some(Value::RecordId(id)) => id.to_sql(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}
