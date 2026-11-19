//! Window state and message handling. The views are in `sidebar.rs`,
//! `settings.rs` and `query_panel.rs`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use iced::widget::{button, canvas, column, container, pin, row, rule, scrollable, stack, text, text_editor};
use iced::{Color, Element, Fill, Font, Point, Subscription, Task, Theme, Vector, window};
use surrealdb_explorer::canton::{Filters, NodeKind, StatusFilter};
use surrealdb_explorer::config::{Config, UserLevel, Visualization};
use surrealdb_explorer::db::{self, Database};
use surrealdb_explorer::graph::{self, GraphData, ResultGraph};
use surrealdb_explorer::config::DatabaseConfig;
use surrealdb_explorer::data;
use surrealdb_explorer::logging::LogControl;
use surrealdb_explorer::mcp::{self, WindowCommand};
use tokio::sync::{mpsc, oneshot};
use surrealdb_explorer::query::{self, QueryOutcome};
use surrealdb_explorer::saved_queries::SavedQueries;
use tracing::{debug, error, info, warn};

use crate::gui::canvas::{CANTON_MARGIN, Camera, GraphView, SCROLLBAR_WIDTH};
use crate::gui::sidebar::SIDEBAR_SCROLL;
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
    /// The main window has opened: macOS has finished launching the app, so
    /// the Dock icon can be set now (set earlier, AppKit resets it).
    WindowOpened,
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
    /// The text typed in the "latest n transactions" box.
    CantonCount(String),
    /// The Canton view's "columns side by side" box, as typed.
    CantonColumns(String),
    /// Scrolls the Canton view down by this many pixels (up when negative).
    CantonScroll(f32),
    /// Scrolls the Canton view to this many pixels from the top.
    CantonScrollTo(f32),
    /// Turns zooming to fit after each filter change on or off (and remembers it).
    ToggleFitOnFilter(bool),
    /// Turns following the newest transactions in the Canton view on or off (and remembers it).
    ToggleFollowNewest(bool),
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
    /// A fingerprint of what the view shows, to compare with the last one.
    Checked(Result<serde_json::Value, String>),
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
    UserLevelChanged(UserLevel),
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
    /// Puts the saved query of this name in the editor.
    PickSavedQuery(String),
    /// The name typed for saving the query.
    SavedQueryName(String),
    /// Saves the editor's query under the typed name.
    SaveQuery,
    /// Asks whether to delete the picked saved query.
    AskDeleteSavedQuery,
    /// Deletes it (true) or keeps it (false).
    ConfirmDeleteSavedQuery(bool),

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
    /// Runs the database queries, off iced's own threads.
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
    /// Named queries from `queries.toml`.
    pub(super) saved_queries: SavedQueries,
    /// The saved query last picked or saved, if it still exists.
    pub(super) saved_query: Option<String>,
    /// The name typed for saving the query.
    pub(super) saved_query_name: String,
    /// Waiting for the user to confirm deleting `saved_query`.
    pub(super) confirm_delete_query: bool,
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
    /// The database's fingerprint when the graph was last loaded, for auto
    /// refresh; `None` until the first check after a load.
    change_marker: Option<serde_json::Value>,
    /// Auto refresh is checking the database.
    checking: bool,
    /// The camera follows the standard layout while it settles.
    following: bool,
    /// The graph shows the database's tables, not its records.
    pub(super) overview: bool,
    /// Which transactions the Canton view loads, and the offsets it got.
    pub(super) canton_window: graph::CantonWindow,
    pub(super) canton_offsets: Option<(i64, i64)>,
    /// Records of the loaded window left out because of `max_result_records`.
    pub(super) canton_left_out: usize,
    /// The "latest n transactions" box as typed; also the step for Older / Newer.
    pub(super) canton_count: String,
    /// The "columns side by side" box as typed.
    pub(super) canton_columns: String,
    /// How far the Canton view is scrolled down, in pixels.
    pub(super) canton_scroll: f32,
}

