//! Guidance edges: the world markup that tells the player what can be grabbed/landed on.
//!
//! Mirrors the game's GuidanceObject (RE/06 §2.1): an edge between two vertices plus the normals of
//! the two faces that meet there and a subtype. In the game these are baked into .forge data; the
//! greybox level generates them from box tops (the "geometry fallback" described in RE/06 §8).

use bevy::prelude::*;
pub mod beam_contacts;

/// `GuidanceObjectSubType` enum, values verbatim from the exe (desc 0x18E087C).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuidanceSubType {
    None = 0,
    LedgeGrab = 1,
    Beam = 2,
    Ladder = 3,
    Pole = 4,
    Rope = 5,
    Surface = 6,
    Quadruped = 7,
    Kiosk = 8,
}

#[derive(Clone, Debug)]
pub struct GuidanceEdge {
    pub p0: Vec3,
    pub p1: Vec3,
    /// Normal of the "top" face (usually up).
    pub n0: Vec3,
    /// Normal of the "wall" face (points away from the solid).
    pub n1: Vec3,
    pub subtype: GuidanceSubType,
}

impl GuidanceEdge {
    pub fn closest_point(&self, p: Vec3) -> Vec3 {
        let d = self.p1 - self.p0;
        let t = ((p - self.p0).dot(d) / d.length_squared().max(1e-6)).clamp(0.0, 1.0);
        self.p0 + d * t
    }
}

/// PORT: half a hand's width kept between a hand contact and the end of its edge.
const HAND_MARGIN: f32 = 0.05;

#[derive(Resource, Default)]
pub struct GuidanceWorld {
    pub edges: Vec<GuidanceEdge>,
    /// Haystacks (EntityDescriptorObject_HayStack): Leap of Faith / jump targets of type 0x800, not solid.
    pub haystacks: Vec<crate::collision::Aabb3>,
    /// PORT: derived once after loading an immutable native map; never populated for editable greybox worlds.
    pub jump_pilotis: Option<Vec<Vec3>>,
}

/// A hit from a guidance query: closest point on an edge plus that edge's data.
#[derive(Clone, Copy, Debug)]
pub struct GuidanceHit {
    pub point: Vec3,
    pub edge: usize,
    /// Wall normal (n1) of the edge, pointing out of the solid.
    pub wall_normal: Vec3,
    pub dir: Vec3,
}

impl GuidanceWorld {
    /// PORT (legacy): the closest LedgeGrab point to `p` within `radius` horizontally and `vtol` vertically, on an edge
    /// whose slope is ≤ 40° (RE/06 post-filter) and whose wall faces `facing` within `max_angle`. Only the pre-A2 air
    /// catch uses it (`tuning::AIR_CATCHES = false`); every Human search now runs the game's own queries
    /// (`probe_facing`, `probe_facing_pair`, `step_search`, `vertical_search`, `handhold_pair`, `chains`).
    pub fn probe(&self, p: Vec3, radius: f32, vtol: f32, facing: Option<Vec3>, max_angle: f32) -> Option<GuidanceHit> {
        let mut best: Option<(f32, GuidanceHit)> = None;
        for (i, e) in self.edges.iter().enumerate() {
            if e.subtype != GuidanceSubType::LedgeGrab {
                continue;
            }
            let d = (e.p1 - e.p0).normalize_or_zero();
            if d.y.abs() > 0.643 {
                continue; // > 40° from horizontal
            }
            if let Some(f) = facing {
                let f = Vec3::new(f.x, 0.0, f.z).normalize_or_zero();
                if (-e.n1).dot(f) < max_angle.cos() {
                    continue;
                }
            }
            let q = e.closest_point(p);
            let dv = (q.y - p.y).abs();
            let dh = Vec2::new(q.x - p.x, q.z - p.z).length();
            if dv > vtol || dh > radius {
                continue;
            }
            let score = dh + dv;
            if best.as_ref().is_none_or(|b| score < b.0) {
                best = Some((score, GuidanceHit { point: q, edge: i, wall_normal: e.n1, dir: d }));
            }
        }
        best.map(|b| b.1)
    }

    /// A hang centred at `p` on the edge carrying it, moved along that edge so both hands (`HAND_SPACING` apart, plus
    /// a hand's width) stay on it. The closest point of an edge is clamped to its ends, so a grab near an end used to
    /// put one hand past the end, outside the climbable markup. An edge shorter than the hands keeps its middle.
    pub fn fit_hands(&self, p: Vec3, n: Vec3) -> Vec3 {
        let Some(hit) = self.on_edge(p, n, 0.15) else { return p };
        let e = &self.edges[hit.edge];
        let d = e.p1 - e.p0;
        let len = d.length();
        if len < 1e-4 {
            return p;
        }
        let half = crate::tuning::HAND_SPACING * 0.5 + HAND_MARGIN;
        let t = (hit.point - e.p0).dot(d) / len;
        let t = if len <= 2.0 * half { len * 0.5 } else { t.clamp(half, len - half) };
        e.p0 + d * (t / len)
    }

    /// The game's oriented-box edge probe (`sub_11713A0` → `sub_1170A70`). A box zone at `centre` with axes `right`,
    /// `forward`, `up` and half extents `half` (clip = 1: each edge is cut to the box, RE/06 §4.1). Edge types are
    /// filtered by `type_mask` (bit `1 << subtype`). The wall normal (the one of n0 / n1 with the lower vertical part,
    /// flattened) must face against `forward` within `max_angle`. On each kept piece the point closest to `reference`
    /// is taken, the piece first shortened by `step` at both ends (`sub_116D3C0`: its midpoint when it is shorter than
    /// 2·step); the nearest point wins. The hit carries the flattened wall normal.
    #[allow(clippy::too_many_arguments)]
    pub fn probe_box(
        &self,
        centre: Vec3,
        right: Vec3,
        forward: Vec3,
        up: Vec3,
        half: Vec3,
        reference: Vec3,
        max_angle: f32,
        step: f32,
        type_mask: u32,
    ) -> Option<GuidanceHit> {
        let local = |p: Vec3| {
            let d = p - centre;
            Vec3::new(d.dot(right), d.dot(forward), d.dot(up))
        };
        let world = |l: Vec3| centre + right * l.x + forward * l.y + up * l.z;
        let mut best: Option<(f32, GuidanceHit)> = None;
        for (i, e) in self.edges.iter().enumerate() {
            if type_mask & (1 << e.subtype as u32) == 0 {
                continue;
            }
            // the wall normal: the lower of the two face normals, flattened (0x1170A70)
            let wall = if e.n1.y <= e.n0.y { e.n1 } else { e.n0 };
            let wall = Vec3::new(wall.x, 0.0, wall.z).normalize_or_zero();
            if wall == Vec3::ZERO || wall.angle_between(forward) < std::f32::consts::PI - max_angle {
                continue;
            }
            let Some((a, b)) = clip_segment(local(e.p0), local(e.p1), half) else { continue };
            let (a, b) = (world(a), world(b));
            let len = a.distance(b);
            let q = if step * 2.0 + 0.0005 < len {
                let dir = (b - a) / len;
                closest_on_segment(a + dir * step, a + dir * (len - step), reference)
            } else {
                (a + b) * 0.5
            };
            let score = q.distance_squared(reference);
            if best.as_ref().is_none_or(|b| score < b.0) {
                // a ladder hit snaps to the nearest 0.5 m rung (`sub_116DEA0` at 0x1170EAB), after the score
                let q = if e.subtype == GuidanceSubType::Ladder { snap_to_rung(e, q) } else { q };
                best = Some((score, GuidanceHit { point: q, edge: i, wall_normal: wall, dir: (e.p1 - e.p0).normalize_or_zero() }));
            }
        }
        best.map(|b| b.1)
    }

