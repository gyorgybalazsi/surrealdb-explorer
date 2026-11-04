//! Turning a single record into a node or an edge.

use surrealdb::types::{ToSql, Value};

use super::model::{GraphData, GraphEdge, GraphNode};
use super::tables::Table;

/// Adds one record to the graph: as an edge if it is a relation
/// (has `in` and `out` record ids), otherwise as a node.
pub fn add(data: &mut GraphData, table: &Table, row: Value) {
    let Value::Object(obj) = &row else { return };
    let Some(Value::RecordId(id)) = obj.get("id") else { return };
    let id = id.to_sql();

    let endpoints = match (obj.get("in"), obj.get("out")) {
        (Some(Value::RecordId(from)), Some(Value::RecordId(to))) => {
            Some((from.to_sql(), to.to_sql()))
        }
        _ => None,
    };

    match endpoints {
        Some((from, to)) => data.edges.push(GraphEdge {
            id,
            from,
            to,
            label: table.name.clone(),
            data: row.into_json_value(),
        }),
        // A relation-table record without valid endpoints has nothing to connect.
        None if table.is_relation => {}
        None => data.nodes.push(GraphNode {
            id,
            table: table.name.clone(),
            data: row.into_json_value(),
        }),
    }
}
