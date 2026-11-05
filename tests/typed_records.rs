//! Reading and writing records as Rust structs, including a nested struct.
//! (This was the `getting_started` example.)

mod common;

use surrealdb::types::{RecordId, SurrealValue};
use surrealdb_explore::db;

#[derive(Debug, SurrealValue)]
struct Name {
    first: String,
    last: String,
}

#[derive(Debug, SurrealValue)]
struct Person {
    title: String,
    name: Name,
    marketing: bool,
}

#[derive(Debug, SurrealValue)]
struct Responsibility {
    marketing: bool,
}

/// A person as read back: the id plus every payload field, including the
/// nested `name`. Fields are optional because not every person record has
/// all of them.
#[derive(Debug, SurrealValue)]
struct PersonRecord {
    id: RecordId,
    title: Option<String>,
    name: Option<Name>,
    marketing: Option<bool>,
}

/// One row of a `GROUP BY marketing` query.
#[derive(Debug, SurrealValue)]
struct MarketingGroup {
    marketing: Option<bool>,
    count: i64,
}

#[test]
fn records_round_trip_through_structs() {
    let config = common::temp_database("typed");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();

        // Create a person with a fixed id; the nested `Name` is stored as an object.
        let created: Option<PersonRecord> = db
            .upsert(("person", "tobie"))
            .content(Person {
                title: "Founder & CEO".into(),
                name: Name { first: "Tobie".into(), last: "Morgan Hitchcock".into() },
                marketing: true,
            })
            .await
            .unwrap();
        let created = created.unwrap();
        assert_eq!(created.id, RecordId::new("person", "tobie"));
        let name = created.name.unwrap();
        assert_eq!((name.first.as_str(), name.last.as_str()), ("Tobie", "Morgan Hitchcock"));
        assert_eq!(created.title.as_deref(), Some("Founder & CEO"));

        // `update … merge` on a missing record does nothing; `upsert` creates it.
        let updated: Option<PersonRecord> =
            db.update(("person", "jaime")).merge(Responsibility { marketing: true }).await.unwrap();
        assert!(updated.is_none());
        let upserted: Option<PersonRecord> =
            db.upsert(("person", "jaime")).merge(Responsibility { marketing: false }).await.unwrap();
        let jaime = upserted.unwrap();
        assert!(jaime.name.is_none(), "merge sets only the given fields");
        assert_eq!(jaime.marketing, Some(false));

        // Select all people into structs.
        let people: Vec<PersonRecord> = db.select("person").await.unwrap();
        assert_eq!(people.len(), 2);

        // A custom query, read into structs.
        let mut groups: Vec<MarketingGroup> = db
            .query("SELECT marketing, count() FROM type::table($table) GROUP BY marketing")
            .bind(("table", "person"))
            .await
            .unwrap()
            .take(0)
            .unwrap();
        groups.sort_by_key(|g| g.marketing);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.count == 1));
    });
}
