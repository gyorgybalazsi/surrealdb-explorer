//! The Canton event graph: the sample loads into SurrealDB in the toolbox's
//! shape, and the model, filters and layout treat it as alt-canton's ledger
//! explorer does.

mod common;

use std::collections::HashMap;

use surrealdb_explorer::canton::{self, Filters, Model, ModelEdge, ModelNode, NODE_H, NODE_W, NodeKind, Status, StatusFilter};
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

/// The sample's model, with record ids by index.
fn model_of(g: &GraphData) -> (Model, Vec<String>) {
    let nodes: Vec<ModelNode> = g
        .nodes
        .iter()
        .map(|n| ModelNode { kind: NodeKind::from_table(&n.table), data: &n.data, name: &n.id })
        .collect();
    let index: HashMap<&str, usize> = g.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let edges = g
        .edges
        .iter()
        .map(|e| ModelEdge {
            from: index[e.from.as_str()],
            to: index[e.to.as_str()],
            kind: e.label.to_ascii_uppercase(),
            index: e.data.get("index").and_then(|v| v.as_i64()),
        })
        .collect();
    (Model::new(&nodes, edges), g.nodes.iter().map(|n| n.id.clone()).collect())
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

        let rels = || g.edges.iter().map(|e| e.label.as_str());
        assert_eq!(count(rels(), "requested"), 5);
        assert_eq!(count(rels(), "action"), 5);
        assert_eq!(count(rels(), "consequence"), 4);
        assert_eq!(count(rels(), "target"), 4);
        assert_eq!(count(rels(), "consumes"), 3);

        let ids: std::collections::HashSet<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(g.edges.iter().all(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str())));
        assert!(canton::looks_like_event_graph(g.nodes.iter().map(|n| n.table.as_str())));
    }
}

#[test]
fn facts_match_the_explorer() {
    let g = load_sample("canton-facts", 1);
    let (m, ids) = model_of(&g);
    let at = |id: &str| ids.iter().position(|i| i == id).unwrap_or_else(|| panic!("{id} not in {ids:?}"));
    let f = |id: &str| &m.facts[at(id)];

    assert_eq!((f("transaction:3").title.as_str(), f("transaction:3").subtitle.as_str()), ("Tx 3", "by Bob"));
    assert_eq!(f("created:00c1a5b2").title, "Cash");
    assert_eq!(f("created:00c1a5b2").template.as_deref(), Some("Main:Cash"));
    assert_eq!(f("exercised:[3, 1]").subtitle, "Main:Cash");
    // Consumed contracts are archived; the rest are active.
    assert_eq!(f("created:00c1a5b2").status, Some(Status::Archived));
    assert_eq!(f("created:00c3c7d4").status, Some(Status::Active));
    // The tree: the accept at depth 1, then the transfer (node 1) before the ticket (node 3).
    let tx3 = at("transaction:3");
    assert_eq!((f("exercised:[3, 0]").root, f("exercised:[3, 0]").depth), (Some(tx3), 1));
    assert_eq!((f("exercised:[3, 1]").depth, f("exercised:[3, 1]").order), (2, 2));
    assert_eq!((f("created:00c3c7d4").depth, f("created:00c3c7d4").order), (3, 3));
    assert_eq!((f("created:00c4d8e5").depth, f("created:00c4d8e5").order), (2, 4));
    // Events inherit the transaction's domain, and the transaction collects its tree's parties.
    assert_eq!(f("created:00c4d8e5").domain.as_deref(), Some("global-domain::1220ab"));
    assert!(f("transaction:3").parties.contains("Venue::1220b7e4a0f5"));
    assert_eq!(m.templates().into_iter().collect::<Vec<_>>(), ["Main:Cash", "Main:Ticket", "Main:TicketOffer"]);
}

