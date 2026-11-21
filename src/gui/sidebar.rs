//! The sidebar: graph summary, legend, actions and the selected record.

use iced::widget::{Column, button, checkbox, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Color, Element, Fill, Font};
use crate::canton::{NodeKind, StatusFilter};
use crate::config::Visualization;

use crate::gui::app::{App, CantonFilterChange as F, DataAction, Message, Selection, plural};
use crate::gui::canton_draw;
use crate::canton;
use crate::graph;

pub(super) const SIDEBAR_WIDTH: f32 = 320.0;
/// The sidebar's scrollable, scrolled to the end to show a new selection.
pub(super) const SIDEBAR_SCROLL: &str = "sidebar";
pub(super) const MUTED: Color = Color::from_rgb(0.47, 0.47, 0.47);
pub(super) const WARNING: Color = Color::from_rgb(0.67, 0.4, 0.0);
pub(super) const ERROR: Color = Color::from_rgb(0.7, 0.0, 0.0);
pub(super) const SUCCESS: Color = Color::from_rgb(0.1, 0.5, 0.2);

impl App {
    /// Switches the main area between the graph and the analytics dashboard.
    #[cfg(feature = "dashboard")]
    fn dashboard_button(&self) -> Element<'_, Message> {
        button(text(if self.dashboard_open { "Graph" } else { "Dashboard" }).size(12))
            .style(button::secondary)
            .on_press(Message::ToggleDashboard)
            .into()
    }

    #[cfg(not(feature = "dashboard"))]
    fn dashboard_button(&self) -> Element<'_, Message> {
        iced::widget::Row::new().into()
    }

    pub(super) fn sidebar(&self) -> Element<'_, Message> {
        let g = &self.graph;
        let mut col = Column::new().spacing(6).padding(16).width(SIDEBAR_WIDTH);

        col = col.push(
            row![text("SurrealDB Explorer").size(18).width(Fill), self.dashboard_button()]
                .push(button(text("Settings").size(12)).style(button::secondary).on_press(Message::OpenSettings))
                .spacing(6)
                .align_y(iced::Alignment::Center),
        );
        let summary = if self.overview {
            let records: i64 = g.nodes.iter().filter_map(|n| n.data["records"].as_i64()).sum();
            format!("Table overview: {} with {records} records", plural(g.nodes.len(), "table"))
        } else {
            format!(
                "{} in {}, {}",
                plural(g.nodes.len(), "record"),
                plural(g.tables.len(), "table"),
                plural(g.edges.len(), "relation")
            )
        };
        col = col.push(text(summary).size(12).color(MUTED));
        if let Some(error) = &self.error {
            col = col.push(text(error).size(12).color(ERROR));
        }
        let mcp_error = self.mcp.as_ref().and_then(|m| m.window.lock().ok()?.server_error.clone());
        if let Some(error) = mcp_error {
            col = col.push(text(error).size(12).color(ERROR));
        }
        if !g.truncated.is_empty() {
            col = col.push(
                text(format!(
                    "Only the first {} records shown for: {}",
                    self.config.graph.max_records_per_table,
                    g.truncated.join(", ")
                ))
                .size(12)
                .color(WARNING),
            );
        }
        if g.dangling_edges > 0 {
            col = col.push(
                text(format!("{} point at records that aren't shown", plural(g.dangling_edges, "relation")))
                    .size(12)
                    .color(WARNING),
            );
        }

        if g.nodes.is_empty() && self.db.is_some() && self.error.is_none() {
            col = col.push(text("The database is empty.").size(13));
            col = col.push(
                button(text(if self.data_busy { "Loading…" } else { "Load sample data" }).size(13))
                    .style(button::primary)
                    .on_press_maybe((!self.data_busy).then_some(Message::RunDataAction(DataAction::LoadSampleData))),
            );
        }

        // The view picker, then the options every view shares, then a box with
        // the options of the chosen view only, so switching views changes the
        // box and nothing else.
        let visualization = self.config.graph.visualization;
        col = col.push(heading("View"));
        col = col.push(
            row![
                pick_list(Visualization::ALL, Some(visualization), Message::SetVisualization)
                    .text_size(13)
                    .width(Fill),
                button(text("Fit").size(12)).style(button::secondary).on_press(Message::FitView),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );
        if visualization == Visualization::Standard && g.looks_like_event_graph() {
            col = col.push(
                row![
                    text("This looks like a Canton event graph.").size(12).color(MUTED),
                    button(text("Show it as one").size(12))
                        .style(button::text)
                        .padding(0)
                        .on_press(Message::SetVisualization(Visualization::Canton)),
                ]
                .spacing(6),
            );
        }

        col = col.push(self.general_options());

        let view_options = match visualization {
            Visualization::Canton => self.canton_filters_view(),
            Visualization::Standard => self.tables_legend(),
        };
        let title = match visualization {
            Visualization::Standard => "Standard view",
            Visualization::Canton => "Canton event graph view",
        };
        col = col.push(view_box(title, view_options));

        col = col.push(heading("Selected"));
        col = col.push(self.details());

        scrollable(col).id(SIDEBAR_SCROLL).height(Fill).into()
    }

    /// Options that apply whichever view is chosen: reloading, the table
    /// overview, auto refresh and hidden records.
    fn general_options(&self) -> Element<'_, Message> {
        let g = &self.graph;
        let mut col = Column::new().spacing(6);
        col = col.push(heading("All views"));
        col = col.push(
            row![
                button(text("Reload").size(13)).on_press(Message::Reload),
                button(text("Tables").size(13))
                    .style(button::secondary)
                    // The Canton view draws transactions and events only; its
                    // canvas would be empty with the tables in it.
                    .on_press_maybe(
                        (!self.overview && self.db.is_some() && visualization_is_standard(self))
                            .then_some(Message::ShowOverview),
                    ),
                checkbox(self.config.graph.auto_refresh)
                    .label("Auto refresh")
                    .on_toggle(Message::ToggleAutoRefresh)
                    .text_size(12),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
        );
        if self.config.graph.auto_refresh && self.result_view.is_some() {
            col = col.push(
                text("Auto refresh is paused while a query result is shown. Reload to go back to the whole database.")
                    .size(12)
                    .color(MUTED),
            );
        }
        let hidden = g.hidden_count();
        if hidden > 0 {
            col = col.push(
                row![
                    text(format!("{} hidden", plural(hidden, "record"))).size(12).color(MUTED),
                    button(text("Show all").size(12)).style(button::text).padding(0).on_press(Message::ShowAll),
                ]
                .spacing(8),
            );
        }
        if self.overview {
            let records: i64 = g.nodes.iter().filter_map(|n| n.data["records"].as_i64()).sum();
            col = col.push(
                text(format!(
                    "{records} records: too many to draw at once (more than {}). Each dot is a table. Right-click one to show its records, then right-click a record to show its neighbours.",
                    self.config.graph.overview_above
                ))
                .size(12)
                .color(MUTED),
            );
            col = col.push(
                button(text("Draw all records anyway").size(12))
                    .style(button::secondary)
                    .on_press(Message::LoadEverything),
            );
        }
        if let Some(notice) = &self.notice {
            col = col.push(text(notice).size(12).color(MUTED));
        }
        let hint = if self.canton() {
            "Hover a record to highlight its neighbours. Click for details; right-click for more actions. Scroll, drag the background or use the scrollbar to move up and down."
        } else {
            "Hover a record to highlight its neighbours. Click for details; right-click for more actions. Drag to move, scroll to zoom, drag the background to pan."
        };
        col = col.push(
            text(hint)
                .size(12)
                .color(MUTED),
        );
        col.into()
    }

    /// A table's records: drawn ones, or in the overview all of them.
    fn table_count(&self, t: &crate::gui::model::Table) -> i64 {
        if self.overview {
            self.graph.nodes.iter().find(|n| n.table == t.name).and_then(|n| n.data["records"].as_i64()).unwrap_or(0)
        } else {
            t.count as i64
        }
    }

    fn tables_legend(&self) -> Element<'_, Message> {
        let g = &self.graph;
        let mut col = Column::new().spacing(6);
        col = col.push(text("Tables").size(13).font(bold()));
        if g.tables.is_empty() {
            col = col.push(text("No tables with records.").size(12).color(MUTED));
        }
        for t in &g.tables {
            col = col.push(
                row![
                    text("●").color(t.color),
                    text(&t.name).size(14),
                    text(format!("({})", self.table_count(t))).size(12).color(MUTED)
                ]
                .spacing(6),
            );
        }
        col = col.push(text("→  relation (RELATE)").size(14));
        col = col.push(
            text("A record in a relation table, linking in → out. Labelled with the table name; can have fields of its own.")
                .size(12)
                .color(MUTED),
        );
        col.into()
    }

    fn details(&self) -> Element<'_, Message> {
        let g = &self.graph;
        match self.selection {
            None => text("Nothing selected.").size(12).color(MUTED).into(),
            Some(Selection::Node(i)) => {
                let n = &g.nodes[i];
                let outgoing: Vec<String> = g
                    .edges
                    .iter()
                    .filter(|e| e.from == i)
                    .map(|e| format!("{} → {}", e.label, g.nodes[e.to].label))
                    .collect();
                let incoming: Vec<String> = g
                    .edges
                    .iter()
                    .filter(|e| e.to == i)
                    .map(|e| format!("{} → {}", g.nodes[e.from].label, e.label))
                    .collect();
                column![
                    text(&n.label).size(15).font(bold()),
                    text(&n.id).size(12).font(Font::MONOSPACE),
                    heading("Outgoing"),
                    list(outgoing),
                    heading("Incoming"),
                    list(incoming),
                    heading("Fields"),
                    json_block(&n.data),
                ]
                .spacing(6)
                .into()
            }
            Some(Selection::Edge(i)) => {
                let e = &g.edges[i];
                column![
                    text(format!("{} → {} → {}", g.nodes[e.from].label, e.label, g.nodes[e.to].label))
                        .size(15)
                        .font(bold()),
                    text(format!("Relation record {}", e.id)).size(12).color(MUTED),
                    heading("Fields"),
                    json_block(&e.data),
                ]
                .spacing(6)
                .into()
            }
        }
    }
}

