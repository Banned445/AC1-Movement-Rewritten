//! Jump targets: the game's candidate query 0xE18970 and scorer 0xE96BF0 (`jump_candidates`, RE/18 §1), turned into
//! the port's `JumpTarget`s. Ground targets land `LAND_INSET` onto the roof; ledge targets hang from the edge (aim =
//! hang root).

use bevy::prelude::*;

use super::ledge::{hang_root, LedgeHangType};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceSubType, GuidanceWorld};
use super::jump_candidates::{self, Candidate, Query};
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
    /// Ladder target (type 0x1000): the Ladder context's entry on arrival (0xE07D00 → EntryType 1, ladder state 2).
    pub ladder: Option<super::ladder::LadderEntry>,
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
/// Ladder targets (flight `…_to_surface`, arrival `HumanInAir__CheckJumpTargetArrival` 0xE07D00 case 0x1000).
pub const TARGET_LADDER: u32 = 0x1000;

/// PORT: static post candidates depend on map geometry, not the jumper. Build after all placements load.
pub(crate) fn find_jump_pilotis(guidance: &GuidanceWorld, collision: &CollisionWorld) -> Vec<Vec3> {
    pilotis_for_edges(guidance.edges.iter(), guidance, collision)
}

/// The posts among some of the world's edges (a streamed map adds them per object as it loads).
pub(crate) fn pilotis_for_edges<'a>(edges: impl Iterator<Item = &'a crate::guidance::GuidanceEdge>, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Vec<Vec3> {
    let mut pilotis: Vec<Vec3> = Vec::new();
    for e in edges {
        if e.subtype != GuidanceSubType::LedgeGrab { continue; }
        let mid = (e.p0 + e.p1) * 0.5;
        let into = -Vec3::new(e.n1.x, 0.0, e.n1.z);
        if let Some(top) = super::narrow::find_pilotis(mid, mid, into, false, guidance, collision) {
            if !pilotis.iter().any(|p| (*p - top).length() < 0.1) { pilotis.push(top); }
        }
    }
    pilotis
}

