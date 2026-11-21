//! Drawing the graph and turning mouse input into messages.

use std::collections::{HashMap, HashSet};

use iced::alignment;
use iced::mouse::{self, Button, Cursor, ScrollDelta};
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Font, Point, Rectangle, Renderer, Theme, Vector};
use crate::config::Visualization;

use crate::gui::app::{Message, Selection};
use crate::gui::canton_draw;
use crate::gui::model::Graph;

/// Radius of a record with no relations; better-connected records are bigger.
const NODE_RADIUS: f32 = 8.0;
const ARROW_LENGTH: f32 = 9.0;
const ARROW_WIDTH: f32 = 3.6;
/// How far apart (on screen) relations between the same two records bend.
const BEND: f32 = 26.0;
/// How the mouse must move before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 4.0;

/// The Canton view's room around its layout, in pixels: left and right, top and bottom.
pub const CANTON_MARGIN: (f32, f32) = (20.0, 10.0);
/// The Canton view's scrollbar, along the canvas's right edge.
pub const SCROLLBAR_WIDTH: f32 = 12.0;
const SCROLLBAR_TRACK: Color = rgb(0xe5, 0xe7, 0xeb);
const SCROLLBAR_THUMB: Color = rgb(0x9c, 0xa3, 0xaf);

const BACKGROUND: Color = rgb(0xf7, 0xf8, 0xfa);
const GRID_DOT: Color = rgb(0xdd, 0xe1, 0xe7);
const EDGE_COLOR: Color = rgb(0xa8, 0xb1, 0xbe);
const EDGE_LIT: Color = rgb(0x4b, 0x55, 0x63);
const EDGE_LABEL: Color = rgb(0x4b, 0x55, 0x63);
const NODE_LABEL: Color = rgb(0x1f, 0x29, 0x37);
const PILL: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.88 };
const PILL_LINE: Color = rgb(0xe3, 0xe6, 0xeb);
const SHADOW: Color = Color { r: 0.06, g: 0.09, b: 0.16, a: 0.16 };
const SELECTED_RING: Color = rgb(0x11, 0x18, 0x27);

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

/// Maps graph space to the canvas: `screen = centre + offset + pos * scale`.
#[derive(Clone, Copy)]
pub struct Camera {
    pub offset: Vector,
    pub scale: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { offset: Vector::ZERO, scale: 1.0 }
    }
}

impl Camera {
    pub fn to_screen(&self, bounds: Rectangle, pos: Vector) -> Point {
        Point::new(bounds.width / 2.0, bounds.height / 2.0) + self.offset + pos * self.scale
    }

    pub fn to_graph(&self, bounds: Rectangle, point: Point) -> Vector {
        (point - Point::new(bounds.width / 2.0, bounds.height / 2.0) - self.offset) * (1.0 / self.scale)
    }
}

/// The canvas program: reads the app's graph, camera and highlight state.
pub struct GraphView<'a> {
    pub graph: &'a Graph,
    pub camera: Camera,
    pub hover: Option<usize>,
    pub selection: Option<Selection>,
    pub visualization: Visualization,
    /// The canvas size the app last heard of, so a change can be reported.
    pub known_size: iced::Size,
    /// In the Canton view: how far it is scrolled down and how tall its
    /// content is, in pixels.
    pub scroll: Option<(f32, f32)>,
}

/// Mouse state kept by the canvas between events.
#[derive(Default)]
pub enum Interaction {
    #[default]
    Idle,
    /// Left button down on a record; becomes a drag once the mouse moves.
    PressedNode { node: usize, start: Point, dragging: bool },
    /// Left button down on the background; drags pan the view.
    Panning { start: Point, last: Point },
    /// Dragging the scrollbar's thumb, held `grab` pixels below its top.
    Scrolling { grab: f32 },
}

