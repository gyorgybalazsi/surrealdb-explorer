//! The sidebar: graph summary, legend, actions and the selected record.

use iced::widget::{Column, button, column, container, pick_list, row, scrollable, text};
use iced::{Color, Element, Fill, Font};
use surrealdb_explorer::canton::{NodeKind, RelKind};
use surrealdb_explorer::config::Visualization;

use crate::gui::app::{App, DataAction, Message, Selection, plural};
use crate::gui::canton_draw;

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
            col = col.push(canton_legend(g.looks_like_event_graph()));
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
        col = col.push(button(text("Reload").size(13)).on_press(Message::Reload));
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

/// What the shapes and arrows mean in the Canton event graph.
fn canton_legend(has_event_graph: bool) -> Element<'static, Message> {
    let mut col = Column::new().spacing(4);
    col = col.push(heading("Events"));
    let shapes = [
        (NodeKind::Transaction, "▬", "a committed transaction"),
        (NodeKind::Created, "●", "a contract created (pale once consumed)"),
        (NodeKind::Exercised, "◆", "a choice exercised"),
        (NodeKind::Party, "⬢", "a party that requested a transaction"),
    ];
    for (kind, symbol, meaning) in shapes {
        let (color, _, _) = canton_draw::colors(kind);
        col = col.push(
            row![text(symbol).color(color), text(kind.name()).size(14), text(meaning).size(12).color(MUTED)]
                .spacing(6)
                .align_y(iced::Alignment::Center),
        );
    }
    col = col.push(heading("Relations"));
    let meanings = [
        (RelKind::Action, "a transaction's root event"),
        (RelKind::Consequence, "an event an exercise caused (dashed)"),
        (RelKind::Target, "the contract exercised on (dotted)"),
        (RelKind::Consumes, "the contract the exercise archived"),
        (RelKind::Requested, "who submitted the transaction"),
    ];
    for (kind, meaning) in meanings {
        let (color, _, _) = canton_draw::edge_style(kind);
        col = col.push(
            row![text("→").color(color), text(kind.name()).size(12), text(meaning).size(12).color(MUTED)]
                .spacing(6)
                .align_y(iced::Alignment::Center),
        );
    }
    if !has_event_graph {
        col = col.push(
            text("No Canton event graph found: expected tables transaction, created, exercised and party, \
                  with action, consequence, target, consumes and requested relations. \
                  Settings → Data → Load Canton sample adds one.")
                .size(12)
                .color(crate::gui::sidebar::WARNING),
        );
    }
    col.into()
}
