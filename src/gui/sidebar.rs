//! The sidebar: graph summary, legend, actions and the selected record.

use iced::widget::{Column, button, checkbox, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Color, Element, Fill, Font};
use surrealdb_explorer::canton::{NodeKind, StatusFilter};
use surrealdb_explorer::config::Visualization;

use crate::gui::app::{App, CantonFilterChange as F, DataAction, Message, Selection, plural};
use crate::gui::canton_draw;
use surrealdb_explorer::canton;

pub(super) const SIDEBAR_WIDTH: f32 = 320.0;
pub(super) const MUTED: Color = Color::from_rgb(0.47, 0.47, 0.47);
pub(super) const WARNING: Color = Color::from_rgb(0.67, 0.4, 0.0);
pub(super) const ERROR: Color = Color::from_rgb(0.7, 0.0, 0.0);
pub(super) const SUCCESS: Color = Color::from_rgb(0.1, 0.5, 0.2);

impl App {
    pub(super) fn sidebar(&self) -> Element<'_, Message> {
        let g = &self.graph;
        let mut col = Column::new().spacing(6).padding(16).width(SIDEBAR_WIDTH);

        col = col.push(
            row![
                text("SurrealDB Explorer").size(18).width(Fill),
                button(text("Settings").size(12)).style(button::secondary).on_press(Message::OpenSettings),
            ]
            .align_y(iced::Alignment::Center),
        );
        col = col.push(
            text(format!(
                "{} in {}, {}",
                plural(g.nodes.len(), "record"),
                plural(g.tables.len(), "table"),
                plural(g.edges.len(), "relation")
            ))
            .size(12)
            .color(MUTED),
        );
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
        if visualization == Visualization::Canton {
            col = col.push(self.canton_filters_view());
        } else {
            col = col.push(self.tables_legend());
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
        col = col.push(
            row![
                button(text("Reload").size(13)).on_press(Message::Reload),
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
        if let Some(notice) = &self.notice {
            col = col.push(text(notice).size(12).color(MUTED));
        }
        col = col.push(
            text("Hover a record to highlight its neighbours. Click for details; right-click for more actions. Drag to move, scroll to zoom, drag the background to pan.")
                .size(12)
                .color(MUTED),
        );

        col = col.push(heading("Selected"));
        col = col.push(self.details());

        scrollable(col).height(Fill).into()
    }

    fn tables_legend(&self) -> Element<'_, Message> {
        let g = &self.graph;
        let mut col = Column::new().spacing(6);
        col = col.push(heading("Tables"));
        if g.tables.is_empty() {
            col = col.push(text("No tables with records.").size(12).color(MUTED));
        }
        for t in &g.tables {
            col = col.push(
                row![
                    text("●").color(t.color),
                    text(&t.name).size(14),
                    text(format!("({})", t.count)).size(12).color(MUTED)
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

pub(super) fn bold() -> Font {
    Font { weight: iced::font::Weight::Bold, ..Font::default() }
}

pub(super) fn heading(label: &str) -> Element<'_, Message> {
    container(text(label.to_uppercase()).size(12).color(Color::from_rgb(0.33, 0.33, 0.33)))
        .padding(iced::Padding { top: 10.0, ..iced::Padding::ZERO })
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
        let model = self.graph.canton_model();
        let counts = model.counts();
        let mut col = Column::new().spacing(5);

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
            model.parties().into_iter().map(|p| (p.clone(), canton::ellipsize(&p, 34))).collect(),
        );
        let party = parties.iter().find(|c| c.value == f.party).cloned();
        col = col.push(heading("Party"));
        col = col.push(
            pick_list(parties, party, |c: Choice| Message::CantonFilter(F::Party(c.value))).text_size(13).width(Fill),
        );
        let templates = choices("any template", model.templates().into_iter().map(|t| (t.clone(), t)).collect());
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

        col = col.push(heading("Offset"));
        let from = f.offset_from.map(|o| o.to_string()).unwrap_or_default();
        let to = f.offset_to.map(|o| o.to_string()).unwrap_or_default();
        col = col.push(
            row![
                text_input("from", &from).on_input(|t| Message::CantonFilter(F::OffsetFrom(t))).size(13),
                text("–").size(13),
                text_input("to", &to).on_input(|t| Message::CantonFilter(F::OffsetTo(t))).size(13),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        );

        col = col.push(heading("Search"));
        col = col.push(
            text_input("contract id, choice, party…", &f.search)
                .on_input(|t| Message::CantonFilter(F::Search(t)))
                .size(13),
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
