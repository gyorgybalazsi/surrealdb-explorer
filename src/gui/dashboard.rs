//! The Dashboard tab: charts and a table over a Delta Lake table written by
//! the canton-analytics runner (see `analytics/`). Today it knows the
//! contract-lifetimes plugin's table.
//!
//! Polars does the work on a background thread; the view only draws plain
//! values. While the tab is open the table's `_delta_log` is checked every
//! couple of seconds, and the dashboard reloads when the runner commits.

use std::sync::Arc;

use analytics_reader::lifetimes::{self, Dashboard as Data, Lifetime, TemplateStats};
use iced::widget::{
    button, column, container, pane_grid, responsive, row, scrollable, text, text_input, Id,
};
use iced::{Alignment, Color, Element, Fill, Font, Task, Theme};
use iced_plot::{LineStyle, MarkerStyle, PlotUiMessage, PlotWidget, PlotWidgetBuilder, Series};
use iced_table2::table::{self, table};
use plotters::prelude::*;
use plotters::style::Color as _;
use plotters_iced2::{Chart, ChartWidget, DrawingBackend};

use crate::gui::sidebar::{ERROR, MUTED};

const ACCENT: Color = Color::from_rgb(0.18, 0.42, 0.78);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Totals,
    OverTime,
    ByTemplate,
    Longest,
}

#[derive(Debug, Clone)]
pub enum Msg {
    PathEdited(String),
    /// Enter in the folder box: load that table (the app also saves it).
    PathSubmitted,
    Reload,
    /// Time to look for a new commit.
    Check,
    Version(Result<Option<i64>, String>),
    /// A load finished; the number says which `reload` started it.
    Loaded(u64, Result<Arc<Data>, String>),
    Plot(PlotUiMessage),
    PaneResized(pane_grid::ResizeEvent),
    PaneDragged(pane_grid::DragEvent),
    TableSynced(iced::widget::scrollable::AbsoluteOffset),
    HeaderPressed(usize),
    ColumnDragged(usize, f32),
    ColumnReleased,
}

pub struct Dashboard {
    /// Folder of the Delta table, as typed.
    pub path: String,
    data: Option<Arc<Data>>,
    error: Option<String>,
    loading: bool,
    /// Counts loads, so a load that a newer one replaced is ignored.
    load_id: u64,
    /// The table version last loaded or tried, so a version that fails to
    /// load isn't retried every check.
    tried: Option<i64>,
    panes: pane_grid::State<Pane>,
    plot: Option<PlotWidget>,
    /// The longest-lived contracts, in the table's sort order.
    rows: Vec<Lifetime>,
    columns: Vec<TableColumn>,
    sort: (usize, bool),
    header: Id,
    body: Id,
}

impl Dashboard {
    pub fn new(path: String) -> Dashboard {
        use pane_grid::{Axis, Configuration};
        let split = |axis, ratio, a, b| Configuration::Split { axis, ratio, a: Box::new(a), b: Box::new(b) };
        let config = split(
            Axis::Horizontal,
            0.16,
            Configuration::Pane(Pane::Totals),
            split(
                Axis::Horizontal,
                0.5,
                split(Axis::Vertical, 0.55, Configuration::Pane(Pane::OverTime), Configuration::Pane(Pane::ByTemplate)),
                Configuration::Pane(Pane::Longest),
            ),
        );
        Dashboard {
            path,
            data: None,
            error: None,
            loading: false,
            load_id: 0,
            tried: None,
            panes: pane_grid::State::with_configuration(config),
            plot: None,
            rows: Vec::new(),
            columns: TableColumn::all(),
            sort: (4, false),
            header: Id::unique(),
            body: Id::unique(),
        }
    }

