//! One plugin, one table, stepping through the database in cursor ranges.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use tracing::info;

use crate::plugin::Plugin;
use crate::source::{self, Db};
use crate::table::{Progress, Table};

pub struct Runner {
    pub plugin: Plugin,
    pub table: Table,
    pub progress: Progress,
    /// Largest cursor range per step, e.g. this many offsets.
    pub batch: i64,
}

/// What a step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub from: i64,
    pub to: i64,
    pub rows: usize,
}

impl Runner {
    pub async fn new(plugin_path: &Path, table_path: &Path, batch: i64) -> Result<Runner> {
        let plugin = tokio::task::block_in_place(|| Plugin::load(plugin_path))?;
        let table = Table::open_or_create(table_path, &plugin.info.name, &plugin.schema).await?;
        let progress = table.progress().await?;
        info!(plugin = %plugin.info.name, version = %plugin.info.version, cursor = progress.cursor, "plugin loaded");
        Ok(Runner { plugin, table, progress, batch: batch.max(1) })
    }

    /// Processes the next cursor range, if the database has one.
    pub async fn step(&mut self, db: &Db) -> Result<Option<Step>> {
        let high = source::high_water(db, &self.plugin.source.high_water).await?;
        let from = self.progress.cursor;
        if high <= from {
            return Ok(None);
        }
        let to = high.min(from.saturating_add(self.batch));
        let results = source::run(db, &self.plugin.source.query, from, to).await?;
        let state = std::mem::take(&mut self.progress.state);
        let transformed = tokio::task::block_in_place(|| self.plugin.transform(results, state.clone()));
        let (batches, new_state) = match transformed {
            Ok(out) => out,
            Err(e) => {
                self.progress.state = state;
                return Err(e);
            }
        };
        let rows = batches.iter().map(|b| b.num_rows()).sum();
        let next = Progress { cursor: to, state: new_state };
        if let Err(e) = self.table.append(batches, &next).await {
            self.progress.state = state;
            return Err(e);
        }
        self.progress = next;
        info!(from, to, rows, "step committed");
        Ok(Some(Step { from, to, rows }))
    }

    /// Steps until the plugin has caught up with the database.
    pub async fn catch_up(&mut self, db: &Db) -> Result<Vec<Step>> {
        let mut steps = Vec::new();
        while let Some(step) = self.step(db).await? {
            steps.push(step);
        }
        Ok(steps)
    }

    /// Catches up, then keeps following the database, checking every `every`.
    pub async fn follow(&mut self, db: &Db, every: Duration) -> Result<()> {
        loop {
            self.catch_up(db).await?;
            tokio::time::sleep(every).await;
        }
    }
}
