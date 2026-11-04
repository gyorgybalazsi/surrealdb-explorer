//! Explores a SurrealDB database and shows it as an interactive graph in the browser.

pub mod db;
pub mod graph;
pub mod key;
pub mod server;

/// The error type of fallible operations in this crate.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
