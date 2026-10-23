//! Jump targets: the game's candidate query 0xE18970 and scorer 0xE96BF0 (`jump_candidates`, RE/18 §1), turned into
//! the port's `JumpTarget`s. Ground targets are the candidate on the roof edge (the flight is pulled back, `air::pull_back`); ledge targets hang from the edge (aim =
//! hang root).

use bevy::prelude::*;

use super::ledge::{hang_root, LedgeHangType};
use crate::collision::CollisionWorld;
use crate::guidance::GuidanceWorld;
use super::jump_candidates::{self, Candidate, Query};

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
    /// The candidate's normal (pointing back at the jumper) and whether its hit type is 4 (a beam): what the free-step
    /// arrival writes to NarrowObjectData +0x100 / +0x140 (0xE09A13–0xE09A94, RE/05 §2.15).
    pub support: Option<(Vec3, bool)>,
}

/// Roof-edge landing spot: the game's free-step target type 1 (narrow object / edge; its flight is
/// `xx_h_air_*_to_freestep` and arrival plays the free-step reception, 0xB1EC40 / 0xE07D00). 0x8000 is the
/// air-assassination target (its flight is `…_to_assassinate`), not plain ground.
pub const TARGET_GROUND: u32 = super::jump_blend::TARGET_FREESTEP;
/// Ledge hang targets: 0x40 = wall hang (flight `…_to_surface`), 0x80 = free hang (flight `…_to_swing`) (0xB1EC40).
/// The arrival (0xE07D00) picks the reception by the foot holds, not the type: holds → the wall reception, none → the
/// swing for 0x80 (RE/24 AIR-37).
pub const TARGET_LEDGE: u32 = 0x40;
pub const TARGET_LEDGE_FREE: u32 = 0x80;
/// Ladder targets (flight `…_to_surface`, arrival `HumanInAir__CheckJumpTargetArrival` 0xE07D00 case 0x1000).
pub const TARGET_LADDER: u32 = 0x1000;

