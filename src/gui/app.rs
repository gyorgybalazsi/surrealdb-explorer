//! Window state and message handling. The views are in `sidebar.rs`,
//! `settings.rs` and `query_panel.rs`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use iced::widget::{button, canvas, column, container, pin, row, rule, stack, text, text_editor};
use iced::{Color, Element, Fill, Font, Point, Subscription, Task, Theme, Vector, window};
use surrealdb_explorer::assistant::history::{self, Exchange};
use surrealdb_explorer::assistant::{self, Assistant, Attachment, Request, Suggestion};
use surrealdb_explorer::config::{AssistantMode, Config, Connection};
use surrealdb_explorer::db::{self, Database};
use surrealdb_explorer::graph::{self, GraphData, ResultGraph};
use surrealdb_explorer::config::DatabaseConfig;
use surrealdb_explorer::data;
use surrealdb_explorer::logging::LogControl;
use surrealdb_explorer::mcp::{self, WindowCommand};
use tokio::sync::{mpsc, oneshot};
use surrealdb_explorer::query::{self, QueryOutcome};
use tracing::{debug, error, info, warn};

use crate::gui::canvas::{Camera, GraphView};
use crate::gui::layout::Layout;
use crate::gui::model::Graph;
use crate::gui::settings::{SettingsField, SettingsForm};

/// A query's result shown as the graph: the records that answer it, and
/// the relations that show how they connect.
#[derive(Debug, Clone, Default)]
pub struct ResultView {
    pub answer: HashSet<String>,
    pub context_edges: HashSet<String>,
    /// Records and relations the queries returned.
    pub found: usize,
    /// How many weren't drawn because of the limit.
    pub left_out: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Selection {
    Node(usize),
    Edge(usize),
}

/// Actions that change the whole database. Both ask for confirmation first
/// (except loading sample data into an empty database).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DataAction {
    LoadSampleData,
    EraseDatabase,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// An animation frame while the layout is moving.
    Tick,
    Hover(Option<usize>),
    Select(Option<Selection>),
    DragNode(usize, Vector),
    DragEnd,
    Pan(Vector),
    /// Zoom by `factor`, keeping graph point `anchor` under the cursor
    /// (`from_centre` is the cursor's offset from the canvas centre).
    Zoom { factor: f32, anchor: Vector, from_centre: Vector },
    OpenMenu(usize, Point),
    CloseMenu,
    MenuInspect,
    MenuHide,
    MenuFocus,
    MenuCopy,
    ShowAll,
    Reload,

    // Settings
    OpenSettings,
    CloseSettings,
    SettingChanged(SettingsField, String),
    AssistantModeChanged(AssistantMode),
    ConnectionChanged(Connection),
    SaveSettings,

    // Query panel
    ToggleQuery,
    QueryEdited(text_editor::Action),
    RunQuery,
    /// The query's outcome, the outcome of its context query if one ran, and
    /// whether the query may have changed data.
    QueryDone(QueryOutcome, Option<QueryOutcome>, bool),
    /// The last query's result, read from the database as a graph.
    ResultLoaded(Result<ResultGraph, String>),
    /// Turns drawing every relation between a result's records on or off.
    ToggleConnectResults(bool),

    // Claude Code, through the MCP server
    /// A command from the MCP server (taken out of the cell once handled).
    FromMcp(Arc<Mutex<Option<WindowCommand>>>),
    /// The user's answer to a write Claude Code asked for.
    AnswerWrite(bool),
    /// Turns running the context query with a suggested query on or off.
    ToggleConnections(bool),
    ShowQueryInGraph,
    /// Switches path results between path rows and JSON.
    ToggleJson,

    // Plain-English questions
    QuestionEdited(String),
    Ask,
    RetryWithOpus,
    /// The assistant to keep (it holds the API key), the question, the
    /// names of the files attached to it, and the answer.
    AskDone(Option<Assistant>, String, Vec<String>, Result<Suggestion, String>),
    /// Opens the file picker to attach files to the next question.
    AttachFiles,
    /// The files picked, read, or why one couldn't be.
    FilesPicked(Vec<Result<Attachment, String>>),
    RemoveAttachment(usize),
    /// Puts the query of a chat entry back in the editor.
    UseAnswer(usize),
    /// Puts the context query of a chat entry in the editor, as a query of its own.
    UseContext(usize),
    ClearChat,

    // Data actions
    /// Shows the confirmation for an action.
    AskDataAction(DataAction),
    CancelDataAction,
    /// Runs the action waiting for confirmation.
    ConfirmDataAction,
    /// Runs an action without asking (safe cases only).
    RunDataAction(DataAction),
    DataActionDone(DataAction, Result<(), String>),
}

