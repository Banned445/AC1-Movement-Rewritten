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

/// Event 42's contact (0xB25230) for the port's capsule: the obstacle face just ahead, ≥ 0.5 m above the feet and
/// within 45° of the facing. Returns (contact on the face at the feet height, outward normal, obstacle height).
/// The event is unreachable from player input in v1.02 (RE/02 §4.5), so only the tests use this. PORT: how the sender
/// measures the height is not traced; this takes the top of the box it touches.
#[allow(dead_code)]
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

/// `HumanGround__ObstacleCollision_Enter` 0xD9CB90.
#[allow(dead_code)]
pub fn enter(feet: Vec3, heading: f32, contact: Vec3, n: Vec3, height: f32, seq: u32) -> Option<Collide> {
    let (kind, lo, hi, id) = if height >= 0.7 * H { (CollideKind::Hand, 0.7 * H, 1.5 * H, COLLIDE_HAND) } else { (CollideKind::Foot, 0.5 * H, 0.7 * H, COLLIDE_FOOT) };
    let h = ((height - lo) / (hi - lo)).clamp(0.0, 1.0);
    let action = blend(id, 0, &[1.0 - h, h])?;
    Some(Collide { kind, h, normal: n, from: feet, to: contact + n * 0.4 * H, phase: CollidePhase::Entry(0), action, t: 0.0, seq: seq.wrapping_add(1), warp_t: 0.0, from_heading: heading })
}

/// What `update` asks the ground context to do.
pub enum CollideOut {
    Stay,
    /// Leave through these actions (root motion), then Movement at `speed` (0 = standing). `face`: the facing the
    /// exit ends in (its clips turn the character away; the port takes the stick direction, clamped to along the
    /// obstacle, since the clips' root yaw is not dumped (hypothesis)), applied when `then` starts.
    Leave { first: Option<ActionBlend>, then: Option<ActionBlend>, speed: f32, face: Option<Vec3> },
}

/// `HumanGround__ObstacleCollision_Update` 0xD9DA20. `stick` = wanted direction (None = released), `jog` = high
/// profile (+1504, hypothesis). Moves `feet` / `heading` during the entry interpolation.
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
    let decide = match c.phase {
        CollidePhase::Entry(i) => {
            if !ended {
                return CollideOut::Stay;
            }
            if c.kind == CollideKind::Hand && i == 0 {
                if let Some(a) = blend(COLLIDE_HAND, 1, &[1.0 - c.h, c.h]) {
                    c.phase = CollidePhase::Entry(1);
                    c.action = a;
                    c.t = 0.0;
                    c.seq = c.seq.wrapping_add(1);
                    return CollideOut::Stay;
                }
            }
            true
        }
        // the wait may be left at any time (its item is interruptible)
        CollidePhase::Wait => true,
    };
    if !decide {
        return CollideOut::Stay;
    }
    let away = c.normal;
    match stick {
        Some(s) => {
            let a = s.dot(away).clamp(-1.0, 1.0).acos();
            if a > 135f32.to_radians() {
                // pushing into the obstacle: lean (the wait again)
                if c.phase != CollidePhase::Wait || ended {
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
