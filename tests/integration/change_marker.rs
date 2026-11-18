//! Auto refresh's check for changes: a fingerprint that changes when records
//! are added or removed, without counting every record of a big table.

use crate::common;

use surrealdb_explorer::graph::{self, Watch};
use surrealdb_explorer::{data, db};

#[test]
fn the_tables_marker_changes_when_drawn_records_are_added_or_removed() {
    let config = common::temp_database("change-marker-tables");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        let watch = Watch::Tables { per_table: 2 };
        let marker = |db: db::Database| async move { graph::change_marker(&db, watch).await.unwrap() };

        let empty = marker(db.clone()).await;
        db.query("CREATE person:a; CREATE person:b; RELATE person:a->knows->person:b;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let before = marker(db.clone()).await;
        assert_ne!(before, empty);

        // Changing a field doesn't count; adding or removing does.
        db.query("UPDATE person:a SET name = 'A';").await.unwrap().check().unwrap();
        assert_eq!(marker(db.clone()).await, before);
        db.query("CREATE person:c;").await.unwrap().check().unwrap();
        let three = marker(db.clone()).await;
        assert_ne!(three, before, "one more than the limit shows the table went over it");
        // Past the limit and its one extra, more records aren't drawn and aren't read.
        db.query("CREATE person:d;").await.unwrap().check().unwrap();
        assert_eq!(marker(db.clone()).await, three);
        db.query("DELETE person:a;").await.unwrap().check().unwrap();
        assert_ne!(marker(db.clone()).await, three);
        // A new table counts too, even an empty one.
        let now = marker(db.clone()).await;
        db.query("DEFINE TABLE empty;").await.unwrap().check().unwrap();
        assert_ne!(marker(db.clone()).await, now);
    });
}

#[test]
fn the_canton_marker_follows_the_newest_transaction_or_the_syncers_state() {
    let config = common::temp_database("change-marker-canton");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        let marker = |db: db::Database| async move { graph::change_marker(&db, Watch::Canton).await.unwrap() };
        data::load_canton_sample(&db).await.unwrap();

        let sample = marker(db.clone()).await;
        assert_eq!(sample["marker"], serde_json::json!([5]), "{sample}");
        db.query("UPSERT transaction:6 MERGE { offset: 6 };").await.unwrap().check().unwrap();
        let newer = marker(db.clone()).await;
        assert_ne!(newer, sample);

        // With the syncer's sync_state, that is what's read.
        db.query("UPSERT sync_state:stream SET last_offset = 6;").await.unwrap().check().unwrap();
        let synced = marker(db.clone()).await;
        assert_eq!(synced["marker"][0]["last_offset"], 6, "{synced}");
        db.query("UPSERT sync_state:stream SET last_offset = 7;").await.unwrap().check().unwrap();
        assert_ne!(marker(db.clone()).await, synced);
    });
}
