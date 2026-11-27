//! The query panel under the graph: saved queries, a SurrealQL editor and
//! Run, and the last result under them.

use std::collections::HashMap;

use iced::keyboard::{Key, key::Named};
use iced::widget::text_editor::{Binding, KeyPress};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    Column, Row, button, checkbox, column, container, pick_list, row, scrollable, space, text, text_editor, text_input,
    tooltip,
};
use iced::{Alignment, Color, Element, Fill, Font, Length};
use crate::query::QueryOutcome;

use crate::gui::app::{App, Message, plural};
use crate::gui::sidebar::{ERROR, MUTED, SUCCESS, bold};

/// Results longer than this many lines per statement are cut, so a huge
/// result can't make the window sluggish.
const MAX_RESULT_LINES: usize = 400;
/// Most rows drawn in a result table.
const MAX_TABLE_ROWS: usize = 300;

/// The open query panel's height to begin with, and its limits when resized.
pub(super) const PANEL_HEIGHT: f32 = 300.0;
pub(super) const MIN_PANEL_HEIGHT: f32 = 160.0;
/// The graph above the panel keeps at least this height.
pub(super) const MIN_GRAPH_HEIGHT: f32 = 160.0;
/// The panel's rows other than the editor and the result: the toggle, saved
/// queries, the Run row, spacing and padding.
const PANEL_CHROME: f32 = 120.0;
/// Room left for the result when the panel grows to fit the query.
const MIN_RESULT_HEIGHT: f32 = 110.0;
/// One line of the editor at text size 13, and its padding.
const EDITOR_LINE: f32 = 17.0;
const EDITOR_PADDING: f32 = 12.0;
/// One character of the editor's monospace font at size 13.
const EDITOR_CHAR_WIDTH: f32 = 7.9;
/// The editor shows at least this many lines, and grows up to the most.
const EDITOR_MIN_LINES: usize = 3;
const EDITOR_MAX_LINES: usize = 24;

impl App {
    pub(super) fn query_panel(&self) -> Element<'_, Message> {
        let toggle = button(text(if self.query_open { "▾ Query" } else { "▸ Query" }).size(13))
            .style(button::text)
            .on_press(Message::ToggleQuery);
        if !self.query_open {
            return container(toggle).padding([2, 8]).width(Fill).into();
        }
        // The editor fits the query, but leaves room for the result.
        let editor_height = self
            .editor_height()
            .min(self.panel_height - PANEL_CHROME - MIN_RESULT_HEIGHT)
            .max(editor_height_for(EDITOR_MIN_LINES));

        let editor = text_editor(&self.query_text)
            .placeholder("SurrealQL, e.g. SELECT * FROM person WHERE name = 'Tom Hanks';")
            .on_action(Message::QueryEdited)
            .key_binding(run_on_cmd_enter)
            .font(Font::MONOSPACE)
            .size(13)
            .height(editor_height);

        let run = button(text(if self.query_running { "Running…" } else { "Run  ⌘↩" }).size(13))
            .style(button::primary)
            .on_press_maybe((!self.query_running && self.db.is_some()).then_some(Message::RunQuery));
        let show = button(text("Show in graph").size(13))
            .style(button::secondary)
            .on_press_maybe(self.query_drawable.then_some(Message::ShowQueryInGraph));
        let clear = button(text("Clear").size(13))
            .style(button::secondary)
            .on_press_maybe((!self.query_text.text().trim().is_empty()).then_some(Message::ClearQuery));
        let outcome = self.query_outcome.as_ref();
        let copy_json = button(text("Copy JSON").size(13))
            .style(button::secondary)
            .on_press_maybe(outcome.is_some().then_some(Message::CopyQueryResult(CopyFormat::Json)));
        let copy_table = tooltip(
            button(text("Copy table").size(13))
                .style(button::secondary)
                .on_press_maybe(outcome.is_some_and(has_table).then_some(Message::CopyQueryResult(CopyFormat::Table))),
            container(text("Tab-separated, pastes into a spreadsheet").size(12)).padding(6).style(container::rounded_box),
            tooltip::Position::Bottom,
        );
        let mut status = row![
            run,
            show,
            clear,
            copy_json,
            copy_table,
            checkbox(self.config.graph.connect_results)
                .label("Connect shown records")
                .on_toggle(Message::ToggleConnectResults)
                .text_size(12),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(summary) = outcome.map(summary) {
            status = status.push(summary);
        }
        // Right after a copy, next to the button, so it isn't missed.
        if let Some(copied) = &self.query_copied {
            status = status.push(text(format!("· {copied}")).size(12).color(SUCCESS));
        }
        // On a narrow window the summary moves to a second line rather than
        // being cut off.
        let status = status.wrap().vertical_spacing(4);
        column![container(toggle).padding([2, 0]), self.saved_queries_row(), editor, status, self.result_view()]
            .spacing(6)
            .padding([4, 8])
            .height(self.panel_height)
            .into()
    }

