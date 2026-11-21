//! The MCP server, spoken to over HTTP the way Claude Code does. A stand-in
//! for the window answers write approvals and records what it was told.

use crate::common;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use surrealdb_explorer::mcp::{self, Shared, WindowCommand};
use surrealdb_explorer::{data, db};
use tokio::sync::mpsc;

/// One MCP session: initialize once, then call tools.
struct Session {
    http: reqwest::Client,
    url: String,
    id: String,
    next: u64,
}

impl Session {
    async fn open(url: String) -> Session {
        let http = reqwest::Client::new();
        let response = http
            .post(&url)
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}}}))
            .send()
            .await
            .unwrap();
        let id = response.headers()["mcp-session-id"].to_str().unwrap().to_string();
        response.text().await.unwrap();
        let session = Session { http, url, id, next: 1 };
        session.post(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
        session
    }

    async fn post(&self, body: Value) -> Option<Value> {
        let text = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", &self.id)
            .header("MCP-Protocol-Version", "2025-06-18")
            .json(&body)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        // Replies come as server-sent events: take the JSON `data:` line.
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .find(|v| v.get("result").is_some() || v.get("error").is_some())
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        self.post(json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params}))
            .await
            .expect("a reply")
    }

    /// The tool's text, and whether it reported an error.
    async fn call(&mut self, tool: &str, arguments: Value) -> (String, bool) {
        let reply = self.request("tools/call", json!({"name": tool, "arguments": arguments})).await;
        if let Some(error) = reply.get("error") {
            return (error["message"].as_str().unwrap_or_default().to_string(), true);
        }
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default().to_string();
        (text, result["isError"].as_bool().unwrap_or(false))
    }
}

#[test]
fn claude_code_can_read_draw_and_ask_to_write() {
    let config = common::temp_database("mcp");
    common::run(async move {
        let database = db::connect(&config).await.unwrap();
        data::load_sample_data(&database).await.unwrap();

        let (to_window, mut window) = mpsc::unbounded_channel();
        let shared = Shared::new(to_window);
        *shared.db.lock().unwrap() = Some(database.clone());

        // The stand-in window: allows writes only while `allow` is set.
        let allow = Arc::new(Mutex::new(false));
        let shown = Arc::new(Mutex::new(Vec::<(String, usize)>::new()));
        {
            let (allow, shown) = (allow.clone(), shown.clone());
            tokio::spawn(async move {
                while let Some(command) = window.recv().await {
                    match command {
                        WindowCommand::ApproveWrite { reply, .. } => {
                            let _ = reply.send(*allow.lock().unwrap());
                        }
                        WindowCommand::Show { query, graph, .. } => {
                            shown.lock().unwrap().push((query, graph.data.nodes.len()));
                        }
                        // The window answers for person:alice only, as if only she were drawn.
                        WindowCommand::Select { id, reply } | WindowCommand::Scroll { to: id, reply } => {
                            let _ = reply.send(if id == "person:alice" || id.is_empty() || id == "bottom" {
                                Ok(json!({ "id": id }))
                            } else {
                                Err(format!("no record {id} is drawn in the window"))
                            });
                        }
                        _ => {}
                    }
                }
            });
        }

        let port = 18_768;
        tokio::spawn(mcp::serve(shared.clone(), port));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let mut session = Session::open(format!("http://127.0.0.1:{port}/mcp")).await;

        let tools = session.request("tools/list", json!({})).await;
        let mut names: Vec<&str> =
            tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["get_schema", "get_selection", "overview", "query", "scroll_graph", "select_record", "show"]);

        let (schema, _) = session.call("get_schema", json!({})).await;
        assert!(schema.contains("connects: person ->knows-> person"), "{schema}");

        let (people, failed) = session.call("query", json!({"query": "SELECT VALUE id FROM person;"})).await;
        assert!(!failed && people.contains("person:alice"), "{people}");

        // Writes need the user's OK.
        let (declined, failed) = session.call("query", json!({"query": "DELETE person:bob;"})).await;
        assert!(failed, "{declined}");
        let (count, _) = session.call("query", json!({"query": "SELECT VALUE count() FROM person GROUP ALL;"})).await;
        assert!(count.contains('6'), "bob is still there: {count}");
        *allow.lock().unwrap() = true;
        let (_, failed) = session.call("query", json!({"query": "DELETE person:bob;"})).await;
        assert!(!failed);
        let (count, _) = session.call("query", json!({"query": "SELECT VALUE count() FROM person GROUP ALL;"})).await;
        assert!(count.contains('5'), "bob is gone: {count}");

        // USE would switch the app's database.
        let (_, failed) = session.call("query", json!({"query": "USE DB other; SELECT * FROM person;"})).await;
        assert!(failed);

        // show draws in the window and won't write.
        let (drawn, failed) = session
            .call("show", json!({"query": "SELECT * FROM person:alice->knows->person;"}))
            .await;
        assert!(!failed && drawn.starts_with("Drew"), "{drawn}");
        assert_eq!(shown.lock().unwrap().len(), 1);
        let (_, failed) = session.call("show", json!({"query": "DELETE person;"})).await;
        assert!(failed);

        let (selection, _) = session.call("get_selection", json!({})).await;
        assert!(selection.contains("Nothing is selected"));
        shared.window.lock().unwrap().selection = Some(json!({"id": "person:alice"}));
        let (selection, _) = session.call("get_selection", json!({})).await;
        assert!(selection.contains("person:alice"));

        // select_record and scroll_graph pass the window's answer on.
        let (picked, failed) = session.call("select_record", json!({"id": "person:alice"})).await;
        assert!(!failed && picked.contains("person:alice"), "{picked}");
        let (missing, failed) = session.call("select_record", json!({"id": "person:nobody"})).await;
        assert!(failed && missing.contains("no record person:nobody"), "{missing}");
        let (_, failed) = session.call("scroll_graph", json!({"to": "bottom"})).await;
        assert!(!failed);
    });
}

