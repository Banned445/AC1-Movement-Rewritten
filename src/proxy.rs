//! The character proxy, as the game runs it (RE/01 §7.2). The game's `CharacterController` is Havok's
//! `hkpCharacterProxy` with the cinfo of `CharacterController__ctor` 0x57B4F0; this is a reimplementation of its
//! integrate (0x57C7C0), manifold update (0x57C150), contact → plane rules (0x57A6B0 / 0x57A1D0), simplex solver
//! (0x101F3F0 with the 1/2/3-plane solves 0x101E520 / 0x101E8A0 / 0x101ED00) and stick-to-ground cast (0x57D240),
//! on the port's static boxes.
//!
//! Each frame the proxy gets a velocity (the context's wanted displacement / dt; `sub_52F150` builds it), then up to
//! 10 rounds of:
//! 1. closest points around the shape and a cast along the planned displacement;
//! 2. the manifold is refreshed from them (persistent across frames);
//! 3. every manifold contact becomes a plane (minus the keep distance; penetration adds a push-out velocity), and a
//!    walkable-facing contact steeper than 45° adds a vertical plane;
//! 4. the simplex solver moves through the planes in time, sliding the velocity along those it hits;
//! 5. when the solved displacement differs from the planned one, it is cast; a hit that is new beyond the first one
//!    stops the move at it.

use bevy::prelude::*;

use crate::collision::{Aabb3, CollisionWorld};
use crate::tuning::{CAPSULE_HEIGHT, CAPSULE_RADIUS};

/// Ctor 0x57B4F0 +204: keep distance (the shape is this much smaller than the capsule radius, and planes are pushed
/// out by it).
pub const KEEP_DISTANCE: f32 = 0.05;
/// +176: walkable slope, cos 45°.
pub const MAX_SLOPE_COS: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// +188: dynamic friction.
const DYNAMIC_FRICTION: f32 = 1.0;
/// +212: contact angle sensitivity (manifold metric 0x579330).
const ANGLE_SENSITIVITY: f32 = 1.0;
/// +216: the solver's max surface velocity.
const MAX_SURFACE_VELOCITY: f32 = 10.0;
/// +224: penetration recovery speed.
const PENETRATION_RECOVERY: f32 = 1.0;
/// Integrate 0x57C7C0: at most 10 cast rounds per frame.
const MAX_ITERATIONS: usize = 10;
/// PORT (hypothesis): the closest-point tolerance of the shape queries (the collision input's tolerance, read at
/// 0x57C8A6 from the world; Havok's default 0.1).
const COLLISION_TOLERANCE: f32 = 0.1;
/// StickToGround's collector (`MinAngleCdPointCollector` add 0x57AA00) keeps only hits whose normal points up at
/// least this much (dword_18D81A0 = cos 55°).
const STICK_MIN_UP: f32 = 0.573_576_4;
const EPS: f32 = 1.192_092_9e-7;

/// A manifold / query contact (Havok root CD point: position, normal, distance, the body).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub pos: Vec3,
    /// From the body toward the shape.
    pub normal: Vec3,
    /// Shape surface to body surface (negative: penetrating). For a cast hit: the distance along the normal at the
    /// hit fraction (the collector's conversion, 0x579870).
    pub dist: f32,
    /// Cast hits: the hit fraction of the cast.
    pub fraction: f32,
    pub body: usize,
}

/// The proxy's persistent state (controller +232: the manifold; +0x68: the collision filter word, of which the port
/// keeps the layer, set by the contexts through `Human__CharacterLayer` 0xB0EAA0; 0 = collide with everything).
#[derive(Clone, Debug, Default)]
pub struct ProxyState {
    pub manifold: Vec<Contact>,
    pub layer: u8,
}