    /// The editor's height for the query in it, counting a line that wraps
    /// as the lines it takes.
    fn editor_height(&self) -> f32 {
        // The editor is as wide as the graph, less the panel's padding.
        let columns = (((self.canvas_size.width - 32.0) / EDITOR_CHAR_WIDTH) as usize).max(20);
        let lines: usize = self.query_text.text().lines().map(|l| l.chars().count().div_ceil(columns).max(1)).sum();
        editor_height_for(lines.clamp(EDITOR_MIN_LINES, EDITOR_MAX_LINES))
    }

    /// Grows the panel so the whole query fits in the editor (up to
    /// `EDITOR_MAX_LINES`), as long as the graph keeps `MIN_GRAPH_HEIGHT`.
    /// It never shrinks the panel: that is left to the user's drag.
    pub(super) fn fit_panel(&mut self) {
        if !self.query_open {
            return;
        }
        let wanted = PANEL_CHROME + self.editor_height() + MIN_RESULT_HEIGHT;
        let room = self.panel_height + self.canvas_size.height - MIN_GRAPH_HEIGHT;
        self.panel_height = self.panel_height.max(wanted.min(room)).max(MIN_PANEL_HEIGHT);
    }

    /// Saved queries: pick one to put it in the editor, delete it, or save
    /// the editor's query under a name.
    fn saved_queries_row(&self) -> Element<'_, Message> {
        let names = self.saved_queries.names();
        let picked = self.saved_query.clone().filter(|n| names.contains(n));
        let placeholder = if names.is_empty() { "No saved queries" } else { "Choose a saved query…" };
        let picker = pick_list(names, picked.clone(), Message::PickSavedQuery)
            .placeholder(placeholder)
            .style(saved_queries_style)
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

    /// The last result, under the Run row: each statement's result as path
    /// rows when it is a list of paths, as a table when it is rows of
    /// fields, otherwise as JSON.
    fn result_view(&self) -> Element<'_, Message> {
        let Some(outcome) = &self.query_outcome else {
            return container(text("Run a query to see its result here.").size(13).color(MUTED))
                .padding(8)
                .height(Fill)
                .into();
        };
        let results = match &outcome.statements {
            // Why the query didn't run, such as a refused write.
            Err(e) => return scrollable(text(e.clone()).size(12).color(ERROR)).height(Fill).width(Fill).into(),
            Ok(results) => results,
        };
        let mut col = Column::new().spacing(10).padding([4, 0]).width(Fill);
        // Such as: a write run as the read-only viewer changed nothing.
        if let Some(note) = &outcome.note {
            col = col.push(text(note.clone()).size(12).color(ERROR));
        }
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
            let rows = r.result.as_ref().ok().and_then(table_rows);
            if let Some(rows) = rows {
                heading = heading.push(text(plural(rows.len(), "row")).size(12).color(MUTED));
            }
            let body: Element<'_, Message> = match (&r.result, paths, rows) {
                (Ok(_), Some(paths), _) => self.path_rows(paths),
                (Ok(_), None, Some(rows)) => result_table(rows, &self.query_ran),
                (Ok(v), None, None) => grey_box(cut(serde_json::to_string_pretty(v).unwrap_or_default())),
                (Err(e), _, _) => grey_box(e.clone()),
            };
            col = col.push(column![heading, body].spacing(4));
        }
        scrollable(col)
            .direction(Direction::Both { vertical: Scrollbar::default(), horizontal: Scrollbar::default() })
            .height(Fill)
            .width(Fill)
            .into()
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

/// How the Copy buttons put the last result on the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyFormat {
    /// The whole result as pretty JSON: one statement's value, or an array
    /// with one entry per statement (`{"error": …}` for a failed one, or
    /// for the whole query when it didn't run).
    Json,
    /// The tables (rows of fields) as tab-separated text with a header row,
    /// a blank line between statements, ready to paste into a spreadsheet.
    Table,
}

