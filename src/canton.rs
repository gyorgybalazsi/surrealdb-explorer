//! The Canton event graph: how the explorer reads a SurrealDB copy of the
//! graph that rust-canton-toolbox's `event-graph-indexer` builds, and how it
//! lays it out the way that toolbox's `event-graph-ui` does.
//!
//! The schema mirrors the indexer's Neo4j graph, with one table per label
//! (table names are matched case-insensitively):
//!
//! | Table         | Fields read                                         |
//! |---------------|-----------------------------------------------------|
//! | `transaction` | `offset`, `update_id`                               |
//! | `created`     | `template_name` (e.g. `Main.Cash`)                  |
//! | `exercised`   | `choice_name`                                       |
//! | `party`       | `party_id`                                          |
//!
//! and one relation table per relationship type:
//!
//! | Relation      | From        | To                  |
//! |---------------|-------------|---------------------|
//! | `action`      | transaction | created / exercised |
//! | `consequence` | exercised   | created / exercised |
//! | `target`      | exercised   | created             |
//! | `consumes`    | exercised   | created             |
//! | `requested`   | party       | transaction         |
//!
//! Records in other tables are still shown, as plain records.

use std::collections::{HashMap, HashSet, VecDeque};

/// What a record is in the event graph, from its table name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Transaction,
    Created,
    Exercised,
    Party,
    /// A record of a table that isn't part of the event graph.
    Other,
}

impl NodeKind {
    pub const ALL: [NodeKind; 4] = [NodeKind::Transaction, NodeKind::Created, NodeKind::Exercised, NodeKind::Party];

    pub fn from_table(table: &str) -> NodeKind {
        match table.to_ascii_lowercase().as_str() {
            "transaction" => NodeKind::Transaction,
            "created" => NodeKind::Created,
            "exercised" => NodeKind::Exercised,
            "party" => NodeKind::Party,
            _ => NodeKind::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            NodeKind::Transaction => "Transaction",
            NodeKind::Created => "Created",
            NodeKind::Exercised => "Exercised",
            NodeKind::Party => "Party",
            NodeKind::Other => "Other",
        }
    }
}

/// What a relation is in the event graph, from its table name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelKind {
    Action,
    Consequence,
    Target,
    Consumes,
    Requested,
    /// A relation that isn't part of the event graph.
    Other,
}

impl RelKind {
    pub const ALL: [RelKind; 5] =
        [RelKind::Action, RelKind::Consequence, RelKind::Target, RelKind::Consumes, RelKind::Requested];

    pub fn from_table(table: &str) -> RelKind {
        match table.to_ascii_lowercase().as_str() {
            "action" => RelKind::Action,
            "consequence" => RelKind::Consequence,
            "target" => RelKind::Target,
            "consumes" => RelKind::Consumes,
            "requested" => RelKind::Requested,
            _ => RelKind::Other,
        }
    }

    /// The name the toolbox shows on the edge.
    pub fn name(self) -> &'static str {
        match self {
            RelKind::Action => "ACTION",
            RelKind::Consequence => "CONSEQUENCE",
            RelKind::Target => "TARGET",
            RelKind::Consumes => "CONSUMES",
            RelKind::Requested => "REQUESTED",
            RelKind::Other => "",
        }
    }
}

/// The short name the toolbox shows under a node: `T:<update id>`, the
/// template's entity name, the choice name, or the party id.
pub fn display_name(kind: NodeKind, data: &serde_json::Value) -> Option<String> {
    let field = |name: &str| data.get(name).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    Some(match kind {
        NodeKind::Transaction => match field("update_id") {
            Some(uid) => format!("T:{}", prefix(uid, 12)),
            None => "Transaction".into(),
        },
        NodeKind::Created => match field("template_name") {
            Some(t) => t.rsplit('.').next().unwrap_or(t).to_string(),
            None => "Created".into(),
        },
        NodeKind::Exercised => field("choice_name").unwrap_or("Exercised").to_string(),
        NodeKind::Party => field("party_id").map(|p| prefix(p, 20).to_string()).unwrap_or_else(|| "Party".into()),
        NodeKind::Other => return None,
    })
}

fn prefix(s: &str, chars: usize) -> &str {
    s.char_indices().nth(chars).map_or(s, |(i, _)| &s[..i])
}

/// Horizontal distance between neighbouring nodes.
pub const H_SPACING: f32 = 200.0;
/// Vertical distance between levels.
pub const V_SPACING: f32 = 120.0;

/// A node as the layout sees it.
pub struct LayoutNode<'a> {
    pub kind: NodeKind,
    pub data: &'a serde_json::Value,
}

/// An edge as the layout sees it: indices into the nodes.
pub struct LayoutEdge {
    pub from: usize,
    pub to: usize,
    pub kind: RelKind,
}

