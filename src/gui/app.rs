//! Window state and message handling. The views are in `sidebar.rs`,
//! `settings.rs` and `query_panel.rs`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use iced::widget::{button, canvas, column, container, pin, row, rule, stack, text, text_editor};
use iced::{Color, Element, Fill, Font, Point, Subscription, Task, Theme, Vector, window};
use surrealdb_explorer::canton::{self, Filters, NodeKind, StatusFilter};
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

/// One change to the Canton view's filters.
#[derive(Debug, Clone)]
pub enum CantonFilterChange {
    Kind(NodeKind, bool),
    /// Show exactly these kinds.
    Kinds(Vec<NodeKind>),
    Party(Option<String>),
    Template(Option<String>),
    Status(StatusFilter),
    Search(String),
    /// The text typed in the "from" or "to" offset box.
    OffsetFrom(String),
    OffsetTo(String),
    Reset,
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
    /// Shows the tables as a graph.
    ShowOverview,
    /// Loads another window of transactions in the Canton view.
    CantonPage(graph::CantonWindow),
    /// Draws the whole database even though it's bigger than `overview_above`.
    LoadEverything,
    /// From an overview table's menu: draws that table's records.
    MenuShowRecords,
    /// From a record's menu: adds its neighbours to the graph.
    MenuNeighbours,
    /// A record's neighbours, read from the database.
    NeighboursLoaded(String, Result<ResultGraph, String>),
    /// Turns auto refresh on or off (and remembers it).
    ToggleAutoRefresh(bool),
    /// Time for auto refresh to look for changes in the database.
    CheckForChanges,
    /// How many records each table has now.
    Counted(Result<Vec<(String, i64)>, String>),
    /// Switches how the graph is drawn (and remembers it).
    SetVisualization(Visualization),
    /// Fits the whole graph in view.
    FitView,
    /// The canvas has a new size (the Canton layout wraps to its width).
    CanvasResized(iced::Size),
    /// A change to the Canton view's filters.
    CantonFilter(CantonFilterChange),

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

