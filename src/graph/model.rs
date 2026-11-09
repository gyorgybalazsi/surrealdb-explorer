//! The graph data: records as nodes and relations as edges.

/// A record drawn as a node.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: String,
    pub table: String,
    /// All fields of the record.
    pub data: serde_json::Value,
}

/// A relation between two records: a record in a relation table
/// (created with `RELATE`), drawn from its `in` to its `out`.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    /// Relation table name.
    pub label: String,
    /// All fields of the relation record.
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Tables that had more than `MAX_PER_TABLE` records.
    pub truncated: Vec<String>,
}