    /// Is there a LedgeGrab edge carrying point `p` (within `tol`) with wall normal ≈ `n`?
    pub fn on_edge(&self, p: Vec3, n: Vec3, tol: f32) -> Option<GuidanceHit> {
        self.edges
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.subtype, GuidanceSubType::LedgeGrab | GuidanceSubType::Pole) && e.n1.dot(n) > 0.9)
            .map(|(i, e)| (i, e, e.closest_point(p)))
            .filter(|(_, _, q)| (*q - p).length() <= tol)
            .min_by(|a, b| (a.2 - p).length().total_cmp(&(b.2 - p).length()))
            .map(|(i, e, q)| GuidanceHit { point: q, edge: i, wall_normal: e.n1, dir: (e.p1 - e.p0).normalize_or_zero() })
    }

    /// `Guidance__ProbeBox` 0x11713A0 as the Human searches call it: a box zone at `centre` facing `forward` (right =
    /// forward × up, up = world up) with half extents `half` (right, forward, up), the reference point `back` m behind
    /// the centre (their local reference (0, −back, 0)), the edge pieces shortened by 0.1 m (`PROBE_STEP`).
    pub fn probe_facing(&self, centre: Vec3, forward: Vec3, half: Vec3, back: f32, max_angle: f32, type_mask: u32) -> Option<GuidanceHit> {
        let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        self.probe_box(centre, forward.cross(Vec3::Y), forward, Vec3::Y, half, centre - forward * back, max_angle, PROBE_STEP, type_mask)
    }

    /// `sub_1171050`: one box zone (as `probe_facing`, no reference offset) and the nearest edge to each of two
    /// reference points (`sub_116FF40` per hand, the same type, facing and shortening rules as the box probe). Both
    /// must find an edge.
    #[allow(clippy::too_many_arguments)]
    pub fn probe_facing_pair(
        &self,
        centre: Vec3,
        forward: Vec3,
        half: Vec3,
        refs: [Vec3; 2],
        max_angle: f32,
        step: f32,
        type_mask: u32,
    ) -> Option<[GuidanceHit; 2]> {
        let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let right = forward.cross(Vec3::Y);
        let a = self.probe_box(centre, right, forward, Vec3::Y, half, refs[0], max_angle, step, type_mask)?;
        let b = self.probe_box(centre, right, forward, Vec3::Y, half, refs[1], max_angle, step, type_mask)?;
        Some([a, b])
    }
}

/// What `GuidanceWorld::step_search` found: the step distance `t` along the move, the hand hit, and for climb holds the
/// feet hit (`None` = a hang: the caller looks for wall supports under the hands).
#[derive(Clone, Copy, Debug)]
pub struct StepFound {
    pub t: f32,
    pub hands: GuidanceHit,
    pub feet: Option<GuidanceHit>,
    /// The hand box's centre (the hang case's wall-support search starts 1.0 m below it, `sub_11718B0`).
    pub hand_centre: Vec3,
}

impl GuidanceWorld {
    /// `sub_B153C0`: steps t = step/2, 3·step/2, … while t − step/2 < `range` along `dir`. Each step tiles a box zone facing
    /// `forward` with half extents (step/2, half_yz) and the reference at its centre (45°, mask 50, pieces shortened
    /// 0.1 m):
    /// 1. `climb`: a feet box at `feet_start` + t·dir, then a hand box 1.2 m above that box → climb holds;
    /// 2. `hang`: a hand box at `hands_start` + t·dir → a hang.
    #[allow(clippy::too_many_arguments)]
    pub fn step_search(
        &self,
        hands_start: Vec3,
        feet_start: Vec3,
        dir: Vec3,
        forward: Vec3,
        range: f32,
        step: f32,
        half_yz: Vec2,
        climb: bool,
        hang: bool,
    ) -> Option<StepFound> {
        let half = Vec3::new(step * 0.5, half_yz.x, half_yz.y);
        let mut t = step * 0.5;
        loop {
            if climb {
                let fc = feet_start + dir * t;
                if let Some(feet) = self.probe_facing(fc, forward, half, 0.0, 0.785_398_2, MASK_CLIMB_HOLDS) {
                    let hc = fc + Vec3::Y * 1.2;
                    if let Some(hands) = self.probe_facing(hc, forward, half, 0.0, 0.785_398_2, MASK_CLIMB_HOLDS) {
                        return Some(StepFound { t, hands, feet: Some(feet), hand_centre: hc });
                    }
                }
            }
            if hang {
                let hc = hands_start + dir * t;
                if let Some(hands) = self.probe_facing(hc, forward, half, 0.0, 0.785_398_2, MASK_CLIMB_HOLDS) {
                    return Some(StepFound { t, hands, feet: None, hand_centre: hc });
                }
            }
            // the counter runs 0, step, 2·step … and stops at `range` (t = counter + step/2)
            t += step;
            if range <= t - step * 0.5 {
                return None;
            }
        }
    }
}

/// The `sub_116F4A0` wedge: half-angle about the step's facing (rad) and the height above the start of the capsule
/// (r 0.2, ±0.5 m along the right axis) cut out of the candidates first (the edge being held).
#[derive(Clone, Copy, Debug)]
pub struct Wedge {
    pub angle: f32,
    pub cut_up: f32,
}

