//! How long each Canton contract lived: one row per archived contract, with
//! the offsets and times of its create and its archive.
//!
//! Reads the `created` and `exercised` tables the Canton syncer writes.
//! Contracts that are created but not yet archived are kept in the state
//! between steps; a row is only written once a consuming choice archives the
//! contract, so rows never change after they are written.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::builder::{Float64Builder, Int64Builder, StringBuilder, TimestampMicrosecondBuilder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use serde::{Deserialize, Serialize};
use serde_json::Value;

wit_bindgen::generate!({
    world: "analytics-plugin",
    path: "../../wit",
});

use exports::canton::analytics::plugin::{Guest, Output, PluginInfo, QuerySource};

struct Plugin;

export!(Plugin);

/// Creates and consuming choices with an offset in `($from, $to]`. Contracts
/// loaded from the active contract set (ACS) carry the ACS offset.
const QUERY: &str = "\
SELECT contract_id, template_name, offset, created_at FROM created \
    WHERE offset > $from AND offset <= $to ORDER BY offset;
SELECT target_contract_id, choice_name, offset, transaction_effective_at FROM exercised \
    WHERE consuming = true AND offset > $from AND offset <= $to ORDER BY offset;";

/// The syncer records the last offset it fully wrote in `sync_state:stream`;
/// without it (e.g. sample data) the highest transaction offset is used.
/// The runner takes the result of the last statement.
const HIGH_WATER: &str = "\
LET $synced = IF (INFO FOR DB).tables.sync_state != NONE { \
    (SELECT VALUE last_offset FROM ONLY sync_state:stream) } ELSE { NONE };
RETURN $synced ?? math::max(SELECT VALUE offset FROM transaction) ?? 0;";

/// A contract seen created and not yet archived.
#[derive(Serialize, Deserialize)]
struct Open {
    template: Option<String>,
    offset: i64,
    created_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    open: BTreeMap<String, Open>,
}

fn schema() -> SchemaRef {
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
    Arc::new(Schema::new(vec![
        Field::new("contract_id", DataType::Utf8, false),
        // Unknown when the contract was created before the data starts.
        Field::new("template_name", DataType::Utf8, true),
        Field::new("created_offset", DataType::Int64, true),
        Field::new("archived_offset", DataType::Int64, false),
        Field::new("created_at", ts.clone(), true),
        Field::new("archived_at", ts, true),
        Field::new("lifetime_seconds", DataType::Float64, true),
        Field::new("archived_by", DataType::Utf8, true),
    ]))
}

fn ipc(batch: &RecordBatch) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut writer = StreamWriter::try_new(&mut out, &batch.schema()).map_err(|e| e.to_string())?;
    writer.write(batch).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    drop(writer);
    Ok(out)
}

/// Microseconds since the epoch, from an RFC 3339 string. The syncer stores
/// datetimes, which come out as strings like `2026-11-02T09:00:00Z`.
fn micros(v: Option<&Value>) -> Option<i64> {
    let s = v?.as_str()?;
    let s = s.strip_prefix("d'").and_then(|s| s.strip_suffix('\'')).unwrap_or(s);
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.timestamp_micros())
}

fn string(v: Option<&Value>) -> Option<String> {
    v?.as_str().map(str::to_string)
}

fn rows(doc: &str, what: &str) -> Result<Vec<Value>, String> {
    match serde_json::from_str(doc).map_err(|e| format!("{what}: {e}"))? {
        Value::Array(rows) => Ok(rows),
        Value::Null => Ok(Vec::new()),
        other => Err(format!("{what}: expected a list of rows, got {other}")),
    }
}

/// An event, in ledger order. At the same offset creates come first: a
/// contract can be created and archived in one transaction, never the reverse.
enum Event {
    Created { id: String, open: Open },
    Archived { id: String, offset: i64, at: Option<i64>, choice: Option<String> },
}

