//! Static triangle queries for imported MeshShape geometry (RE/14, 0x647480 / 0x6468B0).
//! PORT: analytic triangle distances and conservative casts replace Havok's mesh agents/MOPP.
use bevy::prelude::*;
use crate::collision::Aabb3;

#[derive(Clone, Debug)]
pub struct Triangle {
    pub vertices: [Vec3; 3],
    pub bounds: Aabb3,
    pub normal: Vec3,
    pub layer: u8,
}

impl Triangle {
    pub fn new(vertices: [Vec3; 3], layer: u8) -> Option<Self> {
        let [a, b, c] = vertices;
        let n = (b - a).cross(c - a);
        if !a.is_finite() || !b.is_finite() || !c.is_finite() || n.length_squared() < 1e-14 { return None; }
        Some(Self { vertices, normal: n.normalize(), bounds: Aabb3 { min: a.min(b).min(c), max: a.max(b).max(c) }, layer })
    }

    pub fn overlaps(&self, min: Vec3, max: Vec3) -> bool {
        self.bounds.min.cmple(max).all() && self.bounds.max.cmpge(min).all()
    }

    pub fn ray(&self, origin: Vec3, direction: Vec3, max: f32) -> Option<f32> {
        let [a, b, c] = self.vertices;
        let ab = b - a;
        let ac = c - a;
        let p = direction.cross(ac);
        let det = ab.dot(p);
        if det.abs() < 1e-8 { return None; }
        let inv = det.recip();
        let s = origin - a;
        let u = s.dot(p) * inv;
        let q = s.cross(ab);
        let v = direction.dot(q) * inv;
        let t = ac.dot(q) * inv;
        (u >= -1e-6 && v >= -1e-6 && u + v <= 1.0 + 1e-6 && t >= 0.0 && t <= max).then_some(t)
    }

    pub fn closest_point(&self, p: Vec3) -> Vec3 {
        let [a, b, c] = self.vertices;
        let (ab, ac, ap) = (b - a, c - a, p - a);
        let (d1, d2) = (ab.dot(ap), ac.dot(ap));
        if d1 <= 0.0 && d2 <= 0.0 { return a; }
        let bp = p - b;
        let (d3, d4) = (ab.dot(bp), ac.dot(bp));
        if d3 >= 0.0 && d4 <= d3 { return b; }
        let vc = d1 * d4 - d3 * d2;
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 { return a + ab * (d1 / (d1 - d3)); }
        let cp = p - c;
        let (d5, d6) = (ab.dot(cp), ac.dot(cp));
        if d6 >= 0.0 && d5 <= d6 { return c; }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 { return a + ac * (d2 / (d2 - d6)); }
        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 { return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6))); }
        let inv = (va + vb + vc).recip();
        a + ab * (vb * inv) + ac * (vc * inv)
    }

    pub fn segment_points(&self, a: Vec3, b: Vec3) -> (Vec3, Vec3) {
        let delta = b - a;
        if let Some(t) = self.ray(a, delta, 1.0) { let p = a + delta * t; return (p, p); }
        let qa = self.closest_point(a);
        let qb = self.closest_point(b);
        let mut best = if a.distance_squared(qa) < b.distance_squared(qb) { (a, qa) } else { (b, qb) };
        for (c, d) in [(self.vertices[0], self.vertices[1]), (self.vertices[1], self.vertices[2]), (self.vertices[2], self.vertices[0])] {
            let pair = segment_pair(a, b, c, d);
            if pair.0.distance_squared(pair.1) < best.0.distance_squared(best.1) { best = pair; }
        }
        best
    }

    pub fn intersects_box(&self, centre: Vec3, axes: [Vec3; 3], half: Vec3) -> bool {
        let local = |p: Vec3| { let d = p - centre; Vec3::new(d.dot(axes[0]), d.dot(axes[1]), d.dot(axes[2])) };
        let [a, b, c] = self.vertices.map(local);
        let edges = [b - a, c - b, a - c];
        let mut tests = vec![Vec3::X, Vec3::Y, Vec3::Z, edges[0].cross(edges[1])];
        for edge in edges { for axis in [Vec3::X, Vec3::Y, Vec3::Z] { tests.push(edge.cross(axis)); } }
        tests.into_iter().all(|axis| {
            let dots = [a.dot(axis), b.dot(axis), c.dot(axis)];
            let lo = dots.into_iter().fold(f32::INFINITY, f32::min);
            let hi = dots.into_iter().fold(f32::NEG_INFINITY, f32::max);
            let radius = half.dot(axis.abs());
            lo <= radius && hi >= -radius
        })
    }

    pub fn clipped_box_points(&self, centre: Vec3, axes: [Vec3; 3], half: Vec3) -> Vec<Vec3> {
        if !self.intersects_box(centre, axes, half) { return Vec::new(); }
        let mut polygon = self.vertices.to_vec();
        for axis in 0..3 { for sign in [-1.0, 1.0] {
            let n = axes[axis] * sign;
            let distance = |p: Vec3| half[axis] - (p - centre).dot(n);
            let mut clipped = Vec::new();
            if polygon.is_empty() { return clipped; }
            for i in 0..polygon.len() {
                let a = polygon[i]; let b = polygon[(i + 1) % polygon.len()];
                let da = distance(a); let db = distance(b);
                if da >= 0.0 { clipped.push(a); }
                if (da >= 0.0) != (db >= 0.0) { clipped.push(a.lerp(b, da / (da - db))); }
            }
            polygon = clipped;
        } }
        polygon
    }
}

fn segment_pair(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> (Vec3, Vec3) {
    let (u, v, w) = (b - a, d - c, a - c);
    let (uu, vv, uv, uw, vw) = (u.dot(u), v.dot(v), u.dot(v), u.dot(w), v.dot(w));
    if uu < 1e-12 { return (a, c + v * (vw / vv.max(1e-12)).clamp(0.0, 1.0)); }
    if vv < 1e-12 { return (a + u * (-uw / uu).clamp(0.0, 1.0), c); }
    let den = uu * vv - uv * uv;
    let mut s = if den > 1e-12 { ((uv * vw - uw * vv) / den).clamp(0.0, 1.0) } else { 0.0 };
    let mut t = (uv * s + vw) / vv;
    if t < 0.0 { t = 0.0; s = (-uw / uu).clamp(0.0, 1.0); }
    else if t > 1.0 { t = 1.0; s = ((uv - uw) / uu).clamp(0.0, 1.0); }
    (a + u * s, c + v * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn triangle_queries_distinguish_the_surface_from_its_bounds() {
        let t = Triangle::new([Vec3::ZERO, Vec3::X * 4.0, Vec3::Z * 4.0], 1).unwrap();
        assert_eq!(t.ray(Vec3::new(1.0, 2.0, 1.0), Vec3::NEG_Y, 3.0), Some(2.0));
        assert!(t.ray(Vec3::new(3.0, 2.0, 3.0), Vec3::NEG_Y, 3.0).is_none());
        let (s, q) = t.segment_points(Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.0, 3.0, 1.0));
        assert!((s.distance(q) - 1.0).abs() < 1e-6);
        assert!(!t.intersects_box(Vec3::new(3.5, 0.0, 3.5), [Vec3::X, Vec3::Y, Vec3::Z], Vec3::splat(0.1)));
    }
}
