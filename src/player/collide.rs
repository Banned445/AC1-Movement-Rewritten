//! HumanGround ObstacleCollision (sub-state 5): running into an obstacle at least 0.5 m high (RE/02 §4.2).
//!
//! The character controller's collision event **42** (guard 0xB25230: vertical speed ≤ 0.2 m/s, a contact within
//! 45° of the facing whose point is ≥ 0.5 m above the feet) → `HumanGround__ObstacleCollision_Enter` 0xD9CB90:
//! - obstacle height ≥ 0.7·h → the hand type (+220 = 0): `collide_full_hand_{070,150}cm` then its transition into
//!   the two-hand **lean** (`lean_040cm_twohand_{070,150}cm_wait`), blended by (height − 0.7h) / (1.5h − 0.7h);
//! - lower → the foot type (+220 = 1): `collide_full_footl_{050,070}cm`, blended by (height − 0.5h) / (0.7h − 0.5h);
//! - the root is interpolated over 0.15 s to the contact + 0.4·h along the normal, facing the obstacle.
//!
//! `HumanGround__ObstacleCollision_Update` 0xD9DA20 (each tick, decisions when the action ends or may be left):
//! - the stick more than 135° from "away from the wall" (pushing into it) → keep leaning (the wait again);
//! - otherwise leave along the stick: the left (signed angle ≥ 0) or right exit, [back, side] blended by
//!   min(|angle|, 90°) / 90°, × [70, 150 cm] × [walk, jog] (8 clips, hand) or [back, side] × [50, 70 cm] (4 clips,
//!   foot, run), then (hand) the exit's own transition into the walk / jog → Movement;
//! - no stick → `lean_*_wait_tr_{l,h}_wait_hipm_footr` → Movement standing.

use bevy::prelude::*;

use super::jump_blend::{self, ActionBlend};
use crate::collision::CollisionWorld;

pub const COLLIDE_HAND: u32 = 0x0121_B894; // items: collide_full_hand_{070,150}cm, …_tr_h_lean_025cm_twohand_*_wait
pub const LEAN_WAIT: u32 = 0x0127_39B6; // lean_040cm_twohand_{070,150}cm_wait
/// Exits [left (angle ≥ 0), right]: 8 clips [back 70, back 150, side 70, side 150] × [walk, jog].
pub const LEAN_EXIT: [u32; 2] = [0x0121_B895, 0x0127_10F0];
/// The exits' transitions into locomotion: [back walk, side walk, back jog, side jog].
pub const LEAN_EXIT_TR: [u32; 2] = [0x0129_1741, 0x0129_1742];
/// No stick: back to the wait, low / high profile (2 clips [70, 150]).
pub const LEAN_TO_WAIT: [u32; 2] = [0x0127_3AA1, 0x0127_3AA2];
pub const COLLIDE_FOOT: u32 = 0x012B_2919; // items a / b: collide_full_footl_{050,070}cm_{a,b}
pub const COLLIDE_FOOT_WAIT: u32 = 0x012B_2D19;
/// Foot exits [left, right]: 4 clips [back 50, back 70, side 50, side 70] (run).
pub const COLLIDE_FOOT_EXIT: [u32; 2] = [0x012D_81AB, 0x012D_81AC];

pub const DUMPED_ACTIONS: &[u32] = &[
    COLLIDE_HAND, LEAN_WAIT, LEAN_EXIT[0], LEAN_EXIT[1], LEAN_EXIT_TR[0], LEAN_EXIT_TR[1], LEAN_TO_WAIT[0], LEAN_TO_WAIT[1],
    COLLIDE_FOOT, COLLIDE_FOOT_WAIT, COLLIDE_FOOT_EXIT[0], COLLIDE_FOOT_EXIT[1],
];

