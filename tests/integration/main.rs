//! All integration tests, as one test binary. Each binary links the whole of
//! SurrealDB, so one binary instead of one per file saves most of the link
//! time and disk space. Run one area with e.g. `cargo test --test integration canton::`.
//!
//! Tests here run in parallel in one process: give each a unique temporary
//! database name and port.

mod common;

mod canton;
mod canton_window;
mod change_marker;
mod config_and_query;
mod data;
mod explore;
mod mcp;
mod result_graph;
mod schema;
mod server;
mod typed_records;
