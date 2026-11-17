//! The Canton event graph: how the explorer reads a SurrealDB copy of a Canton
//! ledger's event graph, and lays it out the way alt-canton's ledger explorer
//! (`explorer-ui/src/model.rs`) does: one column per transaction, its events
//! as an indented tree under it, columns wrapping like text, and parties in a
//! column of their own on the left.
//!
//! Tables are matched case-insensitively. Node tables:
//!
//! | Table                  | Fields read                                               |
//! |------------------------|-----------------------------------------------------------|
//! | `transaction`          | `offset`, `domain` (or `synchronizer_id`)                 |
//! | `created`              | `template_name` (`Main.Cash` or `Cash`), `template_module`, `contract_id`, `signatories`, `observers` |
//! | `exercised`            | `choice_name`, `acting_parties`                           |
//! | `fetched`              |                                                           |
//! | `reassignment`         | `kind`, `offset`, `domain`                                |
//! | `topology_transaction` | `op`, `mapping`, `party`, `participant`, `member_id`, `offset`, `position`, `domain` |
//! | `party`                | `party_id`                                                |
//!
//! Relation tables, named as the Neo4j relationship types: `action` and
//! `consequence` (both may carry an `index`, the child's position), `target`,
//! `consumes`, `requested`, `signatory`, `observer`, `acted`,
//! `choice_observer`, `assigns`, `unassigns`, `hosts`, `keys`.
//!
//! Records in other tables are shown as plain cards in a row of their own.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// What a record is in the event graph, from its table name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeKind {
    Transaction,
    Created,
    Exercised,
    Fetched,
    Reassignment,
    Topology,
    Party,
    /// A record of a table that isn't part of the event graph.
    Other,
}

impl NodeKind {
    /// The kinds of the event graph, in the order the explorer lists them.
    pub const ALL: [NodeKind; 7] = [
        NodeKind::Transaction,
        NodeKind::Created,
        NodeKind::Exercised,
        NodeKind::Fetched,
        NodeKind::Reassignment,
        NodeKind::Topology,
        NodeKind::Party,
    ];

    pub fn from_table(table: &str) -> NodeKind {
        match table.to_ascii_lowercase().as_str() {
            "transaction" => NodeKind::Transaction,
            "created" => NodeKind::Created,
            "exercised" => NodeKind::Exercised,
            "fetched" => NodeKind::Fetched,
            "reassignment" => NodeKind::Reassignment,
            "topology_transaction" | "topologytransaction" | "topology" => NodeKind::Topology,
            "party" => NodeKind::Party,
            _ => NodeKind::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            NodeKind::Transaction => "Transaction",
            NodeKind::Created => "Created",
            NodeKind::Exercised => "Exercised",
            NodeKind::Fetched => "Fetched",
            NodeKind::Reassignment => "Reassignment",
            NodeKind::Topology => "TopologyTransaction",
            NodeKind::Party => "Party",
            NodeKind::Other => "Other",
        }
    }

    /// A root: a node that starts a column.
    fn is_root(self) -> bool {
        matches!(self, NodeKind::Transaction | NodeKind::Reassignment | NodeKind::Topology)
    }
}

/// The edges that build a transaction's tree, from its root down.
const TREE: &[&str] = &["ACTION", "CONSEQUENCE", "ASSIGNS"];

/// Fields that hold party ids, as the Canton toolbox's indexer writes them.
const PARTY_FIELDS: &[&str] = &["signatories", "observers", "acting_parties", "party_id", "party"];

/// A contract's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Active,
    Archived,
    InTransit,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Archived => "archived",
            Status::InTransit => "in transit",
        }
    }
}

/// A record, as the model reads it.
pub struct ModelNode<'a> {
    pub kind: NodeKind,
    pub data: &'a serde_json::Value,
    /// Shown for records outside the event graph.
    pub name: &'a str,
}

/// A relation, as the model reads it: indices into the nodes, and the
/// relation's type in upper case (`ACTION`, `CONSUMES`, …).
pub struct ModelEdge {
    pub from: usize,
    pub to: usize,
    pub kind: String,
    /// The child's position, on `ACTION` and `CONSEQUENCE`.
    pub index: Option<i64>,
}

