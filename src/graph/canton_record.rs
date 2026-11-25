//! Following one record of a Canton event graph through the ledger: the
//! transaction it is in, the event that archived a contract, and a
//! contract's whole lifecycle from its create to its archive.

use std::collections::BTreeSet;

use surrealdb::types::Value;

use super::result::{ResultGraph, load_result};
use super::tables::{self, Table};
use crate::canton::NodeKind;
use crate::db::Database;

/// A contract's lifecycle, as [`lifecycle`] found it.
#[derive(Debug, Clone)]
pub struct Lifecycle {
    /// The contract's `created` record.
    pub contract: String,
    /// The events that exercise, fetch or archive it.
    pub events: usize,
    /// The contract, those events, and the transactions they are in.
    pub graph: ResultGraph,
}

/// The transaction record `id` is in (or is), with every event at its
/// offset: at most `limit` records. A record with no offset, such as a
/// contract from the active contract set, comes alone.
pub async fn transaction_of(db: &Database, id: &str, limit: usize) -> surrealdb::Result<ResultGraph> {
    let tables = tables::list(db).await?;
    let mut ids = vec![id.to_string()];
    if let Some(offset) = offsets(db, &ids).await?.first().copied() {
        ids.extend(at_offset(db, &tables, offset).await?);
    }
    load_result(db, &ids, &[], limit, true).await
}

/// The event that archived contract `id` (a `created` record), and the
/// transaction it is in, with the contract: `None` while the contract is
/// active, or when `id` isn't a contract.
pub async fn archiving_event(db: &Database, id: &str, limit: usize) -> surrealdb::Result<Option<(String, ResultGraph)>> {
    if kind_of(id) != NodeKind::Created {
        return Ok(None);
    }
    let Some(exercise) = ends(db, id, Direction::In, &["consumes"]).await?.into_iter().next() else {
        return Ok(None);
    };
    let tables = tables::list(db).await?;
    let mut ids = vec![exercise.clone()];
    if let Some(offset) = offsets(db, &ids).await?.first().copied() {
        ids.extend(at_offset(db, &tables, offset).await?);
    }
    ids.push(id.to_string());
    Ok(Some((exercise.clone(), load_result(db, &ids, &[], limit, true).await?)))
}

/// The lifecycle of the contract `id` is (a `created` record) or acts on
/// (an exercise or fetch): its create, every exercise and fetch of it up to
/// the one that archives it, and the transactions those are in. At most
/// `limit` records, the contract and its events first. `None` when `id`
/// is neither, or its contract isn't in the database.
pub async fn lifecycle(db: &Database, id: &str, limit: usize) -> surrealdb::Result<Option<Lifecycle>> {
    let contract = match kind_of(id) {
        NodeKind::Created => id.to_string(),
        NodeKind::Exercised | NodeKind::Fetched => match ends(db, id, Direction::Out, &["target"]).await?.into_iter().next() {
            Some(c) => c,
            None => return Ok(None),
        },
        _ => return Ok(None),
    };
    let mut events = ends(db, &contract, Direction::In, &["target", "consumes"]).await?;
    events.sort();
    events.dedup();

    let mut answer = vec![contract.clone()];
    answer.extend(events.iter().cloned());
    let tables = tables::list(db).await?;
    let mut context = Vec::new();
    let offsets: BTreeSet<i64> = offsets(db, &answer).await?.into_iter().collect();
    for offset in offsets {
        context.extend(at_offset(db, &tables, offset).await?);
    }
    let graph = load_result(db, &answer, &context, limit, true).await?;
    if !graph.data.nodes.iter().any(|n| n.id == contract) {
        return Ok(None);
    }
    Ok(Some(Lifecycle { contract, events: events.len(), graph }))
}

/// What a record is, from the table in its id.
fn kind_of(id: &str) -> NodeKind {
    NodeKind::from_table(id.split_once(':').map(|(t, _)| t).unwrap_or(id))
}

#[derive(Clone, Copy)]
enum Direction {
    /// Relations that start at the record: the records they point to.
    Out,
    /// Relations that end at the record: the records they come from.
    In,
}

/// The records at the other end of `id`'s relations in these tables
/// (matched case-insensitively), as ids.
async fn ends(db: &Database, id: &str, direction: Direction, relations: &[&str]) -> surrealdb::Result<Vec<String>> {
    let sql = match direction {
        Direction::Out => {
            "LET $nodes = [type::record($id)]; \
             SELECT VALUE <string> out FROM array::flatten((SELECT VALUE ->? FROM $nodes)) \
             WHERE string::lowercase(record::tb(id)) IN $relations;"
        }
        Direction::In => {
            "LET $nodes = [type::record($id)]; \
             SELECT VALUE <string> in FROM array::flatten((SELECT VALUE <-? FROM $nodes)) \
             WHERE string::lowercase(record::tb(id)) IN $relations;"
        }
    };
    let relations: Vec<String> = relations.iter().map(|r| r.to_string()).collect();
    let rows: Vec<Value> = db.query(sql).bind(("id", id.to_string())).bind(("relations", relations)).await?.take(1)?;
    Ok(strings(rows))
}

/// The ledger offsets of these records, leaving out those without one
/// (contracts from the active contract set have offset -1).
async fn offsets(db: &Database, ids: &[String]) -> surrealdb::Result<Vec<i64>> {
    let rows: Vec<Value> = db
        .query("SELECT VALUE offset FROM $ids.map(|$i| type::record($i))")
        .bind(("ids", ids.to_vec()))
        .await?
        .take(0)?;
    Ok(rows.into_iter().filter_map(|v| v.into_json_value().as_i64()).filter(|&o| o >= 0).collect())
}

/// The transaction at `offset` and every event in it, the transaction first.
async fn at_offset(db: &Database, tables: &[Table], offset: i64) -> surrealdb::Result<Vec<String>> {
    let mut ids = Vec::new();
    let wanted = [NodeKind::Transaction, NodeKind::Created, NodeKind::Exercised, NodeKind::Fetched];
    for kind in wanted {
        for t in tables.iter().filter(|t| !t.is_relation && NodeKind::from_table(&t.name) == kind) {
            let rows: Vec<Value> = db
                .query("SELECT VALUE <string> id FROM type::table($t) WHERE offset = $o")
                .bind(("t", t.name.clone()))
                .bind(("o", offset))
                .await?
                .take(0)?;
            ids.extend(strings(rows));
        }
    }
    Ok(ids)
}

fn strings(rows: Vec<Value>) -> Vec<String> {
    rows.into_iter().filter_map(|v| v.into_json_value().as_str().map(String::from)).collect()
}