    fn table_path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(self.path.trim())
    }

    /// Reloads the dashboard from the table.
    pub fn reload(&mut self) -> Task<Msg> {
        self.loading = true;
        self.load_id += 1;
        let id = self.load_id;
        let path = self.table_path();
        Task::perform(
            async move { lifetimes::load(&path).await.map(Arc::new).map_err(|e| format!("{e:#}")) },
            move |result| Msg::Loaded(id, result),
        )
    }

    pub fn update(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::PathEdited(path) => self.path = path,
            Msg::PathSubmitted | Msg::Reload => {
                self.data = None;
                self.tried = None;
                return self.reload();
            }
            Msg::Check => {
                if self.loading {
                    return Task::none();
                }
                let path = self.table_path();
                return Task::perform(
                    async move { analytics_reader::latest_version(&path).map_err(|e| format!("{e:#}")) },
                    Msg::Version,
                );
            }
            Msg::Version(Ok(version)) => {
                if version.is_some() && version != self.tried {
                    self.tried = version;
                    return self.reload();
                }
            }
            Msg::Version(Err(e)) => self.error = Some(e),
            Msg::Loaded(id, _) if id != self.load_id => {}
            Msg::Loaded(_, result) => {
                self.loading = false;
                match result {
                    Ok(data) => {
                        self.error = None;
                        self.tried = data.version;
                        self.plot = timeline_plot(&data);
                        self.rows = data.longest.clone();
                        self.sort_rows();
                        self.data = Some(data);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::Plot(m) => {
                if let Some(plot) = &mut self.plot {
                    plot.update(m);
                }
            }
            Msg::PaneResized(pane_grid::ResizeEvent { split, ratio }) => self.panes.resize(split, ratio),
            Msg::PaneDragged(pane_grid::DragEvent::Dropped { pane, target }) => self.panes.drop(pane, target),
            Msg::PaneDragged(_) => {}
            Msg::TableSynced(offset) => return iced::widget::operation::scroll_to(self.header.clone(), offset),
            Msg::HeaderPressed(index) => {
                self.sort = if self.sort.0 == index { (index, !self.sort.1) } else { (index, true) };
                self.sort_rows();
            }
            Msg::ColumnDragged(index, offset) => {
                if let Some(c) = self.columns.get_mut(index) {
                    c.resize_offset = Some(offset);
                }
            }
            Msg::ColumnReleased => {
                for c in &mut self.columns {
                    if let Some(offset) = c.resize_offset.take() {
                        c.width = (c.width + offset).max(40.0);
                    }
                }
            }
        }
        Task::none()
    }

    fn sort_rows(&mut self) {
        let (index, ascending) = self.sort;
        for (i, c) in self.columns.iter_mut().enumerate() {
            c.sort = (i == index).then_some(ascending);
        }
        // Missing values go last either way.
        fn opt<T: PartialOrd>(a: &Option<T>, b: &Option<T>, ascending: bool) -> std::cmp::Ordering {
            use std::cmp::Ordering::*;
            match (a, b) {
                (Some(a), Some(b)) => {
                    let o = a.partial_cmp(b).unwrap_or(Equal);
                    if ascending { o } else { o.reverse() }
                }
                (Some(_), None) => Less,
                (None, Some(_)) => Greater,
                (None, None) => Equal,
            }
        }
        self.rows.sort_by(|a, b| match index {
            0 => opt(&Some(&a.contract_id), &Some(&b.contract_id), ascending),
            1 => opt(&a.template, &b.template, ascending),
            2 => opt(&a.created_offset, &b.created_offset, ascending),
            3 => opt(&Some(a.archived_offset), &Some(b.archived_offset), ascending),
            4 => opt(&a.lifetime_seconds, &b.lifetime_seconds, ascending),
            _ => opt(&a.archived_by, &b.archived_by, ascending),
        });
    }

    pub fn view(&self) -> Element<'_, Msg> {
        let header = row![
            text("Delta table").size(13),
            text_input("tables/contract_lifetimes", &self.path)
                .on_input(Msg::PathEdited)
                .on_submit(Msg::PathSubmitted)
                .size(13)
                .width(360),
            button(text("Reload").size(12)).style(button::secondary).on_press(Msg::Reload),
            text(match (&self.data, self.loading) {
                (_, true) => "Loading…".to_string(),
                (Some(d), _) => d.version.map(|v| format!("version {v}")).unwrap_or_default(),
                (None, _) => String::new(),
            })
            .size(12)
            .color(MUTED),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let body: Element<'_, Msg> = match (&self.data, &self.error) {
            (_, Some(e)) if self.data.is_none() => container(
                column![
                    text(e).size(13).color(ERROR),
                    text(
                        "Run the analytics runner to create the table, e.g. \
                         canton-analytics run --plugin contract_lifetimes.wasm --table tables/contract_lifetimes …",
                    )
                    .size(12)
                    .color(MUTED),
                ]
                .spacing(6),
            )
            .padding(12)
            .into(),
            (None, _) => container(text("Loading…").size(13).color(MUTED)).padding(12).into(),
            (Some(data), _) => self.panes_view(data),
        };
        let mut col = column![header].spacing(8).padding(10).width(Fill).height(Fill);
        if let (Some(e), Some(_)) = (&self.error, &self.data) {
            col = col.push(text(e).size(12).color(ERROR));
        }
        col.push(body).into()
    }

    fn panes_view<'a>(&'a self, data: &'a Data) -> Element<'a, Msg> {
        pane_grid(&self.panes, move |_, pane, _| {
            let (title, content): (&str, Element<'a, Msg>) = match pane {
                Pane::Totals => ("Totals", totals(data)),
                Pane::OverTime => (
                    match data.over_time.bucket_seconds {
                        3_600 => "Archives per hour",
                        86_400 => "Archives per day",
                        _ => "Archives per week",
                    },
                    match &self.plot {
                        Some(plot) => plot.view().map(Msg::Plot),
                        None => empty("No archive times."),
                    },
                ),
                Pane::ByTemplate => (
                    "Mean lifetime per template",
                    if data.by_template.is_empty() {
                        empty("No templates.")
                    } else {
                        ChartWidget::new(TemplateBars(&data.by_template)).width(Fill).height(Fill).into()
                    },
                ),
                Pane::Longest => ("Longest-lived contracts", self.table_view()),
            };
            let title_bar = pane_grid::TitleBar::new(text(title).size(13).font(Font {
                weight: iced::font::Weight::Semibold,
                ..Font::DEFAULT
            }))
            .padding([4, 8])
            .style(|theme: &Theme| container::Style {
                background: Some(theme.extended_palette().background.weak.color.into()),
                ..Default::default()
            });
            pane_grid::Content::new(container(content).padding(6).width(Fill).height(Fill))
                .title_bar(title_bar)
                .style(container::bordered_box)
        })
        .spacing(6)
        .on_resize(8, Msg::PaneResized)
        .on_drag(Msg::PaneDragged)
        .into()
    }

    fn table_view(&self) -> Element<'_, Msg> {
        if self.rows.is_empty() {
            return empty("No archived contracts yet.");
        }
        responsive(move |size| {
            table(self.header.clone(), self.body.clone(), &self.columns, &self.rows, Msg::TableSynced)
                .on_header_press(Msg::HeaderPressed)
                .on_column_resize(Msg::ColumnDragged, Msg::ColumnReleased)
                .cell_padding([3, 6])
                .min_width(size.width)
                .into()
        })
        .into()
    }
}