/// What the explorer knows about one record beyond its fields.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Facts {
    pub kind: Option<NodeKind>,
    pub domain: Option<String>,
    pub offset: Option<i64>,
    /// The transaction, reassignment or topology transaction this record sits
    /// under, itself for a root. `None` for a party, and for an event whose
    /// root isn't loaded.
    pub root: Option<usize>,
    /// The event it is a consequence of, `None` for a root's own events.
    pub parent: Option<usize>,
    pub depth: usize,
    /// Its place in its root's preorder walk; 0 for the root.
    pub order: usize,
    /// The template a contract is, or an exercise targets.
    pub template: Option<String>,
    /// Only on `Created`.
    pub status: Option<Status>,
    /// Every party joined to the record, and for a root every party in its tree.
    /// For an event that records `witness_parties` (as the syncer writes
    /// them), exactly those: the parties whose projection holds it.
    pub parties: BTreeSet<String>,
    pub title: String,
    pub subtitle: String,
}

/// The event graph with each record's facts.
pub struct Model {
    pub facts: Vec<Facts>,
    kinds: Vec<NodeKind>,
    data: Vec<serde_json::Value>,
    edges: Vec<ModelEdge>,
    out: Vec<Vec<usize>>,
    inc: Vec<Vec<usize>>,
}

/// `alice::1220ab…` reads as `alice`.
pub fn short_party(p: &str) -> &str {
    p.split_once("::").map(|(n, _)| n).unwrap_or(p)
}

/// The first `n` characters, with an ellipsis when cut.
pub fn ellipsize(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>())
    }
}

fn str_of<'a>(data: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    data.get(key)?.as_str().filter(|s| !s.is_empty())
}

fn int_of(data: &serde_json::Value, key: &str) -> Option<i64> {
    data.get(key)?.as_i64()
}

/// Party ids in a field: a string, a list of strings, or the indexer's
/// `'a', 'b'` text.
fn parties_in(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::String(s) if s.contains("::") => s
            .split(',')
            .map(|p| p.trim().trim_matches('\'').to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        serde_json::Value::Array(items) => items.iter().flat_map(parties_in).collect(),
        _ => Vec::new(),
    }
}

impl Model {
    pub fn new(nodes: &[ModelNode], edges: Vec<ModelEdge>) -> Model {
        let n = nodes.len();
        let edges: Vec<ModelEdge> = edges.into_iter().filter(|e| e.from < n && e.to < n).collect();
        let mut out = vec![Vec::new(); n];
        let mut inc = vec![Vec::new(); n];
        for (i, e) in edges.iter().enumerate() {
            out[e.from].push(i);
            inc[e.to].push(i);
        }
        let mut m = Model {
            facts: Vec::new(),
            kinds: nodes.iter().map(|n| n.kind).collect(),
            data: nodes.iter().map(|n| n.data.clone()).collect(),
            edges,
            out,
            inc,
        };
        // Children in their recorded position (or event node id), so the
        // preorder walk is the transaction's own.
        for i in 0..n {
            let mut list = std::mem::take(&mut m.out[i]);
            list.sort_by_key(|&e| {
                let edge = &m.edges[e];
                (edge.index.or_else(|| int_of(&m.data[edge.to], "node_id")).unwrap_or(0), e)
            });
            m.out[i] = list;
        }
        m.facts = (0..n).map(|i| m.own_facts(i, nodes[i].name)).collect();
        m.walk_trees();
        m.lift_parties();
        m
    }

    pub fn kind(&self, i: usize) -> NodeKind {
        self.kinds[i]
    }

    fn str_prop(&self, i: usize, key: &str) -> Option<&str> {
        str_of(&self.data[i], key)
    }

    fn targets<'a>(&'a self, i: usize, kind: &'a str) -> impl Iterator<Item = usize> + 'a {
        self.out[i].iter().filter(move |&&e| self.edges[e].kind == kind).map(|&e| self.edges[e].to)
    }

    fn sources<'a>(&'a self, i: usize, kind: &'a str) -> impl Iterator<Item = usize> + 'a {
        self.inc[i].iter().filter(move |&&e| self.edges[e].kind == kind).map(|&e| self.edges[e].from)
    }

    fn template_of(&self, i: usize) -> Option<String> {
        let name = self.str_prop(i, "template_name")?;
        Some(match self.str_prop(i, "template_module") {
            Some(module) => format!("{module}:{name}"),
            // The toolbox's indexer writes `Module.Entity`.
            None => match name.rsplit_once('.') {
                Some((module, entity)) => format!("{module}:{entity}"),
                None => name.to_string(),
            },
        })
    }

