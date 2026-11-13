//! The graph data: records as nodes and relations as edges.

/// A record drawn as a node.
pub struct GraphNode {
    pub id: String,
    pub table: String,
    /// All fields of the record.
    pub data: serde_json::Value,
}

/// A relation between two records: a record in a relation table
/// (created with `RELATE`), drawn from its `in` to its `out`.
pub struct GraphEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    /// Relation table name.
    pub label: String,
    /// All fields of the relation record.
    pub data: serde_json::Value,
}

#[derive(Default)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Tables that had more than `MAX_PER_TABLE` records.
    pub truncated: Vec<String>,
}

impl GraphData {
    /// The graph as JSON: `{ nodes: [{ id, table, data }], edges: [{ id,
    /// from, to, label, data }], truncated: [table, …] }`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "nodes": self.nodes.iter().map(|n| serde_json::json!({
                "id": n.id, "table": n.table, "data": n.data,
            })).collect::<Vec<_>>(),
            "edges": self.edges.iter().map(|e| serde_json::json!({
                "id": e.id, "from": e.from, "to": e.to, "label": e.label, "data": e.data,
            })).collect::<Vec<_>>(),
            "truncated": self.truncated,
        })
    }
}
