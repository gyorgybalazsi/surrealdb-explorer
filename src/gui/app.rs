//! Window state and message handling. The views are in `sidebar.rs`,
//! `settings.rs` and `query_panel.rs`.

use std::collections::{HashMap, HashSet};

use iced::widget::{button, canvas, column, container, pin, row, rule, stack, text, text_editor};
use iced::{Color, Element, Fill, Font, Point, Subscription, Task, Theme, Vector, window};
use surrealdb_explorer::assistant::{self, Assistant, Suggestion};
use surrealdb_explorer::config::{AssistantMode, Config};
use surrealdb_explorer::db::{self, Database};
use surrealdb_explorer::graph::{self, GraphData};
use surrealdb_explorer::config::DatabaseConfig;
use surrealdb_explorer::data;
use surrealdb_explorer::logging::LogControl;
use surrealdb_explorer::query::{self, QueryOutcome};
use tracing::{debug, error, info, warn};

use crate::gui::canvas::{Camera, GraphView};
use crate::gui::layout::Layout;
use crate::gui::model::Graph;
use crate::gui::settings::{SettingsField, SettingsForm};

/// A query's result shown in the graph: the records that answer it, and
/// the relations that show how they connect. Kept as ids, so it survives
/// reloads.
#[derive(Debug, Clone, Default)]
pub struct ResultView {
    pub answer: HashSet<String>,
    pub context_edges: HashSet<String>,
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
    SaveSettings,

    // Query panel
    ToggleQuery,
    QueryEdited(text_editor::Action),
    RunQuery,
    /// The query's outcome, and the outcome of its context query if one ran.
    QueryDone(QueryOutcome, Option<QueryOutcome>),
    ShowQueryInGraph,

    // Plain-English questions
    QuestionEdited(String),
    Ask,
    RetryWithOpus,
    /// The assistant to keep (it holds the API key) and the answer.
    AskDone(Option<Assistant>, Result<Suggestion, String>),

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

    pub(super) question: String,
    pub(super) asking: bool,
    /// The last query Claude suggested.
    pub(super) suggestion: Option<Suggestion>,
    pub(super) ask_error: Option<String>,
    /// Kept after the first question, so the API key is read only once.
    pub(super) assistant: Option<Assistant>,

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

        App {
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
            question: String::new(),
            asking: false,
            suggestion: None,
            ask_error: None,
            assistant: None,
            pending_data_action: None,
            data_busy: false,
        }
    }

    pub fn theme(&self) -> Theme {
        Theme::Light
    }

    pub fn subscription(&self) -> Subscription<Message> {
        if self.layout.is_active() || self.dragging.is_some() {
            window::frames().map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
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
                for n in &mut self.graph.nodes {
                    n.hidden = false;
                }
                self.result_view = None;
                self.notice = None;
                self.layout.reheat(0.3);
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
            Message::SaveSettings => self.save_settings(),

            Message::ToggleQuery => self.query_open = !self.query_open,
            Message::QueryEdited(action) => self.query_text.perform(action),
            Message::RunQuery => return self.run_query(),
            Message::QueryDone(outcome, context) => {
                self.query_running = false;
                let changed_anything = outcome
                    .statements
                    .as_ref()
                    .is_ok_and(|s| s.iter().any(|r| r.result.is_ok()));
                let suggested = context.is_some();
                self.query_outcome = Some(outcome);
                self.query_context = context;
                // The query may have changed data; show the current state.
                if changed_anything {
                    self.reload();
                }
                // A suggested query shows its answer, and how it connects, straight away.
                if suggested {
                    self.show_query_in_graph();
                }
            }
            Message::ShowQueryInGraph => self.show_query_in_graph(),

            Message::QuestionEdited(question) => self.question = question,
            Message::Ask => return self.ask(false),
            Message::RetryWithOpus => return self.ask(true),
            Message::AskDone(assistant, result) => {
                self.asking = false;
                if assistant.is_some() {
                    self.assistant = assistant;
                }
                match result {
                    Ok(suggestion) => {
                        self.query_text = text_editor::Content::with_text(&suggestion.query);
                        self.query_outcome = None;
                        self.suggestion = Some(suggestion);
                    }
                    Err(e) => {
                        warn!(error = %e, "question failed");
                        // A rejected key is read again next time.
                        if e.contains("rejected the API key") {
                            self.assistant = None;
                        }
                        self.ask_error = Some(e);
                    }
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
    /// The same folder keeps its connection and only switches namespace and
    /// database. A different folder is opened first, and the old one is
    /// closed only if that succeeds, which releases its lock so it can be
    /// opened again later.
    fn switch_database(&mut self, config: &DatabaseConfig) -> Result<(), String> {
        let same_folder = config.path == self.config.database.path;
        match &self.db {
            Some(db) if same_folder => {
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
        // A query Claude suggested, run unchanged, brings its context query.
        let context = self
            .suggestion
            .as_ref()
            .filter(|s| s.query.trim() == sql.trim())
            .and_then(|s| s.context_query.clone());
        // Run on our runtime (big thread stacks, see `runtime`), and only
        // wait for the result on iced's side, so the window stays responsive.
        let handle = self.runtime.spawn(async move {
            let outcome = query::run(&db, sql).await;
            let context = match context {
                Some(context) if outcome.statements.is_ok() => Some(query::run(&db, context).await),
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
            |(outcome, context)| Message::QueryDone(outcome, context),
        )
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
        self.ask_error = None;
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
                _ => assistant.ask(&db, &question, mode).await,
            };
            (Some(assistant), result.map_err(|e| e.to_string()))
        });
        Task::perform(
            async move { handle.await.unwrap_or_else(|e| (None, Err(format!("the task failed: {e}")))) },
            |(assistant, result)| Message::AskDone(assistant, result),
        )
    }

    /// Shows the last query's result in the graph: the records it returned,
    /// marked as the answer, plus the relations its context query returned
    /// and the records along them. Everything else is hidden.
    fn show_query_in_graph(&mut self) {
        let Some(outcome) = &self.query_outcome else { return };
        let answer_ids: HashSet<String> = query::record_ids(outcome).into_iter().collect();
        let context_ids: HashSet<String> =
            self.query_context.iter().flat_map(query::record_ids).collect();

        let mut answer: HashSet<usize> = HashSet::new();
        let mut keep: HashSet<usize> = HashSet::new();
        for (i, n) in self.graph.nodes.iter().enumerate() {
            if answer_ids.contains(&n.id) {
                answer.insert(i);
                keep.insert(i);
            } else if context_ids.contains(&n.id) {
                keep.insert(i);
            }
        }
        // Relations the query or its context returned bring both of their records.
        let mut context_edges: HashSet<String> = HashSet::new();
        for e in &self.graph.edges {
            if answer_ids.contains(&e.id) || context_ids.contains(&e.id) {
                keep.insert(e.from);
                keep.insert(e.to);
                context_edges.insert(e.id.clone());
            }
        }
        if keep.is_empty() {
            self.notice = Some("None of the query's records are in the graph.".into());
            return;
        }
        for (i, n) in self.graph.nodes.iter_mut().enumerate() {
            n.hidden = !keep.contains(&i);
        }
        info!(answer = answer.len(), shown = keep.len(), relations = context_edges.len(), "showing query results in the graph");
        self.notice = None;
        self.result_view = Some(ResultView {
            answer: answer.iter().map(|&i| self.graph.nodes[i].id.clone()).collect(),
            context_edges,
        });
        self.selection = None;
        self.hover = None;
        self.layout.reheat(0.3);
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