    fn own_facts(&self, i: usize, name: &str) -> Facts {
        let kind = self.kinds[i];
        let data = &self.data[i];
        let mut parties: BTreeSet<String> = self.out[i]
            .iter()
            .map(|&e| self.edges[e].to)
            .chain(self.inc[i].iter().map(|&e| self.edges[e].from))
            .filter(|&o| self.kinds[o] == NodeKind::Party)
            .filter_map(|o| self.str_prop(o, "party_id").map(str::to_string))
            .collect();
        // The Ledger API's witnesses say who sees an event; without them,
        // fall back to the parties named on the record.
        match data.get("witness_parties") {
            Some(w) if matches!(kind, NodeKind::Created | NodeKind::Exercised | NodeKind::Fetched) => {
                parties = parties_in(w).into_iter().collect();
            }
            _ => {
                for field in PARTY_FIELDS {
                    if let Some(v) = data.get(*field) {
                        parties.extend(parties_in(v));
                    }
                }
            }
        }
        let mut f = Facts {
            kind: Some(kind),
            // The toolbox's indexer names the domain `synchronizer_id`.
            domain: self.str_prop(i, "domain").or_else(|| self.str_prop(i, "synchronizer_id")).map(str::to_string),
            // The syncer gives contracts from the active contract set offset
            // -1: they have no place in the ledger's history.
            offset: int_of(data, "offset").filter(|&o| o >= 0),
            parties,
            title: kind.name().to_string(),
            ..Facts::default()
        };
        match kind {
            NodeKind::Transaction => {
                f.title = format!("Tx {}", f.offset.unwrap_or(-1));
                let by: Vec<&str> = self
                    .sources(i, "REQUESTED")
                    .filter_map(|p| self.str_prop(p, "party_id"))
                    .map(short_party)
                    .collect();
                f.subtitle = if by.is_empty() { String::new() } else { format!("by {}", by.join(", ")) };
            }
            NodeKind::Reassignment => {
                f.title = format!("{} {}", self.str_prop(i, "kind").unwrap_or("reassign"), f.offset.unwrap_or(-1));
                f.subtitle = f.domain.clone().unwrap_or_default();
            }
            NodeKind::Created => {
                f.template = self.template_of(i);
                f.status = Some(if self.sources(i, "CONSUMES").next().is_some() {
                    Status::Archived
                } else if data.get("reassigned_to").is_some() {
                    Status::InTransit
                } else {
                    Status::Active
                });
                let t = self.str_prop(i, "template_name").unwrap_or("Created");
                f.title = t.rsplit('.').next().unwrap_or(t).to_string();
                f.subtitle = ellipsize(self.str_prop(i, "contract_id").unwrap_or(""), 14);
            }
            NodeKind::Exercised | NodeKind::Fetched => {
                f.template = self.targets(i, "TARGET").next().and_then(|t| self.template_of(t));
                f.title = match kind {
                    NodeKind::Exercised => self.str_prop(i, "choice_name").unwrap_or("Exercised").to_string(),
                    _ => "Fetch".to_string(),
                };
                f.subtitle = f.template.clone().unwrap_or_else(|| "target not loaded".into());
            }
            NodeKind::Topology => {
                let mapping = self.str_prop(i, "mapping").unwrap_or("?");
                f.title = format!("{} {}", self.str_prop(i, "op").unwrap_or("?"), mapping);
                f.subtitle = match mapping {
                    "hosting" => format!(
                        "{} @ {}",
                        short_party(self.str_prop(i, "party").unwrap_or("")),
                        self.str_prop(i, "participant").unwrap_or("")
                    ),
                    "party_key" => short_party(self.str_prop(i, "party").unwrap_or("")).to_string(),
                    _ => self.str_prop(i, "member_id").unwrap_or("").to_string(),
                };
            }
            NodeKind::Party => {
                let p = self.str_prop(i, "party_id").unwrap_or("").to_string();
                f.title = short_party(&p).to_string();
                f.subtitle = ellipsize(p.split_once("::").map(|(_, fp)| fp).unwrap_or(""), 14);
                f.parties = BTreeSet::from([p]);
            }
            NodeKind::Other => {
                f.title = name.to_string();
            }
        }
        f
    }

