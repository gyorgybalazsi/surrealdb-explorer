//! Fills the database with sample data for the graph: a few people with
//! `knows` relations between them, and a company two of them link to.
//!
//! Run with `cargo run --example seed`. Stop the graph server first: only one
//! program can have the database open at a time.
//!
//! Safe to run repeatedly: records are upserted and relations are recreated.
//! Note that it deletes all existing `knows` relations before adding the sample ones.

use surrealdb_explore::db;

#[tokio::main]
async fn main() -> Result<(), surrealdb_explore::Error> {
    let db = db::connect().await?;

    db.query(
        "
        UPSERT person:tobie MERGE { name: { first: 'Tobie', last: 'Morgan Hitchcock' }, title: 'Founder & CEO', marketing: true };
        UPSERT person:alice MERGE { name: { first: 'Alice', last: 'Smith' }, title: 'CTO',      marketing: false };
        UPSERT person:bob   MERGE { name: { first: 'Bob',   last: 'Jones' }, title: 'Engineer', marketing: false };
        UPSERT person:carol MERGE { name: { first: 'Carol', last: 'White' }, title: 'Designer', marketing: true };
        UPSERT person:dave  MERGE { name: { first: 'Dave',  last: 'Brown' }, title: 'Sales',    marketing: true };
        UPSERT person:jaime MERGE { name: { first: 'Jaime', last: 'Lee' },   title: 'Engineer' };

        -- A second table, connected through record-link fields rather than RELATE.
        UPSERT company:surrealdb MERGE { name: 'SurrealDB', city: 'London' };
        UPSERT person:tobie MERGE { company: company:surrealdb };
        UPSERT person:jaime MERGE { company: company:surrealdb };

        DEFINE TABLE IF NOT EXISTS knows TYPE RELATION;
        DELETE knows;
        RELATE person:tobie->knows->person:alice;
        RELATE person:alice->knows->person:bob;
        RELATE person:alice->knows->person:carol;
        RELATE person:bob->knows->person:dave;
        RELATE person:carol->knows->person:dave;
        RELATE person:dave->knows->person:jaime;
        RELATE person:jaime->knows->person:alice;
        ",
    )
    .await?
    .check()?;

    println!("Sample data added. Start the graph with `cargo run`.");
    Ok(())
}
