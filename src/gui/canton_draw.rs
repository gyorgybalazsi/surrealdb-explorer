//! Drawing the graph as a Canton event graph, in the style of alt-canton's
//! ledger explorer (`explorer-ui/src/main.rs` and `index.html`): each record a
//! card with a coloured bar, a bold title and a monospace subtitle; tree
//! relations running down the left of their column; other relations as curves
//! between the cards' nearer sides; archived contracts faded and struck through.

use std::collections::{HashMap, HashSet};

use iced::alignment;
use iced::widget::canvas::{Frame, LineDash, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Font, Point, Rectangle, Size, Vector};
use crate::canton::{self, NODE_H, NODE_W, NodeKind, Status};

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

/// Whether a relation is drawn as a tree branch: down the left of its column.
fn is_tree(kind: &str, (ax, _): (f32, f32), (bx, _): (f32, f32)) -> bool {
    matches!(kind, "ACTION" | "CONSEQUENCE") && (bx - ax).abs() < NODE_W
}

/// Which sides of the two cards a curved relation joins: `true` for the right
/// side. As the explorer's `edge_path`, the nearer sides.
fn sides((ax, _): (f32, f32), (bx, _): (f32, f32)) -> (bool, bool) {
    if bx >= ax + NODE_W {
        (true, false)
    } else if bx + NODE_W <= ax {
        (false, true)
    } else {
        (true, true)
    }
}

/// The path of each relation in graph space, `None` for one not drawn: the
/// points to draw through, ending at the tip. The graph's edges come first,
/// then its bridges (see `canton::bridges`), which are drawn as tree branches.
///
/// A tree relation runs down the left of its column and into its child's left
/// side; any other curves between the cards' nearer sides. Every relation
/// gets its own port on each side it touches, spread down the side in the
/// order of the cards at the other end (a tree branch on top), so that two
/// relations never meet a card at the same point or run along the same line,
/// and each arrow can be told apart.
pub fn routes(view: &GraphView) -> Vec<Option<Vec<Point>>> {
    let g = view.graph;
    let corners: Vec<(f32, f32)> = g.nodes.iter().map(|n| corner(n.pos)).collect();
    // Every relation as (from, to, kind, drawn).
    let shown = |i: usize| g.nodes[i].card.shown;
    let rels: Vec<(usize, usize, &str, bool)> = g
        .edges
        .iter()
        .map(|e| (e.from, e.to, e.kind.as_str(), edge_drawn(view, e)))
        .chain(g.bridges.iter().map(|&(a, b)| (a, b, "ACTION", shown(a) && shown(b))))
        .collect();
    // The relations at each side of each card: (node, right side) ->
    // (tree branch first, the other card's y, relation, at its end).
    let mut ports: HashMap<(usize, bool), Vec<(bool, f32, usize, bool)>> = HashMap::new();
    for (i, &(from, to, kind, drawn)) in rels.iter().enumerate() {
        if !drawn {
            continue;
        }
        let (a, b) = (corners[from], corners[to]);
        if is_tree(kind, a, b) {
            ports.entry((to, false)).or_default().push((false, a.1, i, true));
        } else {
            let (from_right, to_right) = sides(a, b);
            ports.entry((from, from_right)).or_default().push((true, b.1, i, false));
            ports.entry((to, to_right)).or_default().push((true, a.1, i, true));
        }
    }
    // The y of each relation's ends.
    let mut port_y: HashMap<(usize, bool), f32> = HashMap::new();
    for ((node, _), mut list) in ports {
        list.sort_by(|p, q| (p.0, p.1, p.2, p.3).partial_cmp(&(q.0, q.1, q.2, q.3)).unwrap_or(std::cmp::Ordering::Equal));
        let n = list.len() as f32;
        for (k, &(_, _, edge, at_end)) in list.iter().enumerate() {
            port_y.insert((edge, at_end), corners[node].1 + NODE_H * (k as f32 + 1.0) / (n + 1.0));
        }
    }
    rels.iter()
        .enumerate()
        .map(|(i, &(from, to, kind, drawn))| {
            if !drawn {
                return None;
            }
            let (a, b) = (corners[from], corners[to]);
            let ey = port_y[&(i, true)];
            if is_tree(kind, a, b) {
                let x = a.0 + 10.0;
                return Some(vec![Point::new(x, a.1 + NODE_H), Point::new(x, ey), Point::new(b.0, ey)]);
            }
            let sy = port_y[&(i, false)];
            let (from_right, to_right) = sides(a, b);
            let sx = if from_right { a.0 + NODE_W } else { a.0 };
            let ex = if to_right { b.0 + NODE_W } else { b.0 };
            Some(curve(Point::new(sx, sy), Point::new(ex, ey), from_right, to_right))
        })
        .collect()
}

