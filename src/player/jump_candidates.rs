//! Jump candidates: the game's candidate query `Human__QueryJumpCandidates` 0xE18970 (called by IHuman vt56
//! `Human__FindJumpCandidates` 0xB13260) and the scorer 0xE96BF0 that picks one (RE/18 §1).
//!
//! The query collects guidance edges in a box in front of the jumper. On each edge chain it takes the grab point the
//! forward line meets. Each grab point is classified by three rays: the room on top, what lies below, a clear line of
//! sight. The point is tested against side-profile reach zones (13 polygons, read live from `JumpZones` 0x1A2BF40).
//! Every candidate carries a type, a sub-type and the jump-type flags it allows. The scorer prefers the nearest roof
//! edge in front, then the highest other target, then targets below / behind.
//!
//! Port coordinates are Y-up; the game's are Z-up (forward, up) = (y, z).

use bevy::prelude::*;

use crate::collision::CollisionWorld;
use crate::guidance::{beam_contacts, clip_segment, GuidanceSubType, GuidanceWorld};

/// The reach zones of `JumpZones` 0x1A2BF40 (built by 0xD77DE0, read live 2026-10-09): (horizontal distance, height
/// above the feet). Slots 10–12 are declared one vertex longer than written; the game tests heap garbage there
/// (PORT: closed without it).
pub const ZONES: [&[[f32; 2]]; 13] = [
    &[[0.5, -5.0], [0.5, 0.2], [3.5, 0.6], [4.7, 0.3], [6.0, -0.8], [7.5, -3.0], [7.5, -5.0]],
    &[[0.5, -5.0], [0.5, 1.3], [3.5, 1.3], [4.7, 0.8], [6.0, -0.5], [8.0, -3.0], [8.0, -5.0]],
    &[[0.5, -3.0], [0.5, 2.5], [3.5, 2.5], [5.0, 1.5], [6.5, -0.5], [8.5, -3.0]],
    &[[0.5, -3.0], [0.5, 3.0], [3.5, 3.0], [5.3, 1.8], [7.0, -0.5], [9.0, -3.0]],
    &[[0.5, -5.0], [0.5, 1.3], [4.0, 1.3], [5.2, 0.8], [6.5, -0.5], [8.5, -3.0], [8.5, -5.0]],
    &[[0.5, -3.0], [0.5, 2.5], [4.0, 2.5], [5.5, 1.5], [7.0, -0.5], [9.0, -3.0]],
    &[[0.5, -3.0], [0.5, 3.0], [4.0, 3.0], [5.8, 1.8], [7.5, -0.5], [9.5, -3.0]],
    &[[0.5, -4.3], [0.5, 2.0], [3.5, 2.0], [4.7, 1.5], [6.0, 0.2], [7.0, -2.3], [7.0, -4.3]],
    &[[0.5, -2.3], [0.5, 3.2], [3.5, 3.2], [5.0, 2.3], [6.5, 0.2], [7.5, -2.3]],
    &[[0.5, -2.3], [0.5, 3.7], [3.5, 3.7], [5.3, 2.5], [7.0, 0.2], [8.0, -2.3]],
    &[[0.5, -5.0], [0.5, 0.0], [1.0, -0.2], [2.0, -1.0], [3.0, -3.0], [3.0, -5.0]],
    &[[0.5, -3.0], [0.5, 1.5], [1.0, 1.3], [2.25, -0.3], [3.5, -3.0]],
    &[[0.5, -3.0], [0.5, 2.5], [1.0, 2.3], [2.5, 1.8], [4.0, -3.0]],
];

/// Is `p` inside reach zone `slot` of a jumper at `origin` (`sub_1173590`: cylindrical, the point turned onto the
/// forward plane)? `k` = the character scale (IHuman+2712, live 1.0): vertices from the third on stretch forward by it
/// (0xD75990).
pub fn in_zone(slot: usize, origin: Vec3, p: Vec3, k: f32) -> bool {
    let x = Vec2::new(p.x - origin.x, p.z - origin.z).length();
    let y = p.y - origin.y;
    let poly = ZONES[slot];
    let v = |i: usize| -> [f32; 2] {
        let q = poly[i % poly.len()];
        if i % poly.len() >= 2 { [q[0] * k, q[1]] } else { q }
    };
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (v(i), v(i + 1));
        if (a[1] > y) != (b[1] > y) {
            let t = (y - a[1]) / (b[1] - a[1]);
            if x < a[0] + t * (b[0] - a[0]) {
                inside = !inside;
            }
        }
    }
    inside
}

/// The request (the query's a7 / a8 / a10 / a12 / a13).
#[derive(Clone, Copy, Debug, Default)]
pub struct Query {
    /// The caller's request type (a8): 0 tap jump, 1 / 2 the jump off an edge (low / high profile), 3 / 4 the free-run
    /// target jump (Legs released / held), 5 excludes zone 0.
    pub kind: u32,
    /// IHuman+2452 (a7): 3 / 4 outside the ground (RE/04 §4.1.10); 0 on the ground.
    pub mode: u32,
    /// a10: the longer zones 4–6.
    pub long: bool,
    /// a12: beams (runtime beam contacts) and far-edge landings.
    pub beams: bool,
    /// a13: subtypes left out, bit `1 << subtype` (the free run leaves out ladders: 8).
    pub exclude: u32,
}