    /// Places every event under its root, depth first in recorded order.
    fn walk_trees(&mut self) {
        let roots: Vec<usize> = (0..self.kinds.len()).filter(|&i| self.kinds[i].is_root()).collect();
        for r in roots {
            self.facts[r].root = Some(r);
            let mut order = 0;
            let mut stack: Vec<(usize, usize, Option<usize>)> =
                self.children(r).into_iter().rev().map(|c| (c, 1, None)).collect();
            while let Some((i, depth, parent)) = stack.pop() {
                // Already under an earlier root (or a root itself): it stays there.
                if self.facts[i].root.is_some() {
                    continue;
                }
                order += 1;
                let (domain, offset) = (self.facts[r].domain.clone(), self.facts[r].offset);
                let f = &mut self.facts[i];
                f.root = Some(r);
                f.parent = parent;
                f.depth = depth;
                f.order = order;
                f.domain = f.domain.clone().or(domain);
                f.offset = f.offset.or(offset);
                for c in self.children(i).into_iter().rev() {
                    stack.push((c, depth + 1, Some(i)));
                }
            }
        }
    }

    fn children(&self, i: usize) -> Vec<usize> {
        self.out[i]
            .iter()
            .filter(|&&e| TREE.contains(&self.edges[e].kind.as_str()))
            .map(|&e| self.edges[e].to)
            .collect()
    }

    /// A root's parties are its whole tree's, so filtering by party keeps the transaction.
    fn lift_parties(&mut self) {
        for i in 0..self.facts.len() {
            if let Some(r) = self.facts[i].root
                && r != i
            {
                let ps = self.facts[i].parties.clone();
                self.facts[r].parties.extend(ps);
            }
        }
    }

    /// Every party the graph names, sorted.
    pub fn parties(&self) -> BTreeSet<String> {
        self.facts.iter().flat_map(|f| f.parties.iter().cloned()).collect()
    }

    /// Every template the graph names, sorted.
    pub fn templates(&self) -> BTreeSet<String> {
        self.facts.iter().filter_map(|f| f.template.clone()).collect()
    }

    /// How many records of each kind there are.
    pub fn counts(&self) -> BTreeMap<NodeKind, usize> {
        let mut counts = BTreeMap::new();
        for &k in &self.kinds {
            *counts.entry(k).or_default() += 1;
        }
        counts
    }
}

/// Which contracts to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    Active,
    Archived,
}

/// What the sidebar asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct Filters {
    pub kinds: BTreeSet<NodeKind>,
    pub party: Option<String>,
    pub template: Option<String>,
    pub status: StatusFilter,
    pub search: String,
    /// The offsets to show, both ends included. A record with no offset,
    /// such as a party, is not filtered by it.
    pub offset_from: Option<i64>,
    pub offset_to: Option<i64>,
}

impl Default for Filters {
    fn default() -> Filters {
        Filters {
            // Parties join most records, so they start hidden; the sidebar turns them on.
            kinds: NodeKind::ALL.into_iter().filter(|k| *k != NodeKind::Party).chain([NodeKind::Other]).collect(),
            party: None,
            template: None,
            status: StatusFilter::All,
            search: String::new(),
            offset_from: None,
            offset_to: None,
        }
    }
}

impl Filters {
    /// Whether anything narrows the graph beyond the kinds.
    fn narrows(&self) -> bool {
        self.party.is_some() || self.template.is_some() || self.status != StatusFilter::All || !self.search.trim().is_empty()
    }
}

