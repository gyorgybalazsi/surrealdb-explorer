//! The original SurrealDB getting-started examples.
//!
//! Run with `cargo run --example getting_started`.
//! Uses the same `./data` database as the graph server, so stop the server first:
//! only one program can have the database open at a time.

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

/// A person as read back from the database: the id plus every payload field,
/// including the nested `name`. Fields are optional because not every person
/// record has all of them (for example, if `person:jaime` was never seeded).
#[derive(Debug, SurrealValue)]
#[allow(dead_code)] // fields are only read through `Debug` printing
struct PersonRecord {
    id: RecordId,
    title: Option<String>,
    name: Option<Name>,
    marketing: Option<bool>,
}

/// One row of the `GROUP BY marketing` query.
#[derive(Debug, SurrealValue)]
#[allow(dead_code)]
struct MarketingGroup {
    marketing: Option<bool>,
    count: i64,
}

#[tokio::main]
async fn main() -> Result<(), surrealdb_explore::Error> {
    // Open the project's database (SurrealKV in ./data, namespace and database "main")
    let db = db::connect().await?;

    // Create or update a person with a fixed id
    let created: Option<PersonRecord> = db
        .upsert(("person", "tobie"))
        .content(Person {
            title: "Founder & CEO".to_string(),
            name: Name {
                first: "Tobie".to_string(),
                last: "Morgan Hitchcock".to_string(),
            },
            marketing: true,
        })
        .await?;
    println!("Created: {created:#?}");

    // Update a person record with a specific id
    let updated: Option<PersonRecord> = db
        .update(("person", "jaime"))
        .merge(Responsibility { marketing: true })
        .await?;
    println!("Updated: {updated:#?}");

    // Select all people records
    let people: Vec<PersonRecord> = db.select("person").await?;
    println!("People: {people:#?}");

    // Perform a custom advanced query
    let groups: Vec<MarketingGroup> = db
        .query("SELECT marketing, count() FROM type::table($table) GROUP BY marketing")
        .bind(("table", "person"))
        .await?
        .take(0)?;
    println!("Groups: {groups:#?}");

    Ok(())
}
