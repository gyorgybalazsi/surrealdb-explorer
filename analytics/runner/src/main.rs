//! `canton-analytics`: runs a WASM analytics plugin over SurrealDB into a
//! Delta Lake table, and shows a table with Polars.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use canton_analytics::{analysis, runner::Runner, source};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a plugin: catch up with the database, then keep following it.
    Run {
        /// The plugin, a WebAssembly component (.wasm).
        #[arg(long)]
        plugin: PathBuf,
        /// Folder of the Delta table; created if missing.
        #[arg(long)]
        table: PathBuf,
        #[command(flatten)]
        connection: source::Connection,
        /// Largest cursor range (e.g. offsets) per commit.
        #[arg(long, default_value_t = 10_000)]
        batch: i64,
        /// Stop once caught up, instead of following the database.
        #[arg(long)]
        once: bool,
        /// Seconds between checks for new data while following.
        #[arg(long, default_value_t = 5)]
        every: u64,
    },
    /// Print a table with Polars.
    Show {
        #[arg(long)]
        table: PathBuf,
        /// Rows to print.
        #[arg(long, default_value_t = 20)]
        rows: u32,
        /// Summarise contract lifetimes per template instead.
        #[arg(long)]
        by_template: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Run { plugin, table, connection, batch, once, every } => {
            let db = source::connect(&connection).await?;
            let mut runner = Runner::new(&plugin, &table, batch).await?;
            if once {
                let steps = runner.catch_up(&db).await?;
                let rows: usize = steps.iter().map(|s| s.rows).sum();
                eprintln!("{} commits, {rows} rows; cursor at {}", steps.len(), runner.progress.cursor);
            } else {
                runner.follow(&db, Duration::from_secs(every.max(1))).await?;
            }
        }
        Command::Show { table, rows, by_template } => {
            let frame = analysis::scan(&table).await?;
            let frame = if by_template { analysis::lifetimes_by_template(frame) } else { frame };
            println!("{}", frame.limit(rows).collect()?);
        }
    }
    Ok(())
}
