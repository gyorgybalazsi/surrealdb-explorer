//! Helpers shared by the tests: a temporary database and a runtime with the
//! bigger thread stacks embedded SurrealDB needs.

#![allow(dead_code)]

use std::path::PathBuf;

use surrealdb_explore::config::DatabaseConfig;

pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("surrealdb-explore-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn temp_database(name: &str) -> DatabaseConfig {
    DatabaseConfig {
        path: temp_dir(name).join("data").to_string_lossy().into_owned(),
        ..DatabaseConfig::default()
    }
}

/// Runs `future` to completion on a runtime whose threads have the bigger
/// stack embedded SurrealDB needs. It runs on a worker thread, not the test's
/// own thread: test threads get only 2 MB of stack, on which some SurrealDB
/// queries spin forever.
pub fn run<F>(future: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(10 * 1024 * 1024)
        .build()
        .unwrap();
    runtime.block_on(runtime.spawn(future)).unwrap()
}