impl GuidanceWorld {
    /// `sub_116F4A0` (also through `sub_1170A00`): the ledge's vertical step search. A frame at `start` (right =
    /// forward × up, forward, up); each candidate edge (the ledge report: no Ladder edges, none steeper than 40°, no
    /// Surface unless `surface`) is cut to the box |x| ≤ width/2, |y| ≤ 0.5, z in [lo, hi] (and to `z_window`, the
    /// report box's heights in world space). With a `wedge`, the capsule's part is cut out first (GuidanceZoneCapsule
    /// vslot 10 mode 2), then the piece is cut to the two half-spaces through start + lo (up) / hi (down) whose normals
    /// are the step direction (±z) turned ±(90° − angle) about the forward axis: a V in the wall plane. The wall (the lower
    /// face normal, flattened) must face within 60° (dot < −0.5). On each piece the point nearest the line through
    /// start + x_ref·right along right; the piece farthest along the step wins (|z − (lo − 0.1)| up, |z − (hi + 0.1)|
    /// down), ties within 1 cm going to the point nearest x_ref.
    #[allow(clippy::too_many_arguments)]
    pub fn vertical_search(
        &self,
        start: Vec3,
        up: bool,
        forward: Vec3,
        lo: f32,
        hi: f32,
        width: f32,
        x_ref: f32,
        wedge: Option<Wedge>,
        surface: bool,
        z_window: (f32, f32),
    ) -> Option<GuidanceHit> {
        let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let right = forward.cross(Vec3::Y);
        let local = |p: Vec3| {
            let d = p - start;
            Vec3::new(d.dot(right), d.dot(forward), d.y)
        };
        let world = |l: Vec3| start + right * l.x + forward * l.y + Vec3::Y * l.z;
        // the box, also cut to the report's heights
        let (z0, z1) = (lo.max(z_window.0 - start.y), hi.min(z_window.1 - start.y));
        if z0 > z1 {
            return None;
        }
        let centre = Vec3::new(0.0, 0.0, (z0 + z1) * 0.5);
        let half = Vec3::new(width * 0.5, 0.5, (z1 - z0) * 0.5);
        let target_z = if up { lo - 0.1 } else { hi + 0.1 };
        let mut best: Option<(f32, f32, GuidanceHit)> = None;
        for (i, e) in self.edges.iter().enumerate() {
            if e.subtype == GuidanceSubType::Ladder || (!surface && e.subtype == GuidanceSubType::Surface) {
                continue;
            }
            let dir = (e.p1 - e.p0).normalize_or_zero();
            if dir.y.abs() > 0.642_787_6 {
                continue; // GuidanceReport__RemoveSteepEdges: more than 40° from horizontal
            }
            let wall = if e.n1.y <= e.n0.y { e.n1 } else { e.n0 };
            let wall = Vec3::new(wall.x, 0.0, wall.z).normalize_or_zero();
            if wall.dot(forward) >= -0.5 {
                continue;
            }
            let pieces: Vec<(Vec3, Vec3)> = match wedge {
                Some(w) => {
                    let c = start + Vec3::Y * w.cut_up;
                    outside_capsule(e.p0, e.p1, c - right * 0.5, c + right * 0.5, 0.2)
                }
                None => vec![(e.p0, e.p1)],
            };
            for (a, b) in pieces {
                let Some((la, lb)) = clip_segment(local(a) - centre, local(b) - centre, half) else { continue };
                let (mut la, mut lb) = (la + centre, lb + centre);
                if let Some(w) = wedge {
                    let apex = Vec3::new(0.0, 0.0, if up { lo } else { hi });
                    // the step direction (±z of the local frame right, forward, up) turned ±(90° − a15) about the
                    // forward axis (`sub_55E390`(a8, a9, …) at 0x116FA59–0x116FAC2): the wedge lies in the wall plane,
                    // |x| ≤ tan(a15)·|z| from the apex (RE/24 LEDGE-13)
                    let f = if up { 1.0f32 } else { -1.0 };
                    let turn = std::f32::consts::FRAC_PI_2 - w.angle;
                    let mut ok = true;
                    for s in [1.0f32, -1.0] {
                        let (sn, cs) = (turn * s).sin_cos();
                        let n = Vec3::new(f * sn, 0.0, f * cs);
                        match clip_half_space(la, lb, apex, n) {
                            Some((p, q)) => (la, lb) = (p, q),
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        continue;
                    }
                }
                // the point nearest the reference line (x = x_ref, y = 0, along x)
                let p = nearest_to_x_line(la, lb, x_ref);
                let score = (p.z - target_z).abs();
                let tie = best.as_ref().is_some_and(|b| (score - b.0).abs() < 0.01);
                let better = match &best {
                    None => true,
                    Some(b) if tie => (p.x - x_ref).abs() < (b.1 - x_ref).abs(),
                    Some(b) => score > b.0,
                };
                if better {
                    best = Some((score, p.x, GuidanceHit { point: world(p), edge: i, wall_normal: wall, dir }));
                }
            }
        }
        best.map(|b| b.2)
    }
}

/// `sub_116DEA0` (from `Guidance__ProbeBoxNearest` 0x1170A70 for Ladder hits): the point's distance from the ladder
/// entity's origin rounded to the nearest 0.5 m (a remainder of 0.25 or more rounds up), along origin → top (RE/24
/// GUIDE-1). PORT: the origin is the edge's lower end (the entity's origin when it stands at the ladder's base; the
/// placement origin is not kept per edge).
pub fn snap_to_rung(e: &GuidanceEdge, p: Vec3) -> Vec3 {
    let (o, top) = if e.p0.y <= e.p1.y { (e.p0, e.p1) } else { (e.p1, e.p0) };
    let axis = (top - o).normalize_or_zero();
    let d = (p - o).length();
    let k = (d / 0.5).floor();
    let k = if d - k * 0.5 >= 0.25 { k + 1.0 } else { k };
    o + axis * (k * 0.5)
}

/// The parts of segment a→b farther than `r` from segment c→d (none, one or two pieces): the cut of a capsule zone
/// (GuidanceZoneCapsule vslot 10, mode 2).
fn outside_capsule(a: Vec3, b: Vec3, c: Vec3, d: Vec3, r: f32) -> Vec<(Vec3, Vec3)> {
    let dist = |t: f32| {
        let p = a + (b - a) * t;
        p.distance(closest_on_segment(c, d, p))
    };
    // the distance is convex along the segment: its minimum by ternary search, then the crossings by bisection
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..60 {
        let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if dist(m1) < dist(m2) { hi = m2 } else { lo = m1 }
    }
    let tm = (lo + hi) * 0.5;
    if dist(tm) > r {
        return vec![(a, b)];
    }
    let cross = |mut inside: f32, mut outside: f32| {
        for _ in 0..40 {
            let m = (inside + outside) * 0.5;
            if dist(m) <= r { inside = m } else { outside = m }
        }
        (inside + outside) * 0.5
    };
    let mut out = Vec::new();
    if dist(0.0) > r {
        let t0 = cross(tm, 0.0);
        out.push((a, a + (b - a) * t0));
    }
    if dist(1.0) > r {
        let t1 = cross(tm, 1.0);
        out.push((a + (b - a) * t1, b));
    }
    out
}

/// Segment a→b cut to the half-space dot(x − p, n) ≥ 0 (`sub_977900`); None when nothing is left.
fn clip_half_space(a: Vec3, b: Vec3, p: Vec3, n: Vec3) -> Option<(Vec3, Vec3)> {
    let (da, db) = ((a - p).dot(n), (b - p).dot(n));
    if da < 0.0 && db < 0.0 {
        return None;
    }
    if da >= 0.0 && db >= 0.0 {
        return Some((a, b));
    }
    let t = da / (da - db);
    let m = a + (b - a) * t;
    Some(if da >= 0.0 { (a, m) } else { (m, b) })
}

/// The point of segment a→b (local frame) nearest the line y = 0, z = 0 … through x = `x_ref` along x (`sub_116D4F0`):
/// the closest points of the segment and the line; for a segment parallel to it, the point nearest x = `x_ref`.
fn nearest_to_x_line(a: Vec3, b: Vec3, x_ref: f32) -> Vec3 {
    // distance to the line along x depends on (y, z) only
    let d = b - a;
    let dyz = Vec2::new(d.y, d.z);
    let l2 = dyz.length_squared();
    if l2 < 1e-10 {
        let t = if d.x.abs() < 1e-9 { 0.0 } else { ((x_ref - a.x) / d.x).clamp(0.0, 1.0) };
        return a + d * t;
    }
    let t = (-(Vec2::new(a.y, a.z).dot(dyz)) / l2).clamp(0.0, 1.0);
    a + d * t
}

impl GuidanceWorld {
    /// A guidance report (`GuidanceZone__Query` 0x5AB580 with a GuidanceZoneBox): the edges meeting the box at
    /// `centre` facing `forward` (right = forward × up, up = world up) between the local corners `min` and `max`, as a
    /// world of their own; `clip` cuts each edge to the box, else whole edges are kept. Edge indices are the report's.
    pub fn report(&self, centre: Vec3, forward: Vec3, min: Vec3, max: Vec3, clip: bool) -> GuidanceWorld {
        self.report_indexed(centre, forward, min, max, clip).world
    }