fn visualization_is_standard(app: &App) -> bool {
    app.config.graph.visualization == Visualization::Standard
}

pub(super) fn bold() -> Font {
    Font { weight: iced::font::Weight::Bold, ..Font::default() }
}

pub(super) fn heading(label: &str) -> Element<'_, Message> {
    container(text(label.to_uppercase()).size(12).color(Color::from_rgb(0.33, 0.33, 0.33)))
        .padding(iced::Padding { top: 10.0, ..iced::Padding::ZERO })
        .into()
}

/// The chosen view's own options, in a framed box under its name.
fn view_box<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    column![
        heading(title),
        container(content).padding(10).width(Fill).style(|_| container::Style {
            background: Some(Color::from_rgb(0.97, 0.97, 0.98).into()),
            border: iced::Border { color: Color::from_rgb(0.85, 0.85, 0.88), width: 1.0, radius: 6.0.into() },
            ..container::Style::default()
        }),
    ]
    .spacing(6)
    .into()
}

fn list(items: Vec<String>) -> Element<'static, Message> {
    if items.is_empty() {
        return text("none").size(12).color(MUTED).into();
    }
    Column::with_children(items.into_iter().map(|i| text(format!("• {i}")).size(13).into()))
        .spacing(2)
        .into()
}

/// JSON pretty-printed in a grey box.
pub(super) fn json_block(data: &serde_json::Value) -> Element<'static, Message> {
    let json = serde_json::to_string_pretty(data).unwrap_or_default();
    container(text(json).size(12).font(Font::MONOSPACE))
        .padding(8)
        .width(Fill)
        .style(|_| container::background(Color::from_rgb(0.95, 0.95, 0.95)))
        .into()
}