fn empty<'a>(message: &'a str) -> Element<'a, Msg> {
    container(text(message).size(12).color(MUTED)).center(Fill).into()
}

fn totals(data: &Data) -> Element<'_, Msg> {
    let card = |label: &'static str, value: String| {
        container(column![text(value).size(22).color(ACCENT), text(label).size(11).color(MUTED)].spacing(2))
            .padding([6, 14])
            .style(container::rounded_box)
    };
    scrollable(
        row![
            card("contracts archived", data.archived.to_string()),
            card("mean lifetime", data.mean_seconds.map(duration).unwrap_or("–".into())),
            card("median lifetime", data.median_seconds.map(duration).unwrap_or("–".into())),
            card("templates", data.by_template.len().to_string()),
            card("newest archive offset", data.newest_offset.map(|o| o.to_string()).unwrap_or("–".into())),
        ]
        .spacing(10),
    )
    .direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new()))
    .into()
}

/// The archives-over-time line, or `None` if there are no archive times.
fn timeline_plot(data: &Data) -> Option<PlotWidget> {
    let buckets = &data.over_time.buckets;
    if buckets.is_empty() {
        return None;
    }
    let bucket = data.over_time.bucket_seconds;
    let points: Vec<[f64; 2]> = buckets.iter().map(|&(t, n)| [t as f64, n as f64]).collect();
    let series = Series::new(points, MarkerStyle::circle(5.0), LineStyle::solid().with_pixel_width(2.0))
        .with_label("archives")
        .with_color(ACCENT);
    let first = buckets[0].0 as f64;
    let last = buckets[buckets.len() - 1].0 as f64;
    let top = buckets.iter().map(|&(_, n)| n).max().unwrap_or(0) as f64;
    PlotWidgetBuilder::new()
        .add_series(series)
        .with_x_lim(first - bucket as f64 / 2.0, last + bucket as f64 / 2.0)
        .with_y_lim(0.0, (top * 1.15).max(1.0))
        .with_x_tick_producer(move |min, max| time_ticks(min, max, bucket))
        .with_x_tick_formatter(move |tick| short_time_label(tick.value as i64, bucket))
        .with_y_tick_formatter(|tick| format!("{:.0}", tick.value + 0.0))
        .with_hover_highlight_provider(move |_, point| {
            Some(format!("{}: {} archived", time_label(point.x as i64, bucket), point.y))
        })
        .with_tick_label_size(11.0)
        .disable_legend()
        .disable_controls_help()
        .build()
        .ok()
}