impl GraphView<'_> {
    /// The scrollbar's thumb, when the Canton view is taller than the canvas.
    fn thumb(&self, bounds: Rectangle) -> Option<Rectangle> {
        let (scroll, content) = self.scroll?;
        let h = bounds.height;
        if content <= h + 0.5 {
            return None;
        }
        let height = (h * h / content).max(30.0).min(h);
        let top = scroll / (content - h) * (h - height);
        Some(Rectangle { x: bounds.width - SCROLLBAR_WIDTH, y: top, width: SCROLLBAR_WIDTH, height })
    }

    /// The scroll position that puts the thumb's top at `top` pixels.
    fn scroll_for(&self, bounds: Rectangle, thumb: Rectangle, top: f32) -> f32 {
        let content = self.scroll.map_or(0.0, |(_, c)| c);
        let room = (bounds.height - thumb.height).max(1.0);
        top.clamp(0.0, room) / room * (content - bounds.height)
    }

    /// The visible record under `point`, topmost first.
    fn node_at(&self, bounds: Rectangle, point: Point) -> Option<usize> {
        if self.visualization == Visualization::Canton {
            return (0..self.graph.nodes.len())
                .rev()
                .find(|&i| self.graph.nodes[i].card.shown && canton_draw::on_card(self, bounds, i, point));
        }
        let radii = radii(self.graph, self.camera.scale);
        self.graph
            .nodes
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, n)| !n.hidden)
            .find(|(i, n)| self.camera.to_screen(bounds, n.pos).distance(point) <= radii[*i] + 3.0)
            .map(|(i, _)| i)
    }

    /// The visible relation passing close to `point`.
    fn edge_at(&self, bounds: Rectangle, point: Point) -> Option<usize> {
        if self.visualization == Visualization::Standard {
            let radii = radii(self.graph, self.camera.scale);
            let bends = bends(self.graph);
            return (0..self.graph.edges.len())
                .filter(|&i| self.graph.edge_visible(&self.graph.edges[i]))
                .find(|&i| {
                    curve(self, bounds, i, &radii, bends[i]).is_some_and(|c| {
                        c.points().windows(2).any(|w| distance_to_segment(point, w[0], w[1]) <= 5.0)
                    })
                });
        }
        canton_draw::edge_at(self, bounds, point)
    }

    /// Records (and their relations) to draw in full colour while hovering.
    /// In the Canton view a selected record lights up its neighbours too, as in alt-canton's explorer.
    fn lit(&self) -> Option<HashSet<usize>> {
        let selected = match self.selection {
            Some(Selection::Node(n)) if self.visualization == Visualization::Canton => Some(n),
            _ => None,
        };
        self.hover.or(selected).map(|h| self.graph.neighbourhood(h))
    }
}

