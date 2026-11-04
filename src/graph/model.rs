//! The graph data sent to the web page as JSON.

use serde::Serialize;

/// A record drawn as a node.
#[derive(Serialize)]
pub struct GraphNode {
    pub id: String,
    pub table: String,
    /// All fields of the record.
    pub data: serde_json::Value,
}

/// A relation between two records: a record in a relation table
/// (created with `RELATE`), drawn from its `in` to its `out`.
#[derive(Serialize)]
pub struct GraphEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    /// Relation table name.
    pub label: String,
    /// All fields of the relation record.
    pub data: serde_json::Value,
}

#[derive(Serialize, Default)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Tables that had more than `MAX_PER_TABLE` records.
    pub truncated: Vec<String>,
}
