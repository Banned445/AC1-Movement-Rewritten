//! Static world collision + the character proxy (RE/01 §7, RE/02 §7.1).
//!
//! The game's `CharacterController` (vftable 0x168C484, ctor 0x57B4F0) is Havok's `hkpCharacterProxy`: each frame
//! `CharacterController__Integrate` 0x57C7C0 casts its shape along the velocity (≤ 10 iterations), turns the contacts
//! into planes (0x57A6B0: the plane is pushed out by the keep distance 0.05; a walkable contact steeper than the
//! max slope, cos 45° at +176, also gets a vertical plane, 0x57A1D0), and lets the simplex solver (0x101F3F0) slide
//! the velocity along them. With the "stick to ground" flag (+127) it then casts the shape down and sets it on what
//! it finds (`CharacterController__StickToGround` 0x57D240).
//!
//! The Human's shape (`HumanGround__OnEnterInit` 0xDA7D20 → `PhysicComponent__SetHeight / SetRadius` 0x52ED20 /
//! 0x52ED40, built by 0x52E9C0): a vertical capsule of radius 0.4 m (shape 0.35 + keep distance 0.05) and height
//! 1.8 m, scaled by entity+0x7C (1 for Altaïr). On the ground (`HumanGround__OnActivateSetup` 0xDAE6E0) both flags
//! are set: the capsule is **lifted 0.37 m** (+100, `CharacterController__ApplyStepOffset` 0x579500; its height drops
//! to 1.8 − 0.37) and the stick-to-ground cast reaches **0.58 m** (+96) below the feet. So on the ground nothing lower
//! than 0.37 m touches the capsule (a step), the rounded bottom slides up edges whose contact is within 45° of vertical,
//! and the feet follow drops of up to 0.58 m. Jumps, InAir, Ledge, Climb and Walling clear the stick flag, which also
//! drops the lift: in the air the capsule spans the full 1.8 m from the feet.
//!
//! The proxy itself (manifold, planes, simplex solver, casts, stick-to-ground) is `crate::proxy`; this module holds the
//! world's boxes and the queries the contexts make of them.

use bevy::prelude::*;

use crate::tuning::*;

/// Ctor 0x57B4F0 +176: max walkable slope, cos 45°.
pub const MAX_SLOPE_COS: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// `Human__ShouldFallOffSupport` 0xB23CB0: with no contact flatter than 45°, a ray straight down of 0.8 m.
pub const OFF_SUPPORT_RAY: f32 = 0.8;

#[derive(Clone, Copy, Debug)]
pub struct Aabb3 {
    pub min: Vec3,
    pub max: Vec3,
}

#[derive(Resource)]
pub struct CollisionWorld {
    pub boxes: Vec<Aabb3>,
    pub triangles: Vec<crate::triangles::Triangle>,
    /// Each box's collision layer (`crate::layers`); boxes past the end of the list are `STATIC`, as the greybox is.
    pub layers: Vec<u8>,
    /// PORT: conservative bounds rejection, not Havok MOPP. Disable for comparison with
    /// AC_NATIVE_QUERY_CULLING=0; exact collision tests and authored geometry are unchanged.
    pub native_query_culling: bool,
}

impl Default for CollisionWorld {
    fn default() -> Self {
        Self { boxes: Vec::new(), triangles: Vec::new(), layers: Vec::new(),
            native_query_culling: std::env::var("AC_NATIVE_QUERY_CULLING").as_deref() != Ok("0") }
    }
}

impl CollisionWorld {
    /// Box `i`'s collision layer.
    pub fn layer_of(&self, i: usize) -> u8 {
        if i >= self.boxes.len() { return self.triangles[i - self.boxes.len()].layer; }
        self.layers.get(i).copied().unwrap_or(crate::layers::STATIC)
    }

    /// Does a query or shape on `layer` see box `i`? A filter word of 0 (layer 0) collides with everything (0x4DC2B0).
    pub fn sees(&self, layer: u8, i: usize) -> bool {
        layer == 0 || crate::layers::collides(layer, self.layer_of(i))
    }

