//! The Dashboard: the Delta Lake tables the canton-analytics daemon writes
//! (see the syncer repo's `analytics/`), each drawn from the panels its
//! plugin declares, or from panels made from its columns.
//!
//! The explorer only reads. It lists the tables at a location (a folder, or
//! an `s3://`, `az://` or `gs://` URL), opens one in a DataFusion session,
//! and every couple of seconds checks the table's log and the daemon's
//! heartbeat. A new commit reloads the panels, reading only the new log
//! entries. Queries run on background tasks; the view draws plain values.

use std::collections::HashMap;
use std::sync::Arc;

use analytics_reader::catalog::{self, Entry};
use analytics_reader::dashboard::{self, Dashboard as Data, PanelData, Period};
use analytics_reader::frame::{Column, Frame, Values};
use analytics_reader::location::{self, Location};
use analytics_reader::manifest::{Format, PanelKind};
use analytics_reader::status::{Status, now_ms};
use analytics_reader::tuning::Tuning;
use analytics_reader::Session;
use iced::widget::{button, column, container, pick_list, responsive, row, scrollable, text, text_input, Id};
use iced::{Alignment, Color, Element, Fill, Font, Length, Task, Theme};
use iced_plot::{LineStyle, MarkerStyle, PlotUiMessage, PlotWidget, PlotWidgetBuilder, Series};
use iced_table2::table::{self, table};
use plotters::prelude::*;
use plotters::style::Color as _;
use plotters_iced2::{Chart, ChartWidget, DrawingBackend};
use tokio::sync::Mutex;

use crate::gui::sidebar::{ERROR, MUTED, SIDEBAR_WIDTH};

const ACCENT: Color = Color::from_rgb(0.18, 0.42, 0.78);
const SERIES: [Color; 4] = [
    ACCENT,
    Color::from_rgb(0.85, 0.45, 0.12),
    Color::from_rgb(0.20, 0.60, 0.35),
    Color::from_rgb(0.55, 0.30, 0.70),
];
const OK: Color = Color::from_rgb(0.20, 0.60, 0.35);
const WARN: Color = Color::from_rgb(0.80, 0.55, 0.10);

/// An open table, shared with the background tasks that refresh it.
#[derive(Clone)]
pub struct Shared(Arc<Mutex<Session>>);

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Session")
    }
}

#[derive(Debug, Clone)]
pub struct Opened {
    session: Shared,
    data: Arc<Data>,
    status: Option<Status>,
}

