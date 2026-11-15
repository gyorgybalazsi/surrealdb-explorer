//! Loading a window of a Canton ledger's history: a range of transactions
//! with their events, instead of every record (a long history has far too
//! many to draw).

use surrealdb::types::Value;

use super::model::GraphData;
use super::result::load_result;
use super::tables;
use crate::canton::NodeKind;
use crate::db::Database;

/// Which transactions to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// The newest `n`.
    Latest(usize),
    /// The `n` before an offset.
    Before(i64, usize),
    /// The `n` after an offset.
    After(i64, usize),
    /// Every transaction in an offset range, both ends included.
    Range(i64, i64),
}

/// A loaded window: the records and relations, and the offsets they cover
/// (`None` when there were no transactions in it).
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub data: GraphData,
    pub offsets: Option<(i64, i64)>,
    /// How many records had to be left out because of `limit`.
    pub left_out: usize,
}

/// Loads the transactions in `window`, every event at their offsets, all
/// parties, the earlier contracts their events act on, and up to
/// `other_limit` records of each table that isn't part of the event graph.
/// At most `limit` records in all.
pub async fn load_window(db: &Database, window: Window, other_limit: usize, limit: usize) -> surrealdb::Result<Loaded> {
    let tables = tables::list(db).await?;
    let has = |name: &str| tables.iter().any(|t| t.name.eq_ignore_ascii_case(name));

    let offsets = if has("transaction") { offset_range(db, window).await? } else { None };

    // Collected by kind, so when there are more than `limit` records the
    // ones left out are the least important: the transactions and their
    // events first, then the earlier contracts they act on, then parties and
    // other tables.
    let (mut roots, mut events, mut parties, mut others) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for t in tables.iter().filter(|t| !t.is_relation) {
        let kind = NodeKind::from_table(&t.name);
        let rows: Vec<Value> = match (kind, offsets) {
            (NodeKind::Party, _) => db.query("SELECT VALUE <string> id FROM type::table($t)").bind(("t", t.name.clone())).await?.take(0)?,
            (NodeKind::Other, _) => db
                .query("SELECT VALUE <string> id FROM type::table($t) LIMIT $n")
                .bind(("t", t.name.clone()))
                .bind(("n", other_limit as i64))
                .await?
                .take(0)?,
            (_, Some((from, to))) => db
                .query("SELECT VALUE <string> id FROM type::table($t) WHERE offset >= $from AND offset <= $to")
                .bind(("t", t.name.clone()))
                .bind(("from", from))
                .bind(("to", to))
                .await?
                .take(0)?,
            (_, None) => Vec::new(),
        };
        let bucket = match kind {
            NodeKind::Transaction | NodeKind::Reassignment | NodeKind::Topology => &mut roots,
            NodeKind::Party => &mut parties,
            NodeKind::Other => &mut others,
            _ => &mut events,
        };
        bucket.extend(strings(rows));
    }
    let mut ids: Vec<String> = roots;
    ids.append(&mut events);

    // Contracts created before the window (or loaded from the ACS) that its
    // events exercise or archive.
    if offsets.is_some() && (has("target") || has("consumes")) {
        let rows: Vec<Value> = db
            .query(
                "LET $nodes = $ids.map(|$i| type::record($i)); \
                 SELECT VALUE <string> out FROM array::flatten((SELECT VALUE ->? FROM $nodes)) \
                 WHERE record::tb(id) IN ['target', 'consumes', 'TARGET', 'CONSUMES'];",
            )
            .bind(("ids", ids.clone()))
            .await?
            .take(1)?;
        let have: std::collections::HashSet<String> = ids.iter().cloned().collect();
        let mut earlier: Vec<String> = strings(rows).into_iter().filter(|id| !have.contains(id)).collect();
        earlier.sort();
        earlier.dedup();
        ids.append(&mut earlier);
    }
    ids.append(&mut parties);
    ids.append(&mut others);

    let result = load_result(db, &ids, &[], limit, true).await?;
    Ok(Loaded { data: result.data, offsets, left_out: result.left_out })
}

/// The lowest and highest offset of the transactions in `window`.
async fn offset_range(db: &Database, window: Window) -> surrealdb::Result<Option<(i64, i64)>> {
    let (sql, a, n) = match window {
        Window::Range(from, to) => {
            // The offsets actually there, so an open end shows a real number.
            let sql = "SELECT VALUE offset FROM transaction WHERE offset >= $a AND offset <= $b ORDER BY offset ASC LIMIT 1;
                       SELECT VALUE offset FROM transaction WHERE offset >= $a AND offset <= $b ORDER BY offset DESC LIMIT 1;";
            let mut res = db.query(sql).bind(("a", from.min(to))).bind(("b", from.max(to))).await?;
            let first: Vec<Value> = res.take(0)?;
            let last: Vec<Value> = res.take(1)?;
            let get = |v: Vec<Value>| v.into_iter().next().and_then(|v| v.into_json_value().as_i64());
            return Ok(get(first).zip(get(last)));
        }
        Window::Latest(n) => ("SELECT VALUE offset FROM transaction ORDER BY offset DESC LIMIT $n", 0, n),
        Window::Before(o, n) => ("SELECT VALUE offset FROM transaction WHERE offset < $a ORDER BY offset DESC LIMIT $n", o, n),
        Window::After(o, n) => ("SELECT VALUE offset FROM transaction WHERE offset > $a ORDER BY offset ASC LIMIT $n", o, n),
    };
    let offsets: Vec<Value> = db.query(sql).bind(("a", a)).bind(("n", n as i64)).await?.take(0)?;
    let offsets: Vec<i64> = offsets.into_iter().filter_map(|v| v.into_json_value().as_i64()).collect();
    Ok(match (offsets.iter().min(), offsets.iter().max()) {
        (Some(&lo), Some(&hi)) => Some((lo, hi)),
        _ => None,
    })
}

fn strings(rows: Vec<Value>) -> Vec<String> {
    rows.into_iter().filter_map(|v| v.into_json_value().as_str().map(String::from)).collect()
}