    pub fn triangle_candidates(&self, min: Vec3, max: Vec3) -> impl Iterator<Item = usize> + '_ {
        self.triangles.iter().enumerate().filter(move |(_, t)| t.overlaps(min, max)).map(|(i, _)| self.boxes.len() + i)
    }

    fn triangles_in_bounds(&self, min: Vec3, max: Vec3) -> impl Iterator<Item = &crate::triangles::Triangle> {
        // Include boundary contacts despite floating-point projection/ray rounding.
        self.triangles.iter().filter(move |t| {
            let epsilon = Vec3::splat(0.0001) + (t.bounds.max - t.bounds.min) * 0.000002;
            !self.native_query_culling || t.overlaps(min - epsilon, max + epsilon)
        })
    }

    fn box_extent(axes: [Vec3; 3], half: Vec3) -> Vec3 {
        axes[0].abs() * half.x + axes[1].abs() * half.y + axes[2].abs() * half.z
    }

    /// Layer-filtered ray used by 0xE044D0 / 0x116D960.
    /// PORT: static analytic geometry replaces Havok; the player's own body is not in this world.
    pub fn ray_distance(&self, origin: Vec3, direction: Vec3, max: f32, layer: u8) -> f32 {
        let direction = direction.normalize_or_zero();
        if direction == Vec3::ZERO { return 0.0; }
        let mut nearest = max;
        for (i, b) in self.boxes.iter().enumerate() {
            if !self.sees(layer, i) { continue; }
            let (mut lo, mut hi) = (0.0f32, nearest);
            let mut hit = true;
            for axis in 0..3 {
                if direction[axis].abs() < 1e-8 {
                    if origin[axis] < b.min[axis] || origin[axis] > b.max[axis] { hit = false; break; }
                } else {
                    let a = (b.min[axis] - origin[axis]) / direction[axis];
                    let c = (b.max[axis] - origin[axis]) / direction[axis];
                    lo = lo.max(a.min(c)); hi = hi.min(a.max(c));
                    if lo > hi { hit = false; break; }
                }
            }
            if hit { nearest = nearest.min(lo); }
        }
        let end = origin + direction * nearest;
        self.triangles_in_bounds(origin.min(end), origin.max(end))
            .filter(|t| layer == 0 || crate::layers::collides(layer, t.layer))
            .filter_map(|t| t.ray(origin, direction, nearest)).fold(nearest, f32::min)
    }

    /// As `ray_distance`, with the surface normal at the hit (against the ray): the foot IK's ground ray
    /// (`FootIK__RayDown` 0x432570). None when nothing is hit within `max`.
    pub fn ray_hit(&self, origin: Vec3, direction: Vec3, max: f32, layer: u8) -> Option<(f32, Vec3)> {
        let direction = direction.normalize_or_zero();
        if direction == Vec3::ZERO { return None; }
        let mut best: Option<(f32, Vec3)> = None;
        for (i, b) in self.boxes.iter().enumerate() {
            if !self.sees(layer, i) { continue; }
            let (mut lo, mut hi, mut axis_lo) = (0.0f32, best.map_or(max, |h| h.0), usize::MAX);
            let mut hit = true;
            for axis in 0..3 {
                if direction[axis].abs() < 1e-8 {
                    if origin[axis] < b.min[axis] || origin[axis] > b.max[axis] { hit = false; break; }
                } else {
                    let a = (b.min[axis] - origin[axis]) / direction[axis];
                    let c = (b.max[axis] - origin[axis]) / direction[axis];
                    if a.min(c) > lo { lo = a.min(c); axis_lo = axis; }
                    hi = hi.min(a.max(c));
                    if lo > hi { hit = false; break; }
                }
            }
            if hit && axis_lo != usize::MAX {
                let mut n = Vec3::ZERO;
                n[axis_lo] = -direction[axis_lo].signum();
                best = Some((lo, n));
            }
        }
        let reach = best.map_or(max, |h| h.0);
        let end = origin + direction * reach;
        for t in self.triangles_in_bounds(origin.min(end), origin.max(end)).filter(|t| layer == 0 || crate::layers::collides(layer, t.layer)) {
            if let Some(d) = t.ray(origin, direction, best.map_or(max, |h| h.0)) {
                if best.is_none_or(|h| d < h.0) {
                    best = Some((d, if t.normal.dot(direction) > 0.0 { -t.normal } else { t.normal }));
                }
            }
        }
        best
    }

    /// PORT: camera obstruction ray against imported static faces; native NavigationCamera remains open.
    pub fn camera_distance(&self, origin: Vec3, direction: Vec3, max: f32) -> f32 {
        let end = origin + direction * max;
        self.triangles_in_bounds(origin.min(end),origin.max(end)).filter_map(|t|t.ray(origin,direction,max)).fold(max,f32::min)
    }
}