/// What a check found: the heartbeat, and the panels again if the table
/// has a new version (or the error loading it).
#[derive(Debug, Clone)]
pub struct Checked {
    status: Option<Status>,
    version: i64,
    data: Option<Result<Arc<Data>, String>>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    LocationEdited(String),
    /// Enter in the location box: list the tables there (the app also
    /// saves it).
    LocationSubmitted,
    /// A table picked (the app saves it).
    Select(String),
    /// Look for new tables and reload the shown one now.
    RefreshNow,
    /// Time to look for a new commit and read the heartbeat.
    Check,
    /// The tables at the location; the number says which request.
    Listed(u64, Result<Vec<Entry>, String>),
    Opened(u64, Result<Opened, String>),
    Checked(u64, Result<Checked, String>),
    Plot(usize, PlotUiMessage),
    /// Something in the table of the panel with this number.
    Table(usize, TableMsg),
    /// The Day / Week switch, for panels that use `{period}`.
    Period(Period),
    /// The Settings button while the Dashboard is open.
    OpenSettings,
    CloseSettings,
    /// The shown table's lag settings, to fill the form with.
    TuningRead(Result<Option<Tuning>, String>),
    SettingEdited(LagField, String),
    /// Save: the app keeps the check interval in config.toml; the daemon's
    /// settings go next to every table at the location.
    SaveSettings,
    /// How many tables got the daemon's settings.
    SettingsSaved(Result<usize, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LagField {
    Check,
    Every,
    MinRows,
    MaxWait,
}

/// The dashboard settings as edited, kept as text until saved: how often
/// this window looks for a commit, and the daemon's lag settings (see
/// `analytics_reader::tuning`), empty for the daemon's own value.
pub struct LagForm {
    check: String,
    every: String,
    min_rows: String,
    max_wait: String,
    saving: bool,
    /// The outcome of the last save: success, and what it said.
    said: Option<(bool, String)>,
}

impl LagForm {
    fn field(&mut self, which: LagField) -> &mut String {
        match which {
            LagField::Check => &mut self.check,
            LagField::Every => &mut self.every,
            LagField::MinRows => &mut self.min_rows,
            LagField::MaxWait => &mut self.max_wait,
        }
    }

    /// The check interval and the daemon's settings, or what is wrong.
    fn parse(&self) -> Result<(u64, Tuning), String> {
        let number = |name: &str, v: &str, at_least: u64| -> Result<Option<u64>, String> {
            match v.trim() {
                "" => Ok(None),
                v => match v.parse::<u64>() {
                    Ok(n) if n >= at_least => Ok(Some(n)),
                    _ => Err(format!("{name}: a whole number of at least {at_least}, or empty")),
                },
            }
        };
        let check = number("This window looks every", &self.check, 1)?.ok_or("This window looks every: a number of seconds")?;
        Ok((
            check,
            Tuning {
                every_seconds: number("Check the database every", &self.every, 1)?,
                min_rows: number("Commit at", &self.min_rows, 1)?,
                max_wait_seconds: number("…or after", &self.max_wait, 0)?,
            },
        ))
    }
}

/// The table widget takes plain functions, so its messages say nothing of
/// the panel; [`Msg::Table`] adds it.
#[derive(Debug, Clone)]
pub enum TableMsg {
    Synced(iced::widget::scrollable::AbsoluteOffset),
    HeaderPressed(usize),
    ColumnDragged(usize, f32),
    ColumnReleased,
}

pub struct Dashboard {
    /// Where the tables are, as typed.
    pub location: String,
    /// `key=value` object store options, from the settings.
    pub storage_options: Vec<String>,
    /// The table shown, by name.
    pub selected: Option<String>,
    entries: Vec<Entry>,
    session: Option<Shared>,
    data: Option<Arc<Data>>,
    status: Option<Status>,
    error: Option<String>,
    /// A listing, open or check is running.
    busy: bool,
    /// Counts requests, so the answer to a replaced one is ignored.
    request: u64,
    /// The version last loaded or tried, so a version that fails to load
    /// isn't retried at every check.
    tried: Option<i64>,
    plots: Vec<Option<PlotWidget>>,
    bars: Vec<Option<Bars>>,
    columns: Vec<Option<Columns>>,
    tables: Vec<Option<TableState>>,
    /// What `{period}` stands for in the panels that use it.
    period: Period,
    /// How often to look for a new commit, in seconds.
    pub check_seconds: u64,
    /// The dashboard settings, while they are open.
    settings: Option<LagForm>,
}

impl Dashboard {
    pub fn new(location: String, storage_options: Vec<String>, selected: Option<String>) -> Dashboard {
        Dashboard {
            location,
            storage_options,
            selected,
            entries: Vec::new(),
            session: None,
            data: None,
            status: None,
            error: None,
            busy: false,
            request: 0,
            tried: None,
            plots: Vec::new(),
            bars: Vec::new(),
            columns: Vec::new(),
            tables: Vec::new(),
            period: Period::Day,
            check_seconds: 2,
            settings: None,
        }
    }

    fn root(&self) -> Result<Location, String> {
        let options = location::parse_options(&self.storage_options).map_err(|e| format!("{e:#}"))?;
        Location::parse(&self.location, options).map_err(|e| format!("{e:#}"))
    }

    fn next_request(&mut self) -> u64 {
        self.request += 1;
        self.busy = true;
        self.request
    }

    /// Lists the tables at the location, then opens the selected one (or
    /// the first).
    pub fn reload(&mut self) -> Task<Msg> {
        let id = self.next_request();
        let root = self.root();
        Task::perform(
            async move {
                let root = root?;
                catalog::list(&root).await.map_err(|e| format!("{e:#}"))
            },
            move |result| Msg::Listed(id, result),
        )
    }

    fn open(&mut self, name: String) -> Task<Msg> {
        self.selected = Some(name.clone());
        self.session = None;
        self.tried = None;
        let id = self.next_request();
        let location = self.root().map(|root| root.join(&name));
        let period = self.period;
        Task::perform(
            async move {
                let session = Session::open(&location?).await.map_err(|e| format!("{e:#}"))?;
                let data = dashboard::load_with(&session, period).await.map_err(|e| format!("{e:#}"))?;
                let status = session.status().await.ok().flatten();
                Ok(Opened { session: Shared(Arc::new(Mutex::new(session))), data: Arc::new(data), status })
            },
            move |result| Msg::Opened(id, result),
        )
    }

    fn check(&mut self) -> Task<Msg> {
        let Some(Shared(session)) = self.session.clone() else {
            return Task::none();
        };
        let tried = self.tried;
        let period = self.period;
        let id = self.next_request();
        Task::perform(
            async move {
                let mut session = session.lock().await;
                let status = session.status().await.ok().flatten();
                let version = session.latest_version().await.map_err(|e| format!("{e:#}"))?;
                let data = if Some(version) != tried {
                    let loaded = async {
                        session.refresh().await?;
                        dashboard::load_with(&session, period).await
                    };
                    Some(loaded.await.map(Arc::new).map_err(|e| format!("{e:#}")))
                } else {
                    None
                };
                Ok(Checked { status, version, data })
            },
            move |result| Msg::Checked(id, result),
        )
    }

    pub fn update(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::LocationEdited(location) => self.location = location,
            Msg::LocationSubmitted => {
                self.selected = None;
                self.clear();
                return self.reload();
            }
            Msg::Select(name) => {
                self.clear();
                return self.open(name);
            }
            Msg::RefreshNow => {
                self.tried = None;
                return self.reload();
            }
            Msg::Check => {
                if !self.busy {
                    return self.check();
                }
            }
            Msg::Period(period) => {
                self.period = period;
                // Load the same version again with the new period; if a
                // request is running, the next check does it.
                self.tried = None;
                if !self.busy {
                    return self.check();
                }
            }
            Msg::OpenSettings => {
                let in_force = self.status.as_ref().filter(|s| s.min_rows > 0);
                self.settings = Some(LagForm {
                    check: self.check_seconds.to_string(),
                    every: in_force.map(|s| (s.every_ms / 1000).to_string()).unwrap_or_default(),
                    min_rows: in_force.map(|s| s.min_rows.to_string()).unwrap_or_default(),
                    max_wait: in_force.map(|s| (s.max_wait_ms / 1000).to_string()).unwrap_or_default(),
                    saving: false,
                    said: None,
                });
                // What the table's file says wins over the heartbeat.
                let (Ok(root), Some(name)) = (self.root(), self.selected.clone()) else { return Task::none() };
                return Task::perform(
                    async move { Tuning::read_at(&root.join(&name)).await.map_err(|e| format!("{e:#}")) },
                    Msg::TuningRead,
                );
            }
            Msg::CloseSettings => self.settings = None,
            Msg::TuningRead(result) => {
                if let (Some(form), Ok(Some(t))) = (&mut self.settings, result) {
                    let text = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_default();
                    form.every = text(t.every_seconds);
                    form.min_rows = text(t.min_rows);
                    form.max_wait = text(t.max_wait_seconds);
                }
            }
            Msg::SettingEdited(which, value) => {
                if let Some(form) = &mut self.settings {
                    *form.field(which) = value;
                    form.said = None;
                }
            }
            Msg::SaveSettings => {
                let Some(form) = &mut self.settings else { return Task::none() };
                let (check, tuning) = match form.parse() {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        form.said = Some((false, e));
                        return Task::none();
                    }
                };
                self.check_seconds = check;
                let root = match self.root() {
                    Ok(root) => root,
                    Err(e) => {
                        if let Some(form) = &mut self.settings {
                            form.said = Some((false, e));
                        }
                        return Task::none();
                    }
                };
                if let Some(form) = &mut self.settings {
                    form.saving = true;
                }
                let names: Vec<String> = self.entries.iter().map(|e| e.name.clone()).collect();
                return Task::perform(
                    async move {
                        for name in &names {
                            tuning.write_at(&root.join(name)).await.map_err(|e| format!("{name}: {e:#}"))?;
                        }
                        Ok(names.len())
                    },
                    Msg::SettingsSaved,
                );
            }
            Msg::SettingsSaved(result) => {
                if let Some(form) = &mut self.settings {
                    form.saving = false;
                    form.said = Some(match result {
                        Ok(0) => (true, "Saved. There are no tables here yet for the daemon's settings.".into()),
                        Ok(n) => (
                            true,
                            format!("Saved. The daemon takes up its settings for the {n} table{} at its next check.", if n == 1 { "" } else { "s" }),
                        ),
                        Err(e) => (false, format!("The daemon's settings could not be written: {e}")),
                    });
                }
            }
            Msg::Listed(id, _) | Msg::Opened(id, _) | Msg::Checked(id, _) if id != self.request => {}
            Msg::Listed(_, result) => {
                self.busy = false;
                match result {
                    Ok(entries) => {
                        self.entries = entries;
                        let keep = self.selected.clone().filter(|s| self.entries.iter().any(|e| &e.name == s));
                        let name = keep.or_else(|| self.entries.first().map(|e| e.name.clone()));
                        match name {
                            // Already open: just look for a new version.
                            Some(name) if self.session.is_some() && self.selected.as_ref() == Some(&name) => {
                                return self.check();
                            }
                            Some(name) => return self.open(name),
                            None => {
                                self.clear();
                                self.error = Some(format!(
                                    "No tables at {} yet. Start the canton-analytics daemon with --tables pointing here.",
                                    self.location.trim()
                                ));
                            }
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::Opened(_, result) => {
                self.busy = false;
                match result {
                    Ok(opened) => {
                        self.session = Some(opened.session);
                        self.status = opened.status;
                        self.show(opened.data);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::Checked(_, result) => {
                self.busy = false;
                match result {
                    Ok(checked) => {
                        self.status = checked.status;
                        match checked.data {
                            Some(Ok(data)) => self.show(data),
                            Some(Err(e)) => {
                                self.tried = Some(checked.version);
                                self.error = Some(e);
                            }
                            None => {}
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::Plot(i, m) => {
                if let Some(Some(plot)) = self.plots.get_mut(i) {
                    plot.update(m);
                }
            }
            Msg::Table(i, m) => {
                let Some(Some(t)) = self.tables.get_mut(i) else { return Task::none() };
                match m {
                    TableMsg::Synced(offset) => return iced::widget::operation::scroll_to(t.header.clone(), offset),
                    TableMsg::HeaderPressed(column) => t.sort_by(column),
                    TableMsg::ColumnDragged(column, offset) => {
                        if let Some(c) = t.columns.get_mut(column) {
                            c.resize_offset = Some(offset);
                        }
                    }
                    TableMsg::ColumnReleased => {
                        for c in &mut t.columns {
                            if let Some(offset) = c.resize_offset.take() {
                                c.width = (c.width + offset).max(40.0);
                            }
                        }
                    }
                }
            }
        }
        Task::none()
    }

    fn clear(&mut self) {
        self.session = None;
        self.data = None;
        self.status = None;
        self.error = None;
        self.tried = None;
        self.plots.clear();
        self.bars.clear();
        self.columns.clear();
        self.tables.clear();
    }

    /// Shows new panels, keeping a table's sort and column widths when the
    /// same panel comes back.
    fn show(&mut self, data: Arc<Data>) {
        self.error = None;
        self.tried = data.version;
        let mut old: HashMap<String, TableState> = HashMap::new();
        if let Some(previous) = &self.data {
            for (p, t) in previous.panels.iter().zip(self.tables.drain(..)) {
                if let Some(t) = t {
                    old.insert(p.panel.title.clone(), t);
                }
            }
        }
        self.plots = data.panels.iter().map(time_plot).collect();
        self.bars = data.panels.iter().map(Bars::new).collect();
        self.columns = data.panels.iter().map(Columns::new).collect();
        self.tables = (0..data.panels.len()).map(|i| TableState::new(&data, i, old.remove(&data.panels[i].panel.title))).collect();
        self.data = Some(data);
    }

    pub fn view(&self) -> Element<'_, Msg> {
        let names: Vec<String> = self.entries.iter().map(|e| e.name.clone()).collect();
        let header = row![
            text("Tables").size(13),
            text_input("tables, or s3://bucket/canton-analytics", &self.location)
                .on_input(Msg::LocationEdited)
                .on_submit(Msg::LocationSubmitted)
                .size(13)
                .width(300),
            pick_list(names, self.selected.clone(), Msg::Select).placeholder("table").text_size(13),
            button(text("Refresh now").size(12)).style(button::secondary).on_press(Msg::RefreshNow),
            text(match (&self.data, self.busy) {
                (None, true) => "Loading…".to_string(),
                (Some(d), _) => d.version.map(|v| format!("version {v}")).unwrap_or_default(),
                (None, false) => String::new(),
            })
            .size(12)
            .color(MUTED),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        // Day / Week, when a panel of this table lets the viewer pick.
        let header = match &self.data {
            Some(d) if d.panels.iter().any(|p| dashboard::uses_period(&p.panel)) => {
                let choice = |label: &'static str, period: Period| {
                    button(text(label).size(12))
                        .style(if self.period == period { button::primary } else { button::secondary })
                        .on_press(Msg::Period(period))
                };
                header.push(
                    row![text("Per").size(12).color(MUTED), choice("Day", Period::Day), choice("Week", Period::Week)]
                        .spacing(4)
                        .align_y(Alignment::Center),
                )
            }
            _ => header,
        };

        let mut col = column![header].spacing(8).padding(10).width(Fill).height(Fill);
        if let Some(status) = self.status_view() {
            col = col.push(status);
        }
        if let Some(e) = &self.error {
            col = col.push(text(e).size(12).color(ERROR));
        }
        let body: Element<'_, Msg> = match &self.data {
            Some(data) => self.panels_view(data),
            None if self.busy => container(text("Loading…").size(13).color(MUTED)).padding(12).into(),
            None => container(
                text(
                    "The dashboard shows the tables the canton-analytics daemon writes. \
                     Point the box above at its --tables location and press Enter.",
                )
                .size(12)
                .color(MUTED),
            )
            .padding(12)
            .into(),
        };
        col.push(body).into()
    }

    /// The dashboard settings, beside the dashboard while they are open.
    pub fn settings_view(&self) -> Option<Element<'_, Msg>> {
        let form = self.settings.as_ref()?;
        let field = |label: &'static str, unit: &'static str, value: &str, which: LagField| {
            row![
                text(label).size(13).width(Fill),
                text_input("", value).on_input(move |v| Msg::SettingEdited(which, v)).size(13).width(80),
                text(unit).size(12).color(MUTED).width(36),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
        };
        let heading = |label: &'static str| text(label).size(14).font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT });
        let note = |s: String| text(s).size(11).color(MUTED);
        let tables = self.entries.len();

        let mut col = iced::widget::Column::new().spacing(10).padding(16).width(SIDEBAR_WIDTH);
        col = col.push(
            row![
                text("Dashboard settings").size(18).width(Fill),
                button(text("Back").size(12)).style(button::secondary).on_press(Msg::CloseSettings),
            ]
            .align_y(Alignment::Center),
        );
        col = col.push(note(
            "A new ledger row shows up here once the daemon has seen it, committed it to its table, and this window has looked for the commit."
                .into(),
        ));

        col = col.push(heading("This window"));
        col = col.push(field("Look for new commits every", "s", &form.check, LagField::Check));
        col = col.push(note("Saved in config.toml.".into()));

        col = col.push(heading("The daemon"));
        col = col.push(field("Check the database every", "s", &form.every, LagField::Every));
        col = col.push(field("Commit at", "rows", &form.min_rows, LagField::MinRows));
        col = col.push(field("…or once the oldest has waited", "s", &form.max_wait, LagField::MaxWait));
        col = col.push(note(format!(
            "Saved next to {} at this location; the daemon takes them up at its next check, without a restart. Empty: the daemon's own setting. Committing less often keeps fewer, bigger files.",
            match tables {
                1 => "the table".to_string(),
                n => format!("all {n} tables"),
            }
        )));
        if let Some(s) = self.status.as_ref().filter(|s| s.min_rows > 0) {
            col = col.push(note(format!(
                "In force now for {}: every {} s, at {} rows or after {} s.",
                self.selected.as_deref().unwrap_or("this table"),
                s.every_ms / 1000,
                s.min_rows,
                s.max_wait_ms / 1000
            )));
        }
        if let Ok((check, tuning)) = form.parse() {
            let every = tuning.every_seconds.or(self.status.as_ref().map(|s| (s.every_ms / 1000) as u64));
            let wait = tuning.max_wait_seconds.or(self.status.as_ref().filter(|s| s.min_rows > 0).map(|s| (s.max_wait_ms / 1000) as u64));
            if let (Some(every), Some(wait)) = (every, wait) {
                col = col.push(text(format!("On a quiet ledger, new rows show up within about {} s.", every + wait + check)).size(12));
            }
        }
        let save = button(text(if form.saving { "Saving…" } else { "Save" }).size(13)).style(button::primary);
        col = col.push(if form.saving { save } else { save.on_press(Msg::SaveSettings) });
        if let Some((ok, said)) = &form.said {
            col = col.push(text(said).size(12).color(if *ok { OK } else { ERROR }));
        }
        Some(scrollable(col).height(Fill).into())
    }

    /// The daemon's heartbeat, in words.
    fn status_view(&self) -> Option<Element<'_, Msg>> {
        let data = self.data.as_ref()?;
        let title = if data.description.is_empty() { data.title.clone() } else { format!("{}: {}", data.title, data.description) };
        let (color, said) = match &self.status {
            None => (MUTED, "no heartbeat from the daemon yet".to_string()),
            Some(s) => {
                let ago = ago((now_ms() - s.updated_at_ms).max(0) / 1000);
                if let Some(e) = &s.error {
                    (ERROR, format!("failing ({ago}): {e}"))
                } else if s.stopped {
                    (MUTED, format!("daemon stopped {ago}, at {}", s.committed))
                } else if !s.alive(now_ms()) {
                    (WARN, format!("daemon not heard from since {ago}"))
                } else if s.behind() > 0 {
                    (WARN, format!("catching up: at {} of {} ({} to go)", s.committed, s.high_water, s.behind()))
                } else {
                    (OK, format!("up to date at {} (checked {ago})", s.committed))
                }
            }
        };
        Some(
            // The heartbeat first, so a long title never hides it.
            row![text("●").size(12).color(color), text(said).size(12).color(color), text(title).size(12).color(MUTED)]
                .spacing(6)
                .align_y(Alignment::Center)
                .into(),
        )
    }

    fn panels_view<'a>(&'a self, data: &'a Data) -> Element<'a, Msg> {
        // Stats and tables take the whole width; charts go two to a row.
        let mut rows = column![].spacing(10);
        let mut charts: Vec<Element<'a, Msg>> = Vec::new();
        for (i, p) in data.panels.iter().enumerate() {
            let wide = matches!(p.panel.kind, PanelKind::Stats | PanelKind::Table);
            let height = match p.panel.kind {
                PanelKind::Stats => Length::Shrink,
                PanelKind::Table => Length::Fixed(380.0),
                _ => Length::Fixed(300.0),
            };
            let card = self.panel_view(i, p, height);
            if wide {
                if !charts.is_empty() {
                    rows = rows.push(row(std::mem::take(&mut charts)).spacing(10));
                }
                rows = rows.push(card);
            } else {
                charts.push(card);
                if charts.len() == 2 {
                    rows = rows.push(row(std::mem::take(&mut charts)).spacing(10));
                }
            }
        }
        if !charts.is_empty() {
            rows = rows.push(row(charts).spacing(10));
        }
        // Room on the right for the scroll bar.
        scrollable(rows.padding(iced::Padding { right: 12.0, ..iced::Padding::ZERO })).height(Fill).into()
    }

    fn panel_view<'a>(&'a self, i: usize, p: &'a PanelData, height: Length) -> Element<'a, Msg> {
        let mut title = p.panel.title.clone();
        if let Some(b) = p.bucket_seconds {
            title.push_str(&format!(" (per {})", bucket_name(b)));
        }
        if p.truncated {
            title.push_str(" (first rows)");
        }
        let content: Element<'a, Msg> = match &p.frame {
            Err(e) => text(e).size(12).color(ERROR).into(),
            Ok(frame) if frame.rows() == 0 => empty("No rows yet."),
            Ok(frame) => match p.panel.kind {
                PanelKind::Stats => stats(frame),
                PanelKind::Timeseries => match &self.plots[i] {
                    Some(plot) => plot.view().map(move |m| Msg::Plot(i, m)),
                    None => empty("No time or number column to draw."),
                },
                PanelKind::Bar => match (&self.columns[i], &self.bars[i]) {
                    (Some(columns), _) => ChartWidget::new(columns).width(Fill).height(Fill).into(),
                    (None, Some(bars)) => ChartWidget::new(bars).width(Fill).height(Fill).into(),
                    (None, None) => empty("Needs a label column and a number column."),
                },
                PanelKind::Table => match &self.tables[i] {
                    Some(t) => t.view().map(move |m| Msg::Table(i, m)),
                    None => empty("No rows yet."),
                },
            },
        };
        let title = text(title).size(13).font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT });
        // How long the panel's query took, so the dashboard's speed shows.
        let mut note = format!("{} ms", p.millis);
        if p.panel.kind == PanelKind::Timeseries {
            note.push_str(" · drag to pan · Ctrl/⌘ + scroll to zoom · double-click to show all");
        }
        let title: Element<'a, Msg> =
            row![title, text(note).size(11).color(MUTED)].spacing(10).align_y(Alignment::Center).into();
        container(column![title, container(content).width(Fill).height(Fill)].spacing(6))
            .padding(8)
            .width(Fill)
            .height(height)
            .style(container::bordered_box)
            .into()
    }
}

fn empty<'a>(message: &'a str) -> Element<'a, Msg> {
    container(text(message).size(12).color(MUTED)).center(Fill).into()
}

/// One card per column, from the first row.
fn stats(frame: &Frame) -> Element<'_, Msg> {
    let cards = frame.columns.iter().map(|c| {
        // Plain figures with their label, not boxes: boxes read as buttons.
        container(column![text(value(c, 0)).size(24).color(ACCENT), text(c.name.clone()).size(12).color(MUTED)].spacing(0))
            .padding([2, 0])
            .into()
    });
    scrollable(row(cards).spacing(36)).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new())).into()
}

