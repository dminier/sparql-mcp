//! Force-directed layout — plain Rust (`bevy::math` for `Vec3` only, no
//! App/ECS/systems). The Bevy wrapper (`layout_step_system`) lives in
//! `render.rs`. Repulsion uses Barnes-Hut over an octree (`octree.rs`);
//! the exact all-pairs pass stays as the reference for tests.

use bevy::math::Vec3;
use bevy::prelude::Resource;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::graph::{EdgeStyle, NodeId, Remap, ViewerGraph};

pub mod octree;
use octree::Octree;

/// Below this many bodies per thread, the repulsion pass stays serial.
const PARALLEL_CHUNK_MIN: usize = 512;

#[derive(Debug, Clone, Copy, Resource)]
pub struct LayoutParams {
    pub repulsion: f32,
    pub spring: f32,
    pub type_edge_stiffness: f32,
    pub rest_length: f32,
    pub damping: f32,
    pub max_speed: f32,
    pub sleep_energy: f32,
    /// Barnes-Hut approximation factor — unused until Phase 4's octree.
    pub theta: f32,
}

impl Default for LayoutParams {
    fn default() -> Self {
        Self {
            repulsion: 50.0,
            spring: 2.0,
            type_edge_stiffness: 6.0,
            rest_length: 2.0,
            damping: 0.85,
            max_speed: 20.0,
            sleep_energy: 0.01,
            theta: 0.8,
        }
    }
}

#[derive(Resource)]
pub struct LayoutState {
    pub pos: Vec<Vec3>,
    vel: Vec<Vec3>,
    /// Mirrors which `ViewerGraph.nodes` slots are live, refreshed every
    /// `step` — lets the camera frame the graph without reading it.
    live: Vec<bool>,
    pub awake: bool,
    rng: SmallRng,
}

impl LayoutState {
    pub fn new(seed: u64) -> Self {
        Self {
            pos: Vec::new(),
            vel: Vec::new(),
            live: Vec::new(),
            awake: false,
            rng: SmallRng::seed_from_u64(seed),
        }
    }

    fn ensure_capacity(&mut self, len: usize) {
        while self.pos.len() < len {
            self.pos.push(Vec3::ZERO);
            self.vel.push(Vec3::ZERO);
            self.live.push(false);
        }
    }

    /// Positions of every live (placed and not removed) node.
    pub fn live_positions(&self) -> Vec<Vec3> {
        self.pos
            .iter()
            .zip(&self.live)
            .filter(|(_, live)| **live)
            .map(|(p, _)| *p)
            .collect()
    }

    fn sync_live(&mut self, graph: &ViewerGraph) {
        self.ensure_capacity(graph.nodes.len());
        for (i, slot) in graph.nodes.iter().enumerate() {
            self.live[i] = slot.is_some();
        }
    }

    fn centroid_excluding(&self, graph: &ViewerGraph, exclude: &[NodeId]) -> Vec3 {
        let mut sum = Vec3::ZERO;
        let mut count = 0u32;
        for (i, slot) in graph.nodes.iter().enumerate() {
            if slot.is_none() || i >= self.pos.len() {
                continue;
            }
            if exclude.contains(&NodeId(i as u32)) {
                continue;
            }
            sum += self.pos[i];
            count += 1;
        }
        if count == 0 {
            Vec3::ZERO
        } else {
            sum / count as f32
        }
    }

    fn jitter(&mut self) -> Vec3 {
        Vec3::new(
            self.rng.random_range(-0.5..0.5),
            self.rng.random_range(-0.5..0.5),
            self.rng.random_range(-0.5..0.5),
        )
    }

    /// Place each of `ids` (freshly added nodes) near its first connected
    /// neighbor that already has a position, or near the graph centroid
    /// if it has none. Existing positions are never touched. Wakes the
    /// simulation.
    pub fn place_new(&mut self, graph: &ViewerGraph, ids: &[NodeId]) {
        if ids.is_empty() {
            return;
        }
        self.ensure_capacity(graph.nodes.len());
        let centroid = self.centroid_excluding(graph, ids);

        for &id in ids {
            let i = id.0 as usize;
            let neighbor_pos = graph.edges.iter().find_map(|e| {
                let other = if e.from == id && !ids.contains(&e.to) {
                    Some(e.to)
                } else if e.to == id && !ids.contains(&e.from) {
                    Some(e.from)
                } else {
                    None
                };
                other
                    .filter(|n| (n.0 as usize) < self.pos.len())
                    .map(|n| self.pos[n.0 as usize])
            });
            let base = neighbor_pos.unwrap_or(centroid);
            let jitter = self.jitter();
            self.pos[i] = base + jitter;
            self.vel[i] = Vec3::ZERO;
        }
        self.sync_live(graph);
        self.awake = true;
    }