impl canvas::Program<Message> for GraphView<'_> {
    type State = Interaction;

    fn update(
        &self,
        state: &mut Interaction,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<Message>> {
        // Tell the app the canvas size, which the Canton layout wraps to.
        let size = bounds.size();
        if (size.width - self.known_size.width).abs() > 1.0 || (size.height - self.known_size.height).abs() > 1.0 {
            return Some(Action::publish(Message::CanvasResized(size)));
        }
        let Event::Mouse(event) = event else { return None };

        // Releases and drags are handled even if the cursor left the canvas.
        match (event, &mut *state) {
            (mouse::Event::CursorMoved { .. }, Interaction::PressedNode { node, start, dragging }) => {
                let point = cursor.position_in(bounds).or_else(|| {
                    cursor.position().map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))
                })?;
                if !*dragging && point.distance(*start) < DRAG_THRESHOLD {
                    return None;
                }
                *dragging = true;
                let pos = self.camera.to_graph(bounds, point);
                return Some(Action::publish(Message::DragNode(*node, pos)).and_capture());
            }
            (mouse::Event::CursorMoved { .. }, Interaction::Panning { last, .. }) => {
                let point = cursor.position().map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))?;
                let delta = point - *last;
                *last = point;
                return Some(Action::publish(Message::Pan(delta)).and_capture());
            }
            (mouse::Event::CursorMoved { .. }, Interaction::Scrolling { grab }) => {
                let point = cursor.position().map(|p| Point::new(p.x - bounds.x, p.y - bounds.y))?;
                let thumb = self.thumb(bounds)?;
                let to = self.scroll_for(bounds, thumb, point.y - *grab);
                return Some(Action::publish(Message::CantonScrollTo(to)).and_capture());
            }
            (mouse::Event::ButtonReleased(Button::Left), Interaction::Scrolling { .. }) => {
                *state = Interaction::Idle;
                return Some(Action::capture());
            }
            (mouse::Event::ButtonReleased(Button::Left), Interaction::PressedNode { node, dragging, .. }) => {
                let message = if *dragging {
                    Message::DragEnd
                } else {
                    Message::Select(Some(Selection::Node(*node)))
                };
                *state = Interaction::Idle;
                return Some(Action::publish(message).and_capture());
            }
            (mouse::Event::ButtonReleased(Button::Left), Interaction::Panning { start, last }) => {
                let clicked = start.distance(*last) < DRAG_THRESHOLD;
                let point = *start;
                *state = Interaction::Idle;
                if clicked {
                    // A click on the background selects a relation there, or clears the selection.
                    let selection = self.edge_at(bounds, point).map(Selection::Edge);
                    return Some(Action::publish(Message::Select(selection)).and_capture());
                }
                return Some(Action::capture());
            }
            _ => {}
        }

        let point = cursor.position_in(bounds)?;
        match event {
            mouse::Event::CursorMoved { .. } => {
                let hover = self.node_at(bounds, point);
                (hover != self.hover).then(|| Action::publish(Message::Hover(hover)))
            }
            mouse::Event::ButtonPressed(Button::Left)
                if self.thumb(bounds).is_some() && point.x >= bounds.width - SCROLLBAR_WIDTH =>
            {
                // On the thumb, drag it; elsewhere on the track, jump there.
                let thumb = self.thumb(bounds)?;
                if thumb.contains(point) {
                    *state = Interaction::Scrolling { grab: point.y - thumb.y };
                    return Some(Action::publish(Message::CloseMenu).and_capture());
                }
                *state = Interaction::Scrolling { grab: thumb.height / 2.0 };
                let to = self.scroll_for(bounds, thumb, point.y - thumb.height / 2.0);
                Some(Action::publish(Message::CantonScrollTo(to)).and_capture())
            }
            mouse::Event::ButtonPressed(Button::Left) => {
                *state = match self.node_at(bounds, point) {
                    Some(node) => Interaction::PressedNode { node, start: point, dragging: false },
                    None => Interaction::Panning { start: point, last: point },
                };
                Some(Action::publish(Message::CloseMenu).and_capture())
            }
            mouse::Event::ButtonPressed(Button::Right) => {
                let message = match self.node_at(bounds, point) {
                    Some(node) => Message::OpenMenu(node, point),
                    None => Message::CloseMenu,
                };
                Some(Action::publish(message).and_capture())
            }
            mouse::Event::WheelScrolled { delta } => {
                let lines = match delta {
                    ScrollDelta::Lines { y, .. } => *y,
                    ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                if self.scroll.is_some() {
                    return Some(Action::publish(Message::CantonScroll(-lines * 40.0)).and_capture());
                }
                let factor = 1.1_f32.powf(lines);
                let anchor = self.camera.to_graph(bounds, point);
                let from_centre = point - Point::new(bounds.width / 2.0, bounds.height / 2.0);
                Some(Action::publish(Message::Zoom { factor, anchor, from_centre }).and_capture())
            }
            mouse::Event::CursorLeft => self.hover.map(|_| Action::publish(Message::Hover(None))),
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &Interaction,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), BACKGROUND);

        let lit = self.lit();
        if self.visualization == Visualization::Canton {
            canton_draw::draw(self, &mut frame, bounds, lit.as_ref());
            if let Some(thumb) = self.thumb(bounds) {
                let track = Point::new(thumb.x, 0.0);
                frame.fill_rectangle(track, iced::Size::new(SCROLLBAR_WIDTH, bounds.height), SCROLLBAR_TRACK);
                let pill = Path::rounded_rectangle(
                    Point::new(thumb.x + 2.0, thumb.y + 2.0),
                    iced::Size::new(thumb.width - 4.0, thumb.height - 4.0),
                    4.0.into(),
                );
                frame.fill(&pill, SCROLLBAR_THUMB);
            }
            return vec![frame.into_geometry()];
        }
        draw_grid(&mut frame, bounds, self.camera);
        let is_lit = |i: usize| lit.as_ref().is_none_or(|set| set.contains(&i));
        let scale = self.camera.scale;
        let radii = radii(self.graph, scale);
        let bends = bends(self.graph);
        // The record whose relations are named: the hovered one, else the selected one.
        let focus = self.hover.or(match self.selection {
            Some(Selection::Node(n)) => Some(n),
            _ => None,
        });

        // Lots of relations are drawn fainter, so the records stay readable.
        let density = (400.0 / self.graph.edges.len().max(1) as f32).clamp(0.3, 1.0);

        // Relations first, so records are drawn on top; their names last, above everything.
        let mut edge_labels = Vec::new();
        for (i, e) in self.graph.edges.iter().enumerate() {
            if !self.graph.edge_visible(e) {
                continue;
            }
            let Some(c) = curve(self, bounds, i, &radii, bends[i]) else { continue };
            let touches_focus = focus.is_some_and(|f| e.from == f || e.to == f);
            let selected = self.selection == Some(Selection::Edge(i));
            let dimmed = lit.is_some() && !touches_focus && !selected;
            let (color, width) = match () {
                _ if selected => (SELECTED_RING, 2.4),
                _ if touches_focus => (EDGE_LIT, 1.8),
                _ if dimmed => (Color { a: 0.25 * density, ..EDGE_COLOR }, 1.2),
                _ => (Color { a: density, ..EDGE_COLOR }, 1.3),
            };
            let width = width * scale.clamp(0.7, 1.6);
            draw_curve(&mut frame, &c, color, width);
            let overview = e.id.starts_with(crate::graph::TABLE_NODE);
            if selected || touches_focus || overview || (focus.is_none() && scale >= 2.5) {
                edge_labels.push((c.at(0.5), e.label.clone()));
            }
        }

        let mut label_candidates = Vec::new();
        let bounds_with_margin = Rectangle::new(Point::new(-100.0, -40.0), iced::Size::new(bounds.width + 200.0, bounds.height + 80.0));
        let many = self.graph.nodes.len() > 400;
        for (i, n) in self.graph.nodes.iter().enumerate() {
            if n.hidden {
                continue;
            }
            let p = self.camera.to_screen(bounds, n.pos);
            let r = radii[i];
            if !bounds_with_margin.contains(p) {
                continue;
            }
            let lit_node = is_lit(i);
            let color = if lit_node { n.color } else { Color { a: 0.28, ..n.color } };
            if lit_node && !many {
                frame.fill(&Path::circle(p + Vector::new(0.0, 1.5), r + 1.5), SHADOW);
            }
            if self.hover == Some(i) {
                frame.fill(&Path::circle(p, r + 6.0), Color { a: 0.22, ..n.color });
            }
            frame.fill(&Path::circle(p, r), color);
            frame.stroke(
                &Path::circle(p, r),
                Stroke::default().with_color(Color { a: if lit_node { 1.0 } else { 0.6 }, ..Color::WHITE }).with_width(2.0),
            );
            if self.selection == Some(Selection::Node(i)) {
                frame.stroke(&Path::circle(p, r + 4.0), Stroke::default().with_color(SELECTED_RING).with_width(2.0));
            }
            if lit_node && bounds_with_margin.contains(p) {
                label_candidates.push((i, p, r));
            }
        }

        // Labels: the focus first, then the best-connected records; a label
        // that would overlap one already placed is left out, so zooming in
        // reveals more of them.
        let degree_of = |i: usize| radii[i];
        label_candidates.sort_by(|a, b| {
            let rank = |i: usize| (focus == Some(i), self.selection == Some(Selection::Node(i)));
            rank(b.0).cmp(&rank(a.0)).then(degree_of(b.0).total_cmp(&degree_of(a.0)))
        });
        let mut placed = Placed::default();
        for (i, p, r) in label_candidates {
            let n = &self.graph.nodes[i];
            let text = ellipsize(&n.label, 28);
            let strong = focus == Some(i);
            let size = 12.5;
            let w = text.chars().count() as f32 * size * 0.56 + 10.0;
            let h = size + 6.0;
            let rect = Rectangle::new(Point::new(p.x - w / 2.0, p.y + r + 5.0), iced::Size::new(w, h));
            if !strong && !placed.try_place(rect) {
                continue;
            }
            if strong {
                placed.place(rect);
            }
            pill_text(
                &mut frame,
                &text,
                Point::new(p.x, p.y + r + 5.0),
                size,
                if strong { SELECTED_RING } else { NODE_LABEL },
                strong,
                alignment::Vertical::Top,
            );
        }

        for (at, label) in edge_labels {
            pill_text(&mut frame, &label, at, 10.5, EDGE_LABEL, false, alignment::Vertical::Center);
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &Interaction, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        match state {
            Interaction::PressedNode { dragging: true, .. } | Interaction::Panning { .. } => {
                mouse::Interaction::Grabbing
            }
            _ => match cursor.position_in(bounds) {
                Some(p) if self.node_at(bounds, p).is_some() => mouse::Interaction::Pointer,
                _ => mouse::Interaction::default(),
            },
        }
    }
}

