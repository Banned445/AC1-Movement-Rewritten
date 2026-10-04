//! Jump-target selection — reduced port of the candidate scorer 0xE96BF0 (RE/01 §7b).
//!
//! Game rules used here:
//! - candidates within a 45° cone of the wanted direction, height difference dz > -3 m;
//! - per target type, height/distance bands from Human__ComputeJumpAnimBlend 0xB1EC40:
//!   roof edges (free-step type 1): max up 1.3 m, far 7 m; ledges (hang targets, flag 0x40): max up 3.0 m, far 8 m;
//! - among candidates in front, prefer the **highest**, ties → nearest.
//! Simplification: candidates are points on LedgeGrab edges that face the player. Ground targets
//! land `LAND_INSET` onto the roof; ledge targets hang from the edge (aim = hang root).

use bevy::prelude::*;

use super::ledge::{hang_root, LedgeHangType};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceSubType, GuidanceWorld};
use crate::tuning::*;

#[derive(Clone, Copy, Debug)]
pub struct JumpTarget {
    /// Where the jump ends (feet / root).
    pub position: Vec3,
    /// Target type flags as in HumanInAirData+0x290 (1 = free-step / roof edge, 0x40 = ledge hang).
    pub type_flags: u32,
    /// For hang targets: hand midpoint on the edge and the edge's wall normal.
    pub hang: Option<(Vec3, Vec3)>,
    /// Standing straight jump at a hand target (0xB21DA0): its band (flight, weights, arrival).
    pub straight: Option<super::ledge_moves::HangJumpIn>,
    /// Pass-over target (type 2): the near edge point on the wall top and its outward normal.
    pub pass: Option<(Vec3, Vec3)>,
}

/// Roof-edge landing spot: the game's free-step target type 1 (narrow object / edge; its flight is
/// `xx_h_air_*_to_freestep` and arrival plays the free-step reception, 0xB1EC40 / 0xE07D00). 0x8000 is the
/// air-assassination target (its flight is `…_to_assassinate`), not plain ground.
pub const TARGET_GROUND: u32 = super::jump_blend::TARGET_FREESTEP;
/// Ledge hang targets: 0x40 = wall hang (flight `…_to_surface`, wall reception), 0x80 = free hang (flight
/// `…_to_swing`, swing reception) (0xB1EC40 / 0xE07D00).
pub const TARGET_LEDGE: u32 = 0x40;
/// PORT: thickest wall top offered as a pass-over target (type 2).
pub const PASSOVER_MAX_THICKNESS: f32 = 1.0;
pub const TARGET_LEDGE_FREE: u32 = 0x80;

