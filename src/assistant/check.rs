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
    if query.trim().is_empty() {
        return Some("the query is empty".into());
    }
    if has_transaction_statements(query) {
        return None;
    }
    // New lines around the query, so a trailing `-- comment` can't swallow
    // the CANCEL.
    let sql = format!("BEGIN TRANSACTION;\n{query}\n;CANCEL TRANSACTION;");
    match db.query(sql).await {
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

fn has_transaction_statements(query: &str) -> bool {
    query
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| ["BEGIN", "COMMIT", "CANCEL"].contains(&word.to_ascii_uppercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::has_transaction_statements;

    #[test]
    fn spots_transaction_statements() {
        assert!(has_transaction_statements("BEGIN; CREATE a; COMMIT;"));
        assert!(has_transaction_statements("begin transaction; select * from a; cancel transaction"));
        assert!(!has_transaction_statements("SELECT * FROM person WHERE name = 'Beginner'"));
    }
}