    /// The filtered report the ledge, climb-grid, pull-down and ladder side-jump searches read (RE/24 GUIDE-2): the edges
    /// within `radius` of `centre`, every Ladder edge as it is (the probes clip their finds to the report with Ladder
    /// edges exempt, GUIDE-3), every other edge with the ladder zones cut out (`sub_1170880` → `sub_116F060`: per ladder
    /// a box right = n × up, forward = n, up = along the ladder, origin its lower end, ±0.3 across, ±0.75 in front /
    /// behind, from 0.2 m below to 0.2 m past it; `CutReport` mode 2 Exclude, exempt mask 8) and, with `steep`, those
    /// steeper than 40° removed (`GuidanceReport__RemoveSteepEdges` 0x66C350). PORT: the depth filter that follows
    /// (`sub_B27E80` / `sub_B28180`: a 0.1 m depth map along each edge, intervals under the threshold rejected) is not
    /// applied; its sampled segments and frame are not decoded yet.
    pub fn ledge_report(&self, centre: Vec3, radius: f32, steep: bool) -> GuidanceWorld {
        let near = |e: &GuidanceEdge| {
            let (lo, hi) = (e.p0.min(e.p1), e.p0.max(e.p1));
            centre.clamp(lo, hi).distance_squared(centre) <= radius * radius
        };
        // the ladder zones: (origin, right, forward, up, length)
        let zones: Vec<(Vec3, Vec3, Vec3, Vec3, f32)> = self
            .edges
            .iter()
            .filter(|e| e.subtype == GuidanceSubType::Ladder && near(e))
            .filter_map(|e| {
                let (b, t) = if e.p0.y <= e.p1.y { (e.p0, e.p1) } else { (e.p1, e.p0) };
                let up = (t - b).normalize_or_zero();
                let fwd = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
                let right = fwd.cross(Vec3::Y).normalize_or_zero();
                (up != Vec3::ZERO && right != Vec3::ZERO).then_some((b, right, fwd, up, (t - b).length()))
            })
            .collect();
        let mut out = GuidanceWorld { haystacks: self.haystacks.clone(), ..Default::default() };
        for e in self.edges.iter().filter(|e| near(e)) {
            if e.subtype == GuidanceSubType::Ladder {
                out.edges.push(e.clone());
                continue;
            }
            if steep && (e.p1 - e.p0).normalize_or_zero().y.abs() > 0.642_787_6 {
                continue;
            }
            let mut pieces = vec![(e.p0, e.p1)];
            for &(o, r, f, u, len) in &zones {
                let local = |p: Vec3| {
                    let d = p - o;
                    Vec3::new(d.dot(r), d.dot(f), d.dot(u))
                };
                let mid = Vec3::new(0.0, 0.0, len * 0.5);
                let half = Vec3::new(0.3, 0.75, len * 0.5 + 0.2);
                let mut next = Vec::new();
                for (a, b) in pieces {
                    let ab = b - a;
                    let l2 = ab.length_squared();
                    match clip_segment(local(a) - mid, local(b) - mid, half) {
                        Some((ia, ib)) if l2 > 1e-12 => {
                            // the inside part as parameters along a → b, then keep what lies outside it
                            let back = |l: Vec3| {
                                let l = l + mid;
                                o + r * l.x + f * l.y + u * l.z
                            };
                            let (ta, tb) = ((back(ia) - a).dot(ab) / l2, (back(ib) - a).dot(ab) / l2);
                            let (t0, t1) = (ta.min(tb).clamp(0.0, 1.0), ta.max(tb).clamp(0.0, 1.0));
                            if t0 > 1e-4 {
                                next.push((a, a + ab * t0));
                            }
                            if t1 < 1.0 - 1e-4 {
                                next.push((a + ab * t1, b));
                            }
                        }
                        _ => next.push((a, b)),
                    }
                }
                pieces = next;
            }
            for (a, b) in pieces {
                out.edges.push(GuidanceEdge { p0: a, p1: b, ..e.clone() });
            }
        }
        out
    }