impl Query {
    pub const TAP: Query = Query { kind: 0, mode: 0, long: false, beams: true, exclude: 0 };
    /// The free-run target jump (0xEE7F7A): Legs held = 4, else 3; ladders left out.
    pub fn free_run(legs_held: bool) -> Query {
        Query { kind: if legs_held { 4 } else { 3 }, mode: 0, long: false, beams: false, exclude: 8 }
    }
    /// The jump off an edge (0xEE8771): high profile 2, else 1, with beams.
    pub fn edge(high: bool) -> Query {
        Query { kind: if high { 2 } else { 1 }, mode: 0, long: false, beams: true, exclude: 0 }
    }
}

/// A 96-byte candidate (+0 position, +0x20 top normal, +0x30 wall normal, +0x50 type, +0x54 sub-type, +0x58 flags).
#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    pub pos: Vec3,
    /// The face normal with the larger vertical part (the top).
    pub top: Vec3,
    /// The wall normal, flattened, pointing out of the solid (toward the jumper for a facing edge).
    pub wall: Vec3,
    /// Room on top: 2 (≥ 1 m), 8 (0.3–1 m), 16 (< 0.3 m), 32 (a pole), 4 (a beam), 1 (a ladder).
    pub ty: u32,
    /// What lies below: 2 (floor within 0.5 m), 4 (within 2 m), 8 (a deep drop with a wall), 16 (a deep drop, no wall);
    /// 1 for ladders, beams and far-edge landings.
    pub sub: u32,
    /// The jump types it allows: 1 free step, 2 pass-over, 0x40 wall hang, 0x80 free hang, 0x200 horse, 0x400 kiosk,
    /// 0x1000 ladder, 0x10000 beam / far-edge landing.
    pub flags: u32,
    pub subtype: GuidanceSubType,
    /// The guidance edge it lies on (main loop and far edges).
    pub edge: Option<usize>,
    /// Ladders: (bottom, top) of the usable part.
    pub ladder: Option<(Vec3, Vec3)>,
}

struct Zones {
    primary: usize,
    secondary: usize,
    tertiary: usize,
    only_primary: bool,
    exclude: Option<usize>,
    min: Vec3,
    max: Vec3,
}

/// Zone and box choice of 0xE18970 (box = lateral, forward, up from the jumper).
fn zones(q: &Query) -> Zones {
    let default_box = (Vec3::new(-1.0, 0.5, -5.0), Vec3::new(1.0, 9.0, 3.0));
    let exclude = (q.kind == 5).then_some(0);
    if q.kind == 1 || q.kind == 2 || q.mode == 4 {
        return Zones { primary: 10, secondary: 11, tertiary: 12, only_primary: true, exclude, min: Vec3::new(-1.0, 0.5, -5.0), max: Vec3::new(1.0, 4.2, 2.7) };
    }
    if q.mode == 3 {
        return Zones { primary: 7, secondary: 8, tertiary: 9, only_primary: true, exclude, min: Vec3::new(-1.0, 0.5, -4.3), max: Vec3::new(1.0, 9.0, 3.7) };
    }
    let (primary, secondary, tertiary) = if q.long { (4, 5, 6) } else { (1, 2, 3) };
    let (only_primary, (min, max)) = match q.kind {
        4 => (true, (Vec3::new(-0.5, 0.5, 0.45), Vec3::new(0.5, 2.0, 1.3))),
        3 => (true, (Vec3::new(-0.5, 0.5, 0.45), Vec3::new(0.5, 1.3, 1.3))),
        _ => (false, default_box),
    };
    Zones { primary, secondary, tertiary, only_primary, exclude: None, min, max }
}

/// PORT: the collision layer of the query's rays (a15: 5, or 21 for an entity with flag +168 & 7 == 1; the player is
/// taken to be the main character, hypothesis).
const RAY_LAYER: u8 = crate::layers::MAIN_CHARACTER;
/// Ladders' own downward ray (layer 13, 0xE18970).
const LADDER_RAY_LAYER: u8 = crate::layers::ONLY_STATIC;

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z).normalize_or_zero()
}

/// The wall normal of an edge: the face normal with the lower vertical part, flattened (`v221`).
fn wall_of(n0: Vec3, n1: Vec3) -> Vec3 {
    flat(if n1.y <= n0.y { n1 } else { n0 })
}

fn top_of(n0: Vec3, n1: Vec3) -> Vec3 {
    if n1.y <= n0.y { n0 } else { n1 }
}

/// Is the ray from `from` to `to` clear (`sub_E1C820` / `sub_E1C8F0` compared with the length)?
fn clear(collision: &CollisionWorld, from: Vec3, to: Vec3) -> bool {
    let d = to - from;
    let len = d.length();
    len < 1e-4 || collision.ray_distance(from, d / len, len, RAY_LAYER) >= len - 1e-4
}

struct Piece {
    edge: usize,
    a: Vec3,
    b: Vec3,
    subtype: GuidanceSubType,
    sector: u8,
}