/// At most about six ticks, on whole buckets (hours or days).
fn time_ticks(min: f64, max: f64, bucket: i64) -> Vec<iced_plot::Tick> {
    if !(max > min) {
        return Vec::new();
    }
    let span = (max - min) / bucket as f64;
    let every = [1.0, 2.0, 3.0, 6.0, 12.0, 24.0, 48.0, 168.0]
        .into_iter()
        .find(|k| span / k <= 6.0)
        .unwrap_or((span / 6.0).ceil());
    let step = every * bucket as f64;
    let mut t = (min / step).ceil() * step;
    let mut ticks = Vec::new();
    while t <= max {
        ticks.push(iced_plot::Tick { value: t, step_size: step, line_type: iced_plot::TickWeight::Major });
        t += step;
    }
    ticks
}

/// `11-02 09:00` for hourly buckets, `11-02` otherwise (UTC).
fn short_time_label(seconds: i64, bucket: i64) -> String {
    let label = time_label(seconds, bucket);
    label.get(5..).unwrap_or(&label).to_string()
}

/// Horizontal bars: mean lifetime per template, most archived on top.
struct TemplateBars<'a>(&'a [TemplateStats]);

impl Chart<Msg> for TemplateBars<'_> {
    type State = ();

    fn build_chart<DB: DrawingBackend>(&self, _: &(), mut builder: ChartBuilder<DB>) {
        let rows: Vec<&TemplateStats> = self.0.iter().take(12).collect();
        let max = rows.iter().filter_map(|t| t.mean_seconds).fold(0.0, f64::max).max(1.0);
        let (unit, unit_name) = unit_for(max);
        let names: Vec<String> = rows
            .iter()
            .map(|t| t.template.clone().unwrap_or_else(|| "(created before the data)".into()))
            .collect();
        let n = rows.len() as i32;
        let Ok(mut chart) = builder
            .margin(8)
            .x_label_area_size(42)
            .y_label_area_size(170)
            .build_cartesian_2d(0.0..max / unit * 1.1, (0..n).into_segmented())
        else {
            return;
        };
        let label_style = ("sans-serif", 12).into_font().color(&RGBColor(90, 90, 90));
        // Row 0 (the most archived template) is drawn at the top.
        let top = |i: usize| n - 1 - i as i32;
        let _ = chart
            .configure_mesh()
            .disable_y_mesh()
            .y_labels(n as usize)
            .y_label_formatter(&|v| match v {
                SegmentValue::CenterOf(y) | SegmentValue::Exact(y) => {
                    let i = (n - 1 - y) as usize;
                    match (names.get(i), rows.get(i)) {
                        (Some(name), Some(t)) => format!("{} ({})", shorten(name, 20), t.archived),
                        _ => String::new(),
                    }
                }
                SegmentValue::Last => String::new(),
            })
            .x_desc(format!("mean lifetime, {unit_name}"))
            .label_style(label_style.clone())
            .axis_desc_style(label_style)
            .light_line_style(RGBColor(235, 235, 235))
            .bold_line_style(RGBColor(220, 220, 220))
            .draw();
        let bar = RGBColor(46, 107, 199);
        let _ = chart.draw_series(rows.iter().enumerate().filter_map(|(i, t)| {
            let y = top(i);
            let x = t.mean_seconds? / unit;
            let mut r = Rectangle::new([(0.0, SegmentValue::Exact(y)), (x, SegmentValue::Exact(y + 1))], bar.mix(0.8).filled());
            r.set_margin(4, 4, 0, 0);
            Some(r)
        }));
    }
}

/// A unit that keeps bar lengths readable: seconds, minutes, hours or days.
fn unit_for(seconds: f64) -> (f64, &'static str) {
    match seconds {
        s if s >= 2.0 * 86_400.0 => (86_400.0, "days"),
        s if s >= 2.0 * 3_600.0 => (3_600.0, "hours"),
        s if s >= 120.0 => (60.0, "minutes"),
        _ => (1.0, "seconds"),
    }
}