/// Each record's radius on screen: bigger the more relations it has.
fn radii(graph: &Graph, scale: f32) -> Vec<f32> {
    let mut degree = vec![0usize; graph.nodes.len()];
    for e in &graph.edges {
        if graph.edge_visible(e) {
            degree[e.from] += 1;
            degree[e.to] += 1;
        }
    }
    let zoom = scale.clamp(0.35, 2.0);
    degree
        .into_iter()
        .zip(&graph.nodes)
        .map(|(d, n)| {
            // A table in the overview is sized by how many records it has.
            let base = match n.data["records"].as_i64() {
                Some(records) if n.id.starts_with(crate::graph::TABLE_NODE) => {
                    10.0 + 5.0 * ((records.max(0) + 1) as f32).log10()
                }
                _ => (NODE_RADIUS + 2.2 * (d as f32).sqrt()).min(18.0),
            };
            base.min(32.0) * zoom
        })
        .collect()
}

/// How far each relation bends, in units of `BEND`: relations between the
/// same two records (either way round) fan out instead of overlapping, and
/// a lone relation is straight.
fn bends(graph: &Graph) -> Vec<f32> {
    let mut groups: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (i, e) in graph.edges.iter().enumerate() {
        groups.entry((e.from.min(e.to), e.from.max(e.to))).or_default().push(i);
    }
    let mut bend = vec![0.0; graph.edges.len()];
    for members in groups.values() {
        let mid = (members.len() as f32 - 1.0) / 2.0;
        for (k, &i) in members.iter().enumerate() {
            bend[i] = k as f32 - mid;
        }
    }
    bend
}