    /// Carry positions over to a rebuilt graph's new ids (same-graph
    /// snapshot). `new_len` is the new `ViewerGraph.nodes.len()`.
    pub fn remap(&mut self, remap: &Remap, new_len: usize) {
        let mut new_pos = vec![Vec3::ZERO; new_len];
        let mut new_vel = vec![Vec3::ZERO; new_len];
        for (&old_id, &new_id) in &remap.0 {
            let (old_i, new_i) = (old_id.0 as usize, new_id.0 as usize);
            if old_i < self.pos.len() && new_i < new_len {
                new_pos[new_i] = self.pos[old_i];
                new_vel[new_i] = self.vel[old_i];
            }
        }
        self.pos = new_pos;
        self.vel = new_vel;
        self.live = vec![false; new_len];
    }

    /// Exact O(n²) repulsion on every live node — the reference the
    /// Barnes-Hut pass is checked against.
    pub fn repulsion_all_pairs(&self, graph: &ViewerGraph, params: &LayoutParams) -> Vec<Vec3> {
        let n = graph.nodes.len().min(self.pos.len());
        let mut forces = vec![Vec3::ZERO; n];
        for i in 0..n {
            if graph.nodes[i].is_none() {
                continue;
            }
            for j in (i + 1)..n {
                if graph.nodes[j].is_none() {
                    continue;
                }
                let delta = self.pos[i] - self.pos[j];
                let dist = delta.length().max(0.01);
                let f = params.repulsion / (dist * dist);
                let dir = delta / dist;
                forces[i] += dir * f;
                forces[j] -= dir * f;
            }
        }
        forces
    }

    /// Barnes-Hut approximation of [`Self::repulsion_all_pairs`],
    /// O(n log n), controlled by `params.theta`.
    pub fn repulsion_barnes_hut(&self, graph: &ViewerGraph, params: &LayoutParams) -> Vec<Vec3> {
        let n = graph.nodes.len().min(self.pos.len());
        let bodies: Vec<(u32, Vec3)> = (0..n)
            .filter(|&i| graph.nodes[i].is_some())
            .map(|i| (i as u32, self.pos[i]))
            .collect();
        let tree = Octree::build(&bodies);

        // The tree is read-only and each body's force is independent, so
        // the traversal splits cleanly across cores. Small graphs stay on
        // one thread, where spawning would cost more than it saves.
        let mut per_body = vec![Vec3::ZERO; bodies.len()];
        let threads = std::thread::available_parallelism()
            .map_or(1, |t| t.get())
            .min(bodies.len().div_ceil(PARALLEL_CHUNK_MIN).max(1));
        let chunk = bodies.len().div_ceil(threads.max(1)).max(1);
        let compute = |bodies: &[(u32, Vec3)], out: &mut [Vec3]| {
            let mut stack = Vec::new();
            for (&(i, p), f) in bodies.iter().zip(out) {
                *f = tree.force_on_with(i, p, params.theta, params.repulsion, &mut stack);
            }
        };
        if threads <= 1 {
            compute(&bodies, &mut per_body);
        } else {
            std::thread::scope(|scope| {
                for (b, out) in bodies.chunks(chunk).zip(per_body.chunks_mut(chunk)) {
                    scope.spawn(move || compute(b, out));
                }
            });
        }

        let mut forces = vec![Vec3::ZERO; n];
        for (&(i, _), f) in bodies.iter().zip(per_body) {
            forces[i as usize] = f;
        }
        forces
    }