/// The query. `pos` = feet, `dir` = the wanted direction.
pub fn query(pos: Vec3, dir: Vec3, q: &Query, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Vec<Candidate> {
    let dir = flat(dir);
    if dir == Vec3::ZERO {
        return Vec::new();
    }
    let k = 1.0;
    let up = Vec3::Y;
    let right = dir.cross(up).normalize();
    let z = zones(q);
    let centre = pos + dir * 0.15 - up * 5.0;
    let box_mid = (z.min + z.max) * 0.5;
    let half = (z.max - z.min) * 0.5;
    let local = |p: Vec3| {
        let d = p - pos;
        Vec3::new(d.dot(right), d.dot(dir), d.dot(up)) - box_mid
    };
    let world = |l: Vec3| {
        let l = l + box_mid;
        pos + right * l.x + dir * l.y + up * l.z
    };
    // a quick world-space bound of the box
    let corners = [z.min, z.max, Vec3::new(z.min.x, z.max.y, z.min.z), Vec3::new(z.max.x, z.min.y, z.max.z)];
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for c in corners {
        for zc in [z.min.z, z.max.z] {
            let w = world(Vec3::new(c.x, c.y, zc) - box_mid);
            lo = lo.min(w);
            hi = hi.max(w);
        }
    }
    let in_bounds = |a: Vec3, b: Vec3| a.min(b).cmple(hi + Vec3::splat(0.01)).all() && a.max(b).cmpge(lo - Vec3::splat(0.01)).all();

    let in1_test = |p: Vec3| in_zone(z.primary, pos, p, k) && !z.exclude.is_some_and(|s| in_zone(s, pos, p, k));
    let mut out = Vec::new();

    // ---------------------------------------------------------------- the report (clipped to the box)
    let mut pieces: Vec<Piece> = Vec::new();
    let mut ladders: Vec<(usize, Vec3, Vec3)> = Vec::new();
    let mut authored_beams: Vec<(Vec3, Vec3)> = Vec::new();
    for (i, e) in guidance.edges.iter().enumerate() {
        if !in_bounds(e.p0, e.p1) {
            continue;
        }
        let Some((a, b)) = clip_segment(local(e.p0), local(e.p1), half) else { continue };
        let (a, b) = (world(a), world(b));
        if a.distance(b) <= 0.0005 {
            continue;
        }
        match e.subtype {
            // ladders are moved to their own list (mask 8, 0x66C220)
            GuidanceSubType::Ladder => {
                ladders.push((i, e.p0, e.p1));
                continue;
            }
            // PORT: the greybox's authored Beam lines (both normals up) are offered with the runtime beams
            GuidanceSubType::Beam => {
                authored_beams.push((a, b));
                continue;
            }
            _ => {}
        }
        // steep edges are removed (0x66C350; 40 degrees as in 0xB12480)
        if (b - a).normalize().y.abs() > 0.643 {
            continue;
        }
        let w = wall_of(e.n0, e.n1);
        if w == Vec3::ZERO {
            continue;
        }
        // the three sectors of 0x116CD50: facing the jumper (within 45 degrees of -dir) and the two sides
        // (PORT: up to 135 degrees from -dir; the chain builder is not decoded)
        let ang = w.angle_between(-dir);
        let sector = if ang <= std::f32::consts::FRAC_PI_4 + 1e-4 {
            0
        } else if ang <= 3.0 * std::f32::consts::FRAC_PI_4 + 1e-4 {
            if w.dot(right) > 0.0 { 1 } else { 2 }
        } else {
            3
        };
        pieces.push(Piece { edge: i, a, b, subtype: e.subtype, sector });
    }

    // ---------------------------------------------------------------- chains (PORT: shared end points)
    let mut chains: Vec<Vec<(usize, Vec3, Vec3)>> = Vec::new(); // (piece, start, end) in chain order
    let mut used = vec![false; pieces.len()];
    for s in 0..pieces.len() {
        if used[s] || pieces[s].sector == 3 {
            continue;
        }
        used[s] = true;
        let mut chain = std::collections::VecDeque::from([(s, pieces[s].a, pieces[s].b)]);
        loop {
            let tail = chain.back().unwrap().2;
            let head = chain.front().unwrap().1;
            let mut grew = false;
            for o in 0..pieces.len() {
                if used[o] || pieces[o].sector != pieces[s].sector || pieces[o].subtype != pieces[s].subtype {
                    continue;
                }
                let (a, b) = (pieces[o].a, pieces[o].b);
                if a.distance(tail) < 0.01 {
                    chain.push_back((o, a, b));
                } else if b.distance(tail) < 0.01 {
                    chain.push_back((o, b, a));
                } else if b.distance(head) < 0.01 {
                    chain.push_front((o, a, b));
                } else if a.distance(head) < 0.01 {
                    chain.push_front((o, b, a));
                } else {
                    continue;
                }
                used[o] = true;
                grew = true;
                break;
            }
            if !grew {
                break;
            }
        }
        chains.push(chain.into_iter().collect());
    }

    // ---------------------------------------------------------------- the main loop (grab point per chain)
    for chain in &chains {
        let piece0 = &pieces[chain[0].0];
        if (1u32 << piece0.subtype as u32) & q.exclude != 0 {
            continue;
        }
        let Some((pi, g)) = grab_point(chain, centre, dir) else { continue };
        let piece = &pieces[pi];
        let e = &guidance.edges[piece.edge];
        let kind = piece.subtype;
        let w = wall_of(e.n0, e.n1);
        let g = if kind == GuidanceSubType::Kiosk { (piece.a + piece.b) * 0.5 } else { g };
        // facing: a piece off the forward line must face the jumper within 45 degrees (0x9765E0, -0.7071)
        let sa = (piece.a - centre).dot(right);
        let sb = (piece.b - centre).dot(right);
        let crosses = sa * sb <= 0.0;
        if !crosses && dir.dot(w) > -std::f32::consts::FRAC_1_SQRT_2 {
            continue;
        }
        // the zones
        let (in1, in2) = if in1_test(g) {
            (true, true)
        } else if in_zone(z.primary, pos, g, k) || z.only_primary {
            continue;
        } else if in_zone(z.secondary, pos, g, k) {
            (false, true)
        } else if in_zone(z.tertiary, pos, g, k) {
            (false, false)
        } else {
            continue;
        };
        // room on top and what lies below (rays from 0.5 m out, 5 cm above the edge)
        let (ty, sub, below) = if kind == GuidanceSubType::Quadruped {
            (2, 1, 0.0)
        } else {
            let o = g + w * 0.5 + up * 0.05;
            let room = collision.ray_distance(o, -w, 1.75, RAY_LAYER) - 0.5;
            let mut ty = if room < 0.03 { 64 } else if room < 0.3 { 16 } else if room < 1.0 { 8 } else { 2 };
            if kind == GuidanceSubType::Pole {
                ty = 32;
            }
            let below = collision.ray_distance(o, -up, 3.05, RAY_LAYER) - 0.05;
            let sub = if below < 0.5 {
                2
            } else if below < 2.0 {
                4
            } else {
                let o2 = g + w * 0.5 - up;
                if collision.ray_distance(o2, -w, 1.4, RAY_LAYER) - 0.5 >= 0.7 { 16 } else { 8 }
            };
            (ty, sub, below)
        };
        let mut flags: u32 = match kind {
            GuidanceSubType::Ladder => 0x40,
            GuidanceSubType::Quadruped => if in1 { 0x200 } else { 0 },
            GuidanceSubType::Kiosk => if in1 { 0x400 } else { 0 },
            GuidanceSubType::Pole => 0xC2,
            _ => 0xC3,
        };
        if g.y - pos.y < -0.5 {
            flags &= !2;
        }
        if !in1 {
            flags &= !3;
        }
        if !in2 {
            flags &= !0x40;
        }
        if ty & 0x40 != 0 {
            continue;
        }
        if ty & 0x10 != 0 {
            flags &= !1;
        }
        if ty & 2 == 0 {
            flags &= !2;
        }
        if below < 2.5 {
            flags &= !0x80;
        }
        if sub & 6 != 0 {
            flags &= !0xC0;
        }
        if sub & 0x10 != 0 {
            flags &= !0x40;
        }
        let eye = pos + up * 0.75;
        if flags & 0x10703 != 0 && !clear(collision, eye, g + w * 0.2) {
            flags &= !0x10703;
        }
        if flags & 0x40 != 0 && !clear(collision, eye, g + w * 0.2 - up * 1.1) {
            flags &= !0x40;
        }
        if flags & 0x80 != 0 && !clear(collision, eye, g + w * 0.2 - up * 1.1) {
            flags &= !0x80;
        }
        if flags == 0 {
            continue;
        }
        out.push(Candidate { pos: g, top: top_of(e.n0, e.n1), wall: w, ty, sub, flags, subtype: kind, edge: Some(piece.edge), ladder: None });
    }

    // ---------------------------------------------------------------- ladders (type 1 / sub-type 1 / 0x1000)
    for (i, p0, p1) in ladders {
        if (1u32 << GuidanceSubType::Ladder as u32) & q.exclude != 0 {
            continue;
        }
        let e = &guidance.edges[i];
        let (mut bottom, top) = if p0.y <= p1.y { (p0, p1) } else { (p1, p0) };
        let along = (top - bottom).normalize_or_zero();
        let len = top.distance(bottom);
        // PORT: the ray starts 5 cm out from the ladder's line, which the greybox and the native maps put on the wall's
        // face (a ray along the face counts as a hit at once)
        let off = wall_of(e.n0, e.n1) * 0.05;
        let d = collision.ray_distance(top + off, -along, len, LADDER_RAY_LAYER);
        if d < len - 1e-4 {
            bottom = top - along * d;
        }
        if top.distance(bottom) < 2.0 {
            continue;
        }
        let (a, b) = (bottom + along, top - along * 0.5);
        let p = nearest_on_shortened(pos + up * 1.5, a, b, 1.5);
        if !in_zone(z.tertiary, pos, p, k) {
            continue;
        }
        let mut n = wall_of(e.n0, e.n1);
        if flat(p - pos).dot(n) > 0.0 {
            n = -n;
        }
        if !clear(collision, pos + up * 0.75, p + n * 0.2) {
            continue;
        }
        out.push(Candidate { pos: p, top: top_of(e.n0, e.n1), wall: n, ty: 1, sub: 1, flags: 0x1000, subtype: GuidanceSubType::Ladder, edge: Some(i), ladder: Some((bottom, top)) });
    }

    if q.beams {
        // ------------------------------------------------------------ beams (type 4 / sub-type 1 / 0x10000)
        let mut beams: Vec<(Vec3, Vec3)> = beam_contacts::detect(guidance, collision, (lo + hi) * 0.5, (hi - lo) * 0.5).into_iter().map(|b| (b.p0, b.p1)).collect();
        beams.extend(authored_beams);
        for (a, b) in beams {
            let along = flat(b - a);
            if along == Vec3::ZERO || along.dot(dir).abs() <= 0.766_044_4 {
                continue;
            }
            // PORT: 0x116E070 clips the beam to the primary zone's front vertices (not decoded); the port takes the
            // in-zone point (sampled every 0.1 m, 0.3 m from the beam's ends) closest to 4 m ahead, as before
            let len = a.distance(b);
            let steps = (len / 0.1).ceil().max(1.0) as usize;
            let ahead = pos + dir * 4.0;
            let margin = (0.3 / len.max(1e-3)).min(0.5);
            let p = (0..=steps).map(|s| s as f32 / steps as f32).filter(|t| (margin..=1.0 - margin).contains(t)).map(|t| a.lerp(b, t)).filter(|p| in1_test(*p))
                .min_by(|x, y| x.distance_squared(ahead).total_cmp(&y.distance_squared(ahead)));
            let Some(p) = p else { continue };
            let eye = pos + up * 0.75;
            if !clear(collision, eye, p + up * 0.05) {
                continue;
            }
            out.push(Candidate { pos: p, top: up, wall: -flat(p - eye), ty: 4, sub: 1, flags: 0x10000, subtype: GuidanceSubType::Beam, edge: None, ladder: None });
        }
        // ------------------------------------------------------------ far edges (type 2 / sub-type 1 / 0x10000)
        for piece in &pieces {
            if piece.subtype != GuidanceSubType::LedgeGrab {
                continue;
            }
            let e = &guidance.edges[piece.edge];
            let w = wall_of(e.n0, e.n1);
            let facing = dir.dot(w);
            if facing <= 0.0 {
                continue;
            }
            let (sa, sb) = ((piece.a - centre).dot(right), (piece.b - centre).dot(right));
            let p = if sa * sb <= 0.0 {
                piece.a.lerp(piece.b, sa / (sa - sb))
            } else if facing >= 0.5 {
                let (near, far) = if sa.abs() <= sb.abs() { (piece.a, piece.b) } else { (piece.b, piece.a) };
                nearest_on_shortened(near, near, far, 0.5)
            } else {
                continue;
            };
            if !in1_test(p) {
                continue;
            }
            let beyond = p + w * 0.3 + up * 0.5;
            if !clear(collision, beyond, beyond - up) {
                continue;
            }
            let onto = p - w * 0.5 + up * 0.5;
            let d = collision.ray_distance(onto, -up, 1.0, RAY_LAYER);
            if d >= 1.0 - 1e-4 {
                continue;
            }
            let top = onto - up * d;
            if !clear(collision, beyond, beyond - w * 1.3) {
                continue;
            }
            if !clear(collision, pos + up * 0.75, top + up * 0.2) {
                continue;
            }
            out.push(Candidate { pos: top, top: up, wall: w, ty: 2, sub: 1, flags: 0x10000, subtype: GuidanceSubType::LedgeGrab, edge: Some(piece.edge), ladder: None });
        }
    }
    out
}

/// `Guidance__NearestOnShortenedSegment` 0x116D3C0: the point of a→b shortened by `step` at both ends closest to
/// `p` (the midpoint when it is shorter than 2·step).
fn nearest_on_shortened(p: Vec3, a: Vec3, b: Vec3, step: f32) -> Vec3 {
    let len = a.distance(b);
    if len <= 2.0 * step + 0.0005 {
        return (a + b) * 0.5;
    }
    let d = (b - a) / len;
    crate::guidance::closest_on_segment(a + d * step, b - d * step, p)
}

/// `GuidanceChain__FindGrabPoint` 0x66E150 (margin 0.5, no hand offsets): `centre` projected along `dir` onto the
/// chain's vertical plane, kept 0.5 m from the chain's ends; the piece carrying it and the point on that piece.
fn grab_point(chain: &[(usize, Vec3, Vec3)], centre: Vec3, dir: Vec3) -> Option<(usize, Vec3)> {
    let s = chain.first()?.1;
    let e = chain.last()?.2;
    let u = flat(e - s);
    if u == Vec3::ZERO {
        // a chain without horizontal extent: its first piece's middle
        let (pi, a, b) = chain[0];
        return Some((pi, (a + b) * 0.5));
    }
    let n = u.cross(Vec3::Y);
    let denom = dir.dot(n);
    let proj = if denom.abs() > 1e-6 { centre + dir * ((s - centre).dot(n) / denom) } else { centre - n * (centre - s).dot(n) };
    let len = Vec2::new(e.x - s.x, e.z - s.z).length();
    let t = (proj - s).dot(u);
    let t = if len > 1.0 { t.clamp(0.5, len - 0.5) } else { len * 0.5 };
    // the piece whose flat extent along the chain carries t
    let mut best: Option<(f32, usize, Vec3)> = None;
    for &(pi, a, b) in chain {
        let (ta, tb) = ((a - s).dot(u), (b - s).dot(u));
        if (tb - ta).abs() > 1e-6 && (ta.min(tb)..=ta.max(tb)).contains(&t) {
            return Some((pi, a.lerp(b, (t - ta) / (tb - ta))));
        }
        for (q, tq) in [(a, ta), (b, tb)] {
            let d = (tq - t).abs();
            if best.is_none_or(|x| d < x.0) {
                best = Some((d, pi, q));
            }
        }
    }
    best.map(|b| (b.1, b.2))
}

/// The scorer 0xE96BF0. `actor` = feet, `facing` = the actor's forward, `want` = the wanted direction (the stick or
/// the facing), `mode` = a5 (2: the nearest roof edge in front only), `behind_far` = a7 & 1. Returns the candidate and
/// the jump type.
pub fn select(cands: &[Candidate], actor: Vec3, facing: Vec3, want: Vec3, mode: u32, behind_far: bool) -> Option<(usize, u32)> {
    let want = if want.length_squared() > 2.5e-7 { flat(want) } else { flat(facing) };
    let fwd = flat(facing);
    let plane_n = (fwd * 0.5 + Vec3::Y * 0.7).normalize();
    let ledge_like = |c: &Candidate| (c.flags & 1 != 0 && c.ty == 2) || (c.flags & 0x10000 != 0 && c.sub == 1 && c.ty == 4);
    let is_pole = |c: &Candidate| c.flags & 0x80 != 0 && c.sub == 16 && c.ty == 32;
    let is_far = |c: &Candidate| c.flags & 0x10000 != 0 && c.sub == 1 && c.ty == 2;
    let (mut front_ledge, mut front_ledge_d, mut front_ledge_dz) = (None, 1000.0f32, -1000.0f32);
    let (mut pole, mut pole_d, mut pole_dz) = (None, 1000.0f32, -1000.0f32);
    let (mut front_far, mut front_far_d) = (None, 0.0f32);
    let (mut front_other, mut front_other_dz) = (None, -1000.0f32);
    let (mut back_ledge, mut back_ledge_d) = (None, -1000.0f32);
    let (mut back_far, mut back_far_d) = (None, 0.0f32);
    let (mut back_other, mut back_other_dz) = (None, -1000.0f32);
    for (i, c) in cands.iter().enumerate() {
        if c.sub == 2 {
            continue;
        }
        let toward = -flat(c.wall);
        let theta = toward.dot(want).clamp(-1.0, 1.0).acos();
        let d = Vec2::new(c.pos.x - actor.x, c.pos.z - actor.z).length();
        let dz = c.pos.y - actor.y;
        let front = (c.pos - (actor + Vec3::Y * 0.5)).dot(plane_n) > 0.0;
        let cone = theta < std::f32::consts::FRAC_PI_4 && dz > -3.0;
        if !front {
            if ledge_like(c) {
                if d > back_ledge_d {
                    back_ledge = Some(i);
                    back_ledge_d = d;
                }
            } else if is_far(c) {
                if behind_far && d > back_far_d {
                    back_far = Some(i);
                    back_far_d = d;
                }
            } else if cone && dz > back_other_dz {
                back_other = Some(i);
                back_other_dz = dz;
            }
        } else {
            if ledge_like(c) && d < front_ledge_d {
                front_ledge = Some(i);
                front_ledge_d = d;
                front_ledge_dz = dz;
            }
            if is_pole(c) {
                if cone && ((pole_d > d && pole_dz - dz < 2.5) || dz - pole_dz > 2.5) {
                    pole = Some(i);
                    pole_d = d;
                    pole_dz = dz;
                }
            } else if is_far(c) {
                if dz < 0.5 && (dz < -0.5 || d > 3.0) && d > front_far_d {
                    front_far = Some(i);
                    front_far_d = d;
                }
            } else if !ledge_like(c) && cone && dz > front_other_dz {
                front_other = Some(i);
                front_other_dz = dz;
            }
        }
    }
    let ledge_type = |c: &Candidate| if c.flags & 1 != 0 { 1 } else if c.flags & 0x10000 != 0 { 0x10000 } else { 1 };
    if mode == 2 {
        return front_ledge.map(|i| (i, 2));
    }
    if let Some(p) = pole {
        match front_ledge {
            None => return Some((p, 0x80)),
            Some(l) if l != p => {
                if front_ledge_dz <= pole_dz && (pole_d <= front_ledge_d || pole_dz - front_ledge_dz >= 2.5) {
                    return Some((p, 0x80));
                }
                return Some((l, ledge_type(&cands[l])));
            }
            _ => {}
        }
    }
    if let Some(l) = front_ledge {
        return Some((l, ledge_type(&cands[l])));
    }
    if let Some(i) = front_other {
        return Some((i, first_type(cands[i].flags)));
    }
    if let Some(i) = front_far {
        return Some((i, 0x10000));
    }
    if let Some(i) = back_ledge {
        return Some((i, ledge_type(&cands[i])));
    }
    if let Some(i) = back_other {
        return Some((i, first_type(cands[i].flags)));
    }
    back_far.map(|i| (i, 0x10000))
}

/// A foot-level edge report (IHuman vt132 `Human__BuildEdgeReports` 0xB1BC50, 64 bytes): an edge with a drop
/// beyond it near the feet.
#[derive(Clone, Copy, Debug)]
pub struct EdgeReport {
    /// +16: where the line through the feet meets the edge.
    pub point: Vec3,
    /// +32: the wall normal, flattened, pointing out over the drop.
    pub normal: Vec3,
    /// +48: the drop beyond the edge.
    pub drop: f32,
    /// +52: the horizontal distance to the edge, negative once the feet are past it.
    pub dist: f32,
    pub edge: usize,
}

/// `Human__BuildEdgeReports` 0xB1BC50 (pos, radius, min drop; the floor case is not ported): every guidance piece
/// within `radius` that is not steep, whose perpendicular through the feet meets it within ±0.3 m of foot height,
/// with a drop ≥ `min_drop` beyond it.
pub fn edge_reports(pos: Vec3, radius: f32, min_drop: f32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Vec<EdgeReport> {
    let mut out = Vec::new();
    for (i, e) in guidance.edges.iter().enumerate() {
        if matches!(e.subtype, GuidanceSubType::Ladder | GuidanceSubType::Beam) {
            continue;
        }
        let d = e.p1 - e.p0;
        let len = d.length();
        if len < 1e-4 || (d.y / len).abs() > 0.643 {
            continue;
        }
        // the sphere zone (radius) around the feet
        if crate::guidance::closest_on_segment(e.p0, e.p1, pos).distance(pos) > radius {
            continue;
        }
        // where the plane through the feet across the edge meets it (0x9765E0 with the flattened edge direction)
        let u = flat(d);
        let (sa, sb) = ((e.p0 - pos).dot(u), (e.p1 - pos).dot(u));
        if sa * sb > 0.0 || (sa - sb).abs() < 1e-6 {
            continue;
        }
        let p = e.p0.lerp(e.p1, sa / (sa - sb));
        if (pos.y - p.y).abs() >= 0.3 {
            continue;
        }
        let n = wall_of(e.n0, e.n1);
        if n == Vec3::ZERO {
            continue;
        }
        let drop = measure_drop(p, n, 7.0, 0.02, collision);
        if drop < min_drop {
            continue;
        }
        let h = Vec2::new(p.x - pos.x, p.z - pos.z).length();
        let dist = if n.dot(pos - p) > 0.0 { -h } else { h };
        out.push(EdgeReport { point: p, normal: n, drop, dist, edge: i });
    }
    out
}

/// `Human__MeasureDropBeyondEdge` 0xB19620 (max, step out). PORT: one ray down from just past the edge; the exe
/// sweeps angled rays and follows stepped drops with nested edge reports.
fn measure_drop(p: Vec3, n: Vec3, max: f32, step: f32, collision: &CollisionWorld) -> f32 {
    let o = p + n * (step + 0.05) + Vec3::Y * 0.05;
    collision.ray_distance(o, -Vec3::Y, max + 0.05, RAY_LAYER) - 0.05
}

/// IHuman vt136 0xB17EF0: the report within `radius` whose normal is closest in angle to `dir`, at most `max_angle`.
pub fn pick_report(reports: &[EdgeReport], pos: Vec3, dir: Vec3, radius: f32, max_angle: f32) -> Option<EdgeReport> {
    let dir = flat(dir);
    reports.iter()
        .filter(|r| Vec2::new(r.point.x - pos.x, r.point.z - pos.z).length_squared() < radius * radius)
        .map(|r| (r.normal.dot(dir).clamp(-1.0, 1.0).acos(), r))
        .filter(|(a, _)| *a <= max_angle)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, r)| *r)
}

