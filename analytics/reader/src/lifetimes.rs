//! Everything the explorer's dashboard shows for a contract-lifetimes table,
//! computed with Polars and handed over as plain Rust values, so drawing it
//! never touches a DataFrame.

use std::path::Path;

use anyhow::{Result, anyhow};
use polars::prelude::*;

/// Most rows kept for the "longest-lived contracts" table.
pub const MAX_ROWS: u32 = 500;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dashboard {
    /// The Delta version this was read from.
    pub version: Option<i64>,
    pub archived: usize,
    /// Over the contracts whose create was seen.
    pub mean_seconds: Option<f64>,
    pub median_seconds: Option<f64>,
    /// The newest archive offset in the table.
    pub newest_offset: Option<i64>,
    pub over_time: Timeline,
    /// Most archived template first.
    pub by_template: Vec<TemplateStats>,
    /// Longest-lived first, at most `MAX_ROWS`.
    pub longest: Vec<Lifetime>,
}

/// Archives per time bucket.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Timeline {
    /// Bucket length in seconds: an hour, a day or a week, whichever gives at
    /// most 200 buckets.
    pub bucket_seconds: i64,
    /// (bucket start, seconds since the epoch; archives in it), oldest first,
    /// with empty buckets included.
    pub buckets: Vec<(i64, u32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateStats {
    /// `None` for contracts whose create wasn't seen.
    pub template: Option<String>,
    pub archived: u32,
    pub mean_seconds: Option<f64>,
    pub max_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lifetime {
    pub contract_id: String,
    pub template: Option<String>,
    pub created_offset: Option<i64>,
    pub archived_offset: i64,
    pub lifetime_seconds: Option<f64>,
    pub archived_by: Option<String>,
}

/// Reads the table and computes the dashboard. The Polars work runs on a
/// blocking thread.
pub async fn load(table_path: &Path) -> Result<Dashboard> {
    let version = crate::latest_version(table_path)?;
    if version.is_none() {
        return Err(anyhow!("no Delta table at {} yet; run canton-analytics first", table_path.display()));
    }
    let frame = crate::scan(table_path).await?;
    let mut dashboard = tokio::task::spawn_blocking(move || compute(frame)).await??;
    dashboard.version = version;
    Ok(dashboard)
}

/// Computes the dashboard from a contract-lifetimes frame.
pub fn compute(frame: LazyFrame) -> Result<Dashboard> {
    let frame = frame.collect()?;
    if frame.height() == 0 {
        return Ok(Dashboard::default());
    }
    let frame = frame.lazy();

    let totals = frame
        .clone()
        .select([
            len().alias("archived"),
            col("lifetime_seconds").mean().alias("mean"),
            col("lifetime_seconds").median().alias("median"),
            col("archived_offset").max().alias("newest"),
        ])
        .collect()?;
    let archived = u32s(&totals, "archived")?.first().copied().flatten().unwrap_or(0) as usize;
    let mean_seconds = f64s(&totals, "mean")?.first().copied().flatten();
    let median_seconds = f64s(&totals, "median")?.first().copied().flatten();
    let newest_offset = i64s(&totals, "newest")?.first().copied().flatten();

    let by = crate::lifetimes_by_template(frame.clone()).collect()?;
    let names = strings(&by, "template_name")?;
    let counts = u32s(&by, "archived")?;
    let means = f64s(&by, "mean_seconds")?;
    let maxes = f64s(&by, "max_seconds")?;
    let by_template = (0..by.height())
        .map(|i| TemplateStats {
            template: names[i].clone(),
            archived: counts[i].unwrap_or(0),
            mean_seconds: means[i],
            max_seconds: maxes[i],
        })
        .collect();

    let times = frame
        .clone()
        .select([col("archived_at").cast(DataType::Int64).alias("micros")])
        .collect()?;
    let over_time = timeline(i64s(&times, "micros")?.into_iter().flatten().map(|us| us.div_euclid(1_000_000)));

    let longest_frame = frame
        .sort(
            ["lifetime_seconds"],
            SortMultipleOptions::default().with_order_descending(true).with_nulls_last(true),
        )
        .limit(MAX_ROWS)
        .collect()?;
    let ids = strings(&longest_frame, "contract_id")?;
    let templates = strings(&longest_frame, "template_name")?;
    let created = i64s(&longest_frame, "created_offset")?;
    let archived_at = i64s(&longest_frame, "archived_offset")?;
    let seconds = f64s(&longest_frame, "lifetime_seconds")?;
    let by_choice = strings(&longest_frame, "archived_by")?;
    let longest = (0..longest_frame.height())
        .map(|i| Lifetime {
            contract_id: ids[i].clone().unwrap_or_default(),
            template: templates[i].clone(),
            created_offset: created[i],
            archived_offset: archived_at[i].unwrap_or_default(),
            lifetime_seconds: seconds[i],
            archived_by: by_choice[i].clone(),
        })
        .collect();

    Ok(Dashboard {
        version: None,
        archived,
        mean_seconds,
        median_seconds,
        newest_offset,
        over_time,
        by_template,
        longest,
    })
}

/// Counts `seconds` (since the epoch) per hour, day or week.
fn timeline(seconds: impl Iterator<Item = i64>) -> Timeline {
    let seconds: Vec<i64> = seconds.collect();
    let (Some(&first), Some(&last)) = (seconds.iter().min(), seconds.iter().max()) else {
        return Timeline::default();
    };
    const SIZES: [i64; 3] = [3_600, 86_400, 7 * 86_400];
    let bucket = SIZES.into_iter().find(|b| (last - first) / b < 200).unwrap_or(SIZES[2]);
    let start = first.div_euclid(bucket) * bucket;
    let mut buckets: Vec<(i64, u32)> =
        (0..=(last.div_euclid(bucket) * bucket - start) / bucket).map(|i| (start + i * bucket, 0)).collect();
    for s in seconds {
        buckets[((s.div_euclid(bucket) * bucket - start) / bucket) as usize].1 += 1;
    }
    Timeline { bucket_seconds: bucket, buckets }
}

fn column<'a>(frame: &'a DataFrame, name: &str) -> Result<&'a Column> {
    frame.column(name).map_err(|e| anyhow!("the table has no {name} column: {e}"))
}