/// "any party" / "any template" and the names, for a pick list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    value: Option<String>,
    label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

impl App {
    /// The Canton view's filters, as alt-canton's explorer has them: node
    /// kinds, party, template, contract status, an offset range and a search.
    fn canton_filters_view(&self) -> Element<'_, Message> {
        let f = &self.canton_filters;
        let summary = &self.graph.canton_summary;
        let counts = &summary.counts;
        let mut col = Column::new().spacing(5);

        col = col.push(heading("Layout"));
        col = col.push(
            row![
                text("Transactions side by side").size(13).width(Fill),
                text_input("3", &self.canton_columns).on_input(Message::CantonColumns).width(56).size(13),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        );
        col = col.push(
            checkbox(self.config.graph.canton_follow_newest)
                .label("Follow the newest transactions")
                .on_toggle(Message::ToggleFollowNewest)
                .text_size(12),
        );

        col = col.push(heading("Node kinds"));
        for kind in NodeKind::ALL {
            col = col.push(
                row![
                    checkbox(f.kinds.contains(&kind))
                        .on_toggle(move |on| Message::CantonFilter(F::Kind(kind, on)))
                        .size(15),
                    text("●").color(canton_draw::kind_color(kind)),
                    text(kind.name()).size(14).width(Fill),
                    text(counts.get(&kind).copied().unwrap_or(0).to_string()).size(13).color(MUTED),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center),
            );
        }
        if let Some(&other) = counts.get(&NodeKind::Other) {
            col = col.push(
                row![
                    checkbox(f.kinds.contains(&NodeKind::Other))
                        .on_toggle(|on| Message::CantonFilter(F::Kind(NodeKind::Other, on)))
                        .size(15),
                    text("●").color(canton_draw::kind_color(NodeKind::Other)),
                    text("Other tables").size(14).width(Fill),
                    text(other.to_string()).size(13).color(MUTED),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center),
            );
        }
        let small = |label: &'static str, kinds: &[NodeKind]| {
            button(text(label).size(12))
                .style(button::secondary)
                .padding([2, 8])
                .on_press(Message::CantonFilter(F::Kinds(kinds.to_vec())))
        };
        col = col.push(
            row![
                small("all", &NodeKind::ALL),
                small(
                    "events",
                    &[NodeKind::Transaction, NodeKind::Created, NodeKind::Exercised, NodeKind::Fetched, NodeKind::Reassignment]
                ),
                small("topology", &[NodeKind::Topology, NodeKind::Party]),
            ]
            .spacing(6),
        );

