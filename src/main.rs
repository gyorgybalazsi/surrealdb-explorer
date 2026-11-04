//! Starts the graph server: opens the database and serves the page.
//! It only reads the database. For sample data, run `cargo run --example seed`.

use surrealdb_explore::{db, graph, server};

fn main() -> Result<(), surrealdb_explore::Error> {
    // Embedded SurrealDB needs a bigger stack than tokio worker threads get by
    // default (2 MB); SurrealDB's docs recommend 10 MB. With the default, queries
    // run from web requests spun forever at 100% CPU here.
    // See https://docs.rs/surrealdb/latest/surrealdb/engine/local/index.html
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(10 * 1024 * 1024)
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), surrealdb_explore::Error> {
    let db = db::connect().await?;

    let loaded = graph::load(&db).await?;
    println!(
        "Loaded {} records and {} relations",
        loaded.nodes.len(),
        loaded.edges.len()
    );

    server::serve(db).await
}