fn strings(frame: &DataFrame, name: &str) -> Result<Vec<Option<String>>> {
    let c = column(frame, name)?.cast(&DataType::String)?;
    let c = c.str()?;
    Ok((0..c.len()).map(|i| c.get(i).map(str::to_string)).collect())
}

fn f64s(frame: &DataFrame, name: &str) -> Result<Vec<Option<f64>>> {
    let c = column(frame, name)?.cast(&DataType::Float64)?;
    let c = c.f64()?;
    Ok((0..c.len()).map(|i| c.get(i)).collect())
}

fn i64s(frame: &DataFrame, name: &str) -> Result<Vec<Option<i64>>> {
    let c = column(frame, name)?.cast(&DataType::Int64)?;
    let c = c.i64()?;
    Ok((0..c.len()).map(|i| c.get(i)).collect())
}

fn u32s(frame: &DataFrame, name: &str) -> Result<Vec<Option<u32>>> {
    let c = column(frame, name)?.cast(&DataType::UInt32)?;
    let c = c.u32()?;
    Ok((0..c.len()).map(|i| c.get(i)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_picks_a_bucket_size_and_fills_gaps() {
        let t = timeline([0, 10, 7_200].into_iter());
        assert_eq!(t.bucket_seconds, 3_600);
        assert_eq!(t.buckets, vec![(0, 2), (3_600, 0), (7_200, 1)]);
        let t = timeline([0, 30 * 86_400].into_iter());
        assert_eq!(t.bucket_seconds, 86_400);
        assert_eq!(t.buckets.len(), 31);
        assert_eq!(timeline(std::iter::empty()), Timeline::default());
    }

    #[test]
    fn computes_totals_templates_and_longest() {
        let frame = df![
            "contract_id" => ["a", "b", "c"],
            "template_name" => [Some("T.A"), Some("T.A"), None],
            "created_offset" => [Some(1i64), Some(2), None],
            "archived_offset" => [3i64, 4, 5],
            "archived_at" => [Some(0i64), Some(3_600_000_000), Some(3_600_000_000)],
            "lifetime_seconds" => [Some(10.0), Some(30.0), None],
            "archived_by" => ["X", "Y", "Z"],
        ]
        .unwrap();
        let d = compute(frame.lazy()).unwrap();
        assert_eq!(d.archived, 3);
        assert_eq!(d.mean_seconds, Some(20.0));
        assert_eq!(d.newest_offset, Some(5));
        assert_eq!(d.by_template[0].template.as_deref(), Some("T.A"));
        assert_eq!(d.by_template[0].archived, 2);
        assert_eq!(d.over_time.buckets, vec![(0, 1), (3_600, 2)]);
        let order: Vec<&str> = d.longest.iter().map(|l| l.contract_id.as_str()).collect();
        assert_eq!(order, ["b", "a", "c"]);
    }
}
