//! The query panel under the main area (a SurrealQL editor and Run), and the
//! Table view of the last result.

use std::collections::HashMap;

use iced::keyboard::{Key, key::Named};
use iced::widget::text_editor::{Binding, KeyPress};
use iced::widget::{
    Column, button, checkbox, column, container, pick_list, row, scrollable, space, text, text_editor, text_input,
};
use iced::{Alignment, Color, Element, Fill, Font, Length};
use surrealdb_explorer::query::QueryOutcome;

use crate::gui::app::{App, Message, plural};
use crate::gui::sidebar::{ERROR, MUTED, SUCCESS, bold};

/// Results longer than this many lines per statement are cut, so a huge
/// result can't make the window sluggish.
const MAX_RESULT_LINES: usize = 400;

impl App {
    pub(super) fn query_panel(&self) -> Element<'_, Message> {
        let toggle = button(text(if self.query_open { "▾ Query" } else { "▸ Query" }).size(13))
            .style(button::text)
            .on_press(Message::ToggleQuery);
        if !self.query_open {
            return container(toggle).padding([2, 8]).width(Fill).into();
        }

        let editor = text_editor(&self.query_text)
            .placeholder("SurrealQL, e.g. SELECT * FROM person WHERE name = 'Tom Hanks';")
            .on_action(Message::QueryEdited)
            .key_binding(run_on_cmd_enter)
            .font(Font::MONOSPACE)
            .size(13)
            .height(110);

        let run = button(text(if self.query_running { "Running…" } else { "Run  ⌘↩" }).size(13))
            .style(button::primary)
            .on_press_maybe((!self.query_running && self.db.is_some()).then_some(Message::RunQuery));
        let show = button(text("Show in graph").size(13))
            .style(button::secondary)
            .on_press_maybe(self.query_outcome.as_ref().map(|_| Message::ShowQueryInGraph));
        let mut status = row![
            run,
            show,
            checkbox(self.config.graph.connect_results)
                .label("Connect shown records")
                .on_toggle(Message::ToggleConnectResults)
                .text_size(12),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(summary) = self.query_outcome.as_ref().map(summary) {
            status = status.push(summary);
        }

        column![container(toggle).padding([2, 0]), self.saved_queries_row(), editor, status]
            .spacing(6)
            .padding([4, 8])
            .height(Length::Shrink)
            .into()
    }

    /// Saved queries: pick one to put it in the editor, delete it, or save
    /// the editor's query under a name.
    fn saved_queries_row(&self) -> Element<'_, Message> {
        let names = self.saved_queries.names();
        let picked = self.saved_query.clone().filter(|n| names.contains(n));
        let placeholder = if names.is_empty() { "No saved queries" } else { "Saved queries…" };
        let picker = pick_list(names, picked.clone(), Message::PickSavedQuery)
            .placeholder(placeholder)
            .text_size(13)
            .width(Length::FillPortion(3));

        let delete: Element<'_, Message> = match (&picked, self.confirm_delete_query) {
            (Some(name), true) => row![
                text(format!("Delete \"{name}\"?")).size(12),
                button(text("Delete").size(12))
                    .style(button::danger)
                    .on_press(Message::ConfirmDeleteSavedQuery(true)),
                button(text("Keep").size(12))
                    .style(button::secondary)
                    .on_press(Message::ConfirmDeleteSavedQuery(false)),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
            _ => button(text("Delete").size(13))
                .style(button::secondary)
                .on_press_maybe(picked.as_ref().map(|_| Message::AskDeleteSavedQuery))
                .into(),
        };

        let name = text_input("Name for this query", &self.saved_query_name)
            .on_input(Message::SavedQueryName)
            .on_submit(Message::SaveQuery)
            .size(13)
            .width(Length::FillPortion(2));
        let can_save = !self.saved_query_name.trim().is_empty() && !self.query_text.text().trim().is_empty();
        let save = button(text("Save query").size(13))
            .style(button::secondary)
            .on_press_maybe(can_save.then_some(Message::SaveQuery));

