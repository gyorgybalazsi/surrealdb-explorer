//! Tests for plain-English questions that don't need the Claude API: the
//! schema description Claude gets, and the check of suggested queries.
//! `asks_claude` calls the real API and is skipped unless asked for:
//! `cargo test --test assistant -- --ignored`.

mod common;

use surrealdb_explore::assistant::{self, Assistant, Model};
use surrealdb_explore::config::{AssistantMode, Config};
use surrealdb_explore::{data, db};

#[test]
fn describes_tables_fields_and_what_relations_connect() {
    let config = common::temp_database("assistant-schema");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        assert_eq!(assistant::describe(&db).await.unwrap(), "The database has no tables yet.");

        data::load_sample_data(&db).await.unwrap();
        let schema = assistant::describe(&db).await.unwrap();
        assert!(schema.contains("- person: 6 records (drawn as nodes)"), "{schema}");
        assert!(schema.contains("- knows: relation table (7 records, drawn as edges)"), "{schema}");
        assert!(schema.contains("connects: person ->knows-> person"), "{schema}");
        assert!(schema.contains("fields: ") && schema.contains("name"), "{schema}");
        assert!(schema.contains("example: {"), "{schema}");
    });
}

#[test]
fn the_check_finds_problems_and_changes_nothing() {
    let config = common::temp_database("assistant-check");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        data::load_sample_data(&db).await.unwrap();

        assert_eq!(assistant::check(&db, "SELECT ->knows->person.name FROM person:alice;").await, None);
        assert_eq!(assistant::check(&db, "SELECT * FROM person -- all of them").await, None);

        let missing = assistant::check(&db, "SELECT * FROM nosuchtable;").await;
        assert!(missing.is_some_and(|p| p.contains("nosuchtable")));
        let syntax = assistant::check(&db, "SELEC * FROM person;").await;
        assert!(syntax.is_some_and(|p| p.contains("Parse error")));
        assert!(assistant::check(&db, "  ").await.is_some());

        // A checked DELETE must not delete.
        assert_eq!(assistant::check(&db, "DELETE person;").await, None);
        let left: Vec<surrealdb::types::Value> =
            db.query("SELECT * FROM person").await.unwrap().take(0).unwrap();
        assert_eq!(left.len(), 6);
    });
}

#[test]
fn the_api_key_setting_must_be_a_1password_reference() {
    let mut config = Config::default();
    config.assistant.api_key = "op://Personal/key/credential".into();
    assert!(config.validate().is_ok());
    config.assistant.api_key = "sk-ant-api03-secret".into();
    assert!(config.validate().is_err(), "a pasted key must not be saved in config.toml");
}

/// Calls the real Claude API with the key from 1Password. Costs a fraction
/// of a cent. Set SURREALDB_EXPLORE_KEY_REF to the op:// reference.
#[test]
#[ignore]
fn asks_claude() {
    let reference = std::env::var("SURREALDB_EXPLORE_KEY_REF").expect("set SURREALDB_EXPLORE_KEY_REF");
    let key = assistant::key::fetch(&reference).unwrap();
    let config = common::temp_database("assistant-live");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        data::load_sample_data(&db).await.unwrap();
        let assistant = Assistant::new(key);

        for question in ["who does alice know?", "how many people are there?", "who knows someone who knows bob?"] {
            let s = assistant.ask(&db, question, AssistantMode::Auto).await.unwrap();
            println!(
                "Q: {question}\n  {} {:?} confident={} problem={:?}\n  {}\n  {}\n",
                s.model.name(),
                s.notes,
                s.confident,
                s.problem,
                s.query.replace('\n', "\n  "),
                s.explanation
            );
            assert!(s.problem.is_none(), "{:?}", s.problem);
        }

        let first = assistant.ask(&db, "who does alice know?", AssistantMode::Haiku).await.unwrap();
        assert_eq!(first.model, Model::Haiku);
        let again = assistant.retry_with_opus(&db, &first).await.unwrap();
        assert_eq!(again.model, Model::Opus);
        println!("Opus retry: {}\n  {}", again.query, again.explanation);
    });
}