impl CopyFormat {
    /// The text to copy and a few words on what it is, or `None` when there
    /// is nothing to copy in this format.
    pub(super) fn render(self, outcome: &QueryOutcome, query: &str) -> Option<(String, String)> {
        let results = match (&outcome.statements, self) {
            // The query didn't run (such as a refused write): its error.
            (Err(e), CopyFormat::Json) => {
                let json = serde_json::to_string_pretty(&serde_json::json!({ "error": e })).ok()?;
                return Some((json, "the error as JSON".to_string()));
            }
            (Err(_), CopyFormat::Table) => return None,
            (Ok(results), _) => results,
        };
        match self {
            CopyFormat::Json => {
                let value = |r: &crate::query::StatementResult| match &r.result {
                    Ok(v) => v.clone(),
                    Err(e) => serde_json::json!({ "error": e }),
                };
                let json = match results.as_slice() {
                    [one] => value(one),
                    many => serde_json::Value::Array(many.iter().map(value).collect()),
                };
                let what = match results.as_slice() {
                    [one] => one.result.as_ref().ok().and_then(|v| v.as_array()).map_or_else(
                        || "the result as JSON".to_string(),
                        |rows| format!("{} as JSON", plural(rows.len(), "row")),
                    ),
                    many => format!("{} statements' results as JSON", many.len()),
                };
                Some((serde_json::to_string_pretty(&json).ok()?, what))
            }
            CopyFormat::Table => {
                let tables: Vec<&Vec<serde_json::Value>> =
                    results.iter().filter_map(|r| r.result.as_ref().ok().and_then(table_rows)).collect();
                if tables.is_empty() {
                    return None;
                }
                let rows: usize = tables.iter().map(|t| t.len()).sum();
                let text = tables.into_iter().map(|t| tsv(t, query)).collect::<Vec<_>>().join("\n");
                Some((text, format!("{} as a table", plural(rows, "row"))))
            }
        }
    }
}

/// Whether the result has a table to copy.
fn has_table(outcome: &QueryOutcome) -> bool {
    outcome
        .statements
        .as_ref()
        .is_ok_and(|rs| rs.iter().any(|r| r.result.as_ref().ok().and_then(table_rows).is_some()))
}

/// Rows of fields as tab-separated text, with a header row and the columns
/// in the order the table shows them. Every row is copied (the table only
/// draws the first `MAX_TABLE_ROWS`), values in full.
fn tsv(rows: &[serde_json::Value], query: &str) -> String {
    let columns = table_columns(rows, query);
    let mut out = columns.iter().map(|&c| clean(c)).collect::<Vec<_>>().join("\t");
    out.push('\n');
    for r in rows {
        let line: Vec<String> = columns
            .iter()
            .map(|c| match r.get(*c) {
                None | Some(serde_json::Value::Null) => String::new(),
                Some(serde_json::Value::String(s)) => clean(s),
                Some(v) => clean(&v.to_string()),
            })
            .collect();
        out.push_str(&line.join("\t"));
        out.push('\n');
    }
    out
}

/// A value on one line with no tabs, so it stays in its cell. A value with
/// a quote mark is quoted (`"` doubled inside), as spreadsheets expect:
/// unquoted, one that starts with `"` would be read as the start of a
/// quoted value and swallow the cells after it.
fn clean(s: &str) -> String {
    let flat = s.replace(['\t', '\n', '\r'], " ");
    if flat.contains('"') { format!("\"{}\"", flat.replace('"', "\"\"")) } else { flat }
}

/// A table's columns: every field of every row, ordered like the query.
fn table_columns<'a>(rows: &'a [serde_json::Value], query: &str) -> Vec<&'a str> {
    let mut columns: Vec<&str> = Vec::new();
    for r in rows {
        for key in r.as_object().into_iter().flat_map(|o| o.keys()) {
            if !columns.contains(&key.as_str()) {
                columns.push(key);
            }
        }
    }
    order_like_query(&mut columns, query);
    columns
}

/// The saved queries list looks like a control to use, even before a query
/// is picked: white like the editor, with dark placeholder text and an
/// accent border and arrow (the default is grey, as if it were disabled).
fn saved_queries_style(theme: &iced::Theme, status: pick_list::Status) -> pick_list::Style {
    let palette = theme.extended_palette();
    let default = pick_list::default(theme, status);
    let hovered = !matches!(status, pick_list::Status::Active);
    pick_list::Style {
        background: Color::WHITE.into(),
        placeholder_color: palette.background.base.text,
        handle_color: palette.primary.base.color,
        border: iced::Border {
            color: if hovered { palette.primary.strong.color } else { palette.primary.base.color },
            width: 1.0,
            radius: 4.0.into(),
        },
        ..default
    }
}

