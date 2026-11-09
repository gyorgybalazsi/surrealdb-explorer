//! Checks a generated query against the database without changing anything:
//! it runs inside a transaction that is then cancelled.

use crate::db::Database;

/// What SurrealDB reports for statements that ran fine inside a cancelled
/// transaction. Any other error is a problem with the query.
const CANCELLED: &str = "cancelled transaction";

/// Returns the first problem SurrealDB finds with `query`, or `None` if it
/// runs. Queries with their own transaction statements can't be wrapped and
/// are not checked.
pub async fn check(db: &Database, query: &str) -> Option<String> {
    check_with(db, query, &serde_json::Value::Object(Default::default())).await
}

/// Like `check`, with parameters the query can read (see `query::run_with`).
pub async fn check_with(db: &Database, query: &str, params: &serde_json::Value) -> Option<String> {
    if query.trim().is_empty() {
        return Some("the query is empty".into());
    }
    if has_transaction_statements(query) {
        return None;
    }
    // New lines around the query, so a trailing `-- comment` can't swallow
    // the CANCEL.
    let sql = format!("BEGIN TRANSACTION;\n{query}\n;CANCEL TRANSACTION;");
    match db.query(sql).bind(params.clone()).await {
        Err(e) => Some(e.to_string()),
        Ok(mut response) => {
            let mut errors: Vec<(usize, String)> = response
                .take_errors()
                .into_iter()
                .map(|(i, e)| (i, e.to_string()))
                .filter(|(_, e)| !e.contains(CANCELLED))
                .collect();
            errors.sort();
            errors.into_iter().next().map(|(_, e)| e)
        }
    }
}

/// Whether `query` only reads: it has none of the statements that change
/// data or definitions. Cautious: a field named like a keyword also counts.
pub fn is_read_only(query: &str) -> bool {
    !has_word(
        query,
        &["CREATE", "UPDATE", "UPSERT", "DELETE", "RELATE", "INSERT", "DEFINE", "REMOVE", "ALTER", "REBUILD", "KILL", "LIVE"],
    ) && !has_transaction_statements(query)
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
}