/// One solver plane (64 bytes: normal + distance, velocity, frictions, priority).
#[derive(Clone, Copy, Debug)]
struct Plane {
    n: Vec3,
    d: f32,
    v: Vec3,
    static_f: f32,
    extra_up: f32,
    extra_down: f32,
    dynamic_f: f32,
    priority: i32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Interaction {
    surface_time: f32,
    penalty: f32,
    status: i32,
}

/// The capsule for a proxy at `p` (`CharacterController__RebuildCapsule` 0x52E9C0): vertex A r above the position,
/// B at the height (minus the lift when the step offset is on) − r, radius r − keep.
fn shape(p: Vec3, lift: f32) -> (Vec3, Vec3, f32) {
    let r = CAPSULE_RADIUS;
    let span = ((CAPSULE_HEIGHT - lift) - 2.0 * r).max(0.001);
    let a = p + Vec3::Y * r;
    (a, a + Vec3::Y * span, r - KEEP_DISTANCE)
}

fn closest_on_segment_to_box(a: Vec3, b: Vec3, bx: &Aabb3) -> Vec3 {
    let dist = |t: f32| {
        let p = a.lerp(b, t);
        (p - p.clamp(bx.min, bx.max)).length_squared()
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..30 {
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

/// Closest point between the shape at `p` and box `i`.
fn closest(world: &CollisionWorld, i: usize, p: Vec3, lift: f32) -> Contact {
    if i >= world.boxes.len() {
        let t = &world.triangles[i - world.boxes.len()];
        let (a, b, rs) = shape(p, lift);
        let (s, q) = t.segment_points(a, b);
        let d = s - q;
        let len = d.length();
        let normal = if len > 1e-5 { d / len } else if t.normal.dot(p - t.vertices[0]) >= 0.0 { t.normal } else { -t.normal };
        return Contact { pos: q, normal, dist: len - rs, fraction: 0.0, body: i };
    }
    let bx = &world.boxes[i];
    let (a, b, rs) = shape(p, lift);
    let s = closest_on_segment_to_box(a, b, bx);
    let q = s.clamp(bx.min, bx.max);
    let d = s - q;
    let len = d.length();
    if len > 1e-5 {
        Contact { pos: q, normal: d / len, dist: len - rs, fraction: 0.0, body: i }
    } else {
        // the segment is inside the box: out along the nearest face
        let c = [
            (s.x - bx.min.x, Vec3::NEG_X),
            (bx.max.x - s.x, Vec3::X),
            (s.y - bx.min.y, Vec3::NEG_Y),
            (bx.max.y - s.y, Vec3::Y),
            (s.z - bx.min.z, Vec3::NEG_Z),
            (bx.max.z - s.z, Vec3::Z),
        ];
        let (depth, n) = c.into_iter().min_by(|x, y| x.0.total_cmp(&y.0)).unwrap();
        Contact { pos: s + n * depth, normal: n, dist: -depth - rs, fraction: 0.0, body: i }
    }
}

/// The closest-point query of the shape (the start collector of 0x57B200): every box within the tolerance.
fn closest_points(world: &CollisionWorld, layer: u8, p: Vec3, lift: f32) -> Vec<Contact> {
    let (a, b, r) = shape(p, lift);
    let margin = Vec3::splat(r + COLLISION_TOLERANCE);
    (0..world.boxes.len()).chain(world.triangle_candidates(a.min(b) - margin, a.max(b) + margin))
        .filter(|&i| world.sees(layer, i)).map(|i| closest(world, i, p, lift)).filter(|c| c.dist < COLLISION_TOLERANCE).collect()
}

/// The linear cast of the shape from `p` by `disp` (0x57B200 with the cast collector), hits sorted by fraction
/// (sub_FE4FC0). Each hit carries the fraction and the distance along its normal (−disp·n × fraction, 0x579870).
/// PORT: conservative advancement against each convex box/triangle; shapes already touching at the start are left to the
/// closest-point query.
fn linear_cast(world: &CollisionWorld, layer: u8, p: Vec3, disp: Vec3, lift: f32) -> Vec<Contact> {
    let len = disp.length();
    let mut hits = Vec::new();
    if len < 1e-7 {
        return hits;
    }
    let (a, b, r) = shape(p, lift);
    let margin = Vec3::splat(r + COLLISION_TOLERANCE);
    let min = a.min(b).min(a + disp).min(b + disp) - margin;
    let max = a.max(b).max(a + disp).max(b + disp) + margin;
    for i in (0..world.boxes.len()).chain(world.triangle_candidates(min, max)).filter(|&i| world.sees(layer, i)) {
        let mut t = 0.0f32;
        let mut c = closest(world, i, p, lift);
        if c.dist <= 0.0 {
            continue;
        }
        // If the budget expires, retain the last conservative fraction instead of declaring the path clear.
        let mut hit = true;
        for _ in 0..40 {
            if c.dist <= 1e-4 {
                hit = true;
                break;
            }
            // The closest normal defines a supporting plane of this convex collider. Advance to that plane,
            // not by total motion length: tangential motion must not slow convergence at shallow incidence.
            let closing_speed = -disp.dot(c.normal);
            if closing_speed <= 0.0 {
                hit = false;
                break;
            }
            t += c.dist / closing_speed;
            if t > 1.0 {
                hit = false;
                break;
            }
            c = closest(world, i, p + disp * t, lift);
        }
        if hit {
            c.fraction = t;
            c.dist = -disp.dot(c.normal) * t;
            hits.push(c);
        }
    }
    hits.sort_by(|a, b| a.fraction.total_cmp(&b.fraction));
    hits
}

/// `sub_579330`: how different two contacts are (velocities are 0 on the static world).
fn metric(a: &Contact, b: &Contact) -> f32 {
    let angle = (1.0 - a.normal.dot(b.normal)) * ANGLE_SENSITIVITY * ANGLE_SENSITIVITY;
    angle * 10.0 + (a.dist - b.dist) * (a.dist - b.dist)
}

/// `sub_579480`: the manifold contact closest to `c` under 0.1.
fn find(manifold: &[Contact], c: &Contact) -> Option<usize> {
    let mut best = (0.1f32, None);
    for (i, m) in manifold.iter().enumerate() {
        let d = metric(c, m);
        if best.0 > d {
            best = (d, Some(i));
        }
    }
    best.1
}

/// `hkpCharacterProxy::updateManifold` 0x57C150.
fn update_manifold(manifold: &mut Vec<Contact>, start: &[Contact], cast: &[Contact]) {
    let mut start = start.to_vec();
    let min_d = start.iter().map(|c| c.dist).fold(f32::MAX, f32::min);
    // every manifold contact is replaced by its nearest start point (metric < 1.1), or dropped
    for i in (0..manifold.len()).rev() {
        let mut best = (1.1f32, None);
        for (j, s) in start.iter().enumerate() {
            let d = metric(s, &manifold[i]);
            if best.0 > d {
                best = (d, Some(j));
            }
        }
        match best.1 {
            None => {
                manifold.swap_remove(i);
            }
            Some(j) => {
                manifold[i] = start[j];
                start.swap_remove(j);
            }
        }
    }
    // new contacts: only the closest start points
    for s in &start {
        if s.dist == min_d && find(manifold, s).is_none() {
            manifold.push(*s);
        }
    }
    if let Some(c) = cast.first() {
        if find(manifold, c).is_none() {
            manifold.push(*c);
        }
    }
    // duplicates
    for i in (1..manifold.len()).rev() {
        if (0..i).rev().any(|j| metric(&manifold[i], &manifold[j]) < 0.1) {
            manifold.swap_remove(i);
        }
    }
}

/// Contact → plane (`CharacterController__ContactToSurfaceConstraint` 0x57A6B0) and the max-slope plane (0x57A1D0).
/// `moving`: the controller's static and extra up / down friction are 0 while it has a velocity, 1 when it has none
/// (`sub_52F150`).
fn planes_from(manifold: &[Contact], moving: bool) -> Vec<Plane> {
    let still = if moving { 0.0 } else { 1.0 };
    let mut planes = Vec::with_capacity(manifold.len() * 2);
    for c in manifold {
        let mut p = Plane {
            n: c.normal,
            d: c.dist - KEEP_DISTANCE,
            v: Vec3::ZERO,
            static_f: still,
            extra_up: still,
            extra_down: still,
            dynamic_f: DYNAMIC_FRICTION,
            priority: 0,
        };
        if p.d < -EPS {
            // penetration: push out at the recovery speed
            p.v += p.n * (PENETRATION_RECOVERY * -p.d);
            p.d = 0.0;
        }
        planes.push(p);
        let up = p.n.dot(Vec3::Y);
        if up > 0.01 && up < MAX_SLOPE_COS {
            // a walkable-facing surface steeper than 45° also stops the character like a wall
            let h = p.n - Vec3::Y * up;
            let inv = 1.0 / h.length();
            planes.push(Plane { n: h * inv, d: p.d * inv, ..p });
        }
    }
    planes
}

/// `sub_101E4C0`: does `vel` move into plane `p`?
fn violates(p: &Plane, vel: Vec3) -> bool {
    (vel - p.v).dot(p.n) < -0.001
}

/// `sub_101E520`: the velocity `vin` against one plane, with its frictions.
fn solve1(p: &Plane, vin: Vec3, up: Vec3) -> Vec3 {
    let rel = vin - p.v;
    let vn = p.n.dot(rel);
    let mut t = rel - p.n * vn;
    let (vn2, rel2) = (vn * vn, rel.length_squared());
    let extra = if up.dot(t) <= 0.0 { p.extra_down } else { p.extra_up };
    let s = p.static_f;
    let dynamic = |t: Vec3| {
        let t2 = t.length_squared();
        if p.dynamic_f >= 1.0 || t2 < EPS || rel2 * 0.001 >= t2 {
            p.v + t
        } else {
            let k = (rel2 / t2).sqrt() * (1.0 - p.dynamic_f) + p.dynamic_f;
            let tt = t * k;
            p.v + tt - p.n * p.n.dot(tt)
        }
    };
    if extra <= 0.0 {
        return if rel2 > (s * s + 1.0) * vn2 { dynamic(t) } else { p.v };
    }
    let mut h = 0.0;
    let side = up.cross(p.n);
    let side = if side.length_squared() > EPS {
        let side = side.normalize();
        h = side.dot(t);
        if h * h <= s * s * vn2 {
            t -= side * h;
            h = 0.0;
        }
        side
    } else {
        Vec3::ZERO
    };
    if rel2 - h * h - vn2 > (extra + s) * (extra + s) * vn2 {
        dynamic(t)
    } else if h == 0.0 {
        p.v
    } else {
        dynamic(side * h)
    }
}

/// The velocity on the line where two planes meet that satisfies both planes' velocities.
fn line_velocity(a: &Plane, b: &Plane, axis: Vec3) -> Option<Vec3> {
    let m = Mat3::from_cols(a.n, b.n, axis).transpose();
    if m.determinant().abs() < EPS {
        return None;
    }
    Some(m.inverse() * Vec3::new(a.v.dot(a.n), b.v.dot(b.n), 0.5 * (a.v + b.v).dot(axis)))
}

/// `sub_101E8A0`: the velocity against two planes (slide along their intersection line).
fn solve2(a: &Plane, b: &Plane, vin: Vec3, up: Vec3, inter: &mut [Interaction], ia: usize, ib: usize) -> Vec3 {
    let cross = b.n.cross(a.n);
    let base = (cross.length_squared() > EPS).then(|| cross.normalize()).and_then(|axis| line_velocity(a, b, axis).map(|u| (axis, u)));
    let Some((axis, u)) = base.filter(|(_, u)| u.abs().max_element() <= MAX_SURFACE_VELOCITY) else {
        // parallel planes (or a line moving too fast): one plane after the other, lower priority first
        inter[ia].status = 2;
        inter[ib].status = 2;
        let (first, second) = if a.priority <= b.priority { (a, b) } else { (b, a) };
        return solve1(second, solve1(first, vin, up), up);
    };
    let rel = vin - u;
    let rel2 = rel.length_squared();
    let ua = up.dot(axis);
    let mut along = rel.dot(axis);
    let extra = if along * ua <= 0.0 { a.extra_down + b.extra_down } else { a.extra_up + b.extra_up };
    let k = (ua * extra + a.static_f + b.static_f) * 0.5;
    let dynf = 0.5 * (a.dynamic_f + b.dynamic_f);
    if (rel2 - along * along) * k * k < along * along {
        if dynf < 1.0 && 0.001 * rel2 < along * along {
            along *= rel2.sqrt() * (1.0 / along).abs() * (1.0 - dynf) + dynf;
        }
        u + axis * along
    } else {
        u
    }
}

/// `sub_101ED00`: the velocity against three planes (their common point's velocity).
fn solve3(planes: [&Plane; 3], idx: [usize; 3], vin: Vec3, up: Vec3, inter: &mut [Interaction]) -> Vec3 {
    let m = Mat3::from_cols(planes[0].n, planes[1].n, planes[2].n).transpose();
    let u = (m.determinant().abs() >= EPS)
        .then(|| m.inverse() * Vec3::new(planes[0].v.dot(planes[0].n), planes[1].v.dot(planes[1].n), planes[2].v.dot(planes[2].n)))
        .filter(|u| u.abs().max_element() <= MAX_SURFACE_VELOCITY);
    if let Some(u) = u {
        return u;
    }
    for &i in &idx {
        inter[i].status = 1;
    }
    let v = solve2(planes[0], planes[1], vin, up, inter, idx[0], idx[1]);
    let v = solve2(planes[0], planes[2], v, up, inter, idx[0], idx[2]);
    solve2(planes[1], planes[2], v, up, inter, idx[1], idx[2])
}

/// `sub_101EFC0`: the output velocity for the active planes (newest last), dropping planes that no longer constrain it.
fn solve_active(active: &mut Vec<usize>, planes: &[Plane], vin: Vec3, up: Vec3, inter: &mut [Interaction]) -> Vec3 {
    loop {
        match active.len() {
            1 => return solve1(&planes[active[0]], vin, up),
            2 => {
                let (p0, p1) = (active[0], active[1]);
                let v = solve1(&planes[p1], vin, up);
                if violates(&planes[p0], v) {
                    return solve2(&planes[p0], &planes[p1], vin, up, inter, p0, p1);
                }
                active.remove(0);
                return v;
            }
            3 => {
                let (p0, p1, p2) = (active[0], active[1], active[2]);
                let v = solve1(&planes[p2], vin, up);
                if !violates(&planes[p0], v) && !violates(&planes[p1], v) {
                    *active = vec![p2];
                    return v;
                }
                // a pair with the newest plane that keeps clear of the third
                let mut reduced = false;
                for (keep, other) in [(p0, p1), (p1, p0)] {
                    let v = solve2(&planes[keep], &planes[p2], vin, up, inter, keep, p2);
                    if !violates(&planes[other], v) {
                        *active = vec![keep, p2];
                        reduced = true;
                        break;
                    }
                }
                if reduced {
                    continue;
                }
                return solve3([&planes[p0], &planes[p1], &planes[p2]], [p0, p1, p2], vin, up, inter);
            }
            4 => {
                // PORT: the four-plane case keeps the three highest-priority planes (stable order: newest last)
                active.sort_by_key(|&i| planes[i].priority);
                for i in 0..3 {
                    let tri = [active[(i + 1) % 3], active[(i + 2) % 3], active[3]];
                    let v = solve3([&planes[tri[0]], &planes[tri[1]], &planes[tri[2]]], tri, vin, up, inter);
                    if !violates(&planes[active[i]], v) {
                        active[i] = active[2];
                        active[2] = active[3];
                        active.truncate(3);
                        break;
                    }
                }
                if active.len() == 4 {
                    let worst = (0..4).max_by_key(|&i| inter[active[i]].status).unwrap_or(0);
                    let v = solve3([&planes[active[0]], &planes[active[1]], &planes[active[3]]], [active[0], active[1], active[3]], vin, up, inter);
                    active.swap_remove(worst);
                    for &i in active.iter() {
                        inter[i].status = 0;
                    }
                    return v;
                }
            }
            _ => return vin,
        }
    }
}

/// `hkSimplexSolverSolve` 0x101F3F0: move from 0 with `vel` for `dt`, stopping at each plane in time, re-solving the
/// velocity against the planes touched. Returns (displacement, velocity, time used).
fn simplex_solve(planes: &[Plane], vel: Vec3, up: Vec3, dt: f32, min_dt: f32) -> (Vec3, Vec3, f32) {
    let mut inter = vec![Interaction::default(); planes.len()];
    let (mut pos, mut v) = (Vec3::ZERO, vel);
    let (mut time, mut remaining) = (0.0f32, dt);
    let mut active: Vec<usize> = Vec::new();
    let mut used = dt;
    while remaining >= 0.0 {
        let mut min_t = remaining;
        let mut hit = None;
        for (i, p) in planes.iter().enumerate() {
            if active.iter().take(3).any(|&a| a == i) || inter[i].status != 0 {
                continue;
            }
            let approach = -(v - p.v).dot(p.n);
            if approach > 0.0 {
                let mut dist = (pos - p.v * time).dot(p.n) + p.d;
                if dist <= EPS {
                    dist = 0.0;
                }
                dist += inter[i].penalty;
                if approach * min_t > dist {
                    hit = Some(i);
                    min_t = dist / approach;
                }
            }
        }
        if min_t > 1e-4 {
            time += min_t;
            pos += v * min_t;
            remaining -= min_t;
            for &a in &active {
                inter[a].surface_time += min_t;
            }
            used = time;
            if min_dt < time {
                return (pos, v, used);
            }
        }
        let Some(h) = hit else {
            return (pos, v, dt);
        };
        active.push(h);
        inter[h].penalty = (inter[h].penalty + EPS) * 2.0;
        v = solve_active(&mut active, planes, vel, up, &mut inter);
    }
    (pos, v, used)
}

/// What the proxy touched after a move (for the contexts' wall / ceiling / floor tests).
#[derive(Clone, Copy, Debug, Default)]
pub struct Touched {
    pub floor: bool,
    pub wall: bool,
    pub ceiling: bool,
}

/// `CharacterController__Integrate` 0x57C7C0: integrate the proxy at `p` (the shape's base; lifted by `lift` on the
/// ground) with `vel` over `dt`. Returns the new position and velocity.
pub fn integrate(world: &CollisionWorld, state: &mut ProxyState, p: Vec3, vel: Vec3, dt: f32, lift: f32) -> (Vec3, Vec3, Touched) {
    let up = Vec3::Y;
    let mut pos = p;
    let mut remaining = dt;
    let mut planned = vel * dt;
    let speed = vel.length();
    let min_dt = if speed > 0.0 { KEEP_DISTANCE / speed * 0.5 } else { f32::INFINITY };
    let moving = speed > 0.0;
    let mut out_vel = vel;
    if dt > EPS {
        for _ in 0..MAX_ITERATIONS {
            let start = closest_points(world, state.layer, pos, lift);
            let cast = linear_cast(world, state.layer, pos, planned, lift);
            update_manifold(&mut state.manifold, &start, &cast);
            let planes = planes_from(&state.manifold, moving);
            let (disp, v, used) = simplex_solve(&planes, vel, up, remaining, min_dt);
            out_vel = v;
            let mut moved = false;
            if (disp - planned).abs().max_element() > 0.001 {
                let hits = linear_cast(world, state.layer, pos, disp, lift);
                if let Some(first) = hits.first() {
                    if find(&state.manifold, first).is_none() {
                        state.manifold.push(*first);
                    }
                    // the hits already in the manifold are skipped; a new one stops the move at it (0x5791A0)
                    if let Some(h) = hits.iter().find(|h| find(&state.manifold, h).is_none()) {
                        let len = disp.length();
                        if len > 0.0 {
                            let cosv = h.normal.dot(disp) / len;
                            let back = KEEP_DISTANCE / -cosv;
                            let f = (h.fraction - back / len).clamp(0.0, 1.0);
                            pos += disp * f;
                            remaining -= used * f;
                        }
                        moved = true;
                    }
                }
            }
            if !moved {
                pos += disp;
                remaining -= used;
            }
            planned = disp;
            if remaining <= EPS {
                break;
            }
        }
    }
    // what is touching now (within a centimetre of the keep distance)
    let mut t = Touched::default();
    for c in closest_points(world, state.layer, pos, lift) {
        if c.dist - KEEP_DISTANCE > 0.01 {
            continue;
        }
        if c.normal.y >= MAX_SLOPE_COS {
            t.floor = true;
        } else if c.normal.y < -0.5 {
            t.ceiling = true;
        } else {
            t.wall = true;
        }
    }
    (pos, out_vel, t)
}

/// `CharacterController__StickToGround` 0x57D240 for feet at `feet` on the ground (step offset on): the shape (lifted
/// by `lift`) is cast down by `reach + lift`; on the first hit flatter than 55° the shape rests on it at the keep
/// distance along its normal, and the feet are there. Returns (feet height, the hit normal's up component).
pub fn stick_to_ground(world: &CollisionWorld, feet: Vec3, lift: f32, reach: f32) -> Option<(f32, f32)> {
    let p = feet + Vec3::Y * lift;
    let cast = Vec3::NEG_Y * (reach + lift);
    // the Ground context's own cast: the character's layer (MainCharacter sees Static)
    let hits = linear_cast(world, crate::layers::MAIN_CHARACTER, p, cast, lift);
    let h = hits.iter().find(|h| h.normal.y >= STICK_MIN_UP)?;
    let rest = p + cast * h.fraction + Vec3::Y * (KEEP_DISTANCE / h.normal.y);
    Some((rest.y, h.normal.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxes(b: &[(Vec3, Vec3)]) -> CollisionWorld {
        CollisionWorld { boxes: b.iter().map(|&(min, max)| Aabb3 { min, max }).collect(), ..Default::default() }
    }

    fn run(w: &CollisionWorld, mut p: Vec3, vel: Vec3, frames: usize, lift: f32) -> (Vec3, Vec3) {
        let mut s = ProxyState::default();
        let mut v = vel;
        for _ in 0..frames {
            let (np, nv, _) = integrate(w, &mut s, p, vel, 1.0 / 60.0, lift);
            p = np;
            v = nv;
        }
        (p, v)
    }

    #[test]
    fn review_shallow_incidence_cast_detects_the_wall() {
        let w = boxes(&[(Vec3::new(0.451, -2.0, -50.0), Vec3::new(1.451, 4.0, 50.0))]);
        let disp = Vec3::new(0.102, 0.0, 0.75);
        let hits = linear_cast(&w, 0, Vec3::ZERO, disp, 0.0);
        assert_eq!(hits.len(), 1, "capsule crosses the wall at 0.101 / 0.102 but cast returned {hits:?}");
        assert!((hits[0].fraction - 0.101 / 0.102).abs() < 0.002, "{hits:?}");
        assert!(hits[0].normal.dot(Vec3::NEG_X) > 0.999);
    }

    #[test]
    fn review_shallow_incidence_cast_detects_a_triangle() {
        let tri = crate::triangles::Triangle::new([
            Vec3::new(0.451, -10.0, -10.0),
            Vec3::new(0.451, 10.0, -10.0),
            Vec3::new(0.451, 0.0, 10.0),
        ], crate::layers::STATIC).unwrap();
        let w = CollisionWorld { triangles: vec![tri], ..Default::default() };
        let hits = linear_cast(&w, 0, Vec3::ZERO, Vec3::new(0.102, 0.0, 0.75), 0.0);
        assert_eq!(hits.len(), 1, "shallow triangle crossing was missed: {hits:?}");
        assert!((hits[0].fraction - 0.101 / 0.102).abs() < 0.002, "{hits:?}");
        assert!(hits[0].normal.dot(Vec3::NEG_X) > 0.999);
    }

    #[test]
    fn review_cast_clear_paths_and_layers_remain_clear() {
        let w = boxes(&[(Vec3::new(0.451, -2.0, -50.0), Vec3::new(1.451, 4.0, 50.0))]);
        for disp in [Vec3::new(0.1, 0.0, 0.75), Vec3::new(-0.102, 0.0, 0.75), Vec3::new(0.0, 0.0, 0.75)] {
            assert!(linear_cast(&w, 0, Vec3::ZERO, disp, 0.0).is_empty(), "false contact for {disp:?}");
        }
        assert!(linear_cast(&w, crate::layers::MAIN_CHARACTER_NO_STATIC, Vec3::ZERO, Vec3::new(0.102, 0.0, 0.75), 0.0).is_empty());
    }

    #[test]
    fn a_wall_at_45_degrees_keeps_the_tangential_speed() {
        // wall face at x = 1 (normal -X); push at 45° into it: the solver keeps the tangential part (dynamic friction 1)
        let w = boxes(&[(Vec3::new(1.0, -1.0, -50.0), Vec3::new(2.0, 3.0, 50.0))]);
        let v = Vec3::new(3.0, 0.0, 3.0);
        let (p, nv) = run(&w, Vec3::new(0.0, 1.0, 0.0), v, 60, 0.0);
        assert!((p.x - (1.0 - CAPSULE_RADIUS)).abs() < 0.01, "stopped at the keep distance: {p:?}");
        assert!((nv.z - 3.0).abs() < 1e-3 && nv.x.abs() < 1e-3, "slides along: {nv:?}");
        assert!(p.z > 2.0, "{p:?}");
    }

    #[test]
    fn the_layer_table_matches_the_game() {
        use crate::layers::*;
        assert!(collides(MAIN_CHARACTER, STATIC) && collides(STATIC, MAIN_CHARACTER));
        assert!(!collides(MAIN_CHARACTER_NO_STATIC, STATIC), "hanging / climbing: no static world");
        assert!(collides(MAIN_CHARACTER_NO_STATIC, CHARACTER), "… but other characters still");
        assert!(!collides(CAMERA, CHARACTER), "the camera passes through characters");
        assert!(collides(CAMERA, STATIC));
    }

    #[test]
    fn a_no_static_capsule_passes_through_the_wall() {
        let w = boxes(&[(Vec3::new(1.0, -1.0, -50.0), Vec3::new(2.0, 3.0, 50.0))]);
        let step = |layer: u8| {
            let mut s = ProxyState { layer, ..Default::default() };
            let mut p = Vec3::new(0.0, 1.0, 0.0);
            for _ in 0..60 {
                p = integrate(&w, &mut s, p, Vec3::new(3.0, 0.0, 0.0), 1.0 / 60.0, 0.0).0;
            }
            p
        };
        assert!(step(crate::layers::MAIN_CHARACTER).x < 1.0, "blocked");
        assert!(step(crate::layers::MAIN_CHARACTER_NO_STATIC).x > 2.9, "passes");
    }

    #[test]
    fn an_inner_corner_stops_the_character() {
        let w = boxes(&[(Vec3::new(1.0, -1.0, -5.0), Vec3::new(2.0, 3.0, 5.0)), (Vec3::new(-5.0, -1.0, 1.0), Vec3::new(5.0, 3.0, 2.0))]);
        let (p, nv) = run(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::new(2.0, 0.0, 2.5), 60, 0.0);
        assert!(nv.length() < 1e-3, "{nv:?}");
        assert!((p.x - 0.6).abs() < 0.01 && (p.z - 0.6).abs() < 0.01, "{p:?}");
    }

    #[test]
    fn falling_onto_a_floor_rests_on_it() {
        let w = boxes(&[(Vec3::new(-5.0, -1.0, -5.0), Vec3::new(5.0, 0.0, 5.0))]);
        let (p, nv) = run(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, -6.0, 0.0), 30, 0.0);
        assert!(p.y.abs() < 0.005, "the shape (r − keep) at the keep distance: the base on the floor: {p:?}");
        assert!(nv.y.abs() < 1e-3 && (nv.x - 1.0).abs() < 1e-3, "{nv:?}");
    }

    #[test]
    fn stick_to_ground_rests_the_feet_on_the_surface() {
        let w = boxes(&[(Vec3::new(-5.0, -1.0, -5.0), Vec3::new(5.0, 0.0, 5.0))]);
        let (y, ny) = stick_to_ground(&w, Vec3::new(0.0, 0.3, 0.0), 0.37, 0.58).unwrap();
        assert!(y.abs() < 1e-3 && (ny - 1.0).abs() < 1e-5, "{y} {ny}");
        assert!(stick_to_ground(&w, Vec3::new(0.0, 0.7, 0.0), 0.37, 0.58).is_none(), "beyond 0.58 m below");
    }

    #[test]
    fn source_triangles_slide_stop_and_respect_character_layers() {
        use crate::triangles::Triangle;
        let mut w = CollisionWorld::default();
        let a = Vec3::new(1.0, -2.0, -10.0);
        let b = Vec3::new(1.0, 4.0, -10.0);
        let c = Vec3::new(1.0, 4.0, 10.0);
        let d = Vec3::new(1.0, -2.0, 10.0);
        w.triangles = vec![Triangle::new([a,b,c], crate::layers::STATIC).unwrap(), Triangle::new([a,c,d], crate::layers::STATIC).unwrap()];
        let (p, v) = run(&w, Vec3::Y, Vec3::new(3.0, 0.0, 3.0), 60, 0.0);
        assert!((p.x - 0.6).abs() < 0.01 && p.z > 2.0, "{p:?}");
        assert!(v.x.abs() < 1e-3 && (v.z - 3.0).abs() < 1e-3, "{v:?}");
        let mut state = ProxyState { layer: crate::layers::MAIN_CHARACTER_NO_STATIC, ..Default::default() };
        let p = integrate(&w, &mut state, Vec3::Y, Vec3::X * 3.0, 1.0, 0.0).0;
        assert!(p.x > 2.9, "climbing layer passes through static triangles: {p:?}");
    }

    #[test]
    fn source_slope_support_tracks_the_triangle_surface() {
        let mut w = CollisionWorld::default();
        w.triangles.push(crate::triangles::Triangle::new([Vec3::new(-5.0,-1.0,-5.0), Vec3::new(5.0,1.0,-5.0), Vec3::new(0.0,0.0,5.0)], crate::layers::STATIC).unwrap());
        let (y, normal_y) = stick_to_ground(&w, Vec3::new(0.0,0.3,0.0), 0.37, 0.58).unwrap();
        assert!(y.abs() < 0.02 && normal_y > 0.97, "{y} {normal_y}");
        assert!(w.floor_height_below(Vec3::new(4.5,1.0,4.5),2.0).is_none(), "outside the face, inside its AABB");
    }
}