/// The tap jump's target (the interpreter's jump branch 0xEE817E): the game's candidate query with beams
/// (`jump_candidates::query`, kind 0) and the scorer 0xE96BF0 (mode 1). With no candidate, the Leap of Faith search
/// (IHuman vt76, the haystacks) follows, as in the interpreter (0xEE8239).
pub fn find_jump_target(
    feet: Vec3,
    want_dir: Vec3,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<JumpTarget> {
    find_target(feet, want_dir, want_dir, &Query::TAP, false, guidance, collision).or_else(|| haystack_target(feet, want_dir, want_dir, guidance, collision))
}

/// The swing jump's target (the interpreter's ledge state, event 7, 0xEECEC6–0xEED063): the candidate query along the
/// facing and the scorer; the Leap of Faith search (vt76) only when the query found no candidates at all (with
/// candidates but none chosen the interpreter goes straight to the free jump). The interpreter runs two selections over
/// the same candidates, mode 1 and mode 128 (only mode 128 with the empty hand held, `sub_6D0F50`(3) at 0xEECEF2), and
/// hands on a pair (IHumanLedge vt96 0xDE43D0) or one target (vt100 0xDCEB00 = vt96 with an empty second, type
/// 0x80000000); `HumanLedge__SetupSwingJump` 0xDCD0B0 passes the second as `Human__SetupJumpToTarget`'s chained target.
/// The scorer reads its mode only as `a5 == 2` (0xE97584) and both calls pass the same actor and a zero wanted direction,
/// so both selections return the same candidate and type: the chain is the target itself, and one selection is the
/// game's result. PORT: the query is the tap jump's, and the Leap of Faith looks along the facing (the game: the
/// camera-relative stick).
pub fn swing_jump_target(feet: Vec3, fwd: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<JumpTarget> {
    let cands = jump_candidates::query(feet, fwd, &Query::TAP, guidance, collision);
    if cands.is_empty() {
        return haystack_target(feet, fwd, fwd, guidance, collision);
    }
    let sel = jump_candidates::select(&cands, feet, fwd, fwd, 1, false);
    let t = sel.and_then(|(i, ty)| to_target(&cands[i], ty, feet, guidance, collision));
    if std::env::var_os("AC_JUMP_LOG").is_some() {
        let g = |v: Vec3| format!("({:.3} {:.3} {:.3})", v.x, -v.z, v.y);
        eprintln!("swing jump query pos={} dir={} -> {:?} target {:?}", g(feet), g(fwd), sel, t.map(|t| (g(t.position), t.type_flags)));
        for c in &cands {
            eprintln!("  cand {} n={} t={} s={} f={:#x}", g(c.pos), g(c.wall), c.ty, c.sub, c.flags);
        }
    }
    super::target_debug::record(|| "swing jump".into(), feet, fwd, &cands, t.as_ref());
    t
}

/// Query + scorer + the port's target for the chosen candidate. `facing` = the actor's forward (the scorer's frame),
/// `want` = the wanted direction.
pub fn find_target(feet: Vec3, facing: Vec3, want: Vec3, q: &Query, behind_far: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<JumpTarget> {
    let cands = jump_candidates::query(feet, want, q, guidance, collision);
    let sel = jump_candidates::select(&cands, feet, facing, want, 1, behind_far);
    let t = sel.and_then(|(i, ty)| to_target(&cands[i], ty, feet, guidance, collision));
    if std::env::var_os("AC_JUMP_LOG").is_some() {
        // game axes (x, y, z) = (x, -z, y), as the lab's probes print them
        let g = |v: Vec3| format!("({:.3} {:.3} {:.3})", v.x, -v.z, v.y);
        eprintln!("query kind {} pos={} dir={} facing={} -> {:?}", q.kind, g(feet), g(want), g(facing), sel);
        for c in &cands {
            eprintln!("  cand {} n={} t={} s={} f={:#x}", g(c.pos), g(c.wall), c.ty, c.sub, c.flags);
        }
    }
    super::target_debug::record(|| format!("jump (query kind {})", q.kind), feet, want, &cands, t.as_ref());
    t
}

/// A10 (RE/31c g13): a hang target's aim and reception normal are the candidate's flat wall normal (`Candidate::wall`).
/// Off: the piece's raw n1, as before.
pub const FLAT_HANG_TARGET_NORMAL: bool = true;

/// The port's jump target for a candidate and the scorer's jump type.
pub fn to_target(c: &Candidate, ty: u32, feet: Vec3, guidance: &GuidanceWorld, _collision: &CollisionWorld) -> Option<JumpTarget> {
    let base = JumpTarget { position: c.pos, type_flags: ty, hang: None, straight: None, pass: None, ladder: None, support: Some((c.wall, c.ty == 4)) };
    match ty {
        1 | 0x10000 => {
            // beams and far-edge landings already lie on the surface
            if c.ty == 4 || (c.ty == 2 && c.sub == 1) {
                return Some(base);
            }
            // the target is the candidate on the edge: 0xB1EC40 pulls the flight's aim 0.5 m back toward the jumper and
            // the free-step arrival interpolates the root 0.5 m forward, onto the edge (`air::pull_back`, RE/24 AIR-2);
            // the roof is checked just inside it. A post's top too: the free-step stay's reception carries the root on
            // and `TryPilotisFreeStep` 0xE50190 interpolates it to the top's centre (RE/05 §2.15)
            // the candidate itself (its position is the request's +0, `Human__SetupJumpToTarget` 0xB20468): a post's top
            // as any edge, the free-step stay's `TryPilotisFreeStep` 0xE50190 then carries the root to its centre
            // (RE/05 §2.15). A wall top however thin is a free step here: the pass-over (type 2) is the scorer's mode 2
            // only (`Human__SelectJumpCandidate` 0xE96BF0 at 0xE97584), which no ground jump asks for (RE/18 §1.8)
            Some(base)
        }
        2 => Some(JumpTarget { position: c.pos, pass: Some((c.pos, c.wall)), ..base }),
        TARGET_LEDGE | TARGET_LEDGE_FREE => {
            // a Pole edge (type 32: the horizontal swing bars, subtype 4) is a free-hang target (the scorer's pole case
            // 0xE96BF0 → 0x80, the swing flight); the cut content of RE/05 §3.5 is the vertical pole climb, not the bars
            if c.ty == 32 && ty != TARGET_LEDGE_FREE {
                return None;
            }
            let e = &guidance.edges[c.edge?];
            // the request's normal is the candidate's, flattened (the scorer's records, lab probe `Sel` 0xE96BF0: c1's
            // n = (−0.726, 0.687, 0) on a piece whose wall face leans, n1.z = 0.36): on a leaning piece the raw n1 lifted
            // the aim 0.18 m (A10, lab 10-12 `freerun_east_cands`: the game's root lands at the edge − 1.1 m)
            let n = if FLAT_HANG_TARGET_NORMAL { c.wall } else { e.n1 };
            let on_edge = guidance.fit_hands(c.pos, n);
            // the aim (0xB1EC40 0xB1F60E, RE/24 AIR-3): the edge + 0.5·n, 1.1 m down for a wall hang (the wall-hang
            // root) and only 0.5 m down for a free hang (0x80); the swing reception then interpolates the root to the
            // free-hang pose (0xE09662). PORT: the game takes this aim only for a target with an owner entity (world
            // geometry has one), else the pulled-back point
            let position = if ty == TARGET_LEDGE { hang_root(on_edge, on_edge, n, LedgeHangType::Wall) } else { on_edge + n * 0.5 - Vec3::Y * 0.5 };
            Some(JumpTarget { position, hang: Some((on_edge, n)), ..base })
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

/// The Leap of Faith search (IHuman vt76 → `Human__sub_E15AA0` with `hay::LEAP_OF_FAITH`, RE/24 AIR-26 / CENSUS-1): the
/// first haystack in the box / cone / height window with clear rays; the target is its top (entity origin + 1.6 m), type
/// 0x800. A Leap of Faith when it lies 3 m or more below (bands −30 m / 7.5 m), else the haystack bands (0xB1EC40).
pub fn haystack_target(feet: Vec3, want_dir: Vec3, facing: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<JumpTarget> {
    let s = super::hay::search(feet, want_dir, facing, &super::hay::LEAP_OF_FAITH, &guidance.haystacks, collision)?;
    let t = JumpTarget { position: super::hay::top_of(&s), type_flags: super::jump_blend::TARGET_HAYSTACK, hang: None, straight: None, pass: None, ladder: None, support: None };
    super::target_debug::record(|| "leap of faith".into(), feet, want_dir, &[], Some(&t));
    Some(t)
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
/// The jump off a wall hang or a climb (event 2 → the rebound, then `HumanLedge__ReboundEndJump` 0xDD0A40 /
/// `HumanClimb__ReboundEndJump` 0xDEAE30: jump kind 2 after turning away from the wall, RE/24 AIR-5). The takeoff foot
/// is the rebound item's word bits 2–3 (0xDD0AC6 / 0xDEAE7A); the climb's only rebound, `rebound_footl` (0x0025), takes
/// off on the left, and the ledge sets it from its rebound at the end (`takeoff_foot_left`).
pub fn jump_off_wall_entry(from: Vec3, dir: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> super::air::InAirEntry {
    super::air::InAirEntry::ReboundJump { from, target: find_jump_target(from, dir, guidance, collision), dir, foot_left: true }
}

pub fn edge_ahead(feet: Vec3, forward: Vec3, collision: &CollisionWorld) -> bool {
    let probe = feet + forward * 0.6 + Vec3::Y * 0.05;
    collision.ground_height(probe, 0.6).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guidance::{GuidanceEdge, GuidanceSubType};

    /// A10 (RE/31c g13): a wall-hang target on a piece whose wall face leans (n1 with an up part, as the 10-12
    /// `freerun_east_cands` post's, n1.z = 0.36) aims at the edge + 0.5·n − 1.1 m with the candidate's flat normal: the
    /// root 1.1 m under the edge, not 0.18 m higher.
    #[test]
    fn a_hang_target_on_a_leaning_piece_aims_with_the_flat_normal() {
        let leaning = Vec3::new(0.0, 0.362, 0.932).normalize();
        let mut g = GuidanceWorld::default();
        g.edges.push(GuidanceEdge { p0: Vec3::new(-1.0, 3.0, 0.0), p1: Vec3::new(1.0, 3.0, 0.0), n0: Vec3::new(0.0, 0.93, -0.36), n1: leaning, subtype: GuidanceSubType::LedgeGrab });
        let c = Candidate { pos: Vec3::new(0.0, 3.0, 0.0), top: Vec3::Y, wall: Vec3::Z, ty: 16, sub: 8, flags: 0xC0, subtype: GuidanceSubType::LedgeGrab, edge: Some(0), ladder: None };
        let t = to_target(&c, TARGET_LEDGE, Vec3::new(0.0, 3.0, 4.0), &g, &CollisionWorld::default()).expect("a wall-hang target");
        assert!((t.position.y - 1.9).abs() < 1e-4, "the root 1.1 m under the edge: {}", t.position);
        assert!((t.position.z - 0.5).abs() < 1e-4, "0.5 m out along the flat normal: {}", t.position);
        assert_eq!(t.hang.map(|(_, n)| n), Some(Vec3::Z), "the reception's normal is the flat one");
    }
}
