//! Auto refresh's check for changes: how many records each table has.

mod common;

use surrealdb_explorer::{db, graph};

#[test]
fn record_counts_change_when_records_are_added_or_removed() {
    let config = common::temp_database("record-counts");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        assert!(graph::record_counts(&db).await.unwrap().is_empty());

        db.query("CREATE person:a; CREATE person:b; DEFINE TABLE empty; RELATE person:a->knows->person:b;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let counts = graph::record_counts(&db).await.unwrap();
        assert_eq!(
            counts,
            vec![("empty".to_string(), 0), ("knows".to_string(), 1), ("person".to_string(), 2)]
        );

        // Changing a field doesn't count; adding or removing does.
        db.query("UPDATE person:a SET name = 'A';").await.unwrap().check().unwrap();
        assert_eq!(graph::record_counts(&db).await.unwrap(), counts);
        db.query("DELETE person:b;").await.unwrap().check().unwrap();
        assert_ne!(graph::record_counts(&db).await.unwrap(), counts);
    });
}
