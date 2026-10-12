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

/// `sub_E153C0`: the outline of zone `slot` from its second vertex on (`count` vertices), in the frame of a jumper at
/// `origin` facing `dir` (x forward, y up; vertices from the third on stretched by `k`, as in `in_zone`).
fn zone_outline(slot: usize, origin: Vec3, dir: Vec3, count: usize, k: f32) -> Vec<Vec3> {
    let poly = ZONES[slot];
    let f = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
    (1..=count).filter(|&i| i < poly.len()).map(|i| {
        let q = poly[i];
        let x = if i >= 2 { q[0] * k } else { q[0] };
        origin + f * x + Vec3::Y * q[1]
    }).collect()
}

/// `sub_116E070` (with `sub_976A50`, the segment–segment closest approach): the point of `a`–`b` nearest the polyline.
fn nearest_to_outline(a: Vec3, b: Vec3, outline: &[Vec3]) -> Vec3 {
    let mut best = (f32::MAX, 0.0f32);
    for w in outline.windows(2) {
        let (t, d) = segment_closest(a, b, w[0], w[1]);
        if d < best.0 {
            best = (d, t);
        }
    }
    if outline.len() < 2 || best.1 <= 0.0 { a } else if best.1 >= 1.0 { b } else { a.lerp(b, best.1) }
}

