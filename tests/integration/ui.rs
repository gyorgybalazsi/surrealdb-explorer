//! Tests of the window, with `iced_test`: the app is booted against a test
//! database, widgets are found and clicked by their text with a
//! `Simulator`, and the tasks the app starts are run to the end.
//!
//! The graph is a canvas, so its records have no widget text: tests pick
//! them with messages and read what the canvas would draw from `App`.

use std::collections::VecDeque;

use iced_test::runtime::{Action, task};
use iced_test::simulator;
use tempfile::TempDir;

use surrealdb_explorer::canton::NodeKind;
use surrealdb_explorer::config::{Config, DatabaseConfig, Visualization};
use surrealdb_explorer::gui::{App, CantonFilterChange, Files, Message, Selection};
use surrealdb_explorer::saved_queries::{SavedQueries, SavedQuery};
use surrealdb_explorer::{data, db, query};

use crate::common;

/// The app, booted against a test database, with its settings and saved
/// queries in a temporary folder. MCP and auto refresh are off.
struct Ui {
    app: App,
    _files: TempDir,
}

impl Ui {
    fn boot(database: DatabaseConfig, visualization: Visualization) -> Ui {
        Ui::boot_with_queries(database, visualization, None)
    }

    /// Boots with these saved queries (`None`: the defaults the app writes).
    fn boot_with_queries(database: DatabaseConfig, visualization: Visualization, queries: Option<SavedQueries>) -> Ui {
        let dir = common::temp_dir("ui");
        let files = Files { config: dir.path().join("config.toml"), queries: dir.path().join("queries.toml") };
        if let Some(queries) = queries {
            queries.save_to(&files.queries).unwrap();
        }
        let mut config = Config { database, ..Config::default() };
        config.mcp.enabled = false;
        config.graph.auto_refresh = false;
        config.graph.visualization = visualization;
        let app = App::boot(config, files, None, None);
        Ui { app, _files: dir }
    }

    /// Sends `message` to the app, then every message its tasks produce,
    /// until no task is left.
    fn send(&mut self, message: Message) {
        let mut queue = VecDeque::from([message]);
        while let Some(message) = queue.pop_front() {
            let Some(stream) = task::into_stream(self.app.update(message)) else { continue };
            for action in iced::futures::executor::block_on_stream(stream) {
                if let Action::Output(message) = action {
                    queue.push_back(message);
                }
            }
        }
    }

    /// Clicks the widget showing `label`, as the user would, and handles
    /// the messages that produces.
    fn click(&mut self, label: &str) {
        let mut ui = simulator(self.app.view());
        if let Err(e) = ui.click(label) {
            panic!("no \"{label}\" to click: {e}");
        }
        let messages: Vec<Message> = ui.into_messages().collect();
        for message in messages {
            self.send(message);
        }
    }

    /// Whether the window shows `text` somewhere (exactly, in one widget).
    fn shows(&self, text: &str) -> bool {
        simulator(self.app.view()).find(text).is_ok()
    }

    /// The id of the drawn record whose id contains `part`.
    fn record(&self, part: &str) -> String {
        let shown = self.app.shown_records();
        let found: Vec<&String> = shown.iter().filter(|id| id.contains(part)).collect();
        match found.as_slice() {
            [id] => (*id).clone(),
            _ => panic!("expected one shown record matching {part:?}, got {found:?} of {shown:?}"),
        }
    }

    fn is_shown(&self, part: &str) -> bool {
        self.app.shown_records().iter().any(|id| id.contains(part))
    }
}

/// A database with the Canton sample, then `extra` run on it.
fn canton_database(name: &str, extra: &'static str) -> DatabaseConfig {
    let config = common::temp_database(name);
    let server = config.clone();
    common::run(async move {
        let db = db::connect(&server).await.unwrap();
        data::load_canton_sample(&db).await.unwrap();
        if !extra.is_empty() {
            db.query(extra).await.unwrap().check().unwrap();
        }
    });
    config
}

#[test]
fn other_tables_show_only_when_there_are_some_and_can_be_hidden() {
    let ui = Ui::boot(canton_database("ui_no_other", ""), Visualization::Canton);
    assert!(ui.is_shown("transaction"), "the sample's transactions are drawn: {:?}", ui.app.shown_records());
    assert!(!ui.shows("Other tables"), "no other tables, so no \"Other tables\" filter");

    let mut ui = Ui::boot(canton_database("ui_other", "CREATE note:1 SET text = 'not Canton';"), Visualization::Canton);
    assert!(ui.shows("Other tables"));
    assert!(ui.is_shown("note:1"), "records of other tables are drawn: {:?}", ui.app.shown_records());

    ui.send(Message::CantonFilter(CantonFilterChange::Kind(NodeKind::Other, false)));
    assert!(!ui.is_shown("note:1"), "unticked, other tables are hidden");
    assert!(ui.is_shown("transaction"), "the Canton records stay");

    ui.send(Message::CantonFilter(CantonFilterChange::Kind(NodeKind::Other, true)));
    assert!(ui.is_shown("note:1"));
}