/// Which records `filters` keeps.
///
/// A transaction or reassignment stays while anything under it does, so a
/// matching event never loses the root that says where it sits. A narrowing
/// filter removes a root whose tree holds nothing it matches.
pub fn visible(m: &Model, filters: &Filters) -> Vec<bool> {
    let needle = filters.search.trim().to_lowercase();
    let own = |i: usize| -> bool {
        let kind = m.kinds[i];
        let f = &m.facts[i];
        if !filters.kinds.contains(&kind) {
            return false;
        }
        if let Some(p) = &filters.party
            && !f.parties.contains(p)
        {
            return false;
        }
        if let Some(t) = &filters.template {
            if matches!(kind, NodeKind::Created | NodeKind::Exercised | NodeKind::Fetched) && f.template.as_ref() != Some(t) {
                return false;
            }
            if matches!(kind, NodeKind::Topology | NodeKind::Party | NodeKind::Other) {
                return false;
            }
        }
        match (filters.status, f.status) {
            (StatusFilter::Active, Some(s)) if s != Status::Active => return false,
            (StatusFilter::Archived, Some(s)) if s != Status::Archived => return false,
            (StatusFilter::Active | StatusFilter::Archived, None)
                if kind != NodeKind::Transaction && kind != NodeKind::Reassignment =>
            {
                return false;
            }
            _ => {}
        }
        if !needle.is_empty() {
            let hay = [f.title.clone(), f.subtitle.clone()]
                .into_iter()
                .chain(f.template.clone())
                .chain(f.parties.iter().cloned())
                .chain(std::iter::once(m.data[i].to_string()))
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase();
            if !hay.contains(&needle) {
                return false;
            }
        }
        true
    };
    let mut keep: Vec<bool> = (0..m.kinds.len()).map(own).collect();
    let structural = |i: usize| matches!(m.kinds[i], NodeKind::Transaction | NodeKind::Reassignment);
    // A narrowing filter keeps a root only through its tree.
    if filters.narrows() {
        for (i, k) in keep.iter_mut().enumerate() {
            if structural(i) {
                *k = false;
            }
        }
    }
    for i in 0..keep.len() {
        if keep[i]
            && let Some(r) = m.facts[i].root
            && r != i
            && structural(r)
            && filters.kinds.contains(&m.kinds[r])
        {
            keep[r] = true;
        }
    }
    // An offset range drops whole trees: every event carries its root's offset.
    for (i, k) in keep.iter_mut().enumerate() {
        if let Some(o) = m.facts[i].offset
            && (filters.offset_from.is_some_and(|from| o < from) || filters.offset_to.is_some_and(|to| o > to))
        {
            *k = false;
        }
    }
    keep
}

pub const NODE_W: f32 = 168.0;
pub const NODE_H: f32 = 40.0;
const INDENT: f32 = 22.0;
const ROW_H: f32 = 56.0;
const COL_GAP: f32 = 40.0;
const DOMAIN_GAP: f32 = 70.0;
const TOP: f32 = 40.0;
const LINE_GAP: f32 = 60.0;

/// Where the shown records go, and the bands' headers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// The top-left corner of each card, `None` when not shown.
    pub pos: Vec<Option<(f32, f32)>>,
    /// Each band's header and where it starts.
    pub bands: Vec<(String, f32, f32)>,
    pub width: f32,
    pub height: f32,
}