        row![picker, delete, space::horizontal().width(16), name, save]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
    }

    /// The Table view: each statement's result, as path rows when it is a
    /// list of paths, otherwise as JSON.
    pub(super) fn table_view(&self) -> Element<'_, Message> {
        let Some(outcome) = &self.query_outcome else {
            return container(text("Run a query to see its result here.").size(13).color(MUTED))
                .padding(16)
                .into();
        };
        let results = match &outcome.statements {
            Err(e) => return container(grey_box(e.clone())).padding(12).into(),
            Ok(results) => results,
        };
        let mut col = Column::new().spacing(10).padding(12).width(Fill);
        for (i, r) in results.iter().enumerate() {
            let time = r
                .time
                .map(|t| format!(" · {:.2} ms", t.as_secs_f64() * 1000.0))
                .unwrap_or_default();
            let paths = r.result.as_ref().ok().and_then(|v| self.paths(v));
            let mut heading = row![
                text(format!("Statement {}", i + 1)).size(12).font(bold()),
                text(format!("{}{time}", if r.result.is_ok() { "ok" } else { "error" }))
                    .size(12)
                    .color(if r.result.is_ok() { SUCCESS } else { ERROR }),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            if let Some(paths) = &paths {
                heading = heading.push(text(plural(paths.len(), "path")).size(12).color(MUTED));
            }
            let body: Element<'_, Message> = match (&r.result, paths) {
                (Ok(_), Some(paths)) => self.path_rows(paths),
                (Ok(v), None) => grey_box(cut(serde_json::to_string_pretty(v).unwrap_or_default())),
                (Err(e), _) => grey_box(e.clone()),
            };
            col = col.push(column![heading, body].spacing(4));
        }
        scrollable(col).height(Fill).width(Fill).into()
    }

    /// A result that is a list of paths, each `[record, relation, record, …]`
    /// with every record and relation in the graph, as graph indices.
    fn paths(&self, value: &serde_json::Value) -> Option<Vec<Vec<usize>>> {
        let rows = value.as_array().filter(|rows| !rows.is_empty())?;
        let nodes: HashMap<&str, usize> =
            self.graph.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let edges: HashMap<&str, usize> =
            self.graph.edges.iter().enumerate().map(|(i, e)| (e.id.as_str(), i)).collect();
        rows.iter()
            .map(|row| {
                let ids = row.as_array().filter(|ids| ids.len() >= 3 && ids.len() % 2 == 1)?;
                let mut steps = Vec::with_capacity(ids.len());
                for (k, id) in ids.iter().enumerate() {
                    let id = id.as_str()?;
                    steps.push(if k % 2 == 0 { *nodes.get(id)? } else { *edges.get(id)? });
                }
                // Each relation must join the records on either side of it.
                for k in (1..steps.len()).step_by(2) {
                    let e = &self.graph.edges[steps[k]];
                    let (a, b) = (steps[k - 1], steps[k + 1]);
                    if !((e.from == a && e.to == b) || (e.from == b && e.to == a)) {
                        return None;
                    }
                }
                Some(steps)
            })
            .collect()
    }

    /// Paths drawn like `Tom Hanks —ACTED_IN→ Big ←DIRECTED— Penny Marshall`.
    fn path_rows(&self, paths: Vec<Vec<usize>>) -> Element<'_, Message> {
        const MAX_ROWS: usize = 300;
        let g = &self.graph;
        let more = paths.len().saturating_sub(MAX_ROWS);
        let mut col = Column::new().spacing(4).width(Fill);
        for steps in paths.into_iter().take(MAX_ROWS) {
            let mut line = row![].spacing(4).align_y(Alignment::Center);
            for (k, &step) in steps.iter().enumerate() {
                if k % 2 == 0 {
                    let n = &g.nodes[step];
                    let tint = Color { a: 0.25, ..n.color };
                    line = line.push(
                        container(text(n.label.clone()).size(12))
                            .padding([2, 8])
                            .style(move |_| container::background(tint).border(iced::border::rounded(10))),
                    );
                } else {
                    let e = &g.edges[step];
                    let forward = e.from == steps[k - 1];
                    let label = e.label.to_uppercase();
                    let arrow = if forward { format!("—{label}→") } else { format!("←{label}—") };
                    line = line.push(text(arrow).size(11).font(Font::MONOSPACE).color(MUTED));
                }
            }
            col = col.push(line);
        }
        if more > 0 {
            col = col.push(text(format!("… and {more} more paths")).size(12).color(MUTED));
        }
        container(col).padding(6).width(Fill).into()
    }

}

/// ⌘↩ runs the query; every other key does what it normally does.
fn run_on_cmd_enter(key: KeyPress) -> Option<Binding<Message>> {
    if key.key == Key::Named(Named::Enter) && key.modifiers.command() {
        Some(Binding::Custom(Message::RunQuery))
    } else {
        Binding::from_key_press(key)
    }
}

/// e.g. "3 statements · 2 ok, 1 error · 4.2 ms"
fn summary(outcome: &QueryOutcome) -> Element<'static, Message> {
    let time = format!("{:.1} ms", outcome.elapsed.as_secs_f64() * 1000.0);
    match &outcome.statements {
        Err(_) => text(format!("failed · {time}")).size(12).color(ERROR).into(),
        Ok(results) => {
            let errors = results.iter().filter(|r| r.result.is_err()).count();
            let color = if errors > 0 { ERROR } else { SUCCESS };
            text(format!(
                "{} statement{} · {} ok, {} error{} · {time}",
                results.len(),
                if results.len() == 1 { "" } else { "s" },
                results.len() - errors,
                errors,
                if errors == 1 { "" } else { "s" },
            ))
            .size(12)
            .color(color)
            .into()
        }
    }
}

fn cut(text: String) -> String {
    let lines = text.lines().count();
    if lines <= MAX_RESULT_LINES {
        return text;
    }
    let kept: Vec<&str> = text.lines().take(MAX_RESULT_LINES).collect();
    format!("{}\n… {} more lines not shown", kept.join("\n"), lines - MAX_RESULT_LINES)
}

/// Monospace text in a grey box.
fn grey_box(body: String) -> Element<'static, Message> {
    container(text(body).size(12).font(Font::MONOSPACE))
        .padding(6)
        .width(Fill)
        .style(|_| container::background(Color::from_rgb(0.95, 0.95, 0.95)))
        .into()
}