pub struct App {
    /// Runs the database queries. Embedded SurrealDB needs a bigger stack than
    /// tokio worker threads get by default (2 MB); SurrealDB's docs recommend
    /// 10 MB. With the default, queries run on worker threads spun forever at
    /// 100% CPU here. See https://docs.rs/surrealdb/latest/surrealdb/engine/local/index.html
    pub(super) runtime: tokio::runtime::Runtime,
    pub(super) db: Option<Database>,
    /// The configuration the app is running with.
    pub(super) config: Config,
    /// Changes the log level when settings are saved.
    pub(super) log: Option<LogControl>,

    pub(super) graph: Graph,
    pub(super) layout: Layout,
    pub(super) camera: Camera,
    pub(super) hover: Option<usize>,
    pub(super) selection: Option<Selection>,
    /// Open record menu: the record and where it was right-clicked.
    pub(super) menu: Option<(usize, Point)>,
    pub(super) dragging: Option<usize>,
    pub(super) error: Option<String>,
    pub(super) notice: Option<String>,

    /// The settings being edited, while the settings view is open.
    pub(super) settings: Option<SettingsForm>,

    pub(super) query_open: bool,
    pub(super) query_text: text_editor::Content,
    pub(super) query_running: bool,
    pub(super) query_outcome: Option<QueryOutcome>,
    /// What the suggested context query returned for the last run.
    pub(super) query_context: Option<QueryOutcome>,
    /// The last query result, shown in the graph.
    pub(super) result_view: Option<ResultView>,
    /// Show path results as JSON instead of path rows.
    pub(super) show_json: bool,

    pub(super) question: String,
    pub(super) asking: bool,
    /// Attached files: Claude sees them with every question, and queries
    /// read their data as parameters (`movies.csv` is `$movies`).
    pub(super) attachments: Vec<Attachment>,
    /// Why picked files couldn't be attached.
    pub(super) attach_problems: Vec<String>,
    pub(super) picking_files: bool,
    /// The suggested query that is in the editor (Run uses its context query).
    pub(super) suggestion: Option<Suggestion>,
    /// Every question asked and its answer, oldest first.
    pub(super) chat: Vec<Exchange>,
    /// Kept after the first question, so the API key is read only once.
    pub(super) assistant: Option<Assistant>,

    /// Shared with the MCP server; `None` if it's turned off.
    pub(super) mcp: Option<mcp::Shared>,
    /// A query Claude Code wants to run that changes data, waiting for the
    /// user, and where to send the answer.
    pub(super) pending_write: Option<(String, Arc<Mutex<Option<oneshot::Sender<bool>>>>)>,
    /// The selection last reported to the MCP server.
    synced_selection: Option<String>,

    /// A data action waiting for confirmation.
    pub(super) pending_data_action: Option<DataAction>,
    /// A data action is running.
    pub(super) data_busy: bool,
}

impl App {
    /// `startup_error` is a problem found before the window opened (such as
    /// an invalid config file), shown in the sidebar.
    pub fn boot(config: Config, startup_error: Option<String>, log: Option<LogControl>) -> App {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(10 * 1024 * 1024)
            .build()
            .expect("failed to start the async runtime");

        let limit = config.graph.max_records_per_table;
        let (db, data, error) = match runtime.block_on(db::connect(&config.database)) {
            Ok(db) => match runtime.block_on(graph::load(&db, limit)) {
                Ok(data) => (Some(db), data, None),
                Err(e) => {
                    error!(error = %e, "could not load the graph");
                    (Some(db), GraphData::default(), Some(e.to_string()))
                }
            },
            Err(e) => (None, GraphData::default(), Some(format!("Could not open the database: {e}"))),
        };

        let mcp = config.mcp.enabled.then(|| start_mcp(&runtime, config.mcp.port));
        if let Some(shared) = &mcp {
            *shared.db.lock().unwrap() = db.clone();
            *shared.limits.lock().unwrap() = (config.graph.max_result_records, config.graph.connect_results);
        }

        App {
            mcp,
            pending_write: None,
            synced_selection: None,
            runtime,
            db,
            config,
            log,
            graph: Graph::new(data, None),
            layout: Layout::new(),
            camera: Camera::default(),
            hover: None,
            selection: None,
            menu: None,
            dragging: None,
            error: startup_error.or(error),
            notice: None,
            settings: None,
            query_open: false,
            query_text: text_editor::Content::with_text("SELECT * FROM person;"),
            query_running: false,
            query_outcome: None,
            query_context: None,
            result_view: None,
            show_json: false,
            question: String::new(),
            asking: false,
            attachments: Vec::new(),
            attach_problems: Vec::new(),
            picking_files: false,
            suggestion: None,
            chat: history::load(&history::path()),
            assistant: None,
            pending_data_action: None,
            data_busy: false,
        }
    }

