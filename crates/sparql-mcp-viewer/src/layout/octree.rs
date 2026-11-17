//! Barnes-Hut octree for the layout's repulsion pass: O(n log n) per step
//! instead of all-pairs O(n²). Arena-backed; rebuilt every step.

use bevy::math::Vec3;

const NONE: u32 = u32::MAX;
/// Beyond this depth, coincident (or nearly coincident) bodies share one
/// aggregate leaf instead of splitting forever.
const MAX_DEPTH: u32 = 24;

#[derive(Clone)]
struct Cell {
    center: Vec3,
    half: f32,
    mass: f32,
    com_sum: Vec3,
    children: [u32; 8],
    leaf: bool,
    /// The single body in a leaf, or `NONE` for an empty, internal or
    /// aggregate cell.
    body: u32,
    body_pos: Vec3,
}

impl Cell {
    fn new(center: Vec3, half: f32) -> Self {
        Self {
            center,
            half,
            mass: 0.0,
            com_sum: Vec3::ZERO,
            children: [NONE; 8],
            leaf: true,
            body: NONE,
            body_pos: Vec3::ZERO,
        }
    }

    fn contains(&self, p: Vec3) -> bool {
        (p - self.center).abs().max_element() <= self.half
    }
}

pub struct Octree {
    cells: Vec<Cell>,
}

impl Octree {
    /// Build over `(body index, position)` pairs.
    pub fn build(bodies: &[(u32, Vec3)]) -> Self {
        let mut tree = Self {
            cells: Vec::with_capacity(bodies.len() * 2 + 1),
        };
        if bodies.is_empty() {
            return tree;
        }
        let (min, max) = bodies.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), (_, p)| (lo.min(*p), hi.max(*p)),
        );
        let center = (min + max) / 2.0;
        let half = ((max - min).max_element() / 2.0).max(1.0) * 1.001;
        tree.cells.push(Cell::new(center, half));
        for &(idx, p) in bodies {
            tree.insert(0, idx, p, 0);
        }
        tree
    }

    fn child_for(&mut self, ci: usize, p: Vec3) -> usize {
        let c = &self.cells[ci];
        let octant = usize::from(p.x >= c.center.x)
            | usize::from(p.y >= c.center.y) << 1
            | usize::from(p.z >= c.center.z) << 2;
        if c.children[octant] == NONE {
            let quarter = c.half / 2.0;
            let offset = Vec3::new(
                if octant & 1 != 0 { quarter } else { -quarter },
                if octant & 2 != 0 { quarter } else { -quarter },
                if octant & 4 != 0 { quarter } else { -quarter },
            );
            let child = Cell::new(c.center + offset, quarter);
            self.cells.push(child);
            let new_index = (self.cells.len() - 1) as u32;
            self.cells[ci].children[octant] = new_index;
        }
        self.cells[ci].children[octant] as usize
    }

    fn insert(&mut self, ci: usize, idx: u32, p: Vec3, depth: u32) {
        let cell = &mut self.cells[ci];
        cell.mass += 1.0;
        cell.com_sum += p;

        if !cell.leaf {
            let child = self.child_for(ci, p);
            self.insert(child, idx, p, depth + 1);
            return;
        }
        if cell.mass == 1.0 {
            cell.body = idx;
            cell.body_pos = p;
            return;
        }
        if depth >= MAX_DEPTH {
            cell.body = NONE; // aggregate leaf
            return;
        }

        // Split: push the resident body and the new one down a level.
        let (old, old_pos) = (cell.body, cell.body_pos);
        cell.body = NONE;
        cell.leaf = false;
        if old != NONE {
            let child = self.child_for(ci, old_pos);
            self.insert(child, old, old_pos, depth + 1);
        }
        let child = self.child_for(ci, p);
        self.insert(child, idx, p, depth + 1);
    }

    /// Repulsive force on body `self_idx` at `p`: `repulsion · m / d²` from
    /// every other body, with cells approximated as a point mass when
    /// `size / distance < theta` and `p` lies outside them.
    pub fn force_on(&self, self_idx: u32, p: Vec3, theta: f32, repulsion: f32) -> Vec3 {
        self.force_on_with(self_idx, p, theta, repulsion, &mut Vec::new())
    }

    /// [`Self::force_on`] with a caller-owned traversal stack, so a pass
    /// over every body allocates once instead of once per body.
    pub fn force_on_with(
        &self,
        self_idx: u32,
        p: Vec3,
        theta: f32,
        repulsion: f32,
        stack: &mut Vec<u32>,
    ) -> Vec3 {
        let mut force = Vec3::ZERO;
        if self.cells.is_empty() {
            return force;
        }
        let theta_sq = theta * theta;
        stack.clear();
        stack.push(0);
        while let Some(ci) = stack.pop() {
            let c = &self.cells[ci as usize];
            if c.mass == 0.0 {
                continue;
            }
            let com = c.com_sum / c.mass;
            if c.leaf {
                if c.body == self_idx {
                    continue;
                }
                let delta = p - com;
                if c.body == NONE && delta.length_squared() < 1e-12 {
                    continue; // aggregate of points coincident with `p`
                }
                force += point_force(delta, c.mass, repulsion);
                continue;
            }
            let delta = p - com;
            let dist_sq = delta.length_squared();
            let size = 2.0 * c.half;
            // size / dist < theta, without the square root.
            if !c.contains(p) && dist_sq > 0.0 && size * size < theta_sq * dist_sq {
                force += point_force(delta, c.mass, repulsion);
            } else {
                stack.extend(c.children.iter().copied().filter(|&ch| ch != NONE));
            }
        }
        force
    }
}

fn point_force(delta: Vec3, mass: f32, repulsion: f32) -> Vec3 {
    let dist = delta.length().max(0.01);
    delta / dist * (repulsion * mass / (dist * dist))
}
