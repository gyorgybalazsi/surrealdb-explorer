//! Desktop viewer for a SurrealDB database as an interactive graph, built with iced.
//!
//! Run with `cargo run`. Settings are in `config.toml` (created on first run,
//! also editable in the app), logs go to the terminal and to `logs/`.
//! The app keeps the database open while running, so only one copy can run
//! at a time. Sample data can be loaded from the app (Settings → Data).
//!
//! `surrealdb-explorer mcp` runs an MCP server on stdin/stdout instead of the
//! window, for agents such as Claude Code (see `mcp`). It opens the same
//! database, so it can't run while the app has it open.

mod gui;

use surrealdb_explorer::config::Config;
use surrealdb_explorer::{db, logging, mcp};

const MCP_USAGE: &str = "\
Usage: surrealdb-explorer mcp [--dir <folder>] [--allow-writes]

Runs an MCP server on stdin/stdout for agents such as Claude Code.
  --dir <folder>   folder with config.toml and the database (default: the current folder)
  --allow-writes   let run_query change data and definitions (default: read-only)

Add it to Claude Code with:
  claude mcp add surrealdb-explorer -- /path/to/surrealdb-explorer mcp --dir /path/to/folder";

fn main() -> iced::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("mcp") {
        if let Err(e) = run_mcp(&args[1..]) {
            eprintln!("surrealdb-explorer mcp: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let (config, created, startup_error) = match Config::load_or_create() {
        Ok((config, created)) => (config, created, None),
        Err(e) => {
            let message = format!("Could not read {}: {e}. Using defaults.", Config::path().display());
            (Config::default(), false, Some(message))
        }
    };

    // Keep the guard until the app exits, so the log file is flushed.
    let (_log_guard, log_control) = match logging::init(&config.logging) {
        Ok((guard, control)) => (Some(guard), Some(control)),
        Err(e) => {
            eprintln!("Logging is off: {e}");
            (None, None)
        }
    };

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "surrealdb-explorer starting");
    if created {
        tracing::info!(path = %Config::path().display(), "created a default config file");
    }
    if let Some(e) = &startup_error {
        tracing::error!("{e}");
    }

    let result = gui::run(config, startup_error, log_control);
    tracing::info!("surrealdb-explorer exiting");
    result
}

/// `surrealdb-explorer mcp …`: serves MCP on stdin/stdout until the client
/// disconnects. Nothing but MCP messages may go to stdout, so logs go to
/// stderr and the log folder.
fn run_mcp(args: &[String]) -> Result<(), surrealdb_explorer::Error> {
    let mut options = mcp::Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--allow-writes" => options.allow_writes = true,
            "--dir" => {
                let dir = args.next().ok_or("--dir needs a folder")?;
                std::env::set_current_dir(dir).map_err(|e| format!("can't use folder {dir}: {e}"))?;
            }
            "-h" | "--help" => {
                eprintln!("{MCP_USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown option {other}\n\n{MCP_USAGE}").into()),
        }
    }

    let (config, _) = Config::load_or_create()?;
    let _log_guard = match logging::init_to_stderr(&config.logging) {
        Ok((guard, _)) => Some(guard),
        Err(e) => {
            eprintln!("Logging is off: {e}");
            None
        }
    };
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "surrealdb-explorer MCP server starting");

    // Embedded SurrealDB needs bigger thread stacks than tokio's default.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(10 * 1024 * 1024)
        .build()?;
    runtime.block_on(async move {
        let database = db::connect(&config.database).await.map_err(|e| {
            format!(
                "could not open the database at {}: {e} (is the explorer app open on it?)",
                config.database.path
            )
        })?;
        let served = mcp::serve_stdio(database.clone(), options).await;
        db::close(database).await?;
        served
    })
}