    /// One repulsion (Barnes-Hut) + spring-attraction tick. No-op while
    /// asleep. Sets `awake = false` once total kinetic energy drops below
    /// `params.sleep_energy`.
    pub fn step(&mut self, graph: &ViewerGraph, dt: f32, params: &LayoutParams) {
        self.sync_live(graph);
        if !self.awake {
            return;
        }
        let n = graph.nodes.len();
        let mut forces = self.repulsion_barnes_hut(graph, params);

        for edge in &graph.edges {
            let (i, j) = (edge.from.0 as usize, edge.to.0 as usize);
            if i >= n || j >= n {
                continue;
            }
            let delta = self.pos[j] - self.pos[i];
            let dist = delta.length().max(0.01);
            let dir = delta / dist;
            let stiffness = if edge.style == EdgeStyle::Type {
                params.type_edge_stiffness
            } else {
                params.spring
            };
            let f = stiffness * (dist - params.rest_length);
            forces[i] += dir * f;
            forces[j] -= dir * f;
        }

        let mut total_ke = 0.0f32;
        for i in 0..n {
            if graph.nodes[i].is_none() {
                continue;
            }
            self.vel[i] = (self.vel[i] + forces[i] * dt) * params.damping;
            let speed = self.vel[i].length();
            if speed > params.max_speed {
                self.vel[i] *= params.max_speed / speed;
            }
            self.pos[i] += self.vel[i] * dt;
            total_ke += self.vel[i].length_squared();
        }

        if total_ke < params.sleep_energy {
            self.awake = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Edge, Node, NodeKind};
    use std::collections::{BTreeMap, BTreeSet};

    fn bare_node(term: &str) -> Node {
        Node {
            term: term.to_string(),
            kind: NodeKind::Instance,
            label: term.to_string(),
            attributes: BTreeMap::new(),
            types: BTreeSet::new(),
            mentions: 1,
            last_touched: 0,
        }
    }

    /// A graph with `n` nodes and the given edges, bypassing
    /// `from_snapshot`/`apply_event` — layout only reads `nodes` and
    /// `edges`, so a hand-built graph is enough here.
    fn bare_graph(n: usize, edges: Vec<(u32, u32, EdgeStyle)>) -> ViewerGraph {
        let mut g = ViewerGraph::empty();
        g.nodes = (0..n)
            .map(|i| Some(bare_node(&format!("<urn:n{i}>"))))
            .collect();
        g.edges = edges
            .into_iter()
            .map(|(from, to, style)| Edge {
                from: NodeId(from),
                to: NodeId(to),
                predicate: "<urn:p>".to_string(),
                style,
            })
            .collect();
        g
    }

    #[test]
    fn two_linked_nodes_converge_toward_rest_length() {
        let graph = bare_graph(2, vec![(0, 1, EdgeStyle::Relation)]);
        let params = LayoutParams::default();
        let mut layout = LayoutState::new(1);
        layout.place_new(&graph, &[NodeId(0), NodeId(1)]);
        layout.pos[0] = Vec3::new(-10.0, 0.0, 0.0);
        layout.pos[1] = Vec3::new(10.0, 0.0, 0.0);
        layout.awake = true;

        for _ in 0..2000 {
            layout.step(&graph, 1.0 / 60.0, &params);
        }

        // Equilibrium sits somewhat past `rest_length` because repulsion
        // never reaches zero — exact tuning is deferred to Phase 4 (the
        // design's own "other defaults are tuned in Phase 4" note).
        // What matters here: starting 20 units apart, it converges to a
        // small, stable distance rather than flying apart or collapsing.
        let dist = (layout.pos[0] - layout.pos[1]).length();
        assert!(
            (1.0..6.0).contains(&dist),
            "expected a small, stable distance, got {dist}"
        );
        layout.step(&graph, 1.0 / 60.0, &params);
        let dist_after_one_more_step = (layout.pos[0] - layout.pos[1]).length();
        assert!(
            (dist - dist_after_one_more_step).abs() < 0.05,
            "expected the distance to have stabilized, got {dist} then {dist_after_one_more_step}"
        );
    }

    #[test]
    fn new_node_is_placed_near_its_neighbor() {
        let graph = bare_graph(2, vec![(0, 1, EdgeStyle::Relation)]);
        let mut layout = LayoutState::new(2);
        layout.place_new(&graph, &[NodeId(0)]);
        layout.pos[0] = Vec3::new(5.0, 5.0, 5.0);

        layout.place_new(&graph, &[NodeId(1)]);
        let dist = (layout.pos[0] - layout.pos[1]).length();
        assert!(
            dist < 1.0,
            "expected the new node close to its neighbor, got {dist}"
        );
    }

    #[test]
    fn node_with_no_neighbor_is_placed_near_centroid() {
        let graph = bare_graph(3, vec![]);
        let mut layout = LayoutState::new(3);
        layout.place_new(&graph, &[NodeId(0), NodeId(1)]);
        layout.pos[0] = Vec3::new(10.0, 0.0, 0.0);
        layout.pos[1] = Vec3::new(-10.0, 0.0, 0.0);
        let centroid = (layout.pos[0] + layout.pos[1]) / 2.0;

        layout.place_new(&graph, &[NodeId(2)]);
        let dist = (layout.pos[2] - centroid).length();
        assert!(
            dist < 1.0,
            "expected the isolated node near the centroid, got {dist}"
        );
    }

    #[test]
    fn place_new_does_not_move_existing_positions() {
        let graph = bare_graph(2, vec![]);
        let mut layout = LayoutState::new(4);
        layout.place_new(&graph, &[NodeId(0)]);
        let original = layout.pos[0];

        layout.place_new(&graph, &[NodeId(1)]);
        assert_eq!(layout.pos[0], original);
    }

    #[test]
    fn layout_falls_asleep_then_wakes_on_place_new() {
        let graph = bare_graph(1, vec![]);
        let mut layout = LayoutState::new(5);
        layout.place_new(&graph, &[NodeId(0)]);
        let params = LayoutParams {
            sleep_energy: 1e6, // guaranteed to be below this after one tick
            ..LayoutParams::default()
        };
        layout.step(&graph, 1.0 / 60.0, &params);
        assert!(!layout.awake, "expected the layout to fall asleep");

        layout.place_new(&graph, &[NodeId(0)]);
        assert!(layout.awake, "place_new must wake the simulation");
    }

    #[test]
    fn positions_are_deterministic_with_a_fixed_seed() {
        let graph = bare_graph(
            4,
            vec![(0, 1, EdgeStyle::Relation), (1, 2, EdgeStyle::Type)],
        );
        let params = LayoutParams::default();

        let run = || {
            let mut layout = LayoutState::new(42);
            layout.place_new(&graph, &[NodeId(0), NodeId(1), NodeId(2), NodeId(3)]);
            for _ in 0..50 {
                layout.step(&graph, 1.0 / 60.0, &params);
            }
            layout.pos.clone()
        };

        assert_eq!(run(), run());
    }

    /// Bare graph of `n` nodes at seeded random positions inside a cube
    /// of side `extent`, with every node live.
    fn scattered(n: usize, extent: f32, seed: u64) -> (ViewerGraph, LayoutState) {
        let graph = bare_graph(n, vec![]);
        let mut layout = LayoutState::new(seed);
        let ids: Vec<NodeId> = (0..n as u32).map(NodeId).collect();
        layout.place_new(&graph, &ids);
        let mut rng = SmallRng::seed_from_u64(seed);
        let h = extent / 2.0;
        for p in &mut layout.pos {
            *p = Vec3::new(
                rng.random_range(-h..h),
                rng.random_range(-h..h),
                rng.random_range(-h..h),
            );
        }
        (graph, layout)
    }

    #[test]
    fn barnes_hut_stays_within_5_percent_of_all_pairs() {
        let (graph, layout) = scattered(200, 40.0, 7);
        let params = LayoutParams::default();
        let exact = layout.repulsion_all_pairs(&graph, &params);
        let approx = layout.repulsion_barnes_hut(&graph, &params);

        let mean_relative_error = exact
            .iter()
            .zip(&approx)
            .map(|(e, a)| (*e - *a).length() / e.length().max(1e-6))
            .sum::<f32>()
            / exact.len() as f32;
        assert!(
            mean_relative_error < 0.05,
            "mean relative error {mean_relative_error} ≥ 5%"
        );
    }

    /// Phase 4 budget: 5,000 nodes (50 classes, 4,950 instances), 10,000
    /// edges, 100 steps, mean step ≤ 4 ms. Run with
    /// `cargo test --release -p sparql-mcp-viewer -- --ignored layout_budget_5000`.
    #[test]
    #[ignore]
    fn layout_budget_5000() {
        const CLASSES: u32 = 50;
        const INSTANCES: u32 = 4_950;
        const N: u32 = CLASSES + INSTANCES;
        let mut rng = SmallRng::seed_from_u64(42);

        let mut edges: Vec<(u32, u32, EdgeStyle)> = (CLASSES..N)
            .map(|i| (i, rng.random_range(0..CLASSES), EdgeStyle::Type))
            .collect();
        while edges.len() < 10_000 {
            let (a, b) = (rng.random_range(0..N), rng.random_range(0..N));
            if a != b {
                edges.push((a, b, EdgeStyle::Relation));
            }
        }
        let (_, mut layout) = scattered(N as usize, 60.0, 42);
        let graph = bare_graph(N as usize, edges);
        let params = LayoutParams::default();

        const STEPS: u32 = 100;
        let start = std::time::Instant::now();
        for _ in 0..STEPS {
            layout.awake = true;
            layout.step(&graph, 1.0 / 60.0, &params);
        }
        let mean_ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(STEPS);
        println!("layout_budget_5000: mean step {mean_ms:.2} ms");
        assert!(
            mean_ms <= 4.0,
            "mean step {mean_ms:.2} ms exceeds the 4 ms budget"
        );
    }
}
