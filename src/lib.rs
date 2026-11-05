//! Explores a SurrealDB database as a graph: opening the database
//! and turning its records into nodes and edges. The desktop app is in `src/main.rs`.

pub mod config;
pub mod db;
pub mod graph;
pub mod logging;
pub mod query;

/// The error type of fallible operations in this crate.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