    /// `report` with each report edge's index in this world and, for a clipped report, the lengths cut off its two ends
    /// (the contact's runtime scratch, `sub_6764E0`; none for Beam edges).
    pub fn report_indexed(&self, centre: Vec3, forward: Vec3, min: Vec3, max: Vec3, clip: bool) -> Report {
        let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let right = forward.cross(Vec3::Y);
        let mid = (min + max) * 0.5;
        let half = (max - min) * 0.5;
        let local = |p: Vec3| {
            let d = p - centre;
            Vec3::new(d.dot(right), d.dot(forward), d.y) - mid
        };
        let world = |l: Vec3| {
            let l = l + mid;
            centre + right * l.x + forward * l.y + Vec3::Y * l.z
        };
        let mut out = Report::default();
        for (i, e) in self.edges.iter().enumerate() {
            let Some((a, b)) = clip_segment(local(e.p0), local(e.p1), half) else { continue };
            if clip {
                let (pa, pb) = (world(a), world(b));
                let cut = if e.subtype == GuidanceSubType::Beam { (0.0, 0.0) } else { (pa.distance(e.p0), pb.distance(e.p1)) };
                out.world.edges.push(GuidanceEdge { p0: pa, p1: pb, ..e.clone() });
                out.cuts.push(cut);
            } else {
                out.world.edges.push(e.clone());
                out.cuts.push((0.0, 0.0));
            }
            out.source.push(i);
        }
        out
    }
}

/// A guidance chain (`sub_678670`): report edges linked end to end, each link (edge index, start, end) in chain order.
#[derive(Clone, Debug)]
pub struct Chain {
    pub links: Vec<(usize, Vec3, Vec3)>,
}

impl Chain {
    pub fn start(&self) -> Vec3 {
        self.links[0].1
    }
    pub fn end(&self) -> Vec3 {
        self.links[self.links.len() - 1].2
    }
}

/// The chain builder's settings (`sub_116CD50`): the reference direction, the right axis, the turn ranges and the
/// parallel tolerance, the type mask.
#[derive(Clone, Copy, Debug)]
pub struct ChainParams {
    /// +16: the reference direction the walls' turn is measured from (the searches pass −forward).
    pub dir: Vec3,
    /// +32: the axis the edges must run along.
    pub right: Vec3,
    /// +56 … +68: two signed turn ranges (rad) about up from `dir` to the edge's outward perpendicular.
    pub ranges: [(f32, f32); 2],
    /// +72: an edge's direction within this angle of `right` (|cos| ≥ |cos(parallel)|, +76).
    pub parallel: f32,
    /// +80: the type mask.
    pub mask: u32,
}

/// The end-point tolerance of chain links (`flt_19D6800`, 0.01 m; squared at +52).
const CHAIN_LINK_TOL: f32 = 0.01;

impl GuidanceWorld {
    /// `sub_678670`: the candidates (`sub_677B00`): edges of the mask whose direction runs within `parallel` of
    /// `right` and whose outward perpendicular (horizontal, across the edge, on the lower face's side) is turned from
    /// `dir` about up by a signed angle in one of the ranges; an edge with the same ends as a kept one (0.001 m) is
    /// dropped. Then chains: the last candidate seeds one, which grows at its front, then at its back, by the first
    /// candidate with an end within 0.01 m of the free end (`sub_678010` / `sub_677E40`), until none is left.
    pub fn chains(&self, p: &ChainParams) -> Vec<Chain> {
        let cos_par = p.parallel.cos().abs();
        let dir = Vec3::new(p.dir.x, 0.0, p.dir.z).normalize_or_zero();
        let right = p.right.normalize_or_zero();
        let mut cands: Vec<usize> = Vec::new();
        for (i, e) in self.edges.iter().enumerate() {
            if p.mask != 1 && (1u32 << e.subtype as u32) & p.mask == 0 {
                continue;
            }
            let d = e.p1 - e.p0;
            if d.normalize_or_zero().dot(right).abs() < cos_par {
                continue;
            }
            let low = if e.n1.y <= e.n0.y { e.n1 } else { e.n0 };
            let mut perp = d.cross(Vec3::Y).normalize_or_zero();
            if perp.dot(low) < 0.0 {
                perp = -perp;
            }
            // the signed angle about up from the edge's perpendicular to the reference (`GuidanceChainBuilder__CollectCandidates`
            // 0x677B00: Math__SignedAngleAroundAxis(&perp, ref, &up), 0x55E470 / 0x55E2C0; RE/24 GUIDE-8)
            let turn = perp.cross(dir).dot(Vec3::Y).atan2(perp.dot(dir));
            if !p.ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&turn)) {
                continue;
            }
            let same = |j: &usize| {
                let o = &self.edges[*j];
                (o.p0.distance(e.p0) <= 0.001 && o.p1.distance(e.p1) <= 0.001) || (o.p0.distance(e.p1) <= 0.001 && o.p1.distance(e.p0) <= 0.001)
            };
            if !cands.iter().any(same) {
                cands.push(i);
            }
        }
        let mut chains = Vec::new();
        while let Some(seed) = cands.pop() {
            let e = &self.edges[seed];
            let mut links = std::collections::VecDeque::from([(seed, e.p0, e.p1)]);
            // the front, then the back
            loop {
                let front = links.front().unwrap().1;
                let Some(k) = cands.iter().position(|&j| self.edges[j].p0.distance(front) < CHAIN_LINK_TOL || self.edges[j].p1.distance(front) < CHAIN_LINK_TOL) else { break };
                let j = cands.remove(k);
                let o = &self.edges[j];
                links.push_front(if o.p1.distance(front) < CHAIN_LINK_TOL { (j, o.p0, o.p1) } else { (j, o.p1, o.p0) });
            }
            loop {
                let back = links.back().unwrap().2;
                let Some(k) = cands.iter().position(|&j| self.edges[j].p0.distance(back) < CHAIN_LINK_TOL || self.edges[j].p1.distance(back) < CHAIN_LINK_TOL) else { break };
                let j = cands.remove(k);
                let o = &self.edges[j];
                links.push_back(if o.p0.distance(back) < CHAIN_LINK_TOL { (j, o.p0, o.p1) } else { (j, o.p1, o.p0) });
            }
            chains.push(Chain { links: links.into_iter().collect() });
        }
        chains
    }

    /// `GuidanceChain__FindGrabPoint` 0x66E150 with no preferred direction and hand offsets `offsets`, on a chain of a
    /// report whose edges were cut `cuts` (per edge: at p0, at p1; empty = none). The chain's straight line from its
    /// start to its end at the target's height, length L. Each offset less the cut at that end (s_i = o − cut_start,
    /// e_i = o − cut_end) must leave room: s_i + e_i + 0.0005 < L, or the chain cannot take the hands. The line is
    /// trimmed by the largest s_i / e_i (it grows where they are negative: the edge continues past the cut), then by
    /// `margin` less the positive trim; the target, along the line, is clamped onto that piece (its middle when none is
    /// left). That point is carried onto the links by the plane across the line (the chain's two free link ends pulled
    /// in by the positive trims, `sub_9765E0`); when no link meets the plane, the link end nearest along the line.
    pub fn grab_point(&self, chain: &Chain, target: Vec3, offsets: &[f32], margin: f32, cuts: &[(f32, f32)]) -> Option<GuidanceHit> {
        self.grab_point_toward(chain, target, None, None, offsets, margin, cuts)
    }

    /// `grab_point` with a preferred direction (the jumper's facing in 0xE165D0): the chain is measured across it (its end
    /// carried onto the vertical plane through its start whose normal is `pref`), so the grab point lies straight along
    /// `pref` from the target. `along` (a5): the target is carried onto the chain's vertical plane along this direction
    /// before the clamp (`Vec_ProjectOntoPlane` 0x9470B0; the air catches pass their box forward, RE/24 GUIDE-9); without
    /// it, perpendicularly.
    #[allow(clippy::too_many_arguments)]
    pub fn grab_point_toward(&self, chain: &Chain, target: Vec3, pref: Option<Vec3>, along: Option<Vec3>, offsets: &[f32], margin: f32, cuts: &[(f32, f32)]) -> Option<GuidanceHit> {
        let cut_at = |edge: usize, p: Vec3| {
            let c = cuts.get(edge).copied().unwrap_or((0.0, 0.0));
            let e = &self.edges[edge];
            if p.distance_squared(e.p0) <= p.distance_squared(e.p1) { c.0 } else { c.1 }
        };
        let (first, last) = (chain.links[0], chain.links[chain.links.len() - 1]);
        let (cut_s, cut_e) = (cut_at(first.0, first.1), cut_at(last.0, last.2));
        let (s, mut e) = (chain.start().with_y(target.y), chain.end().with_y(target.y));
        if let Some(p) = pref.map(|p| p.with_y(0.0).normalize_or_zero()).filter(|p| *p != Vec3::ZERO) {
            e -= p * (e - s).dot(p);
        }
        let d = e - s;
        if Vec3::new(d.x, 0.0, d.z).length() <= 0.0005 {
            return None;
        }
        let len = d.length();
        let u = d / len;
        let (mut trim_s, mut trim_e) = (-1000.0f32, -1000.0f32);
        for &o in offsets {
            let (si, ei) = (o - cut_s, o - cut_e);
            if si + ei + 0.0005 >= len {
                return None;
            }
            trim_s = trim_s.max(si);
            trim_e = trim_e.max(ei);
        }
        let a0 = s + u * trim_s;
        let b0 = e - u * trim_e;
        let span = (b0 - a0).length();
        let v = (b0 - a0).normalize_or_zero();
        let lo = a0 + v * (margin - trim_s.max(0.0)).min(span);
        let hi = b0 - v * (margin - trim_e.max(0.0)).min(span);
        // the target carried onto the chain's vertical plane along `along` (0x9470B0: p + t·dir, t = −((p − s)·n) / (dir·n))
        let projected = {
            let p = target.with_y(s.y);
            let n = Vec3::new(-u.z, 0.0, u.x).normalize_or_zero();
            match along.map(|a| a.with_y(0.0).normalize_or_zero()).filter(|a| a.dot(n).abs() > 1e-6) {
                Some(a) => p + a * (-(p - s).dot(n) / a.dot(n)),
                None => p,
            }
        };
        let q = if (b0 - a0).dot(hi - lo) > 0.0 { closest_on_segment(lo, hi, projected) } else { (lo + hi) * 0.5 };
        let along = Vec3::new(u.x, 0.0, u.z).normalize_or_zero();
        let n = chain.links.len();
        let mut nearest: Option<(f32, Vec3, usize)> = None;
        for (k, &(edge, mut a, mut b)) in chain.links.iter().enumerate() {
            let ld = (b - a).normalize_or_zero();
            if k == 0 {
                a += ld * trim_s.max(0.0);
            }
            if k == n - 1 {
                b -= ld * trim_e.max(0.0);
            }
            let (da, db) = ((a - q).dot(along), (b - q).dot(along));
            if da * db <= 0.0 && (da - db).abs() > 1e-9 {
                let p = a + (b - a) * (da / (da - db));
                return Some(self.hit_on(edge, p));
            }
            for (dist, p) in [(da.abs(), a), (db.abs(), b)] {
                if nearest.as_ref().is_none_or(|x| dist < x.0) {
                    nearest = Some((dist, p, edge));
                }
            }
        }
        nearest.map(|(_, p, edge)| self.hit_on(edge, p))
    }

    fn hit_on(&self, edge: usize, point: Vec3) -> GuidanceHit {
        let e = &self.edges[edge];
        let low = if e.n1.y <= e.n0.y { e.n1 } else { e.n0 };
        GuidanceHit { point, edge, wall_normal: Vec3::new(low.x, 0.0, low.z).normalize_or_zero(), dir: (e.p1 - e.p0).normalize_or_zero() }
    }

    /// `HumanInAir__FindHandholdPair` 0xE0A150: the edges cut to a box zone (centre, facing `forward`, half extents
    /// `half`), their chains (−forward, the walls turned within ±`angle`, edges within `angle` of the right axis,
    /// `mask`), and for each hand reference (`refs`, in the box frame: right, forward, up) the grab point nearest it over
    /// all chains (`sub_E07820`: hand offsets 0.5 / 0.1, `margin`). Both hands must find one.
    pub fn handhold_pair(&self, centre: Vec3, forward: Vec3, half: Vec3, refs: [Vec3; 2], angle: f32, margin: f32, mask: u32) -> Option<[GuidanceHit; 2]> {
        let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let right = forward.cross(Vec3::Y);
        let report = self.report_indexed(centre, forward, -half, half, true);
        let chains = report.world.chains(&ChainParams { dir: -forward, right, ranges: [(-angle, 0.0), (0.0, angle)], parallel: angle, mask });
        let world = |l: Vec3| centre + right * l.x + forward * l.y + Vec3::Y * l.z;
        let grab = |r: Vec3| {
            chains
                .iter()
                .filter_map(|c| report.world.grab_point_toward(c, r, None, Some(forward), &[0.5, 0.1], margin, &report.cuts))
                .min_by(|a, b| a.point.distance_squared(r).total_cmp(&b.point.distance_squared(r)))
        };
        let (a, b) = (grab(world(refs[0]))?, grab(world(refs[1]))?);
        let to_world = |h: GuidanceHit| GuidanceHit { edge: report.source[h.edge], ..h };
        Some([to_world(a), to_world(b)])
    }
}