    pub fn theme(&self) -> Theme {
        Theme::Light
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let frames = if self.layout.is_active() || self.dragging.is_some() {
            window::frames().map(|_| Message::Tick)
        } else {
            Subscription::none()
        };
        Subscription::batch([frames, Subscription::run(mcp_commands)])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.sync_mcp();
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => self.layout.step(&mut self.graph, self.dragging),
            Message::Hover(hover) => self.hover = hover,
            Message::Select(selection) => {
                self.selection = selection;
                self.menu = None;
            }
            Message::DragNode(node, pos) => {
                self.dragging = Some(node);
                self.graph.nodes[node].pos = pos;
                self.layout.reheat(0.3);
            }
            Message::DragEnd => self.dragging = None,
            Message::Pan(delta) => self.camera.offset = self.camera.offset + delta,
            Message::Zoom { factor, anchor, from_centre } => {
                let scale = (self.camera.scale * factor).clamp(0.1, 5.0);
                self.camera.offset = from_centre - anchor * scale;
                self.camera.scale = scale;
            }
            Message::OpenMenu(node, at) => self.menu = Some((node, at)),
            Message::CloseMenu => self.menu = None,
            Message::MenuInspect => {
                if let Some((node, _)) = self.menu.take() {
                    self.selection = Some(Selection::Node(node));
                }
            }
            Message::MenuHide => {
                if let Some((node, _)) = self.menu.take() {
                    debug!(record = %self.graph.nodes[node].id, "hide record");
                    self.hide(node);
                }
            }
            Message::MenuFocus => {
                if let Some((node, _)) = self.menu.take() {
                    self.selection = Some(Selection::Node(node));
                    self.camera.scale = 1.5;
                    self.camera.offset = self.graph.nodes[node].pos * -1.5;
                }
            }
            Message::MenuCopy => {
                if let Some((node, _)) = self.menu.take() {
                    let id = self.graph.nodes[node].id.clone();
                    self.notice = Some(format!("Copied {id}"));
                    return iced::clipboard::write(id);
                }
            }
            Message::ShowAll => {
                if self.result_view.is_some() {
                    // Back from a query result to the whole-database view.
                    self.reload();
                    self.notice = None;
                } else {
                    for n in &mut self.graph.nodes {
                        n.hidden = false;
                    }
                    self.notice = None;
                    self.layout.reheat(0.3);
                }
            }
            Message::Reload => self.reload(),

            Message::OpenSettings => {
                self.settings = Some(SettingsForm::from_config(&self.config));
                self.menu = None;
            }
            Message::CloseSettings => self.settings = None,
            Message::SettingChanged(field, value) => {
                if let Some(form) = &mut self.settings {
                    form.set(field, value);
                }
            }
            Message::AssistantModeChanged(mode) => {
                if let Some(form) = &mut self.settings {
                    form.assistant_mode = mode;
                    form.message = None;
                }
            }
            Message::ConnectionChanged(connection) => {
                if let Some(form) = &mut self.settings {
                    form.connection = connection;
                    form.message = None;
                }
            }
            Message::SaveSettings => self.save_settings(),

            Message::ToggleQuery => self.query_open = !self.query_open,
            Message::QueryEdited(action) => self.query_text.perform(action),
            Message::RunQuery => return self.run_query(),
            Message::QueryDone(outcome, context, changes_data) => {
                self.query_running = false;
                let has_records = !query::record_ids(&outcome).is_empty();
                self.query_outcome = Some(outcome);
                self.query_context = context;
                // A result with records becomes the graph. Otherwise, a query
                // that changed data shows the current state of the database.
                if has_records {
                    return self.load_result();
                }
                if changes_data {
                    self.reload();
                }
            }
            Message::ShowQueryInGraph => return self.load_result(),
            Message::ToggleJson => self.show_json = !self.show_json,
            Message::ResultLoaded(Ok(result)) => self.apply_result(result),
            Message::ResultLoaded(Err(e)) => {
                error!(error = %e, "could not draw the query result");
                self.error = Some(format!("Could not draw the result: {e}"));
            }
            Message::FromMcp(cell) => {
                let Some(command) = cell.lock().ok().and_then(|mut c| c.take()) else { return Task::none() };
                match command {
                    WindowCommand::Show { query, outcome, context, graph } => {
                        info!("showing a result from Claude Code");
                        self.query_open = true;
                        self.query_text = text_editor::Content::with_text(&query);
                        self.suggestion = None;
                        self.query_outcome = Some(outcome);
                        self.query_context = context;
                        self.apply_result(graph);
                    }
                    WindowCommand::Overview => {
                        self.reload();
                        self.notice = None;
                    }
                    WindowCommand::DataChanged => {
                        if self.result_view.is_none() {
                            self.reload();
                        } else {
                            self.notice = Some("Claude Code changed data. Show the whole graph to see it.".into());
                        }
                    }
                    WindowCommand::ApproveWrite { query, reply } => {
                        // A newer request replaces an unanswered one, which counts as declined.
                        self.answer_write(false);
                        self.pending_write = Some((query, Arc::new(Mutex::new(Some(reply)))));
                    }
                }
            }
            Message::AnswerWrite(allow) => self.answer_write(allow),
            Message::ToggleConnectResults(on) => {
                self.config.graph.connect_results = on;
                if let Err(e) = self.config.save() {
                    warn!(error = %e, "could not save the setting");
                }
                if self.result_view.is_some() {
                    return self.load_result();
                }
            }

            Message::QuestionEdited(question) => self.question = question,
            Message::Ask => return self.ask(false),
            Message::RetryWithOpus => return self.ask(true),
            Message::AttachFiles => {
                if self.picking_files {
                    return Task::none();
                }
                self.picking_files = true;
                return Task::perform(pick_files(), Message::FilesPicked);
            }
            Message::FilesPicked(picked) => {
                self.picking_files = false;
                self.attach_problems = assistant::attachment::add(&mut self.attachments, picked);
                for problem in &self.attach_problems {
                    warn!(%problem, "could not attach a file");
                }
            }
            Message::RemoveAttachment(i) => {
                if i < self.attachments.len() {
                    self.attachments.remove(i);
                }
                self.attach_problems.clear();
            }
            Message::AskDone(assistant, question, attached, result) => {
                self.asking = false;
                if assistant.is_some() {
                    self.assistant = assistant;
                }
                match &result {
                    Ok(suggestion) => {
                        self.query_text = text_editor::Content::with_text(&suggestion.query);
                        self.query_outcome = None;
                        self.suggestion = Some(suggestion.clone());
                        // Attached files stay: the query reads them as parameters.
                        if self.question.trim() == question {
                            self.question.clear();
                        }
                    }
                    Err(e) => {
                        // The question stays in the box, to ask again.
                        warn!(error = %e, "question failed");
                        // A rejected key is read again next time.
                        if e.contains("rejected the API key") {
                            self.assistant = None;
                        }
                    }
                }
                self.chat.push(Exchange { question, attachments: attached, answer: result });
                self.save_chat();
            }
            Message::UseAnswer(i) => {
                if let Some(Exchange { answer: Ok(suggestion), .. }) = self.chat.get(i) {
                    self.query_text = text_editor::Content::with_text(&suggestion.query);
                    self.query_outcome = None;
                    self.suggestion = Some(suggestion.clone());
                }
            }
            Message::UseContext(i) => {
                if let Some(Exchange { answer: Ok(s), .. }) = self.chat.get(i)
                    && let Some(context) = &s.context_query
                {
                    self.query_text = text_editor::Content::with_text(context);
                    self.query_outcome = None;
                    self.suggestion = None;
                }
            }
            Message::ToggleConnections(on) => {
                self.config.assistant.show_connections = on;
                if let Err(e) = self.config.save() {
                    warn!(error = %e, "could not save the setting");
                }
            }
            Message::ClearChat => {
                self.chat.clear();
                self.save_chat();
            }

            Message::AskDataAction(action) => self.pending_data_action = Some(action),
            Message::CancelDataAction => self.pending_data_action = None,
            Message::ConfirmDataAction => {
                if let Some(action) = self.pending_data_action.take() {
                    return self.run_data_action(action);
                }
            }
            Message::RunDataAction(action) => return self.run_data_action(action),
            Message::DataActionDone(action, result) => {
                self.data_busy = false;
                match result {
                    Ok(()) => {
                        self.reload();
                        self.notice = Some(match action {
                            DataAction::LoadSampleData => "Sample data loaded.".into(),
                            DataAction::EraseDatabase => "Database erased.".into(),
                        });
                    }
                    Err(e) => {
                        error!(?action, error = %e, "data action failed");
                        self.error = Some(e);
                    }
                }
            }
        }
        Task::none()
    }

    fn hide(&mut self, node: usize) {
        self.graph.nodes[node].hidden = true;
        let touches = |s: Selection| match s {
            Selection::Node(n) => n == node,
            Selection::Edge(e) => {
                let e = &self.graph.edges[e];
                e.from == node || e.to == node
            }
        };
        if self.selection.is_some_and(touches) {
            self.selection = None;
        }
        self.hover = None;
        self.layout.reheat(0.2);
    }

    /// Reads the database again; records already on screen keep their place.
    fn reload(&mut self) {
        let Some(db) = &self.db else { return };
        let limit = self.config.graph.max_records_per_table;
        match self.runtime.block_on(graph::load(db, limit)) {
            Ok(data) => {
                self.graph = Graph::new(data, Some(&self.graph));
                self.result_view = None;
                self.error = None;
                self.notice = Some("Reloaded".into());
            }
            Err(e) => {
                error!(error = %e, "could not reload the graph");
                self.error = Some(e.to_string());
            }
        }
        self.hover = None;
        self.selection = None;
        self.menu = None;
        self.dragging = None;
        self.layout.reheat(0.5);
    }

    /// Runs a data action in the background, on our runtime (see `runtime`).
    fn run_data_action(&mut self, action: DataAction) -> Task<Message> {
        let Some(db) = self.db.clone() else { return Task::none() };
        if self.data_busy {
            return Task::none();
        }
        self.data_busy = true;
        self.pending_data_action = None;
        let config = self.config.database.clone();
        let handle = self.runtime.spawn(async move {
            match action {
                DataAction::LoadSampleData => data::load_sample_data(&db).await,
                DataAction::EraseDatabase => data::erase(&db, &config).await,
            }
            .map_err(|e| e.to_string())
        });
        Task::perform(
            async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
            move |result| Message::DataActionDone(action, result),
        )
    }

    /// Saves the settings and applies them straight away: switches the
    /// database, changes the log level and reloads the graph. Only a new log
    /// folder waits for a restart.
    fn save_settings(&mut self) {
        let Some(form) = &mut self.settings else { return };
        let new = match form.to_config() {
            Ok(config) => config,
            Err(e) => {
                form.message = Some(Err(e));
                return;
            }
        };
        if let Err(e) = new.save() {
            warn!(error = %e, "could not save settings");
            form.message = Some(Err(e.to_string()));
            return;
        }
        info!(?new, "settings saved");

        let old = self.config.clone();
        let mut problems: Vec<String> = Vec::new();
        let mut notes: Vec<&str> = Vec::new();

        let database_changed = new.database != old.database || self.db.is_none();
        if database_changed {
            match self.switch_database(&new.database) {
                Ok(()) => self.config.database = new.database.clone(),
                Err(e) => problems.push(format!("Could not open the new database, still using the previous one: {e}")),
            }
        }

        if new.logging.level != old.logging.level {
            match self.log.as_ref().map(|log| log.set_level(&new.logging.level)) {
                Some(Ok(true)) | None => {}
                Some(Ok(false)) => notes.push("RUST_LOG is set, so it still decides the log level."),
                Some(Err(e)) => problems.push(e.to_string()),
            }
            self.config.logging.level = new.logging.level.clone();
        }
        if new.logging.directory != old.logging.directory {
            notes.push("The new log folder is used after a restart.");
        }

        if new.assistant.api_key != old.assistant.api_key {
            // Read the key again from the new place on the next question.
            self.assistant = None;
        }
        self.config.assistant = new.assistant.clone();

        let graph_changed = new.graph != old.graph;
        self.config.graph = new.graph.clone();
        if database_changed || graph_changed {
            self.reload();
        }

        let message = if problems.is_empty() {
            let mut m = String::from("Saved and applied.");
            for n in notes {
                m.push(' ');
                m.push_str(n);
            }
            Ok(m)
        } else {
            Err(format!("Saved, but: {}", problems.join(" ")))
        };
        if let Some(form) = &mut self.settings {
            form.message = Some(message);
        }
    }

    /// Opens the database `config` describes in place of the current one.
    /// A different folder or server is opened first, and the old connection
    /// is closed only if that succeeds (closing a folder releases its lock,
    /// so it can be opened again later).
    fn switch_database(&mut self, config: &DatabaseConfig) -> Result<(), String> {
        // The same folder or server keeps its connection and only switches
        // namespace and database.
        let same_place = config.same_place(&self.config.database);
        match &self.db {
            Some(db) if same_place => {
                self.runtime
                    .block_on(async { db.use_ns(&config.namespace).use_db(&config.database).await })
                    .map_err(|e| e.to_string())?;
                info!(namespace = %config.namespace, database = %config.database, "switched namespace/database");
            }
            _ => {
                let new = self.runtime.block_on(db::connect(config)).map_err(|e| e.to_string())?;
                if let Some(old) = self.db.replace(new)
                    && let Err(e) = self.runtime.block_on(db::close(old))
                {
                    warn!(error = %e, "could not close the previous database");
                }
            }
        }
        // The query results and pending actions belong to the previous database.
        self.query_outcome = None;
        self.query_context = None;
        self.result_view = None;
        self.suggestion = None;
        self.pending_data_action = None;
        Ok(())
    }

    fn run_query(&mut self) -> Task<Message> {
        let sql = self.query_text.text();
        let Some(db) = self.db.clone() else { return Task::none() };
        if sql.trim().is_empty() || self.query_running {
            return Task::none();
        }
        self.query_running = true;
        // A query Claude suggested, run unchanged, brings its context query
        // (unless that's turned off).
        let changes_data = !assistant::is_read_only(&sql);
        let context = self
            .suggestion_in_editor()
            .filter(|_| self.config.assistant.show_connections)
            .and_then(|s| s.context_query.clone());
        // Attached files are parameters the query can read, e.g. `$movies`.
        let params = assistant::attachment::params(&self.attachments);
        // Run on our runtime (big thread stacks, see `runtime`), and only
        // wait for the result on iced's side, so the window stays responsive.
        let handle = self.runtime.spawn(async move {
            let outcome = query::run_with(&db, sql, &params).await;
            let context = match context {
                Some(context) if outcome.statements.is_ok() => Some(query::run_with(&db, context, &params).await),
                _ => None,
            };
            (outcome, context)
        });
        Task::perform(
            async move {
                handle.await.unwrap_or_else(|e| {
                    let failed = QueryOutcome {
                        statements: Err(format!("the query task failed: {e}")),
                        elapsed: Default::default(),
                    };
                    (failed, None)
                })
            },
            move |(outcome, context)| Message::QueryDone(outcome, context, changes_data),
        )
    }

    /// Draws a query result as the graph.
    fn apply_result(&mut self, result: ResultGraph) {
        if result.data.nodes.is_empty() {
            self.notice = Some("None of the returned records could be drawn.".into());
            return;
        }
        let limit = self.config.graph.max_result_records;
        self.graph = Graph::new(result.data, None);
        self.notice = (result.left_out > 0).then(|| {
            format!("Drew the first {limit} of {} records. Raise the limit in Settings to see more.", result.found)
        });
        self.result_view = Some(ResultView {
            answer: result.answer,
            context_edges: result.context_edges,
            found: result.found,
            left_out: result.left_out,
        });
        self.camera = Camera::default();
        self.hover = None;
        self.selection = None;
        self.menu = None;
        self.dragging = None;
        self.layout.reheat(1.0);
    }

    fn answer_write(&mut self, allow: bool) {
        if let Some((query, reply)) = self.pending_write.take() {
            info!(allow, %query, "answered a write from Claude Code");
            if let Some(reply) = reply.lock().ok().and_then(|mut r| r.take()) {
                let _ = reply.send(allow);
            }
        }
    }

    /// Keeps the MCP server's view of the app current: the open database,
    /// the result limits and the selection.
    fn sync_mcp(&mut self) {
        let Some(shared) = &self.mcp else { return };
        if let Ok(mut db) = shared.db.lock() {
            // A cheap handle copy; keeps up with database switches in Settings.
            *db = self.db.clone();
        }
        if let Ok(mut limits) = shared.limits.lock() {
            *limits = (self.config.graph.max_result_records, self.config.graph.connect_results);
        }
        let key = self.selection.map(|s| match s {
            Selection::Node(i) => self.graph.nodes.get(i).map(|n| n.id.clone()).unwrap_or_default(),
            Selection::Edge(i) => self.graph.edges.get(i).map(|e| e.id.clone()).unwrap_or_default(),
        });
        if key != self.synced_selection {
            let json = self.selection.map(|s| self.selection_json(s));
            if let Ok(mut window) = shared.window.lock() {
                window.selection = json;
            }
            self.synced_selection = key;
        }
    }

    /// The selected record or relation, with its fields and connections.
    fn selection_json(&self, selection: Selection) -> serde_json::Value {
        let g = &self.graph;
        match selection {
            Selection::Node(i) => {
                let n = &g.nodes[i];
                let outgoing: Vec<_> = g.edges.iter().filter(|e| e.from == i)
                    .map(|e| serde_json::json!({"relation": e.id, "table": e.label, "to": g.nodes[e.to].id})).collect();
                let incoming: Vec<_> = g.edges.iter().filter(|e| e.to == i)
                    .map(|e| serde_json::json!({"relation": e.id, "table": e.label, "from": g.nodes[e.from].id})).collect();
                serde_json::json!({"record": n.id, "table": n.table, "fields": n.data,
                    "outgoing_in_view": outgoing, "incoming_in_view": incoming})
            }
            Selection::Edge(i) => {
                let e = &g.edges[i];
                serde_json::json!({"relation": e.id, "table": e.label, "from": g.nodes[e.from].id,
                    "to": g.nodes[e.to].id, "fields": e.data})
            }
        }
    }

    /// The suggestion whose query is in the editor, unchanged.
    pub(super) fn suggestion_in_editor(&self) -> Option<&Suggestion> {
        self.suggestion.as_ref().filter(|s| s.query.trim() == self.query_text.text().trim())
    }

    fn save_chat(&self) {
        if let Err(e) = history::save(&history::path(), &self.chat) {
            warn!(error = %e, "could not save the chat history");
        }
    }

    /// Asks Claude for a query, in the background on our runtime. The first
    /// time, the API key is read from 1Password, which may ask for approval.
    /// `retry_with_opus` redoes the last answer with Opus.
    fn ask(&mut self, retry_with_opus: bool) -> Task<Message> {
        let Some(db) = self.db.clone() else { return Task::none() };
        let previous = self.suggestion.clone();
        let question = self.question.trim().to_string();
        if self.asking || (retry_with_opus && previous.is_none()) || (!retry_with_opus && question.is_empty()) {
            return Task::none();
        }
        self.asking = true;
        let (asked, attached) = match (&previous, retry_with_opus) {
            (Some(previous), true) => {
                (previous.question.clone(), previous.attachments.iter().map(|a| a.name.clone()).collect())
            }
            _ => (question.clone(), self.attachments.iter().map(|a| a.name.clone()).collect::<Vec<_>>()),
        };
        // Claude sees the query in the editor, so the question can ask to change it.
        let editor_query = Some(self.query_text.text()).filter(|q| !q.trim().is_empty());
        let request = Request { question: question.clone(), editor_query, attachments: self.attachments.clone() };
        let mode = self.config.assistant.mode;
        let reference = self.config.assistant.api_key.clone();
        let cached = self.assistant.clone();

        let handle = self.runtime.spawn(async move {
            let assistant = match cached {
                Some(assistant) => assistant,
                None => match tokio::task::spawn_blocking(move || assistant::key::fetch(&reference)).await {
                    Ok(Ok(key)) => Assistant::new(key),
                    Ok(Err(e)) => return (None, Err(e.to_string())),
                    Err(e) => return (None, Err(format!("reading the API key failed: {e}"))),
                },
            };
            let result = match (retry_with_opus, previous) {
                (true, Some(previous)) => assistant.retry_with_opus(&db, &previous).await,
                _ => assistant.ask(&db, &request, mode).await,
            };
            (Some(assistant), result.map_err(|e| e.to_string()))
        });
        Task::perform(
            async move { handle.await.unwrap_or_else(|e| (None, Err(format!("the task failed: {e}")))) },
            move |(assistant, result)| Message::AskDone(assistant, asked.clone(), attached.clone(), result),
        )
    }

    /// Draws the last query's result as the graph, in the background: the
    /// records and relations it (and its context query) returned, read from
    /// the database by id.
    fn load_result(&mut self) -> Task<Message> {
        let (Some(db), Some(outcome)) = (self.db.clone(), &self.query_outcome) else { return Task::none() };
        let answer = query::record_ids(outcome);
        let context: Vec<String> = self.query_context.iter().flat_map(query::record_ids).collect();
        let limit = self.config.graph.max_result_records;
        let connect = self.config.graph.connect_results;
        self.notice = Some("Drawing the result…".into());
        let handle = self.runtime.spawn(async move {
            graph::load_result(&db, &answer, &context, limit, connect).await.map_err(|e| e.to_string())
        });
        Task::perform(
            async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
            Message::ResultLoaded,
        )
    }

    /// The result view as record and relation indices into the current graph.
    pub(super) fn result_indices(&self) -> Option<(HashSet<usize>, HashSet<usize>)> {
        let view = self.result_view.as_ref()?;
        let nodes: HashMap<&str, usize> =
            self.graph.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let answer = view.answer.iter().filter_map(|id| nodes.get(id.as_str()).copied()).collect();
        let edges = self
            .graph
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| view.context_edges.contains(&e.id))
            .map(|(i, _)| i)
            .collect();
        Some((answer, edges))
    }

    pub fn view(&self) -> Element<'_, Message> {
        let graph = canvas(GraphView {
            graph: &self.graph,
            camera: self.camera,
            hover: self.hover,
            selection: self.selection,
            result: self.result_indices(),
        })
        .width(Fill)
        .height(Fill);

        let mut layers = stack![graph].width(Fill).height(Fill);
        if let Some((node, at)) = self.menu {
            layers = layers.push(pin(self.record_menu(node)).x(at.x).y(at.y));
        }
        if let Some((query, _)) = &self.pending_write {
            layers = layers.push(write_banner(query));
        }

        let side = match &self.settings {
            Some(form) => form.view(self.pending_data_action, self.data_busy, self.db.is_some()),
            None => self.sidebar(),
        };

        row![
            column![layers, rule::horizontal(1), self.query_panel()].width(Fill),
            rule::vertical(1),
            side
        ]
        .into()
    }

    fn record_menu(&self, node: usize) -> Element<'_, Message> {
        let n = &self.graph.nodes[node];
        let item = |label: &'static str, message: Message| {
            button(text(label).size(14))
                .width(Fill)
                .style(button::text)
                .on_press(message)
        };
        container(column![
            container(row![text("●").color(n.color), text(&n.id).font(Font::MONOSPACE).size(13)].spacing(8))
                .padding([6, 10]),
            rule::horizontal(1),
            item("Inspect", Message::MenuInspect),
            item("Hide record", Message::MenuHide),
            item("Focus record", Message::MenuFocus),
            item("Copy record id", Message::MenuCopy),
        ])
        .width(210)
        .padding(4)
        .style(|theme| container::bordered_box(theme).background(Color::WHITE))
        .into()
    }
}

