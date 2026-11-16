//! Reading a plugin's Delta table with Polars.

use std::path::Path;

use anyhow::{Context, Result};
use deltalake::ensure_table_uri;
use deltalake::kernel::engine::arrow_conversion::TryIntoArrow;
use polars::prelude::*;

/// The table's current version as a Polars lazy frame. Reads a consistent
/// snapshot: files the runner is writing meanwhile are not part of it until
/// their commit lands.
pub async fn scan(table_path: &Path) -> Result<LazyFrame> {
    let path = table_path.canonicalize().with_context(|| format!("no table at {}", table_path.display()))?;
    let table = deltalake::open_table(ensure_table_uri(path.to_string_lossy())?).await?;
    let files: Vec<PlRefPath> = table
        .get_file_uris()?
        .map(|u| PlRefPath::new(u.strip_prefix("file://").map(str::to_string).unwrap_or(u)))
        .collect();
    if files.is_empty() {
        // No rows yet: an empty frame with the table's columns.
        let schema = table.snapshot()?.schema();
        let arrow: deltalake::arrow::datatypes::Schema = schema.as_ref().try_into_arrow()?;
        let columns: Vec<Column> = arrow
            .fields()
            .iter()
            .map(|f| Column::new_empty(f.name().into(), &DataType::Null))
            .collect();
        return Ok(DataFrame::new_infer_height(columns)?.lazy());
    }
    Ok(LazyFrame::scan_parquet_sources(ScanSources::Paths(files.into_iter().collect()), ScanArgsParquet::default())?)
}

/// For the contract-lifetimes plugin: per template, how many contracts were
/// archived and how long they lived.
pub fn lifetimes_by_template(frame: LazyFrame) -> LazyFrame {
    frame
        .group_by([col("template_name")])
        .agg([
            len().alias("archived"),
            col("lifetime_seconds").mean().alias("mean_seconds"),
            col("lifetime_seconds").max().alias("max_seconds"),
        ])
        .sort(["archived"], SortMultipleOptions::default().with_order_descending(true))
}
