//! A small force-directed layout: records push each other apart, relations
//! pull their ends together, and a weak pull toward the centre keeps
//! unconnected records from drifting away.
//!
//! The simulation "cools down" (`alpha` decays) so it settles and stops;
//! dragging a record warms it up again.

use iced::Vector;

use crate::model::Graph;

const REPULSION: f32 = 6000.0;
const SPRING: f32 = 0.04;
const SPRING_LENGTH: f32 = 110.0;
const GRAVITY: f32 = 0.012;
const DAMPING: f32 = 0.6;
const MAX_STEP: f32 = 40.0;
const ALPHA_DECAY: f32 = 0.985;
const ALPHA_MIN: f32 = 0.005;

pub struct Layout {
    alpha: f32,
}

impl Layout {
    pub fn new() -> Self {
        Layout { alpha: 1.0 }
    }

    /// Whether the layout is still moving (and needs animation frames).
    pub fn is_active(&self) -> bool {
        self.alpha > ALPHA_MIN
    }

    /// Restarts movement, e.g. while a record is being dragged.
    pub fn reheat(&mut self, alpha: f32) {
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

        // Every pair of visible records repels.
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