/// The tap jump's target (the interpreter's jump branch 0xEE817E): the game's candidate query with beams
/// (`jump_candidates::query`, kind 0) and the scorer 0xE96BF0 (mode 1). With no candidate, the Leap of Faith search
/// (IHuman vt76, the haystacks) follows, as in the interpreter (0xEE8239).
pub fn find_jump_target(
    feet: Vec3,
    want_dir: Vec3,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<JumpTarget> {
    find_target(feet, want_dir, want_dir, &Query::TAP, false, guidance, collision).or_else(|| haystack_target(feet, want_dir, guidance))
}

/// Query + scorer + the port's target for the chosen candidate. `facing` = the actor's forward (the scorer's frame),
/// `want` = the wanted direction.
pub fn find_target(feet: Vec3, facing: Vec3, want: Vec3, q: &Query, behind_far: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<JumpTarget> {
    let cands = jump_candidates::query(feet, want, q, guidance, collision);
    let (i, ty) = jump_candidates::select(&cands, feet, facing, want, 1, behind_far)?;
    to_target(&cands[i], ty, feet, guidance, collision)
}

/// The port's jump target for a candidate and the scorer's jump type.
pub fn to_target(c: &Candidate, ty: u32, feet: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<JumpTarget> {
    let base = JumpTarget { position: c.pos, type_flags: ty, hang: None, straight: None, pass: None, ladder: None };
    match ty {
        1 | 0x10000 => {
            // beams and far-edge landings already lie on the surface
            if c.ty == 4 || (c.ty == 2 && c.sub == 1) {
                return Some(base);
            }
            // PORT: a roof edge's landing is `LAND_INSET` onto its top (the game aims at the edge and 0xB1EC40 pulls
            // the target back); a narrower top (posts) takes a shallower inset
            // a top narrower than two insets (a post) is landed on in its middle
            let narrow = super::passover::far_edge(c.pos, -c.wall, guidance).map(|(_, t)| t).filter(|t| *t < 2.0 * LAND_INSET);
            let insets: &[f32] = match narrow { Some(t) => &[t * 0.5][..], None => &[LAND_INSET, 0.25, 0.1][..] };
            for &inset in insets {
                let landing = c.pos - c.wall * inset;
                if let Some(h) = collision.ground_height(landing + Vec3::Y * 0.05, 0.2) {
                    let target = JumpTarget { position: Vec3::new(landing.x, h, landing.z), ..base };
                    // PORT: a thin wall top is a pass-over (type 2). The query gives a railing type 1 | 2 like a
                    // roof; which of the two the game jumps at there is not established (LIVE:, RE/18 §9 item 2)
                    if c.flags & 2 != 0 && c.sub == 4 {
                        if let Some((_, t)) = super::passover::far_edge(c.pos, -c.wall, guidance).filter(|(_, t)| *t <= PASSOVER_MAX_THICKNESS) {
                            let _ = t;
                            return Some(JumpTarget { position: c.pos + c.wall * 0.5, type_flags: 2, pass: Some((c.pos, c.wall)), ..base });
                        }
                    }
                    return Some(target);
                }
            }
            None
        }
        2 => Some(JumpTarget { position: c.pos + c.wall * 0.5, pass: Some((c.pos, c.wall)), ..base }),
        TARGET_LEDGE | TARGET_LEDGE_FREE => {
            // hang targets need an edge (poles, type 32, are cut content in v1.02: RE/05 §3.5)
            if c.ty == 32 {
                return None;
            }
            let e = &guidance.edges[c.edge?];
            let on_edge = guidance.fit_hands(c.pos, e.n1);
            let hang = if ty == TARGET_LEDGE { LedgeHangType::Wall } else { LedgeHangType::Free };
            Some(JumpTarget { position: hang_root(on_edge, on_edge, e.n1, hang), hang: Some((on_edge, e.n1)), ..base })
        }
        TARGET_LADDER => {
            let e = &guidance.edges[c.edge?];
            let (bottom, top) = if e.p0.y <= e.p1.y { (e.p0, e.p1) } else { (e.p1, e.p0) };
            let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
            if Vec3::new(feet.x - bottom.x, 0.0, feet.z - bottom.z).dot(n) <= 0.0 {
                return None;
            }
            // the air catch's rung rule (0xE04100) at the candidate's height
            let h = super::ladder::catch_height(c.pos.y - bottom.y + 0.0005, top.y - bottom.y)?;
            let pos = bottom + n * super::ladder::ATTACH_OUT + Vec3::Y * h;
            let entry = super::ladder::LadderEntry { base: bottom, top, n, from: pos, facing: -n, from_ledge: true, height: Some(h), action: Some(super::ladder::ARRIVE_TARGET), chain: true, ..Default::default() };
            Some(JumpTarget { position: pos, ladder: Some(entry), ..base })
        }
        // a kiosk's piece (0x400): its middle (0xE18970), handed to the Kiosk context on arrival (0xE07D00)
        0x400 => Some(base),
        // horse (0x200) targets have no port context
        _ => None,
    }
}

/// The Leap of Faith search (IHuman vt76, ability LeapOfFaith): a haystack's top in the 45-degree cone; a Leap of
/// Faith when it lies 3 m or more below (bands -30 m / 7.5 m), else the haystack free-step bands (-3 m / 6 m)
/// (0xB1EC40). PORT: vt76 itself is not traced (RE/04 §4.1.12).
pub fn haystack_target(feet: Vec3, want_dir: Vec3, guidance: &GuidanceWorld) -> Option<JumpTarget> {
    let want = Vec3::new(want_dir.x, 0.0, want_dir.z).normalize_or_zero();
    if want == Vec3::ZERO {
        return None;
    }
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
        return Some(JumpTarget { position: top, type_flags: super::jump_blend::TARGET_HAYSTACK, hang: None, straight: None, pass: None, ladder: None });
    }
    None
}

/// True when there is no floor a short way ahead (approaching a roof edge).
/// The jump off a ledge or a climb wall (`GoAssassinActionInterpreter__LedgeState` 0xEEC89F, `__ClimbState`
/// 0xEE6372, event 2): the stick when it is past the dead zone, else straight away from the wall (`away` = the wall's
/// outward normal); turned to 89 degrees off `away` when it points further round, refused beyond 135 (into the wall).
pub fn jump_off_wall_dir(stick: Option<Vec3>, away: Vec3) -> Option<Vec3> {
    let away = Vec3::new(away.x, 0.0, away.z).normalize_or_zero();
    let want = stick.map(|d| Vec3::new(d.x, 0.0, d.z).normalize_or(away)).unwrap_or(away);
    let a = away.angle_between(want);
    if a > 135f32.to_radians() {
        return None;
    }
    if a > std::f32::consts::FRAC_PI_2 {
        let side = if away.cross(want).y >= 0.0 { 1.0 } else { -1.0 };
        return Some(Quat::from_rotation_y(1.553_343_1 * side) * away);
    }
    Some(want)
}

/// Its target (0xEEC89F): the guidance search along `dir`, else the free-jump target (vt68 -> 0xB1E7F0, 8 m along it,
/// 3 m down). PORT: the Leap of Faith search in between (vt76) is not modelled for these jumps.
pub fn jump_off_wall_entry(from: Vec3, dir: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> super::air::InAirEntry {
    match find_jump_target(from, dir, guidance, collision) {
        Some(target) => super::air::InAirEntry::JumpToTarget { from, target, speed_param: 0.5, foot_left: true },
        None => super::air::InAirEntry::FreeJump { from, dir, ahead: FREE_JUMP_AHEAD, speed_param: 0.5, foot_left: true },
    }
}

pub fn edge_ahead(feet: Vec3, forward: Vec3, collision: &CollisionWorld) -> bool {
    let probe = feet + forward * 0.6 + Vec3::Y * 0.05;
    collision.ground_height(probe, 0.6).is_none()
}
