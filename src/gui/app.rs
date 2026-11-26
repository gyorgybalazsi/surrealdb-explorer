//! Window state and message handling. The views are in `sidebar.rs`,
//! `settings.rs` and `query_panel.rs`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use iced::widget::{button, canvas, column, container, pin, row, rule, scrollable, stack, text, text_editor};
use iced::{Color, Element, Fill, Font, Point, Subscription, Task, Theme, Vector, window};
use crate::canton::{Filters, NodeKind, StatusFilter, ellipsize};
use crate::config::{Config, UserLevel, Visualization};
use crate::db::{self, Database, Queries};
use crate::graph::{self, GraphData, ResultGraph};
use crate::config::DatabaseConfig;
use crate::data;
use crate::logging::LogControl;
use crate::mcp::{self, WindowCommand};
use tokio::sync::{mpsc, oneshot};
use crate::query::{self, QueryOutcome};
use crate::saved_queries::SavedQueries;
use tracing::{debug, error, info, warn};

use crate::gui::canvas::{CANTON_MARGIN, Camera, GraphView, SCROLLBAR_WIDTH};
use crate::gui::sidebar::SIDEBAR_SCROLL;
use crate::gui::layout::Layout;
use crate::gui::model::Graph;
use crate::gui::settings::{SettingsField, SettingsForm};
use crate::gui::Files;

/// A query's result is shown as the graph (instead of the overview).
#[derive(Debug, Clone, Copy, Default)]
pub struct ResultView;

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

/// What a background load of the graph is for, which decides how its
/// result is shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadPurpose {
    /// Show what was read in place of the graph, with `notice` in the
    /// sidebar; `fit` also fits it in view.
    Reload { notice: Option<&'static str>, fit: bool },
    /// Auto refresh saw a change: keep the camera, the selection and the
    /// records the user hid.
    Refresh,
    /// Another window of transactions in the Canton view.
    Page(graph::CantonWindow),
}

/// What a background load reads.
#[derive(Debug, Clone, Copy)]
enum LoadWhat {
    /// The tables, as a graph.
    Overview,
    /// The records: in the Canton view the window of transactions, otherwise
    /// every table's records up to the per-table limit.
    Records,
    /// The records, or the overview when there are more than this many.
    RecordsOrOverview(usize),
}

/// What a background load read from the database.
#[derive(Debug, Clone)]
pub enum Loaded {
    Overview(GraphData),
    /// `offsets` and `left_out` are only known in the Canton view. `marker`
    /// is auto refresh's fingerprint of the database, read just before the
    /// records (`None` if auto refresh is off or it couldn't be read).
    Records { data: GraphData, offsets: Option<(i64, i64)>, left_out: usize, marker: Option<serde_json::Value> },
}

/// A database opened in the background after the settings changed.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The new connection, or `None` when the current one only switched
    /// namespace and database.
    db: Option<Database>,
    queries: Queries,
}

/// What saving the settings found so far, finished once the new database
/// is open.
#[derive(Debug, Clone, Default)]
pub struct SettingsReport {
    problems: Vec<String>,
    notes: Vec<&'static str>,
    /// The graph settings changed, so the graph is read again even if the
    /// new database can't be opened.
    graph_changed: bool,
}

/// What a record's menu asked to add around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Related {
    /// The transaction it is in, with every event in it.
    Transaction,
    /// The event that archived the contract, with its transaction.
    ArchivingEvent,
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
    /// The memory watch's warning changed (`None`: it cleared).
    MemoryAlarm(Option<String>),
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
    /// From a record's menu: shows every hidden record again.
    MenuUnhideAll,
    /// From a record's menu: deselects the focused record and fits the view again.
    MenuClearFocus,
    /// From a record's menu: copies the contract id a Canton record is or acts on.
    MenuCopyContractId,
    /// From a record's menu: copies the record's fields as JSON.
    MenuCopyJson,
    /// From a record's menu: runs `SELECT * FROM <record>` in the query panel,
    /// keeping the graph as it is.
    MenuShowInTable,
    /// From a record's menu: filters the Canton view to this template (`None`: any).
    MenuFilterTemplate(Option<String>),
    /// From a record's menu: filters the Canton view to this party (`None`: any).
    MenuFilterParty(Option<String>),
    /// From a record's menu: adds the transaction a Canton record is in, with its events.
    MenuTransaction,
    /// From a contract's menu: adds the event that archived it, with its transaction.
    MenuArchivingEvent,
    /// From a Canton record's menu: draws its contract's lifecycle.
    MenuLifecycle,
    /// Records related to a record in the graph, read from the database: what
    /// was asked for, that record, and the record to select with what to add
    /// (`None`: nothing was found).
    RelatedLoaded(Related, String, Result<Option<(String, ResultGraph)>, String>),
    /// A contract's lifecycle, asked for from this record (`None`: no contract found).
    LifecycleLoaded(String, Result<Option<graph::Lifecycle>, String>),
    /// Opens the background menu where the canvas was right-clicked, off any record.
    CanvasMenu(Point),
    /// Copies a picture of the whole graph to the clipboard (also ⌘⇧C / Ctrl+Shift+C).
    CopyImage,
    /// Saves a picture of the whole graph as a PNG in the Downloads folder.
    SaveImage,
    /// A picture was copied or saved: what to tell the user, or what went wrong.
    ImageDone(Result<String, String>),
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
    /// A background load of the graph has finished. Only the latest load
    /// (by its number) is shown.
    GraphLoaded(u64, LoadPurpose, Result<Loaded, String>),
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
    /// Carries the number of the latest load when the check started: a
    /// load that finished since then has a newer marker.
    Checked(u64, Result<serde_json::Value, String>),
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
    ViewerUserLevelChanged(UserLevel),
    SaveSettings,
    /// The database the saved settings name has been opened (or not).
    DatabaseOpened(DatabaseConfig, Result<Opened, String>, SettingsReport),

    // Query panel
    ToggleQuery,
    QueryEdited(text_editor::Action),
    /// Empties the query editor.
    ClearQuery,
    RunQuery,
    /// Shows the analytics dashboard (true) or the graph (false).
    #[cfg(feature = "dashboard")]
    ShowDashboard(bool),
    /// Anything in the dashboard.
    #[cfg(feature = "dashboard")]
    Dashboard(crate::gui::dashboard::Msg),
    /// The query's outcome.
    QueryDone(QueryOutcome),
    /// The last query's result, read from the database as a graph.
    ResultLoaded(Result<ResultGraph, String>),
    /// Turns drawing every relation between a result's records on or off.
    ToggleConnectResults(bool),
    /// The handle above the query panel was pressed: dragging starts.
    PanelDragStart,
    /// The pointer moved to this height in the window while dragging.
    PanelDrag(f32),
    PanelDragEnd,
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
    /// Where the queries typed in the query panel run: as the read-only
    /// viewer user, if one is set.
    pub(super) queries: Queries,
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
    /// The record "Focus record" zoomed to, until the focus is cleared or
    /// another record is selected.
    pub(super) focused: Option<String>,
    /// Open background menu: where the canvas was right-clicked.
    pub(super) canvas_menu: Option<Point>,
    pub(super) dragging: Option<usize>,
    pub(super) error: Option<String>,
    pub(super) notice: Option<String>,
    /// The memory watch's warning, shown until it clears.
    pub(super) memory_alarm: Option<String>,

    /// The settings being edited, while the settings view is open.
    pub(super) settings: Option<SettingsForm>,

    pub(super) query_open: bool,
    pub(super) query_text: text_editor::Content,
    pub(super) query_running: bool,
    pub(super) query_outcome: Option<QueryOutcome>,
    /// The last result has record ids, so it can be shown in the graph.
    pub(super) query_drawable: bool,
    /// The query running was started by "Show in table": its result stays
    /// in the panel instead of replacing the graph.
    keep_graph_for_query: bool,
    /// The query that gave `query_outcome`, as run.
    pub(super) query_ran: String,
    /// Height of the open query panel, in pixels. It grows to fit a longer
    /// query, and the handle above it resizes it.
    pub(super) panel_height: f32,
    /// While the handle is dragged: where the pointer was first seen, and the
    /// panel and graph heights together then.
    panel_drag: Option<Option<(f32, f32, f32)>>,
    /// Named queries from `queries.toml`.
    pub(super) saved_queries: SavedQueries,
    /// The saved query last picked or saved, if it still exists.
    pub(super) saved_query: Option<String>,
    /// The name typed for saving the query.
    pub(super) saved_query_name: String,
    /// Waiting for the user to confirm deleting `saved_query`.
    pub(super) confirm_delete_query: bool,
    /// `queries.toml` could not be read, so saving is refused: writing the
    /// list the app has (empty) would wipe the user's queries.
    pub(super) saved_queries_unreadable: bool,
    /// The last query result, shown as the graph.
    pub(super) result_view: Option<ResultView>,
    /// The main area shows the analytics dashboard instead of the graph.
    #[cfg(feature = "dashboard")]
    pub(super) dashboard_open: bool,
    #[cfg(feature = "dashboard")]
    pub(super) dashboard: crate::gui::dashboard::Dashboard,

    /// Where the settings and the saved queries are kept.
    pub(super) files: Files,
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
    /// What to say once the next page of transactions is shown, in place
    /// of "Reloaded" (a dashboard column's day or week).
    page_notice: Option<String>,
    /// The "latest n transactions" box as typed; also the step for Older / Newer.
    pub(super) canton_count: String,
    /// The "columns side by side" box as typed.
    pub(super) canton_columns: String,
    /// How far the Canton view is scrolled down, in pixels.
    pub(super) canton_scroll: f32,
    /// The number of the latest background load of the graph: an older
    /// one's result is dropped when it arrives.
    load_generation: u64,
    /// The latest load hasn't finished yet.
    loading: bool,
    /// The database the saved settings name is being opened.
    pub(super) switching: bool,
}

