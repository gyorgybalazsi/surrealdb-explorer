//! Tests for the MCP server's tools, called directly on a temporary database
//! with the sample data.

mod common;

use rmcp::handler::server::wrapper::Parameters;
use surrealdb_explorer::mcp::{Explorer, NeighbourhoodRequest, Options, QueryRequest};
use surrealdb_explorer::{data, db, graph};

fn query(text: &str) -> Parameters<QueryRequest> {
    Parameters(QueryRequest { query: text.into(), params: None })
}

fn around(id: &str, depth: usize) -> Parameters<NeighbourhoodRequest> {
    Parameters(NeighbourhoodRequest { record_id: id.into(), depth: Some(depth), max_relations: None })
}

#[test]
fn read_only_server_reads_and_refuses_writes() {
    let config = common::temp_database("mcp-read-only");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        data::load_sample_data(&db).await.unwrap();
        let explorer = Explorer::new(db.clone(), Options::default());

        let schema = explorer.describe_database().await.unwrap();
        assert!(schema.contains("connects: person ->knows-> person"));

        let mut params = serde_json::Map::new();
        params.insert("t".into(), "CTO".into());
        let answer = explorer
            .run_query(Parameters(QueryRequest {
                query: "SELECT VALUE name.first FROM person WHERE title = $t".into(),
                params: Some(params),
            }))
            .await
            .unwrap();
        let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(answer[0]["status"], "OK");
        assert_eq!(answer[0]["result"], serde_json::json!(["Alice"]));

        assert!(explorer.run_query(query("DELETE person:bob")).await.is_err());
        let count = explorer.run_query(query("SELECT count() FROM person GROUP ALL")).await.unwrap();
        assert!(count.contains("\"count\":6"), "nothing was deleted: {count}");

        assert_eq!(explorer.check_query(query("DELETE person:bob")).await, "OK");
        assert!(explorer.check_query(query("SELECT * FROM person WHERE")).await.starts_with("Problem:"));
    });
}

#[test]
fn writable_server_runs_writes() {
    let config = common::temp_database("mcp-writes");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        let explorer = Explorer::new(db.clone(), Options { allow_writes: true });
        let answer = explorer.run_query(query("CREATE person:ann SET name = 'Ann'")).await.unwrap();
        assert!(answer.contains("\"status\":\"OK\""));
        assert_eq!(graph::load(&db, 10).await.unwrap().nodes.len(), 1);
    });
}

#[test]
fn neighbourhood_follows_relations_both_ways() {
    let config = common::temp_database("mcp-neighbourhood");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        data::load_sample_data(&db).await.unwrap();

        // alice: tobie and jaime know her; she knows bob and carol.
        let near = graph::neighbourhood::load(&db, "person:alice", 1, 100).await.unwrap();
        let mut ids: Vec<&str> = near.nodes.iter().map(|n| n.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["person:alice", "person:bob", "person:carol", "person:jaime", "person:tobie"]);
        assert_eq!(near.edges.len(), 4);

        // Two steps out also reaches dave, through bob, carol and jaime.
        let far = graph::neighbourhood::load(&db, "person:alice", 2, 100).await.unwrap();
        assert_eq!(far.nodes.len(), 6);
        assert_eq!(far.edges.len(), 7);

        let capped = graph::neighbourhood::load(&db, "person:alice", 2, 2).await.unwrap();
        assert_eq!(capped.edges.len(), 2);
        assert_eq!(capped.truncated, vec!["knows".to_string()]);

        let explorer = Explorer::new(db.clone(), Options::default());
        let json: serde_json::Value =
            serde_json::from_str(&explorer.get_neighbourhood(around("person:dave", 1)).await.unwrap()).unwrap();
        assert_eq!(json["edges"].as_array().unwrap().len(), 3);
        assert!(explorer.get_neighbourhood(around("person:nobody", 1)).await.is_err());
    });
}
