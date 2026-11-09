//! The app's MCP server: tools that let Claude Code (or any MCP client) read
//! the database and draw in the app's window.
//!
//! It listens on `127.0.0.1` only. Queries that would change data run only
//! after the user allows them in the window.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};

use crate::db::Database;
use crate::graph::{self, ResultGraph};
use crate::query::{self, QueryOutcome, is_read_only};
use crate::schema::describe;

/// Results longer than this are cut before they are sent back, so a huge
/// result can't flood the conversation.
const MAX_RESULT_CHARS: usize = 100_000;
/// How long a write waits for the user to allow it in the window.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

/// What the MCP server asks the window to do.
#[derive(Debug)]
pub enum WindowCommand {
    /// Draw a query's result, as if it had been run in the query panel.
    Show { query: String, outcome: QueryOutcome, graph: ResultGraph },
    /// Go back to the whole-database view.
    Overview,
    /// Ask the user to allow a query that changes data.
    ApproveWrite { query: String, reply: oneshot::Sender<bool> },
    /// Data was changed through a tool; the window should show the current state.
    DataChanged,
}

/// What the window tells the MCP server: kept up to date by the window.
#[derive(Debug, Clone, Default)]
pub struct WindowState {
    /// The selected record or relation, as JSON (id, fields, connections).
    pub selection: Option<serde_json::Value>,
    /// Why the MCP server isn't running, if it stopped.
    pub server_error: Option<String>,
}

/// Everything the tools share with the window.
#[derive(Clone)]
pub struct Shared {
    /// The open database; replaced when the user switches in Settings.
    pub db: Arc<Mutex<Option<Database>>>,
    /// Most records drawn for a result, and whether to connect them.
    pub limits: Arc<Mutex<(usize, bool)>>,
    pub window: Arc<Mutex<WindowState>>,
    pub to_window: mpsc::UnboundedSender<WindowCommand>,
}

impl Shared {
    pub fn new(to_window: mpsc::UnboundedSender<WindowCommand>) -> Shared {
        Shared {
            db: Arc::new(Mutex::new(None)),
            limits: Arc::new(Mutex::new((2000, false))),
            window: Arc::new(Mutex::new(WindowState::default())),
            to_window,
        }
    }

    fn database(&self) -> Result<Database, McpError> {
        self.db
            .lock()
            .ok()
            .and_then(|db| db.clone())
            .ok_or_else(|| McpError::internal_error("the app has no database open", None))
    }
}

