//! A small force-directed layout: records push each other apart, relations
//! pull their ends together, and a weak pull toward the centre keeps
//! unconnected records from drifting away.
//!
//! The simulation "cools down" (`alpha` decays) so it settles and stops;
//! dragging a record warms it up again.

use iced::Vector;

use crate::gui::model::Graph;

const REPULSION: f32 = 9000.0;
const SPRING: f32 = 0.04;
const SPRING_LENGTH: f32 = 130.0;
const GRAVITY: f32 = 0.012;
const DAMPING: f32 = 0.6;
const MAX_STEP: f32 = 40.0;
const ALPHA_DECAY: f32 = 0.985;
const ALPHA_MIN: f32 = 0.005;
/// From how many visible records repulsion is approximated.
const BARNES_HUT_FROM: usize = 250;
/// How coarse the approximation is: a group is treated as one point when
/// its size is less than THETA times its distance.
const THETA: f32 = 0.9;

pub struct Layout {
    alpha: f32,
    /// Off while records are placed some other way (the Canton event graph
    /// layout): then nothing moves unless dragged.
    enabled: bool,
}

impl Layout {
    pub fn new() -> Self {
        Layout { alpha: 1.0, enabled: true }
    }

    /// Turns the simulation on (moving straight away) or off.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.alpha = if enabled { 1.0 } else { 0.0 };
    }

    /// Whether the layout is still moving (and needs animation frames).
    pub fn is_active(&self) -> bool {
        self.alpha > ALPHA_MIN
    }

    /// Restarts movement, e.g. while a record is being dragged.
    pub fn reheat(&mut self, alpha: f32) {
        if !self.enabled {
            return;
        }
        self.alpha = self.alpha.max(alpha);
    }

    /// Advances the simulation one step. `pinned` is a record held by the
    /// mouse: it pushes and pulls others but doesn't move by itself.
    pub fn step(&mut self, graph: &mut Graph, pinned: Option<usize>) {
        if !self.is_active() {
            return;
        }
        let n = graph.nodes.len();
        let mut force = vec![Vector::ZERO; n];

        // Every pair of visible records repels. Past a few hundred records,
        // far-away groups are approximated by their centre (Barnes–Hut), so a
        // step costs about n·log n instead of n².
        if graph.nodes.iter().filter(|n| !n.hidden).count() > BARNES_HUT_FROM {
            let points: Vec<(usize, Vector)> =
                graph.nodes.iter().enumerate().filter(|(_, n)| !n.hidden).map(|(i, n)| (i, n.pos)).collect();
            let tree = QuadTree::build(&points);
            for &(i, p) in &points {
                force[i] = force[i] + tree.repulsion(i, p);
            }
        } else {
            self.exact_repulsion(graph, &mut force);
        }

        self.springs_and_move(graph, pinned, force);
    }

    /// Runs the simulation for up to `budget` without drawing, so a big
    /// graph appears (nearly) settled instead of slowly untangling on screen.
    pub fn settle(&mut self, graph: &mut Graph, budget: std::time::Duration) {
        let started = std::time::Instant::now();
        while self.is_active() && started.elapsed() < budget {
            self.step(graph, None);
        }
    }

    fn exact_repulsion(&self, graph: &Graph, force: &mut [Vector]) {
        let n = graph.nodes.len();
        for i in 0..n {
            if graph.nodes[i].hidden {
                continue;
            }
            for j in (i + 1)..n {
                if graph.nodes[j].hidden {
                    continue;
                }
                let mut d = graph.nodes[i].pos - graph.nodes[j].pos;
                let mut dist2 = d.x * d.x + d.y * d.y;
                if dist2 < 0.01 {
                    // Records on the same spot: nudge them apart deterministically.
                    d = Vector::new((i as f32).sin(), (j as f32).cos());
                    dist2 = 1.0;
                }
                let push = d * (REPULSION / (dist2 * dist2.sqrt()));
                force[i] = force[i] + push;
                force[j] = force[j] - push;
            }
        }
    }

    fn springs_and_move(&mut self, graph: &mut Graph, pinned: Option<usize>, mut force: Vec<Vector>) {
        // Relations act as springs.
        for e in &graph.edges {
            if !graph.edge_visible(e) || e.from == e.to {
                continue;
            }
            let d = graph.nodes[e.to].pos - graph.nodes[e.from].pos;
            let dist = (d.x * d.x + d.y * d.y).sqrt().max(0.01);
            let pull = d * (SPRING * (dist - SPRING_LENGTH) / dist);
            force[e.from] = force[e.from] + pull;
            force[e.to] = force[e.to] - pull;
        }

        for (i, node) in graph.nodes.iter_mut().enumerate() {
            if node.hidden || Some(i) == pinned {
                node.vel = Vector::ZERO;
                continue;
            }
            let f = force[i] - node.pos * GRAVITY;
            node.vel = (node.vel + f * self.alpha) * DAMPING;
            let len = (node.vel.x * node.vel.x + node.vel.y * node.vel.y).sqrt();
            if len > MAX_STEP {
                node.vel = node.vel * (MAX_STEP / len);
            }
            node.pos = node.pos + node.vel;
        }

        self.alpha *= ALPHA_DECAY;
    }
}