/// A guidance report: the edges a zone query kept, each one's index in the queried world and the lengths cut off its
/// ends (p0, p1).
#[derive(Default)]
pub struct Report {
    pub world: GuidanceWorld,
    pub source: Vec<usize>,
    pub cuts: Vec<(f32, f32)>,
}

/// The type mask of the air catches' hand searches (306 = 0x132: LedgeGrab, Pole, Rope, Kiosk; 0xE0A990 / 0xE0AC70 /
/// 0xE0A810).
pub const MASK_CATCH: u32 = 306;

/// The edge-piece shortening (`sub_116D3C0`) every Human box probe passes (0.1 m).
pub const PROBE_STEP: f32 = 0.1;

/// Guidance subtype masks (bit `1 << GuidanceObjectSubType`). Every ledge and climb search of the Human contexts
/// passes LedgeGrab, Pole and Rope (mask 50 = 0x32) to the box probe 0x11713A0: the climb grid (BuildHoldGrid
/// 0xDF6A40), the ledge jumps (0xDD5E10, 0xDDD490 via 0xB153C0, 0xDDF390), the air catches (0xDFFE30, 0xE0AC70), the
/// ground climb start (0xB2E860) and the hand-pair search 0xB153C0. The game has no other filter between ledges and
/// climb holds: both are LedgeGrab edges, told apart only by where each search looks (the `dword_1934100` …
/// arguments are debug-draw colours, RE/01 §6). The probe reports a kind: 0 ledge, 1 surface, 2 rope, 3 pole.
pub const MASK_CLIMB_HOLDS: u32 = 50;
/// Ladder edges only (mask 8): the ladder searches (0xDD4CD0, 0xDD55F0, 0xDF10C0, 0xE04100).
pub const MASK_LADDER: u32 = 8;

