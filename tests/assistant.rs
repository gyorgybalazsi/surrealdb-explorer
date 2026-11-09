//! Tests for plain-English questions that don't need the Claude API: the
//! schema description Claude gets, and the check of suggested queries.
//! `asks_claude` calls the real API and is skipped unless asked for:
//! `cargo test --test assistant -- --ignored`.

mod common;

use surrealdb_explorer::assistant::{self, Assistant, Model, Request};
use surrealdb_explorer::config::{AssistantMode, Config};
use surrealdb_explorer::{data, db, query};

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
        // When one statement fails, the others say "failed transaction";
        // the check reports the statement that actually failed.
        let cast = assistant::check(&db, "LET $a = 1; LET $b = <array>'[1, 2]'; RETURN $b;").await;
        assert!(cast.as_deref().is_some_and(|p| p.contains("Could not cast")), "{cast:?}");

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
/// of a cent. Set SURREALDB_EXPLORER_KEY_REF to the op:// reference.
#[test]
#[ignore]
fn asks_claude() {
    let reference = std::env::var("SURREALDB_EXPLORER_KEY_REF").expect("set SURREALDB_EXPLORER_KEY_REF");
    let key = assistant::key::fetch(&reference).unwrap();
    let config = common::temp_database("assistant-live");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        data::load_sample_data(&db).await.unwrap();
        let assistant = Assistant::new(key);

        for question in ["who does alice know?", "how many people are there?", "who knows someone who knows bob?"] {
            let s = assistant.ask(&db, &Request::new(question), AssistantMode::Auto).await.unwrap();
            println!(
                "Q: {question}\n  {} {:?} confident={} problem={:?}\n  query:   {}\n  context: {:?}\n  {}\n",
                s.model.name(),
                s.notes,
                s.confident,
                s.problem,
                s.query.replace('\n', "\n  "),
                s.context_query,
                s.explanation
            );
            assert!(s.problem.is_none(), "{:?}", s.problem);
        }

        // The answer comes with the relations that explain it.
        let s = assistant.ask(&db, &Request::new("who knows someone who knows bob?"), AssistantMode::Auto).await.unwrap();
        let context = s.context_query.expect("a context query");
        let ids = query::record_ids(&query::run(&db, context).await);
        println!("context ids: {ids:?}");
        assert!(ids.iter().any(|id| id.starts_with("knows:")), "the context returns knows relations");

        let first = assistant.ask(&db, &Request::new("who does alice know?"), AssistantMode::Haiku).await.unwrap();
        assert_eq!(first.model, Model::Haiku);
        let again = assistant.retry_with_opus(&db, &first).await.unwrap();
        assert_eq!(again.model, Model::Opus);
        println!("Opus retry: {}\n  {}", again.query, again.explanation);
    });
}

#[test]
fn the_chat_history_survives_a_restart() {
    use surrealdb_explorer::assistant::Suggestion;
    use surrealdb_explorer::assistant::history::{self, Exchange, MAX_KEPT};

    let dir = common::temp_dir("chat-history");
    let path = dir.join(history::HISTORY_FILE);
    assert!(history::load(&path).is_empty(), "no file yet means no history");

    let answered = Exchange {
        question: "who does alice know?".into(),
        attachments: vec![],
        answer: Ok(Suggestion {
            question: "who does alice know?".into(),
            query: "SELECT ->knows->person.* FROM person:alice;".into(),
            context_query: Some("SELECT * FROM knows WHERE in = person:alice;".into()),
            explanation: "People Alice knows.".into(),
            model: Model::Haiku,
            confident: true,
            problem: None,
            notes: vec![],
            editor_query: None,
            attachments: vec![],
        }),
    };
    let failed = Exchange { question: "and bob?".into(), attachments: vec!["names.csv".into()], answer: Err("no network".into()) };
    history::save(&path, &[answered, failed]).unwrap();

    let loaded = history::load(&path);
    assert_eq!(loaded.len(), 2);
    let first = loaded[0].answer.as_ref().unwrap();
    assert_eq!(first.model, Model::Haiku);
    assert_eq!(first.context_query.as_deref(), Some("SELECT * FROM knows WHERE in = person:alice;"));
    assert_eq!(loaded[1].answer.as_ref().unwrap_err(), "no network");
    assert_eq!(loaded[1].attachments, ["names.csv"]);

    // Only the newest exchanges are kept.
    let many: Vec<Exchange> = (0..MAX_KEPT + 5)
        .map(|i| Exchange { question: format!("q{i}"), attachments: vec![], answer: Err("x".into()) })
        .collect();
    history::save(&path, &many).unwrap();
    let loaded = history::load(&path);
    assert_eq!(loaded.len(), MAX_KEPT);
    assert_eq!(loaded[0].question, "q5");

    // A damaged file is moved aside, not lost.
    std::fs::write(&path, "{ not json").unwrap();
    assert!(history::load(&path).is_empty());
    assert!(dir.join("chat-history.json.bad").exists());
}

/// Attached files are query parameters, so a query can load a whole file,
/// like the example in Claude's instructions.
#[test]
fn queries_load_attached_files_through_parameters() {
    use surrealdb_explorer::assistant::attachment::{self, Attachment};

    let config = common::temp_database("assistant-files");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        let mut attached = Vec::new();
        let problems = attachment::add(
            &mut attached,
            vec![
                Attachment::from_bytes("movies.csv", b"movieId,title,year\n1,Toy Story,1995\n2,Jumanji,\n".to_vec()),
                Attachment::from_bytes("people.csv", b"tmdbId,name\n31,Tom Hanks\n".to_vec()),
                Attachment::from_bytes("acted_in.csv", b"personTmdbId,movieId,role\n31,1,Woody (voice)\n".to_vec()),
            ],
        );
        assert!(problems.is_empty(), "{problems:?}");
        let params = attachment::params(&attached);

        let load = "\
            FOR $row IN $movies { CREATE type::record('movie', <int>$row.movieId) CONTENT { title: $row.title, year: <option<int>>$row.year } };\n\
            FOR $row IN $people { CREATE type::record('person', <int>$row.tmdbId) CONTENT { name: $row.name } };\n\
            FOR $row IN $acted_in { RELATE (type::record('person', <int>$row.personTmdbId))->acted_in->(type::record('movie', <int>$row.movieId)) CONTENT { role: $row.role } };";

        // The check sees the files too, and changes nothing.
        assert_eq!(assistant::check_with(&db, load, &params).await, None);

        let outcome = query::run_with(&db, load.into(), &params).await;
        let statements = outcome.statements.unwrap();
        assert!(statements.iter().all(|s| s.result.is_ok()), "{statements:?}");

        let roles = query::run(&db, "SELECT VALUE ->acted_in->movie.title FROM person:31;".into()).await;
        let roles = roles.statements.unwrap()[0].result.clone().unwrap();
        assert_eq!(roles, serde_json::json!([["Toy Story"]]));
        let year = query::run(&db, "SELECT VALUE year FROM movie ORDER BY id;".into()).await;
        assert_eq!(year.statements.unwrap()[0].result.clone().unwrap(), serde_json::json!([1995, null]), "an empty cell isn't stored");
    });
}
