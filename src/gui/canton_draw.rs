//! Drawing the graph as a Canton event graph, in the style of
//! rust-canton-toolbox's event-graph-ui: transactions as pills, created
//! contracts as circles (faded once consumed), exercised choices as
//! diamonds and parties as hexagons, each with a white icon; relations as
//! curved arrows coloured and dashed by type.

use std::collections::HashSet;

use iced::alignment;
use iced::widget::canvas::{Frame, LineDash, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Point, Rectangle, Size, Vector};
use surrealdb_explorer::canton::{NodeKind, RelKind};

use crate::gui::app::Selection;
use crate::gui::canvas::{ANSWER_RING, GraphView};
use crate::gui::model::{Edge, Node};

/// Radius of a created contract, in graph units.
const R: f32 = 24.0;
/// Radius of a record that isn't part of the event graph.
const OTHER_R: f32 = 12.0;
const TX_SIZE: Size = Size::new(56.0, 36.0);

const FADED: Color = Color::from_rgb(0.89, 0.89, 0.89);
const SELECTED: Color = Color::from_rgb(1.0, 0.84, 0.0);
const SHADOW: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.18);
const EDGE_LABEL: Color = Color::from_rgb(0.4, 0.4, 0.4);

const fn hex(rgb: u32) -> Color {
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Main colour, lighter tint and label colour of each kind, as in the toolbox.
pub fn colors(kind: NodeKind) -> (Color, Color, Color) {
    match kind {
        NodeKind::Transaction => (hex(0x4A90D9), hex(0x6DB3F8), hex(0x1a3a5c)),
        NodeKind::Created => (hex(0x50C878), hex(0x7EDBA0), hex(0x1a4a2a)),
        NodeKind::Exercised => (hex(0xF5A623), hex(0xFFD080), hex(0x5a3a00)),
        NodeKind::Party => (hex(0x9B59B6), hex(0xC49ADB), hex(0x3a1a4a)),
        NodeKind::Other => (hex(0x7f8c8d), hex(0xa5b1b2), hex(0x222222)),
    }
}

/// Colour, dash pattern and width of each relation type, as in the toolbox.
pub fn edge_style(kind: RelKind) -> (Color, &'static [f32], f32) {
    match kind {
        RelKind::Action => (hex(0x555555), &[], 2.0),
        RelKind::Consequence => (hex(0x888888), &[6.0, 3.0], 2.0),
        RelKind::Target => (hex(0x4A90D9), &[3.0, 3.0], 2.0),
        RelKind::Consumes => (hex(0xE74C3C), &[], 2.0),
        RelKind::Requested => (hex(0x9B59B6), &[], 1.0),
        RelKind::Other => (hex(0x999999), &[], 1.2),
    }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgb(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t)
}

/// Half the size of a node in graph units, for hit testing.
pub fn hit_radius(kind: NodeKind) -> f32 {
    match kind {
        NodeKind::Other => OTHER_R,
        NodeKind::Transaction => TX_SIZE.width / 2.0,
        _ => R + 4.0,
    }
}

/// The curve of an edge: start, control point and end, on screen.
pub fn curve(view: &GraphView, bounds: Rectangle, e: &Edge) -> (Point, Point, Point) {
    let a = view.camera.to_screen(bounds, view.graph.nodes[e.from].pos);
    let b = view.camera.to_screen(bounds, view.graph.nodes[e.to].pos);
    // Edges between the same pair of records fan out by type; edges within a
    // row need a strong arc to be seen at all.
    let by_type = match e.kind {
        RelKind::Consequence => 30.0,
        RelKind::Target => -40.0,
        RelKind::Consumes => 40.0,
        _ => 0.0,
    } * view.camera.scale;
    let d = b - a;
    let len = (d.x * d.x + d.y * d.y).sqrt().max(1.0);
    let offset = if (a.y - b.y).abs() < 1.0 { (b.x - a.x).abs().max(50.0) * 0.4 + by_type } else { by_type };
    let normal = Vector::new(-d.y / len, d.x / len);
    let mid = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    (a, mid + normal * offset, b)
}

fn bezier(a: Point, c: Point, b: Point, t: f32) -> Point {
    let u = 1.0 - t;
    Point::new(u * u * a.x + 2.0 * u * t * c.x + t * t * b.x, u * u * a.y + 2.0 * u * t * c.y + t * t * b.y)
}

/// Whether `point` is within a few pixels of the edge's curve.
pub fn near_edge(view: &GraphView, bounds: Rectangle, e: &Edge, point: Point) -> bool {
    let (a, c, b) = curve(view, bounds, e);
    let samples: Vec<Point> = (0..=16).map(|i| bezier(a, c, b, i as f32 / 16.0)).collect();
    samples.windows(2).any(|w| super::canvas::distance_to_segment(point, w[0], w[1]) <= 5.0)
}

/// Draws the whole graph. `lit` are the records drawn in colour while hovering.
pub fn draw(view: &GraphView, frame: &mut Frame, bounds: Rectangle, lit: Option<&HashSet<usize>>) {
    let s = view.camera.scale;
    let g = view.graph;
    let is_lit = |i: usize| lit.is_none_or(|set| set.contains(&i));
    let context = view.result.as_ref().map(|(_, edges)| edges).filter(|edges| !edges.is_empty());
    let consumed = g.consumed();

    for (i, e) in g.edges.iter().enumerate() {
        if !g.edge_visible(e) {
            continue;
        }
        let in_context = context.is_some_and(|edges| edges.contains(&i));
        let lit_edge = (lit.is_none() || view.hover.is_some_and(|h| e.from == h || e.to == h))
            && (context.is_none() || in_context);
        let selected = view.selection == Some(Selection::Edge(i));
        let (color, dash, width) = edge_style(e.kind);
        let color = if selected { SELECTED } else if lit_edge { color } else { FADED };
        let width = if selected { width + 1.5 } else { width } * s.clamp(0.5, 2.0);
        draw_edge(frame, view, bounds, e, color, dash, width, lit_edge && s > 0.5);
    }

    for (i, n) in g.nodes.iter().enumerate() {
        if n.hidden {
            continue;
        }
        let p = view.camera.to_screen(bounds, n.pos);
        let answer = view.result.as_ref().is_some_and(|(answer, _)| answer.contains(&i));
        let selected = view.selection == Some(Selection::Node(i));
        draw_node(frame, n, p, s, is_lit(i), consumed.contains(&i), selected, answer);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_edge(
    frame: &mut Frame,
    view: &GraphView,
    bounds: Rectangle,
    e: &Edge,
    color: Color,
    dash: &'static [f32],
    width: f32,
    label: bool,
) {
    let (a, c, b) = curve(view, bounds, e);
    let path = Path::new(|p| {
        p.move_to(a);
        p.quadratic_curve_to(c, b);
    });
    let stroke = Stroke {
        line_dash: LineDash { segments: dash, offset: 0 },
        ..Stroke::default().with_color(color).with_width(width)
    };
    frame.stroke(&path, stroke);

    // Arrowhead halfway along the curve, as in the toolbox.
    let mid = bezier(a, c, b, 0.5);
    let t = b - a;
    let len = (t.x * t.x + t.y * t.y).sqrt();
    if len < 1.0 {
        return;
    }
    let dir = t * (1.0 / len);
    let normal = Vector::new(-dir.y, dir.x);
    let size = 8.0 * view.camera.scale.clamp(0.5, 2.0);
    let head = Path::new(|p| {
        p.move_to(mid + dir * size);
        p.line_to(mid - dir * size + normal * (size * 0.5));
        p.line_to(mid - dir * size - normal * (size * 0.5));
        p.close();
    });
    frame.fill(&head, color);

    if label && (e.kind != RelKind::Other || !e.label.is_empty()) {
        let name = if e.kind == RelKind::Other { e.label.clone() } else { e.kind.name().to_string() };
        // Beside a steep arrow, above a flat one, so it stays clear of the node labels.
        let gap = 6.0 * view.camera.scale.max(0.7);
        let (position, align_x, align_y) = if dir.y.abs() > dir.x.abs() {
            // On the side the curve bends to, so parallel edges' labels don't collide.
            if c.x < (a.x + b.x) / 2.0 - 1.0 {
                (Point::new(mid.x - gap - size * 0.5, mid.y), Alignment::Right, alignment::Vertical::Center)
            } else {
                (Point::new(mid.x + gap + size * 0.5, mid.y), Alignment::Left, alignment::Vertical::Center)
            }
        } else {
            (Point::new(mid.x, mid.y - gap), Alignment::Center, alignment::Vertical::Bottom)
        };
        frame.fill_text(Text {
            content: name,
            position,
            color: EDGE_LABEL,
            size: (9.0 * view.camera.scale).max(8.0).into(),
            align_x,
            align_y,
            ..Text::default()
        });
    }
}

/// The outline of a node of `kind` at `p`, grown by `grow` pixels.
fn shape(kind: NodeKind, p: Point, s: f32, grow: f32) -> Path {
    match kind {
        NodeKind::Transaction => {
            let size = Size::new(TX_SIZE.width * s + grow * 2.0, TX_SIZE.height * s + grow * 2.0);
            Path::rounded_rectangle(Point::new(p.x - size.width / 2.0, p.y - size.height / 2.0), size, (10.0 * s + grow).into())
        }
        NodeKind::Created => Path::circle(p, R * s + grow),
        NodeKind::Exercised => {
            let r = (R + 4.0) * s + grow * 1.4;
            Path::new(|b| {
                b.move_to(Point::new(p.x, p.y - r));
                b.line_to(Point::new(p.x + r, p.y));
                b.line_to(Point::new(p.x, p.y + r));
                b.line_to(Point::new(p.x - r, p.y));
                b.close();
            })
        }
        NodeKind::Party => {
            let r = (R + 4.0) * s + grow * 1.15;
            Path::new(|b| {
                for i in 0..6 {
                    let angle = std::f32::consts::PI / 3.0 * i as f32 - std::f32::consts::PI / 6.0;
                    let v = Point::new(p.x + r * angle.cos(), p.y + r * angle.sin());
                    if i == 0 { b.move_to(v) } else { b.line_to(v) }
                }
                b.close();
            })
        }
        NodeKind::Other => Path::circle(p, OTHER_R * s.clamp(0.6, 2.0) + grow),
    }
}

/// How far below its centre a node's label goes.
fn label_offset(kind: NodeKind, s: f32) -> f32 {
    match kind {
        NodeKind::Transaction => (TX_SIZE.height / 2.0 + 6.0) * s,
        NodeKind::Created => (R + 8.0) * s,
        NodeKind::Exercised | NodeKind::Party => (R + 8.0) * s,
        NodeKind::Other => OTHER_R * s.clamp(0.6, 2.0) + 3.0,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_node(frame: &mut Frame, n: &Node, p: Point, s: f32, lit: bool, consumed: bool, selected: bool, answer: bool) {
    let (main, light, text_color) = colors(n.kind);
    let (fill, ring, text_color) = match () {
        _ if !lit => (FADED, FADED, FADED),
        _ if n.kind == NodeKind::Other => (n.color, n.color, text_color),
        // Consumed (archived) contracts are washed out.
        _ if consumed && n.kind == NodeKind::Created => (hex(0xBBCEBB), hex(0xBFCFBF), hex(0x8a9a8a)),
        _ => (mix(light, main, 0.65), light, text_color),
    };

    if answer {
        frame.stroke(&shape(n.kind, p, s, 5.0), Stroke::default().with_color(ANSWER_RING).with_width(3.0));
    }
    if lit {
        frame.fill(&shape(n.kind, p + Vector::new(2.0, 2.0) * s.min(1.0), s, 0.0), SHADOW);
    }
    if n.kind == NodeKind::Created {
        let ring = Color { a: 0.4, ..ring };
        frame.stroke(&shape(n.kind, p, s, 2.0 * s), Stroke::default().with_color(ring).with_width(2.0 * s));
    }
    let outline = shape(n.kind, p, s, 0.0);
    frame.fill(&outline, fill);
    if selected {
        frame.stroke(&shape(n.kind, p, s, 3.0), Stroke::default().with_color(Color { a: 0.45, ..SELECTED }).with_width(5.0));
        frame.stroke(&outline, Stroke::default().with_color(SELECTED).with_width(3.0));
    } else {
        frame.stroke(&outline, Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.6)).with_width(1.5));
    }
    if lit && s > 0.3 {
        icon(frame, n.kind, p, s);
    }
    if lit && s > 0.35 {
        frame.fill_text(Text {
            content: n.canton_label.clone(),
            position: Point::new(p.x, p.y + label_offset(n.kind, s)),
            color: text_color,
            size: (11.0 * s).max(9.0).into(),
            align_x: Alignment::Center,
            align_y: alignment::Vertical::Top,
            ..Text::default()
        });
    }
}

/// The white icon inside a node, drawn at `p` (sizes as in the toolbox).
fn icon(frame: &mut Frame, kind: NodeKind, p: Point, s: f32) {
    let white = Color::WHITE;
    let pt = |x: f32, y: f32| Point::new(p.x + x * s, p.y + y * s);
    match kind {
        NodeKind::Transaction => {
            // A ticked box: committed.
            let b = 22.0 * 0.38;
            let size = Size::new(2.0 * b * s, 2.0 * b * s);
            frame.stroke(
                &Path::rounded_rectangle(pt(-b, -b), size, (22.0 * 0.08 * s).into()),
                Stroke::default().with_color(white).with_width(1.5 * s.min(1.5)),
            );
            let tick = Path::new(|t| {
                t.move_to(pt(-b * 0.5, 0.0));
                t.line_to(pt(-b * 0.1, b * 0.4));
                t.line_to(pt(b * 0.55, -b * 0.4));
            });
            frame.stroke(
                &tick,
                Stroke {
                    line_cap: iced::widget::canvas::LineCap::Round,
                    line_join: iced::widget::canvas::LineJoin::Round,
                    ..Stroke::default().with_color(white).with_width(2.5 * s.min(1.5))
                },
            );
        }
        NodeKind::Created => {
            // A document with a folded corner: a contract.
            let (w, h, fold) = (24.0 * 0.6, 24.0 * 0.8, 24.0 * 0.35);
            let doc = Path::new(|d| {
                d.move_to(pt(-w / 2.0, -h / 2.0));
                d.line_to(pt(w / 2.0 - fold, -h / 2.0));
                d.line_to(pt(w / 2.0, -h / 2.0 + fold));
                d.line_to(pt(w / 2.0, h / 2.0));
                d.line_to(pt(-w / 2.0, h / 2.0));
                d.close();
            });
            frame.fill(&doc, Color { a: 0.9, ..white });
            let corner = Path::new(|d| {
                d.move_to(pt(w / 2.0 - fold, -h / 2.0));
                d.line_to(pt(w / 2.0 - fold, -h / 2.0 + fold));
                d.line_to(pt(w / 2.0, -h / 2.0 + fold));
                d.close();
            });
            frame.fill(&corner, Color::from_rgba(0.0, 0.0, 0.0, 0.12));
        }
        NodeKind::Exercised => {
            // A lightning bolt: an action.
            let hs = 11.0;
            let bolt = Path::new(|d| {
                d.move_to(pt(-hs * 0.1, -hs * 0.8));
                d.line_to(pt(-hs * 0.5, hs * 0.05));
                d.line_to(pt(-hs * 0.1, hs * 0.05));
                d.line_to(pt(hs * 0.1, hs * 0.8));
                d.line_to(pt(hs * 0.5, -hs * 0.05));
                d.line_to(pt(hs * 0.1, -hs * 0.05));
                d.close();
            });
            frame.fill(&bolt, Color { a: 0.95, ..white });
        }
        NodeKind::Party => {
            // A person: head and shoulders.
            let size = 24.0;
            frame.fill(&Path::circle(pt(0.0, -size * 0.15), size * 0.18 * s), Color { a: 0.95, ..white });
            let body = Path::new(|d| {
                d.move_to(pt(-size * 0.35, size * 0.35));
                d.quadratic_curve_to(pt(-size * 0.35, size * 0.05), pt(0.0, size * 0.05));
                d.quadratic_curve_to(pt(size * 0.35, size * 0.05), pt(size * 0.35, size * 0.35));
                d.close();
            });
            frame.fill(&body, Color { a: 0.95, ..white });
        }
        NodeKind::Other => {}
    }
}
