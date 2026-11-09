//! Tests for the configuration file and for running queries.
//! They use a temporary database, so they never touch `./data`.

mod common;

use common::temp_dir;
use surrealdb_explorer::config::{Config, DatabaseConfig};
use surrealdb_explorer::{db, graph, query};

#[test]
fn config_is_created_with_defaults_and_round_trips() {
    let dir = temp_dir("config");
    let path = dir.join("config.toml");

    let (config, created) = Config::load_or_create_at(&path).unwrap();
    assert!(created);
    assert_eq!(config, Config::default());

    let mut changed = config.clone();
    changed.graph.max_records_per_table = 42;
    changed.logging.level = "debug".into();
    changed.save_to(&path).unwrap();

    let (loaded, created) = Config::load_or_create_at(&path).unwrap();
    assert!(!created);
    assert_eq!(loaded, changed);
}

#[test]
fn missing_fields_take_defaults_and_bad_values_are_refused() {
    let dir = temp_dir("partial");
    let path = dir.join("config.toml");

    std::fs::write(&path, "[graph]\nmax_records_per_table = 7\n").unwrap();
    let (config, _) = Config::load_or_create_at(&path).unwrap();
    assert_eq!(config.graph.max_records_per_table, 7);
    assert_eq!(config.database, DatabaseConfig::default());

    // Settings added later take their default, and sections that are no
    // longer used (like an old [assistant]) are ignored.
    std::fs::write(&path, "[assistant]\nmode = \"haiku\"\n[mcp]\nport = 9000\n").unwrap();
    let (config, _) = Config::load_or_create_at(&path).unwrap();
    assert_eq!(config.mcp.port, 9000);
    assert!(config.mcp.enabled);
    assert!(!config.graph.connect_results);

    std::fs::write(&path, "[logging]\nlevel = \"loud\"\n").unwrap();
    assert!(Config::load_or_create_at(&path).is_err());
}

#[test]
fn queries_report_each_statement_and_the_graph_sees_the_changes() {
    let config = common::temp_database("query");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();

        let outcome = query::run(
            &db,
            "CREATE person:ann SET name = 'Ann'; \
             CREATE person:ben SET name = 'Ben'; \
             RELATE person:ann->knows->person:ben; \
             SELECT * FROM nonsense WHERE ;"
                .into(),
        )
        .await;
        // A syntax error fails the whole query before anything runs.
        assert!(outcome.statements.is_err());

        let outcome = query::run(
            &db,
            "CREATE person:ann SET name = 'Ann'; \
             CREATE person:ben SET name = 'Ben'; \
             RELATE person:ann->knows->person:ben; \
             CREATE person:ann SET name = 'Again';"
                .into(),
        )
        .await;
        let statements = outcome.statements.as_ref().unwrap();
        assert_eq!(statements.len(), 4);
        assert!(statements[..3].iter().all(|s| s.result.is_ok()));
        assert!(statements[3].result.is_err(), "creating an existing record fails");

        let ids = query::record_ids(&outcome);
        assert!(ids.contains(&"person:ann".to_string()));
        assert!(ids.contains(&"person:ben".to_string()));

        let data = graph::load(&db, 1000).await.unwrap();
        assert_eq!(data.nodes.len(), 2);
        assert_eq!(data.edges.len(), 1);

        let limited = graph::load(&db, 1).await.unwrap();
        assert_eq!(limited.nodes.len(), 1);
        assert_eq!(limited.truncated, vec!["person".to_string()]);
    });
}