impl Event {
    fn key(&self) -> (i64, u8) {
        match self {
            Event::Created { open, .. } => (open.offset, 0),
            Event::Archived { offset, .. } => (*offset, 1),
        }
    }
}

fn run(results: Vec<String>, state: Vec<u8>) -> Result<Output, String> {
    let [created, archived] = <[String; 2]>::try_from(results)
        .map_err(|r| format!("expected 2 query results, got {}", r.len()))?;
    let mut state: State = if state.is_empty() {
        State::default()
    } else {
        serde_json::from_slice(&state).map_err(|e| format!("bad state: {e}"))?
    };

    let mut events = Vec::new();
    for row in rows(&created, "created")? {
        let (Some(id), Some(offset)) = (string(row.get("contract_id")), row.get("offset").and_then(Value::as_i64))
        else {
            continue;
        };
        let open = Open { template: string(row.get("template_name")), offset, created_at: micros(row.get("created_at")) };
        events.push(Event::Created { id, open });
    }
    for row in rows(&archived, "exercised")? {
        let (Some(id), Some(offset)) =
            (string(row.get("target_contract_id")), row.get("offset").and_then(Value::as_i64))
        else {
            continue;
        };
        let at = micros(row.get("transaction_effective_at"));
        events.push(Event::Archived { id, offset, at, choice: string(row.get("choice_name")) });
    }
    events.sort_by_key(Event::key);

    let mut contract_id = StringBuilder::new();
    let mut template_name = StringBuilder::new();
    let mut created_offset = Int64Builder::new();
    let mut archived_offset = Int64Builder::new();
    let mut created_at = TimestampMicrosecondBuilder::new().with_timezone("UTC");
    let mut archived_at = TimestampMicrosecondBuilder::new().with_timezone("UTC");
    let mut lifetime = Float64Builder::new();
    let mut archived_by = StringBuilder::new();

    for event in events {
        match event {
            Event::Created { id, open } => {
                state.open.insert(id, open);
            }
            Event::Archived { id, offset, at, choice } => {
                let open = state.open.remove(&id);
                let created = open.as_ref().and_then(|o| o.created_at);
                contract_id.append_value(&id);
                template_name.append_option(open.as_ref().and_then(|o| o.template.as_deref()));
                created_offset.append_option(open.as_ref().map(|o| o.offset));
                archived_offset.append_value(offset);
                created_at.append_option(created);
                archived_at.append_option(at);
                lifetime.append_option(match (created, at) {
                    (Some(c), Some(a)) => Some((a - c) as f64 / 1e6),
                    _ => None,
                });
                archived_by.append_option(choice);
            }
        }
    }

    let columns: Vec<ArrayRef> = vec![
        Arc::new(contract_id.finish()),
        Arc::new(template_name.finish()),
        Arc::new(created_offset.finish()),
        Arc::new(archived_offset.finish()),
        Arc::new(created_at.finish()),
        Arc::new(archived_at.finish()),
        Arc::new(lifetime.finish()),
        Arc::new(archived_by.finish()),
    ];
    let batch = RecordBatch::try_new(schema(), columns).map_err(|e| e.to_string())?;
    Ok(Output {
        rows: ipc(&batch)?,
        state: serde_json::to_vec(&state).map_err(|e| e.to_string())?,
    })
}

impl Guest for Plugin {
    fn info() -> PluginInfo {
        PluginInfo {
            name: "contract-lifetimes".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "How long each Canton contract lived, from its create to its archive.".into(),
        }
    }

    fn schema() -> Vec<u8> {
        ipc(&RecordBatch::new_empty(schema())).expect("the schema encodes")
    }

    fn source() -> QuerySource {
        QuerySource { query: QUERY.into(), high_water: HIGH_WATER.into() }
    }

    fn transform(results: Vec<String>, state: Vec<u8>) -> Result<Output, String> {
        run(results, state)
    }
}
