//! Runs WASM analytics plugins over a SurrealDB database and appends their
//! output to Delta Lake tables, which Polars (or DuckDB, Spark, Python…) reads.
//!
//! - [`plugin`] loads a plugin component and calls it, in a sandbox.
//! - [`source`] connects to SurrealDB and runs the plugin's queries.
//! - [`table`] creates the Delta table and appends to it, with the cursor and
//!   the plugin's state in the same commit.
//! - [`runner`] ties these together, one step at a time.
//! - [`analysis`] reads a table with Polars.

pub mod analysis;
pub mod plugin;
pub mod runner;
pub mod source;
pub mod table;
