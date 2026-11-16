//! The Delta Lake table a plugin writes to.
//!
//! Each step is one commit, holding the new Parquet files, an application
//! transaction `(plugin name, cursor)`, and the plugin's state in the commit
//! info. So the rows, the cursor and the state always move together: if the
//! runner stops halfway, the next run starts from the last commit.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use deltalake::arrow::datatypes::Schema as ArrowSchema;
use deltalake::arrow::record_batch::RecordBatch;
use deltalake::kernel::engine::arrow_conversion::TryIntoKernel;
use deltalake::kernel::transaction::CommitProperties;
use deltalake::kernel::{StructType, Transaction};
use deltalake::writer::{DeltaWriter, RecordBatchWriter};
use deltalake::{DeltaTable, ensure_table_uri};
use futures_util::StreamExt;

/// Key of the plugin's state in each commit's info.
const STATE_KEY: &str = "canton_analytics";

/// Where the last commit left off.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Progress {
    /// Highest cursor value (e.g. offset) already processed.
    pub cursor: i64,
    pub state: Vec<u8>,
}

pub struct Table {
    pub table: DeltaTable,
    app_id: String,
}

impl Table {
    /// Opens the table at `path`, or creates it with `schema`.
    pub async fn open_or_create(path: &Path, name: &str, schema: &ArrowSchema) -> Result<Table> {
        std::fs::create_dir_all(path)?;
        let path = path.canonicalize()?;
        let url = ensure_table_uri(path.to_string_lossy())?;
        let mut table = DeltaTable::try_from_url(url.clone()).await?;
        let table = if table.verify_deltatable_existence().await? {
            table.load().await?;
            table
        } else {
            let columns: StructType = schema.try_into_kernel()?;
            table
                .create()
                .with_table_name(name)
                .with_comment(format!("Written by the canton-analytics plugin {name}"))
                .with_columns(columns.fields().cloned())
                .await
                .context("could not create the Delta table")?
        };
        Ok(Table { table, app_id: name.to_string() })
    }

    /// The cursor and state of the last commit.
    pub async fn progress(&self) -> Result<Progress> {
        let snapshot = self.table.snapshot()?;
        let Some(cursor) = snapshot.transaction_version(&self.table.log_store(), &self.app_id).await? else {
            return Ok(Progress::default());
        };
        // The newest commit that carries our state.
        let mut history = self.table.history(None);
        while let Some(commit) = history.next().await {
            if let Some(info) = commit?.info.get(STATE_KEY) {
                let state = info.get("state").and_then(|s| s.as_str()).unwrap_or_default();
                return Ok(Progress { cursor, state: BASE64.decode(state)? });
            }
        }
        Err(anyhow!("the table has a cursor but no plugin state"))
    }

    /// Appends `batches` and records `progress`, in one commit.
    pub async fn append(&mut self, batches: Vec<RecordBatch>, progress: &Progress) -> Result<()> {
        let info = serde_json::json!({ "cursor": progress.cursor, "state": BASE64.encode(&progress.state) });
        let properties = CommitProperties::default()
            .with_application_transaction(Transaction::new(&self.app_id, progress.cursor))
            .with_metadata([(STATE_KEY.to_string(), info)]);
        let mut writer = RecordBatchWriter::for_table(&self.table)?.with_commit_properties(properties);
        for batch in batches.into_iter().filter(|b| b.num_rows() > 0) {
            writer.write(batch).await?;
        }
        writer.flush_and_commit(&mut self.table).await?;
        Ok(())
    }

    /// Paths of the Parquet files in the current version of the table.
    pub fn files(&self) -> Result<Vec<String>> {
        Ok(self.table.get_file_uris()?.map(|u| u.strip_prefix("file://").map(str::to_string).unwrap_or(u)).collect())
    }
}
