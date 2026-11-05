//! Running SurrealQL typed by the user.

use std::time::{Duration, Instant};

use surrealdb::types::Value;

use crate::db::Database;

/// The outcome of one statement of a query.
#[derive(Debug, Clone)]
pub struct StatementResult {
    /// Time the database reports for the statement.
    pub time: Option<Duration>,
    /// The result as JSON, or the error message.
    pub result: Result<serde_json::Value, String>,
}

/// The outcome of a whole query.
#[derive(Debug, Clone)]
pub struct QueryOutcome {
    /// One entry per statement, or a single error if the query didn't run
    /// at all (for example, a syntax error).
    pub statements: Result<Vec<StatementResult>, String>,
    /// Wall-clock time of the whole query.
    pub elapsed: Duration,
}

/// Runs `sql` and collects every statement's result.
pub async fn run(db: &Database, sql: String) -> QueryOutcome {
    let started = Instant::now();
    let statements = match db.query(sql.as_str()).with_stats().await {
        Ok(mut response) => Ok((0..response.num_statements())
            .filter_map(|i| response.take::<Value>(i))
            .map(|(stats, result)| StatementResult {
                time: stats.execution_time,
                result: result.map(Value::into_json_value).map_err(|e| e.to_string()),
            })
            .collect::<Vec<_>>()),
        Err(e) => Err(e.to_string()),
    };
    let elapsed = started.elapsed();

    match &statements {
        Ok(results) => {
            let failed = results.iter().filter(|s| s.result.is_err()).count();
            tracing::info!(
                statements = results.len(),
                failed,
                elapsed_ms = elapsed.as_millis() as u64,
                "query ran"
            );
        }
        Err(e) => tracing::warn!(error = %e, "query failed"),
    }
    tracing::debug!(%sql, "query text");

    QueryOutcome { statements, elapsed }
}

/// Every string in the results that looks like a record id (`table:key`).
/// Used to show a query's records in the graph; the caller keeps only ids
/// that are actually in the graph.
pub fn record_ids(outcome: &QueryOutcome) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) if s.contains(':') => out.push(s.clone()),
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            serde_json::Value::Object(o) => o.values().for_each(|i| walk(i, out)),
            _ => {}
        }
    }
    let mut ids = Vec::new();
    if let Ok(statements) = &outcome.statements {
        for s in statements {
            if let Ok(v) = &s.result {
                walk(v, &mut ids);
            }
        }
    }
    ids
}
