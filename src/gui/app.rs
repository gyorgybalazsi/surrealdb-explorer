//! Window state and message handling. The views are in `sidebar.rs`,
//! `settings.rs` and `query_panel.rs`.

use std::sync::{Arc, Mutex, OnceLock};

use iced::widget::{button, canvas, column, container, pin, row, rule, stack, text, text_editor};
use iced::{Color, Element, Fill, Font, Point, Subscription, Task, Theme, Vector, window};
use surrealdb_explorer::config::{Config, Connection, Visualization};
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

/// A query's result is shown as the graph (instead of the overview).
#[derive(Debug, Clone, Copy, Default)]
pub struct ResultView;

/// What the main area shows: the graph, or the last result as a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Graph,
    Table,
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
    /// A sample Canton event graph, shown with the Canton visualization.
    LoadCantonSample,
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
    /// Switches how the graph is drawn (and remembers it).
    SetVisualization(Visualization),
    /// Fits the whole graph in view.
    FitView,

    // Settings
    OpenSettings,
    CloseSettings,
    SettingChanged(SettingsField, String),
    ConnectionChanged(Connection),
    SaveSettings,

    // Query panel
    ToggleQuery,
    QueryEdited(text_editor::Action),
    RunQuery,
    /// The query's outcome, and whether the query may have changed data.
    QueryDone(QueryOutcome, bool),
    ShowTab(Tab),
    /// The last query's result, read from the database as a graph.
    ResultLoaded(Result<ResultGraph, String>),
    /// Turns drawing every relation between a result's records on or off.
    ToggleConnectResults(bool),

    // Claude Code, through the MCP server
    /// A command from the MCP server (taken out of the cell once handled).
    FromMcp(Arc<Mutex<Option<WindowCommand>>>),
    /// The user's answer to a write Claude Code asked for.
    AnswerWrite(bool),
    ShowQueryInGraph,

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
    /// The last query result, shown as the graph.
    pub(super) result_view: Option<ResultView>,
    pub(super) tab: Tab,

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

        let mut app = App {
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
            result_view: None,
            tab: Tab::Graph,
            pending_data_action: None,
            data_busy: false,
        };
        app.apply_visualization();
        app
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
                self.relayout_canton();
            }
            Message::Reload => self.reload(),
            Message::SetVisualization(v) => {
                if v != self.config.graph.visualization {
                    info!(visualization = %v, "visualization changed");
                    self.config.graph.visualization = v;
                    if let Err(e) = self.config.save() {
                        warn!(error = %e, "could not save the setting");
                    }
                    self.apply_visualization();
                }
            }
            Message::FitView => self.fit_view(),

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
            Message::ShowTab(tab) => self.tab = tab,
            Message::QueryDone(outcome, changes_data) => {
                self.query_running = false;
                let has_records = !query::record_ids(&outcome).is_empty();
                self.query_outcome = Some(outcome);
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
            Message::ResultLoaded(Ok(result)) => self.apply_result(result),
            Message::ResultLoaded(Err(e)) => {
                error!(error = %e, "could not draw the query result");
                self.error = Some(format!("Could not draw the result: {e}"));
            }
            Message::FromMcp(cell) => {
                let Some(command) = cell.lock().ok().and_then(|mut c| c.take()) else { return Task::none() };
                match command {
                    WindowCommand::Show { query, outcome, graph } => {
                        info!("showing a result from Claude Code");
                        self.query_open = true;
                        self.query_text = text_editor::Content::with_text(&query);
                        self.query_outcome = Some(outcome);
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
                        if action == DataAction::LoadCantonSample {
                            self.config.graph.visualization = Visualization::Canton;
                            if let Err(e) = self.config.save() {
                                warn!(error = %e, "could not save the setting");
                            }
                            self.apply_visualization();
                        }
                        self.notice = Some(match action {
                            DataAction::LoadSampleData => "Sample data loaded.".into(),
                            DataAction::LoadCantonSample => "Canton sample loaded.".into(),
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
                if self.canton() {
                    self.graph.canton_layout();
                }
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

    fn canton(&self) -> bool {
        self.config.graph.visualization == Visualization::Canton
    }

    /// Lays the graph out for the current visualization: the Canton event
    /// graph is placed like the Canton toolbox does and stays put; the
    /// standard view starts the force-directed layout.
    fn apply_visualization(&mut self) {
        let canton = self.canton();
        self.layout.set_enabled(!canton);
        if canton {
            self.graph.canton_layout();
        }
        self.fit_view();
    }

    /// In the Canton view, places the visible records again (after records
    /// were shown or hidden in bulk) and fits them in view.
    fn relayout_canton(&mut self) {
        if self.canton() {
            self.graph.canton_layout();
            self.fit_view();
        }
    }

    /// Zooms and pans so every visible record is in view. The standard view
    /// just recentres, since its layout is still spreading out.
    fn fit_view(&mut self) {
        // Roughly the canvas: the window less the sidebar and the query bar.
        const AREA: Vector = Vector::new(840.0, 640.0);
        const MARGIN: f32 = 90.0;
        let Some((min, max)) = self.graph.bounds() else {
            self.camera = Camera::default();
            return;
        };
        let size = max - min + Vector::new(2.0 * MARGIN, 2.0 * MARGIN);
        let scale = if self.canton() { (AREA.x / size.x).min(AREA.y / size.y).clamp(0.1, 1.5) } else { 1.0 };
        let centre = (min + max) * 0.5;
        self.camera = Camera { offset: centre * -scale, scale };
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
                DataAction::LoadCantonSample => data::load_canton_sample(&db).await,
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
        self.result_view = None;
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
        let changes_data = !query::is_read_only(&sql);
        // Run on our runtime (big thread stacks, see `runtime`), and only
        // wait for the result on iced's side, so the window stays responsive.
        let handle = self.runtime.spawn(async move { query::run(&db, sql).await });
        Task::perform(
            async move {
                handle.await.unwrap_or_else(|e| QueryOutcome {
                    statements: Err(format!("the query task failed: {e}")),
                    elapsed: Default::default(),
                })
            },
            move |outcome| Message::QueryDone(outcome, changes_data),
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
        self.result_view = Some(ResultView);
        self.camera = Camera::default();
        self.hover = None;
        self.selection = None;
        self.menu = None;
        self.dragging = None;
        self.layout.reheat(1.0);
        self.relayout_canton();
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

    /// Draws the last query's result as the graph, in the background: the
    /// records and relations it returned, read from the database by id.
    fn load_result(&mut self) -> Task<Message> {
        let (Some(db), Some(outcome)) = (self.db.clone(), &self.query_outcome) else { return Task::none() };
        let answer = query::record_ids(outcome);
        let limit = self.config.graph.max_result_records;
        let connect = self.config.graph.connect_results;
        self.notice = Some("Drawing the result…".into());
        let handle = self.runtime.spawn(async move {
            graph::load_result(&db, &answer, &[], limit, connect).await.map_err(|e| e.to_string())
        });
        Task::perform(
            async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
            Message::ResultLoaded,
        )
    }

    pub fn view(&self) -> Element<'_, Message> {
        let graph = canvas(GraphView {
            graph: &self.graph,
            camera: self.camera,
            hover: self.hover,
            selection: self.selection,
            visualization: self.config.graph.visualization,
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

        let main: Element<'_, Message> = match self.tab {
            Tab::Graph => layers.into(),
            Tab::Table => self.table_view(),
        };
        // A pending write must be answered whichever tab is open.
        let main: Element<'_, Message> = match (&self.pending_write, self.tab) {
            (Some((query, _)), Tab::Table) => column![write_banner(query), main].into(),
            _ => main,
        };

        row![
            column![self.tab_bar(), main, rule::horizontal(1), self.query_panel()].width(Fill),
            rule::vertical(1),
            side
        ]
        .into()
    }

    /// Switches the main area between the graph and the result table.
    fn tab_bar(&self) -> Element<'_, Message> {
        let tab = |label: &'static str, which: Tab| {
            button(text(label).size(13))
                .style(if self.tab == which { button::primary } else { button::text })
                .padding([3, 12])
                .on_press(Message::ShowTab(which))
        };
        container(row![tab("Graph", Tab::Graph), tab("Table", Tab::Table)].spacing(4))
            .padding([4, 8])
            .width(Fill)
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