/// The toolbox's hierarchical layout; returns a position per node (y grows downwards).
///
/// 1. Each transaction (ordered by `offset`) gets a column of its own: the
///    transaction on top, then its ACTION children, then their CONSEQUENCE
///    children level by level, each level centred under the transaction.
/// 2. Parties go in a row above, each over the transactions it requested.
/// 3. Anything else goes in a row below everything.
pub fn layout(nodes: &[LayoutNode], edges: &[LayoutEdge]) -> Vec<(f32, f32)> {
    let mut pos = vec![(0.0, 0.0); nodes.len()];
    if nodes.is_empty() {
        return pos;
    }

    // The tree: ACTION and CONSEQUENCE edges.
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut has_parent: HashSet<usize> = HashSet::new();
    let mut party_to_txs: HashMap<usize, Vec<usize>> = HashMap::new();
    for e in edges {
        if e.from >= nodes.len() || e.to >= nodes.len() {
            continue;
        }
        match e.kind {
            RelKind::Action | RelKind::Consequence => {
                children.entry(e.from).or_default().push(e.to);
                has_parent.insert(e.to);
            }
            RelKind::Requested => party_to_txs.entry(e.from).or_default().push(e.to),
            _ => {}
        }
    }

    let offset = |i: usize| nodes[i].data.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    let mut roots: Vec<usize> = (0..nodes.len())
        .filter(|&i| nodes[i].kind == NodeKind::Transaction && !has_parent.contains(&i))
        .collect();
    roots.sort_by_key(|&i| offset(i));

    let mut placed = vec![false; nodes.len()];
    let mut root_centre: HashMap<usize, f32> = HashMap::new();
    let mut cursor = 0.0;
    for &root in &roots {
        // Breadth-first levels under the transaction.
        let mut levels: Vec<Vec<usize>> = Vec::new();
        let mut queue = VecDeque::from([(root, 0usize)]);
        let mut seen = HashSet::from([root]);
        while let Some((i, depth)) = queue.pop_front() {
            if placed[i] && i != root {
                continue; // already in an earlier transaction's column
            }
            if levels.len() <= depth {
                levels.push(Vec::new());
            }
            levels[depth].push(i);
            for &kid in children.get(&i).map(Vec::as_slice).unwrap_or_default() {
                if seen.insert(kid) {
                    queue.push_back((kid, depth + 1));
                }
            }
        }

        let widest = levels.iter().map(Vec::len).max().unwrap_or(1);
        let width = widest as f32 * H_SPACING;
        for (depth, level) in levels.iter().enumerate() {
            let start = cursor + (width - level.len() as f32 * H_SPACING) / 2.0;
            for (j, &i) in level.iter().enumerate() {
                pos[i] = (start + j as f32 * H_SPACING + H_SPACING / 2.0, V_SPACING + depth as f32 * V_SPACING);
                placed[i] = true;
            }
        }
        root_centre.insert(root, cursor + width / 2.0);
        cursor += width.max(H_SPACING);
    }

    // Parties above the transactions they requested, kept apart.
    let mut parties: Vec<(usize, f32)> = (0..nodes.len())
        .filter(|&i| nodes[i].kind == NodeKind::Party)
        .map(|i| {
            let xs: Vec<f32> = party_to_txs
                .get(&i)
                .into_iter()
                .flatten()
                .filter_map(|t| root_centre.get(t).copied())
                .collect();
            let x = if xs.is_empty() { f32::NEG_INFINITY } else { xs.iter().sum::<f32>() / xs.len() as f32 };
            (i, x)
        })
        .collect();
    parties.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut last = f32::NEG_INFINITY;
    for (i, x) in parties {
        let x = if x.is_finite() { x.max(last + H_SPACING) } else if last.is_finite() { last + H_SPACING } else { H_SPACING / 2.0 };
        pos[i] = (x, 0.0);
        placed[i] = true;
        last = x;
    }

    // Everything else in a row underneath.
    let bottom = (0..nodes.len()).filter(|&i| placed[i]).map(|i| pos[i].1).fold(0.0, f32::max);
    let mut x = H_SPACING / 2.0;
    for (i, p) in pos.iter_mut().enumerate() {
        if !placed[i] {
            *p = (x, bottom + V_SPACING * 1.5);
            x += H_SPACING;
        }
    }
    pos
}

/// Whether the graph has the event graph's tables: transactions and at
/// least one kind of event.
pub fn looks_like_event_graph<'a>(tables: impl IntoIterator<Item = &'a str>) -> bool {
    let kinds: HashSet<NodeKind> = tables.into_iter().map(NodeKind::from_table).collect();
    kinds.contains(&NodeKind::Transaction) && (kinds.contains(&NodeKind::Created) || kinds.contains(&NodeKind::Exercised))
}