/// Serves the tools at `http://127.0.0.1:<port>/mcp` until the process ends.
pub async fn serve(shared: Shared, port: u16) -> Result<(), crate::Error> {
    let service: StreamableHttpService<Explorer, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(Explorer::new(shared.clone())),
        Default::default(),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("could not listen on 127.0.0.1:{port}: {e}"))?;
    tracing::info!(port, "MCP server listening on http://127.0.0.1:{port}/mcp");
    axum::serve(listener, router).await?;
    Ok(())
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryRequest {
    #[schemars(description = "SurrealQL to run. Several statements are fine, separated by semicolons.")]
    pub query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ShowRequest {
    #[schemars(description = "Read-only SurrealQL whose result is drawn. Records it returns are \
        drawn; relations bring the records at both ends. To get path rows in the window's Table \
        view (like Neo4j), return one array per path: [record, relation, record, ...].")]
    pub query: String,
}

#[derive(Clone)]
pub struct Explorer {
    shared: Shared,
    tool_router: ToolRouter<Self>,
}

impl Explorer {
    pub fn new(shared: Shared) -> Explorer {
        Explorer { shared, tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl Explorer {
    #[tool(description = "Describe the open database: its tables, their fields, an example record \
        each, and which tables each relation table connects.")]
    async fn get_schema(&self) -> Result<CallToolResult, McpError> {
        let db = self.shared.database()?;
        let schema = describe(&db).await.map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(schema)]))
    }

    #[tool(description = "Run SurrealQL on the open database and return each statement's result \
        as JSON. Queries that change data (CREATE, UPDATE, DELETE, RELATE, DEFINE, ...) run only \
        after the user allows them in the app window. Don't use USE; the user picks the database \
        in the app.")]
    async fn query(&self, Parameters(QueryRequest { query }): Parameters<QueryRequest>) -> Result<CallToolResult, McpError> {
        refuse_use(&query)?;
        let db = self.shared.database()?;
        let writes = !is_read_only(&query);
        if writes && !self.approved(&query).await {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "The user didn't allow this query to change data, so it didn't run.",
            )]));
        }
        let outcome = query::run(&db, query).await;
        if writes {
            let _ = self.shared.to_window.send(WindowCommand::DataChanged);
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(outcome_text(&outcome))]))
    }

    #[tool(description = "Draw a read-only query's result in the app window, replacing the graph. \
        The result also appears in the window's query panel; a list of paths \
        ([record, relation, record, ...]) is shown as path rows. Returns what was drawn.")]
    async fn show(&self, Parameters(ShowRequest { query }): Parameters<ShowRequest>) -> Result<CallToolResult, McpError> {
        refuse_use(&query)?;
        if !is_read_only(&query) {
            return Err(McpError::invalid_params("show only runs read-only queries; use the query tool to change data", None));
        }
        let db = self.shared.database()?;
        let outcome = query::run(&db, query.clone()).await;
        if let Err(e) = &outcome.statements {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!("The query failed: {e}"))]));
        }
        let (limit, connect) = *self.shared.limits.lock().map_err(|_| internal("limits lock"))?;
        let answer = query::record_ids(&outcome);
        let result = graph::load_result(&db, &answer, &[], limit, connect).await.map_err(internal)?;

        let summary = format!(
            "Drew {} records and {} relations in the window{}.\n\n{}",
            result.data.nodes.len(),
            result.data.edges.len(),
            if result.left_out > 0 {
                format!("; {} more weren't drawn because of the {limit}-record limit", result.left_out)
            } else {
                String::new()
            },
            outcome_text(&outcome)
        );
        self.shared
            .to_window
            .send(WindowCommand::Show { query, outcome, graph: result })
            .map_err(|_| internal("the window is closed"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(summary)]))
    }

    #[tool(description = "What the user has selected in the app window: the record or relation's \
        id, its fields and its connections. Use it when the user refers to \"this\" or \"the \
        selected\" record.")]
    async fn get_selection(&self) -> Result<CallToolResult, McpError> {
        let selection = self.shared.window.lock().map_err(|_| internal("window lock"))?.selection.clone();
        let text = match selection {
            Some(s) => serde_json::to_string_pretty(&s).unwrap_or_default(),
            None => "Nothing is selected in the app window.".to_string(),
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Go back to the overview of the whole database in the app window.")]
    async fn overview(&self) -> Result<CallToolResult, McpError> {
        self.shared.to_window.send(WindowCommand::Overview).map_err(|_| internal("the window is closed"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text("The window shows the whole database again.")]))
    }
}

impl Explorer {
    /// Asks the user in the window; `false` if they decline or don't answer in time.
    async fn approved(&self, query: &str) -> bool {
        let (reply, answer) = oneshot::channel();
        if self.shared.to_window.send(WindowCommand::ApproveWrite { query: query.to_string(), reply }).is_err() {
            return false;
        }
        let allowed = matches!(tokio::time::timeout(APPROVAL_TIMEOUT, answer).await, Ok(Ok(true)));
        tracing::info!(allowed, "write requested over MCP");
        allowed
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Explorer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "SurrealDB Explorer: a graph view of a SurrealDB database, open in a window on the \
             user's screen. Use get_schema first, query to explore, and show to draw results for \
             the user. Prefer starting queries from the specific records a question names and \
             following relations with arrows (->rel->table, <-rel<-table).",
        )
    }
}

/// `USE` would switch the database under the app's feet.
fn refuse_use(query: &str) -> Result<(), McpError> {
    let uses = query
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| word.eq_ignore_ascii_case("USE"));
    if uses {
        return Err(McpError::invalid_params(
            "USE isn't allowed here; the user picks the namespace and database in the app's Settings",
            None,
        ));
    }
    Ok(())
}

/// Each statement's result or error, as text for the conversation.
fn outcome_text(outcome: &QueryOutcome) -> String {
    let statements = match &outcome.statements {
        Err(e) => return format!("The query failed: {e}"),
        Ok(statements) => statements,
    };
    let mut text = String::new();
    for (i, s) in statements.iter().enumerate() {
        match &s.result {
            Ok(v) => text.push_str(&format!(
                "Statement {} (ok):\n{}\n\n",
                i + 1,
                serde_json::to_string_pretty(v).unwrap_or_default()
            )),
            Err(e) => text.push_str(&format!("Statement {} (error): {e}\n\n", i + 1)),
        }
    }
    if text.chars().count() > MAX_RESULT_CHARS {
        let kept: String = text.chars().take(MAX_RESULT_CHARS).collect();
        return format!("{kept}\n… (cut; ask for fewer records, e.g. with LIMIT)");
    }
    text
}

fn internal(e: impl std::fmt::Display) -> McpError {
    McpError::internal_error(e.to_string(), None)
}