pub(super) fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Asks which files to attach, and reads them.
async fn pick_files() -> Vec<Result<Attachment, String>> {
    let Some(files) = rfd::AsyncFileDialog::new()
        .set_title("Attach files to the question")
        .pick_files()
        .await
    else {
        return Vec::new();
    };
    let mut picked = Vec::new();
    for file in files {
        let name = file.file_name();
        // Check the size first, so a huge file isn't read for nothing.
        let size = std::fs::metadata(file.path()).map(|m| m.len()).unwrap_or(0);
        if size > assistant::attachment::MAX_BYTES as u64 {
            picked.push(Err(assistant::attachment::too_large(&name, size)));
            continue;
        }
        picked.push(Attachment::from_bytes(&name, file.read().await));
    }
    picked
}

/// Where the MCP server's commands wait for the window's subscription.
static MCP_COMMANDS: OnceLock<Mutex<Option<mpsc::UnboundedReceiver<WindowCommand>>>> = OnceLock::new();

/// Starts the MCP server on our runtime and returns what it shares with the window.
fn start_mcp(runtime: &tokio::runtime::Runtime, port: u16) -> mcp::Shared {
    let (to_window, commands) = mpsc::unbounded_channel();
    let _ = MCP_COMMANDS.set(Mutex::new(Some(commands)));
    let shared = mcp::Shared::new(to_window);
    let server = shared.clone();
    runtime.spawn(async move {
        if let Err(e) = mcp::serve(server.clone(), port).await {
            error!(error = %e, "the MCP server stopped");
            let hint = if e.to_string().contains("in use") {
                " Is another copy of SurrealDB Explorer open? Close it and restart this one."
            } else {
                ""
            };
            if let Ok(mut window) = server.window.lock() {
                window.server_error = Some(format!("Claude Code's tools aren't available: {e}.{hint}"));
            }
        }
    });
    shared
}

