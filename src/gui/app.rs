//! Window state, the sidebar and the record menu.

use iced::widget::{
    Column, button, canvas, column, container, pin, row, rule, scrollable, stack, text,
};
use iced::{Color, Element, Fill, Font, Point, Subscription, Task, Theme, Vector, window};
use surrealdb_explore::db::{self, Database};
use surrealdb_explore::graph::{self, GraphData, MAX_PER_TABLE};

use crate::gui::canvas::{Camera, GraphView};
use crate::gui::layout::Layout;
use crate::gui::model::Graph;

const SIDEBAR_WIDTH: f32 = 320.0;
const MUTED: Color = Color::from_rgb(0.47, 0.47, 0.47);
const WARNING: Color = Color::from_rgb(0.67, 0.4, 0.0);
const ERROR: Color = Color::from_rgb(0.7, 0.0, 0.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Selection {
    Node(usize),
    Edge(usize),
}

#[derive(Debug, Clone)]
pub enum Message {
    /// An animation frame while the layout is moving.
    Tick,
    Hover(Option<usize>),
    Select(Option<Selection>),
    DragNode(usize, Vector),
    DragEnd,
    Pan(Vector),
    /// Zoom by `factor`, keeping graph point `anchor` under the cursor
    /// (`from_centre` is the cursor's offset from the canvas centre).
    Zoom { factor: f32, anchor: Vector, from_centre: Vector },
    OpenMenu(usize, Point),
    CloseMenu,
    MenuInspect,
    MenuHide,
    MenuFocus,
    MenuCopy,
    ShowAll,
    Reload,
}

pub struct App {
    /// Runs the database queries. Embedded SurrealDB needs a bigger stack than
    /// tokio worker threads get by default (2 MB); SurrealDB's docs recommend
    /// 10 MB. With the default, queries run on worker threads spun forever at
    /// 100% CPU here. See https://docs.rs/surrealdb/latest/surrealdb/engine/local/index.html
    runtime: tokio::runtime::Runtime,
    db: Option<Database>,
    graph: Graph,
    layout: Layout,
    camera: Camera,
    hover: Option<usize>,
    selection: Option<Selection>,
    /// Open record menu: the record and where it was right-clicked.
    menu: Option<(usize, Point)>,
    dragging: Option<usize>,
    error: Option<String>,
    notice: Option<String>,
}

impl App {
    pub fn boot() -> App {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(10 * 1024 * 1024)
            .build()
            .expect("failed to start the async runtime");

        let (db, data, error) = match runtime.block_on(db::connect()) {
            Ok(db) => match runtime.block_on(graph::load(&db)) {
                Ok(data) => (Some(db), data, None),
                Err(e) => (Some(db), GraphData::default(), Some(e.to_string())),
            },
            Err(e) => (None, GraphData::default(), Some(format!("Could not open the database: {e}"))),
        };

        App {
            runtime,
            db,
            graph: Graph::new(data, None),
            layout: Layout::new(),
            camera: Camera::default(),
            hover: None,
            selection: None,
            menu: None,
            dragging: None,
            error,
            notice: None,
        }
    }

    pub fn theme(&self) -> Theme {
        Theme::Light
    }

    pub fn subscription(&self) -> Subscription<Message> {
        if self.layout.is_active() || self.dragging.is_some() {
            window::frames().map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => self.layout.step(&mut self.graph, self.dragging),
            Message::Hover(hover) => self.hover = hover,
            Message::Select(selection) => {
                self.selection = selection;
                self.menu = None;
            }
            Message::DragNode(node, pos) => {
                self.dragging = Some(node);
                self.graph.nodes[node].pos = pos;
                self.layout.reheat(0.3);
            }
            Message::DragEnd => self.dragging = None,
            Message::Pan(delta) => self.camera.offset = self.camera.offset + delta,
            Message::Zoom { factor, anchor, from_centre } => {
                let scale = (self.camera.scale * factor).clamp(0.1, 5.0);
                self.camera.offset = from_centre - anchor * scale;
                self.camera.scale = scale;
            }
            Message::OpenMenu(node, at) => self.menu = Some((node, at)),
            Message::CloseMenu => self.menu = None,
            Message::MenuInspect => {
                if let Some((node, _)) = self.menu.take() {
                    self.selection = Some(Selection::Node(node));
                }
            }
            Message::MenuHide => {
                if let Some((node, _)) = self.menu.take() {
                    self.hide(node);
                }
            }
            Message::MenuFocus => {
                if let Some((node, _)) = self.menu.take() {
                    self.selection = Some(Selection::Node(node));
                    self.camera.scale = 1.5;
                    self.camera.offset = self.graph.nodes[node].pos * -1.5;
                }
            }
            Message::MenuCopy => {
                if let Some((node, _)) = self.menu.take() {
                    let id = self.graph.nodes[node].id.clone();
                    self.notice = Some(format!("Copied {id}"));
                    return iced::clipboard::write(id);
                }
            }
            Message::ShowAll => {
                for n in &mut self.graph.nodes {
                    n.hidden = false;
                }
                self.layout.reheat(0.3);
            }
            Message::Reload => self.reload(),
        }
        Task::none()
    }

    fn hide(&mut self, node: usize) {
        self.graph.nodes[node].hidden = true;
        let touches = |s: Selection| match s {
            Selection::Node(n) => n == node,
            Selection::Edge(e) => {
                let e = &self.graph.edges[e];
                e.from == node || e.to == node
            }
        };
        if self.selection.is_some_and(touches) {
            self.selection = None;
        }
        self.hover = None;
        self.layout.reheat(0.2);
    }

    /// Reads the database again; records already on screen keep their place.
    fn reload(&mut self) {
        let Some(db) = &self.db else { return };
        match self.runtime.block_on(graph::load(db)) {
            Ok(data) => {
                self.graph = Graph::new(data, Some(&self.graph));
                self.error = None;
                self.notice = Some("Reloaded".into());
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.hover = None;
        self.selection = None;
        self.menu = None;
        self.dragging = None;
        self.layout.reheat(0.5);
    }

    pub fn view(&self) -> Element<'_, Message> {
        let graph = canvas(GraphView {
            graph: &self.graph,
            camera: self.camera,
            hover: self.hover,
            selection: self.selection,
        })
        .width(Fill)
        .height(Fill);

        let mut layers = stack![graph].width(Fill).height(Fill);
        if let Some((node, at)) = self.menu {
            layers = layers.push(pin(self.record_menu(node)).x(at.x).y(at.y));
        }

        row![layers, rule::vertical(1), self.sidebar()].into()
    }

    fn record_menu(&self, node: usize) -> Element<'_, Message> {
        let n = &self.graph.nodes[node];
        let item = |label: &'static str, message: Message| {
            button(text(label).size(14))
                .width(Fill)
                .style(button::text)
                .on_press(message)
        };
        container(column![
            container(row![text("●").color(n.color), text(&n.id).font(Font::MONOSPACE).size(13)].spacing(8))
                .padding([6, 10]),
            rule::horizontal(1),
            item("Inspect", Message::MenuInspect),
            item("Hide record", Message::MenuHide),
            item("Focus record", Message::MenuFocus),
            item("Copy record id", Message::MenuCopy),
        ])
        .width(210)
        .padding(4)
        .style(|theme| container::bordered_box(theme).background(Color::WHITE))
        .into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let g = &self.graph;
        let mut col = Column::new().spacing(6).padding(16).width(SIDEBAR_WIDTH);

        col = col.push(text("SurrealDB graph").size(18));
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
                text(format!("Only the first {MAX_PER_TABLE} records shown for: {}", g.truncated.join(", ")))
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
                    text(&n.label).size(15).font(Font { weight: iced::font::Weight::Bold, ..Font::default() }),
                    text(&n.id).size(12).font(Font::MONOSPACE),
                    heading("Outgoing"),
                    list(outgoing),
                    heading("Incoming"),
                    list(incoming),
                    heading("Fields"),
                    fields(&n.data),
                ]
                .spacing(6)
                .into()
            }
            Some(Selection::Edge(i)) => {
                let e = &g.edges[i];
                column![
                    text(format!("{} → {} → {}", g.nodes[e.from].label, e.label, g.nodes[e.to].label))
                        .size(15)
                        .font(Font { weight: iced::font::Weight::Bold, ..Font::default() }),
                    text(format!("Relation record {}", e.id)).size(12).color(MUTED),
                    heading("Fields"),
                    fields(&e.data),
                ]
                .spacing(6)
                .into()
            }
        }
    }
}

fn heading(label: &str) -> Element<'_, Message> {
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

fn fields(data: &serde_json::Value) -> Element<'static, Message> {
    let json = serde_json::to_string_pretty(data).unwrap_or_default();
    container(text(json).size(12).font(Font::MONOSPACE))
        .padding(8)
        .width(Fill)
        .style(|_| container::background(Color::from_rgb(0.95, 0.95, 0.95)))
        .into()
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}