/// A relation on screen: a quadratic curve from the edge of one record's
/// circle to the edge of the other's.
pub(super) struct Curve {
    start: Point,
    control: Point,
    /// A second control point makes it a cubic curve (used for loops).
    control2: Option<Point>,
    tip: Point,
}

impl Curve {
    fn at(&self, t: f32) -> Point {
        let u = 1.0 - t;
        if let Some(c2) = self.control2 {
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            return Point::new(
                a * self.start.x + b * self.control.x + c * c2.x + d * self.tip.x,
                a * self.start.y + b * self.control.y + c * c2.y + d * self.tip.y,
            );
        }
        let (a, b, c) = (u * u, 2.0 * u * t, t * t);
        Point::new(
            a * self.start.x + b * self.control.x + c * self.tip.x,
            a * self.start.y + b * self.control.y + c * self.tip.y,
        )
    }

    fn points(&self) -> Vec<Point> {
        (0..=16).map(|i| self.at(i as f32 / 16.0)).collect()
    }
}

fn curve(view: &GraphView, bounds: Rectangle, i: usize, radii: &[f32], bend: f32) -> Option<Curve> {
    let e = &view.graph.edges[i];
    if e.from == e.to {
        // A relation from a record to itself: a loop above it (fanned out
        // sideways when there are several), drawn as one quadratic curve
        // through a point above the record.
        let p = view.camera.to_screen(bounds, view.graph.nodes[e.from].pos);
        let r = radii[e.from];
        let tilt = bend * 0.5;
        let up = Vector::new(tilt.sin(), -tilt.cos());
        let side = Vector::new(-up.y, up.x);
        let reach = r + 46.0;
        return Some(Curve {
            start: p + (up * 0.8 + side * 0.6) * r,
            control: p + up * reach + side * (reach * 0.75),
            control2: Some(p + up * reach - side * (reach * 0.75)),
            tip: p + (up * 0.8 - side * 0.6) * (r + 1.5),
        });
    }
    let a = view.camera.to_screen(bounds, view.graph.nodes[e.from].pos);
    let b = view.camera.to_screen(bounds, view.graph.nodes[e.to].pos);
    // Bend sideways from the line, measured the same way whichever end the
    // relation starts at, so relations both ways between two records fan out.
    let (lo, hi) = if e.from < e.to { (a, b) } else { (b, a) };
    let d = hi - lo;
    let len = (d.x * d.x + d.y * d.y).sqrt();
    if len <= radii[e.from] + radii[e.to] + ARROW_LENGTH {
        return None;
    }
    let normal = Vector::new(-d.y / len, d.x / len);
    let control = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0) + normal * (bend * BEND * 2.0);
    let towards = |from: Point, to: Point, by: f32| {
        let v = to - from;
        let l = (v.x * v.x + v.y * v.y).sqrt().max(0.01);
        from + v * (by / l)
    };
    Some(Curve {
        start: towards(a, control, radii[e.from]),
        control,
        control2: None,
        tip: towards(b, control, radii[e.to] + 1.5),
    })
}