impl App {
    /// How many transactions "Latest", "Older" and "Newer" load.
    pub(super) fn canton_n(&self) -> usize {
        self.canton_count.parse().unwrap_or(self.config.graph.canton_transactions)
    }

    /// `startup_error` is a problem found before the window opened (such as
    /// an invalid config file), shown in the sidebar.
    /// `files` says where the settings and saved queries are kept.
    pub fn boot(config: Config, files: Files, startup_error: Option<String>, log: Option<LogControl>) -> App {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start the async runtime");

        let config_canton_transactions = config.graph.canton_transactions;
        let config_canton_columns = config.graph.canton_columns;
        #[cfg(feature = "dashboard")]
        let dashboard_settings = config.analytics.clone();
        // Before the window opens, so it waits for the database here: the
        // first graph is read below, once the app is set up.
        // An empty server address means no database, e.g. when the app is
        // only used for the analytics dashboard.
        let (db, connect_error) = if config.database.server.trim().is_empty() {
            info!("no database server set, so the graph stays empty");
            (None, None)
        } else {
            match runtime.block_on(db::connect(&config.database)) {
                Ok(db) => (Some(db), None),
                Err(e) => (None, Some(format!("Could not open the database: {e}"))),
            }
        };

        let queries = runtime.block_on(Queries::connect(&config.database));
        if let Queries::Failed(e) = &queries {
            error!(error = %e, "could not connect the viewer user");
        }

        let mcp = config.mcp.enabled.then(|| start_mcp(&runtime, config.mcp.port));
        if let Some(shared) = &mcp {
            *shared.db.lock().unwrap() = db.clone();
            *shared.queries.lock().unwrap() = queries.clone();
            *shared.limits.lock().unwrap() = (config.graph.max_result_records, config.graph.connect_results);
        }

        let (saved_queries, queries_error) = match SavedQueries::load_or_create_at(&files.queries) {
            Ok(q) => (q, None),
            Err(e) => {
                warn!(error = %e, "could not read the saved queries");
                (SavedQueries { queries: Vec::new() }, Some(format!("Could not read the saved queries: {e}")))
            }
        };

        let mut app = App {
            files,
            mcp,
            pending_write: None,
            synced_selection: None,
            runtime,
            db,
            queries,
            config,
            log,
            graph: Graph::new(GraphData::default(), None),
            layout: Layout::new(),
            camera: Camera::default(),
            hover: None,
            selection: None,
            menu: None,
            focused: None,
            canvas_menu: None,
            dragging: None,
            error: None,
            notice: None,
            memory_alarm: None,
            settings: None,
            query_open: false,
            query_text: text_editor::Content::with_text("SELECT * FROM person;"),
            query_running: false,
            query_outcome: None,
            query_drawable: false,
            keep_graph_for_query: false,
            query_ran: String::new(),
            panel_height: crate::gui::query_panel::PANEL_HEIGHT,
            panel_drag: None,
            saved_queries,
            saved_query: None,
            saved_query_name: String::new(),
            confirm_delete_query: false,
            saved_queries_unreadable: queries_error.is_some(),
            result_view: None,
            #[cfg(feature = "dashboard")]
            dashboard_open: false,
            #[cfg(feature = "dashboard")]
            dashboard: {
                let mut d = crate::gui::dashboard::Dashboard::new(
                    dashboard_settings.location,
                    dashboard_settings.storage_options,
                    dashboard_settings.selected,
                );
                d.check_seconds = dashboard_settings.check_seconds.max(1);
                d
            },
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
            page_notice: None,
            canton_count: config_canton_transactions.to_string(),
            canton_columns: config_canton_columns.to_string(),
            canton_scroll: 0.0,
            load_generation: 0,
            loading: false,
            switching: false,
        };
        app.apply_visualization();
        if let Some(db) = app.db.clone() {
            let what = app.reload_what();
            let loaded = app.runtime.block_on(app.fetch(db, what, app.canton_window));
            app.show_loaded(LoadPurpose::Reload { notice: None, fit: true }, loaded);
        }
        app.error = startup_error.or(connect_error).or(app.error.take()).or(queries_error);
        app
    }