pub fn find_jump_target(
    feet: Vec3,
    want_dir: Vec3,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<JumpTarget> {
    let want = Vec3::new(want_dir.x, 0.0, want_dir.z).normalize_or_zero();
    if want == Vec3::ZERO {
        return None;
    }
    let mut best: Option<(JumpTarget, f32, f32)> = None; // (target, dz, dist)
    // PORT: the game's guidance candidates for beams and pilotis (IHuman vt56/64/68) are not traced; the port
    // offers free-step targets (type 1) on them: the beam line (≥ 0.3 m from its ends) and the pilotis tops
    // (0xB2B600), whose arrivals mount them (0xE07D00 → 0xE50190 / 0xE52AD0).
    let mut pilotis: Vec<Vec3> = Vec::new();
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::LedgeGrab {
            continue;
        }
        let mid = (e.p0 + e.p1) * 0.5;
        let into = -Vec3::new(e.n1.x, 0.0, e.n1.z);
        if let Some(top) = super::narrow::find_pilotis(mid, mid, into, false, guidance, collision) {
            if !pilotis.iter().any(|p| (*p - top).length() < 0.1) {
                pilotis.push(top);
            }
        }
    }
    let mut narrow: Vec<Vec3> = pilotis.clone();
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::Beam {
            continue;
        }
        let len = (e.p1 - e.p0).length();
        if len < 0.6 {
            continue;
        }
        let ahead = feet + want * 4.0;
        let q = e.closest_point(Vec3::new(ahead.x, e.p0.y, ahead.z));
        let u = ((q - e.p0).length() / len).clamp(0.3 / len, 1.0 - 0.3 / len);
        narrow.push(e.p0.lerp(e.p1, u));
    }
    for pos in narrow {
        let target = JumpTarget { position: pos, type_flags: TARGET_GROUND, hang: None, straight: None, pass: None };
        let flat = Vec3::new(pos.x - feet.x, 0.0, pos.z - feet.z);
        let dist = flat.length();
        let dz = pos.y - feet.y;
        if !(0.6..=GROUND_FAR).contains(&dist) || dz > GROUND_MAX_UP || dz < TARGET_MIN_DZ {
            continue;
        }
        if flat.normalize().dot(want).clamp(-1.0, 1.0).acos() > TARGET_CONE {
            continue;
        }
        // a gap must lie between (not the beam / post we stand on)
        let low = feet.y.min(pos.y);
        let crosses_gap = [0.25f32, 0.5, 0.75].iter().any(|&t| {
            let s = feet.lerp(pos, t);
            collision.ground_height(Vec3::new(s.x, low + 0.05, s.z), 0.5).is_none()
        });
        if !crosses_gap {
            continue;
        }
        let better = match &best {
            None => true,
            Some((_, bdz, bd)) => dz > bdz + 0.25 || ((dz - bdz).abs() <= 0.25 && dist < *bd),
        };
        if better {
            best = Some((target, dz, dist));
        }
    }
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::LedgeGrab {
            continue;
        }
        // a pilotis' own edges are not roof edges
        if pilotis.iter().any(|p| Vec2::new(e.p0.x - p.x, e.p0.z - p.z).length() < 0.6 && (e.p0.y - p.y).abs() < 0.05) {
            continue;
        }
        // the edge must face us (its wall normal points back toward the player)
        let to_player = Vec3::new(feet.x - e.p0.x, 0.0, feet.z - e.p0.z);
        if e.n1.dot(to_player) <= 0.0 {
            continue;
        }
        let ahead = feet + want * 4.0;
        let on_edge = e.closest_point(Vec3::new(ahead.x, e.p0.y, ahead.z));
        let edge_dz = on_edge.y - feet.y;

        let thin = super::passover::far_edge(on_edge, -Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero(), guidance).filter(|(_, t)| *t <= PASSOVER_MAX_THICKNESS);
        let candidate = if edge_dz <= GROUND_MAX_UP && thin.is_some() {
            // pass-over target (type 2) on a thin wall top. PORT: the game's type-2 guidance candidates are not traced;
            // the port offers wall tops ≤ 1 m thick (the vault's 30 ↔ 100 cm blend range, 0xDDB800). The target sits
            // 0.5 m before the edge (0xB1EC40 pulls targets back 0.5·h), the reception carries the root onto it.
            let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
            Some(JumpTarget { position: on_edge + n * 0.5, type_flags: 2, hang: None, straight: None, pass: Some((on_edge, n)) })
        } else if edge_dz <= GROUND_MAX_UP {
            // ground target: land on top of the roof
            let landing = on_edge - e.n1 * LAND_INSET;
            let Some(h) = collision.ground_height(landing + Vec3::Y * 0.05, 0.2) else { continue };
            let pos = Vec3::new(landing.x, h, landing.z);
            Some(JumpTarget { position: pos, type_flags: TARGET_GROUND, hang: None, straight: None, pass: None })
        } else if edge_dz <= LEDGE_MAX_UP + WALL_HANG_DROP {
            // ledge target: hang from the edge (wall hang if there is wall below), both hands on the edge
            let on_edge = guidance.fit_hands(on_edge, e.n1);
            let hang =if collision.point_inside(on_edge - e.n1 * 0.15 - Vec3::Y * 0.9) {
                LedgeHangType::Wall
            } else {
                LedgeHangType::Free
            };
            let pos = hang_root(on_edge, on_edge, e.n1, hang);
            if pos.y - feet.y > LEDGE_MAX_UP {
                continue;
            }
            let flags = if hang == LedgeHangType::Wall { TARGET_LEDGE } else { TARGET_LEDGE_FREE };
            Some(JumpTarget { position: pos, type_flags: flags, hang: Some((on_edge, e.n1)), straight: None, pass: None })
        } else {
            None
        };
        let Some(target) = candidate else { continue };
        let flat = Vec3::new(target.position.x - feet.x, 0.0, target.position.z - feet.z);
        let dist = flat.length();
        let far = match target.hang {
            Some(_) if target.position.y - feet.y > LEDGE_JUMP_UP_RISE => LEDGE_JUMP_FAR_UP,
            Some(_) => LEDGE_JUMP_FAR,
            None => GROUND_FAR,
        };
        if !(0.6..=far).contains(&dist) {
            continue;
        }
        if flat.normalize().dot(want).clamp(-1.0, 1.0).acos() > TARGET_CONE {
            continue;
        }
        let dz = target.position.y - feet.y;
        if dz < TARGET_MIN_DZ {
            continue;
        }
        if target.type_flags == TARGET_GROUND {
            // must actually cross a gap or a step (not a point on the roof we are standing on)
            let low = feet.y.min(target.position.y);
            let crosses_gap = [0.25f32, 0.5, 0.75].iter().any(|&t| {
                let s = feet.lerp(target.position, t);
                collision.ground_height(Vec3::new(s.x, low + 0.05, s.z), 0.5).is_none()
            });
            if !crosses_gap && dz.abs() < 0.3 {
                continue;
            }
        }
        let better = match &best {
            None => true,
            // "highest in front", ties (within 0.25 m) → nearest
            Some((_, bdz, bd)) => dz > bdz + 0.25 || ((dz - bdz).abs() <= 0.25 && dist < *bd),
        };
        if better {
            best = Some((target, dz, dist));
        }
    }
    // haystacks (type 0x800): the top centre; a Leap of Faith when ≥ 3 m below (bands −30 m / 7.5 m), else the
    // haystack free-step bands (−3 m / 6 m) (0xB1EC40). PORT: a haystack in the cone wins over roof targets
    // (the game's LeapOfFaith ability path, IHuman vt1540/1544, is not traced).
    for s in &guidance.haystacks {
        let top = Vec3::new((s.min.x + s.max.x) * 0.5, s.max.y, (s.min.z + s.max.z) * 0.5);
        let flat = Vec3::new(top.x - feet.x, 0.0, top.z - feet.z);
        let dist = flat.length();
        let dz = top.y - feet.y;
        let faith = dz <= -super::jump_blend::FAITH_MIN_DROP;
        let (far, min_dz) = if faith { (super::jump_blend::FAITH_NEAR, -super::jump_blend::FAITH_DOWN) } else { (6.0, TARGET_MIN_DZ) };
        if !(0.6..=far).contains(&dist) || dz < min_dz || dz > 1.3 {
            continue;
        }
        if flat.normalize().dot(want).clamp(-1.0, 1.0).acos() > TARGET_CONE {
            continue;
        }
        return Some(JumpTarget { position: top, type_flags: super::jump_blend::TARGET_HAYSTACK, hang: None, straight: None, pass: None });
    }
    best.map(|b| b.0)
}

/// True when there is no floor a short way ahead (approaching a roof edge).
pub fn edge_ahead(feet: Vec3, forward: Vec3, collision: &CollisionWorld) -> bool {
    let probe = feet + forward * 0.6 + Vec3::Y * 0.05;
    collision.ground_height(probe, 0.6).is_none()
}