/// A cubic Bézier from `p0` to `p3`, sampled, leaving and entering each card
/// square to its side.
fn curve(p0: Point, p3: Point, from_right: bool, to_right: bool) -> Vec<Point> {
    let bend = ((p3.x - p0.x).abs() / 2.0).max(40.0);
    let c1 = if from_right { p0.x + bend } else { p0.x - bend };
    let c2 = if to_right { p3.x + bend } else { p3.x - bend };
    let (p1, p2) = (Point::new(c1, p0.y), Point::new(c2, p3.y));
    (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            Point::new(a * p0.x + b * p1.x + c * p2.x + d * p3.x, a * p0.y + b * p1.y + c * p2.y + d * p3.y)
        })
        .collect()
}

fn to_screen(view: &GraphView, bounds: Rectangle, pts: &[Point]) -> Vec<Point> {
    pts.iter().map(|p| view.camera.to_screen(bounds, Vector::new(p.x, p.y))).collect()
}

/// Whether a relation's two records are both on screen.
pub fn edge_drawn(view: &GraphView, e: &Edge) -> bool {
    view.graph.nodes[e.from].card.shown && view.graph.nodes[e.to].card.shown
}

/// The drawn relation passing within a few pixels of `point`.
pub fn edge_at(view: &GraphView, bounds: Rectangle, point: Point) -> Option<usize> {
    let edges = view.graph.edges.len();
    routes(view).into_iter().take(edges).enumerate().find_map(|(i, pts)| {
        let pts = to_screen(view, bounds, &pts?);
        pts.windows(2).any(|w| super::canvas::distance_to_segment(point, w[0], w[1]) <= 5.0).then_some(i)
    })
}

/// How strongly record `i`'s card is drawn, from 0 to 1: dimmed when
/// another record is hovered or selected and `i` isn't next to it, and
/// faded when it is an archived (consumed) contract.
pub(super) fn card_alpha(lit: Option<&HashSet<usize>>, i: usize, status: Option<Status>) -> f32 {
    let mut alpha: f32 = 1.0;
    if lit.is_some_and(|set| !set.contains(&i)) {
        alpha *= 0.35;
    }
    if status == Some(Status::Archived) {
        alpha *= 0.5;
    }
    alpha
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

    let routes = routes(view);
    for (i, e) in g.edges.iter().enumerate() {
        let Some(route) = &routes[i] else {
            continue;
        };
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
        let pts = to_screen(view, bounds, route);
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
        arrowhead(frame, &pts, color, s);
    }

    // Branches past filtered-out events: dotted, as the events between are hidden.
    for (k, &(a, b)) in g.bridges.iter().enumerate() {
        let Some(route) = &routes[g.edges.len() + k] else {
            continue;
        };
        let focus = view.hover.or(match view.selection {
            Some(Selection::Node(n)) => Some(n),
            _ => None,
        });
        let alpha = match focus {
            Some(f) if f == a || f == b => 1.0,
            Some(_) => 0.15,
            None => 0.85,
        };
        let color = faded(edge_style("ACTION").0, alpha);
        let pts = to_screen(view, bounds, route);
        let path = Path::new(|p| {
            p.move_to(pts[0]);
            for q in &pts[1..] {
                p.line_to(*q);
            }
        });
        let dash = [3.0 * s, 3.0 * s];
        frame.stroke(
            &path,
            Stroke {
                line_dash: LineDash { segments: &dash, offset: 0 },
                ..Stroke::default().with_color(color).with_width(1.4 * s.clamp(0.6, 1.5))
            },
        );
        arrowhead(frame, &pts, color, s);
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
        let alpha = card_alpha(lit, i, n.card.status);
        let archived = n.card.status == Some(Status::Archived);
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

/// An arrowhead at the end of `pts` (on screen), along the last segment.
fn arrowhead(frame: &mut Frame, pts: &[Point], color: Color, s: f32) {
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
    if !g.bridges.is_empty() {
        items.push((edge_style("ACTION").0, "via hidden events", Some(&[2.0, 2.0])));
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