/// A value as text, in its column's format.
fn value(c: &Column, row: usize) -> String {
    let dash = || "–".to_string();
    match (c.effective_format(), &c.values) {
        (Format::Timestamp, Values::Time(v)) => v[row].map(|us| time_label(us.div_euclid(1_000_000), 1)).unwrap_or_else(dash),
        (Format::Timestamp, values) => values.number(row).map(|s| time_label(s as i64, 1)).unwrap_or_else(dash),
        (Format::DurationSeconds, values) => values.number(row).map(duration).unwrap_or_else(dash),
        (Format::Number, values) => values.number(row).map(number).unwrap_or_else(dash),
        (_, values) => values.text(row).unwrap_or_else(dash),
    }
}

fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else if n.abs() >= 100.0 {
        format!("{n:.1}")
    } else {
        format!("{n:.3}").trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// A time series: the first time column across, every number column as a
/// line. Without a time column, the first number column (such as an offset)
/// goes across instead.
fn time_plot(p: &PanelData) -> Option<PlotWidget> {
    if p.panel.kind != PanelKind::Timeseries {
        return None;
    }
    let frame = p.frame.as_ref().ok()?;
    let x_index = x_column_index(frame, p.panel.time_column.as_deref())?;
    let x_column = &frame.columns[x_index];
    let time_x = matches!(x_column.values, Values::Time(_)).then_some(x_index);
    let xs: Vec<Option<f64>> = match &x_column.values {
        Values::Time(v) => v.iter().map(|t| t.map(|us| us as f64 / 1e6)).collect(),
        values => (0..frame.rows()).map(|r| values.number(r)).collect(),
    };
    let (first, last) = xs.iter().flatten().fold((f64::MAX, f64::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    if first > last {
        return None;
    }
    let time = time_x.is_some();
    let bucket = if time {
        p.bucket_seconds.unwrap_or(3_600) as f64
    } else {
        // The gap between neighbouring points, for the margins.
        let mut sorted: Vec<f64> = xs.iter().flatten().copied().collect();
        sorted.sort_by(f64::total_cmp);
        sorted.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0).fold(f64::MAX, f64::min).min(last - first).max(1.0)
    };
    let mut builder = PlotWidgetBuilder::new();
    let mut top: f64 = 0.0;
    let mut lines = 0;
    let lines_of = frame.columns.iter().enumerate().filter(|(i, c)| *i != x_index && matches!(c.values, Values::Number(_)));
    for (k, (_, c)) in lines_of.enumerate() {
        let points: Vec<[f64; 2]> =
            (0..frame.rows()).filter_map(|r| Some([xs[r]?, c.values.number(r)?])).collect();
        top = points.iter().map(|p| p[1]).fold(top, f64::max);
        // Markers only while they can be told apart.
        let marker = if points.len() <= 40 { 4.0 } else { 0.0 };
        let series = Series::new(points, MarkerStyle::circle(marker), LineStyle::solid().with_pixel_width(2.0))
            .with_label(c.name.clone())
            .with_color(SERIES[k % SERIES.len()]);
        builder = builder.add_series(series);
        lines += 1;
    }
    if lines == 0 {
        return None;
    }
    let builder = builder.with_x_lim(first - bucket / 2.0, last + bucket / 2.0).with_y_lim(0.0, (top * 1.15).max(1.0));
    let builder = if time {
        // Ticks follow the visible range (it changes as the user zooms), not
        // the data's: a step fixed from the data gave one tick per minute over
        // a zoomed-out view, and a label widget for each.
        builder
            .with_x_tick_producer(|min, max| axis_ticks(min, max, tick_step(max - min)))
            .with_x_tick_formatter(|tick| short_time_label(tick.value as i64, tick.step_size))
            .with_hover_highlight_provider(move |_, point| {
                Some(format!("{}: {}", time_label(point.x as i64, bucket as i64), number(point.y)))
            })
    } else {
        // Offsets and other counts are whole numbers: no ticks between them.
        let whole = xs.iter().flatten().all(|x| x.fract() == 0.0);
        let floor = if whole { 1.0 } else { 0.0 };
        let name = x_column.name.clone();
        builder
            .with_x_tick_producer(move |min, max| axis_ticks(min, max, round_step((max - min) / 5.0).max(floor)))
            .with_x_tick_formatter(|tick| short_number(tick.value))
            .with_hover_highlight_provider(move |_, point| {
                Some(format!("{name} {}: {}", number(point.x), number(point.y)))
            })
    };
    let mut builder = builder
        .with_y_tick_formatter(|tick| number(tick.value + 0.0))
        .with_tick_label_size(11.0)
        .with_controls(controls())
        .disable_controls_help();
    if lines == 1 {
        builder = builder.disable_legend();
    }
    builder.build().ok()
}

/// Charts sit in a scrolling page, so the scroll wheel scrolls the page.
/// Ctrl (or Cmd) + scroll zooms, dragging pans, right-dragging zooms to a
/// box, and a double click shows all the data again.
fn controls() -> iced_plot::PlotControls {
    use iced::keyboard::Modifiers;
    use iced_plot::ScrollAction;
    let mut controls = iced_plot::PlotControls::default();
    controls.unbind_scroll(Modifiers::NONE);
    controls.bind_scroll(Modifiers::LOGO, ScrollAction::Zoom);
    controls
}

/// The column across: the panel's `time_column` when the result has it,
/// else the first time column, else the first number column.
fn x_column_index(frame: &Frame, time_column: Option<&str>) -> Option<usize> {
    let plottable = |c: &Column| matches!(c.values, Values::Time(_) | Values::Number(_));
    time_column
        .and_then(|name| frame.columns.iter().position(|c| c.name == name && plottable(c)))
        .or_else(|| frame.columns.iter().position(|c| matches!(c.values, Values::Time(_))))
        .or_else(|| frame.columns.iter().position(|c| matches!(c.values, Values::Number(_))))
}

/// The smallest 1, 2 or 5 times a power of ten that is at least `at_least`.
fn round_step(at_least: f64) -> f64 {
    let power = 10f64.powf(at_least.log10().floor());
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * power).find(|s| *s >= at_least).unwrap_or(10.0 * power)
}

/// `1.2M`, `350k`, `42`: short enough for an axis.
fn short_number(n: f64) -> String {
    let trim = |s: String| s.trim_end_matches('0').trim_end_matches('.').to_string();
    match n.abs() {
        a if a >= 1e9 => format!("{}G", trim(format!("{:.2}", n / 1e9))),
        a if a >= 1e6 => format!("{}M", trim(format!("{:.2}", n / 1e6))),
        a if a >= 1e3 => format!("{}k", trim(format!("{:.1}", n / 1e3))),
        _ => number(n),
    }
}

/// A tick spacing that gives at most about six ticks over `span` seconds.
fn tick_step(span: f64) -> f64 {
    const STEPS: [f64; 19] = [
        1.0, 5.0, 10.0, 15.0, 30.0, 60.0, 300.0, 900.0, 1_800.0, 3_600.0, 10_800.0, 21_600.0, 43_200.0, 86_400.0, 172_800.0, 604_800.0,
        1_209_600.0, 2_592_000.0, 7_776_000.0,
    ];
    STEPS.into_iter().find(|s| span / s <= 6.0).unwrap_or((span / 6.0 / 86_400.0).ceil() * 86_400.0)
}

/// The most ticks an axis gets, whatever the range and step. Each tick is a
/// label widget built on every redraw, so a runaway count eats memory fast.
const MAX_TICKS: usize = 50;

/// Ticks `step` apart over `min..=max`. A step too small for the range is
/// doubled until at most [`MAX_TICKS`] fit; a range or step that isn't a
/// positive finite number gives none.
fn axis_ticks(min: f64, max: f64, step: f64) -> Vec<iced_plot::Tick> {
    if !(min.is_finite() && max.is_finite() && max > min && step.is_finite() && step > 0.0) {
        return Vec::new();
    }
    let mut step = step;
    while (max - min) / step > MAX_TICKS as f64 {
        step *= 2.0;
    }
    let first = (min / step).ceil();
    // Counted rather than added up, so a step below the float resolution of
    // `min` can't stall the loop.
    (0..=MAX_TICKS)
        .map(|i| (first + i as f64) * step)
        .take_while(|&value| value <= max)
        .map(|value| iced_plot::Tick { value, step_size: step, line_type: iced_plot::TickWeight::Major })
        .collect()
}

/// `11-02 09:00` for ticks under a day apart, `2026-11-02` otherwise (UTC).
fn short_time_label(seconds: i64, step: f64) -> String {
    if step < 60.0 {
        let label = time_label(seconds, 1);
        label.get(11..).unwrap_or(&label).to_string()
    } else if step < 86_400.0 {
        let label = time_label(seconds, 60);
        label.get(5..).unwrap_or(&label).to_string()
    } else {
        time_label(seconds, 86_400)
    }
}

fn bucket_name(seconds: i64) -> String {
    match seconds {
        s if s % 86_400 == 0 && s / 86_400 == 1 => "day".into(),
        s if s % 604_800 == 0 && s / 604_800 == 1 => "week".into(),
        s if s % 86_400 == 0 => format!("{} days", s / 86_400),
        3_600 => "hour".into(),
        s if s % 3_600 == 0 => format!("{} hours", s / 3_600),
        60 => "minute".into(),
        1 => "second".into(),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}

/// Horizontal bars: the first text column as labels, the first number
/// column as lengths, top row on top.
pub struct Bars {
    labels: Vec<String>,
    values: Vec<Option<f64>>,
    /// Bars of durations are drawn in a unit that suits them.
    unit: f64,
    axis: String,
}

impl Bars {
    fn new(p: &PanelData) -> Option<Bars> {
        if p.panel.kind != PanelKind::Bar {
            return None;
        }
        let frame = p.frame.as_ref().ok()?;
        let label = frame.columns.iter().find(|c| !matches!(c.values, Values::Number(_)))?;
        let value_column = frame.columns.iter().find(|c| matches!(c.values, Values::Number(_)))?;
        let rows = frame.rows().min(15);
        let values: Vec<Option<f64>> = (0..rows).map(|r| value_column.values.number(r)).collect();
        let max = values.iter().flatten().fold(0.0, |m: f64, v| m.max(*v));
        let (unit, axis) = if value_column.effective_format() == Format::DurationSeconds {
            let (unit, name) = unit_for(max);
            (unit, format!("{}, {name}", value_column.name))
        } else {
            (1.0, value_column.name.clone())
        };
        Some(Bars { labels: (0..rows).map(|r| value(label, r)).collect(), values, unit, axis })
    }
}

impl Chart<Msg> for &Bars {
    type State = ();

    fn build_chart<DB: DrawingBackend>(&self, _: &(), mut builder: ChartBuilder<DB>) {
        let n = self.labels.len() as i32;
        let max = self.values.iter().flatten().fold(0.0, |m: f64, v| m.max(*v)).max(f64::MIN_POSITIVE) / self.unit;
        let lo = self.values.iter().flatten().fold(0.0, |m: f64, v| m.min(*v)) / self.unit;
        let Ok(mut chart) = builder
            .margin(8)
            .x_label_area_size(42)
            .y_label_area_size(190)
            .build_cartesian_2d(lo * 1.1..max * 1.1, (0..n).into_segmented())
        else {
            return;
        };
        let label_style = ("sans-serif", 12).into_font().color(&RGBColor(90, 90, 90));
        let _ = chart
            .configure_mesh()
            .disable_y_mesh()
            .y_labels(n as usize)
            .y_label_formatter(&|v| match v {
                SegmentValue::CenterOf(y) | SegmentValue::Exact(y) => {
                    self.labels.get((n - 1 - y) as usize).map(|l| short_id(l)).unwrap_or_default()
                }
                SegmentValue::Last => String::new(),
            })
            .x_desc(self.axis.clone())
            .label_style(label_style.clone())
            .axis_desc_style(label_style)
            .light_line_style(RGBColor(235, 235, 235))
            .bold_line_style(RGBColor(220, 220, 220))
            .draw();
        let bar = RGBColor(46, 107, 199);
        let _ = chart.draw_series(self.values.iter().enumerate().filter_map(|(i, v)| {
            let y = n - 1 - i as i32;
            let x = (*v)? / self.unit;
            let mut r = Rectangle::new([(0.0, SegmentValue::Exact(y)), (x, SegmentValue::Exact(y + 1))], bar.mix(0.8).filled());
            r.set_margin(4, 4, 0, 0);
            Some(r)
        }));
    }
}

/// Columns over time: a bar panel whose label column is a time draws one
/// upright column per row, left to right, labelled by date, or by ISO week
/// number when the rows are weeks.
pub struct Columns {
    /// Each row's start, in seconds.
    starts: Vec<i64>,
    values: Vec<Option<f64>>,
    /// The rows' spacing, in seconds.
    step: i64,
}

impl Columns {
    fn new(p: &PanelData) -> Option<Columns> {
        if p.panel.kind != PanelKind::Bar {
            return None;
        }
        let frame = p.frame.as_ref().ok()?;
        let label = frame.columns.iter().find(|c| !matches!(c.values, Values::Number(_)))?;
        let Values::Time(times) = &label.values else {
            return None;
        };
        let value_column = frame.columns.iter().find(|c| matches!(c.values, Values::Number(_)))?;
        let starts: Vec<i64> = times.iter().map(|t| t.map(|us| us.div_euclid(1_000_000))).collect::<Option<_>>()?;
        let values = (0..starts.len()).map(|r| value_column.values.number(r)).collect();
        let step = p.bucket_seconds.or_else(|| starts.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0).min()).unwrap_or(86_400);
        Some(Columns { starts, values, step })
    }

    /// Rows a week apart that start on Mondays.
    fn weeks(&self) -> bool {
        self.step == 604_800 && self.starts.iter().all(|s| (s.div_euclid(86_400) + 3).rem_euclid(7) == 0)
    }

    /// `W48`, with the year on the first column and on each year's first
    /// week; a date for days; a time for anything shorter.
    fn label(&self, i: usize) -> String {
        let Some(&start) = self.starts.get(i) else {
            return String::new();
        };
        if self.weeks() {
            let (year, week) = iso_week(start.div_euclid(86_400));
            if i == 0 || week == 1 { format!("{year} W{week:02}") } else { format!("W{week:02}") }
        } else {
            time_label(start, self.step)
        }
    }
}

impl Chart<Msg> for &Columns {
    type State = ();

    fn build_chart<DB: DrawingBackend>(&self, _: &(), mut builder: ChartBuilder<DB>) {
        let n = self.starts.len() as i32;
        let max = self.values.iter().flatten().fold(0.0, |m: f64, v| m.max(*v)).max(f64::MIN_POSITIVE);
        let lo = self.values.iter().flatten().fold(0.0, |m: f64, v| m.min(*v));
        let Ok(mut chart) = builder
            .margin(8)
            .x_label_area_size(36)
            .y_label_area_size(64)
            .build_cartesian_2d((0..n).into_segmented(), lo * 1.1..max * 1.1)
        else {
            return;
        };
        let label_style = ("sans-serif", 12).into_font().color(&RGBColor(90, 90, 90));
        // Week numbers are short, so more of them fit than dates.
        let labels = if self.weeks() { 27 } else { 14 };
        let _ = chart
            .configure_mesh()
            .disable_x_mesh()
            .x_labels(labels)
            .x_label_formatter(&|v| match v {
                SegmentValue::CenterOf(x) | SegmentValue::Exact(x) => self.label(*x as usize),
                SegmentValue::Last => String::new(),
            })
            // No axis name: the panel's title already says what is counted,
            // and a side label crowds the numbers in a narrow panel.
            .y_label_formatter(&|v| short_number(*v))
            .label_style(label_style.clone())
            .axis_desc_style(label_style)
            .light_line_style(RGBColor(235, 235, 235))
            .bold_line_style(RGBColor(220, 220, 220))
            .draw();
        let bar = RGBColor(46, 107, 199);
        let gap = if n > 120 { 0 } else { 2 };
        let _ = chart.draw_series(self.values.iter().enumerate().filter_map(|(i, v)| {
            let x = i as i32;
            let mut r = Rectangle::new([(SegmentValue::Exact(x), 0.0), (SegmentValue::Exact(x + 1), (*v)?)], bar.mix(0.8).filled());
            r.set_margin(0, 0, gap, gap);
            Some(r)
        }));
    }
}

/// The ISO 8601 year and week number of a day (days since 1970-01-01):
/// weeks start on Monday, and week 1 holds the year's first Thursday.
fn iso_week(day: i64) -> (i64, i64) {
    let weekday = (day + 3).rem_euclid(7); // Monday is 0; 1970-01-01 was a Thursday.
    let thursday = day - weekday + 3;
    let (year, _, _) = civil_from_days(thursday);
    let january_first = days_from_civil(year, 1, 1);
    (year, (thursday - january_first) / 7 + 1)
}

/// (year, month, day) to days since 1970-01-01, from Howard Hinnant's algorithm.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A table panel: its columns and the order of its rows.
struct TableState {
    columns: Vec<TableColumn>,
    /// Row numbers in the frame, in display order.
    order: Vec<usize>,
    sort: Option<(usize, bool)>,
    header: Id,
    body: Id,
}

impl TableState {
    fn new(data: &Arc<Data>, panel: usize, old: Option<TableState>) -> Option<TableState> {
        let p = &data.panels[panel];
        if p.panel.kind != PanelKind::Table {
            return None;
        }
        let frame = p.frame.as_ref().ok()?;
        let widths: HashMap<String, f32> =
            old.as_ref().map(|o| o.columns.iter().map(|c| (c.title.clone(), c.width)).collect()).unwrap_or_default();
        let columns = frame
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                // Wide enough for the header, and for a whole timestamp.
                let header = c.name.chars().count() as f32 * 7.5 + 24.0;
                let width = widths.get(&c.name).copied().unwrap_or(header.max(match c.effective_format() {
                    Format::Id => 170.0,
                    Format::Timestamp => 150.0,
                    Format::Number | Format::DurationSeconds => 90.0,
                    Format::Auto => 160.0,
                }));
                TableColumn { data: data.clone(), panel, column: i, title: c.name.clone(), width, resize_offset: None, sort: None }
            })
            .collect();
        let mut state = TableState {
            columns,
            order: (0..frame.rows()).collect(),
            sort: None,
            header: old.as_ref().map_or_else(Id::unique, |o| o.header.clone()),
            body: old.as_ref().map_or_else(Id::unique, |o| o.body.clone()),
        };
        // Keep the sort the user chose, if that column is still there.
        if let Some((title, ascending)) = old.and_then(|o| o.sort.map(|(i, a)| (o.columns[i].title.clone(), a))) {
            if let Some(i) = state.columns.iter().position(|c| c.title == title) {
                state.sort = Some((i, !ascending));
                state.sort_by(i);
            }
        }
        Some(state)
    }

    fn sort_by(&mut self, index: usize) {
        let ascending = match self.sort {
            Some((i, a)) if i == index => !a,
            _ => true,
        };
        self.sort = Some((index, ascending));
        for (i, c) in self.columns.iter_mut().enumerate() {
            c.sort = (i == index).then_some(ascending);
        }
        let Some(c) = self.columns.get(index) else { return };
        let values = &c.frame().columns[c.column].values;
        // Missing values go last either way.
        self.order.sort_by(|&a, &b| {
            use std::cmp::Ordering::*;
            let o = match values {
                Values::Text(v) => match (&v[a], &v[b]) {
                    (Some(x), Some(y)) => Some(x.cmp(y)),
                    (Some(_), None) => return Less,
                    (None, Some(_)) => return Greater,
                    (None, None) => return Equal,
                },
                _ => match (values.number(a), values.number(b)) {
                    (Some(x), Some(y)) => x.partial_cmp(&y),
                    (Some(_), None) => return Less,
                    (None, Some(_)) => return Greater,
                    (None, None) => return Equal,
                },
            }
            .unwrap_or(Equal);
            if ascending { o } else { o.reverse() }
        });
    }

    fn view(&self) -> Element<'_, TableMsg> {
        responsive(move |size| {
            table(self.header.clone(), self.body.clone(), &self.columns, &self.order, TableMsg::Synced)
                .on_header_press(TableMsg::HeaderPressed)
                .on_column_resize(TableMsg::ColumnDragged, TableMsg::ColumnReleased)
                .cell_padding([3, 6])
                .min_width(size.width)
                .into()
        })
        .into()
    }
}

