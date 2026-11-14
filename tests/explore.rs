//! Exploring a big database: the table overview, one table's records, and a record's neighbours.

mod common;

use surrealdb_explorer::{data, db, graph};

#[test]
fn overview_table_records_and_neighbours() {
    let config = common::temp_database("explore");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        data::load_sample_data(&db).await.unwrap();
        let records = graph::record_count(&db).await.unwrap();
        assert!(records > 0);

        // One node per record table, with its count; relations between tables.
        let overview = graph::overview(&db).await.unwrap();
        let person = overview.nodes.iter().find(|n| n.table == "person").expect("a person node");
        assert!(person.id.starts_with(graph::TABLE_NODE));
        assert!(person.data["records"].as_i64().unwrap() > 0);
        assert!(overview.edges.iter().any(|e| e.label.starts_with("knows (")), "{:?}", overview.edges);
        assert!(overview.nodes.iter().map(|n| n.data["records"].as_i64().unwrap() as usize).sum::<usize>() == records);

        // A table's records, at most the limit, with relations between them.
        let people = graph::table_records(&db, "person", 3).await.unwrap();
        assert!(people.data.nodes.len() <= 3);
        assert!(people.data.nodes.iter().all(|n| n.table == "person"));

        // Bob, his relations both ways, and the records at their other ends.
        let around = graph::neighbours(&db, "person:bob", 100).await.unwrap();
        assert!(around.data.nodes.iter().any(|n| n.id == "person:bob"));
        assert!(!around.data.edges.is_empty());
        assert!(around.data.edges.iter().all(|e| e.from == "person:bob" || e.to == "person:bob"));
        assert!(around.data.nodes.len() > 1);
    });
}