fn editor_height_for(lines: usize) -> f32 {
    lines as f32 * EDITOR_LINE + EDITOR_PADDING
}

/// A result that is a list of objects (rows of fields), such as most SELECTs.
fn table_rows(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    value.as_array().filter(|rows| !rows.is_empty() && rows.iter().all(|r| r.is_object()))
}

/// Rows of fields as a table, a column per field. Long values are
/// shortened; hovering one shows it whole.
fn result_table(rows: &[serde_json::Value], query: &str) -> Element<'static, Message> {
    let columns = table_columns(rows, query);
    let shown = &rows[..rows.len().min(MAX_TABLE_ROWS)];
    let cells: Vec<Vec<(String, String)>> = shown
        .iter()
        .map(|r| columns.iter().map(|c| cell(r.get(*c))).collect())
        .collect();
    // Each column is as wide as its widest value (monospace, so by characters).
    let widths: Vec<f32> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let chars = cells.iter().map(|r| r[i].0.chars().count()).chain([c.chars().count()]).max().unwrap_or(1);
            chars as f32 * 7.3 + 16.0
        })
        .collect();

    let mut table = Column::new().width(Length::Shrink);
    let header = columns.iter().zip(&widths).fold(Row::new(), |row, (c, w)| {
        row.push(container(text(c.to_string()).size(12).font(bold())).width(*w).padding([3, 8]))
    });
    table = table.push(container(header).style(|_| container::background(Color::from_rgb(0.92, 0.92, 0.92))));
    for (n, r) in cells.into_iter().enumerate() {
        let line = r.into_iter().zip(&widths).fold(Row::new(), |row, ((short, full), w)| {
            let value = container(text(short.clone()).size(12).font(Font::MONOSPACE)).width(*w).padding([3, 8]);
            if short == full {
                row.push(value)
            } else {
                let whole = container(text(full).size(12).font(Font::MONOSPACE))
                    .padding(6)
                    .style(container::rounded_box);
                row.push(tooltip(value, whole, tooltip::Position::Bottom))
            }
        });
        let shade = if n % 2 == 1 { Color::from_rgb(0.97, 0.97, 0.97) } else { Color::WHITE };
        table = table.push(container(line).style(move |_| container::background(shade)));
    }
    if rows.len() > MAX_TABLE_ROWS {
        table = table.push(text(format!("… and {} more rows", rows.len() - MAX_TABLE_ROWS)).size(12).color(MUTED));
    }
    table.into()
}

/// SurrealDB returns a row's fields sorted by name, so `SELECT offset, …
/// AS buyer` would show `buyer` first. Puts the columns in the order their
/// names first appear in the query instead; names it doesn't mention (as
/// with `SELECT *`) keep their order, after the others.
fn order_like_query(columns: &mut [&str], query: &str) {
    columns.sort_by_key(|c| first_word(query, c).unwrap_or(usize::MAX));
}

/// Where `word` first appears in `text` as a whole word.
fn first_word(text: &str, word: &str) -> Option<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).map(|(i, _)| i).find(|&i| {
        !text[..i].chars().next_back().is_some_and(is_word) && !text[i + word.len()..].chars().next().is_some_and(is_word)
    })
}

/// A field's value as shown in the table, and in full.
fn cell(value: Option<&serde_json::Value>) -> (String, String) {
    const MAX_CHARS: usize = 48;
    let full = match value {
        None | Some(serde_json::Value::Null) => return ("—".into(), "—".into()),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    };
    let short = if let Some(party) = short_party_id(&full) {
        party
    } else if full.chars().count() > MAX_CHARS {
        format!("{}…", full.chars().take(MAX_CHARS - 1).collect::<String>())
    } else {
        full.clone()
    };
    (short, full)
}

