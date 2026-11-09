//! The query panel under the graph: a SurrealQL editor and the results.

use iced::keyboard::{Key, key::Named};
use iced::widget::text_editor::{Binding, KeyPress};
use iced::widget::{Column, button, checkbox, column, container, row, rule, scrollable, text, text_editor, text_input};
use std::collections::HashMap;

use iced::{Alignment, Color, Element, Fill, Font, Length};
use surrealdb_explorer::assistant::Model;
use surrealdb_explorer::assistant::history::Exchange;
use surrealdb_explorer::config::AssistantMode;
use surrealdb_explorer::query::QueryOutcome;

use crate::gui::app::{App, Message, plural};
use crate::gui::canvas::ANSWER_RING as ANSWER;
use crate::gui::sidebar::{ERROR, MUTED, SUCCESS, WARNING, bold};

/// Results longer than this many lines per statement are cut, so a huge
/// result can't make the window sluggish.
const MAX_RESULT_LINES: usize = 400;
/// Width of the chat with Claude, left of the editor.
const CHAT_WIDTH: f32 = 380.0;

impl App {
    pub(super) fn query_panel(&self) -> Element<'_, Message> {
        let toggle = button(text(if self.query_open { "▾ Query" } else { "▸ Query" }).size(13))
            .style(button::text)
            .on_press(Message::ToggleQuery);
        if !self.query_open {
            return container(toggle).padding([2, 8]).width(Fill).into();
        }
        let toggle = container(toggle).padding([2, 0]);

        let editor = text_editor(&self.query_text)
            .placeholder("SurrealQL, e.g. SELECT * FROM person WHERE marketing = true;")
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

        let mut status = row![run, show].spacing(8).align_y(Alignment::Center);
        if self.suggestion_in_editor().is_some_and(|s| s.context_query.is_some()) {
            status = status.push(
                checkbox(self.config.assistant.show_connections)
                    .label("Show how they connect")
                    .on_toggle(Message::ToggleConnections)
                    .text_size(12),
            );
        }
        if self.query_outcome.is_some() {
            status = status.push(
                checkbox(self.config.graph.connect_results)
                    .label("Connect shown records")
                    .on_toggle(Message::ToggleConnectResults)
                    .text_size(12),
            );
        }
        if let Some(summary) = self.query_outcome.as_ref().map(summary) {
            status = status.push(summary);
        }

