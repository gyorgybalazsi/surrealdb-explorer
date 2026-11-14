//! Drawing the graph as a Canton event graph, in the style of alt-canton's
//! ledger explorer (`explorer-ui/src/main.rs` and `index.html`): each record a
//! card with a coloured bar, a bold title and a monospace subtitle; tree
//! relations running down the left of their column; other relations as curves
//! between the cards' nearer sides; archived contracts faded and struck through.

use std::collections::HashSet;

use iced::alignment;
use iced::widget::canvas::{Frame, LineDash, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Font, Point, Rectangle, Size, Vector};
use surrealdb_explorer::canton::{self, NODE_H, NODE_W, NodeKind, Status};

use crate::gui::app::Selection;
use crate::gui::canvas::GraphView;
use crate::gui::model::Edge;

pub const BACKGROUND: Color = hex(0xf6f7f9);
const NODE_BG: Color = Color::WHITE;
const TEXT: Color = hex(0x1d2430);
const MUTED: Color = hex(0x687385);
const PANEL_LINE: Color = hex(0xdfe3e8);
const SELECTED: Color = hex(0x2f6fde);

const fn hex(rgb: u32) -> Color {
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// The colour of each kind of record, as in the explorer.
pub fn kind_color(kind: NodeKind) -> Color {
    match kind {
        NodeKind::Transaction => hex(0x4b5563),
        NodeKind::Created => hex(0x16a34a),
        NodeKind::Exercised => hex(0xd97706),
        NodeKind::Fetched => hex(0x0891b2),
        NodeKind::Reassignment => hex(0x7c3aed),
        NodeKind::Topology => hex(0x0d9488),
        NodeKind::Party => hex(0xdb2777),
        NodeKind::Other => hex(0x94a3b8),
    }
}

/// Colour and dash pattern of each relationship type, as in the explorer.
pub fn edge_style(kind: &str) -> (Color, &'static [f32]) {
    match kind {
        "ACTION" | "CONSEQUENCE" => (hex(0x9aa3af), &[]),
        "CONSUMES" => (hex(0xdc2626), &[]),
        "TARGET" => (hex(0x64748b), &[5.0, 4.0]),
        "ASSIGNS" | "UNASSIGNS" => (kind_color(NodeKind::Reassignment), &[6.0, 3.0]),
        "HOSTS" | "KEYS" => (kind_color(NodeKind::Topology), &[2.0, 3.0]),
        _ => (hex(0xc4c9d1), &[2.0, 3.0]),
    }
}

fn faded(c: Color, alpha: f32) -> Color {
    Color { a: c.a * alpha, ..c }
}

/// The card's top-left corner in graph space (positions are card centres).
fn corner(pos: Vector) -> (f32, f32) {
    (pos.x - NODE_W / 2.0, pos.y - NODE_H / 2.0)
}

/// Whether `point` (on screen) is on the card of record `i`.
pub fn on_card(view: &GraphView, bounds: Rectangle, i: usize, point: Point) -> bool {
    let g = view.camera.to_graph(bounds, point);
    let p = view.graph.nodes[i].pos;
    (g.x - p.x).abs() <= NODE_W / 2.0 && (g.y - p.y).abs() <= NODE_H / 2.0
}

/// The path of a relation in graph space, as the explorer's `edge_path`: a
/// tree relation runs down the left of its column; any other curves between
/// the nearer sides. Returns the points to draw through, ending at the tip.
fn edge_points(kind: &str, (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> Vec<Point> {
    if matches!(kind, "ACTION" | "CONSEQUENCE") && (bx - ax).abs() < NODE_W {
        let x = ax + 10.0;
        return vec![Point::new(x, ay + NODE_H), Point::new(x, by + NODE_H / 2.0), Point::new(bx, by + NODE_H / 2.0)];
    }
    let (sx, ex) = if bx >= ax + NODE_W {
        (ax + NODE_W, bx)
    } else if bx + NODE_W <= ax {
        (ax, bx + NODE_W)
    } else {
        (ax + NODE_W, bx + NODE_W)
    };
    let (sy, ey) = (ay + NODE_H / 2.0, by + NODE_H / 2.0);
    let bend = ((ex - sx).abs() / 2.0).max(40.0);
    let (c1, c2) = if ex >= sx {
        (sx + bend, ex - bend)
    } else if (bx - ax).abs() < NODE_W {
        (sx + bend, ex + bend)
    } else {
        (sx - bend, ex + bend)
    };
    // A cubic Bézier, sampled.
    let (p0, p1, p2, p3) = (Point::new(sx, sy), Point::new(c1, sy), Point::new(c2, ey), Point::new(ex, ey));
    (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            Point::new(a * p0.x + b * p1.x + c * p2.x + d * p3.x, a * p0.y + b * p1.y + c * p2.y + d * p3.y)
        })
        .collect()
}

fn screen_points(view: &GraphView, bounds: Rectangle, e: &Edge) -> Vec<Point> {
    let a = corner(view.graph.nodes[e.from].pos);
    let b = corner(view.graph.nodes[e.to].pos);
    edge_points(&e.kind, a, b)
        .into_iter()
        .map(|p| view.camera.to_screen(bounds, Vector::new(p.x, p.y)))
        .collect()
}

/// Whether a relation's two records are both on screen.
pub fn edge_drawn(view: &GraphView, e: &Edge) -> bool {
    view.graph.nodes[e.from].card.shown && view.graph.nodes[e.to].card.shown
}

/// Whether `point` is within a few pixels of the relation's path.
pub fn near_edge(view: &GraphView, bounds: Rectangle, e: &Edge, point: Point) -> bool {
    screen_points(view, bounds, e)
        .windows(2)
        .any(|w| super::canvas::distance_to_segment(point, w[0], w[1]) <= 5.0)
}

/// Draws the whole graph. `lit` are the records to emphasise (a hovered or
/// selected record and its neighbours); the rest are dimmed.
pub fn draw(view: &GraphView, frame: &mut Frame, bounds: Rectangle, lit: Option<&HashSet<usize>>) {
    frame.fill_rectangle(Point::ORIGIN, bounds.size(), BACKGROUND);
    super::canvas::draw_grid(frame, bounds, view.camera);
    let s = view.camera.scale;
    let g = view.graph;

    for (name, at) in &g.bands {
        let p = view.camera.to_screen(bounds, *at + Vector::new(0.0, 16.0));
        frame.fill_text(Text {
            content: name.clone(),
            position: p,
            color: MUTED,
            size: (12.0 * s).into(),
            font: bold(),
            align_y: alignment::Vertical::Bottom,
            ..Text::default()
        });
    }

    for (i, e) in g.edges.iter().enumerate() {
        if !edge_drawn(view, e) {
            continue;
        }
        let touches = |set: &HashSet<usize>| set.contains(&e.from) && set.contains(&e.to);
        let focus = view.hover.or(match view.selection {
            Some(Selection::Node(n)) => Some(n),
            _ => None,
        });
        let lit_edge = focus.is_some_and(|f| e.from == f || e.to == f) && lit.is_some_and(touches);
        let selected = view.selection == Some(Selection::Edge(i));
        let (color, dash) = edge_style(&e.kind);
        let (alpha, width) = match () {
            _ if selected => (1.0, 3.0),
            _ if lit_edge => (1.0, 2.4),
            _ if focus.is_some() => (0.15, 1.4),
            _ => (0.85, 1.4),
        };
        let color = if selected { SELECTED } else { faded(color, alpha) };
        let pts = screen_points(view, bounds, e);
        let path = Path::new(|b| {
            b.move_to(pts[0]);
            for p in &pts[1..] {
                b.line_to(*p);
            }
        });
        let dash: Vec<f32> = dash.iter().map(|d| d * s).collect();
        frame.stroke(
            &path,
            Stroke {
                line_dash: LineDash { segments: &dash, offset: 0 },
                ..Stroke::default().with_color(color).with_width(width * s.clamp(0.6, 1.5))
            },
        );
        // Arrowhead at the end, along the last segment.
        let (tip, before) = (pts[pts.len() - 1], pts[pts.len() - 2]);
        let d = tip - before;
        let len = (d.x * d.x + d.y * d.y).sqrt();
        if len > 0.01 {
            let dir = d * (1.0 / len);
            let normal = Vector::new(-dir.y, dir.x);
            let size = 7.0 * s.clamp(0.6, 1.5);
            let head = Path::new(|b| {
                b.move_to(tip);
                b.line_to(tip - dir * size + normal * (size * 0.5));
                b.line_to(tip - dir * size - normal * (size * 0.5));
                b.close();
            });
            frame.fill(&head, color);
        }
    }

    for (i, n) in g.nodes.iter().enumerate() {
        if !n.card.shown {
            continue;
        }
        let (x, y) = corner(n.pos);
        let p = view.camera.to_screen(bounds, Vector::new(x, y));
        let size = Size::new(NODE_W * s, NODE_H * s);
        let color = kind_color(n.kind);
        let selected = view.selection == Some(Selection::Node(i));
        let hovered = view.hover == Some(i);
        let mut alpha: f32 = 1.0;
        if lit.is_some_and(|set| !set.contains(&i)) {
            alpha *= 0.35;
        }
        let archived = n.card.status == Some(Status::Archived);
        if archived {
            alpha *= 0.5;
        }
        let card = Path::rounded_rectangle(p, size, (8.0 * s).into());
        // A soft shadow, stronger under the hovered card.
        let lift = if hovered || selected { 2.0 } else { 1.0 };
        for (grow, a) in [(0.0, 0.07), (2.0 * s, 0.035)] {
            let shadow = Path::rounded_rectangle(
                p + Vector::new(-grow, (1.5 * lift - grow * 0.5) * s.max(0.5)),
                Size::new(size.width + 2.0 * grow, size.height + 2.0 * grow),
                ((8.0 + grow) * s).into(),
            );
            frame.fill(&shadow, Color { a: a * lift * alpha, ..Color::BLACK });
        }
        // A light tint of the kind's colour, so kinds read at a glance.
        let tint = Color {
            r: 1.0 + (color.r - 1.0) * 0.07,
            g: 1.0 + (color.g - 1.0) * 0.07,
            b: 1.0 + (color.b - 1.0) * 0.07,
            a: 1.0,
        };
        frame.fill(&card, faded(if archived { NODE_BG } else { tint }, alpha.max(0.9)));
        let mut outline = Stroke::default()
            .with_color(faded(if selected { SELECTED } else { color }, alpha))
            .with_width(if selected { 2.5 } else if hovered { 2.0 } else { 1.0 });
        if n.card.status == Some(Status::InTransit) {
            outline.line_dash = LineDash { segments: &[5.0, 3.0], offset: 0 };
        }
        frame.stroke(&card, outline);
        frame.fill(&Path::rounded_rectangle(p, Size::new(6.0 * s, size.height), (3.0 * s).into()), faded(color, alpha));

        if s < 0.3 {
            continue;
        }
        let title = canton::ellipsize(&n.card.title, 20);
        let title_size = 12.0 * s;
        let title_at = p + Vector::new(13.0 * s, 18.5 * s);
        frame.fill_text(Text {
            content: title.clone(),
            position: title_at,
            color: faded(TEXT, alpha),
            size: title_size.into(),
            font: bold(),
            align_y: alignment::Vertical::Bottom,
            ..Text::default()
        });
        if archived {
            // Struck through, as the explorer's `text-decoration: line-through`.
            let width = title.chars().count() as f32 * title_size * 0.5;
            let y = title_at.y - title_size * 0.55;
            frame.stroke(
                &Path::line(Point::new(title_at.x, y), Point::new(title_at.x + width, y)),
                Stroke::default().with_color(faded(TEXT, alpha)).with_width(1.0),
            );
        }
        frame.fill_text(Text {
            content: canton::ellipsize(&n.card.subtitle, 24),
            position: p + Vector::new(13.0 * s, 33.5 * s),
            color: faded(MUTED, alpha),
            size: (11.0 * s).into(),
            font: Font::MONOSPACE,
            align_y: alignment::Vertical::Bottom,
            ..Text::default()
        });
    }

    legend(frame, bounds, g);
}

fn bold() -> Font {
    Font { weight: iced::font::Weight::Semibold, ..Font::default() }
}

/// The key at the bottom left, as the explorer's legend.
/// Only lists the kinds of records and relations the graph has.
fn legend(frame: &mut Frame, bounds: Rectangle, g: &crate::gui::model::Graph) {
    let mut items: Vec<(Color, &str, Option<&'static [f32]>)> = NodeKind::ALL
        .iter()
        .filter(|&&k| g.nodes.iter().any(|n| n.card.shown && n.kind == k))
        .map(|&k| (kind_color(k), k.name(), None))
        .collect();
    if g.edges.iter().any(|e| e.kind == "CONSUMES") {
        items.push((edge_style("CONSUMES").0, "consumes", Some(&[])));
    }
    if g.edges.iter().any(|e| e.kind == "TARGET") {
        items.push((edge_style("TARGET").0, "target", Some(&[4.0, 3.0])));
    }
    if items.is_empty() {
        return;
    }
    let char_w = 11.0 * 0.6;
    let widths: Vec<f32> = items.iter().map(|(_, name, _)| 18.0 + name.len() as f32 * char_w + 10.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 10.0;
    let origin = Point::new(10.0, bounds.height - 34.0);
    frame.fill(&Path::rounded_rectangle(origin, Size::new(total, 24.0), 8.0.into()), Color::WHITE);
    frame.stroke(
        &Path::rounded_rectangle(origin, Size::new(total, 24.0), 8.0.into()),
        Stroke::default().with_color(PANEL_LINE).with_width(1.0),
    );
    let mut x = origin.x + 9.0;
    let mid = origin.y + 12.0;
    for ((color, name, line), w) in items.into_iter().zip(widths) {
        match line {
            None => frame.fill(&Path::circle(Point::new(x + 4.0, mid), 4.0), color),
            Some(dash) => frame.stroke(
                &Path::line(Point::new(x - 2.0, mid), Point::new(x + 12.0, mid)),
                Stroke { line_dash: LineDash { segments: dash, offset: 0 }, ..Stroke::default().with_color(color).with_width(2.0) },
            ),
        }
        frame.fill_text(Text {
            content: name.to_string(),
            position: Point::new(x + 15.0, mid),
            color: MUTED,
            size: 11.0.into(),
            align_x: Alignment::Left,
            align_y: alignment::Vertical::Center,
            ..Text::default()
        });
        x += w;
    }
}