/// A Canton party id (`Alice::1220e604…`, a name and a long hex
/// fingerprint) with its fingerprint cut to six characters.
fn short_party_id(s: &str) -> Option<String> {
    let (name, fingerprint) = s.split_once("::")?;
    let hex = fingerprint.len() > 12 && fingerprint.chars().all(|c| c.is_ascii_hexdigit());
    (hex && !name.is_empty()).then(|| format!("{name}::{}…", &fingerprint[..6]))
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

#[cfg(test)]
mod tests {
    use super::{CopyFormat, cell, order_like_query, short_party_id};
    use crate::query::{QueryOutcome, StatementResult};
    use serde_json::json;
    use std::time::Duration;

    fn outcome(results: Vec<Result<serde_json::Value, String>>) -> QueryOutcome {
        QueryOutcome {
            statements: Ok(results.into_iter().map(|result| StatementResult { time: None, result }).collect()),
            elapsed: Duration::ZERO,
            note: None,
        }
    }

    #[test]
    fn copies_one_statement_as_its_json() {
        let o = outcome(vec![Ok(json!([{ "name": "Tom" }, { "name": "Meg" }]))]);
        let (copied, what) = CopyFormat::Json.render(&o, "SELECT name FROM person").unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&copied).unwrap(), json!([{ "name": "Tom" }, { "name": "Meg" }]));
        assert_eq!(what, "2 rows as JSON");
    }

    #[test]
    fn copies_several_statements_as_an_array_with_errors() {
        let o = outcome(vec![Ok(json!(1)), Err("bad".into())]);
        let (copied, _) = CopyFormat::Json.render(&o, "").unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&copied).unwrap(), json!([1, { "error": "bad" }]));
    }

    #[test]
    fn copies_tables_as_tab_separated_text() {
        let party = "Alice::1220e60453756687070256c5ab36";
        let o = outcome(vec![
            Ok(json!([{ "buyer": party, "offset": 7, "note": "a\tb" }, { "offset": 8, "note": null }])),
            Ok(json!(3)),
        ]);
        let (copied, what) = CopyFormat::Table.render(&o, "SELECT offset, buyer, note FROM t").unwrap();
        assert_eq!(copied, format!("offset\tbuyer\tnote\n7\t{party}\ta b\n8\t\t\n"));
        assert_eq!(what, "2 rows as a table");
        assert!(CopyFormat::Table.render(&outcome(vec![Ok(json!(3))]), "").is_none());
    }

    #[test]
    fn quotes_table_values_with_quote_marks() {
        let o = outcome(vec![Ok(json!([{ "parties": ["Bob::1220aa"], "say": "\"hi\" there" }]))]);
        let (copied, _) = CopyFormat::Table.render(&o, "SELECT parties, say FROM t").unwrap();
        assert_eq!(copied, "parties\tsay\n\"[\"\"Bob::1220aa\"\"]\"\t\"\"\"hi\"\" there\"\n");
    }

    #[test]
    fn copies_a_query_that_did_not_run_as_its_error() {
        let o = QueryOutcome::refused("writes are refused");
        let (copied, what) = CopyFormat::Json.render(&o, "").unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&copied).unwrap(), json!({ "error": "writes are refused" }));
        assert_eq!(what, "the error as JSON");
        assert!(CopyFormat::Table.render(&o, "").is_none());
    }

    #[test]
    fn columns_follow_the_query() {
        let query = "SELECT offset, array::first(->x.owner) AS buyer, array::first(->y.price) AS price FROM exercised";
        let mut columns = vec!["buyer", "offset", "price"];
        order_like_query(&mut columns, query);
        assert_eq!(columns, ["offset", "buyer", "price"]);
        let mut unknown = vec!["id", "name", "age"];
        order_like_query(&mut unknown, "SELECT * FROM person");
        assert_eq!(unknown, ["id", "name", "age"]);
        let mut partial = vec!["id", "offsets", "offset"];
        order_like_query(&mut partial, "SELECT offset, offsets FROM t");
        assert_eq!(partial, ["offset", "offsets", "id"]);
    }

    #[test]
    fn party_ids_are_shortened_and_other_values_kept() {
        assert_eq!(short_party_id("Alice::1220e60453756687070256c5ab36").as_deref(), Some("Alice::1220e6…"));
        assert_eq!(short_party_id("a::b"), None);
        assert_eq!(short_party_id("not a party"), None);
        let id = "Alice::1220e60453756687070256c5ab36";
        assert_eq!(cell(Some(&json!(id))), ("Alice::1220e6…".to_string(), id.to_string()));
        assert_eq!(cell(Some(&json!(25))), ("25".to_string(), "25".to_string()));
        assert_eq!(cell(Some(&json!("10"))).0, "10");
        assert_eq!(cell(None).0, "—");
        let long = "x".repeat(60);
        assert_eq!(cell(Some(&json!(long))).0.chars().count(), 48);
    }
}