/// Who sees each contract, as the syncer writes it. The cash Bob paid with
/// (`00c3c7d4`) is signed by the bank alone, but the venue sees it.
const WITNESSES: &str = "
    UPDATE created:⟨00c1a5b2⟩ SET witness_parties = ['Bank::1220f3a9c1d2', 'Bob::12209c5d1e8a'];
    UPDATE created:⟨00c2b6c3⟩ SET witness_parties = ['Venue::1220b7e4a0f5', 'Bob::12209c5d1e8a'];
    UPDATE created:⟨00c3c7d4⟩ SET witness_parties = ['Bank::1220f3a9c1d2', 'Venue::1220b7e4a0f5'];
    UPDATE created:⟨00c4d8e5⟩ SET witness_parties = ['Venue::1220b7e4a0f5', 'Bob::12209c5d1e8a'];
    UPDATE created:⟨00c5e9f6⟩ SET witness_parties = ['Venue::1220b7e4a0f5', 'Carol::1220d2a6b3c7'];
";

#[test]
fn party_filter_keeps_the_contracts_the_party_witnesses() {
    let mut ui = Ui::boot(canton_database("ui_party", WITNESSES), Visualization::Canton);
    let all = ui.app.shown_records();
    for contract in ["00c1a5b2", "00c2b6c3", "00c3c7d4", "00c4d8e5", "00c5e9f6"] {
        assert!(ui.is_shown(contract), "{contract} is drawn before filtering: {all:?}");
    }

    let party = |p: &str| Message::CantonFilter(CantonFilterChange::Party(Some(p.to_string())));

    ui.send(party("Venue::1220b7e4a0f5"));
    for contract in ["00c2b6c3", "00c3c7d4", "00c4d8e5", "00c5e9f6"] {
        assert!(ui.is_shown(contract), "the venue witnesses {contract}: {:?}", ui.app.shown_records());
    }
    assert!(!ui.is_shown("00c1a5b2"), "the venue doesn't see the bank's first cash");

    ui.send(party("Carol::1220d2a6b3c7"));
    assert!(ui.is_shown("00c5e9f6"));
    for contract in ["00c1a5b2", "00c2b6c3", "00c3c7d4", "00c4d8e5"] {
        assert!(!ui.is_shown(contract), "Carol doesn't see {contract}");
    }

    ui.send(Message::CantonFilter(CantonFilterChange::Party(None)));
    assert_eq!(ui.app.shown_records(), all, "any party shows everything again");
}

#[test]
fn picking_a_saved_query_replaces_the_editor_text_and_clear_empties_it() {
    let saved = SavedQueries {
        queries: vec![
            SavedQuery { name: "People".into(), query: "SELECT * FROM person;".into() },
            SavedQuery { name: "Films".into(), query: "SELECT title FROM movie;".into() },
        ],
    };
    let mut ui = Ui::boot_with_queries(common::temp_database("ui_saved"), Visualization::Standard, Some(saved));
    ui.click("▸ Query");

    ui.send(Message::PickSavedQuery("Films".into()));
    assert_eq!(ui.app.query_text(), "SELECT title FROM movie;");
    ui.send(Message::PickSavedQuery("People".into()));
    assert_eq!(ui.app.query_text(), "SELECT * FROM person;", "nothing of the last query is left");

    ui.click("Clear");
    assert_eq!(ui.app.query_text().trim(), "");
}

#[test]
fn a_write_from_the_query_panel_is_refused() {
    let config = common::temp_database("ui_write");
    let mut ui = Ui::boot(config.clone(), Visualization::Standard);
    ui.click("▸ Query");
    ui.click("Clear");
    ui.send(Message::QueryEdited(iced::widget::text_editor::Action::Edit(iced::widget::text_editor::Edit::Paste(
        std::sync::Arc::new("CREATE person:mallory SET name = 'Mallory';".into()),
    ))));
    ui.click("Run  ⌘↩");

    assert!(ui.shows(query::WRITE_REFUSED_HERE), "the panel says why the query didn't run");
    let tables = common::run(async move {
        let db = db::connect(&config).await.unwrap();
        let mut response = db.query("RETURN object::keys((INFO FOR DB).tables)").await.unwrap();
        response.take::<Vec<String>>(0).unwrap()
    });
    assert!(tables.is_empty(), "nothing was written, not even a table: {tables:?}");
}

/// Contracts the sample archives: their cards are always faded.
const ARCHIVED: [&str; 3] = ["00c1a5b2", "00c2b6c3", "00c4d8e5"];

fn normal_alpha(id: &str) -> f32 {
    if ARCHIVED.iter().any(|a| id.contains(a)) { 0.5 } else { 1.0 }
}

#[test]
fn selecting_a_record_dims_the_rest_and_deselecting_restores_the_view() {
    let mut ui = Ui::boot(canton_database("ui_select", ""), Visualization::Canton);
    for id in ui.app.shown_records() {
        assert_eq!(ui.app.card_alpha(&id), Some(normal_alpha(&id)), "{id} before selecting");
    }

    // Bob's cash transfer: its exercise, the cash it spends and the cash it creates.
    let transfer = ui.record("3, 1");
    let index = ui.app.record_index(&transfer).unwrap();
    ui.send(Message::Select(Some(Selection::Node(index))));

    assert_eq!(ui.app.card_alpha(&transfer), Some(1.0));
    assert_eq!(ui.app.card_alpha(&ui.record("00c3c7d4")), Some(1.0), "the cash it creates is lit");
    assert_eq!(ui.app.card_alpha(&ui.record("00c1a5b2")), Some(0.5), "the cash it spends is lit, but archived");
    assert_eq!(ui.app.card_alpha(&ui.record("00c5e9f6")), Some(0.35), "an unrelated ticket is dimmed");

    ui.send(Message::Select(None));
    for id in ui.app.shown_records() {
        assert_eq!(ui.app.card_alpha(&id), Some(normal_alpha(&id)), "{id} after deselecting");
    }
}
