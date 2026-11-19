//! All integration tests, as one test binary: one binary instead of one per
//! file saves link time and disk space. Run one area with e.g. `cargo test --test integration canton::`.
//!
//! The tests need the `surreal` program: see `common`. They run in parallel
//! in one process: give each a unique database name.

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