pub struct MoveResult {
    pub position: Vec3,
    pub velocity: Vec3,
    pub hit_wall: bool,
    pub hit_ceiling: bool,
    pub landed: bool,
}

/// What the stick-to-ground cast found under the capsule (0x57D240).
#[derive(Clone, Copy, Debug)]
pub struct Support {
    /// Feet height resting on it.
    pub y: f32,
    /// Up component of the contact normal (1 = flat under the capsule; less on a rim).
    pub normal_y: f32,
}

impl CollisionWorld {
    /// Capsule segment (sphere centres) for feet at `feet`, lifted by `lift`.
    fn segment(feet: Vec3, lift: f32) -> (Vec3, Vec3) {
        let a = feet + Vec3::Y * (lift + CAPSULE_RADIUS);
        let b = feet + Vec3::Y * (CAPSULE_HEIGHT - CAPSULE_RADIUS).max(lift + CAPSULE_RADIUS);
        (a, b)
    }

    /// Move the character from `feet` by `delta` over `dt` through the character proxy (`crate::proxy::integrate`,
    /// 0x57C7C0): the velocity delta / dt slides along what the shape touches. `grounded` = the Ground setup: the
    /// proxy floats `STEP_HEIGHT` (0.37 m) above the feet with its height cut by as much, so low obstacles pass under
    /// it; the caller then sets the feet on the support (`ground_support`).
    pub fn move_capsule(&self, proxy: &mut crate::proxy::ProxyState, feet: Vec3, delta: Vec3, grounded: bool, dt: f32) -> MoveResult {
        let lift = if grounded { STEP_HEIGHT } else { 0.0 };
        let dt = dt.max(1e-4);
        let (p, velocity, t) = crate::proxy::integrate(self, proxy, feet + Vec3::Y * lift, delta / dt, dt, lift);
        MoveResult { position: p - Vec3::Y * lift, velocity, hit_wall: t.wall, hit_ceiling: t.ceiling, landed: t.floor }
    }

    /// The stick-to-ground cast (`CharacterController__StickToGround` 0x57D240, `crate::proxy::stick_to_ground`): the
    /// lifted shape cast down by the step offset + `SNAP_DOWN`; it rests on the first surface flatter than 55° at the
    /// keep distance. On a box's rim the rounded bottom rests on the corner, lower than the top and with a tilted
    /// contact normal.
    pub fn support(&self, feet: Vec3) -> Option<Support> {
        crate::proxy::stick_to_ground(self, feet, STEP_HEIGHT, SNAP_DOWN).map(|(y, normal_y)| Support { y, normal_y })
    }

    /// Same stick/fall rule with the live shape height (crouch 0xD859D0).
    pub fn ground_support_height(&self,feet:Vec3,height:f32)->Option<Support> {
        let (y,normal_y)=crate::proxy::stick_to_ground_height(self,feet,STEP_HEIGHT,SNAP_DOWN,height)?;
        (normal_y>=crate::proxy::MAX_SLOPE_COS || self.floor_below(feet,0.8)).then_some(Support{y,normal_y})
    }

    /// Height of the floor straight below `p` within `max` (a ray, no footprint).
    pub fn floor_height_below(&self, p: Vec3, max: f32) -> Option<f32> {
        let origin = p + Vec3::Y * 0.02;
        self.boxes
            .iter()
            .filter(|b| p.x >= b.min.x && p.x <= b.max.x && p.z >= b.min.z && p.z <= b.max.z && b.max.y <= p.y + 0.02 && b.max.y >= p.y - max)
            .map(|b| b.max.y)
            .chain(self.triangles_in_bounds(origin - Vec3::Y * (max + 0.02), origin).filter_map(|t| t.ray(origin, Vec3::NEG_Y, max + 0.02).map(|d| origin.y - d)))
            .reduce(f32::max)
    }

    /// Is there floor within `max` straight below `p` (a ray, no footprint)?
    pub fn floor_below(&self, p: Vec3, max: f32) -> bool {
        self.floor_height_below(p, max).is_some()
    }

    /// Ground's support rule: the stick-to-ground support, kept while its contact is within 45° of vertical, or with a
    /// floor within 0.8 m straight below (`Human__ShouldFallOffSupport` 0xB23CB0). `None` = the character falls.
    pub fn ground_support(&self, feet: Vec3) -> Option<Support> {
        let s = self.support(feet)?;
        (s.normal_y > MAX_SLOPE_COS || self.floor_below(Vec3::new(feet.x, s.y, feet.z), OFF_SUPPORT_RAY)).then_some(s)
    }