/// With a viewer user set, Claude Code's reads run as that user, and a write
/// the user allowed runs as the main user.
#[test]
fn claude_code_reads_as_the_viewer() {
    let mut config = common::temp_database("mcp_viewer");
    common::run(async move {
        let database = db::connect(&config).await.unwrap();
        database
            .query("DEFINE USER viewer ON DATABASE PASSWORD 'viewer-pass' ROLES VIEWER;")
            .await
            .unwrap()
            .check()
            .unwrap();
        data::load_sample_data(&database).await.unwrap();

        let (to_window, mut window) = mpsc::unbounded_channel();
        let shared = Shared::new(to_window);
        *shared.db.lock().unwrap() = Some(database.clone());
        // The stand-in window allows every write.
        tokio::spawn(async move {
            while let Some(command) = window.recv().await {
                if let WindowCommand::ApproveWrite { reply, .. } = command {
                    let _ = reply.send(true);
                }
            }
        });

        let port = 18_769;
        tokio::spawn(mcp::serve(shared.clone(), port));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let mut session = Session::open(format!("http://127.0.0.1:{port}/mcp")).await;

        // A viewer that can't sign in: reads don't run at all.
        config.viewer_username = "viewer".into();
        config.viewer_password = "wrong".into();
        *shared.queries.lock().unwrap() = db::Queries::connect(&config).await;
        let (refused, failed) = session.call("query", json!({"query": "SELECT VALUE id FROM person;"})).await;
        assert!(failed && refused.contains("viewer"), "{refused}");

        config.viewer_password = "viewer-pass".into();
        *shared.queries.lock().unwrap() = db::Queries::connect(&config).await;
        let (people, failed) = session.call("query", json!({"query": "SELECT VALUE id FROM person;"})).await;
        assert!(!failed && people.contains("person:alice"), "{people}");

        // An allowed write runs as the main user.
        let (_, failed) = session.call("query", json!({"query": "DELETE person:bob;"})).await;
        assert!(!failed);
        let (count, _) = session.call("query", json!({"query": "SELECT VALUE count() FROM person GROUP ALL;"})).await;
        assert!(count.contains('5'), "bob is gone: {count}");
    });
}