        let choices = |any: &str, names: Vec<(String, String)>| -> Vec<Choice> {
            std::iter::once(Choice { value: None, label: any.to_string() })
                .chain(names.into_iter().map(|(value, label)| Choice { value: Some(value), label }))
                .collect()
        };
        let parties = choices(
            "any party",
            summary.parties.iter().map(|p| (p.clone(), canton::ellipsize(p, 34))).collect(),
        );
        let party = parties.iter().find(|c| c.value == f.party).cloned();
        col = col.push(heading("Party"));
        col = col.push(
            pick_list(parties, party, |c: Choice| Message::CantonFilter(F::Party(c.value))).text_size(13).width(Fill),
        );
        let templates = choices("any template", summary.templates.iter().map(|t| (t.clone(), t.clone())).collect());
        let template = templates.iter().find(|c| c.value == f.template).cloned();
        col = col.push(heading("Template"));
        col = col.push(
            pick_list(templates, template, |c: Choice| Message::CantonFilter(F::Template(c.value)))
                .text_size(13)
                .width(Fill),
        );

        col = col.push(heading("Contracts"));
        let status = |label: &'static str, value: StatusFilter| {
            button(text(label).size(13).width(Fill).align_x(iced::Alignment::Center))
                .width(Fill)
                .style(if f.status == value { button::primary } else { button::secondary })
                .on_press(Message::CantonFilter(F::Status(value)))
        };
        col = col.push(
            row![
                status("all", StatusFilter::All),
                status("active", StatusFilter::Active),
                status("archived", StatusFilter::Archived)
            ]
            .spacing(2),
        );

        // Which transactions are read from the database: the latest n, a
        // page older or newer than the loaded ones, or an exact offset range.
        col = col.push(heading("Transactions"));
        let n = self.canton_n();
        let latest = Message::CantonPage(graph::CantonWindow::Latest(n));
        col = col.push(
            row![
                text("Latest").size(13),
                text_input("50", &self.canton_count)
                    .on_input(Message::CantonCount)
                    .on_submit(latest.clone())
                    .width(64)
                    .size(13),
                button(text("Show").size(12)).style(button::secondary).on_press(latest),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        );
        let (lo, hi) = self.canton_offsets.unwrap_or((0, 0));
        let page = |label: String, window: Option<graph::CantonWindow>| {
            button(text(label).size(12)).style(button::secondary).on_press_maybe(window.map(Message::CantonPage))
        };
        col = col.push(
            row![
                page(format!("◀ {n} older"), self.canton_offsets.map(|_| graph::CantonWindow::Before(lo, n))),
                page(format!("{n} newer ▶"), self.canton_offsets.map(|_| graph::CantonWindow::After(hi, n))),
            ]
            .spacing(6),
        );

        let from = f.offset_from.map(|o| o.to_string()).unwrap_or_default();
        let to = f.offset_to.map(|o| o.to_string()).unwrap_or_default();
        let range = Message::CantonPage(match (f.offset_from, f.offset_to) {
            (None, None) => graph::CantonWindow::Latest(n),
            (from, to) => graph::CantonWindow::Range(from.unwrap_or(i64::MIN), to.unwrap_or(i64::MAX)),
        });
        col = col.push(
            row![
                text_input("from offset", &from)
                    .on_input(|t| Message::CantonFilter(F::OffsetFrom(t)))
                    .on_submit(range.clone())
                    .size(13),
                text("–").size(13),
                text_input("to offset", &to)
                    .on_input(|t| Message::CantonFilter(F::OffsetTo(t)))
                    .on_submit(range)
                    .size(13),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        );
        let loaded = match self.canton_offsets {
            Some((lo, hi)) => format!(
                "Loaded offsets {lo}–{hi}. Press Enter in an offset box to load exactly that range (an empty box means no limit on that side)."
            ),
            None => "No transactions loaded.".to_string(),
        };
        col = col.push(text(loaded).size(12).color(MUTED));
        if self.canton_left_out > 0 {
            col = col.push(
                text(format!(
                    "{} of this window weren't drawn: it has more than {} (Settings → max records in a query result). Load fewer transactions, or raise the limit.",
                    plural(self.canton_left_out, "record"),
                    self.config.graph.max_result_records
                ))
                .size(12)
                .color(WARNING),
            );
        }

        col = col.push(heading("Search"));
        col = col.push(
            text_input("contract id, choice, party…", &f.search)
                .on_input(|t| Message::CantonFilter(F::Search(t)))
                .size(13),
        );
        col = col.push(
            checkbox(self.config.graph.fit_on_filter)
                .label("Back to the top when filtering")
                .on_toggle(Message::ToggleFitOnFilter)
                .text_size(12),
        );
        col = col.push(
            button(text("Reset filters").size(13).width(Fill).align_x(iced::Alignment::Center))
                .width(Fill)
                .style(button::secondary)
                .on_press(Message::CantonFilter(F::Reset)),
        );
        let shown = self.graph.nodes.iter().filter(|n| n.card.shown).count();
        col = col.push(text(format!("{shown} of {} records shown", self.graph.nodes.len())).size(12).color(MUTED));
        if shown == 0 && !self.graph.nodes.is_empty() {
            col = col.push(text("Nothing matches these filters.").size(12).color(WARNING));
        }
        if !self.graph.looks_like_event_graph() {
            col = col.push(
                text(
                    "No Canton event graph found: expected tables such as transaction, created, exercised and party, \
                     with action, consequence, target and consumes relations. Settings → Data → Load Canton sample adds one.",
                )
                .size(12)
                .color(WARNING),
            );
        }
        col.into()
    }
}