    /// Lower the feet by up to `max` until they touch a box top (or the ground plane at y=0).
    #[allow(dead_code)]
    pub fn snap_down(&self, feet: Vec3, max: f32) -> Vec3 {
        match self.ground_height(feet, max) {
            Some(h) => Vec3::new(feet.x, h, feet.z),
            None => feet,
        }
    }

    /// Is `p` inside any solid box?
    pub fn point_inside(&self, p: Vec3) -> bool {
        if self.boxes.iter().any(|b| p.cmpge(b.min).all() && p.cmple(b.max).all()) { return true; }
        // PORT: parity ray for closed authored mesh volumes, replacing Havok's point query.
        let dir = Vec3::new(0.437, 0.731, 0.527).normalize();
        let inverse = dir.recip();
        let mut hits: Vec<f32> = self.triangles.iter().filter(|t| {
            if !self.native_query_culling { return true; }
            let epsilon = Vec3::splat(0.0001) + (t.bounds.max - t.bounds.min) * 0.000002;
            // The fixed parity ray has positive components: entry/exit intervals need no axis swap.
            let lo = (t.bounds.min - epsilon - p) * inverse;
            let hi = (t.bounds.max + epsilon - p) * inverse;
            lo.max_element().max(0.0) <= hi.min_element()
        }).filter_map(|t| t.ray(p, dir, f32::MAX)).collect();
        hits.sort_by(f32::total_cmp);
        hits.dedup_by(|a, b| (*a - *b).abs() < 0.0001);
        hits.len() % 2 == 1
    }

    /// Distance a sphere of radius `r` can travel from `origin` along `dir` before touching a box
    /// (capped at `max`). Used for the shimmy free-space sweep (ProbeLateral 0xDD9640).
    pub fn sphere_free_distance(&self, origin: Vec3, dir: Vec3, r: f32, max: f32) -> f32 {
        let step = 0.02;
        // Gather once for the entire sweep, rather than scanning the map at every 2 cm sample.
        let end = origin + dir * (max + step);
        let radius = Vec3::splat(r);
        let candidates: Vec<_> = self.triangles_in_bounds(origin.min(end) - radius, origin.max(end) + radius).collect();
        let mut d = 0.0;
        while d < max {
            let c = origin + dir * (d + step);
            if self.boxes.iter().any(|b| (c - c.clamp(b.min, b.max)).length() < r)
                || candidates.iter().any(|t| t.overlaps(c - radius, c + radius) && t.closest_point(c).distance_squared(c) < r * r) {
                return d;
            }
            d += step;
        }
        max
    }

    /// Would a standing capsule fit with its feet at `feet`?
    pub fn capsule_fits(&self, feet: Vec3) -> bool {
        let (a, c) = Self::segment(feet + Vec3::Y * 0.02, 0.0);
        let radius = Vec3::splat(CAPSULE_RADIUS * 0.95);
        !self.boxes.iter().any(|b| {
            let p = closest_segment_point_to_aabb(a, c, b);
            (p - p.clamp(b.min, b.max)).length() < CAPSULE_RADIUS * 0.95
        }) && !self.triangles_in_bounds(a.min(c) - radius, a.max(c) + radius).any(|t| {
            let (p, q) = t.segment_points(a, c);
            p.distance_squared(q) < (CAPSULE_RADIUS * 0.95).powi(2)
        })
    }

    /// Can a capsule from `bottom` up `height` (radius `r`, the bottom of the shape at `bottom`) move by `delta`
    /// without touching a box? A linear cast (`sub_B136E0`), checked in steps of at most 5 cm.
    pub fn capsule_cast_free(&self, bottom: Vec3, height: f32, r: f32, delta: Vec3) -> bool {
        let steps = ((delta.length() / 0.05).ceil() as usize).max(1);
        let a = bottom + Vec3::Y * r;
        let c = bottom + Vec3::Y * (height - r).max(r);
        let radius = Vec3::splat(r);
        let candidates: Vec<_> = self.triangles_in_bounds(a.min(c).min(a + delta).min(c + delta) - radius,
            a.max(c).max(a + delta).max(c + delta) + radius).collect();
        (1..=steps).all(|i| {
            let p = bottom + delta * (i as f32 / steps as f32);
            let (a, c) = (p + Vec3::Y * r, p + Vec3::Y * (height - r).max(r));
            !self.boxes.iter().any(|b| {
                let q = closest_segment_point_to_aabb(a, c, b);
                (q - q.clamp(b.min, b.max)).length() < r
            }) && !candidates.iter().any(|t| {
                if !t.overlaps(a.min(c) - Vec3::splat(r), a.max(c) + Vec3::splat(r)) { return false; }
                let (p, q) = t.segment_points(a, c);
                p.distance_squared(q) < r * r
            })
        })
    }