fn duration(seconds: f64) -> String {
    let (unit, name) = unit_for(seconds);
    if unit == 1.0 {
        format!("{seconds:.0} s")
    } else {
        format!("{:.1} {}", seconds / unit, &name[..1])
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("…{}", s.chars().rev().take(max - 1).collect::<Vec<_>>().into_iter().rev().collect::<String>())
    }
}

/// `2026-11-02 09:00` for hourly buckets, `2026-11-02` otherwise (UTC).
fn time_label(seconds: i64, bucket: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    if bucket < 86_400 {
        let s = seconds.rem_euclid(86_400);
        format!("{y}-{m:02}-{d:02} {:02}:{:02}", s / 3_600, s % 3_600 / 60)
    } else {
        format!("{y}-{m:02}-{d:02}")
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

struct TableColumn {
    title: &'static str,
    width: f32,
    resize_offset: Option<f32>,
    /// `Some(true)` if sorted ascending by this column.
    sort: Option<bool>,
}

impl TableColumn {
    fn all() -> Vec<TableColumn> {
        [("Contract", 150.0), ("Template", 200.0), ("Created at offset", 120.0), ("Archived at offset", 125.0), ("Lifetime", 90.0), ("Archived by", 170.0)]
            .into_iter()
            .map(|(title, width)| TableColumn { title, width, resize_offset: None, sort: None })
            .collect()
    }
}

impl<'a> table::Column<'a, Msg, Theme, iced::Renderer> for TableColumn {
    type Row = Lifetime;

    fn header(&'a self, _: usize) -> Element<'a, Msg> {
        let arrow = match self.sort {
            Some(true) => " ▲",
            Some(false) => " ▼",
            None => "",
        };
        text(format!("{}{arrow}", self.title)).size(12).font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT }).into()
    }

    fn cell(&'a self, col: usize, _: usize, row: &'a Lifetime) -> Element<'a, Msg> {
        let dash = || "–".to_string();
        let value = match col {
            0 => row.contract_id.clone(),
            1 => row.template.clone().unwrap_or_else(dash),
            2 => row.created_offset.map(|o| o.to_string()).unwrap_or_else(dash),
            3 => row.archived_offset.to_string(),
            4 => row.lifetime_seconds.map(duration).unwrap_or_else(dash),
            _ => row.archived_by.clone().unwrap_or_else(dash),
        };
        let t = text(value).size(12);
        if col == 0 { t.font(Font::MONOSPACE).into() } else { t.into() }
    }

    fn width(&self) -> f32 {
        self.width
    }

    fn resize_offset(&self) -> Option<f32> {
        self.resize_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_load_is_ignored() {
        let mut d = Dashboard::new("a".into());
        let _ = d.reload();
        d.path = "b".into();
        let _ = d.update(Msg::PathSubmitted);
        // The first load, for "a", finishes after the second one started.
        let _ = d.update(Msg::Loaded(1, Err("old table".into())));
        assert!(d.loading);
        assert_eq!(d.error, None);
        let _ = d.update(Msg::Loaded(2, Err("new table".into())));
        assert!(!d.loading);
        assert_eq!(d.error.as_deref(), Some("new table"));
    }

    #[test]
    fn a_version_that_fails_to_load_is_tried_once() {
        let mut d = Dashboard::new("t".into());
        let _ = d.update(Msg::Version(Ok(Some(3))));
        assert!(d.loading);
        let _ = d.update(Msg::Loaded(d.load_id, Err("broken".into())));
        // The same version again: no new load.
        let _ = d.update(Msg::Version(Ok(Some(3))));
        assert!(!d.loading);
        // A new commit is tried.
        let _ = d.update(Msg::Version(Ok(Some(4))));
        assert!(d.loading);
    }

    #[test]
    fn labels_and_durations() {
        assert_eq!(time_label(0, 3_600), "1970-01-01 00:00");
        // 2026-11-02 09:00 UTC
        assert_eq!(time_label(1_793_610_000, 3_600), "2026-11-02 09:00");
        assert_eq!(time_label(1_793_610_000, 86_400), "2026-11-02");
        assert_eq!(duration(42.0), "42 s");
        assert_eq!(duration(720.0), "12.0 m");
        assert_eq!(duration(41_400.0), "11.5 h");
        assert_eq!(shorten("Main.TicketOffer", 8), "…etOffer");
    }
}