/// The closest approach of segments p0–p1 and q0–q1: (the parameter on the first, the distance).
fn segment_closest(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> (f32, f32) {
    let (d1, d2, r) = (p1 - p0, q1 - q0, p0 - q0);
    let (a, e, f) = (d1.dot(d1), d2.dot(d2), d2.dot(r));
    let (s, t);
    if a <= 1e-9 && e <= 1e-9 {
        return (0.0, r.length());
    }
    if a <= 1e-9 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= 1e-9 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let bb = d1.dot(d2);
            let denom = a * e - bb * bb;
            let mut s0 = if denom > 1e-9 { ((bb * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t0 = (bb * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((bb - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    (s, ((p0 + d1 * s) - (q0 + d2 * t)).length())
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
pub(crate) fn wall_of(n0: Vec3, n1: Vec3) -> Vec3 {
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
    /// The chain sets the piece belongs to (bit k = set k).
    sets: u8,
    /// The lengths the wedge cut off at `a` and `b` (`GuidanceContact__AddCutLengths` 0x6764E0).
    cut: [f32; 2],
}

/// The jump wedge: the `MeshShape` of the shared `GuidanceZoneMesh` at `sub_4155B0()` +208 (vertices and triangles read
/// from the running game, live 2026-10-12), in its own frame: x right (dir × up), y forward, z up. A sector about ±12.5°
/// wide from its apex, reaching 3 m up to 3 m out, 1.8 m at 4.8, −0.5 at 7.0, −3 at 8.5, its floor 5 m down. Not
/// quite convex: the arc's second vertices stand at 13.4° against 12.7° for their neighbours.
const WEDGE_VERTS: [[f32; 3]; 22] = [
    [0.0, 0.0, 3.0], [0.633, 2.924, 3.0], [1.113, 4.670, 1.8], [1.553, 6.803, -0.5], [1.812, 8.288, -3.0],
    [0.0, 0.0, -5.0], [0.204, 3.0, 3.0], [0.375, 4.801, 1.8], [0.523, 6.985, -0.5], [0.610, 8.5, -3.0],
    [-0.232, 3.0, 3.0], [-0.375, 4.801, 1.8], [-0.523, 6.985, -0.5], [-0.610, 8.5, -3.0], [-0.661, 2.924, 3.0],
    [-1.113, 4.670, 1.8], [-1.553, 6.803, -0.5], [-1.812, 8.288, -3.0], [1.812, 8.288, -5.0], [0.610, 8.5, -5.0],
    [-0.610, 8.5, -5.0], [-1.812, 8.288, -5.0],
];
const WEDGE_TRIS: [[usize; 3]; 40] = [
    [0, 1, 6], [1, 2, 7], [1, 7, 6], [2, 3, 8], [2, 8, 7], [3, 4, 9], [3, 9, 8], [18, 5, 19], [0, 6, 10], [6, 7, 11],
    [6, 11, 10], [7, 8, 12], [7, 12, 11], [8, 9, 13], [8, 13, 12], [19, 5, 20], [0, 10, 14], [10, 11, 15], [10, 15, 14],
    [11, 12, 16], [11, 16, 15], [12, 13, 17], [12, 17, 16], [20, 5, 21], [3, 5, 4], [3, 2, 5], [2, 1, 5], [5, 1, 0],
    [16, 17, 5], [14, 5, 0], [15, 16, 5], [15, 5, 14], [9, 4, 18], [18, 19, 9], [5, 18, 4], [13, 9, 19], [19, 20, 13],
    [17, 13, 20], [20, 21, 17], [5, 17, 21],
];

/// Where the line `o + t·d` crosses the wedge's triangles (Möller–Trumbore), as parameters t.
fn wedge_crossings(o: Vec3, d: Vec3) -> Vec<f32> {
    let v = WEDGE_VERTS.map(|[x, y, z]| Vec3::new(x, y, z));
    WEDGE_TRIS.iter().filter_map(|&[i, j, k]| {
        let (e1, e2) = (v[j] - v[i], v[k] - v[i]);
        let p = d.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            return None;
        }
        let s = o - v[i];
        let u = s.dot(p) / det;
        let q = s.cross(e1);
        let w = d.dot(q) / det;
        (u >= 0.0 && w >= 0.0 && u + w <= 1.0).then(|| e2.dot(q) / det)
    }).collect()
}

/// Is `p` (wedge frame) inside the wedge: an odd number of crossings on a ray from it (a skew direction, clear of
/// the mesh's vertices and edges).
fn in_wedge(p: Vec3) -> bool {
    wedge_crossings(p, Vec3::new(0.123, 0.311, 0.942)).into_iter().filter(|&t| t > 0.0).count() % 2 == 1
}

/// The part of `a`→`b` (wedge frame) inside the wedge, as segment parameters: from `a` (inside) or the first crossing
/// to `b` (inside) or the last (`sub_5AD070` → 0x6334D0: the hits from either end; mode 1 of `GuidanceZoneMesh` vslot
/// 12, 0x659570).
fn wedge_clip(a: Vec3, b: Vec3) -> Option<(f32, f32)> {
    let mut ts: Vec<f32> = wedge_crossings(a, b - a).into_iter().filter(|t| (0.0..=1.0).contains(t)).collect();
    ts.sort_by(f32::total_cmp);
    let t0 = if in_wedge(a) { 0.0 } else { *ts.first()? };
    let t1 = if in_wedge(b) { 1.0 } else { *ts.last()? };
    (t1 > t0).then_some((t0, t1))
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
    // the report's LedgeGrab pieces, paired into beams below (0xE19BE0 `sub_680260` on this report)
    let mut ledge_pieces: Vec<crate::guidance::GuidanceEdge> = Vec::new();
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
            GuidanceSubType::LedgeGrab if q.beams => {
                let mut piece = e.clone();
                piece.p0 = a;
                piece.p1 = b;
                ledge_pieces.push(piece);
            }
            _ => {}
        }
        // steep edges are removed (0x66C350; 40 degrees as in 0xB12480)
        if crate::guidance::steep(b - a) {
            continue;
        }
        let w = wall_of(e.n0, e.n1);
        if w == Vec3::ZERO {
            continue;
        }
        // the wedge (a11: every player jump query but a long one passes it, 0xEE8026 / 0xEE81E0 / 0xEE8757 /
        // 0xEE9712; a10 clears it): the report is clipped to the jump wedge at pos + 0.5·dir (0.7 m higher in mode 3),
        // kiosks exempt (mask 256), after the ladders, the beams' copy and the steep removal (0xE191D0, `sub_5AA970` →
        // 0x659570 mode 1); a piece outside it is dropped and a clipped one keeps its cut lengths. Live (RE/25
        // freerun_east): it drops a corner's side face 0.6 m off the jump line, which the game then does not jump to.
        let (a, b, cut) = if q.long || e.subtype == GuidanceSubType::Kiosk {
            (a, b, [0.0, 0.0])
        } else {
            let origin = pos + dir * 0.5 + if q.mode == 3 { up * 0.7 } else { Vec3::ZERO };
            let wl = |p: Vec3| { let d = p - origin; Vec3::new(d.dot(right), d.dot(dir), d.dot(up)) };
            let Some((t0, t1)) = wedge_clip(wl(a), wl(b)) else { continue };
            let len = a.distance(b);
            (a.lerp(b, t0), a.lerp(b, t1), [t0 * len, (1.0 - t1) * len])
        };
        if a.distance(b) <= 0.0005 {
            continue;
        }
        // the three chain sets (`GuidanceChain__Build` 0x116CD50 at 0xE19AD6 / 0xE19B13 / 0xE19B70, RE/24 AIR-27):
        // (reference, axis, turn range), right = dir × up (var_EAC, 0xE198F0). An edge belongs to a set when its
        // direction runs within 45° of the axis and its outward perpendicular is turned from the reference by a signed
        // angle in the range (`GuidanceWorld::chains`' convention, RE/24 GUIDE-8): set 0 the walls within 45° of facing
        // the jumper, sets 1 / 2 a single 45° wedge from ±right toward the jumper. A wall facing away is in none.
        let q4 = std::f32::consts::FRAC_PI_4;
        let d = (b - a).normalize();
        let low = if e.n1.y <= e.n0.y { e.n1 } else { e.n0 };
        let mut perp = d.cross(up).normalize_or_zero();
        if perp.dot(low) < 0.0 {
            perp = -perp;
        }
        let mut sets = 0u8;
        for (k, (r, axis, lo, hi)) in [(-dir, right, -q4, q4), (right, dir, 0.0, q4), (-right, dir, -q4, 0.0)].into_iter().enumerate() {
            if d.dot(axis).abs() < q4.cos() - 1e-6 {
                continue;
            }
            let turn = perp.cross(r).dot(up).atan2(perp.dot(r));
            if turn >= lo - 1e-4 && turn <= hi + 1e-4 {
                sets |= 1 << k;
            }
        }
        pieces.push(Piece { edge: i, a, b, subtype: e.subtype, sets, cut });
    }

    // ---------------------------------------------------------------- chains (sub_116CD50 × 3 → sub_678670)
    // Each sector is one of the game's three chain sets (facing, right, left). Within a set the last candidate seeds a
    // chain, which grows at its front, then at its back, by the first candidate with an end within 0.01 m of the free
    // end (sub_678010 / sub_677E40); the game does not compare the subtypes.
    let mut chains: Vec<Vec<(usize, Vec3, Vec3)>> = Vec::new(); // (piece, start, end) in chain order
    for sector in 0..3u8 {
        let mut cands: Vec<usize> = (0..pieces.len()).filter(|&i| pieces[i].sets & (1 << sector) != 0).collect();
        while let Some(seed) = cands.pop() {
            let mut chain = std::collections::VecDeque::from([(seed, pieces[seed].a, pieces[seed].b)]);
            loop {
                let head = chain.front().unwrap().1;
                let Some(k) = cands.iter().position(|&o| pieces[o].a.distance(head) < 0.01 || pieces[o].b.distance(head) < 0.01) else { break };
                let o = cands.remove(k);
                let (a, b) = (pieces[o].a, pieces[o].b);
                chain.push_front(if b.distance(head) < 0.01 { (o, a, b) } else { (o, b, a) });
            }
            loop {
                let tail = chain.back().unwrap().2;
                let Some(k) = cands.iter().position(|&o| pieces[o].a.distance(tail) < 0.01 || pieces[o].b.distance(tail) < 0.01) else { break };
                let o = cands.remove(k);
                let (a, b) = (pieces[o].a, pieces[o].b);
                chain.push_back(if a.distance(tail) < 0.01 { (o, a, b) } else { (o, b, a) });
            }
            chains.push(chain.into_iter().collect());
        }
    }

    // ---------------------------------------------------------------- the main loop (grab point per chain)
    for chain in &chains {
        let piece0 = &pieces[chain[0].0];
        if (1u32 << piece0.subtype as u32) & q.exclude != 0 {
            continue;
        }
        let Some((pi, g)) = grab_point(chain, &pieces, centre, dir) else { continue };
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
        // the beams paired from the box-clipped report (0xE19BE0), not the whole neighbourhood: a beam outside the
        // kind's box (kind 3 / 4: 0.45–1.3 m up) is no candidate
        let mut beams: Vec<(Vec3, Vec3)> = beam_contacts::pair_report(std::mem::take(&mut ledge_pieces), collision).into_iter().map(|b| (b.p0, b.p1)).collect();
        beams.extend(authored_beams);
        for (a, b) in beams {
            let along = flat(b - a);
            if along == Vec3::ZERO || along.dot(dir).abs() <= 0.766_044_4 {
                continue;
            }
            // `sub_116E070` (0xE19D3E): the point on the beam nearest the zone's outline, the primary polygon from its
            // second vertex on (4 vertices for the kinds 1 / 2 or mode 4, else 5; `sub_E153C0` 0xE19C18) set in the
            // jump's frame (forward = dir, up); then `JumpZone__Contains` (0xE19D5A). A beam running along the jump
            // gives its far end
            let p = nearest_to_outline(a, b, &zone_outline(z.primary, pos, dir, if q.kind == 1 || q.kind == 2 || q.mode == 4 { 4 } else { 5 }, k));
            // PORT: a beam crossing the outline gives the crossing point itself, on the zone's boundary, where the
            // game's float test is untraced: it counts when 1 mm toward the beam's nearer end lies inside
            let near = if a.distance_squared(pos) <= b.distance_squared(pos) { a } else { b };
            let p = if in1_test(p) { p } else { p + (near - p).normalize_or_zero() * 0.001 };
            if !in1_test(p) {
                continue;
            }
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
fn grab_point(chain: &[(usize, Vec3, Vec3)], pieces: &[Piece], centre: Vec3, dir: Vec3) -> Option<(usize, Vec3)> {
    let s = chain.first()?.1;
    let e = chain.last()?.2;
    // the cut lengths at the chain's free ends lengthen it (0x66E150: each hand offset, here 0, less the end's cut
    // scratch; live RE/25 freerun_east: the grab point lies past the wedge-clipped end, at the middle of the piece the
    // box left)
    let cut_at = |pi: usize, q: Vec3| { let p = &pieces[pi]; if p.a.distance(q) <= p.b.distance(q) { p.cut[0] } else { p.cut[1] } };
    let (cs, ce) = (cut_at(chain[0].0, s), cut_at(chain[chain.len() - 1].0, e));
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
    let (lo, hi) = (-cs, len + ce);
    let t = if hi - lo > 1.0 { t.clamp(lo + 0.5, hi - 0.5) } else { (lo + hi) * 0.5 };
    // past a free end: on the end piece's line
    for (&(pi, a, b), out) in [(chain.first()?, t < 0.0), (chain.last()?, t > len)] {
        let (ta, tb) = ((a - s).dot(u), (b - s).dot(u));
        if out && (tb - ta).abs() > 1e-6 {
            return Some((pi, a.lerp(b, (t - ta) / (tb - ta))));
        }
    }
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
    /// The horizontal distance to the edge, negative once the feet are past it (the game stores its square, `sq`).
    pub dist: f32,
    pub edge: usize,
}

impl EdgeReport {
    /// +52 as the game stores it: the **squared** horizontal distance (`sub_42EC70` is the squared length), negative
    /// once the feet are past the edge. The game's thresholds on it are squared distances (live RE/25: the halt's 0.16
    /// fired with the feet 0.393 m from the edge, +52 = 0.154).
    pub fn sq(&self) -> f32 {
        self.dist * self.dist.abs()
    }
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
        if d.length() < 1e-4 || crate::guidance::steep(d) {
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
        let drop = measure_drop_stepped(p, n, 7.0, 0.02, collision, Some(guidance));
        if drop < min_drop {
            continue;
        }
        let h = Vec2::new(p.x - pos.x, p.z - pos.z).length();
        let dist = if n.dot(pos - p) > 0.0 { -h } else { h };
        out.push(EdgeReport { point: p, normal: n, drop, dist, edge: i });
    }
    out
}

/// `Human__MeasureDropBeyondEdge` 0xB19620 (max, step out), without the stepped-drop refinement (no guidance at hand).
pub(crate) fn measure_drop(p: Vec3, n: Vec3, max: f32, step: f32, collision: &CollisionWorld) -> f32 {
    measure_drop_stepped(p, n, max, step, collision, None)
}

/// `Human__MeasureDropBeyondEdge` 0xB19620 (max, step out): the edge height minus the floor found beyond it.
/// - From `step` past the edge a ray 45 deg out and down (the axis up × normal), to 0.5 m below the edge (at most
///   `max`). A hit is the floor.
/// - Else, from its end, a ray 10 deg off straight down, outward, to `max` below the edge. Nothing → `max`.
/// - A floor steeper than 45 deg (`Math__SignedAngleAroundAxis` from up) is slid down: from 0.01 m off it along its
///   flattened normal, a ray 45 deg down and out from it to 0.5 m below the hit, then the 10 deg ray again.
/// - Stairs: with a drop d, the edge reports at the floor (IHuman vt132 with radius 0.2 · clamp(d / 3, 0, 1) when that
///   exceeds 0.01, minimum drop 0.45, no floor report) add the deepest one's drop (`sub_B14CA0`), so a ledge below the
///   edge adds the drop beyond it (RE/25 #2: the 1.61 m roof edge over a 0.6 m shelf halts the walk).
/// PORT: the two checks before the sweep are left out (the edge's lip from 0.05 m inside, 50 deg down, and the line
/// from 1 m above the feet to the point past the edge; a hit on either is the drop); the rays use one layer.
pub(crate) fn measure_drop_stepped(p: Vec3, n: Vec3, max: f32, step: f32, collision: &CollisionWorld, guidance: Option<&GuidanceWorld>) -> f32 {
    let n = flat(n);
    if n == Vec3::ZERO {
        let o = p + Vec3::Y * 0.05;
        return collision.ray_distance(o, -Vec3::Y, max + 0.05, RAY_LAYER) - 0.05;
    }
    let tilted = |axis: Vec3, deg: f32| Quat::from_axis_angle(axis, (-deg).to_radians()) * -Vec3::Y;
    // the ray from `o` along `d` down to the height `h` (`Vec_ProjectOntoPlane` 0x9470B0 with a horizontal plane)
    let to_plane = |o: Vec3, d: Vec3, h: f32| if d.y < -1e-6 { ((o.y - h) / -d.y).max(0.0) } else { 0.0 };
    let stepped = |hit: Vec3| {
        let drop = p.y - hit.y;
        let r = 0.2 * (drop / 3.0).clamp(0.0, 1.0);
        let below = guidance.filter(|_| r > 0.01).map_or(0.0, |g| {
            edge_reports(hit, r, 0.45, g, collision).iter().map(|e| e.drop).fold(0.0, f32::max)
        });
        drop + below
    };
    let mut axis = Vec3::Y.cross(n);
    let mut from = p + n * step;
    let mut dir = tilted(axis, 45.0);
    let mut len = to_plane(from, dir, p.y - max.min(0.5));
    for _ in 0..32 {
        if let Some((d, _)) = collision.ray_hit(from, dir, len, RAY_LAYER) {
            return stepped(from + dir * d);
        }
        from += dir * len;
        dir = tilted(axis, 10.0);
        len = to_plane(from, dir, p.y - max);
        let Some((d, normal)) = collision.ray_hit(from, dir, len, RAY_LAYER) else { return max };
        let hit = from + dir * d;
        let side = flat(normal);
        if side == Vec3::ZERO || normal.angle_between(Vec3::Y) <= 45f32.to_radians() {
            return stepped(hit);
        }
        axis = Vec3::Y.cross(side);
        from = hit + side * 0.01;
        dir = tilted(axis, 45.0);
        len = to_plane(from, dir, hit.y - 0.5);
    }
    max
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

    /// RE/24 AIR-27: a side set is a single 45° wedge from ±right toward the jumper. An oblique edge across the jump
    /// line whose wall faces 30° toward the jumper from the side is a candidate; the same edge facing 30° away is not.
    #[test]
    fn the_side_chain_sets_take_walls_turned_toward_the_jumper() {
        let found = |n: Vec3| {
            let mut c = CollisionWorld::default();
            c.boxes.push(Aabb3 { min: Vec3::new(-20.0, -1.0, -20.0), max: Vec3::new(20.0, 0.0, 20.0) });
            let d = Vec3::new(-n.z, 0.0, n.x);
            let mid = Vec3::new(0.0, 1.0, -2.0);
            let mut g = GuidanceWorld::default();
            g.edges.push(GuidanceEdge { p0: mid - d * 1.5, p1: mid + d * 1.5, n0: Vec3::Y, n1: n, subtype: GuidanceSubType::LedgeGrab });
            !query(Vec3::ZERO, Vec3::NEG_Z, &Query::TAP, &g, &c).is_empty()
        };
        let (s, co) = (30f32.to_radians().sin(), 30f32.to_radians().cos());
        assert!(found(Vec3::new(co, 0.0, s)), "facing right, turned toward the jumper");
        assert!(found(Vec3::new(-co, 0.0, s)), "facing left, turned toward the jumper");
        assert!(!found(Vec3::new(co, 0.0, -s)), "facing right, turned away");
        assert!(!found(Vec3::new(-co, 0.0, -s)), "facing left, turned away");
    }

    /// The wedge's side at 0.89 m up, where the live clips cut the corner's edges (RE/25 freerun_east, in the wedge
    /// frame: forward, left of the jump line): (2.44, 0.56), (2.54, 0.58), (2.58, 0.59), (3.46, 0.81). The mesh's surface
    /// meets them within 6 mm (the logs give 3 decimals of position and direction).
    #[test]
    fn the_wedge_cuts_where_the_game_cut() {
        for (f, l) in [(2.438, 0.559), (2.541, 0.579), (2.578, 0.594), (3.460, 0.813)] {
            // a lateral segment from the jump line out to the left (x is right)
            let (t0, t1) = wedge_clip(Vec3::new(0.0, f, 0.887), Vec3::new(-2.0, f, 0.887)).unwrap();
            assert!(t0 == 0.0, "{t0}");
            let edge = 2.0 * t1;
            assert!((edge - l).abs() < 0.01, "at {f}: the wedge ends {edge:.3} to the left, the game cut at {l}");
        }
        // behind the apex and above 3 m nothing is inside
        assert!(wedge_clip(Vec3::new(-1.0, -0.5, 0.0), Vec3::new(1.0, -0.5, 0.0)).is_none());
        assert!(wedge_clip(Vec3::new(-1.0, 2.0, 3.2), Vec3::new(1.0, 2.0, 3.2)).is_none());
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