/// Entity+0x7C (character height scale, 1 for Altaïr).
const H: f32 = 1.0;
/// 0xD9CB90: the root interpolation time.
const ENTRY_WARP: f32 = 0.15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollideKind {
    Hand,
    Foot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollidePhase {
    /// The collide item (0), then (hand) its transition into the lean (item 1).
    Entry(usize),
    Wait,
}

#[derive(Clone, Copy, Debug)]
pub struct Collide {
    pub kind: CollideKind,
    /// Height blend (70 ↔ 150 cm or 50 ↔ 70 cm).
    pub h: f32,
    /// Obstacle normal (horizontal, out of it).
    pub normal: Vec3,
    pub from: Vec3,
    pub to: Vec3,
    pub phase: CollidePhase,
    pub action: ActionBlend,
    pub t: f32,
    pub seq: u32,
    /// Time since the collision started: the 0.15 s root interpolation runs once (the action timer `t` restarts
    /// with every item; driving the warp with it re-warped from the first contact point at each item change, a
    /// visible snap back every ~0.1 s while leaning).
    pub warp_t: f32,
    /// Heading when the collision started (turned to face the obstacle over the warp, not in one frame).
    pub from_heading: f32,
}

impl Collide {
    pub fn current(&self) -> (ActionBlend, f32) {
        let ph = self.t / self.action.duration().max(1e-4);
        (self.action, if self.phase == CollidePhase::Wait { ph.fract() } else { ph.min(1.0) })
    }
}

fn blend(id: u32, item: usize, w: &[f32]) -> Option<ActionBlend> {
    let n = jump_blend::action_items(id)?.get(item)?.len();
    let mut v = vec![0.0; n];
    v[..w.len().min(n)].copy_from_slice(&w[..w.len().min(n)]);
    Some(ActionBlend::new(id, item, &v))
}

/// A box face just ahead, ≥ 0.5 m above the feet and within 45° of the facing: (contact on the face at the feet height,
/// outward normal, the box's height). The tests send event 42 with it; the game's sender is `obstacle_query`.
pub fn obstacle_ahead(feet: Vec3, forward: Vec3, collision: &CollisionWorld) -> Option<(Vec3, Vec3, f32)> {
    let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let probe = feet + f * (crate::tuning::CAPSULE_RADIUS + 0.08) + Vec3::Y * 0.55;
    let b = collision.boxes.iter().filter(|b| probe.cmpge(b.min).all() && probe.cmple(b.max).all()).max_by(|a, b| a.max.y.total_cmp(&b.max.y))?;
    let out = feet + Vec3::Y * 0.55;
    let c = out.clamp(b.min, b.max);
    let n = Vec3::new(out.x - c.x, 0.0, out.z - c.z).normalize_or_zero();
    if n == Vec3::ZERO || (-n).dot(f) < 45f32.to_radians().cos() {
        return None;
    }
    let height = b.max.y - feet.y;
    (height >= 0.5).then_some((Vec3::new(c.x, feet.y, c.z), n, height))
}

/// What the obstacle query found (IHuman vt152 `Human__ObstacleQuery` 0xB27900): the contact at the feet's height, the
/// flattened normal toward the character, the obstacle's height above the feet, and whether a guidance edge made it
/// (out flag 0) or a plain obstacle (flag 1).
#[derive(Clone, Copy, Debug)]
pub struct Obstacle {
    pub contact: Vec3,
    pub normal: Vec3,
    pub height: f32,
    pub guided: bool,
}

/// `Human__ObstacleQuery` 0xB27900 along `dir` (flat), reach `reach` (the event-42 sender passes 0.6): the guidance
/// obstacle first (`Human__ObstacleQueryGuided` 0xB253D0), else a plain one (`Human__ObstacleQueryPlain` 0xB25C80).
/// Live (RE/25 GROUND-32): the Bureau crate gave flag 0, contact 0.515 m ahead, height 1.16; the 4.1 m wall flag 1,
/// contact 0.466 m ahead, height 1.7.
pub fn obstacle_query(feet: Vec3, dir: Vec3, reach: f32, guidance: &crate::guidance::GuidanceWorld, collision: &CollisionWorld) -> Option<Obstacle> {
    let f = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if f == Vec3::ZERO {
        return None;
    }
    if let Some((contact, normal, height)) = guided_obstacle(feet, f, reach, guidance, collision) {
        return Some(Obstacle { contact, normal, height, guided: true });
    }
    plain_obstacle(feet, f, reach, collision).map(|(contact, normal, height)| Obstacle { contact, normal, height, guided: false })
}

/// 0xB253D0:
/// - a `GuidanceZonePlane` (0x658780) at the feet + 0.5 m: the rectangle 0 … `reach` along the stick and 0 … 1.7 m up
///   in the vertical plane through the stick (`sub_658B60`, the edge test `sub_979A50`: the edge crosses that plane
///   inside the rectangle);
/// - of its edges, not Quadruped, not steep, the wall normal facing the character (dot with the stick < −0.7071) and
///   more than 0.45 m of drop behind (`Human__MeasureDropBeyondEdge` 0.85 / 0.02), the nearest (flat, from the edge's
///   closest point);
/// - a ray from the root at the edge's height along the stick to 0.01 m short of where the edge crosses the root's
///   vertical plane: clear → a 0.5 m ray from there up and over the edge (the normal turned 135° about normal × up)
///   must be clear too, then the edge is the obstacle (height = edge − feet); blocked → that hit, height 1.7.
/// PORT: the hit entity's class tests (`sub_E1C1F0`, the sender's 0x8F0FDB1C) are not ported (no entities).
fn guided_obstacle(feet: Vec3, f: Vec3, reach: f32, guidance: &crate::guidance::GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3, f32)> {
    use crate::guidance::GuidanceSubType;
    use super::jump_candidates::{measure_drop, wall_of};
    let origin = feet + Vec3::Y * 0.5;
    let side = f.cross(Vec3::Y);
    let crossing = |p0: Vec3, p1: Vec3, o: Vec3| {
        let d = p1 - p0;
        let den = d.dot(side);
        if den.abs() <= 0.0005 {
            return None;
        }
        let t = (o - p0).dot(side) / den;
        (0.0..=1.0).contains(&t).then(|| p0 + d * t)
    };
    let mut best: Option<(f32, usize)> = None;
    for (i, e) in guidance.edges.iter().enumerate() {
        if e.subtype == GuidanceSubType::Quadruped || crate::guidance::steep(e.p1 - e.p0) {
            continue;
        }
        let n = wall_of(e.n0, e.n1);
        if n.dot(f) >= -std::f32::consts::FRAC_1_SQRT_2 {
            continue;
        }
        let Some(x) = crossing(e.p0, e.p1, origin) else { continue };
        let (a, b) = ((x - origin).dot(f), x.y - origin.y);
        if !(0.0..=reach).contains(&a) || !(0.0..=1.7).contains(&b) {
            continue;
        }
        let c = e.closest_point(origin);
        let dist = Vec2::new(c.x - origin.x, c.z - origin.z).length_squared();
        if best.is_some_and(|(d, _)| d <= dist) || measure_drop(c, n, 0.85, 0.02, collision) <= 0.45 {
            continue;
        }
        best = Some((dist, i));
    }
    let e = &guidance.edges[best?.1];
    let n = wall_of(e.n0, e.n1);
    // where the edge crosses the vertical plane through the root along the stick (`sub_9765E0`)
    let d = e.p1 - e.p0;
    let den = d.dot(side);
    let p = if den.abs() < 1e-6 { e.closest_point(feet) } else { e.p0 + d * ((feet - e.p0).dot(side) / den).clamp(0.0, 1.0) };
    let from = Vec3::new(feet.x, p.y, feet.z);
    let len = (Vec2::new(p.x - feet.x, p.z - feet.z).length() - 0.01).max(0.0);
    match collision.ray_hit(from, f, len, crate::layers::MAIN_CHARACTER) {
        None => {
            let over = -n * std::f32::consts::FRAC_1_SQRT_2 + Vec3::Y * std::f32::consts::FRAC_1_SQRT_2;
            collision.ray_hit(from + f * len, over, 0.5, crate::layers::MAIN_CHARACTER).is_none().then(|| (Vec3::new(p.x, feet.y, p.z), n, p.y - feet.y))
        }
        Some((t, hn)) => {
            let hit = from + f * t;
            let hn = Vec3::new(hn.x, 0.0, hn.z).normalize_or_zero();
            (hn != Vec3::ZERO).then(|| (Vec3::new(hit.x, feet.y, hit.z), hn, 1.7))
        }
    }
}

/// 0xB25C80: rays along the stick from the root 0.34, 0.68, 1.02 and 1.36 m up, `reach` long (`sub_1171610`, the
/// nearest hit that is not the character); the nearest (flat) gives the contact at the feet's height and its flattened
/// normal, height 1.7.
fn plain_obstacle(feet: Vec3, f: Vec3, reach: f32, collision: &CollisionWorld) -> Option<(Vec3, Vec3, f32)> {
    let mut best: Option<(f32, Vec3, Vec3)> = None;
    for k in 1..=4 {
        let o = feet + Vec3::Y * (0.34 * k as f32);
        if let Some((t, n)) = collision.ray_hit(o, f, reach, crate::layers::MAIN_CHARACTER) {
            if best.is_none_or(|(b, _, _)| t < b) {
                best = Some((t, o + f * t, n));
            }
        }
    }
    let (_, hit, n) = best?;
    let n = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    (n != Vec3::ZERO).then(|| (Vec3::new(hit.x, feet.y, hit.z), n, 1.7))
}

/// Event 42's guard (`HumanGround__Guard_Event42_Collision` 0xD838F0 → `Human__GetObstacleContact` 0xB25230): a wall
/// contact of the controller's manifold (|normal.y| < 0.707) within 45° of `dir`. PORT: the 0.5 m minimum height of
/// the contact point is left to the query (the port's capsule contacts sit where the shapes touch).
pub fn has_obstacle_contact(manifold: &[crate::proxy::Contact], dir: Vec3) -> bool {
    let f = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    manifold.iter().any(|c| c.normal.y.abs() < std::f32::consts::FRAC_1_SQRT_2 && Vec3::new(c.normal.x, 0.0, c.normal.z).normalize_or_zero().dot(-f) >= std::f32::consts::FRAC_1_SQRT_2)
}

/// `HumanGround__ObstacleCollision_Enter` 0xD9CB90. The game also turns the step offset off
/// (`CharacterController__SetStepOffsetEnabled(0)`, RE/24 GUIDE-20); the port's state moves the root by its warp and never
/// runs the capsule, so there is nothing to switch.
pub fn enter(feet: Vec3, heading: f32, contact: Vec3, n: Vec3, height: f32, seq: u32) -> Option<Collide> {
    let (kind, lo, hi, id) = if height >= 0.7 * H { (CollideKind::Hand, 0.7 * H, 1.5 * H, COLLIDE_HAND) } else { (CollideKind::Foot, 0.5 * H, 0.7 * H, COLLIDE_FOOT) };
    let h = ((height - lo) / (hi - lo)).clamp(0.0, 1.0);
    let action = blend(id, 0, &[1.0 - h, h])?;
    Some(Collide { kind, h, normal: n, from: feet, to: contact + n * 0.4 * H, phase: CollidePhase::Entry(0), action, t: 0.0, seq: seq.wrapping_add(1), warp_t: 0.0, from_heading: heading })
}

/// The wanted direction while leaning: the interpreter's wall slide (0xEDD7C0) against the leaned-on wall. A stick
/// facing the wall (its contact within 90°) and more than 45° off straight into it turns along the wall (so W + D with
/// the camera square to the wall pushes away sideways); within 45° it points straight in (keep leaning); a stick away
/// from the wall finds no contact and stays as it is (the back exit). PORT (hypothesis): the slide runs during the lean
/// (its controller contact is the leaned-on wall); a keyboard diagonal sits on the 135° line, taken as sliding.
pub fn lean_stick(stick: Vec3, normal: Vec3) -> Vec3 {
    let d = Vec3::new(stick.x, 0.0, stick.z).normalize_or_zero();
    let n = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
    if d == Vec3::ZERO || n == Vec3::ZERO || (-d).dot(n) < 0.0 {
        return stick;
    }
    if n.dot(d).clamp(-1.0, 1.0).acos() <= 135f32.to_radians() + 1e-3 {
        (d - n * d.dot(n)).normalize_or(d)
    } else {
        -n
    }
}

/// What `update` asks the ground context to do.
pub enum CollideOut {
    Stay,
    /// Leave through these actions (root motion), then Movement at `speed` (0 = standing). `face`: the facing the
    /// exit ends in, applied when `then` starts: only a fallback for an exit whose clips carry no root yaw (the dumped
    /// exits turn the body by theirs, back π / side π/2, in the exit's start frame; ground.rs `face_next`); the port
    /// takes the stick direction, clamped to along the obstacle.
    Leave { first: Option<ActionBlend>, then: Option<ActionBlend>, speed: f32, face: Option<Vec3> },
}

/// `HumanGround__ObstacleCollision_Update` 0xD9DA20. `stick` = the wanted direction DestHeading (Data+80), i.e. the
/// stick after the interpreter's wall slide (`lean_stick`), None = released; `jog` = high profile (+1504, hypothesis).
/// Moves `feet` / `heading` during the entry interpolation.
pub fn update(c: &mut Collide, dt: f32, stick: Option<Vec3>, jog: bool, feet: &mut Vec3, heading: &mut f32) -> CollideOut {
    c.t += dt;
    c.warp_t += dt;
    let k = (c.warp_t / ENTRY_WARP).min(1.0);
    *feet = c.from.lerp(c.to, k);
    let want = super::heading_of(-c.normal);
    let mut d = (want - c.from_heading) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    } else if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    *heading = c.from_heading + d * k;
    let ended = c.t >= c.action.duration();
    // the collide item (0) runs into its transition into the lean (item 1) inside the one action
    if ended && c.kind == CollideKind::Hand && c.phase == CollidePhase::Entry(0) {
        if let Some(a) = blend(COLLIDE_HAND, 1, &[1.0 - c.h, c.h]) {
            c.phase = CollidePhase::Entry(1);
            c.action = a;
            c.t = 0.0;
            c.seq = c.seq.wrapping_add(1);
            return CollideOut::Stay;
        }
    }
    // 0xD9DAB1: the decisions run when the action ends this frame or the playing item is not locked (word 0x20). The
    // collide items are locked (0x35); the transition into the lean (item 1) and the lean wait are not (0xFC5), so the
    // character can push away or stand up as soon as the hands reach the wall.
    if !ended && super::anim_gate::locked(&c.action) {
        return CollideOut::Stay;
    }
    let away = c.normal;
    match stick {
        Some(s) => {
            let a = s.dot(away).clamp(-1.0, 1.0).acos();
            if a > 135f32.to_radians() {
                // pushing into the obstacle: the lean wait again when the action ends (0xD9E0F5), else keep playing
                if ended {
                    let id = if c.kind == CollideKind::Hand { LEAN_WAIT } else { COLLIDE_FOOT_WAIT };
                    if let Some(w) = blend(id, 0, &[1.0 - c.h, c.h]) {
                        c.phase = CollidePhase::Wait;
                        c.action = w;
                        c.t = 0.0;
                        c.seq = c.seq.wrapping_add(1);
                    }
                }
                return CollideOut::Stay;
            }
            // leave along the stick: left exit when the stick points to the character's left
            let facing = -away;
            let left = s.dot(super::right_of(facing)) < 0.0;
            let side = (a.min(90f32.to_radians()) / 90f32.to_radians()).clamp(0.0, 1.0);
            let h = c.h;
            let q = [(1.0 - side) * (1.0 - h), (1.0 - side) * h, side * (1.0 - h), side * h];
            let idx = (!left) as usize;
            let along = super::right_of(facing) * if left { -1.0 } else { 1.0 };
            let face = Some(if a <= 90f32.to_radians() { Vec3::new(s.x, 0.0, s.z).normalize_or(along) } else { along });
            match c.kind {
                CollideKind::Hand => {
                    let j = if jog { 1.0 } else { 0.0 };
                    let w: Vec<f32> = q.iter().map(|x| x * (1.0 - j)).chain(q.iter().map(|x| x * j)).collect();
                    let tr = [(1.0 - side) * (1.0 - j), side * (1.0 - j), (1.0 - side) * j, side * j];
                    CollideOut::Leave { first: blend(LEAN_EXIT[idx], 0, &w), then: blend(LEAN_EXIT_TR[idx], 0, &tr), speed: if jog { 0.5 } else { 0.25 }, face }
                }
                CollideKind::Foot => CollideOut::Leave { first: blend(COLLIDE_FOOT_EXIT[idx], 0, &q), then: None, speed: 0.75, face },
            }
        }
        None => {
            let then = if c.kind == CollideKind::Hand { blend(LEAN_TO_WAIT[jog as usize], 0, &[1.0 - c.h, c.h]) } else { None };
            CollideOut::Leave { first: then, then: None, speed: 0.0, face: None }
        }
    }
}