/// The MCP server's commands, as messages for the window.
fn mcp_commands() -> impl iced::futures::Stream<Item = Message> {
    let commands = MCP_COMMANDS.get().and_then(|cell| cell.lock().ok()?.take());
    iced::futures::stream::unfold(commands, |commands| async move {
        let mut commands = commands?;
        let command = commands.recv().await?;
        Some((Message::FromMcp(Arc::new(Mutex::new(Some(command)))), Some(commands)))
    })
}

/// Asks the user to allow a query from Claude Code that changes data.
fn write_banner(query: &str) -> Element<'_, Message> {
    let shown: String = {
        let lines: Vec<&str> = query.lines().take(15).collect();
        let more = query.lines().count().saturating_sub(15);
        if more > 0 { format!("{}\n… {more} more lines", lines.join("\n")) } else { lines.join("\n") }
    };
    container(
        column![
            text("Claude Code wants to run a query that changes data:").size(14).font(Font {
                weight: iced::font::Weight::Bold,
                ..Font::default()
            }),
            container(text(shown).size(12).font(Font::MONOSPACE))
                .padding(6)
                .width(Fill)
                .style(|_| container::background(Color::from_rgb(0.95, 0.95, 0.95))),
            row![
                button(text("Allow").size(13)).style(button::danger).on_press(Message::AnswerWrite(true)),
                button(text("Don't allow").size(13)).style(button::secondary).on_press(Message::AnswerWrite(false)),
            ]
            .spacing(8),
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Fill)
    .style(|theme| container::bordered_box(theme).background(Color::from_rgb(1.0, 0.95, 0.88)))
    .into()
}
