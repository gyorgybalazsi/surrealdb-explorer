//! Following one Canton record through the ledger: its transaction, the
//! event that archived a contract, and a contract's lifecycle.

use crate::common;

use surrealdb_explorer::graph::{ResultGraph, archiving_event, lifecycle, transaction_of};
use surrealdb_explorer::{data, db};

/// The drawn record whose id contains `part`, if there is one.
fn has(g: &ResultGraph, part: &str) -> bool {
    g.data.nodes.iter().any(|n| n.id.contains(part))
}

#[test]
fn a_records_transaction_archive_and_lifecycle() {
    let config = common::temp_database("canton_record");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        data::load_canton_sample(&db).await.unwrap();

        // Bob's cash transfer sits in transaction 3, with every event in it.
        let tx = transaction_of(&db, "exercised:[3, 1]", 100).await.unwrap();
        for part in ["transaction:3", "3, 0", "3, 1", "00c3c7d4", "00c4d8e5"] {
            assert!(has(&tx, part), "{part} is in transaction 3: {:?}", tx.data.nodes);
        }
        assert!(!has(&tx, "transaction:4") && !has(&tx, "4, 0"), "nothing from another transaction");
        assert!(tx.data.edges.iter().any(|e| e.label == "action"), "the tree's relations come too");

        // The first cash was spent by that transfer.
        let (by, archive) = archiving_event(&db, "created:⟨00c1a5b2⟩", 100).await.unwrap().expect("it was archived");
        assert!(by.contains("3, 1"), "{by}");
        assert!(has(&archive, "00c1a5b2") && has(&archive, "transaction:3"));
        // Carol's ticket was only shown, not archived; a transaction isn't a contract.
        assert!(archiving_event(&db, "created:⟨00c5e9f6⟩", 100).await.unwrap().is_none());
        assert!(archiving_event(&db, "transaction:1", 100).await.unwrap().is_none());

        // Bob's ticket: created in transaction 3, given to Carol in 4.
        let life = lifecycle(&db, "created:⟨00c4d8e5⟩", 100).await.unwrap().expect("a lifecycle");
        assert!(life.contract.contains("00c4d8e5"));
        assert_eq!(life.events, 1);
        for part in ["transaction:3", "transaction:4", "4, 0", "00c5e9f6"] {
            assert!(has(&life.graph, part), "{part}: {:?}", life.graph.data.nodes);
        }
        assert!(!has(&life.graph, "transaction:5"));

        // From an exercise, the lifecycle of the contract it acts on.
        let life = lifecycle(&db, "exercised:[5, 0]", 100).await.unwrap().expect("a lifecycle");
        assert!(life.contract.contains("00c5e9f6"), "{}", life.contract);
        assert!(has(&life.graph, "transaction:4") && has(&life.graph, "transaction:5"));

        assert!(lifecycle(&db, "transaction:1", 100).await.unwrap().is_none());
    });
}