/// The body box of event 68's guard (`sub_B2E7A0` → `Human__BoxQueryEmpty` 0xB2D2A0): 0.5 m out over the edge and
/// 0.7 m up, extents 0.25 / 0.3 / 1.0 (taken as half extents across / along the normal / up, hypothesis).
pub fn room_past_edge(r: &EdgeReport, collision: &CollisionWorld) -> bool {
    let centre = r.point + r.normal * 0.5 + Vec3::Y * 0.7;
    let across = r.normal.cross(Vec3::Y).normalize_or_zero();
    collision.obb_free(centre, [across, r.normal, Vec3::Y], Vec3::new(0.25, 0.3, 1.0))
}

/// `sub_E96B10`: the jump type of a candidate's flags, by priority.
pub fn first_type(flags: u32) -> u32 {
    for t in [0x1000, 0x2000, 0x4000, 0x100, 0x200, 0x400, 0x10000, 1, 2, 4, 8, 0x40, 0x80] {
        if flags & t != 0 {
            return t;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::Aabb3;
    use crate::guidance::{GuidanceEdge, GuidanceWorld};

    fn crate_world(h: f32, depth: f32) -> (CollisionWorld, GuidanceWorld) {
        let mut c = CollisionWorld::default();
        c.boxes.push(Aabb3 { min: Vec3::new(-20.0, -1.0, -20.0), max: Vec3::new(20.0, 0.0, 20.0) });
        // a box ahead along -Z, near face at z = -1.5
        c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 0.0, -1.5 - depth), max: Vec3::new(2.0, h, -1.5) });
        let mut g = GuidanceWorld::default();
        g.edges.push(GuidanceEdge { p0: Vec3::new(-2.0, h, -1.5), p1: Vec3::new(2.0, h, -1.5), n0: Vec3::Y, n1: Vec3::Z, subtype: GuidanceSubType::LedgeGrab });
        g.edges.push(GuidanceEdge { p0: Vec3::new(-2.0, h, -1.5 - depth), p1: Vec3::new(2.0, h, -1.5 - depth), n0: Vec3::Y, n1: Vec3::NEG_Z, subtype: GuidanceSubType::LedgeGrab });
        (c, g)
    }

    /// Debug: the candidates of the greybox at `AC_PROBE_POS=x,y,z` along `AC_PROBE_DIR=x,y,z` (kind `AC_PROBE_KIND`).
    #[test]
    #[ignore]
    fn probe_candidates() {
        let v = |k: &str, d: Vec3| std::env::var(k).ok().map(|s| { let f: Vec<f32> = s.split(',').map(|x| x.trim().parse().unwrap()).collect(); Vec3::new(f[0], f[1], f[2]) }).unwrap_or(d);
        let pos = v("AC_PROBE_POS", Vec3::ZERO);
        let dir = v("AC_PROBE_DIR", Vec3::NEG_Z);
        let kind = std::env::var("AC_PROBE_KIND").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let (c, g) = crate::level::geometry();
        let q = Query { kind, beams: kind == 0 || kind == 1 || kind == 2, exclude: if kind == 3 || kind == 4 { 8 } else { 0 }, ..Default::default() };
        let cands = query(pos, dir, &q, &g, &c);
        for (i, k) in cands.iter().enumerate() {
            eprintln!("{i}: pos {:.2} wall {:.2} ty {} sub {} flags {:#x} {:?} edge {:?}", k.pos, k.wall, k.ty, k.sub, k.flags, k.subtype, k.edge);
        }
        eprintln!("select: {:?}", select(&cands, pos, dir, dir, 1, false));
    }

    #[test]
    fn zones_match_the_live_table() {
        // slot 1: 1.3 m up at 3 m, not 1.4; 8 m far at 3 m down
        assert!(in_zone(1, Vec3::ZERO, Vec3::new(0.0, 1.29, -3.0), 1.0));
        assert!(!in_zone(1, Vec3::ZERO, Vec3::new(0.0, 1.4, -3.0), 1.0));
        assert!(in_zone(1, Vec3::ZERO, Vec3::new(7.9, -3.5, 0.0), 1.0));
        assert!(!in_zone(1, Vec3::ZERO, Vec3::new(0.3, 0.0, 0.0), 1.0));
        // slot 3 reaches 3 m up
        assert!(in_zone(3, Vec3::ZERO, Vec3::new(0.0, 2.9, -2.0), 1.0));
    }

    #[test]
    fn a_waist_high_crate_ahead_is_a_free_step_candidate_for_the_free_run() {
        let (c, g) = crate_world(1.0, 1.0);
        let cands = query(Vec3::ZERO, Vec3::NEG_Z, &Query::free_run(true), &g, &c);
        assert_eq!(cands.len(), 1, "{cands:?}");
        let k = cands[0];
        assert_eq!((k.ty, k.sub), (2, 4));
        assert_eq!(k.flags & 3, 3);
        assert_eq!(select(&cands, Vec3::ZERO, Vec3::NEG_Z, Vec3::NEG_Z, 1, false), Some((0, 1)));
    }

    #[test]
    fn the_free_run_box_ignores_a_crate_out_of_reach() {
        let (c, g) = crate_world(1.6, 1.0);
        assert!(query(Vec3::ZERO, Vec3::NEG_Z, &Query::free_run(true), &g, &c).is_empty());
        let (c, g) = crate_world(1.0, 1.0);
        // Legs released: the box only reaches 1.3 m ahead
        assert!(query(Vec3::new(0.0, 0.0, 0.2), Vec3::NEG_Z, &Query::free_run(false), &g, &c).is_empty());
    }

    #[test]
    fn a_high_wall_is_a_wall_hang_candidate() {
        let mut c = CollisionWorld::default();
        c.boxes.push(Aabb3 { min: Vec3::new(-20.0, -1.0, -20.0), max: Vec3::new(20.0, 0.0, 20.0) });
        c.boxes.push(Aabb3 { min: Vec3::new(-3.0, 0.0, -10.0), max: Vec3::new(3.0, 2.4, -3.0) });
        let mut g = GuidanceWorld::default();
        g.edges.push(GuidanceEdge { p0: Vec3::new(-3.0, 2.4, -3.0), p1: Vec3::new(3.0, 2.4, -3.0), n0: Vec3::Y, n1: Vec3::Z, subtype: GuidanceSubType::LedgeGrab });
        let cands = query(Vec3::ZERO, Vec3::NEG_Z, &Query::TAP, &g, &c);
        assert_eq!(cands.len(), 1, "{cands:?}");
        assert_eq!(cands[0].flags, 0x40);
        assert_eq!(cands[0].sub, 8);
        assert_eq!(select(&cands, Vec3::ZERO, Vec3::NEG_Z, Vec3::NEG_Z, 1, false), Some((0, 0x40)));
    }

    #[test]
    fn a_roof_edge_in_front_beats_a_higher_hang() {
        let mut c = CollisionWorld::default();
        c.boxes.push(Aabb3 { min: Vec3::new(-20.0, -1.0, -20.0), max: Vec3::new(20.0, 0.0, 20.0) });
        c.boxes.push(Aabb3 { min: Vec3::new(-3.0, 0.0, -6.0), max: Vec3::new(3.0, 1.0, -2.0) });
        c.boxes.push(Aabb3 { min: Vec3::new(-3.0, 0.0, -12.0), max: Vec3::new(3.0, 3.0, -6.0) });
        let mut g = GuidanceWorld::default();
        g.edges.push(GuidanceEdge { p0: Vec3::new(-3.0, 1.0, -2.0), p1: Vec3::new(3.0, 1.0, -2.0), n0: Vec3::Y, n1: Vec3::Z, subtype: GuidanceSubType::LedgeGrab });
        g.edges.push(GuidanceEdge { p0: Vec3::new(-3.0, 3.0, -6.0), p1: Vec3::new(3.0, 3.0, -6.0), n0: Vec3::Y, n1: Vec3::Z, subtype: GuidanceSubType::LedgeGrab });
        let cands = query(Vec3::ZERO, Vec3::NEG_Z, &Query::TAP, &g, &c);
        let (i, ty) = select(&cands, Vec3::ZERO, Vec3::NEG_Z, Vec3::NEG_Z, 1, false).unwrap();
        assert_eq!(ty, 1);
        assert!((cands[i].pos.z + 2.0).abs() < 1e-3, "{:?}", cands[i]);
    }
}