    /// The size of the graph canvas, as last drawn.
    pub(super) canvas_size: iced::Size,
    /// What the Canton view shows.
    pub(super) canton_filters: Filters,
    /// How many records each table had when the graph was last loaded, for
    /// auto refresh; `None` until the first check after a load.
    record_counts: Option<Vec<(String, i64)>>,
    /// Auto refresh is counting records.
    counting: bool,
    /// The camera follows the standard layout while it settles.
    following: bool,
    /// The graph shows the database's tables, not its records.
    pub(super) overview: bool,
    /// Which transactions the Canton view loads, and the offsets it got.
    pub(super) canton_window: graph::CantonWindow,
    pub(super) canton_offsets: Option<(i64, i64)>,
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
        let config_canton_transactions = config.graph.canton_transactions;
        let (db, data, error) = match runtime.block_on(db::connect(&config.database)) {
            // The Canton view loads its own window of transactions below.
            Ok(db) if config.graph.visualization == Visualization::Canton => (Some(db), GraphData::default(), None),
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
            canvas_size: iced::Size::new(880.0, 760.0),
            canton_filters: Filters::default(),
            record_counts: None,
            counting: false,
            following: true,
            overview: false,
            canton_window: graph::CantonWindow::Latest(config_canton_transactions),
            canton_offsets: None,
        };
        app.apply_visualization();
        if app.canton() {
            // The Canton view loads a window of transactions, not the first records of each table.
            app.load_everything();
            app.relayout_canton();
        } else if app.too_big().is_some() {
            app.show_overview();
        }
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
        let refresh = if self.auto_refresh_running() {
            iced::time::every(std::time::Duration::from_secs(self.config.graph.refresh_seconds))
                .map(|_| Message::CheckForChanges)
        } else {
            Subscription::none()
        };
        Subscription::batch([frames, refresh, Subscription::run(mcp_commands)])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.sync_mcp();
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.layout.step(&mut self.graph, self.dragging);
                self.follow_layout();
            }
            Message::Hover(hover) => self.hover = hover,
            Message::Select(selection) => {
                self.selection = selection;
                self.menu = None;
            }
            Message::DragNode(node, pos) => {
                self.following = false;
                self.dragging = Some(node);
                self.graph.nodes[node].pos = pos;
                self.layout.reheat(0.3);
            }
            Message::DragEnd => self.dragging = None,
            Message::Pan(delta) => {
                self.following = false;
                self.camera.offset = self.camera.offset + delta;
            }
            Message::Zoom { factor, anchor, from_centre } => {
                self.following = false;
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
            Message::ShowOverview => self.show_overview(),
            Message::LoadEverything => {
                self.load_everything();
                self.layout.reheat(0.5);
                self.following = true;
                self.presettle();
            }
            Message::MenuShowRecords => {
                let Some((node, _)) = self.menu.take() else { return Task::none() };
                let table = self.graph.nodes[node].table.clone();
                let Some(db) = self.db.clone() else { return Task::none() };
                let limit = self.config.graph.max_records_per_table;
                self.notice = Some(format!("Loading {table}…"));
                let handle = self.runtime.spawn(async move {
                    graph::table_records(&db, &table, limit).await.map_err(|e| e.to_string())
                });
                return Task::perform(
                    async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
                    Message::ResultLoaded,
                );
            }
            Message::MenuNeighbours => {
                let Some((node, _)) = self.menu.take() else { return Task::none() };
                let id = self.graph.nodes[node].id.clone();
                let Some(db) = self.db.clone() else { return Task::none() };
                let limit = self.config.graph.max_result_records;
                let handle = self.runtime.spawn({
                    let id = id.clone();
                    async move { graph::neighbours(&db, &id, limit).await.map_err(|e| e.to_string()) }
                });
                return Task::perform(
                    async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
                    move |result| Message::NeighboursLoaded(id.clone(), result),
                );
            }
            Message::NeighboursLoaded(id, Ok(result)) => self.add_neighbours(&id, result),
            Message::NeighboursLoaded(_, Err(e)) => {
                error!(error = %e, "could not load the neighbours");
                self.error = Some(format!("Could not load the neighbours: {e}"));
            }
            Message::ToggleAutoRefresh(on) => {
                info!(on, "auto refresh changed");
                self.config.graph.auto_refresh = on;
                self.record_counts = None;
                if let Err(e) = self.config.save() {
                    warn!(error = %e, "could not save the setting");
                }
            }
            Message::CheckForChanges => return self.count_records(),
            Message::Counted(result) => {
                self.counting = false;
                match result {
                    // Only act on the answer if auto refresh is still wanted:
                    // a query result may have been drawn meanwhile.
                    Ok(counts) if self.auto_refresh_running() => match &self.record_counts {
                        Some(previous) if *previous != counts => {
                            debug!("the database changed; refreshing the graph");
                            self.refresh();
                            self.record_counts = Some(counts);
                        }
                        Some(_) => {}
                        None => self.record_counts = Some(counts),
                    },
                    Ok(_) => {}
                    Err(e) => debug!(error = %e, "auto refresh could not count the records"),
                }
            }
            Message::SetVisualization(v) => {
                if v != self.config.graph.visualization {
                    info!(visualization = %v, "visualization changed");
                    self.config.graph.visualization = v;
                    if let Err(e) = self.config.save() {
                        warn!(error = %e, "could not save the setting");
                    }
                    self.apply_visualization();
                    // The views load different records: a window of transactions, or every table.
                    self.reload();
                    self.relayout_canton();
                }
            }
            Message::CantonPage(window) => {
                self.canton_window = window;
                self.load_everything();
                self.relayout_canton();
            }
            Message::FitView => self.fit_view(),
            Message::CanvasResized(size) => {
                self.canvas_size = size;
                if self.canton() {
                    self.relayout_canton();
                }
            }
            Message::CantonFilter(change) => {
                let f = &mut self.canton_filters;
                match change {
                    CantonFilterChange::Kind(kind, on) => {
                        if on {
                            f.kinds.insert(kind);
                        } else {
                            f.kinds.remove(&kind);
                        }
                    }
                    CantonFilterChange::Kinds(kinds) => f.kinds = kinds.into_iter().chain([NodeKind::Other]).collect(),
                    CantonFilterChange::Party(p) => f.party = p,
                    CantonFilterChange::Template(t) => f.template = t,
                    CantonFilterChange::Status(st) => f.status = st,
                    CantonFilterChange::Search(text) => f.search = text,
                    CantonFilterChange::OffsetFrom(text) => f.offset_from = parse_offset(&text, f.offset_from),
                    CantonFilterChange::OffsetTo(text) => f.offset_to = parse_offset(&text, f.offset_to),
                    CantonFilterChange::Reset => *f = Filters::default(),
                }
                // Filtering keeps the view where it is, as in the explorer.
                if self.canton() {
                    self.canton_layout_fitted();
                    if self.selection.is_some_and(|s| matches!(s, Selection::Node(n) if !self.graph.nodes[n].card.shown)) {
                        self.selection = None;
                    }
                    self.hover = None;
                }
            }

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
        if !self.canton() && self.too_big().is_some() {
            self.show_overview();
            return;
        }
        self.load_everything();
    }

    /// How many records the database has, if more than `overview_above`.
    fn too_big(&self) -> Option<usize> {
        let db = self.db.as_ref()?;
        match self.runtime.block_on(graph::record_count(db)) {
            Ok(n) if n > self.config.graph.overview_above => Some(n),
            Ok(_) => None,
            Err(e) => {
                warn!(error = %e, "could not count the records");
                None
            }
        }
    }

    /// Draws the tables as a graph: dots sized by their records, arrows for
    /// the relations between them.
    fn show_overview(&mut self) {
        let Some(db) = &self.db else { return };
        match self.runtime.block_on(graph::overview(db)) {
            Ok(data) => {
                info!(tables = data.nodes.len(), "showing the table overview");
                self.graph = Graph::new(data, None);
                self.overview = true;
                self.result_view = None;
                self.error = None;
                self.notice = None;
            }
            Err(e) => {
                error!(error = %e, "could not load the table overview");
                self.error = Some(e.to_string());
                return;
            }
        }
        self.camera = Camera::default();
        self.hover = None;
        self.selection = None;
        self.menu = None;
        self.dragging = None;
        self.layout.reheat(1.0);
        self.following = true;
    }

    /// Reads the graph to draw: in the Canton view the current window of
    /// transactions, otherwise every table's records up to the per-table limit.
    fn fetch_graph(&mut self) -> Result<GraphData, String> {
        let Some(db) = &self.db else { return Err("not connected".into()) };
        let per_table = self.config.graph.max_records_per_table;
        if self.canton() {
            let loaded = self
                .runtime
                .block_on(graph::load_canton_window(db, self.canton_window, per_table, self.config.graph.max_result_records))
                .map_err(|e| e.to_string())?;
            self.canton_offsets = loaded.offsets;
            if loaded.left_out > 0 {
                warn!(left_out = loaded.left_out, "the Canton window had more records than max_result_records");
            }
            return Ok(loaded.data);
        }
        self.runtime.block_on(graph::load(db, per_table)).map_err(|e| e.to_string())
    }

    /// Draws every table's records, up to the per-table limit (in the Canton
    /// view: the current window of transactions).
    fn load_everything(&mut self) {
        self.overview = false;
        if self.db.is_none() {
            return;
        }
        self.record_counts = None;
        match self.fetch_graph() {
            Ok(data) => {
                self.graph = Graph::new(data, Some(&self.graph));
                self.result_view = None;
                if self.canton() {
                    self.canton_layout_fitted();
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
        self.following = true;
        self.presettle();
    }

    /// A graph past a few hundred records is laid out (mostly) before it's
    /// drawn and fitted at once: animating it would take many seconds.
    fn presettle(&mut self) {
        const BIG: usize = 200;
        if self.canton() || self.graph.nodes.iter().filter(|n| !n.hidden).count() < BIG {
            return;
        }
        let started = std::time::Instant::now();
        self.layout.settle(&mut self.graph, std::time::Duration::from_millis(700));
        debug!(elapsed_ms = started.elapsed().as_millis() as u64, "pre-settled a big graph");
        self.camera = self.standard_fit();
    }

    /// Whether auto refresh is checking the database: it's on, and the whole
    /// database is shown (not a query result, which it would replace).
    pub(super) fn auto_refresh_running(&self) -> bool {
        self.config.graph.auto_refresh && self.db.is_some() && self.result_view.is_none() && !self.overview
    }

    /// Counts the records in the background, for auto refresh.
    fn count_records(&mut self) -> Task<Message> {
        let Some(db) = self.db.clone() else { return Task::none() };
        // Skip a check while the user is moving things or one is still running.
        if self.counting || self.dragging.is_some() || self.menu.is_some() || self.data_busy {
            return Task::none();
        }
        self.counting = true;
        let handle = self.runtime.spawn(async move {
            graph::record_counts(&db).await.map_err(|e| e.to_string())
        });
        Task::perform(
            async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
            Message::Counted,
        )
    }

    /// Loads the database again after auto refresh saw a change. Unlike
    /// Reload, it keeps the view as it is: the camera, the selected record
    /// and the records the user hid.
    fn refresh(&mut self) {
        if self.db.is_none() {
            return;
        }
        let data = match self.fetch_graph() {
            Ok(data) => data,
            Err(e) => {
                warn!(error = %e, "auto refresh could not load the graph");
                return;
            }
        };
        let id_of = |g: &Graph, i: usize| g.nodes.get(i).map(|n| n.id.clone());
        let selected = match self.selection {
            Some(Selection::Node(i)) => id_of(&self.graph, i),
            _ => None,
        };
        let hidden: std::collections::HashSet<String> =
            self.graph.nodes.iter().filter(|n| n.hidden).map(|n| n.id.clone()).collect();
        let before = self.graph.nodes.len();

        self.graph = Graph::new(data, Some(&self.graph));
        for n in &mut self.graph.nodes {
            if hidden.contains(&n.id) {
                n.hidden = true;
            }
        }
        self.selection = selected
            .and_then(|id| self.graph.nodes.iter().position(|n| n.id == id))
            .map(Selection::Node);
        self.hover = None;
        if self.canton() {
            self.canton_layout_fitted();
        } else {
            self.layout.reheat(0.3);
        }
        let added = self.graph.nodes.len() as i64 - before as i64;
        self.notice = Some(match added {
            0 => "Updated".into(),
            n if n > 0 => format!("Updated: {} new", plural(n as usize, "record")),
            n => format!("Updated: {} removed", plural((-n) as usize, "record")),
        });
    }

    fn canton(&self) -> bool {
        self.config.graph.visualization == Visualization::Canton
    }

    /// Lays the graph out for the current visualization: the Canton event
    /// graph is placed as alt-canton's ledger explorer does and stays put; the
    /// standard view starts the force-directed layout.
    fn apply_visualization(&mut self) {
        let canton = self.canton();
        self.layout.set_enabled(!canton);
        if canton {
            self.canton_layout_fitted();
        } else {
            self.graph.clear_canton();
            self.following = true;
        }
        self.fit_view();
    }

    /// In the Canton view, places the shown records again (after records
    /// were shown or hidden in bulk, or the canvas changed width) and fits them in view.
    fn relayout_canton(&mut self) {
        if self.canton() {
            self.canton_layout_fitted();
            self.fit_view();
        }
    }

    /// The width the Canton view's columns wrap at, in layout units, as the explorer picks it.
    fn canton_wrap(&self) -> f32 {
        (self.canvas_size.width * 1.35).max(1000.0).round()
    }

    /// Lays out the Canton view with as many lines as suit the canvas's
    /// shape: first on one line to measure it, then wrapped so the whole
    /// history fills the canvas instead of a narrow strip.
    fn canton_layout_fitted(&mut self) {
        let filters = self.canton_filters.clone();
        self.graph.canton_layout(&filters, f32::MAX);
        let corner = Vector::new(canton::NODE_W, canton::NODE_H);
        let (width, height) = self
            .graph
            .nodes
            .iter()
            .filter(|n| n.card.shown)
            .fold((0.0_f32, 0.0_f32), |(w, h), n| (w.max(n.pos.x + corner.x), h.max(n.pos.y + corner.y)));
        let aspect = (self.canvas_size.width - 40.0).max(100.0) / (self.canvas_size.height - 20.0).max(100.0);
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        // Lines of width W/r stacked r high match the canvas when W / (r² · H) = aspect.
        let lines = (width / (height * aspect)).sqrt().max(1.0);
        let wrap = (width / lines + canton::NODE_W).max(self.canton_wrap().min(width + 1.0));
        self.graph.canton_layout(&filters, wrap);
    }

    /// Zooms and pans so every shown record is in view. The Canton view puts
    /// the layout's top-left corner at the canvas's, never zoomed past 1; the
    /// standard view just recentres, since its layout is still spreading out.
    fn fit_view(&mut self) {
        let (w, h) = (self.canvas_size.width, self.canvas_size.height);
        if self.canton() {
            let corner = Vector::new(canton::NODE_W, canton::NODE_H) * 0.5;
            let extent = self
                .graph
                .nodes
                .iter()
                .filter(|n| n.card.shown)
                .fold(Vector::new(canton::NODE_W, canton::NODE_H), |m, n| {
                    let far = n.pos + corner;
                    Vector::new(m.x.max(far.x), m.y.max(far.y))
                });
            let (lw, lh) = (extent.x, extent.y + 56.0);
            let k = ((w - 40.0) / lw).min((h - 20.0) / lh).clamp(0.15, 1.3);
            self.camera = Camera { offset: Vector::new(20.0 - w / 2.0, 10.0 - h / 2.0), scale: k };
            return;
        }
        self.camera = self.standard_fit();
    }

    /// The camera that fits every visible record in the canvas, with room
    /// for labels, never zoomed in past 1.6.
    fn standard_fit(&self) -> Camera {
        let Some((min, max)) = self.graph.bounds() else { return Camera::default() };
        let size = max - min;
        let (w, h) = (self.canvas_size.width, self.canvas_size.height);
        const MARGIN: f32 = 90.0;
        let scale = ((w - 2.0 * MARGIN) / size.x.max(1.0)).min((h - 2.0 * MARGIN) / size.y.max(1.0)).clamp(0.2, 1.6);
        Camera { offset: (min + max) * (-0.5 * scale), scale }
    }

    /// While the standard layout spreads out after a load, eases the camera
    /// toward fitting it, until the user takes over by panning, zooming or
    /// dragging.
    fn follow_layout(&mut self) {
        if !self.following || self.canton() {
            return;
        }
        let goal = self.standard_fit();
        if !self.layout.is_active() {
            self.camera = goal;
            self.following = false;
            return;
        }
        // Ease in small graphs; big ones redraw too slowly for easing to look smooth.
        let k = if self.graph.nodes.len() > 200 { 1.0 } else { 0.12 };
        self.camera = Camera {
            offset: self.camera.offset + (goal.offset - self.camera.offset) * k,
            scale: self.camera.scale + (goal.scale - self.camera.scale) * k,
        };
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
    /// Adds a record's neighbours to the graph, placed around it.
    fn add_neighbours(&mut self, id: &str, result: ResultGraph) {
        let Some(centre) = self.graph.nodes.iter().find(|n| n.id == id).map(|n| n.pos) else { return };
        let mut data = self.graph.to_data();
        let have: HashSet<String> = data.nodes.iter().map(|n| n.id.clone()).collect();
        let have_edges: HashSet<String> = data.edges.iter().map(|e| e.id.clone()).collect();
        let new_nodes: Vec<_> = result.data.nodes.into_iter().filter(|n| !have.contains(&n.id)).collect();
        let new_edges: Vec<_> = result.data.edges.into_iter().filter(|e| !have_edges.contains(&e.id)).collect();
        let (added, links) = (new_nodes.len(), new_edges.len());
        let new_ids: Vec<String> = new_nodes.iter().map(|n| n.id.clone()).collect();
        data.nodes.extend(new_nodes);
        data.edges.extend(new_edges);
        self.graph = Graph::new(data, Some(&self.graph));
        // New records start on a small circle around the one they came from.
        for (k, nid) in new_ids.iter().enumerate() {
            if let Some(n) = self.graph.nodes.iter_mut().find(|n| &n.id == nid) {
                let angle = std::f32::consts::TAU * k as f32 / added.max(1) as f32;
                n.pos = centre + Vector::new(angle.cos(), angle.sin()) * 60.0;
            }
        }
        self.selection = self.graph.nodes.iter().position(|n| n.id == id).map(Selection::Node);
        self.result_view = Some(ResultView);
        self.notice = Some(match (added, links) {
            (0, 0) => format!("{id} has no other relations"),
            (a, l) => format!("Added {} and {} around {id}", plural(a, "record"), plural(l, "relation")),
        });
        if self.canton() {
            self.relayout_canton();
        } else {
            self.layout.reheat(0.4);
        }
    }

    fn apply_result(&mut self, result: ResultGraph) {
        if result.data.nodes.is_empty() {
            self.notice = Some("None of the returned records could be drawn.".into());
            return;
        }
        let limit = self.config.graph.max_result_records;
        self.overview = false;
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
        self.following = true;
        self.presettle();
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
            known_size: self.canvas_size,
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
        let header = container(
            row![text("●").color(n.color), text(if self.overview { &n.table } else { &n.id }).font(Font::MONOSPACE).size(13)]
                .spacing(8),
        )
        .padding([6, 10]);
        let items = if self.overview {
            column![header, rule::horizontal(1), item("Show records", Message::MenuShowRecords), item("Inspect", Message::MenuInspect)]
        } else {
            column![
                header,
                rule::horizontal(1),
                item("Show neighbours", Message::MenuNeighbours),
                item("Inspect", Message::MenuInspect),
                item("Hide record", Message::MenuHide),
                item("Focus record", Message::MenuFocus),
                item("Copy record id", Message::MenuCopy),
            ]
        };
        container(items)
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

/// An offset box's value: empty clears it, digits set it, and anything else
/// keeps what was there, so a stray key does not drop the filter.
fn parse_offset(text: &str, old: Option<i64>) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        text.parse().ok().or(old)
    }
}
