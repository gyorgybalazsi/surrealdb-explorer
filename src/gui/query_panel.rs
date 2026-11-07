//! The query panel under the graph: a SurrealQL editor and the results.

use iced::keyboard::{Key, key::Named};
use iced::widget::text_editor::{Binding, KeyPress};
use iced::widget::{Column, button, column, container, row, scrollable, text, text_editor, text_input};
use iced::{Alignment, Color, Element, Fill, Font, Length};
use surrealdb_explore::assistant::Model;
use surrealdb_explore::config::AssistantMode;
use surrealdb_explore::query::QueryOutcome;

use crate::gui::app::{App, Message};
use crate::gui::sidebar::{ERROR, MUTED, SUCCESS, WARNING, bold};

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
        if let Some(summary) = self.query_outcome.as_ref().map(summary) {
            status = status.push(summary);
        }

        let results: Element<'_, Message> = match &self.query_outcome {
            None => text("Results appear here. Queries that change data reload the graph.")
                .size(12)
                .color(MUTED)
                .into(),
            Some(outcome) => scrollable(results_view(outcome)).height(Fill).into(),
        };

        column![toggle, self.question_box(), editor, status, results]
            .spacing(6)
            .padding([4, 8])
            .height(Length::Fixed(460.0))
            .into()
    }

    /// Asking in plain English, and what came back.
    fn question_box(&self) -> Element<'_, Message> {
        let can_ask = !self.asking && self.db.is_some();
        let input = text_input("Ask in plain English, e.g. who does alice know?", &self.question)
            .on_input(Message::QuestionEdited)
            .on_submit_maybe(can_ask.then_some(Message::Ask))
            .size(13);
        let ask = button(text(if self.asking { "Asking…" } else { "Ask Claude" }).size(13))
            .style(button::primary)
            .on_press_maybe((can_ask && !self.question.trim().is_empty()).then_some(Message::Ask));
        let mut col = column![row![input, ask].spacing(8).align_y(Alignment::Center)].spacing(3);

        if self.asking {
            let who = match self.config.assistant.mode {
                AssistantMode::Auto => "Haiku (Opus if needed)",
                AssistantMode::Haiku => "Haiku",
                AssistantMode::Opus => "Opus",
            };
            let key_note = if self.assistant.is_none() { " 1Password may ask you to approve reading the API key." } else { "" };
            col = col.push(text(format!("Asking {who}…{key_note}")).size(12).color(MUTED));
        } else if let Some(e) = &self.ask_error {
            col = col.push(text(e).size(12).color(ERROR));
        } else if let Some(s) = &self.suggestion {
            let mut by = s.model.name().to_string();
            if !s.notes.is_empty() {
                by = format!("{by} ({})", s.notes.join("; "));
            }
            let mut line = row![text(format!("{by}: {}", s.explanation)).size(12).color(MUTED).width(Fill)]
                .spacing(8)
                .align_y(Alignment::Center);
            if s.model == Model::Haiku || s.problem.is_some() {
                line = line.push(
                    button(text("Try with Opus").size(12))
                        .style(button::secondary)
                        .on_press_maybe(can_ask.then_some(Message::RetryWithOpus)),
                );
            }
            col = col.push(line);
            if let Some(problem) = &s.problem {
                col = col.push(text(format!("The check found a problem: {problem}")).size(12).color(ERROR));
            } else if !s.confident {
                col = col.push(text("Claude wasn't sure about this one; check the result.").size(12).color(WARNING));
            }
            col = col.push(text("The query is in the editor; review it, then Run.").size(11).color(MUTED));
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

fn results_view(outcome: &QueryOutcome) -> Element<'static, Message> {
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
        let (status, color, body) = match &r.result {
            Ok(v) => ("ok", SUCCESS, cut(serde_json::to_string_pretty(v).unwrap_or_default())),
            Err(e) => ("error", ERROR, e.clone()),
        };
        col = col.push(
            column![
                row![
                    text(format!("Statement {}", i + 1)).size(12).font(bold()),
                    text(format!("{status}{time}")).size(12).color(color),
                ]
                .spacing(8),
                container(text(body).size(12).font(Font::MONOSPACE))
                    .padding(6)
                    .width(Fill)
                    .style(|_| container::background(Color::from_rgb(0.95, 0.95, 0.95))),
            ]
            .spacing(3),
        );
    }
    col.into()
}

fn cut(text: String) -> String {
    let lines = text.lines().count();
    if lines <= MAX_RESULT_LINES {
        return text;
    }
    let kept: Vec<&str> = text.lines().take(MAX_RESULT_LINES).collect();
    format!("{}\n… {} more lines not shown", kept.join("\n"), lines - MAX_RESULT_LINES)
}
