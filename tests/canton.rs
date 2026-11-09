//! The Canton event graph: the sample loads into SurrealDB in the toolbox's
//! shape, and the layout places it the way the toolbox does.

mod common;

use std::collections::HashMap;

use surrealdb_explorer::canton::{self, H_SPACING, LayoutEdge, LayoutNode, NodeKind, RelKind, V_SPACING};
use surrealdb_explorer::graph::GraphData;
use surrealdb_explorer::{data, db, graph};

/// Loads the Canton sample into a fresh database.
fn load_sample(name: &str, times: usize) -> GraphData {
    let config = common::temp_database(name);
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        for _ in 0..times {
            data::load_canton_sample(&db).await.unwrap();
        }
        graph::load(&db, 1000).await.unwrap()
    })
}

/// Positions by record id, laid out as the Canton view does.
fn lay_out(g: &GraphData) -> HashMap<String, (f32, f32)> {
    let nodes: Vec<LayoutNode> =
        g.nodes.iter().map(|n| LayoutNode { kind: NodeKind::from_table(&n.table), data: &n.data }).collect();
    let index: HashMap<&str, usize> = g.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let edges: Vec<LayoutEdge> = g
        .edges
        .iter()
        .map(|e| LayoutEdge { from: index[e.from.as_str()], to: index[e.to.as_str()], kind: RelKind::from_table(&e.label) })
        .collect();
    let positions = canton::layout(&nodes, &edges);
    g.nodes.iter().map(|n| n.id.clone()).zip(positions).collect()
}

fn count<T: PartialEq>(items: impl Iterator<Item = T>, of: T) -> usize {
    items.filter(|k| *k == of).count()
}

#[test]
fn sample_loads_as_an_event_graph_and_repeatably() {
    for times in [1, 2] {
        let g = load_sample(&format!("canton-load-{times}"), times);
        let kinds = || g.nodes.iter().map(|n| NodeKind::from_table(&n.table));
        assert_eq!(count(kinds(), NodeKind::Party), 4);
        assert_eq!(count(kinds(), NodeKind::Transaction), 5);
        assert_eq!(count(kinds(), NodeKind::Created), 5);
        assert_eq!(count(kinds(), NodeKind::Exercised), 4);
        assert_eq!(count(kinds(), NodeKind::Other), 0);

        let rels = || g.edges.iter().map(|e| RelKind::from_table(&e.label));
        assert_eq!(count(rels(), RelKind::Requested), 5);
        assert_eq!(count(rels(), RelKind::Action), 5);
        assert_eq!(count(rels(), RelKind::Consequence), 4);
        assert_eq!(count(rels(), RelKind::Target), 4);
        assert_eq!(count(rels(), RelKind::Consumes), 3);

        // Every relation connects two loaded records.
        let ids: std::collections::HashSet<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(g.edges.iter().all(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str())));
        assert!(canton::looks_like_event_graph(g.nodes.iter().map(|n| n.table.as_str())));
    }
}

#[test]
fn layout_matches_the_toolbox() {
    let g = load_sample("canton-layout", 1);
    let pos = lay_out(&g);
    let at = |id: &str| *pos.get(id).unwrap_or_else(|| panic!("{id} not in {:?}", pos.keys()));

    // Transactions in a row, in offset order, one column each.
    let txs: Vec<(f32, f32)> = (1..=5).map(|o| at(&format!("transaction:{o}"))).collect();
    assert!(txs.iter().all(|&(_, y)| y == V_SPACING));
    assert!(txs.windows(2).all(|w| w[1].0 - w[0].0 >= H_SPACING));

    // Parties above, events below their transaction, consequences deeper.
    for n in &g.nodes {
        if NodeKind::from_table(&n.table) == NodeKind::Party {
            assert_eq!(at(&n.id).1, 0.0, "{}", n.id);
        }
    }
    let accept = at("exercised:[3, 0]");
    let transfer = at("exercised:[3, 1]");
    let paid = at("created:00c3c7d4");
    assert_eq!(accept, (txs[2].0, 2.0 * V_SPACING));
    assert_eq!(transfer.1, 3.0 * V_SPACING);
    assert_eq!(at("created:00c4d8e5").1, 3.0 * V_SPACING);
    assert_eq!(paid.1, 4.0 * V_SPACING);
    // The third transaction's tree is two wide, centred under it.
    assert!((transfer.0 - txs[2].0).abs() == H_SPACING / 2.0);

    // Bob requested transactions 3 and 4, so sits between them.
    let bob = at("party:`Bob::12209c5d1e8a`");
    assert_eq!(bob.0, (txs[2].0 + txs[3].0) / 2.0);
}

#[test]
fn records_outside_the_event_graph_go_in_a_row_underneath() {
    let data = [serde_json::json!({ "offset": 1 }), serde_json::json!({}), serde_json::json!({})];
    let nodes = vec![
        LayoutNode { kind: NodeKind::Transaction, data: &data[0] },
        LayoutNode { kind: NodeKind::Other, data: &data[1] },
        LayoutNode { kind: NodeKind::Other, data: &data[2] },
    ];
    let pos = canton::layout(&nodes, &[]);
    assert_eq!(pos[0].1, V_SPACING);
    assert!(pos[1].1 > V_SPACING && pos[1].1 == pos[2].1);
    assert_eq!(pos[2].0 - pos[1].0, H_SPACING);
}

#[test]
fn names_are_the_toolbox_ones() {
    use serde_json::json;
    let name = |kind, data| canton::display_name(kind, &data).unwrap();
    assert_eq!(name(NodeKind::Transaction, json!({ "update_id": "1220a03e7081a2b3c4d5" })), "T:1220a03e7081");
    assert_eq!(name(NodeKind::Created, json!({ "template_name": "Main.TicketOffer" })), "TicketOffer");
    assert_eq!(name(NodeKind::Exercised, json!({ "choice_name": "Cash_Transfer" })), "Cash_Transfer");
    assert_eq!(name(NodeKind::Party, json!({ "party_id": "Carol::1220d2a6b3c7aaaa" })), "Carol::1220d2a6b3c7a");
    assert_eq!(name(NodeKind::Exercised, json!({})), "Exercised");
    assert_eq!(canton::display_name(NodeKind::Other, &json!({ "name": "x" })), None);
    assert_eq!(NodeKind::from_table("Transaction"), NodeKind::Transaction);
    assert_eq!(RelKind::from_table("CONSUMES"), RelKind::Consumes);
}
