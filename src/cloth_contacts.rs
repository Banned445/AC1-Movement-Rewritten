//! PORT: cloth surface contacts; native vertex contacts alone miss crossing triangles (RE/09 §8.4).
use bevy::prelude::*;

#[derive(Clone, Copy)]
pub struct Contact {
    pub point: Vec3,
    pub axis: Vec3,
    pub weights: Vec3,
}

fn triangle_point(p: Vec3, v: [Vec3; 3]) -> (Vec3, Vec3) {
    let [a, b, c] = v;
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 { return (a, Vec3::X); }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 { return (b, Vec3::Y); }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let t = d1 / (d1 - d3);
        return (a + ab * t, Vec3::new(1.0 - t, t, 0.0));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 { return (c, Vec3::Z); }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let t = d2 / (d2 - d6);
        return (a + ac * t, Vec3::new(1.0 - t, 0.0, t));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let t = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * t, Vec3::new(0.0, 1.0 - t, t));
    }
    let sum = va + vb + vc;
    if sum.abs() <= 1e-12 { return (a, Vec3::X); }
    let weights = Vec3::new(va, vb, vc) / sum;
    (a * weights.x + b * weights.y + c * weights.z, weights)
}

fn segment_pair(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> (f32, f32) {
    let u = b - a;
    let v = d - c;
    let r = a - c;
    let uu = u.length_squared();
    let vv = v.length_squared();
    if uu <= 1e-12 && vv <= 1e-12 { return (0.0, 0.0); }
    if uu <= 1e-12 { return (0.0, (v.dot(r) / vv).clamp(0.0, 1.0)); }
    let ur = u.dot(r);
    if vv <= 1e-12 { return ((-ur / uu).clamp(0.0, 1.0), 0.0); }
    let uv = u.dot(v);
    let vr = v.dot(r);
    let denominator = uu * vv - uv * uv;
    let mut s = if denominator > 1e-12 { ((uv * vr - ur * vv) / denominator).clamp(0.0, 1.0) } else { 0.0 };
    let mut t = (uv * s + vr) / vv;
    if t < 0.0 { t = 0.0; s = (-ur / uu).clamp(0.0, 1.0); }
    else if t > 1.0 { t = 1.0; s = ((uv - ur) / uu).clamp(0.0, 1.0); }
    (s, t)
}

pub fn segment_triangle(a: Vec3, b: Vec3, v: [Vec3; 3]) -> Contact {
    let normal = (v[1] - v[0]).cross(v[2] - v[0]);
    let denominator = normal.dot(b - a);
    if denominator.abs() > 1e-12 {
        let t = normal.dot(v[0] - a) / denominator;
        if (0.0..=1.0).contains(&t) {
            let p = a.lerp(b, t);
            let (point, weights) = triangle_point(p, v);
            if p.distance_squared(point) <= 1e-12 { return Contact { point, axis: p, weights }; }
        }
    }
    let (point, weights) = triangle_point(a, v);
    let mut best = Contact { point, axis: a, weights };
    let (point, weights) = triangle_point(b, v);
    if point.distance_squared(b) < best.point.distance_squared(best.axis) { best = Contact { point, axis: b, weights }; }
    for i in 0..3 {
        let j = (i + 1) % 3;
        let (s, t) = segment_pair(a, b, v[i], v[j]);
        let axis = a.lerp(b, s);
        let point = v[i].lerp(v[j], t);
        if point.distance_squared(axis) < best.point.distance_squared(best.axis) {
            let mut weights = Vec3::ZERO;
            weights[i] = 1.0 - t;
            weights[j] = t;
            best = Contact { point, axis, weights };
        }
    }
    best
}

pub fn distribute(positions: &mut [Vec3], triangle: [usize; 3], pinned: &[bool], weights: Vec3, correction: Vec3) {
    let denominator: f32 = (0..3).filter(|&k| !pinned[triangle[k]]).map(|k| weights[k] * weights[k]).sum();
    if denominator <= 1e-8 { return; }
    for k in 0..3 {
        if !pinned[triangle[k]] { positions[triangle[k]] += correction * weights[k] / denominator; }
    }
}

pub fn distribute_bounded(positions: &mut [Vec3], triangle: [usize; 3], pinned: &[bool], weights: Vec3, correction: Vec3, limit: f32) {
    let denominator: f32 = (0..3).filter(|&k| !pinned[triangle[k]]).map(|k| weights[k] * weights[k]).sum();
    let maximum = (0..3).filter(|&k| !pinned[triangle[k]]).map(|k| weights[k]).fold(0.0f32, f32::max);
    if maximum <= 1e-8 { return; }
    distribute(positions, triangle, pinned, weights, correction.clamp_length_max(limit * denominator / maximum));
}

/// PORT: one bounded separation constraint across both surfaces, respecting fixed vertices.
pub fn separate_bounded(positions: &mut [Vec3], a: [usize; 3], b: [usize; 3], pinned: &[bool], aw: Vec3, bw: Vec3, correction: Vec3, limit: f32) {
    let movable = |ids: [usize; 3], weights: Vec3| (0..3).any(|k| !pinned[ids[k]] && weights[k] > 1e-8);
    let a_free = movable(a, aw);
    let b_free = movable(b, bw);
    // Keep the existing equal split when both sides move; transfer the fixed side's share.
    let share = if a_free && b_free { 0.5 } else { 1.0 };
    if a_free { distribute_bounded(positions, a, pinned, aw, correction * share, limit); }
    if b_free { distribute_bounded(positions, b, pinned, bw, -correction * share, limit); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separation_uses_only_movable_surface_weights() {
        for pinned in [[false; 6], [true, true, true, false, false, false], [false, false, false, true, true, true]] {
            let mut p = vec![Vec3::ZERO; 6];
            separate_bounded(&mut p, [0, 1, 2], [3, 4, 5], &pinned, Vec3::X, Vec3::splat(1.0 / 3.0), Vec3::Y * 0.008, 0.01);
            let separation = p[0] - (p[3] + p[4] + p[5]) / 3.0;
            assert!((separation.y - 0.008).abs() < 1e-6);
            assert!((0..6).all(|i| !pinned[i] || p[i] == Vec3::ZERO));
        }
        let mut p = vec![Vec3::ZERO; 6];
        separate_bounded(&mut p, [0, 1, 2], [3, 4, 5], &[false; 6], Vec3::X, Vec3::splat(1.0 / 3.0), Vec3::Y, 0.01);
        assert!(p.iter().all(|v| v.length() <= 0.010001));
    }
    #[test]
    fn detects_segment_crossing_triangle_with_all_vertices_outside() {
        let triangle = [Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0), Vec3::new(0.0, 0.0, 1.0)];
        let c = segment_triangle(-Vec3::Y, Vec3::Y, triangle);
        assert!(c.point.distance(c.axis) < 1e-6);
        assert!((c.weights.element_sum() - 1.0).abs() < 1e-6);
        let mut p = triangle.to_vec();
        distribute(&mut p, [0, 1, 2], &[true, false, false], c.weights, Vec3::Y * 0.2);
        assert_eq!(p[0], triangle[0]);
        let moved = p[0] * c.weights.x + p[1] * c.weights.y + p[2] * c.weights.z;
        assert!((moved.y - 0.2).abs() < 1e-6);
    }
    #[test]
    fn closest_contact_is_finite_for_degenerate_and_parallel_geometry() {
        for tri in [[Vec3::ZERO; 3], [Vec3::ZERO, Vec3::X, Vec3::X * 2.0]] {
            let c = segment_triangle(Vec3::Y, Vec3::Y + Vec3::X, tri);
            assert!(c.point.is_finite() && c.axis.is_finite() && c.weights.is_finite());
        }
    }
}
