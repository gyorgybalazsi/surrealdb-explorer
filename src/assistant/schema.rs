//! A short description of the database for Claude: tables, their fields,
//! an example record, and what each relation table connects.

use std::collections::BTreeSet;

use serde_json::Value as Json;
use surrealdb::types::Value;

use crate::db::Database;
use crate::graph::tables;

/// Example records are cut to this many characters, so a big record can't
/// blow up the prompt.
const MAX_EXAMPLE_CHARS: usize = 400;
/// Records read per table to collect field names.
const SAMPLE_SIZE: i64 = 5;

pub async fn describe(db: &Database) -> surrealdb::Result<String> {
    let tables = tables::list(db).await?;
    if tables.is_empty() {
        return Ok("The database has no tables yet.".into());
    }

    let mut out = String::new();
    for table in &tables {
        let count = first_number(
            db.query("SELECT count() FROM type::table($table) GROUP ALL")
                .bind(("table", table.name.clone()))
                .await?
                .take::<Value>(0)?,
            "count",
        );
        let sample = json(
            db.query("SELECT * FROM type::table($table) LIMIT $limit")
                .bind(("table", table.name.clone()))
                .bind(("limit", SAMPLE_SIZE))
                .await?
                .take::<Value>(0)?,
        );
        let rows = sample.as_array().cloned().unwrap_or_default();
        let fields: BTreeSet<&str> = rows
            .iter()
            .filter_map(Json::as_object)
            .flat_map(|o| o.keys().map(String::as_str))
            .collect();
        let is_relation =
            table.is_relation || (fields.contains("in") && fields.contains("out"));

        let records = count.map(|n| format!("{n} records")).unwrap_or_else(|| "records".into());
        if is_relation {
            out.push_str(&format!("- {}: relation table ({records}, drawn as edges)\n", table.name));
            let pairs = json(
                db.query(
                    "SELECT record::tb(in) AS from, record::tb(out) AS to \
                     FROM type::table($table) GROUP BY from, to",
                )
                .bind(("table", table.name.clone()))
                .await?
                .take::<Value>(0)?,
            );
            for pair in pairs.as_array().into_iter().flatten() {
                let end = |k: &str| pair.get(k).and_then(Json::as_str).unwrap_or("?").to_string();
                out.push_str(&format!("    connects: {} ->{}-> {}\n", end("from"), table.name, end("to")));
            }
        } else {
            out.push_str(&format!("- {}: {records} (drawn as nodes)\n", table.name));
        }
        if !fields.is_empty() {
            let fields: Vec<&str> = fields.into_iter().collect();
            out.push_str(&format!("    fields: {}\n", fields.join(", ")));
        }
        if let Some(example) = rows.first() {
            out.push_str(&format!("    example: {}\n", cut(&example.to_string())));
        }
        if !table.definition.is_empty() {
            out.push_str(&format!("    definition: {}\n", table.definition));
        }
    }
    Ok(out)
}

fn json(value: Value) -> Json {
    value.into_json_value()
}

/// `[{ "count": 6 }]` → `Some(6)`.
fn first_number(value: Value, field: &str) -> Option<i64> {
    json(value).as_array()?.first()?.get(field)?.as_i64()
}

fn cut(text: &str) -> String {
    if text.chars().count() <= MAX_EXAMPLE_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(MAX_EXAMPLE_CHARS).collect();
    format!("{kept}… (cut)")
}
