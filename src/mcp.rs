//! The explorer as an MCP server, so an agent such as Claude Code can look
//! at the database and query it.
//!
//! Start it with `surrealdb-explorer mcp` (see `src/main.rs`); it talks MCP
//! over stdin and stdout. The tools:
//!
//! - `describe_database`: tables, fields, an example record per table, and
//!   what each relation table connects.
//! - `run_query`: runs SurrealQL. Refuses queries that change anything unless
//!   the server was started with `--allow-writes`.
//! - `check_query`: tries a query in a transaction that is then cancelled,
//!   so nothing changes, and reports what SurrealDB says is wrong with it.
//! - `get_neighbourhood`: the records and relations around one record, as
//!   the nodes and edges the graph view draws.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;

use crate::Error;
use crate::assistant;
use crate::db::Database;
use crate::graph;
use crate::query::{self, QueryOutcome};

/// Longest tool answer, in characters. Claude Code warns about tool output
/// over about 25,000 tokens, and a huge answer crowds out everything else.
const MAX_ANSWER_CHARS: usize = 80_000;
/// Steps out from the start record `get_neighbourhood` goes at most.
const MAX_DEPTH: usize = 3;
/// Relations `get_neighbourhood` returns by default.
const DEFAULT_MAX_RELATIONS: usize = 200;

const INSTRUCTIONS: &str = "\
Explores an embedded SurrealDB database (SurrealDB 3+ SurrealQL). \
Call describe_database first to learn the tables, their fields and how relation tables connect them. \
Record ids look like table:key (for example person:alice). Relation tables, created with RELATE, \
connect records: a relation record's `in` is its source and `out` its target. \
Use get_neighbourhood to see what is connected to a record, and run_query for anything else. \
Add LIMIT to queries that may return many records.";

/// Options for the server.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Lets `run_query` run statements that change data or definitions.
    pub allow_writes: bool,
}

#[derive(Debug, Clone)]
pub struct Explorer {
    db: Database,
    options: Options,
    tool_router: ToolRouter<Explorer>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryRequest {
    #[schemars(description = "SurrealQL to run; may hold several statements separated by `;`.")]
    pub query: String,
    #[schemars(description = "Optional parameters: an object whose fields the query reads as $name.")]
    #[serde(default)]
    pub params: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NeighbourhoodRequest {
    #[schemars(description = "The record to start from, as table:key, for example person:alice.")]
    pub record_id: String,
    #[schemars(description = "How many relations out to follow, 1 to 3. Default 1.")]
    #[serde(default)]
    pub depth: Option<usize>,
    #[schemars(description = "Most relations to return. Default 200.")]
    #[serde(default)]
    pub max_relations: Option<usize>,
}

#[tool_router]
impl Explorer {
    pub fn new(db: Database, options: Options) -> Explorer {
        Explorer { db, options, tool_router: Self::tool_router() }
    }

    /// Lists the tables with their record counts, fields, an example record,
    /// their definitions, and for relation tables which tables they connect.
    #[tool(annotations(read_only_hint = true))]
    pub async fn describe_database(&self) -> Result<String, String> {
        assistant::describe(&self.db).await.map_err(|e| e.to_string())
    }

    /// Runs SurrealQL and returns each statement's result or error as JSON.
    /// Queries that create, change or delete anything are refused unless the
    /// server allows writes.
    #[tool]
    pub async fn run_query(&self, Parameters(request): Parameters<QueryRequest>) -> Result<String, String> {
        if !self.options.allow_writes && !assistant::is_read_only(&request.query) {
            return Err("This server is read-only, and the query has a statement that changes data or \
                        definitions (or a transaction statement). Rewrite it to only read, or ask the user \
                        to start the server with --allow-writes."
                .into());
        }
        let params = params(request.params);
        tracing::info!(query = %request.query, "MCP run_query");
        let outcome = query::run_with(&self.db, request.query, &params).await;
        Ok(cut(outcome_json(&outcome)))
    }

    /// Checks SurrealQL without changing anything: runs it inside a
    /// transaction that is then cancelled. Returns "OK" or the first problem
    /// SurrealDB finds. Queries with their own BEGIN/COMMIT can't be checked.
    #[tool(annotations(read_only_hint = true))]
    pub async fn check_query(&self, Parameters(request): Parameters<QueryRequest>) -> String {
        let params = params(request.params);
        match assistant::check_with(&self.db, &request.query, &params).await {
            Some(problem) => format!("Problem: {problem}"),
            None => "OK".into(),
        }
    }

    /// Returns the records and relations around one record, following
    /// relations both ways, as JSON nodes and edges: `{ nodes: [{ id, table,
    /// data }], edges: [{ id, from, to, label, data }], truncated: [table] }`.
    /// This is what the explorer's graph view draws.
    #[tool(annotations(read_only_hint = true))]
    pub async fn get_neighbourhood(
        &self,
        Parameters(request): Parameters<NeighbourhoodRequest>,
    ) -> Result<String, String> {
        let depth = request.depth.unwrap_or(1).clamp(1, MAX_DEPTH);
        let max = request.max_relations.unwrap_or(DEFAULT_MAX_RELATIONS).max(1);
        let data = graph::neighbourhood::load(&self.db, request.record_id.trim(), depth, max)
            .await
            .map_err(|e| e.to_string())?;
        if data.nodes.is_empty() {
            return Err(format!("There is no record {}.", request.record_id));
        }
        Ok(cut(data.to_json().to_string()))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Explorer {
    fn get_info(&self) -> ServerConfig {
        let mut instructions = INSTRUCTIONS.to_string();
        if !self.options.allow_writes {
            instructions.push_str(" The server is read-only: run_query refuses statements that change anything.");
        }
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("surrealdb-explorer", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }
}

/// Serves MCP over stdin and stdout until the client disconnects.
pub async fn serve_stdio(db: Database, options: Options) -> Result<(), Error> {
    tracing::info!(allow_writes = options.allow_writes, "MCP server listening on stdio");
    let service = Explorer::new(db, options).serve(rmcp::transport::stdio()).await?;
    let reason = service.waiting().await?;
    tracing::info!(?reason, "MCP client disconnected");
    Ok(())
}

fn params(params: Option<serde_json::Map<String, serde_json::Value>>) -> serde_json::Value {
    serde_json::Value::Object(params.unwrap_or_default())
}

/// `[{ "status": "OK", "time": "1.2ms", "result": … }, { "status": "ERR", "error": "…" }]`
pub fn outcome_json(outcome: &QueryOutcome) -> String {
    let statements = match &outcome.statements {
        Err(e) => return serde_json::json!([{ "status": "ERR", "error": e }]).to_string(),
        Ok(statements) => statements,
    };
    let items: Vec<serde_json::Value> = statements
        .iter()
        .map(|s| {
            let time = s.time.map(|t| format!("{t:?}"));
            match &s.result {
                Ok(result) => serde_json::json!({ "status": "OK", "time": time, "result": result }),
                Err(e) => serde_json::json!({ "status": "ERR", "time": time, "error": e }),
            }
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

/// Cuts an answer to `MAX_ANSWER_CHARS`, saying so at the end.
fn cut(text: String) -> String {
    if text.chars().count() <= MAX_ANSWER_CHARS {
        return text;
    }
    let kept: String = text.chars().take(MAX_ANSWER_CHARS).collect();
    format!(
        "{kept}\n… (cut at {MAX_ANSWER_CHARS} characters: the answer was longer. \
         Use LIMIT, select fewer fields, or ask for fewer relations.)"
    )
}
