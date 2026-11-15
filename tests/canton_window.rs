//! Loading a window of a Canton ledger: the latest transactions, or a range, with their events.

mod common;

use surrealdb_explorer::graph::{CantonWindow, load_canton_window};
use surrealdb_explorer::{data, db};

#[test]
fn a_window_holds_its_transactions_and_their_events() {
    let config = common::temp_database("canton-window");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        data::load_canton_sample(&db).await.unwrap();

        let txs = |loaded: &surrealdb_explorer::graph::CantonWindowLoaded| {
            let mut offsets: Vec<i64> = loaded
                .data
                .nodes
                .iter()
                .filter(|n| n.table == "transaction")
                .filter_map(|n| n.data["offset"].as_i64())
                .collect();
            offsets.sort();
            offsets
        };

        let all = load_canton_window(&db, CantonWindow::Latest(1000), 100, 5000).await.unwrap();
        let every = txs(&all);
        assert!(every.len() >= 3, "{every:?}");
        assert_eq!(all.offsets, Some((every[0], *every.last().unwrap())));

        // The newest two, and every event at their offsets.
        let latest = load_canton_window(&db, CantonWindow::Latest(2), 100, 5000).await.unwrap();
        assert_eq!(txs(&latest), every[every.len() - 2..].to_vec());
        let (lo, hi) = latest.offsets.unwrap();
        for n in &latest.data.nodes {
            if let (Some(o), false) = (n.data["offset"].as_i64(), n.table == "party") {
                // Events in the window, or earlier contracts its events act on.
                assert!(o <= hi, "{} at {o}", n.id);
                if o < lo {
                    assert_eq!(n.table, "created", "{} at {o}", n.id);
                }
            }
        }
        assert!(latest.data.nodes.iter().any(|n| n.table == "exercised" || n.table == "created"));

        // Paging back from the window gets the ones before it.
        let before = load_canton_window(&db, CantonWindow::Before(lo, 1), 100, 5000).await.unwrap();
        assert_eq!(txs(&before), vec![every[every.len() - 3]]);

        // Over the limit, the transactions are kept and their events, parties
        // and other records are left out first.
        let tight = load_canton_window(&db, CantonWindow::Latest(1000), 100, every.len()).await.unwrap();
        assert_eq!(txs(&tight), every);
        assert_eq!(tight.data.nodes.len(), every.len());
        assert!(tight.left_out > 0);

        // A range.
        let range = load_canton_window(&db, CantonWindow::Range(every[0], every[1]), 100, 5000).await.unwrap();
        assert_eq!(txs(&range), every[..2].to_vec());
    });
}
