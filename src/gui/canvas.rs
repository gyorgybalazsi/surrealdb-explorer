//! Drawing the graph and turning mouse input into messages.

use std::collections::HashSet;

use iced::alignment;
use iced::mouse::{self, Button, Cursor, ScrollDelta};
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Point, Rectangle, Renderer, Theme, Vector};

use crate::gui::app::{Message, Selection};
use crate::gui::model::Graph;

const NODE_RADIUS: f32 = 9.0;
const ARROW_LENGTH: f32 = 9.0;
const ARROW_WIDTH: f32 = 4.5;
/// How far the mouse must move before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 4.0;

const BACKGROUND: Color = Color::from_rgb(0.98, 0.98, 0.98);
const EDGE_COLOR: Color = Color::from_rgb(0.6, 0.6, 0.6);
const EDGE_LABEL: Color = Color::from_rgb(0.47, 0.47, 0.47);
const NODE_LABEL: Color = Color::from_rgb(0.13, 0.13, 0.13);
const FADED: Color = Color::from_rgb(0.89, 0.89, 0.89);
const SELECTED_RING: Color = Color::from_rgb(0.13, 0.13, 0.13);
/// Ring around records that answer a query.
pub(super) const ANSWER_RING: Color = Color::from_rgb(0.95, 0.55, 0.05);
/// Relations that show how a query's answer connects.
const CONTEXT_EDGE: Color = Color::from_rgb(0.3, 0.3, 0.3);

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
    /// A query result shown in the graph: the answer records and the
    /// relations that connect them.
    pub result: Option<(HashSet<usize>, HashSet<usize>)>,
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
}

impl GraphView<'_> {
    /// The visible record under `point`, topmost first.
    fn node_at(&self, bounds: Rectangle, point: Point) -> Option<usize> {
        let r = NODE_RADIUS * self.camera.scale.max(0.6) + 3.0;
        self.graph
            .nodes
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, n)| !n.hidden)
            .find(|(_, n)| self.camera.to_screen(bounds, n.pos).distance(point) <= r)
            .map(|(i, _)| i)
    }

    /// The visible relation passing close to `point`.
    fn edge_at(&self, bounds: Rectangle, point: Point) -> Option<usize> {
        self.graph
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| self.graph.edge_visible(e))
            .find(|(_, e)| {
                let a = self.camera.to_screen(bounds, self.graph.nodes[e.from].pos);
                let b = self.camera.to_screen(bounds, self.graph.nodes[e.to].pos);
                distance_to_segment(point, a, b) <= 5.0
            })
            .map(|(i, _)| i)
    }

    /// Records (and their relations) to draw in full colour while hovering.
    fn lit(&self) -> Option<HashSet<usize>> {
        self.hover.map(|h| self.graph.neighbourhood(h))
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
        let is_lit = |i: usize| lit.as_ref().is_none_or(|set| set.contains(&i));
        let scale = self.camera.scale;
        let radius = NODE_RADIUS * scale.clamp(0.6, 2.0);

        // With a query result shown, only the connecting relations are emphasised.
        let context = self.result.as_ref().map(|(_, edges)| edges).filter(|edges| !edges.is_empty());

        // Relations first, so records are drawn on top.
        for (i, e) in self.graph.edges.iter().enumerate() {
            if !self.graph.edge_visible(e) {
                continue;
            }
            let in_context = context.is_some_and(|edges| edges.contains(&i));
            let lit_edge = (lit.is_none() || self.hover.is_some_and(|h| e.from == h || e.to == h))
                && (context.is_none() || in_context);
            let selected = self.selection == Some(Selection::Edge(i));
            let color = match () {
                _ if selected => SELECTED_RING,
                _ if !lit_edge => FADED,
                _ if in_context => CONTEXT_EDGE,
                _ => EDGE_COLOR,
            };
            let width = if selected || in_context { 2.0 } else { 1.2 };
            let a = self.camera.to_screen(bounds, self.graph.nodes[e.from].pos);
            let b = self.camera.to_screen(bounds, self.graph.nodes[e.to].pos);
            draw_arrow(&mut frame, a, b, radius, color, width);
            if lit_edge && scale > 0.5 {
                frame.fill_text(Text {
                    content: e.label.clone(),
                    position: Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0 - 2.0),
                    color: EDGE_LABEL,
                    size: 11.0.into(),
                    align_x: Alignment::Center,
                    align_y: alignment::Vertical::Bottom,
                    ..Text::default()
                });
            }
        }

        for (i, n) in self.graph.nodes.iter().enumerate() {
            if n.hidden {
                continue;
            }
            let p = self.camera.to_screen(bounds, n.pos);
            let lit_node = is_lit(i);
            if self.result.as_ref().is_some_and(|(answer, _)| answer.contains(&i)) {
                frame.stroke(
                    &Path::circle(p, radius + 4.0),
                    Stroke::default().with_color(ANSWER_RING).with_width(3.0),
                );
            }
            frame.fill(&Path::circle(p, radius), if lit_node { n.color } else { FADED });
            if self.selection == Some(Selection::Node(i)) {
                frame.stroke(
                    &Path::circle(p, radius + 3.0),
                    Stroke::default().with_color(SELECTED_RING).with_width(2.0),
                );
            }
            if lit_node && scale > 0.35 {
                frame.fill_text(Text {
                    content: n.label.clone(),
                    position: Point::new(p.x, p.y + radius + 3.0),
                    color: NODE_LABEL,
                    size: 13.0.into(),
                    align_x: Alignment::Center,
                    align_y: alignment::Vertical::Top,
                    ..Text::default()
                });
            }
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

/// A line from the edge of circle `a` to the edge of circle `b`, with an arrowhead at `b`.
fn draw_arrow(frame: &mut Frame, a: Point, b: Point, radius: f32, color: Color, width: f32) {
    let d = b - a;
    let len = (d.x * d.x + d.y * d.y).sqrt();
    if len <= radius * 2.0 + ARROW_LENGTH {
        return;
    }
    let dir = d * (1.0 / len);
    let normal = Vector::new(-dir.y, dir.x);
    let start = a + dir * radius;
    let tip = b - dir * (radius + 1.0);
    let base = tip - dir * ARROW_LENGTH;

    frame.stroke(&Path::line(start, base), Stroke::default().with_color(color).with_width(width));
    let head = Path::new(|p| {
        p.move_to(tip);
        p.line_to(base + normal * ARROW_WIDTH);
        p.line_to(base - normal * ARROW_WIDTH);
        p.close();
    });
    frame.fill(&head, color);
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 == 0.0 {
        return p.distance(a);
    }
    let ap = p - a;
    let t = ((ap.x * ab.x + ap.y * ab.y) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}