        let results: Element<'_, Message> = match &self.query_outcome {
            None => text("Results appear here. A result with records replaces the graph; Show the whole graph brings the overview back.")
                .size(12)
                .color(MUTED)
                .into(),
            Some(outcome) => {
                let mut col = Column::new().spacing(8);
                if let Some(answer) = self.answer_view() {
                    col = col.push(answer);
                }
                scrollable(col.push(self.results_view(outcome))).height(Fill).into()
            }
        };

        column![
            toggle,
            row![
                self.chat_panel(),
                rule::vertical(1),
                column![editor, status, results].spacing(6).width(Fill),
            ]
            .spacing(10)
            .height(Fill),
        ]
        .padding([4, 8])
        .height(Length::Fixed(460.0))
        .into()
    }

    /// The answer to the last query, as shown in the graph: the records that
    /// answer it and how they connect.
    fn answer_view(&self) -> Option<Element<'_, Message>> {
        const MAX_LISTED: usize = 30;
        let (answer, edges) = self.result_indices()?;
        let g = &self.graph;

        let mut names: Vec<String> = answer.iter().map(|&i| g.nodes[i].label.clone()).collect();
        names.sort();
        let mut connections: Vec<String> = edges
            .iter()
            .map(|&i| {
                let e = &g.edges[i];
                format!("{} → {} → {}", g.nodes[e.from].label, e.label, g.nodes[e.to].label)
            })
            .collect();
        connections.sort();

        let mut answer_col = column![
            row![
                text("◯").color(ANSWER),
                text(format!("Answer: {}", plural(names.len(), "record"))).size(13).font(bold()),
            ]
            .spacing(6),
            listed(names, MAX_LISTED),
        ]
        .spacing(4)
        .width(Length::FillPortion(1));
        if let Some(view) = self.result_view.as_ref().filter(|v| v.left_out > 0) {
            answer_col = answer_col.push(
                text(format!(
                    "Only the first {} of {} returned records are drawn; the limit is in Settings.",
                    view.found - view.left_out,
                    view.found
                ))
                .size(12)
                .color(WARNING),
            );
        }
        answer_col = answer_col.push(
            button(text("Show the whole graph").size(12)).style(button::text).padding(0).on_press(Message::ShowAll),
        );

        let mut body = row![answer_col].spacing(24);
        if !connections.is_empty() {
            body = body.push(
                column![text("How they connect").size(13).font(bold()), listed(connections, MAX_LISTED)]
                    .spacing(4)
                    .width(Length::FillPortion(2)),
            );
        }
        Some(
            container(body)
                .padding(8)
                .width(Fill)
                .style(|_| container::background(Color::from_rgb(1.0, 0.97, 0.92)))
                .into(),
        )
    }

    /// Each statement's result: as path rows when it is a list of paths,
    /// otherwise as JSON.
    fn results_view<'a>(&'a self, outcome: &'a QueryOutcome) -> Element<'a, Message> {
        let results = match &outcome.statements {
            Err(e) => return text(e.clone()).size(12).font(Font::MONOSPACE).color(ERROR).into(),
            Ok(results) => results,
        };
        let mut col = Column::new().spacing(8).width(Fill);
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
                heading = heading.push(
                    button(text(if self.show_json { "Show paths" } else { "Show JSON" }).size(11))
                        .style(button::text)
                        .padding(0)
                        .on_press(Message::ToggleJson),
                );
            }
            let body: Element<'a, Message> = match (&r.result, paths) {
                (Ok(_), Some(paths)) if !self.show_json => self.path_rows(paths),
                (Ok(v), _) => grey_box(cut(serde_json::to_string_pretty(v).unwrap_or_default())),
                (Err(e), _) => grey_box(e.clone()),
            };
            col = col.push(column![heading, body].spacing(3));
        }
        col.into()
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

    /// The chat with Claude: every question and answer, newest at the
    /// bottom, and the box to ask the next question.
    fn chat_panel(&self) -> Element<'_, Message> {
        let can_ask = !self.asking && self.db.is_some();

        let header = row![
            text("Ask Claude").size(13).font(bold()).width(Fill),
            button(text("Clear history").size(11))
                .style(button::text)
                .padding(0)
                .on_press_maybe((!self.chat.is_empty() && !self.asking).then_some(Message::ClearChat)),
        ]
        .align_y(Alignment::Center);

        let mut entries = Column::new().spacing(12).padding([0, 10]);
        if self.chat.is_empty() {
            entries = entries.push(
                text("Ask about the data in plain English, or say how to change the query in the editor. Attach files, such as CSV or JSON data or a list of names: Claude sees them, and queries can read them, e.g. movies.csv as $movies. Claude writes the SurrealQL; it goes in the editor for you to check and run.")
                    .size(12)
                    .color(MUTED),
            );
        }
        let last = self.chat.len().checked_sub(1);
        for (i, exchange) in self.chat.iter().enumerate() {
            entries = entries.push(self.chat_entry(i, exchange, Some(i) == last, can_ask));
        }
        if self.asking {
            let who = match self.config.assistant.mode {
                AssistantMode::Auto => "Haiku (Opus if needed)",
                AssistantMode::Haiku => "Haiku",
                AssistantMode::Opus => "Opus",
            };
            let key_note = if self.assistant.is_none() { " 1Password may ask you to approve reading the API key." } else { "" };
            entries = entries.push(text(format!("Asking {who}…{key_note}")).size(12).color(MUTED));
        }

        let input = text_input("Ask, or say how to change the query in the editor", &self.question)
            .on_input(Message::QuestionEdited)
            .on_submit_maybe(can_ask.then_some(Message::Ask))
            .size(13);
        let ask = button(text(if self.asking { "Asking…" } else { "Ask" }).size(13))
            .style(button::primary)
            .on_press_maybe((can_ask && !self.question.trim().is_empty()).then_some(Message::Ask));

        let attach = button(text("Attach…").size(13))
            .style(button::secondary)
            .on_press_maybe((!self.asking && !self.picking_files).then_some(Message::AttachFiles));

        let mut col = column![
            header,
            // Anchored to the bottom, like a chat: new answers stay in view.
            scrollable(entries).anchor_bottom().height(Fill).width(Fill),
        ]
        .spacing(6);
        for (i, file) in self.attachments.iter().enumerate() {
            col = col.push(
                row![
                    text(format!("Attached: {}", file.label())).size(12).width(Fill),
                    button(text("Remove").size(11))
                        .style(button::text)
                        .padding(0)
                        .on_press_maybe((!self.asking).then_some(Message::RemoveAttachment(i))),
                ]
                .align_y(Alignment::Center),
            );
        }
        for problem in &self.attach_problems {
            col = col.push(text(problem).size(12).color(ERROR));
        }
        col.push(row![attach, input, ask].spacing(8).align_y(Alignment::Center))
        .width(CHAT_WIDTH)
        .into()
    }

    fn chat_entry<'a>(&self, i: usize, exchange: &'a Exchange, is_last: bool, can_ask: bool) -> Element<'a, Message> {
        let question = container(text(&exchange.question).size(13))
            .padding([4, 8])
            .style(|_| container::background(Color::from_rgb(0.9, 0.94, 1.0)));
        let mut col = column![question].spacing(4);
        if !exchange.attachments.is_empty() {
            col = col.push(text(format!("Attached: {}", exchange.attachments.join(", "))).size(11).color(MUTED));
        }

        match &exchange.answer {
            Err(e) => col = col.push(text(e).size(12).color(ERROR)),
            Ok(s) => {
                let mut by = s.model.name().to_string();
                if !s.notes.is_empty() {
                    by = format!("{by} ({})", s.notes.join("; "));
                }
                col = col.push(text(format!("{by}: {}", s.explanation)).size(12));
                col = col.push(
                    container(text(&s.query).size(11).font(Font::MONOSPACE))
                        .padding(4)
                        .width(Fill)
                        .style(|_| container::background(Color::from_rgb(0.95, 0.95, 0.95))),
                );
                if let Some(context) = &s.context_query {
                    col = col.push(
                        row![
                            text("How they connect").size(11).color(MUTED).width(Fill),
                            button(text("Put in editor").size(11))
                                .style(button::text)
                                .padding(0)
                                .on_press(Message::UseContext(i)),
                        ]
                        .align_y(Alignment::Center),
                    );
                    col = col.push(
                        container(text(context).size(11).font(Font::MONOSPACE).color(MUTED))
                            .padding(4)
                            .width(Fill)
                            .style(|_| container::background(Color::from_rgb(0.97, 0.97, 0.97))),
                    );
                }
                if let Some(problem) = &s.problem {
                    col = col.push(text(format!("The check found a problem: {problem}")).size(12).color(ERROR));
                } else if !s.confident {
                    col = col.push(text("Claude wasn't sure about this one; check the result.").size(12).color(WARNING));
                }

                let in_editor = self
                    .suggestion
                    .as_ref()
                    .is_some_and(|current| current.question == s.question && current.query == s.query)
                    && self.query_text.text().trim() == s.query.trim();
                let mut actions = row![].spacing(8).align_y(Alignment::Center);
                actions = if in_editor {
                    actions.push(text("In the editor").size(11).color(SUCCESS))
                } else {
                    actions.push(button(text("Use this query").size(11)).style(button::secondary).on_press(Message::UseAnswer(i)))
                };
                // After a restart the attached files' contents are gone, so Opus couldn't see them.
                let files_kept = s.attachments.iter().all(|a| a.is_loaded());
                if is_last && in_editor && files_kept && (s.model == Model::Haiku || s.problem.is_some()) {
                    actions = actions.push(
                        button(text("Try with Opus").size(11))
                            .style(button::secondary)
                            .on_press_maybe(can_ask.then_some(Message::RetryWithOpus)),
                    );
                }
                col = col.push(actions);
            }
        }
        col.into()
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

/// A bullet list of at most `max` items, with a note about the rest.
fn listed(items: Vec<String>, max: usize) -> Element<'static, Message> {
    let more = items.len().saturating_sub(max);
    let mut col = Column::with_children(items.into_iter().take(max).map(|i| text(format!("• {i}")).size(13).into()))
        .spacing(2);
    if more > 0 {
        col = col.push(text(format!("… and {more} more")).size(12).color(MUTED));
    }
    col.into()
}

/// Monospace text in a grey box.
fn grey_box(body: String) -> Element<'static, Message> {
    container(text(body).size(12).font(Font::MONOSPACE))
        .padding(6)
        .width(Fill)
        .style(|_| container::background(Color::from_rgb(0.95, 0.95, 0.95)))
        .into()
}