    /// What to do once the window opens: with no database server set, the
    /// app is only there for the dashboard, so it opens on it.
    pub fn first_task(&mut self) -> Task<Message> {
        #[cfg(feature = "dashboard")]
        if self.config.database.server.trim().is_empty() {
            self.dashboard_open = true;
            return self.dashboard.reload().map(Message::Dashboard);
        }
        Task::none()
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
        let panel_drag = if self.panel_drag.is_some() {
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => Some(Message::PanelDrag(position.y)),
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Message::PanelDragEnd)
                }
                _ => None,
            })
        } else {
            Subscription::none()
        };
        // Reload the dashboard when the analytics runner commits.
        #[cfg(feature = "dashboard")]
        let dashboard = if self.dashboard_open {
            iced::time::every(std::time::Duration::from_secs(self.dashboard.check_seconds.max(1)))
                .map(|_| Message::Dashboard(crate::gui::dashboard::Msg::Check))
        } else {
            Subscription::none()
        };
        #[cfg(not(feature = "dashboard"))]
        let dashboard = Subscription::none();
        let memory = Subscription::run(memory_alarms);
        // ⌘⇧C (Ctrl+Shift+C elsewhere) copies a picture of the whole graph.
        let copy_image = iced::keyboard::listen().filter_map(|event| match event {
            iced::keyboard::Event::KeyPressed { key: iced::keyboard::Key::Character(c), modifiers, .. }
                if modifiers.command() && modifiers.shift() && c.eq_ignore_ascii_case("c") =>
            {
                Some(Message::CopyImage)
            }
            _ => None,
        });
        Subscription::batch([
            frames,
            refresh,
            opened,
            panel_drag,
            dashboard,
            memory,
            copy_image,
            Subscription::run(mcp_commands),
        ])
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
            Message::MemoryAlarm(alarm) => self.memory_alarm = alarm,
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
                self.focused = None;
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
            Message::OpenMenu(node, at) => {
                self.menu = Some((node, at));
                self.canvas_menu = None;
            }
            Message::CloseMenu => {
                self.menu = None;
                self.canvas_menu = None;
            }
            Message::CanvasMenu(at) => {
                self.menu = None;
                self.canvas_menu = Some(at);
            }
            Message::CopyImage => return self.export_image(false),
            Message::SaveImage => return self.export_image(true),
            Message::ImageDone(Ok(done)) => self.notice = Some(done),
            Message::ImageDone(Err(e)) => {
                self.notice = None;
                self.error = Some(e);
            }
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
                    self.focused = Some(self.graph.nodes[node].id.clone());
                    if self.canton() {
                        // The Canton view doesn't zoom: it scrolls the card into view.
                        self.bring_into_view(node);
                    } else {
                        self.following = false;
                        self.camera.scale = 1.5;
                        self.camera.offset = self.graph.nodes[node].pos * -1.5;
                    }
                }
            }
            Message::MenuClearFocus => {
                self.menu = None;
                self.focused = None;
                self.selection = None;
                if !self.canton() {
                    self.fit_view();
                }
            }
            Message::MenuUnhideAll => {
                self.menu = None;
                for n in &mut self.graph.nodes {
                    n.hidden = false;
                }
                self.notice = None;
                if self.canton() {
                    self.canton_layout_fitted();
                } else {
                    self.layout.reheat(0.3);
                }
            }
            Message::MenuCopyContractId => {
                if let Some((node, _)) = self.menu.take()
                    && let Some(cid) = contract_id(&self.graph.nodes[node])
                {
                    self.notice = Some(format!("Copied contract id {cid}"));
                    return iced::clipboard::write(cid);
                }
            }
            Message::MenuCopyJson => {
                if let Some((node, _)) = self.menu.take() {
                    let n = &self.graph.nodes[node];
                    let mut data = n.data.clone();
                    // The id as SurrealQL writes it, like the other copies.
                    if let Some(fields) = data.as_object_mut() {
                        fields.insert("id".into(), serde_json::Value::String(n.id.clone()));
                    }
                    let json = serde_json::to_string_pretty(&data).unwrap_or_default();
                    self.notice = Some(format!("Copied {} as JSON", n.id));
                    return iced::clipboard::write(json);
                }
            }
            Message::MenuShowInTable => {
                let Some((node, _)) = self.menu.take() else { return Task::none() };
                if self.query_running {
                    // Its result would take this query's place.
                    self.notice = Some("A query is still running: try again when it's done".into());
                    return Task::none();
                }
                let id = self.graph.nodes[node].id.clone();
                self.query_open = true;
                self.query_text = text_editor::Content::with_text(&format!("SELECT * FROM {id};"));
                self.fit_panel();
                self.keep_graph_for_query = true;
                let task = self.run_query();
                if !self.query_running {
                    self.keep_graph_for_query = false;
                }
                return task;
            }
            Message::MenuFilterTemplate(template) => {
                self.menu = None;
                return self.handle(Message::CantonFilter(CantonFilterChange::Template(template)));
            }
            Message::MenuFilterParty(party) => {
                self.menu = None;
                return self.handle(Message::CantonFilter(CantonFilterChange::Party(party)));
            }
            Message::MenuTransaction => {
                let Some((node, _)) = self.menu.take() else { return Task::none() };
                let id = self.graph.nodes[node].id.clone();
                let limit = self.config.graph.max_result_records;
                self.notice = Some("Loading the transaction…".into());
                let select = id.clone();
                return self.in_background(
                    move |db| async move { graph::transaction_of(&db, &select, limit).await.map(|g| Some((select, g))) },
                    move |result| Message::RelatedLoaded(Related::Transaction, id.clone(), result),
                );
            }
            Message::MenuArchivingEvent => {
                let Some((node, _)) = self.menu.take() else { return Task::none() };
                let id = self.graph.nodes[node].id.clone();
                let limit = self.config.graph.max_result_records;
                self.notice = Some("Looking for the archiving event…".into());
                let contract = id.clone();
                return self.in_background(
                    move |db| async move { graph::archiving_event(&db, &contract, limit).await },
                    move |result| Message::RelatedLoaded(Related::ArchivingEvent, id.clone(), result),
                );
            }
            Message::RelatedLoaded(what, id, Ok(Some((select, result)))) => {
                let Some((added, links)) = self.add_around(&id, result) else {
                    // The record went from the graph meanwhile (a reload).
                    self.notice = None;
                    return Task::none();
                };
                self.select_record(&select);
                self.notice = Some(match (what, added + links) {
                    (Related::Transaction, 0) => "Its transaction is already shown".to_string(),
                    (Related::Transaction, _) => {
                        format!("Added {} and {} of the transaction", plural(added, "record"), plural(links, "relation"))
                    }
                    (Related::ArchivingEvent, _) => format!("{select} archived {id}"),
                });
            }
            Message::RelatedLoaded(_, id, Ok(None)) => {
                self.notice = Some(format!("Nothing archived {id}: the contract is active"));
            }
            Message::RelatedLoaded(_, _, Err(e)) => {
                error!(error = %e, "could not load the related records");
                self.notice = None;
                self.error = Some(format!("Could not load the related records: {e}"));
            }
            Message::MenuLifecycle => {
                let Some((node, _)) = self.menu.take() else { return Task::none() };
                let id = self.graph.nodes[node].id.clone();
                let limit = self.config.graph.max_result_records;
                self.notice = Some("Tracing the contract's lifecycle…".into());
                let from = id.clone();
                return self.in_background(
                    move |db| async move { graph::lifecycle(&db, &from, limit).await },
                    move |result| Message::LifecycleLoaded(id.clone(), result),
                );
            }
            Message::LifecycleLoaded(_, Ok(Some(lifecycle))) => {
                let contract = lifecycle.contract.clone();
                self.apply_result(lifecycle.graph);
                if self.notice.is_none() {
                    self.notice = Some(format!(
                        "Lifecycle of {contract}: {}. Show all goes back.",
                        plural(lifecycle.events, "event")
                    ));
                }
            }
            Message::LifecycleLoaded(id, Ok(None)) => {
                self.notice = Some(format!("The contract of {id} isn't in the database"));
            }
            Message::LifecycleLoaded(_, Err(e)) => {
                error!(error = %e, "could not trace the contract's lifecycle");
                self.notice = None;
                self.error = Some(format!("Could not trace the lifecycle: {e}"));
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
                    return self.reload(LoadPurpose::Reload { notice: None, fit: true });
                } else {
                    for n in &mut self.graph.nodes {
                        n.hidden = false;
                    }
                    self.notice = None;
                    self.layout.reheat(0.3);
                }
                self.relayout_canton();
            }
            Message::Reload => return self.reload(RELOADED),
            Message::ShowOverview => return self.start_load(LoadPurpose::Reload { notice: None, fit: false }, LoadWhat::Overview),
            Message::LoadEverything => return self.start_load(RELOADED, LoadWhat::Records),
            Message::GraphLoaded(generation, purpose, result) => {
                if generation != self.load_generation {
                    debug!(generation, "dropped the result of an older load");
                    return Task::none();
                }
                self.loading = false;
                self.show_loaded(purpose, result);
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
                if let Err(e) = self.config.save_to(&self.files.config) {
                    warn!(error = %e, "could not save the setting");
                }
            }
            Message::CheckForChanges => return self.check_for_changes(),
            Message::Checked(generation, result) => {
                self.checking = false;
                match result {
                    // A load finished meanwhile: its marker is newer than this one.
                    Ok(_) if generation != self.load_generation => {}
                    // Only act on the answer if auto refresh is still wanted:
                    // a query result may have been drawn meanwhile.
                    // A load started meanwhile reads the current state anyway.
                    Ok(_) if self.loading => {}
                    Ok(marker) if self.auto_refresh_running() => match &self.change_marker {
                        // The refresh keeps the marker it reads with the records
                        // (see `fetch`). If it fails, this one stays, so a
                        // failing refresh isn't started again at every check.
                        Some(previous) if *previous != marker => {
                            debug!("the database changed; refreshing the graph");
                            self.change_marker = Some(marker);
                            return self.start_load(LoadPurpose::Refresh, LoadWhat::Records);
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
                    if let Err(e) = self.config.save_to(&self.files.config) {
                        warn!(error = %e, "could not save the setting");
                    }
                    // The offsets belong to the other view's load.
                    self.canton_offsets = None;
                    self.apply_visualization();
                    // The views load different records: a window of transactions, or every table.
                    return self.reload(LoadPurpose::Reload { notice: Some("Reloaded"), fit: true });
                }
            }
            Message::ToggleFollowNewest(on) => {
                self.config.graph.canton_follow_newest = on;
                if let Err(e) = self.config.save_to(&self.files.config) {
                    warn!(error = %e, "could not save the setting");
                }
                if on && self.canton() {
                    self.scroll_canton(f32::MAX);
                }
            }
            Message::ToggleFitOnFilter(on) => {
                self.config.graph.fit_on_filter = on;
                if let Err(e) = self.config.save_to(&self.files.config) {
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
                            if let Err(e) = self.config.save_to(&self.files.config) {
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
            // The window changes when its transactions arrive (see `show_loaded`).
            Message::CantonPage(window) => {
                self.page_notice = None;
                return self.start_load(LoadPurpose::Page(window), LoadWhat::Records);
            }
            Message::FitView => self.fit_view(),
            Message::CanvasResized(size) => {
                self.canvas_size = size;
                // A narrower window wraps the query onto more lines.
                self.fit_panel();
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
            Message::ViewerUserLevelChanged(level) => {
                if let Some(form) = &mut self.settings {
                    form.viewer_user_level = level;
                    form.message = None;
                }
            }
            Message::SaveSettings => return self.save_settings(),
            Message::DatabaseOpened(config, result, report) => return self.database_opened(config, result, report),

            Message::ToggleQuery => {
                self.query_open = !self.query_open;
                self.fit_panel();
            }
            Message::QueryEdited(action) => {
                self.query_text.perform(action);
                self.fit_panel();
            }
            Message::ClearQuery => self.query_text = text_editor::Content::new(),
            Message::PanelDragStart => self.panel_drag = Some(None),
            Message::PanelDrag(y) => match self.panel_drag {
                Some(None) => self.panel_drag = Some(Some((y, self.panel_height, self.panel_height + self.canvas_size.height))),
                Some(Some((start_y, start_height, total))) => {
                    let max = (total - crate::gui::query_panel::MIN_GRAPH_HEIGHT).max(crate::gui::query_panel::MIN_PANEL_HEIGHT);
                    self.panel_height = (start_height + start_y - y).clamp(crate::gui::query_panel::MIN_PANEL_HEIGHT, max);
                }
                None => {}
            },
            Message::PanelDragEnd => self.panel_drag = None,
            Message::RunQuery => return self.run_query(),
            #[cfg(feature = "dashboard")]
            Message::ShowDashboard(open) => {
                let opening = open && !self.dashboard_open;
                self.dashboard_open = open;
                if opening {
                    return self.dashboard.reload().map(Message::Dashboard);
                }
            }
            #[cfg(feature = "dashboard")]
            Message::Dashboard(m) => {
                use crate::gui::dashboard::Msg;
                if let Msg::ShowTransactions { from, to, label } = m {
                    return self.show_transactions(from, to, &label);
                }
                let save = matches!(m, Msg::LocationSubmitted | Msg::Select(_) | Msg::SaveSettings);
                let task = self.dashboard.update(m).map(Message::Dashboard);
                // Remember where the tables are, which one was shown, and
                // how often to look for commits.
                if save {
                    self.config.analytics.location = self.dashboard.location.trim().to_string();
                    self.config.analytics.selected = self.dashboard.selected.clone();
                    self.config.analytics.check_seconds = self.dashboard.check_seconds;
                    if let Err(e) = self.config.save_to(&self.files.config) {
                        warn!(error = %e, "could not save the dashboard's settings");
                    }
                }
                return task;
            }
            Message::PickSavedQuery(name) => {
                if let Some(saved) = self.saved_queries.get(&name) {
                    self.query_text = text_editor::Content::with_text(&saved.query);
                    self.fit_panel();
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
            Message::QueryDone(outcome) => {
                self.query_running = false;
                let has_records = !query::record_ids(&outcome).is_empty();
                self.query_drawable = has_records;
                self.query_outcome = Some(outcome);
                // A result with records becomes the graph, unless "Show in
                // table" asked for it ("Show in graph" still draws it).
                let keep_graph = std::mem::take(&mut self.keep_graph_for_query);
                if has_records && !keep_graph {
                    return self.load_result();
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
                        self.query_ran = query.clone();
                        self.fit_panel();
                        self.query_drawable = !query::record_ids(&outcome).is_empty();
                        self.query_outcome = Some(outcome);
                        self.apply_result(graph);
                    }
                    WindowCommand::Overview => {
                        return self.reload(LoadPurpose::Reload { notice: None, fit: false });
                    }
                    WindowCommand::DataChanged => {
                        if self.result_view.is_none() {
                            return self.reload(RELOADED);
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
                if let Err(e) = self.config.save_to(&self.files.config) {
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
                            if let Err(e) = self.config.save_to(&self.files.config) {
                                warn!(error = %e, "could not save the setting");
                            }
                            self.apply_visualization();
                        }
                        let notice = match action {
                            DataAction::LoadSampleData => "Sample data loaded.",
                            DataAction::LoadCantonSample => "Canton sample loaded.",
                            DataAction::EraseDatabase => "Database erased.",
                        };
                        return self.reload(LoadPurpose::Reload { notice: Some(notice), fit: true });
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

    /// Reads the database again in the background; records already on
    /// screen keep their place. A big database is shown as the overview.
    fn reload(&mut self, purpose: LoadPurpose) -> Task<Message> {
        let what = self.reload_what();
        self.start_load(purpose, what)
    }

    /// What Reload reads: the Canton view always loads its window of
    /// transactions; otherwise a database bigger than `overview_above` is
    /// shown as the overview.
    fn reload_what(&self) -> LoadWhat {
        if self.canton() { LoadWhat::Records } else { LoadWhat::RecordsOrOverview(self.config.graph.overview_above) }
    }

    /// Reads the graph on our runtime, so the window stays responsive, and
    /// shows it when it arrives (see `show_loaded`). A newer load replaces
    /// one still running.
    fn start_load(&mut self, purpose: LoadPurpose, what: LoadWhat) -> Task<Message> {
        let Some(db) = self.db.clone() else { return Task::none() };
        // The graph is read again once the new database is open.
        if self.switching {
            return Task::none();
        }
        self.load_generation += 1;
        let generation = self.load_generation;
        self.loading = true;
        if purpose != LoadPurpose::Refresh {
            self.notice = Some("Loading…".into());
        }
        let window = match purpose {
            LoadPurpose::Page(window) => window,
            _ => self.canton_window,
        };
        let handle = self.runtime.spawn(self.fetch(db, what, window));
        Task::perform(
            async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
            move |result| Message::GraphLoaded(generation, purpose, result),
        )
    }

    /// A dashboard column was clicked: opens the Graph tab in the Canton
    /// view on the transactions at ledger offsets `from` to `to` (`label`
    /// names the day or week). A busy day holds more than a graph can show,
    /// so then it shows the last of them, as many as "Latest" says; the
    /// paging buttons go to the others.
    #[cfg(feature = "dashboard")]
    fn show_transactions(&mut self, from: i64, to: i64, label: &str) -> Task<Message> {
        self.dashboard_open = false;
        if !self.canton() {
            self.config.graph.visualization = Visualization::Canton;
            if let Err(e) = self.config.save_to(&self.files.config) {
                warn!(error = %e, "could not save the setting");
            }
            self.canton_offsets = None;
            self.apply_visualization();
        }
        let n = self.canton_n();
        // Every transaction has its own offset, so a range no wider than
        // `n` offsets holds at most `n` transactions.
        let window = if to.saturating_sub(from) < n as i64 {
            graph::CantonWindow::Range(from, to)
        } else {
            graph::CantonWindow::Before(to.saturating_add(1), n)
        };
        info!(from, to, label, "showing a dashboard column's transactions");
        let task = self.start_load(LoadPurpose::Page(window), LoadWhat::Records);
        self.notice = Some(format!("Loading the transactions of {label}…"));
        self.page_notice = Some(match window {
            graph::CantonWindow::Range(..) => format!("The transactions of {label}"),
            _ => format!("The last {n} transactions of {label}; ◀ older shows the rest"),
        });
        task
    }

    /// Reads `what` from the database (in the Canton view, transactions in
    /// `window`). It owns what it needs, so it can run on another thread.
    fn fetch(
        &self,
        db: Database,
        what: LoadWhat,
        window: graph::CantonWindow,
    ) -> impl std::future::Future<Output = Result<Loaded, String>> + Send + 'static {
        let canton = self.canton();
        let per_table = self.config.graph.max_records_per_table;
        let limit = self.config.graph.max_result_records;
        let watch = self.config.graph.auto_refresh.then(|| self.watch());
        async move {
            let overview = match what {
                LoadWhat::Overview => true,
                LoadWhat::Records => false,
                LoadWhat::RecordsOrOverview(above) => match graph::record_count(&db).await {
                    Ok(n) => n > above,
                    Err(e) => {
                        warn!(error = %e, "could not count the records");
                        false
                    }
                },
            };
            if overview {
                return graph::overview(&db).await.map(Loaded::Overview).map_err(|e| e.to_string());
            }
            // Read before the records: a change made while they're read then
            // shows up at the next check, rather than being taken as the baseline.
            let marker = match watch {
                Some(watch) => graph::change_marker(&db, watch)
                    .await
                    .inspect_err(|e| debug!(error = %e, "could not read the change marker"))
                    .ok(),
                None => None,
            };
            if canton {
                let loaded =
                    graph::load_canton_window(&db, window, per_table, limit).await.map_err(|e| e.to_string())?;
                let (data, offsets, left_out) = (loaded.data, loaded.offsets, loaded.left_out);
                return Ok(Loaded::Records { data, offsets, left_out, marker });
            }
            let data = graph::load(&db, per_table).await.map_err(|e| e.to_string())?;
            Ok(Loaded::Records { data, offsets: None, left_out: 0, marker })
        }
    }

    /// Shows what a load read, as `purpose` says.
    fn show_loaded(&mut self, purpose: LoadPurpose, result: Result<Loaded, String>) {
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(e) if purpose == LoadPurpose::Refresh => {
                warn!(error = %e, "auto refresh could not load the graph");
                return;
            }
            Err(e) => {
                error!(error = %e, "could not load the graph");
                self.error = Some(e);
                self.notice = None;
                return;
            }
        };
        match (purpose, loaded) {
            (LoadPurpose::Refresh, Loaded::Overview(_)) => {}
            (_, Loaded::Overview(data)) => self.show_overview(data, notice_of(purpose)),
            (LoadPurpose::Refresh, Loaded::Records { data, offsets, left_out, marker }) => {
                // Read with the records, so newer than the check's.
                if marker.is_some() {
                    self.change_marker = marker;
                }
                self.note_window(offsets, left_out);
                self.refresh(data);
            }
            (LoadPurpose::Page(window), Loaded::Records { data, offsets, left_out, marker }) => {
                // Paging past either end would leave an empty view with both
                // paging buttons off: stay on the transactions shown instead.
                let paging = matches!(window, graph::CantonWindow::Before(..) | graph::CantonWindow::After(..));
                if paging && self.canton_offsets.is_some() && offsets.is_none() {
                    self.notice = Some(match window {
                        graph::CantonWindow::Before(..) => "There are no older transactions.".into(),
                        _ => "There are no newer transactions yet.".into(),
                    });
                    return;
                }
                self.canton_window = window;
                self.note_window(offsets, left_out);
                let said = self.page_notice.take();
                self.show_records(data, marker, said.as_deref().or(notice_of(purpose)));
                self.relayout_canton();
            }
            (LoadPurpose::Reload { notice, fit }, Loaded::Records { data, offsets, left_out, marker }) => {
                self.note_window(offsets, left_out);
                self.show_records(data, marker, notice);
                if fit {
                    self.relayout_canton();
                }
            }
        }
    }

    /// In the Canton view, remembers which offsets the loaded window covers.
    fn note_window(&mut self, offsets: Option<(i64, i64)>, left_out: usize) {
        if !self.canton() {
            return;
        }
        self.canton_offsets = offsets;
        self.canton_left_out = left_out;
        if left_out > 0 {
            warn!(left_out, "the Canton window had more records than max_result_records");
        }
    }

    /// Draws the tables as a graph: dots sized by their records, arrows for
    /// the relations between them.
    fn show_overview(&mut self, data: GraphData, notice: Option<&str>) {
        info!(tables = data.nodes.len(), "showing the table overview");
        self.graph = Graph::new(data, None);
        self.overview = true;
        self.result_view = None;
        self.error = None;
        self.notice = notice.map(Into::into);
        self.camera = Camera::default();
        self.hover = None;
        self.selection = None;
        self.focused = None;
        self.menu = None;
        self.dragging = None;
        self.layout.reheat(1.0);
        self.following = true;
    }

    /// Draws the records read from the database in place of the graph;
    /// records already on screen keep their place.
    /// `marker` is auto refresh's fingerprint of what was read.
    fn show_records(&mut self, data: GraphData, marker: Option<serde_json::Value>, notice: Option<&str>) {
        self.overview = false;
        self.change_marker = marker;
        self.graph = Graph::new(data, Some(&self.graph));
        self.result_view = None;
        if self.canton() {
            self.canton_layout_fitted();
        }
        self.error = None;
        self.notice = notice.map(Into::into);
        self.hover = None;
        self.selection = None;
        self.focused = None;
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

    /// What auto refresh watches for changes in the current view.
    fn watch(&self) -> graph::Watch {
        if self.canton() {
            graph::Watch::Canton
        } else {
            graph::Watch::Tables { per_table: self.config.graph.max_records_per_table }
        }
    }

    /// Reads a fingerprint of what the view shows in the background, for
    /// auto refresh (see `graph::change_marker`).
    fn check_for_changes(&mut self) -> Task<Message> {
        let Some(db) = self.db.clone() else { return Task::none() };
        // Skip a check while the user is moving things, or one (or a load) is still running.
        if self.checking || self.loading || self.switching || self.dragging.is_some() || self.menu.is_some() || self.data_busy {
            return Task::none();
        }
        self.checking = true;
        let generation = self.load_generation;
        let watch = self.watch();
        let handle = self.runtime.spawn(async move {
            graph::change_marker(&db, watch).await.map_err(|e| e.to_string())
        });
        Task::perform(
            async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
            move |result| Message::Checked(generation, result),
        )
    }

    /// Shows the records auto refresh read after it saw a change. Unlike
    /// Reload, it keeps the view as it is: the camera, the selected record
    /// and the records the user hid.
    fn refresh(&mut self, data: GraphData) {
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
        // While the database is switched, `db` may already point at the new one.
        if self.data_busy || self.switching {
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
    fn save_settings(&mut self) -> Task<Message> {
        // Wait for the database the last save named.
        if self.switching {
            if let Some(form) = &mut self.settings {
                form.message = Some(Err("Still opening the database: save again when it's open.".into()));
            }
            return Task::none();
        }
        let Some(form) = &mut self.settings else { return Task::none() };
        let mut new = match form.to_config() {
            Ok(config) => config,
            Err(e) => {
                form.message = Some(Err(e));
                return Task::none();
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
        if let Err(e) = new.save_to(&self.files.config) {
            warn!(error = %e, "could not save settings");
            form.message = Some(Err(e.to_string()));
            return Task::none();
        }
        info!(?new, "settings saved");

        let old = self.config.clone();
        let mut report = SettingsReport::default();

        if new.logging.level != old.logging.level {
            match self.log.as_ref().map(|log| log.set_level(&new.logging.level)) {
                Some(Ok(true)) | None => {}
                Some(Ok(false)) => report.notes.push("RUST_LOG is set, so it still decides the log level."),
                Some(Err(e)) => report.problems.push(e.to_string()),
            }
            self.config.logging.level = new.logging.level.clone();
        }
        if new.logging.directory != old.logging.directory {
            report.notes.push("The new log folder is used after a restart.");
        }

        report.graph_changed = new.graph != old.graph;
        if let graph::CantonWindow::Latest(_) = self.canton_window {
            self.canton_window = graph::CantonWindow::Latest(new.graph.canton_transactions);
        }
        if new.graph.canton_transactions != old.graph.canton_transactions {
            self.canton_count = new.graph.canton_transactions.to_string();
        }
        self.config.graph = new.graph.clone();

        let database_changed = new.database != old.database || self.db.is_none();
        if database_changed {
            // Connecting can take a while (or time out): the window stays
            // responsive meanwhile, and the rest is done when it's open.
            self.switching = true;
            self.pending_data_action = None;
            if let Some(form) = &mut self.settings {
                form.message = Some(Ok("Saved. Opening the database…".into()));
            }
            let config = new.database.clone();
            let same_place = self.db.is_some() && config.same_place(&self.config.database);
            let previous = self.config.database.clone();
            let handle = self.runtime.spawn(open_database(self.db.clone(), same_place, config.clone(), previous));
            return Task::perform(
                async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) },
                move |result| Message::DatabaseOpened(config.clone(), result, report.clone()),
            );
        }
        self.finish_settings(report)
    }

    /// Uses the database the saved settings name, once it's open: in place
    /// of the current one, which is closed if it was a different connection.
    fn database_opened(
        &mut self,
        config: DatabaseConfig,
        result: Result<Opened, String>,
        mut report: SettingsReport,
    ) -> Task<Message> {
        self.switching = false;
        match result {
            Ok(Opened { db, queries }) => {
                if let Some(new) = db
                    && let Some(old) = self.db.replace(new)
                {
                    self.runtime.spawn(async move {
                        if let Err(e) = db::close(old).await {
                            warn!(error = %e, "could not close the previous database");
                        }
                    });
                }
                if let Queries::Failed(e) = &queries {
                    error!(error = %e, "could not connect the viewer user");
                    report.problems.push(format!("Queries won't run: {e}"));
                }
                let old = std::mem::replace(&mut self.queries, queries);
                self.runtime.spawn(old.close());
                self.config.database = config;
                // The query results and pending actions belong to the previous database.
                // A write Claude Code asked for was meant for the previous database.
                self.answer_write(false);
                self.query_outcome = None;
                self.query_drawable = false;
                self.result_view = None;
                self.pending_data_action = None;
                // The graph is read again from the new database.
                report.graph_changed = true;
            }
            Err(e) => report.problems.push(format!("Could not open the new database, still using the previous one: {e}")),
        }
        self.finish_settings(report)
    }

    /// Says in the settings form how saving went, and reads the graph again
    /// if what it shows changed.
    fn finish_settings(&mut self, report: SettingsReport) -> Task<Message> {
        let message = if report.problems.is_empty() {
            let mut m = String::from("Saved and applied.");
            for n in report.notes {
                m.push(' ');
                m.push_str(n);
            }
            Ok(m)
        } else {
            Err(format!("Saved, but: {}", report.problems.join(" ")))
        };
        if let Some(form) = &mut self.settings {
            form.message = Some(message);
        }
        if report.graph_changed { self.reload(RELOADED) } else { Task::none() }
    }

    /// Saves the editor's query under the typed name (replacing a saved
    /// query of that name) and writes `queries.toml`.
    fn save_query(&mut self) {
        if self.refuse_unreadable_queries() {
            return;
        }
        let name = self.saved_query_name.trim().to_string();
        match self.saved_queries.put(&name, &self.query_text.text()) {
            Ok(replaced) => {
                if let Err(e) = self.saved_queries.save_to(&self.files.queries) {
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

    /// Whether saving must be refused because `queries.toml` couldn't be read.
    fn refuse_unreadable_queries(&mut self) -> bool {
        if self.saved_queries_unreadable {
            self.notice = Some(format!(
                "Not saved: {} could not be read. Fix or remove it, then restart.",
                self.files.queries.display()
            ));
        }
        self.saved_queries_unreadable
    }

    fn delete_saved_query(&mut self) {
        if self.refuse_unreadable_queries() {
            return;
        }
        let Some(name) = self.saved_query.take() else { return };
        if !self.saved_queries.remove(&name) {
            return;
        }
        if let Err(e) = self.saved_queries.save_to(&self.files.queries) {
            warn!(error = %e, "could not save the queries");
            self.error = Some(format!("Could not save the queries: {e}"));
            return;
        }
        info!(query = %name, "deleted a saved query");
        self.notice = Some(format!("Deleted \"{name}\""));
    }

    fn run_query(&mut self) -> Task<Message> {
        let sql = self.query_text.text();
        let Some(main) = &self.db else { return Task::none() };
        if sql.trim().is_empty() || self.query_running || self.switching {
            return Task::none();
        }
        self.query_ran = sql.clone();
        // The query panel only reads. With a viewer user the server refuses
        // writes; without one, the app refuses what looks like a write.
        let refused = match self.queries.pick(main) {
            Err(e) => Some(format!("The query didn't run: {e}")),
            Ok(_) if !self.queries.enforced() && !query::is_read_only(&sql) => Some(query::WRITE_REFUSED_HERE.to_string()),
            Ok(_) => None,
        };
        if let Some(message) = refused {
            self.query_drawable = false;
            self.query_outcome = Some(QueryOutcome::refused(message));
            return Task::none();
        }
        let Ok(db) = self.queries.pick(main) else { return Task::none() };
        self.query_running = true;
        let enforced = self.queries.enforced();
        // Run on our runtime, and only wait for the result on iced's side, so the window stays responsive.
        let handle = self.runtime.spawn(async move {
            let mut outcome = query::run(&db, sql.clone()).await;
            if enforced {
                query::explain_refused_writes(&mut outcome, &sql);
            }
            outcome
        });
        Task::perform(
            async move {
                handle.await.unwrap_or_else(|e| QueryOutcome::refused(format!("the query task failed: {e}")))
            },
            Message::QueryDone,
        )
    }

    /// Adds a record's neighbours to the graph, placed around it.
    fn add_neighbours(&mut self, id: &str, result: ResultGraph) {
        let Some((added, links)) = self.add_around(id, result) else { return };
        self.select_record(id);
        self.notice = Some(match (added, links) {
            (0, 0) => format!("{id} has no other relations"),
            (a, l) => format!("Added {} and {} around {id}", plural(a, "record"), plural(l, "relation")),
        });
    }

    /// Selects the record with this id, scrolling the Canton view to it.
    fn select_record(&mut self, id: &str) {
        let found = self.graph.nodes.iter().position(|n| n.id == id);
        self.selection = found.map(Selection::Node);
        if let Some(i) = found
            && self.canton()
        {
            self.bring_into_view(i);
        }
    }

    /// Runs `work` on the open database, on our runtime, and turns its
    /// result into a message. Does nothing without a database.
    fn in_background<T, F>(
        &self,
        work: impl FnOnce(Database) -> F,
        done: impl Fn(Result<T, String>) -> Message + Send + 'static,
    ) -> Task<Message>
    where
        T: Send + 'static,
        F: std::future::Future<Output = surrealdb::Result<T>> + Send + 'static,
    {
        let Some(db) = self.db.clone() else { return Task::none() };
        let future = work(db);
        let handle = self.runtime.spawn(async move { future.await.map_err(|e| e.to_string()) });
        Task::perform(async move { handle.await.unwrap_or_else(|e| Err(format!("the task failed: {e}"))) }, done)
    }

    /// Adds records and relations to the graph around record `id`, new
    /// records placed on a small circle around it. Returns how many records
    /// and relations were new, or `None` if `id` isn't in the graph.
    fn add_around(&mut self, id: &str, result: ResultGraph) -> Option<(usize, usize)> {
        let centre = self.graph.nodes.iter().find(|n| n.id == id).map(|n| n.pos)?;
        // A load still running would replace the neighbours when it arrives.
        self.load_generation += 1;
        self.loading = false;
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
        // The old indices point into the graph that was replaced.
        self.selection = None;
        self.hover = None;
        self.result_view = Some(ResultView);
        if self.canton() {
            self.relayout_canton();
        } else {
            self.layout.reheat(0.4);
        }
        Some((added, links))
    }

    /// Draws a query result as the graph.
    fn apply_result(&mut self, result: ResultGraph) {
        if result.data.nodes.is_empty() {
            self.notice = Some("None of the returned records could be drawn.".into());
            return;
        }
        let limit = self.config.graph.max_result_records;
        // A load still running would replace the result when it arrives.
        self.load_generation += 1;
        self.loading = false;
        self.overview = false;
        self.graph = Graph::new(result.data, None);
        self.notice = (result.left_out > 0).then(|| {
            format!("Drew the first {limit} of {} records. Raise the limit in Settings to see more.", result.found)
        });
        self.result_view = Some(ResultView);
        self.camera = Camera::default();
        self.hover = None;
        self.selection = None;
        self.focused = None;
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
        if let Ok(mut queries) = shared.queries.lock() {
            *queries = self.queries.clone();
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

    /// Draws the whole graph offscreen, then copies the picture to the
    /// clipboard or (with `save`) saves it as a PNG in the Downloads folder.
    /// Encoding and copying run off the window's thread.
    fn export_image(&mut self, save: bool) -> Task<Message> {
        self.canvas_menu = None;
        // The shortcut does nothing while the dashboard is open.
        #[cfg(feature = "dashboard")]
        if self.dashboard_open {
            return Task::none();
        }
        let scene = crate::gui::export::Scene {
            graph: &self.graph,
            visualization: self.config.graph.visualization,
            selection: self.selection,
            camera: self.camera,
            canton: (self.canton_scale(), self.canton_fill_width()),
            canton_height: self.canton_content_height(),
        };
        let image = match scene.render() {
            Ok(Some(image)) => image,
            Ok(None) => {
                self.notice = Some("Nothing to copy: no records are shown".into());
                return Task::none();
            }
            Err(e) => {
                self.error = Some(format!("Could not draw the picture: {e}"));
                return Task::none();
            }
        };
        let stem = if self.canton() { "canton-graph" } else { "graph" };
        self.notice = Some(if save { "Saving the picture…" } else { "Copying the picture…" }.into());
        let size = format!("{} × {} px", image.width, image.height);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    if save {
                        let path = image.save_png(&crate::gui::export::pictures_folder(), stem)?;
                        info!(path = %path.display(), "saved a picture of the graph");
                        Ok(format!("Saved {} ({size})", path.display()))
                    } else {
                        image.copy_to_clipboard().map_err(|e| format!("Could not copy the picture: {e}"))?;
                        Ok(format!("Copied a picture of the whole graph ({size})"))
                    }
                })
                .await
                .map_err(|e| e.to_string())?
            },
            Message::ImageDone,
        )
    }

    /// The graph canvas's view of the app.
    fn graph_view(&self) -> GraphView<'_> {
        GraphView {
            graph: &self.graph,
            camera: self.camera,
            hover: self.hover,
            selection: self.selection,
            visualization: self.config.graph.visualization,
            known_size: self.canvas_size,
            scroll: self.canton().then(|| (self.canton_scroll, self.canton_content_height())),
        }
    }

    // What the canvas draws has no widget text that a test could find, so
    // these say what it shows. They only read the app's state.

    /// The ids of the records kept by hiding and the Canton filters, whether
    /// or not they are scrolled into view.
    pub fn shown_records(&self) -> Vec<String> {
        let canton = self.canton();
        self.graph
            .nodes
            .iter()
            .filter(|n| if canton { n.card.shown } else { !n.hidden })
            .map(|n| n.id.clone())
            .collect()
    }

    /// The index of the record with this id, as [`Selection::Node`] takes it.
    pub fn record_index(&self, id: &str) -> Option<usize> {
        self.graph.nodes.iter().position(|n| n.id == id)
    }

    /// How strongly the Canton view draws this record's card, from 0 to 1
    /// (1 is full strength). `None` if the card isn't drawn.
    pub fn card_alpha(&self, id: &str) -> Option<f32> {
        let i = self.record_index(id)?;
        let node = &self.graph.nodes[i];
        if !(self.canton() && node.card.shown) {
            return None;
        }
        Some(crate::gui::canton_draw::card_alpha(self.graph_view().lit().as_ref(), i, node.card.status))
    }

    /// The text in the query editor.
    pub fn query_text(&self) -> String {
        self.query_text.text()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let graph = canvas(self.graph_view())
        .width(Fill)
        .height(Fill);

        let mut layers = stack![graph].width(Fill).height(Fill);
        if let Some((node, _)) = self.menu {
            layers = layers.push(self.record_menu(node));
        }
        if let Some(at) = self.canvas_menu {
            // Kept inside the canvas, like the record menu.
            let x = at.x.min(self.canvas_size.width - 250.0).max(0.0);
            let y = at.y.min(self.canvas_size.height - 100.0).max(0.0);
            layers = layers.push(pin(self.canvas_menu()).x(x).y(y));
        }
        if let Some((query, _)) = &self.pending_write {
            layers = layers.push(write_banner(query));
        }

        let settings = self.settings.as_ref().map(|form| {
            form.view(self.pending_data_action, self.data_busy || self.switching, self.db.is_some())
        });

        // The dashboard takes the whole window: the graph's sidebar and query
        // panel belong to the graph, so they are hidden while it is open.
        #[cfg(feature = "dashboard")]
        if self.dashboard_open {
            let bar = row![
                self.mode_switch(),
                iced::widget::space::horizontal(),
                button(text("Settings").size(12))
                    .style(button::secondary)
                    .on_press(Message::Dashboard(crate::gui::dashboard::Msg::OpenSettings)),
            ]
            .padding([8, 10])
            .align_y(iced::Alignment::Center);
            let mut main = column![bar, rule::horizontal(1)];
            // A pending write must be answered whichever view is open.
            if let Some((query, _)) = &self.pending_write {
                main = main.push(write_banner(query));
            }
            if let Some(alarm) = &self.memory_alarm {
                main = main.push(container(text(alarm).size(12).color(crate::gui::sidebar::ERROR)).padding([4, 10]));
            }
            let main = main.push(self.dashboard.view().map(Message::Dashboard)).width(Fill);
            // The Dashboard's own settings; the graph's stay with the graph.
            return match self.dashboard.settings_view() {
                Some(side) => row![main, rule::vertical(1), side.map(Message::Dashboard)].into(),
                None => main.into(),
            };
        }

        let side = settings.unwrap_or_else(|| self.sidebar());
        let main: Element<'_, Message> = layers.into();

        // The open query panel can be resized by dragging the line above it.
        let divider: Element<'_, Message> = if self.query_open {
            iced::widget::mouse_area(container(rule::horizontal(1)).height(7).center_y(7).width(Fill))
                .on_press(Message::PanelDragStart)
                .interaction(iced::mouse::Interaction::ResizingVertically)
                .into()
        } else {
            rule::horizontal(1).into()
        };

        row![
            column![main, divider, self.query_panel()].width(Fill),
            rule::vertical(1),
            side
        ]
        .into()
    }

    fn record_menu(&self, node: usize) -> Element<'_, Message> {
        let n = &self.graph.nodes[node];
        let header = container(
            row![text("●").color(n.color), text(if self.overview { &n.table } else { &n.id }).font(Font::MONOSPACE).size(13)]
                .spacing(8),
        )
        .padding([6, 10]);
        let entries = if self.overview {
            vec![
                MenuEntry::Rule,
                MenuEntry::Item("Show records".into(), Message::MenuShowRecords),
                MenuEntry::Item("Inspect".into(), Message::MenuInspect),
            ]
        } else {
            self.record_menu_entries(node)
        };
        // Kept inside the canvas: a long menu opened low down moves up.
        let height = MENU_HEADER
            + entries.iter().map(|e| if matches!(e, MenuEntry::Rule) { MENU_RULE } else { MENU_ITEM }).sum::<f32>();
        let mut items = column![header];
        for entry in entries {
            let line: Element<'_, Message> = match entry {
                MenuEntry::Rule => container(rule::horizontal(1)).padding([3, 0]).into(),
                MenuEntry::Item(label, message) => button(text(ellipsize(&label, 34)).size(13))
                    .width(Fill)
                    .padding([3, 10])
                    .style(button::text)
                    .on_press(message)
                    .into(),
            };
            items = items.push(line);
        }
        let menu = container(items)
            .width(MENU_WIDTH)
            .padding(4)
            .style(|theme| container::bordered_box(theme).background(Color::WHITE));
        let Some((_, at)) = self.menu else { return menu.into() };
        let x = at.x.min(self.canvas_size.width - MENU_WIDTH - 4.0).max(0.0);
        let y = at.y.min(self.canvas_size.height - height - 4.0).max(0.0);
        pin(menu).x(x).y(y).into()
    }

    /// A record's menu, in groups: what to load around it, how to show
    /// it, and what to copy. Canton records get the ledger's own items.
    fn record_menu_entries(&self, node: usize) -> Vec<MenuEntry> {
        use MenuEntry::{Item, Rule};
        let n = &self.graph.nodes[node];
        let event = matches!(n.kind, NodeKind::Created | NodeKind::Exercised | NodeKind::Fetched);
        let mut entries = vec![Rule, Item("Show neighbours".into(), Message::MenuNeighbours)];
        match n.kind {
            NodeKind::Transaction => entries.push(Item("Show whole transaction".into(), Message::MenuTransaction)),
            _ if event => entries.push(Item("Show transaction".into(), Message::MenuTransaction)),
            _ => {}
        }
        if n.kind == NodeKind::Created {
            entries.push(Item("Show archiving event".into(), Message::MenuArchivingEvent));
        }
        if event {
            entries.push(Item("Trace contract lifecycle".into(), Message::MenuLifecycle));
        }
        entries.push(Item("Show in table".into(), Message::MenuShowInTable));

        entries.push(Rule);
        entries.push(Item("Inspect".into(), Message::MenuInspect));
        if self.focused.as_deref() == Some(n.id.as_str()) {
            entries.push(Item("Clear focus".into(), Message::MenuClearFocus));
        } else {
            entries.push(Item("Focus record".into(), Message::MenuFocus));
            if self.focused.is_some() {
                entries.push(Item("Clear focus".into(), Message::MenuClearFocus));
            }
        }
        entries.push(Item("Hide record".into(), Message::MenuHide));
        let hidden = self.graph.hidden_count();
        if hidden > 0 {
            entries.push(Item(format!("Unhide all ({hidden})"), Message::MenuUnhideAll));
        }
        if self.canton() {
            let filters = &self.canton_filters;
            if filters.template.is_some() {
                entries.push(Item("Clear template filter".into(), Message::MenuFilterTemplate(None)));
            } else if let Some(t) = &n.card.template {
                entries.push(Item(format!("Filter to {t}"), Message::MenuFilterTemplate(Some(t.clone()))));
            }
            if filters.party.is_some() {
                entries.push(Item("Clear party filter".into(), Message::MenuFilterParty(None)));
            } else {
                // A transaction joins every party in its tree: offer the first few.
                for p in n.card.parties.iter().take(MENU_PARTIES) {
                    entries.push(Item(
                        format!("Filter to party {}", crate::canton::short_party(p)),
                        Message::MenuFilterParty(Some(p.clone())),
                    ));
                }
            }
        }

        entries.push(Rule);
        entries.push(Item("Copy record id".into(), Message::MenuCopy));
        if contract_id(n).is_some() {
            entries.push(Item("Copy contract id".into(), Message::MenuCopyContractId));
        }
        entries.push(Item("Copy as JSON".into(), Message::MenuCopyJson));
        entries
    }
}

/// One line of a record's menu.
enum MenuEntry {
    Item(String, Message),
    Rule,
}

const MENU_WIDTH: f32 = 240.0;
/// Heights used to keep the menu inside the canvas, in pixels.
const MENU_HEADER: f32 = 40.0;
const MENU_ITEM: f32 = 25.0;
const MENU_RULE: f32 = 7.0;
/// How many "Filter to party" items a record's menu offers at most.
const MENU_PARTIES: usize = 3;

/// The contract id a Canton record is (a contract) or acts on (an exercise or fetch).
fn contract_id(n: &crate::gui::model::Node) -> Option<String> {
    if !matches!(n.kind, NodeKind::Created | NodeKind::Exercised | NodeKind::Fetched) {
        return None;
    }
    ["contract_id", "target_contract_id"]
        .iter()
        .find_map(|f| n.data.get(*f)?.as_str().filter(|s| !s.is_empty()))
        .map(String::from)
}

impl App {
    /// The menu for the canvas's background: pictures of the whole graph.
    fn canvas_menu(&self) -> Element<'_, Message> {
        let shortcut = if cfg!(target_os = "macos") { "⌘⇧C" } else { "Ctrl+Shift+C" };
        let muted = Color::from_rgb8(0x68, 0x73, 0x85);
        let item = |label: &'static str, hint: &'static str, message: Message| {
            button(row![text(label).size(13), iced::widget::space::horizontal(), text(hint).size(12).color(muted)])
                .width(Fill)
                .padding([3, 10])
                .style(button::text)
                .on_press(message)
        };
        container(column![
            container(text("Whole graph").size(12).color(muted)).padding([6, 10]),
            container(rule::horizontal(1)).padding([3, 0]),
            item("Copy as image", shortcut, Message::CopyImage),
            item("Save as PNG in Downloads", "", Message::SaveImage),
        ])
        .width(240)
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
fn memory_alarms() -> impl iced::futures::Stream<Item = Message> {
    use iced::futures::StreamExt;
    crate::memory::alarms().map(Message::MemoryAlarm)
}

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

/// What Reload and Load everything say when they're done.
const RELOADED: LoadPurpose = LoadPurpose::Reload { notice: Some("Reloaded"), fit: false };

/// The notice a load for `purpose` shows when it's done.
fn notice_of(purpose: LoadPurpose) -> Option<&'static str> {
    match purpose {
        LoadPurpose::Reload { notice, .. } => notice,
        LoadPurpose::Page(_) => Some("Reloaded"),
        LoadPurpose::Refresh => None,
    }
}

/// Opens the database `config` describes. The same server and user keep the
/// `current` connection and only switch namespace and database; otherwise a
/// new connection is made. The viewer signs in again: a database or
/// namespace viewer belongs to one database or namespace.
/// If switching fails halfway, the connection goes back to `previous`'s
/// namespace and database, which the app still uses.
async fn open_database(
    current: Option<Database>,
    same_place: bool,
    config: DatabaseConfig,
    previous: DatabaseConfig,
) -> Result<Opened, String> {
    let db = match current {
        Some(db) if same_place => {
            if let Err(e) = db.use_ns(&config.namespace).use_db(&config.database).await {
                if let Err(back) = db.use_ns(&previous.namespace).use_db(&previous.database).await {
                    error!(error = %back, "could not go back to the previous namespace/database");
                }
                return Err(e.to_string());
            }
            info!(namespace = %config.namespace, database = %config.database, "switched namespace/database");
            None
        }
        _ => Some(db::connect(&config).await.map_err(|e| e.to_string())?),
    };
    let queries = Queries::connect(&config).await;
    Ok(Opened { db, queries })
}