    /// Is an oriented box (centre, unit axes, half extents) clear of every box? Separating-axis test.
    pub fn obb_free(&self, centre: Vec3, axes: [Vec3; 3], half: Vec3) -> bool {
        let world = [Vec3::X, Vec3::Y, Vec3::Z];
        let extent = Self::box_extent(axes, half);
        !self.boxes.iter().any(|b| {
            let (bc, bh) = ((b.min + b.max) * 0.5, (b.max - b.min) * 0.5);
            let d = bc - centre;
            let mut tests: Vec<Vec3> = axes.to_vec();
            tests.extend(world);
            for a in axes {
                for w in world {
                    let c = a.cross(w);
                    if c.length_squared() > 1e-8 {
                        tests.push(c.normalize());
                    }
                }
            }
            tests.iter().all(|l| {
                let ra = (0..3).map(|i| half[i] * axes[i].dot(*l).abs()).sum::<f32>();
                let rb = bh.x * l.x.abs() + bh.y * l.y.abs() + bh.z * l.z.abs();
                d.dot(*l).abs() <= ra + rb
            })
        }) && !self.triangles_in_bounds(centre - extent, centre + extent).any(|t| t.intersects_box(centre, axes, half))
    }

    /// `Human__BoxQueryEmpty` 0xB2D2A0 with a contact out: the solid point inside an oriented box (centre, unit
    /// axes, half extents) lowest along `axes[k]`, or None when the box is empty. PORT: the exe takes the query's
    /// contact points; the port samples the box on a 5 cm grid.
    pub fn obb_lowest(&self, centre: Vec3, axes: [Vec3; 3], half: Vec3, k: usize) -> Option<Vec3> {
        let extent = Self::box_extent(axes, half);
        let triangle_point = self.triangles_in_bounds(centre - extent, centre + extent).flat_map(|t| t.clipped_box_points(centre, axes, half))
            .min_by(|a, b| a.dot(axes[k]).total_cmp(&b.dot(axes[k])));
        if self.boxes.is_empty() { return triangle_point; }
        let n = |h: f32| ((h * 2.0 / 0.05).ceil() as i32).max(1);
        let (nx, ny, nz) = (n(half.x), n(half.y), n(half.z));
        let mut best = triangle_point.map(|p| ((p - centre).dot(axes[k]), p));
        for i in 0..=nx {
            for j in 0..=ny {
                for l in 0..=nz {
                    let u = Vec3::new(i as f32 / nx as f32, j as f32 / ny as f32, l as f32 / nz as f32) * 2.0 - Vec3::ONE;
                    let p = centre + axes[0] * (u.x * half.x) + axes[1] * (u.y * half.y) + axes[2] * (u.z * half.z);
                    let along = (p - centre).dot(axes[k]);
                    if best.is_some_and(|(b, _)| b <= along) {
                        continue;
                    }
                    if self.boxes.iter().any(|b| p.cmpge(b.min).all() && p.cmple(b.max).all()) {
                        best = Some((along, p));
                    }
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// Height of the supporting surface under a small footprint within `max` below the feet (a query for the
    /// contexts' probes, not the proxy).
    pub fn ground_height(&self, feet: Vec3, max: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        let r = PROBE_FOOTPRINT;
        for b in &self.boxes {
            let inside = feet.x + r > b.min.x && feet.x - r < b.max.x && feet.z + r > b.min.z && feet.z - r < b.max.z;
            if !inside {
                continue;
            }
            let top = b.max.y;
            if top <= feet.y + 0.02 && top >= feet.y - max {
                best = Some(best.map_or(top, |h: f32| h.max(top)));
            }
        }
        // PORT: five footprint rays retain the greybox probe's footprint on source triangles.
        for offset in [Vec3::ZERO, Vec3::X * r, Vec3::NEG_X * r, Vec3::Z * r, Vec3::NEG_Z * r].into_iter().filter(|_| !self.triangles.is_empty()) {
            if let Some(y) = self.floor_height_below(feet + offset, max) { best = Some(best.map_or(y, |v| v.max(y))); }
        }
        best
    }
}

/// PORT: half-width of the contexts' ground queries (`ground_height`).
const PROBE_FOOTPRINT: f32 = 0.21;

fn closest_segment_point_to_aabb(a: Vec3, b: Vec3, bx: &Aabb3) -> Vec3 {
    // Ternary search along the segment for the point closest to the box (distance is convex).
    let dist = |t: f32| {
        let p = a.lerp(b, t);
        (p - p.clamp(bx.min, bx.max)).length_squared()
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let m1 = lo + (hi - lo) / 3.0;
        let m2 = hi - (hi - lo) / 3.0;
        if dist(m1) < dist(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    a.lerp(b, (lo + hi) * 0.5)
}

#[allow(dead_code)]
fn smallest_exit_axis(p: Vec3, b: &Aabb3) -> Vec3 {
    let cands = [
        (p.x - b.min.x, Vec3::NEG_X),
        (b.max.x - p.x, Vec3::X),
        (p.y - b.min.y, Vec3::NEG_Y),
        (b.max.y - p.y, Vec3::Y),
        (p.z - b.min.z, Vec3::NEG_Z),
        (b.max.z - p.z, Vec3::Z),
    ];
    cands.iter().min_by(|x, y| x.0.total_cmp(&y.0)).unwrap().1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> CollisionWorld {
        // floor slab, a 0.3 m step, a 0.45 m step, a 0.6 m block and a 2 m wall
        let b = |x0: f32, x1: f32, h: f32| Aabb3 { min: Vec3::new(x0, -1.0, -2.0), max: Vec3::new(x1, h, 2.0) };
        CollisionWorld { boxes: vec![b(-20.0, 20.0, 0.0), b(2.0, 3.0, 0.3), b(5.0, 6.0, 0.45), b(8.0, 9.0, 0.6), b(11.0, 12.0, 2.0)], ..Default::default() }
    }

    fn walk(w: &CollisionWorld, from: Vec3, to_x: f32) -> Vec3 {
        let mut f = from;
        let mut proxy = crate::proxy::ProxyState::default();
        for _ in 0..400 {
            let r = w.move_capsule(&mut proxy, f, Vec3::X * 0.05, true, 1.0 / 60.0);
            f = r.position;
            match w.ground_support(f) {
                Some(s) => f.y = s.y,
                None => break,
            }
            if f.x >= to_x {
                break;
            }
        }
        f
    }

    #[test]
    fn the_lifted_capsule_steps_over_low_obstacles_and_is_stopped_by_high_ones() {
        let w = world();
        // 0.3 m: under the 0.37 m step offset
        let f = walk(&w, Vec3::new(0.0, 0.0, 0.0), 2.5);
        assert!(f.x >= 2.5 && (f.y - 0.3).abs() < 1e-3, "0.3 m step: {f:?}");
        // 0.45 m: the rounded bottom meets the edge within 45° of vertical and slides up it
        let f = walk(&w, Vec3::new(4.0, 0.0, 0.0), 5.5);
        assert!(f.x >= 5.5 && (f.y - 0.45).abs() < 1e-3, "0.45 m step: {f:?}");
        // 0.6 m: too steep, a wall
        let f = walk(&w, Vec3::new(7.0, 0.0, 0.0), 8.5);
        assert!(f.x < 8.0 - CAPSULE_RADIUS + 0.05 && f.y.abs() < 1e-3, "0.6 m block: {f:?}");
    }

    #[test]
    fn standing_past_a_roof_edge_holds_until_the_rim_tilts_past_45_degrees() {
        let w = CollisionWorld { boxes: vec![Aabb3 { min: Vec3::new(-5.0, 0.0, -5.0), max: Vec3::new(0.0, 3.0, 5.0) }], ..Default::default() };
        let on = |x: f32| w.ground_support(Vec3::new(x, 3.0, 0.0));
        assert!((on(-0.5).unwrap().y - 3.0).abs() < 1e-5);
        // 0.2 m past: on the rim, sunk by r − √(r² − d²)
        let s = on(0.2).unwrap();
        assert!(s.y < 3.0 && s.y > 2.9, "{s:?}");
        // 0.3 m past (contact > 45°, 0.4·sin45 = 0.283): falls
        assert!(on(0.3).is_none());
    }
}
