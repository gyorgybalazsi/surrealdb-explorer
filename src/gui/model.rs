//! The graph as the GUI keeps it: records with positions, colours and labels.

use std::collections::{HashMap, HashSet};

use iced::{Color, Vector};
use surrealdb_explorer::graph::GraphData;

const PALETTE: [Color; 10] = [
    rgb(0x4a, 0x90, 0xd9),
    rgb(0xe6, 0x7e, 0x22),
    rgb(0x27, 0xae, 0x60),
    rgb(0x8e, 0x44, 0xad),
    rgb(0xc0, 0x39, 0x2b),
    rgb(0x16, 0xa0, 0x85),
    rgb(0xd4, 0xa0, 0x17),
    rgb(0x7f, 0x8c, 0x8d),
    rgb(0xe8, 0x43, 0x93),
    rgb(0x2c, 0x3e, 0x50),
];

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

/// Fields tried, in order, for a record's display name.
const NAME_FIELDS: [&str; 5] = ["name", "title", "label", "username", "email"];

pub struct Node {
    pub id: String,
    pub table: String,
    pub label: String,
    pub data: serde_json::Value,
    pub color: Color,
    /// Position in graph space (the camera maps it to the screen).
    pub pos: Vector,
    pub vel: Vector,
    pub hidden: bool,
}

pub struct Edge {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub label: String,
    pub data: serde_json::Value,
}

pub struct Table {
    pub name: String,
    pub color: Color,
    pub count: usize,
}

pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub tables: Vec<Table>,
    /// Tables that had more records than the loader's per-table limit.
    pub truncated: Vec<String>,
    /// Relations pointing at records that weren't loaded.
    pub dangling_edges: usize,
}

impl Graph {
    /// Builds the GUI graph. Records that were already on screen (in `previous`)
    /// keep their position, so reloading doesn't scramble the layout.
    pub fn new(data: GraphData, previous: Option<&Graph>) -> Graph {
        let old: HashMap<&str, (Vector, bool)> = previous
            .map(|g| g.nodes.iter().map(|n| (n.id.as_str(), (n.pos, n.hidden))).collect())
            .unwrap_or_default();

        let mut table_names: Vec<String> = data.nodes.iter().map(|n| n.table.clone()).collect();
        table_names.sort();
        table_names.dedup();
        let color_of = |table: &str| {
            let i = table_names.iter().position(|t| t == table).unwrap_or(0);
            PALETTE[i % PALETTE.len()]
        };

        let count = data.nodes.len().max(1) as f32;
        let nodes: Vec<Node> = data
            .nodes
            .into_iter()
            .enumerate()
            .map(|(i, n)| {
                // New records start on a circle; the layout spreads them out.
                let angle = std::f32::consts::TAU * i as f32 / count;
                let start = Vector::new(angle.cos(), angle.sin()) * (60.0 + 8.0 * count.sqrt());
                let (pos, hidden) = old.get(n.id.as_str()).copied().unwrap_or((start, false));
                Node {
                    label: display_name(&n.id, &n.data),
                    color: color_of(&n.table),
                    id: n.id,
                    table: n.table,
                    data: n.data,
                    pos,
                    vel: Vector::ZERO,
                    hidden,
                }
            })
            .collect();

        let index: HashMap<&str, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let total_edges = data.edges.len();
        let edges: Vec<Edge> = data
            .edges
            .into_iter()
            .filter_map(|e| {
                Some(Edge {
                    from: *index.get(e.from.as_str())?,
                    to: *index.get(e.to.as_str())?,
                    id: e.id,
                    label: e.label,
                    data: e.data,
                })
            })
            .collect();

        let tables = table_names
            .iter()
            .map(|name| Table {
                name: name.clone(),
                color: color_of(name),
                count: nodes.iter().filter(|n| &n.table == name).count(),
            })
            .collect();

        Graph {
            dangling_edges: total_edges - edges.len(),
            nodes,
            edges,
            tables,
            truncated: data.truncated,
        }
    }

    /// An edge is drawn only if both of its records are visible.
    pub fn edge_visible(&self, e: &Edge) -> bool {
        !self.nodes[e.from].hidden && !self.nodes[e.to].hidden
    }

    /// The record itself plus every record directly connected to it.
    pub fn neighbourhood(&self, node: usize) -> HashSet<usize> {
        let mut set = HashSet::from([node]);
        for e in &self.edges {
            if e.from == node {
                set.insert(e.to);
            }
            if e.to == node {
                set.insert(e.from);
            }
        }
        set
    }

    pub fn hidden_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.hidden).count()
    }
}

/// The first name-like field that has text, or the record id.
fn display_name(id: &str, data: &serde_json::Value) -> String {
    NAME_FIELDS
        .iter()
        .find_map(|f| as_text(data.get(f)?))
        .unwrap_or_else(|| id.to_string())
}

/// Strings and numbers as-is; objects like `{ first, last }` joined from their text parts.
fn as_text(v: &serde_json::Value) -> Option<String> {
    use serde_json::Value::*;
    match v {
        String(s) if !s.is_empty() => Some(s.clone()),
        Number(n) => Some(n.to_string()),
        Object(o) => {
            let parts: Vec<std::string::String> = o
                .values()
                .filter_map(|x| match x {
                    String(s) => Some(s.clone()),
                    Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect();
            (!parts.is_empty()).then(|| parts.join(" "))
        }
        _ => None,
    }
}