pub(crate) fn closest_on_segment(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let d = b - a;
    let t = ((p - a).dot(d) / d.length_squared().max(1e-9)).clamp(0.0, 1.0);
    a + d * t
}

/// Cut segment a→b (local coordinates) to the box |x| ≤ half (Liang–Barsky); None when it misses.
pub(crate) fn clip_segment(a: Vec3, b: Vec3, half: Vec3) -> Option<(Vec3, Vec3)> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..3 {
        let (p, v, h) = (a[k], d[k], half[k]);
        if v.abs() < 1e-9 {
            if p.abs() > h {
                return None;
            }
            continue;
        }
        let (mut ta, mut tb) = ((-h - p) / v, (h - p) / v);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return None;
        }
    }
    Some((a + d * t0, a + d * t1))
}

pub struct GuidancePlugin;

impl Plugin for GuidancePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GuidanceWorld>()
            .init_resource::<ShowGuidance>()
            .add_systems(Update, (toggle_guidance, draw_guidance));
    }
}

#[derive(Resource)]
pub struct ShowGuidance(pub bool);
impl Default for ShowGuidance {
    fn default() -> Self {
        Self(true)
    }
}

fn toggle_guidance(keys: Res<ButtonInput<KeyCode>>, mut show: ResMut<ShowGuidance>) {
    if keys.just_pressed(KeyCode::KeyG) {
        show.0 = !show.0;
    }
}