struct TableColumn {
    data: Arc<Data>,
    panel: usize,
    column: usize,
    title: String,
    width: f32,
    resize_offset: Option<f32>,
    /// `Some(true)` if sorted ascending by this column.
    sort: Option<bool>,
}

impl TableColumn {
    fn frame(&self) -> &Frame {
        self.data.panels[self.panel].frame.as_ref().expect("a table is only made from rows")
    }
}

impl<'a> table::Column<'a, TableMsg, Theme, iced::Renderer> for TableColumn {
    type Row = usize;

    fn header(&'a self, _: usize) -> Element<'a, TableMsg> {
        let arrow = match self.sort {
            Some(true) => " ▲",
            Some(false) => " ▼",
            None => "",
        };
        text(format!("{}{arrow}", self.title)).size(12).font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT }).into()
    }

    fn cell(&'a self, _: usize, _: usize, row: &'a usize) -> Element<'a, TableMsg> {
        let c = &self.frame().columns[self.column];
        // One line, clipped to the column, so a long value can't run over
        // its neighbours; ids are shortened the way the bar charts do.
        let t = match c.effective_format() {
            Format::Id => text(short_id(&value(c, *row))).font(Font::MONOSPACE),
            _ => text(value(c, *row)),
        };
        container(t.size(12).wrapping(iced::widget::text::Wrapping::None)).width(Fill).clip(true).into()
    }

    fn width(&self) -> f32 {
        self.width
    }

    fn resize_offset(&self) -> Option<f32> {
        self.resize_offset
    }
}

