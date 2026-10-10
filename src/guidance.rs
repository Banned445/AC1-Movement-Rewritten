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
    /// Closest LedgeGrab point to `p` within `radius` horizontally and `vtol` vertically, on an edge
    /// whose slope is ≤ 40° (RE/06 post-filter) and whose wall faces `facing` within `max_angle`
    /// (facing = direction the character looks, i.e. into the wall).
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
            .filter(|(_, e)| e.subtype == GuidanceSubType::LedgeGrab && e.n1.dot(n) > 0.9)
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
    /// vslot 10 mode 2), then the piece is cut to the two vertical half-spaces through start + lo (up) / hi (down) whose
    /// normals are the facing (up) or its opposite (down) turned ±(90° − angle) about the vertical. The wall (the lower
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
                    // the facing in the local frame (right, forward, up): +y going up, −y going down
                    let f = if up { Vec3::Y } else { Vec3::NEG_Y };
                    let turn = std::f32::consts::FRAC_PI_2 - w.angle;
                    let mut ok = true;
                    for s in [1.0f32, -1.0] {
                        // the facing turned about the vertical in the (x, y) plane of the local frame
                        let (sn, cs) = (turn * s).sin_cos();
                        let n = Vec3::new(f.x * cs - f.y * sn, f.x * sn + f.y * cs, 0.0);
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