/// A quadtree of record positions for Barnes–Hut: each cell knows how many
/// records it holds and their centre of mass.
struct QuadTree {
    cells: Vec<Cell>,
}

struct Cell {
    center: Vector,
    half: f32,
    mass: f32,
    com: Vector,
    /// The one record in a leaf, with its position.
    body: Option<(usize, Vector)>,
    children: Option<[usize; 4]>,
}

impl Cell {
    fn new(center: Vector, half: f32) -> Cell {
        Cell { center, half, mass: 0.0, com: Vector::ZERO, body: None, children: None }
    }
}

impl QuadTree {
    fn build(points: &[(usize, Vector)]) -> QuadTree {
        let (mut lo, mut hi) = (Vector::new(f32::MAX, f32::MAX), Vector::new(f32::MIN, f32::MIN));
        for &(_, p) in points {
            lo = Vector::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Vector::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let center = (lo + hi) * 0.5;
        let half = ((hi.x - lo.x).max(hi.y - lo.y) / 2.0).max(1.0) + 1.0;
        let mut tree = QuadTree { cells: vec![Cell::new(center, half)] };
        for &(i, p) in points {
            tree.insert(0, i, p, 0);
        }
        tree
    }

    fn insert(&mut self, c: usize, i: usize, p: Vector, depth: u32) {
        let cell = &mut self.cells[c];
        let m = cell.mass;
        cell.com = (cell.com * m + p) * (1.0 / (m + 1.0));
        cell.mass = m + 1.0;
        if m == 0.0 && cell.children.is_none() {
            cell.body = Some((i, p));
            return;
        }
        if depth > 24 {
            // Records (almost) on the same spot: keep them as one heavier point.
            cell.body = None;
            return;
        }
        if cell.children.is_none() {
            let (center, half, old) = (cell.center, cell.half, cell.body.take());
            let base = self.cells.len();
            for k in 0..4 {
                let dx = if k & 1 == 1 { half / 2.0 } else { -half / 2.0 };
                let dy = if k & 2 == 2 { half / 2.0 } else { -half / 2.0 };
                self.cells.push(Cell::new(center + Vector::new(dx, dy), half / 2.0));
            }
            self.cells[c].children = Some([base, base + 1, base + 2, base + 3]);
            if let Some((o, op)) = old {
                let child = self.child_for(c, op);
                self.insert(child, o, op, depth + 1);
            }
        }
        let child = self.child_for(c, p);
        self.insert(child, i, p, depth + 1);
    }

    fn child_for(&self, c: usize, p: Vector) -> usize {
        let cell = &self.cells[c];
        let k = usize::from(p.x >= cell.center.x) | (usize::from(p.y >= cell.center.y) << 1);
        cell.children.expect("subdivided")[k]
    }

    /// The push on record `i` at `p` from all the others.
    fn repulsion(&self, i: usize, p: Vector) -> Vector {
        let mut f = Vector::ZERO;
        let mut stack = vec![0];
        while let Some(c) = stack.pop() {
            let cell = &self.cells[c];
            if cell.mass == 0.0 {
                continue;
            }
            if cell.children.is_none() && cell.body.is_some_and(|(b, _)| b == i) {
                continue;
            }
            let d = p - cell.com;
            let dist2 = d.x * d.x + d.y * d.y;
            let size = cell.half * 2.0;
            match cell.children {
                Some(children) if size * size >= THETA * THETA * dist2 => stack.extend(children),
                _ => {
                    if dist2 < 0.01 {
                        continue;
                    }
                    let dist2 = dist2.max(1.0);
                    f = f + d * (cell.mass * REPULSION / (dist2 * dist2.sqrt()));
                }
            }
        }
        f
    }
}