/// Draws a relation with an arrowhead at its end.
fn draw_curve(frame: &mut Frame, c: &Curve, color: Color, width: f32) {
    let last_control = c.control2.unwrap_or(c.control);
    let d = c.tip - last_control;
    let len = (d.x * d.x + d.y * d.y).sqrt().max(0.01);
    let dir = d * (1.0 / len);
    let normal = Vector::new(-dir.y, dir.x);
    let head = ARROW_LENGTH * (width / 1.3).clamp(1.0, 1.4);
    let base = c.tip - dir * head;
    frame.stroke(
        &Path::new(|p| {
            p.move_to(c.start);
            match c.control2 {
                Some(c2) => p.bezier_curve_to(c.control, c2, base),
                None => p.quadratic_curve_to(c.control, base),
            }
        }),
        Stroke::default().with_color(color).with_width(width),
    );
    frame.fill(
        &Path::new(|p| {
            p.move_to(c.tip);
            p.line_to(base + normal * ARROW_WIDTH);
            p.quadratic_curve_to(base + dir * (head * 0.3), base - normal * ARROW_WIDTH);
            p.close();
        }),
        color,
    );
}

/// A faint dot grid that moves and zooms with the graph.
pub(super) fn draw_grid(frame: &mut Frame, bounds: Rectangle, camera: Camera) {
    let mut step = 28.0 * camera.scale;
    while step < 14.0 {
        step *= 2.0;
    }
    let origin = camera.to_screen(bounds, Vector::ZERO);
    let x0 = origin.x.rem_euclid(step);
    let y0 = origin.y.rem_euclid(step);
    let dots = Path::new(|p| {
        let mut y = y0;
        while y < bounds.height {
            let mut x = x0;
            while x < bounds.width {
                p.circle(Point::new(x, y), 1.1);
                x += step;
            }
            y += step;
        }
    });
    frame.fill(&dots, GRID_DOT);
}

/// Text on a rounded white label, so it stays readable over relations.
fn pill_text(
    frame: &mut Frame,
    content: &str,
    at: Point,
    size: f32,
    color: Color,
    bold: bool,
    align_y: alignment::Vertical,
) {
    let width = content.chars().count() as f32 * size * 0.56 + 10.0;
    let height = size + 6.0;
    let top = match align_y {
        alignment::Vertical::Top => at.y,
        alignment::Vertical::Center => at.y - height / 2.0,
        alignment::Vertical::Bottom => at.y - height,
    };
    let rect = Path::rounded_rectangle(Point::new(at.x - width / 2.0, top), iced::Size::new(width, height), (height / 2.0).into());
    frame.fill(&rect, PILL);
    frame.stroke(&rect, Stroke::default().with_color(PILL_LINE).with_width(1.0));
    frame.fill_text(Text {
        content: content.to_string(),
        position: Point::new(at.x, top + height / 2.0),
        color,
        size: size.into(),
        font: if bold { Font { weight: iced::font::Weight::Semibold, ..Font::default() } } else { Font::default() },
        align_x: Alignment::Center,
        align_y: alignment::Vertical::Center,
        ..Text::default()
    });
}

fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

pub(super) fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 == 0.0 {
        return p.distance(a);
    }
    let ap = p - a;
    let t = ((ap.x * ab.x + ap.y * ab.y) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Screen rectangles already taken by labels, bucketed in a coarse grid so
/// checking a new label only looks at its neighbours.
#[derive(Default)]
struct Placed {
    cells: HashMap<(i32, i32), Vec<Rectangle>>,
}

impl Placed {
    const CELL: f32 = 96.0;

    fn keys(rect: Rectangle) -> impl Iterator<Item = (i32, i32)> {
        let (x0, x1) = ((rect.x / Self::CELL).floor() as i32, ((rect.x + rect.width) / Self::CELL).floor() as i32);
        let (y0, y1) = ((rect.y / Self::CELL).floor() as i32, ((rect.y + rect.height) / Self::CELL).floor() as i32);
        (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
    }

    /// Places `rect` unless it overlaps a placed one.
    fn try_place(&mut self, rect: Rectangle) -> bool {
        let overlaps = Self::keys(rect)
            .any(|k| self.cells.get(&k).is_some_and(|rs| rs.iter().any(|r| r.intersects(&rect))));
        if !overlaps {
            self.place(rect);
        }
        !overlaps
    }

    fn place(&mut self, rect: Rectangle) {
        for k in Self::keys(rect) {
            self.cells.entry(k).or_default().push(rect);
        }
    }
}