/// One column per transaction, reassignment or run of topology transactions,
/// left to right in each domain's order, and each tree drawn down its column.
/// `columns` columns sit side by side on each line, on a grid that is at
/// least `fill` units wide, so the columns stretch to fill the canvas; later
/// columns go on further lines below, as text wraps. Parties sit in a column
/// of their own on the left, and records with no root in a band at the end.
pub fn layout(m: &Model, keep: &[bool], columns: usize, fill: f32) -> Layout {
    let mut pos = vec![None; keep.len()];
    let mut height: f32 = TOP;
    let parties: Vec<usize> = (0..keep.len()).filter(|&i| keep[i] && m.kinds[i] == NodeKind::Party).collect();
    for (row, &i) in parties.iter().enumerate() {
        let y = TOP + row as f32 * ROW_H;
        pos[i] = Some((0.0, y));
        height = height.max(y + NODE_H);
    }
    let left = if parties.is_empty() { 0.0 } else { NODE_W + DOMAIN_GAP };
    // Each root's shown tree members.
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for (j, f) in m.facts.iter().enumerate() {
        if keep[j]
            && let Some(r) = f.root
            && r != j
        {
            members.entry(r).or_default().push(j);
        }
    }
    // Roots by (domain, offset, kind rank).
    let mut groups: BTreeMap<(String, i64, u8), Vec<usize>> = BTreeMap::new();
    for (i, &kept) in keep.iter().enumerate() {
        let rank = match m.kinds[i] {
            NodeKind::Topology => 0,
            NodeKind::Transaction => 1,
            NodeKind::Reassignment => 2,
            _ => continue,
        };
        if !kept && !members.contains_key(&i) {
            continue;
        }
        let f = &m.facts[i];
        groups.entry((f.domain.clone().unwrap_or_default(), f.offset.unwrap_or(-1), rank)).or_default().push(i);
    }
    // The columns: each one's domain and its cards as (record, indent),
    // top to bottom. Consecutive topology transactions share a column.
    let mut cols: Vec<(String, Vec<(usize, f32)>)> = Vec::new();
    let mut last_topology = false;
    for ((domain, _, rank), mut roots) in groups {
        let topology = rank == 0;
        let same_domain = cols.last().is_some_and(|(d, _)| *d == domain);
        if !(topology && last_topology && same_domain) {
            cols.push((domain, Vec::new()));
        }
        last_topology = topology;
        let cards = &mut cols.last_mut().expect("a column was just pushed").1;
        roots.sort_by_key(|&r| int_of(&m.data[r], "position").unwrap_or(0));
        for r in roots {
            if keep[r] {
                cards.push((r, 0.0));
            }
            let mut tree = members.remove(&r).unwrap_or_default();
            tree.sort_by_key(|&j| m.facts[j].order);
            for j in tree {
                // Indented under its nearest shown ancestor: an event whose
                // parent is filtered out (a party that sees only part of a
                // transaction) hangs straight under the transaction.
                let mut depth = 1;
                let mut up = m.facts[j].parent;
                while let Some(a) = up {
                    if keep[a] {
                        depth += 1;
                    }
                    up = m.facts[a].parent;
                }
                let indent = (depth - 1) as f32 * INDENT + if keep[r] { INDENT } else { 0.0 };
                cards.push((j, indent));
            }
        }
    }
    // Every column gets the same slot: as wide as the widest column, or
    // wider when that leaves the line short of `fill`.
    let per_line = columns.max(1).min(cols.len().max(1));
    let widest = cols.iter().flat_map(|(_, cards)| cards.iter().map(|&(_, indent)| indent + NODE_W)).fold(NODE_W, f32::max);
    let pitch = (widest + COL_GAP).max((fill - left + COL_GAP) / per_line as f32);
    let mut bands = Vec::new();
    let mut line_top = TOP;
    let mut line_bottom = TOP;
    let mut last_domain: Option<&str> = None;
    for (k, (domain, cards)) in cols.iter().enumerate() {
        if k > 0 && k % per_line == 0 {
            line_top = line_bottom + LINE_GAP;
        }
        let x = left + (k % per_line) as f32 * pitch;
        if last_domain != Some(domain.as_str()) {
            if !domain.is_empty() {
                bands.push((format!("domain {domain}"), x, line_top - 24.0));
            }
            last_domain = Some(domain);
        }
        let mut y = line_top;
        for &(i, indent) in cards {
            pos[i] = Some((x + indent, y));
            y += ROW_H;
        }
        line_bottom = line_bottom.max(y);
    }
    let mut right = if cols.is_empty() { left + NODE_W } else { left + per_line as f32 * pitch - COL_GAP };
    height = height.max(line_bottom);
    // Records with no root: events whose transaction isn't loaded, and other tables.
    let orphans: Vec<usize> = (0..keep.len()).filter(|&i| keep[i] && pos[i].is_none()).collect();
    if !orphans.is_empty() {
        let top = line_bottom + LINE_GAP;
        let all_other = orphans.iter().all(|&i| m.kinds[i] == NodeKind::Other);
        bands.push((if all_other { "other records" } else { "no root loaded" }.to_string(), left, top - 24.0));
        let per_row = (((right.max(fill) - left + COL_GAP) / (NODE_W + COL_GAP)).floor() as usize).max(1);
        for (k, &i) in orphans.iter().enumerate() {
            let (cx, cy) = (left + (k % per_row) as f32 * (NODE_W + COL_GAP), top + (k / per_row) as f32 * ROW_H);
            pos[i] = Some((cx, cy));
            right = right.max(cx + NODE_W);
            height = height.max(cy + NODE_H);
        }
    }
    Layout { pos, bands, width: right, height: height + ROW_H }
}

/// Whether the graph has the event graph's tables: transactions and at
/// least one kind of event.
pub fn looks_like_event_graph<'a>(tables: impl IntoIterator<Item = &'a str>) -> bool {
    let kinds: BTreeSet<NodeKind> = tables.into_iter().map(NodeKind::from_table).collect();
    kinds.contains(&NodeKind::Transaction) && (kinds.contains(&NodeKind::Created) || kinds.contains(&NodeKind::Exercised))
}
