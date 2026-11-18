//! Arrowheads for edges, as extra line segments.
//!
//! Edges are line lists (`render.rs`), so an arrowhead is a few more segments
//! in the same mesh: no extra entity, no extra material, and it follows the
//! edge's style and fade. The head is four barbs around the edge's axis, so it
//! reads from any camera angle (a flat V disappears when seen edge-on).
//!
//! The tip stops at the surface of the node it points to, not at its centre,
//! so the arrow is not hidden inside the sphere.
//!
//! When two nodes are linked in both directions, `render.rs` draws **one** line
//! with an arrowhead at each end; `edge_segments` takes that as
//! `forward && backward`.

use bevy::prelude::Vec3;

/// Length of an arrowhead, in world units.
pub const ARROW_LEN: f32 = 0.55;
/// How far the barbs spread from the axis.
pub const ARROW_HALF_WIDTH: f32 = 0.2;
/// Edges shorter than this (beyond the two node radii) get no arrowhead: it
/// would cover the whole edge.
const MIN_VISIBLE: f32 = 0.2;

/// The four barbs of an arrowhead whose tip is at `tip`, pointing along the
/// unit vector `dir`, `len` long.
fn arrowhead(tip: Vec3, dir: Vec3, len: f32, out: &mut Vec<[Vec3; 2]>) {
    let u = dir.any_orthonormal_vector();
    let v = dir.cross(u);
    let base = tip - dir * len;
    let w = ARROW_HALF_WIDTH * (len / ARROW_LEN);
    for barb in [u, -u, v, -v] {
        out.push([tip, base + barb * w]);
    }
}

/// Every line segment of the edge between `a` and `b`: the edge itself, then
/// an arrowhead at `b` when `forward` (the edge points a to b) and one at `a`
/// when `backward`. `ra` and `rb` are the node radii, so the tips touch the
/// spheres' surfaces.
pub fn edge_segments(
    a: Vec3,
    b: Vec3,
    ra: f32,
    rb: f32,
    forward: bool,
    backward: bool,
) -> Vec<[Vec3; 2]> {
    let mut out = vec![[a, b]];
    let span = b - a;
    let length = span.length();
    let free = length - ra - rb;
    if length <= f32::EPSILON || free <= MIN_VISIBLE {
        return out;
    }
    let dir = span / length;
    // Shrink the head on a short edge so the two heads of a bidirectional one
    // never overlap.
    let heads = usize::from(forward) + usize::from(backward);
    let len = ARROW_LEN.min(free / (2.0 * heads.max(1) as f32));
    if forward {
        arrowhead(b - dir * rb, dir, len, &mut out);
    }
    if backward {
        arrowhead(a + dir * ra, -dir, len, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: Vec3 = Vec3::X;

    #[test]
    fn a_one_way_edge_is_the_line_plus_four_barbs_at_the_target() {
        let (a, b) = (Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0));
        let s = edge_segments(a, b, 0.25, 0.6, true, false);
        assert_eq!(s.len(), 1 + 4);
        assert_eq!(s[0], [a, b]);
        // Every barb starts at the tip, on the target's surface.
        let tip = b - X * 0.6;
        for barb in &s[1..] {
            assert!((barb[0] - tip).length() < 1e-5);
            // …and ends behind it, towards the source, spread off the axis.
            assert!(barb[1].x < tip.x);
            assert!(barb[1].y.abs() + barb[1].z.abs() > 0.0);
        }
    }

    #[test]
    fn a_two_way_edge_is_one_line_with_a_head_at_each_end() {
        let (a, b) = (Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0));
        let s = edge_segments(a, b, 0.25, 0.25, true, true);
        assert_eq!(s.len(), 1 + 4 + 4, "one line, two heads");
        let tip_b = b - X * 0.25;
        let tip_a = a + X * 0.25;
        assert!(s[1..5].iter().all(|seg| (seg[0] - tip_b).length() < 1e-5));
        assert!(s[5..9].iter().all(|seg| (seg[0] - tip_a).length() < 1e-5));
        // The head at `a` points back towards `a`, so its barbs end to the right.
        assert!(s[5..9].iter().all(|seg| seg[1].x > tip_a.x));
    }

    #[test]
    fn the_barbs_have_the_same_length_and_form_a_cross() {
        let s = edge_segments(
            Vec3::ZERO,
            Vec3::new(0.0, 8.0, 0.0),
            0.25,
            0.25,
            true,
            false,
        );
        let lens: Vec<f32> = s[1..].iter().map(|b| (b[1] - b[0]).length()).collect();
        assert!(
            lens.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-5),
            "{lens:?}"
        );
        // Opposite barbs are mirror images about the axis.
        let off = |i: usize| s[i][1] - Vec3::new(0.0, s[i][1].y, 0.0);
        assert!((off(1) + off(2)).length() < 1e-5);
        assert!((off(3) + off(4)).length() < 1e-5);
    }

    #[test]
    fn an_edge_too_short_for_a_head_is_just_a_line() {
        // The two spheres nearly touch: no room for an arrow.
        let s = edge_segments(Vec3::ZERO, Vec3::new(0.9, 0.0, 0.0), 0.25, 0.6, true, true);
        assert_eq!(s.len(), 1);
        // Coincident nodes do not divide by zero.
        assert_eq!(
            edge_segments(Vec3::ONE, Vec3::ONE, 0.25, 0.25, true, false).len(),
            1
        );
    }

    #[test]
    fn heads_shrink_on_a_short_edge_and_never_overlap() {
        let (a, b) = (Vec3::ZERO, Vec3::new(1.6, 0.0, 0.0));
        let s = edge_segments(a, b, 0.25, 0.25, true, true);
        assert_eq!(s.len(), 9);
        // Free span is 1.1: each head gets at most a quarter of it.
        let reach = |seg: &[Vec3; 2]| (seg[1].x - seg[0].x).abs();
        assert!(s[1..].iter().all(|seg| reach(seg) <= 1.1 / 4.0 + 1e-5));
    }
}