impl App {
    /// How many transactions "Latest", "Older" and "Newer" load.
    pub(super) fn canton_n(&self) -> usize {
        self.canton_count.parse().unwrap_or(self.config.graph.canton_transactions)
    }

    /// `startup_error` is a problem found before the window opened (such as
    /// an invalid config file), shown in the sidebar.
    pub fn boot(config: Config, startup_error: Option<String>, log: Option<LogControl>) -> App {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start the async runtime");

        let limit = config.graph.max_records_per_table;
        let config_canton_transactions = config.graph.canton_transactions;
        let config_canton_columns = config.graph.canton_columns;
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

        let (saved_queries, queries_error) = match SavedQueries::load_or_create_at(&SavedQueries::path()) {
            Ok(q) => (q, None),
            Err(e) => {
                warn!(error = %e, "could not read the saved queries");
                (SavedQueries { queries: Vec::new() }, Some(format!("Could not read the saved queries: {e}")))
            }
        };

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
            error: startup_error.or(error).or(queries_error),
            notice: None,
            settings: None,
            query_open: false,
            query_text: text_editor::Content::with_text("SELECT * FROM person;"),
            query_running: false,
            query_outcome: None,
            saved_queries,
            saved_query: None,
            saved_query_name: String::new(),
            confirm_delete_query: false,
            result_view: None,
            tab: Tab::Graph,
            pending_data_action: None,
            data_busy: false,
            canvas_size: iced::Size::new(880.0, 760.0),
            canton_filters: Filters::default(),
            change_marker: None,
            checking: false,
            following: true,
            overview: false,
            canton_window: graph::CantonWindow::Latest(config_canton_transactions),
            canton_offsets: None,
            canton_left_out: 0,
            canton_count: config_canton_transactions.to_string(),
            canton_columns: config_canton_columns.to_string(),
            canton_scroll: 0.0,
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
        let opened = window::open_events().map(|_| Message::WindowOpened);
        Subscription::batch([frames, refresh, opened, Subscription::run(mcp_commands)])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.sync_mcp();
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::WindowOpened => {
                crate::gui::icon::set_dock_icon();
            }
            Message::Tick => {
                self.layout.step(&mut self.graph, self.dragging);
                self.follow_layout();
            }
            Message::Hover(hover) => self.hover = hover,
            Message::Select(selection) => {
                // The canvas may still point at a record of a graph that was
                // replaced meanwhile (by auto refresh or Claude Code).
                self.selection = selection.filter(|s| match *s {
                    Selection::Node(n) => n < self.graph.nodes.len(),
                    Selection::Edge(e) => e < self.graph.edges.len(),
                });
                self.menu = None;
                // The details are at the end of the sidebar: show them.
                if self.selection.is_some() {
                    return iced::widget::operation::snap_to_end(SIDEBAR_SCROLL);
                }
            }
            Message::DragNode(node, pos) => {
                let Some(n) = self.graph.nodes.get_mut(node) else { return Task::none() };
                n.pos = pos;
                self.following = false;
                self.dragging = Some(node);
                self.layout.reheat(0.3);
            }
            Message::DragEnd => self.dragging = None,
            Message::Pan(delta) if self.canton() => self.scroll_canton(self.canton_scroll - delta.y),
            Message::Pan(delta) => {
                self.following = false;
                self.camera.offset = self.camera.offset + delta;
            }
            // The Canton view always fills the canvas's width, so it scrolls rather than zooms.
            Message::Zoom { .. } if self.canton() => {}
            Message::Zoom { factor, anchor, from_centre } => {
                self.following = false;
                let scale = (self.camera.scale * factor).clamp(0.03, 5.0);
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
                self.change_marker = None;
                if let Err(e) = self.config.save() {
                    warn!(error = %e, "could not save the setting");
                }
            }
            Message::CheckForChanges => return self.check_for_changes(),
            Message::Checked(result) => {
                self.checking = false;
                match result {
                    // Only act on the answer if auto refresh is still wanted:
                    // a query result may have been drawn meanwhile.
                    Ok(marker) if self.auto_refresh_running() => match &self.change_marker {
                        Some(previous) if *previous != marker => {
                            debug!("the database changed; refreshing the graph");
                            self.refresh();
                            self.change_marker = Some(marker);
                        }
                        Some(_) => {}
                        None => self.change_marker = Some(marker),
                    },
                    Ok(_) => {}
                    Err(e) => debug!(error = %e, "auto refresh could not check the database"),
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
            Message::ToggleFollowNewest(on) => {
                self.config.graph.canton_follow_newest = on;
                if let Err(e) = self.config.save() {
                    warn!(error = %e, "could not save the setting");
                }
                if on && self.canton() {
                    self.scroll_canton(f32::MAX);
                }
            }
            Message::ToggleFitOnFilter(on) => {
                self.config.graph.fit_on_filter = on;
                if let Err(e) = self.config.save() {
                    warn!(error = %e, "could not save the setting");
                }
                if on && self.canton() {
                    self.fit_view();
                }
            }
            Message::CantonCount(text) => {
                if text.trim().is_empty() || text.trim().parse::<usize>().is_ok_and(|n| n > 0) {
                    self.canton_count = text.trim().to_string();
                }
            }
            Message::CantonColumns(text) => {
                let text = text.trim().to_string();
                match text.parse::<usize>() {
                    Ok(n) if (1..=100).contains(&n) => {
                        self.canton_columns = text;
                        if n != self.config.graph.canton_columns {
                            self.config.graph.canton_columns = n;
                            if let Err(e) = self.config.save() {
                                warn!(error = %e, "could not save the setting");
                            }
                            self.relayout_canton();
                        }
                    }
                    _ if text.is_empty() => self.canton_columns = text,
                    _ => {}
                }
            }
            Message::CantonScroll(delta) => self.scroll_canton(self.canton_scroll + delta),
            Message::CantonScrollTo(to) => self.scroll_canton(to),
            Message::CantonPage(window) => {
                let previous = std::mem::replace(&mut self.canton_window, window);
                let loaded_before = self.canton_offsets.is_some();
                self.load_everything();
                // Paging past either end would leave an empty view with both
                // paging buttons off: stay on the transactions shown instead.
                let paging = matches!(window, graph::CantonWindow::Before(..) | graph::CantonWindow::After(..));
                if paging && loaded_before && self.canton_offsets.is_none() && self.error.is_none() {
                    self.canton_window = previous;
                    self.load_everything();
                    self.notice = Some(match window {
                        graph::CantonWindow::Before(..) => "There are no older transactions.".into(),
                        _ => "There are no newer transactions yet.".into(),
                    });
                }
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
                // Filtering re-wraps the records that are left; it then zooms
                // so they fill the view, unless that's turned off (then the
                // view stays where it is, as in the explorer).
                if self.canton() {
                    self.canton_layout_fitted();
                    if self.config.graph.fit_on_filter {
                        self.fit_view();
                    }
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
            Message::UserLevelChanged(level) => {
                if let Some(form) = &mut self.settings {
                    form.user_level = level;
                    form.message = None;
                }
            }
            Message::SaveSettings => self.save_settings(),

            Message::ToggleQuery => self.query_open = !self.query_open,
            Message::QueryEdited(action) => self.query_text.perform(action),
            Message::RunQuery => return self.run_query(),
            Message::ShowTab(tab) => self.tab = tab,
            Message::PickSavedQuery(name) => {
                if let Some(saved) = self.saved_queries.get(&name) {
                    self.query_text = text_editor::Content::with_text(&saved.query);
                    self.saved_query_name = name.clone();
                    self.saved_query = Some(name);
                    self.confirm_delete_query = false;
                }
            }
            Message::SavedQueryName(name) => self.saved_query_name = name,
            Message::SaveQuery => self.save_query(),
            Message::AskDeleteSavedQuery => self.confirm_delete_query = self.saved_query.is_some(),
            Message::ConfirmDeleteSavedQuery(delete) => {
                self.confirm_delete_query = false;
                if delete {
                    self.delete_saved_query();
                }
            }
            Message::QueryDone(outcome, changes_data) => {
                self.query_running = false;
                let has_records = !query::record_ids(&outcome).is_empty();
                // A result without records (counts, fields, …) has nothing to
                // draw, so show it as the table.
                if !has_records && outcome.statements.is_ok() && !changes_data {
                    self.tab = Tab::Table;
                }
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
                    WindowCommand::Select { id, reply } if id.is_empty() => {
                        self.selection = None;
                        let _ = reply.send(Ok(serde_json::json!({ "selected": null })));
                    }
                    WindowCommand::Select { id, reply } => {
                        let Some(i) = self.graph.nodes.iter().position(|n| n.id == id) else {
                            let _ = reply.send(Err(format!("no record {id} is drawn in the window")));
                            return Task::none();
                        };
                        info!(record = %id, "selecting a record for Claude Code");
                        self.selection = Some(Selection::Node(i));
                        self.menu = None;
                        self.bring_into_view(i);
                        let _ = reply.send(Ok(self.card_on_screen(i)));
                        return iced::widget::operation::snap_to_end(SIDEBAR_SCROLL);
                    }
                    WindowCommand::Scroll { to, reply } => {
                        let answer = match to.as_str() {
                            "top" if self.canton() => {
                                self.scroll_canton(0.0);
                                Ok(serde_json::json!({ "scroll": self.canton_scroll }))
                            }
                            "bottom" if self.canton() => {
                                self.scroll_canton(f32::MAX);
                                Ok(serde_json::json!({ "scroll": self.canton_scroll }))
                            }
                            id => match self.graph.nodes.iter().position(|n| n.id == id) {
                                Some(i) => {
                                    self.bring_into_view(i);
                                    Ok(self.card_on_screen(i))
                                }
                                None => Err(format!("no record {id} is drawn in the window (or top/bottom outside the Canton view)")),
                            },
                        };
                        let _ = reply.send(answer);
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
                        // Switch to the Canton view first, so the reload reads
                        // a window of transactions rather than every table.
                        if action == DataAction::LoadCantonSample && !self.canton() {
                            self.config.graph.visualization = Visualization::Canton;
                            if let Err(e) = self.config.save() {
                                warn!(error = %e, "could not save the setting");
                            }
                            self.apply_visualization();
                        }
                        self.reload();
                        self.relayout_canton();
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
        if self.canton() {
            // The Canton view draws what its layout placed, so place the cards again.
            self.canton_layout_fitted();
        } else {
            self.layout.reheat(0.2);
        }
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
            self.canton_left_out = loaded.left_out;
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
        self.change_marker = None;
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

    /// Reads a fingerprint of what the view shows in the background, for
    /// auto refresh (see `graph::change_marker`).
    fn check_for_changes(&mut self) -> Task<Message> {
        let Some(db) = self.db.clone() else { return Task::none() };
        // Skip a check while the user is moving things or one is still running.
        if self.checking || self.dragging.is_some() || self.menu.is_some() || self.data_busy {
            return Task::none();
        }
        self.checking = true;
        let watch = if self.canton() {
            graph::Watch::Canton
        } else {
            graph::Watch::Tables { per_table: self.config.graph.max_records_per_table }
        };
        let handle = self.runtime.spawn(async move {
            graph::change_marker(&db, watch).await.map_err(|e| e.to_string())
        });
        Task::perform(
            async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
            Message::Checked,
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
        let added = self.graph.nodes.len() as i64 - before as i64;
        if self.canton() {
            self.canton_layout_fitted();
            if added > 0 && self.config.graph.canton_follow_newest {
                self.scroll_canton(f32::MAX);
            }
        } else {
            self.layout.reheat(0.3);
        }
        self.notice = Some(match added {
            0 => "Updated".into(),
            n if n > 0 => format!("Updated: {} new", plural(n as usize, "record")),
            n => format!("Updated: {} removed", plural((-n) as usize, "record")),
        });
    }

    pub(super) fn canton(&self) -> bool {
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

    /// Lays out the Canton view: the chosen number of transactions side by
    /// side, stretched to fill the canvas's width, keeping the scroll
    /// position where it can.
    fn canton_layout_fitted(&mut self) {
        let filters = self.canton_filters.clone();
        let columns = self.config.graph.canton_columns.max(1);
        self.graph.canton_layout(&filters, columns, self.canton_fill_width());
        self.scroll_canton(self.canton_scroll);
    }

    /// The canvas width the Canton layout fills, in pixels: all of it but the
    /// margins and the scrollbar.
    fn canton_fill_width(&self) -> f32 {
        (self.canvas_size.width - 2.0 * CANTON_MARGIN.0 - SCROLLBAR_WIDTH).max(200.0)
    }

    /// The Canton view's zoom: 1, unless the columns don't fit the canvas
    /// at that size; then smaller, so they still do.
    fn canton_scale(&self) -> f32 {
        let width = self.graph.canton_extent.x;
        if width <= 0.0 { 1.0 } else { (self.canton_fill_width() / width).clamp(0.02, 1.0) }
    }

    /// How tall the Canton view's content is, in pixels.
    pub(super) fn canton_content_height(&self) -> f32 {
        self.graph.canton_extent.y * self.canton_scale() + 2.0 * CANTON_MARGIN.1
    }

    /// Scrolls the Canton view to `to` pixels from the top (kept within the
    /// content) and points the camera there.
    fn scroll_canton(&mut self, to: f32) {
        let (w, h) = (self.canvas_size.width, self.canvas_size.height);
        let max = (self.canton_content_height() - h).max(0.0);
        self.canton_scroll = to.clamp(0.0, max);
        self.camera = Camera {
            offset: Vector::new(CANTON_MARGIN.0 - w / 2.0, CANTON_MARGIN.1 - self.canton_scroll - h / 2.0),
            scale: self.canton_scale(),
        };
    }

    /// Moves the view so record `i`'s card is on screen: in the Canton view
    /// it scrolls (only if the card isn't fully shown, putting it a third of
    /// the way down); in the standard view the camera centres on it.
    fn bring_into_view(&mut self, i: usize) {
        let pos = self.graph.nodes[i].pos;
        if self.canton() {
            let scale = self.canton_scale();
            let h = self.canvas_size.height;
            let top = CANTON_MARGIN.1 + pos.y * scale - self.canton_scroll;
            let half = 40.0 * scale;
            if top - half < 0.0 || top + half > h {
                self.scroll_canton(CANTON_MARGIN.1 + pos.y * scale - h / 3.0);
            }
        } else {
            self.following = false;
            self.camera.offset = pos * (-self.camera.scale);
        }
    }

    /// Where record `i`'s card centre is drawn, in canvas pixels from the
    /// canvas's top-left corner, with the canvas size.
    fn card_on_screen(&self, i: usize) -> serde_json::Value {
        let bounds = iced::Rectangle::new(Point::ORIGIN, self.canvas_size);
        let p = self.camera.to_screen(bounds, self.graph.nodes[i].pos);
        serde_json::json!({
            "id": self.graph.nodes[i].id,
            "x": p.x, "y": p.y,
            "canvas_width": self.canvas_size.width, "canvas_height": self.canvas_size.height,
            "scroll": self.canton_scroll,
        })
    }

    /// Zooms and pans so every shown record is in view. The Canton view
    /// always fills the canvas's width, so it just goes back to the top; the
    /// standard view recentres, since its layout is still spreading out.
    fn fit_view(&mut self) {
        if self.canton() {
            self.scroll_canton(0.0);
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
        let mut new = match form.to_config() {
            Ok(config) => config,
            Err(e) => {
                form.message = Some(Err(e));
                return;
            }
        };
        // Settings the form doesn't edit may have changed while it was open
        // (the query panel's "Connect shown records" stays usable): keep them.
        let live = &self.config;
        new.graph.connect_results = live.graph.connect_results;
        new.graph.auto_refresh = live.graph.auto_refresh;
        new.graph.fit_on_filter = live.graph.fit_on_filter;
        new.graph.canton_columns = live.graph.canton_columns;
        new.graph.canton_follow_newest = live.graph.canton_follow_newest;
        new.graph.visualization = live.graph.visualization;
        new.mcp = live.mcp.clone();
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
        if let graph::CantonWindow::Latest(_) = self.canton_window {
            self.canton_window = graph::CantonWindow::Latest(new.graph.canton_transactions);
        }
        if new.graph.canton_transactions != old.graph.canton_transactions {
            self.canton_count = new.graph.canton_transactions.to_string();
        }
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
    /// A different server (or user) is connected to first, and the old
    /// connection is closed only if that succeeds.
    fn switch_database(&mut self, config: &DatabaseConfig) -> Result<(), String> {
        // The same server and user keep the connection and only switch
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
        // A write Claude Code asked for was meant for the previous database.
        self.answer_write(false);
        self.query_outcome = None;
        self.result_view = None;
        self.pending_data_action = None;
        Ok(())
    }

    /// Saves the editor's query under the typed name (replacing a saved
    /// query of that name) and writes `queries.toml`.
    fn save_query(&mut self) {
        let name = self.saved_query_name.trim().to_string();
        match self.saved_queries.put(&name, &self.query_text.text()) {
            Ok(replaced) => {
                if let Err(e) = self.saved_queries.save_to(&SavedQueries::path()) {
                    warn!(error = %e, "could not save the queries");
                    self.error = Some(format!("Could not save the queries: {e}"));
                    return;
                }
                info!(query = %name, replaced, "saved a query");
                self.notice = Some(format!("{} \"{name}\"", if replaced { "Updated" } else { "Saved" }));
                self.saved_query_name = name.clone();
                self.saved_query = Some(name);
                self.confirm_delete_query = false;
            }
            Err(e) => self.notice = Some(format!("Not saved: {e}")),
        }
    }

    fn delete_saved_query(&mut self) {
        let Some(name) = self.saved_query.take() else { return };
        if !self.saved_queries.remove(&name) {
            return;
        }
        if let Err(e) = self.saved_queries.save_to(&SavedQueries::path()) {
            warn!(error = %e, "could not save the queries");
            self.error = Some(format!("Could not save the queries: {e}"));
            return;
        }
        info!(query = %name, "deleted a saved query");
        self.notice = Some(format!("Deleted \"{name}\""));
    }

    fn run_query(&mut self) -> Task<Message> {
        let sql = self.query_text.text();
        let Some(db) = self.db.clone() else { return Task::none() };
        if sql.trim().is_empty() || self.query_running {
            return Task::none();
        }
        self.query_running = true;
        let changes_data = !query::is_read_only(&sql);
        // Run on our runtime, and only wait for the result on iced's side, so the window stays responsive.
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
            let Some(reply) = reply.lock().ok().and_then(|mut r| r.take()) else { return };
            // Claude Code stops waiting after a while; then the query didn't run.
            if reply.send(allow).is_err() && allow {
                self.notice = Some("Claude Code had stopped waiting, so the query didn't run. Ask it again.".into());
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
            scroll: self.canton().then(|| (self.canton_scroll, self.canton_content_height())),
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

/// Asks the user to allow a query from Claude Code that changes data. The
/// whole query is shown (scrolling when long): the user allows only what
/// they can read.
fn write_banner(query: &str) -> Element<'_, Message> {
    container(
        column![
            text("Claude Code wants to run a query that changes data:").size(14).font(Font {
                weight: iced::font::Weight::Bold,
                ..Font::default()
            }),
            container(scrollable(container(text(query).size(12).font(Font::MONOSPACE)).padding(6).width(Fill)))
                .max_height(260)
                .width(Fill)
                .style(|_| container::background(Color::from_rgb(0.95, 0.95, 0.95))),
            row![
                button(text("Allow").size(13)).style(button::danger).on_press(Message::AnswerWrite(true)),
                button(text("Don't allow").size(13)).style(button::secondary).on_press(Message::AnswerWrite(false)),
                text(format!("{} lines", query.lines().count())).size(12).color(Color::from_rgb(0.47, 0.47, 0.47)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
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
