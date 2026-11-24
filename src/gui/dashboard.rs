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
use analytics_reader::dashboard::{self, Dashboard as Data, PanelData};
use analytics_reader::frame::{Column, Frame, Values};
use analytics_reader::location::{self, Location};
use analytics_reader::manifest::{Format, PanelKind};
use analytics_reader::status::{Status, now_ms};
use analytics_reader::Session;
use iced::widget::{button, column, container, pick_list, responsive, row, scrollable, text, text_input, Id};
use iced::{Alignment, Color, Element, Fill, Font, Length, Task, Theme};
use iced_plot::{LineStyle, MarkerStyle, PlotUiMessage, PlotWidget, PlotWidgetBuilder, Series};
use iced_table2::table::{self, table};
use plotters::prelude::*;
use plotters::style::Color as _;
use plotters_iced2::{Chart, ChartWidget, DrawingBackend};
use tokio::sync::Mutex;

use crate::gui::sidebar::{ERROR, MUTED};

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
    tables: Vec<Option<TableState>>,
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
            tables: Vec::new(),
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
        Task::perform(
            async move {
                let session = Session::open(&location?).await.map_err(|e| format!("{e:#}"))?;
                let data = dashboard::load(&session).await.map_err(|e| format!("{e:#}"))?;
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
        let id = self.next_request();
        Task::perform(
            async move {
                let mut session = session.lock().await;
                let status = session.status().await.ok().flatten();
                let version = session.latest_version().await.map_err(|e| format!("{e:#}"))?;
                let data = if Some(version) != tried {
                    let loaded = async {
                        session.refresh().await?;
                        dashboard::load(&session).await
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
                PanelKind::Bar => match &self.bars[i] {
                    Some(bars) => ChartWidget::new(bars).width(Fill).height(Fill).into(),
                    None => empty("Needs a label column and a number column."),
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
        let step = tick_step(last - first);
        builder
            .with_x_tick_producer(move |min, max| time_ticks(min, max, step))
            .with_x_tick_formatter(move |tick| short_time_label(tick.value as i64, step))
            .with_hover_highlight_provider(move |_, point| {
                Some(format!("{}: {}", time_label(point.x as i64, bucket as i64), number(point.y)))
            })
    } else {
        let mut step = round_step((last - first).max(1.0) / 5.0);
        // Offsets and other counts are whole numbers: no ticks between them.
        if xs.iter().flatten().all(|x| x.fract() == 0.0) {
            step = step.max(1.0);
        }
        let name = x_column.name.clone();
        builder
            .with_x_tick_producer(move |min, max| time_ticks(min, max, step))
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

fn time_ticks(min: f64, max: f64, step: f64) -> Vec<iced_plot::Tick> {
    if !(max > min) {
        return Vec::new();
    }
    let mut t = (min / step).ceil() * step;
    let mut ticks = Vec::new();
    while t <= max {
        ticks.push(iced_plot::Tick { value: t, step_size: step, line_type: iced_plot::TickWeight::Major });
        t += step;
    }
    ticks
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
}