#[test]
fn layout_matches_the_explorer() {
    let g = load_sample("canton-layout", 1);
    let (m, ids) = model_of(&g);
    let at = |id: &str| ids.iter().position(|i| i == id).unwrap();

    // Parties start hidden, so the columns start at the left edge.
    let keep = canton::visible(&m, &Filters::default());
    assert!(ids.iter().enumerate().all(|(i, id)| keep[i] != id.starts_with("party:")));
    let l = canton::layout(&m, &keep, 1000.0);
    let pos = |id: &str| l.pos[at(id)].unwrap_or_else(|| panic!("{id} not placed"));
    assert_eq!(pos("transaction:1"), (0.0, 40.0));
    // One column per transaction, in offset order, each as wide as its indented tree, plus a gap.
    assert_eq!(pos("transaction:2").0 - pos("transaction:1").0, 22.0 + NODE_W + 40.0);
    // The tree goes down its column, each level indented further.
    let tx3 = pos("transaction:3");
    assert_eq!(pos("exercised:[3, 0]"), (tx3.0 + 22.0, tx3.1 + 56.0));
    assert_eq!(pos("exercised:[3, 1]"), (tx3.0 + 44.0, tx3.1 + 112.0));
    assert_eq!(pos("created:00c3c7d4"), (tx3.0 + 66.0, tx3.1 + 168.0));
    assert_eq!(pos("created:00c4d8e5"), (tx3.0 + 44.0, tx3.1 + 224.0));
    // Columns wrap like text: the fifth starts a new line under the deepest column.
    let tx5 = pos("transaction:5");
    assert_eq!(tx5.0, 0.0);
    assert!(tx5.1 > pos("created:00c4d8e5").1 + NODE_H);
    // The band header names the domain.
    assert_eq!(l.bands[0].0, "domain global-domain::1220ab");

    // Turning parties on puts them in a column of their own on the left.
    let mut with_parties = Filters::default();
    with_parties.kinds.insert(NodeKind::Party);
    let keep = canton::visible(&m, &with_parties);
    let l = canton::layout(&m, &keep, 1000.0);
    assert_eq!(l.pos[at("party:`Bank::1220f3a9c1d2`")].unwrap().0, 0.0);
    assert_eq!(l.pos[at("transaction:1")].unwrap().0, NODE_W + 70.0);
}

#[test]
fn filters_keep_a_match_with_its_transaction() {
    let g = load_sample("canton-filters", 1);
    let (m, ids) = model_of(&g);
    let shown = |f: &Filters| -> Vec<String> {
        let keep = canton::visible(&m, f);
        let mut v: Vec<String> = ids.iter().zip(keep).filter(|(_, k)| *k).map(|(i, _)| i.clone()).collect();
        v.sort();
        v
    };

    let active = Filters { status: StatusFilter::Active, ..Filters::default() };
    assert_eq!(shown(&active), ["created:00c3c7d4", "created:00c5e9f6", "transaction:3", "transaction:4"]);

    let search = Filters { search: "ticket_show".into(), ..Filters::default() };
    assert_eq!(shown(&search), ["exercised:[5, 0]", "transaction:5"]);

    let template = Filters { template: Some("Main:Cash".into()), ..Filters::default() };
    assert_eq!(
        shown(&template),
        ["created:00c1a5b2", "created:00c3c7d4", "exercised:[3, 1]", "transaction:1", "transaction:3"]
    );

    // An offset range keeps whole transactions, trees included; with no end it is open.
    let range = Filters { offset_from: Some(3), offset_to: Some(4), ..Filters::default() };
    assert_eq!(
        shown(&range),
        [
            "created:00c3c7d4",
            "created:00c4d8e5",
            "created:00c5e9f6",
            "exercised:[3, 0]",
            "exercised:[3, 1]",
            "exercised:[4, 0]",
            "transaction:3",
            "transaction:4"
        ]
    );
    let from_five = Filters { offset_from: Some(5), ..Filters::default() };
    assert_eq!(shown(&from_five), ["exercised:[5, 0]", "transaction:5"]);
    // Parties carry no offset, so the range leaves them alone.
    let mut parties = Filters { offset_from: Some(5), ..Filters::default() };
    parties.kinds.insert(NodeKind::Party);
    assert!(shown(&parties).contains(&"party:`Bank::1220f3a9c1d2`".to_string()));

    let carol = Filters { party: Some("Carol::1220d2a6b3c7".into()), ..Filters::default() };
    assert!(shown(&carol).contains(&"exercised:[5, 0]".to_string()));
    assert!(!shown(&carol).contains(&"transaction:1".to_string()));
}

#[test]
fn records_outside_the_event_graph_go_in_a_band_underneath() {
    let data = [serde_json::json!({ "offset": 1 }), serde_json::json!({}), serde_json::json!({})];
    let nodes = [
        ModelNode { kind: NodeKind::Transaction, data: &data[0], name: "transaction:1" },
        ModelNode { kind: NodeKind::Other, data: &data[1], name: "Alice" },
        ModelNode { kind: NodeKind::Other, data: &data[2], name: "Bob" },
    ];
    let m = Model::new(&nodes, Vec::new());
    assert_eq!(m.facts[1].title, "Alice");
    let l = canton::layout(&m, &[true, true, true], 1000.0);
    let (a, b) = (l.pos[1].unwrap(), l.pos[2].unwrap());
    assert!(a.1 > l.pos[0].unwrap().1 && a.1 == b.1);
    assert_eq!(b.0 - a.0, NODE_W + 40.0);
    assert_eq!(l.bands.last().unwrap().0, "other records");
}