/// A unit that keeps durations readable: seconds, minutes, hours or days.
fn unit_for(seconds: f64) -> (f64, &'static str) {
    match seconds.abs() {
        s if s >= 2.0 * 86_400.0 => (86_400.0, "days"),
        s if s >= 2.0 * 3_600.0 => (3_600.0, "hours"),
        s if s >= 120.0 => (60.0, "minutes"),
        _ => (1.0, "seconds"),
    }
}

fn duration(seconds: f64) -> String {
    let (unit, name) = unit_for(seconds);
    if unit == 1.0 { format!("{seconds:.0} s") } else { format!("{:.1} {}", seconds / unit, &name[..1]) }
}

/// `3 s ago`, `5 min ago`, `2 h ago`, `4 days ago`.
fn ago(seconds: i64) -> String {
    match seconds {
        s if s < 90 => format!("{s} s ago"),
        s if s < 90 * 60 => format!("{} min ago", s / 60),
        s if s < 48 * 3_600 => format!("{} h ago", s / 3_600),
        s => format!("{} days ago", s / 86_400),
    }
}

/// A party or contract id short enough for a column or an axis:
/// `Alice::1220d45c…bf9f` keeps the party's name and both ends of its hash.
fn short_id(s: &str) -> String {
    let (name, hash) = match s.split_once("::") {
        Some((name, hash)) => (format!("{name}::"), hash),
        None => (String::new(), s),
    };
    let chars: Vec<char> = hash.chars().collect();
    if chars.len() <= 14 {
        return s.to_string();
    }
    let head: String = chars[..8].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{name}{head}…{tail}")
}

