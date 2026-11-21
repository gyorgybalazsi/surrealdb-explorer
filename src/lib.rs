//! Explores a SurrealDB database as a graph: opening the database
//! and turning its records into nodes and edges, and the desktop app ([`gui`]),
//! which `src/main.rs` runs.

pub mod canton;
pub mod config;
pub mod data;
pub mod db;
pub mod graph;
pub mod gui;
pub mod logging;
pub mod mcp;
pub mod query;
pub mod saved_queries;
pub mod schema;
pub mod secret;

/// The error type of fallible operations in this crate.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