fn draw_guidance(world: Res<GuidanceWorld>, show: Res<ShowGuidance>, mut gizmos: Gizmos) {
    if !show.0 {
        return;
    }
    for e in &world.edges {
        let color = match e.subtype {
            GuidanceSubType::LedgeGrab => Color::srgb(1.0, 0.8, 0.1),
            GuidanceSubType::Beam => Color::srgb(0.2, 0.9, 1.0),
            GuidanceSubType::Pole => Color::srgb(0.9, 0.3, 0.9),
            _ => Color::srgb(0.7, 0.7, 0.7),
        };
        let lift = Vec3::Y * 0.02 + e.n1 * 0.02;
        gizmos.line(e.p0 + lift, e.p1 + lift, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledge(a: Vec3, b: Vec3) -> GuidanceEdge {
        GuidanceEdge { p0: a, p1: b, n0: Vec3::Y, n1: Vec3::NEG_Z, subtype: GuidanceSubType::LedgeGrab }
    }

    #[test]
    fn chains_link_edges_end_to_end_and_keep_the_facing_ones() {
        // two pieces of one ledge (meeting within 0.01 m), a separate ledge, and one facing the other way
        let g = GuidanceWorld {
            edges: vec![
                ledge(Vec3::new(0.0, 2.0, 0.0), Vec3::new(1.0, 2.0, 0.0)),
                ledge(Vec3::new(1.005, 2.0, 0.0), Vec3::new(2.5, 2.0, 0.0)),
                ledge(Vec3::new(4.0, 2.0, 0.0), Vec3::new(5.0, 2.0, 0.0)),
                GuidanceEdge { n1: Vec3::Z, ..ledge(Vec3::new(0.0, 3.0, 0.0), Vec3::new(2.0, 3.0, 0.0)) },
            ],
            ..Default::default()
        };
        // facing +Z (into the -Z faces): walls turned within 70° of -forward
        let a = 70f32.to_radians();
        let chains = g.chains(&ChainParams { dir: Vec3::NEG_Z, right: Vec3::X, ranges: [(-a, 0.0), (0.0, a)], parallel: a, mask: MASK_CATCH });
        assert_eq!(chains.len(), 2, "{chains:?}");
        let long = chains.iter().find(|c| c.links.len() == 2).expect("the two pieces form one chain");
        let ends = [long.start().x, long.end().x];
        assert!(ends.contains(&0.0) && ends.contains(&2.5), "{ends:?}");
    }

    #[test]
    fn the_ledge_report_carves_round_ladders_and_drops_steep_edges() {
        // RE/24 GUIDE-2: sub_116F060's ladder zone (±0.3 across, ±0.75 in front / behind, 0.2 m past the ends) is cut out
        // of every non-ladder edge; edges steeper than 40° are removed; the ladder itself stays
        let ladder = GuidanceEdge { p0: Vec3::new(2.0, 0.0, 0.0), p1: Vec3::new(2.0, 4.0, 0.0), n0: Vec3::Y, n1: Vec3::NEG_Z, subtype: GuidanceSubType::Ladder };
        let steep = ledge(Vec3::new(5.0, 1.0, 0.0), Vec3::new(5.5, 2.0, 0.0));
        let g = GuidanceWorld { edges: vec![ledge(Vec3::new(0.0, 2.0, 0.0), Vec3::new(4.0, 2.0, 0.0)), ladder.clone(), steep], ..Default::default() };
        let r = g.ledge_report(Vec3::new(2.0, 2.0, 0.0), 10.0, true);
        assert!(r.edges.iter().any(|e| e.subtype == GuidanceSubType::Ladder), "the ladder stays");
        let mut xs: Vec<(f32, f32)> = r.edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab).map(|e| (e.p0.x.min(e.p1.x), e.p0.x.max(e.p1.x))).collect();
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(xs.len(), 2, "the ledge is cut in two, the steep edge dropped: {xs:?}");
        assert!((xs[0].1 - 1.7).abs() < 1e-4 && (xs[1].0 - 2.3).abs() < 1e-4, "{xs:?}");
        // the carve-only report (the ladder's side jump) keeps the steep edge
        assert_eq!(g.ledge_report(Vec3::new(2.0, 2.0, 0.0), 10.0, false).edges.len(), 4);
    }

    #[test]
    fn ladder_hits_snap_to_the_nearest_rung() {
        // sub_116DEA0 (RE/24 GUIDE-1): 1.3 m up → 1.5 m (the port used to floor the raw 1.3 to 1.0); 1.2 m → 1.0 m
        let ladder = GuidanceEdge { p0: Vec3::new(0.0, 0.0, 0.0), p1: Vec3::new(0.0, 4.0, 0.0), n0: Vec3::Y, n1: Vec3::NEG_Z, subtype: GuidanceSubType::Ladder };
        assert!((snap_to_rung(&ladder, Vec3::new(0.0, 1.3, 0.0)).y - 1.5).abs() < 1e-6);
        assert!((snap_to_rung(&ladder, Vec3::new(0.0, 1.2, 0.0)).y - 1.0).abs() < 1e-6);
        assert!((snap_to_rung(&ladder, Vec3::new(0.0, 1.25, 0.0)).y - 1.5).abs() < 1e-6, "0.25 rounds up");
    }

    #[test]
    fn the_chain_angle_runs_from_the_edges_perpendicular_to_the_reference() {
        // 0x677B00: Math__SignedAngleAroundAxis(&perp, ref, &up) (RE/24 GUIDE-8). An edge turned +30° about up has its
        // perpendicular 30° past the reference, so the angle back to the reference is −30°
        let r = Quat::from_rotation_y(30f32.to_radians());
        let e = GuidanceEdge { p0: r * Vec3::new(-1.0, 2.0, 0.0), p1: r * Vec3::new(1.0, 2.0, 0.0), n0: Vec3::Y, n1: r * Vec3::NEG_Z, subtype: GuidanceSubType::LedgeGrab };
        let g = GuidanceWorld { edges: vec![e], ..Default::default() };
        let q = |ranges| g.chains(&ChainParams { dir: Vec3::NEG_Z, right: Vec3::X, ranges, parallel: 45f32.to_radians(), mask: MASK_CATCH }).len();
        let a = 45f32.to_radians();
        assert_eq!(q([(-a, 0.0), (0.0, 0.0)]), 1, "−30° is in [−45°, 0]");
        assert_eq!(q([(0.0, 0.0), (0.0, a)]), 0, "and not in [0, 45°]");
    }

    #[test]
    fn the_grab_point_is_carried_along_the_callers_direction() {
        // FindGrabPoint 0x66E150 → Vec_ProjectOntoPlane 0x9470B0 along a5 (the air catches' box forward, RE/24 GUIDE-9)
        let g = GuidanceWorld { edges: vec![ledge(Vec3::new(0.0, 2.0, 0.0), Vec3::new(3.0, 2.0, 0.0))], ..Default::default() };
        let chain = Chain { links: vec![(0, Vec3::new(0.0, 2.0, 0.0), Vec3::new(3.0, 2.0, 0.0))] };
        let target = Vec3::new(1.0, 2.0, -0.5);
        let perp = g.grab_point(&chain, target, &[0.5, 0.1], 0.1, &[]).unwrap();
        assert!((perp.point.x - 1.0).abs() < 1e-4, "{:?}", perp.point);
        let slanted = g.grab_point_toward(&chain, target, None, Some(Vec3::new(1.0, 0.0, 1.0)), &[0.5, 0.1], 0.1, &[]).unwrap();
        assert!((slanted.point.x - 1.5).abs() < 1e-4, "{:?}", slanted.point);
    }

    #[test]
    fn grab_points_keep_the_margin_at_true_ends_but_not_at_cuts() {
        let g = GuidanceWorld { edges: vec![ledge(Vec3::new(0.0, 2.0, 0.0), Vec3::new(3.0, 2.0, 0.0))], ..Default::default() };
        let chain = Chain { links: vec![(0, Vec3::new(0.0, 2.0, 0.0), Vec3::new(3.0, 2.0, 0.0))] };
        // a target past the true start: clamped to the margin (0.1) from it
        let h = g.grab_point(&chain, Vec3::new(-1.0, 2.0, -0.3), &[0.5, 0.1], 0.1, &[]).unwrap();
        assert!((h.point.x - 0.5).abs() < 1e-4, "the hands' 0.5 m offset keeps the free end: {:?}", h.point);
        // the same edge cut 2 m short of its start by the report: the hands may use it
        let h = g.grab_point(&chain, Vec3::new(-1.0, 2.0, -0.3), &[0.5, 0.1], 0.1, &[(2.0, 0.0)]).unwrap();
        assert!(h.point.x.abs() < 1e-4, "the edge continues past the cut: {:?}", h.point);
        // a 0.9 m chain with nothing cut cannot take hands 0.5 m from each end
        let short = Chain { links: vec![(0, Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.9, 2.0, 0.0))] };
        assert!(g.grab_point(&short, Vec3::new(0.4, 2.0, 0.0), &[0.5, 0.1], 0.1, &[]).is_none());
    }

    #[test]
    fn the_vertical_search_takes_the_farthest_edge_and_skips_the_held_one() {
        let g = GuidanceWorld {
            edges: vec![
                ledge(Vec3::new(-1.0, 2.0, 0.0), Vec3::new(1.0, 2.0, 0.0)), // held
                ledge(Vec3::new(-1.0, 2.4, 0.0), Vec3::new(1.0, 2.4, 0.0)),
                ledge(Vec3::new(-1.0, 2.7, 0.0), Vec3::new(1.0, 2.7, 0.0)),
            ],
            ..Default::default()
        };
        let wedge = Some(Wedge { angle: 0.523_598_8, cut_up: 0.0 });
        let h = g.vertical_search(Vec3::new(0.0, 2.0, 0.0), true, Vec3::Z, 0.1, 0.8, 0.4, 0.0, wedge, false, (0.0, 5.0)).unwrap();
        assert!((h.point.y - 2.7).abs() < 1e-4, "the farthest up within 0.1–0.8 m: {:?}", h.point);
        let h = g.vertical_search(Vec3::new(0.0, 2.0, 0.0), true, Vec3::Z, 0.1, 0.5, 0.4, 0.0, wedge, false, (0.0, 5.0)).unwrap();
        assert!((h.point.y - 2.4).abs() < 1e-4, "{:?}", h.point);
    }

    #[test]
    fn the_vertical_wedge_lies_in_the_wall_plane() {
        // RE/24 LEDGE-13: the wedge is |x| ≤ tan 30°·z from the apex in the wall plane; an edge 0.1 m in front of the hands'
        // plane (sticking out above them) is kept, where the old horizontal wedge dropped everything in front of it
        let g = GuidanceWorld { edges: vec![ledge(Vec3::new(-1.0, 2.5, -0.1), Vec3::new(1.0, 2.5, -0.1))], ..Default::default() };
        let wedge = Some(Wedge { angle: 0.523_598_8, cut_up: 0.0 });
        let h = g.vertical_search(Vec3::new(0.0, 2.0, 0.0), true, Vec3::Z, 0.1, 0.8, 0.4, 0.0, wedge, false, (0.0, 5.0));
        let h = h.expect("the edge in front of the hands' plane");
        assert!((h.point.y - 2.5).abs() < 1e-4 && h.point.x.abs() < 1e-4, "{:?}", h.point);
    }
}