/// `2026-11-02 09:00:05`, `2026-11-02 09:00` or `2026-11-02` for a
/// resolution under a minute, under a day, or longer (UTC).
fn time_label(seconds: i64, resolution: i64) -> String {
    let (y, m, d) = civil_from_days(seconds.div_euclid(86_400));
    let s = seconds.rem_euclid(86_400);
    match resolution {
        r if r < 60 => format!("{y}-{m:02}-{d:02} {:02}:{:02}:{:02}", s / 3_600, s % 3_600 / 60, s % 60),
        r if r < 86_400 => format!("{y}-{m:02}-{d:02} {:02}:{:02}", s / 3_600, s % 3_600 / 60),
        _ => format!("{y}-{m:02}-{d:02}"),
    }
}

/// Days since 1970-01-01 to (year, month, day), from Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_axes_get_round_steps_and_short_labels() {
        assert_eq!(round_step(1_700_000.0), 2_000_000.0);
        assert_eq!(round_step(0.3), 0.5);
        assert_eq!(round_step(40.0), 50.0);
        assert_eq!(short_number(2_000_000.0), "2M");
        assert_eq!(short_number(1_250_000.0), "1.25M");
        assert_eq!(short_number(350_000.0), "350k");
        assert_eq!(short_number(42.0), "42");
    }

    #[test]
    fn the_named_time_column_goes_across() {
        let col = |name: &str, values| Column { name: name.into(), format: Format::Auto, values };
        let frame = Frame {
            columns: vec![
                col("value", Values::Number(vec![Some(5.0)])),
                col("offset", Values::Number(vec![Some(10_000.0)])),
            ],
        };
        // The value comes first, but the panel says the offset goes across.
        assert_eq!(x_column_index(&frame, Some("offset")), Some(1));
        assert_eq!(x_column_index(&frame, None), Some(0));
        let timed = Frame { columns: vec![col("n", Values::Number(vec![Some(1.0)])), col("at", Values::Time(vec![Some(0)]))] };
        assert_eq!(x_column_index(&timed, None), Some(1));
        assert_eq!(x_column_index(&Frame::default(), Some("offset")), None);
    }

    #[test]
    fn a_replaced_request_is_ignored() {
        let mut d = Dashboard::new("a".into(), Vec::new(), None);
        let _ = d.reload();
        d.location = "b".into();
        let _ = d.update(Msg::LocationSubmitted);
        // The listing for "a" comes back after the one for "b" started.
        let _ = d.update(Msg::Listed(1, Err("old location".into())));
        assert!(d.busy);
        assert_eq!(d.error, None);
        let _ = d.update(Msg::Listed(2, Err("new location".into())));
        assert!(!d.busy);
        assert_eq!(d.error.as_deref(), Some("new location"));
    }

    #[test]
    fn an_empty_location_says_how_to_fill_it() {
        let mut d = Dashboard::new("t".into(), Vec::new(), None);
        let _ = d.reload();
        let _ = d.update(Msg::Listed(1, Ok(Vec::new())));
        assert!(d.error.as_deref().unwrap().contains("canton-analytics"));
    }

    #[test]
    fn bad_storage_options_are_reported() {
        let d = Dashboard::new("s3://bucket/x".into(), vec!["no-equals-sign".into()], None);
        assert!(d.root().unwrap_err().contains("key=value"));
    }

    #[test]
    fn labels_and_durations() {
        assert_eq!(time_label(0, 3_600), "1970-01-01 00:00");
        // 2026-11-02 09:00 UTC
        assert_eq!(time_label(1_793_610_000, 3_600), "2026-11-02 09:00");
        assert_eq!(time_label(1_793_610_005, 1), "2026-11-02 09:00:05");
        assert_eq!(time_label(1_793_610_000, 86_400), "2026-11-02");
        assert_eq!(duration(42.0), "42 s");
        assert_eq!(duration(720.0), "12.0 m");
        assert_eq!(duration(41_400.0), "11.5 h");
        assert_eq!(number(3.0), "3");
        assert_eq!(number(0.125), "0.125");
        assert_eq!(bucket_name(3_600), "hour");
        assert_eq!(bucket_name(21_600), "6 hours");
        assert_eq!(ago(3_700), "61 min ago");
        assert_eq!(short_id("Alice::1220d45c8b0b8dec9f71038b"), "Alice::1220d45c…038b");
        assert_eq!(short_id("00c5e9f6a1b2c3d4e5f60718"), "00c5e9f6…0718");
        assert_eq!(short_id("Bob::1220ab"), "Bob::1220ab");
        assert_eq!(short_time_label(45_296, 5.0), "12:34:56");
        assert_eq!(bucket_name(1), "second");
        assert!(tick_step(86_400.0) <= 21_600.0);
    }

    #[test]
    fn lag_settings_parse_with_empty_for_the_daemons_own() {
        let form = |check: &str, every: &str, min_rows: &str, max_wait: &str| LagForm {
            check: check.into(),
            every: every.into(),
            min_rows: min_rows.into(),
            max_wait: max_wait.into(),
            saving: false,
            said: None,
        };
        let (check, tuning) = form("2", "", " 1000 ", "0").parse().unwrap();
        assert_eq!(check, 2);
        assert_eq!(tuning, Tuning { every_seconds: None, min_rows: Some(1000), max_wait_seconds: Some(0) });
        assert!(form("0", "", "", "").parse().is_err(), "this window must look at least every second");
        assert!(form("", "", "", "").parse().is_err());
        assert!(form("2", "0", "", "").parse().is_err());
        assert!(form("2", "", "lots", "").parse().is_err());
    }

    #[test]
    fn iso_week_numbers() {
        let day = |y, m, d| days_from_civil(y, m, d);
        assert_eq!(civil_from_days(day(2026, 11, 24)), (2026, 11, 24));
        assert_eq!(iso_week(day(2026, 11, 24)), (2026, 48));
        assert_eq!(iso_week(day(2026, 1, 1)), (2026, 1)); // a Thursday
        assert_eq!(iso_week(day(2027, 1, 1)), (2026, 53)); // a Friday
        assert_eq!(iso_week(day(2024, 12, 30)), (2025, 1)); // a Monday
        assert_eq!(iso_week(day(2025, 12, 28)), (2025, 52)); // a Sunday
    }

    #[test]
    fn axis_tick_counts_stay_small() {
        // A single day of points on a week-wide view, once a crash.
        assert!(axis_ticks(0.0, 604_800.0, tick_step(604_800.0)).len() <= 7);
        // A step far too small for the range is widened.
        let ticks = axis_ticks(0.0, 604_800.0, 1.0);
        assert!(!ticks.is_empty() && ticks.len() <= MAX_TICKS + 1);
        // A range far from zero with a step below its float resolution.
        assert!(axis_ticks(1.8e18, 1.8e18 + 1e6, 1e-3).len() <= MAX_TICKS + 1);
        // Zoomed out to the extremes.
        assert!(axis_ticks(-1e300, 1e300, round_step(2e300 / 5.0)).len() <= MAX_TICKS + 1);
        // Nothing to tick.
        assert!(axis_ticks(5.0, 5.0, 1.0).is_empty());
        assert!(axis_ticks(0.0, 10.0, 0.0).is_empty());
        assert!(axis_ticks(0.0, 10.0, round_step(0.0)).is_empty());
        assert!(axis_ticks(f64::NAN, 10.0, 1.0).is_empty());
        let ticks = axis_ticks(0.0, 10.0, 2.0);
        assert_eq!(ticks.iter().map(|t| t.value).collect::<Vec<_>>(), [0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
    }
}
