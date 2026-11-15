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

/// Whether `query` only reads: it has none of the statements that change
/// data or definitions, and calls no custom function (`fn::…`), whose body
/// may change data. Cautious: a field named like a keyword also counts.
pub fn is_read_only(query: &str) -> bool {
    !has_word(
        query,
        &[
            "CREATE", "UPDATE", "UPSERT", "DELETE", "RELATE", "INSERT", "DEFINE", "REMOVE", "ALTER", "REBUILD", "KILL",
            "LIVE", "ACCESS",
        ],
    ) && !has_transaction_statements(query)
        && !calls_custom_function(query)
}

/// `fn::name(…)`, in any case and with spaces around the `::`.
fn calls_custom_function(query: &str) -> bool {
    // Drop the spaces next to colons, so `fn :: name` reads as `fn::name`.
    let chars: Vec<char> = query.to_ascii_lowercase().chars().collect();
    let mut squeezed = String::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() {
            let next = chars[i..].iter().find(|c| !c.is_whitespace());
            if squeezed.ends_with(':') || next == Some(&':') {
                continue;
            }
        }
        squeezed.push(c);
    }
    let bytes = squeezed.as_bytes();
    squeezed
        .match_indices("fn::")
        .any(|(i, _)| i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'))
}

fn has_transaction_statements(query: &str) -> bool {
    has_word(query, &["BEGIN", "COMMIT", "CANCEL"])
}

fn has_word(query: &str, words: &[&str]) -> bool {
    query
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| words.contains(&word.to_ascii_uppercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::{has_transaction_statements, is_read_only};

    #[test]
    fn spots_transaction_statements() {
        assert!(has_transaction_statements("BEGIN; CREATE a; COMMIT;"));
        assert!(has_transaction_statements("begin transaction; select * from a; cancel transaction"));
        assert!(!has_transaction_statements("SELECT * FROM person WHERE name = 'Beginner'"));
    }

    #[test]
    fn spots_queries_that_change_data() {
        assert!(is_read_only("SELECT * FROM knows WHERE out = person:bob; SELECT updated_at FROM person"));
        assert!(!is_read_only("SELECT * FROM person; DELETE person:bob"));
        assert!(!is_read_only("relate person:a->knows->person:b"));
    }

    #[test]
    fn custom_functions_may_change_data() {
        assert!(!is_read_only("RETURN fn::wipe()"));
        assert!(!is_read_only("SELECT fn::wipe() FROM ONLY {}"));
        assert!(!is_read_only("SELECT *, fn :: score(id) FROM person"));
        assert!(!is_read_only("ACCESS account GRANT FOR USER tobie"));
        assert!(is_read_only("SELECT string::len(name), array::len(->knows) FROM person"));
        assert!(is_read_only("SELECT * FROM person WHERE myfn::x = 1"));
    }
}
