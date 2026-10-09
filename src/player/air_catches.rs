//! Air-catch eligibility (0xE0BB70, timers 0xE01C00; RE/05 §5).
use bevy::prelude::*;
use crate::collision::CollisionWorld;
use super::targets::JumpTarget;

/// Manual ladder search remains height-gated even after the grasp-blend timer expires.
pub fn manual(hand: bool, fall_height: f32, drop_type: Option<usize>) -> bool {
    hand && fall_height > 0.3 && drop_type.is_none_or(|ty| ty == 2)
}

pub fn narrow(fall_height: f32, elapsed: f32, drop: bool) -> bool {
    !drop && fall_height < 9.0 && (fall_height > 0.3 || elapsed >= 0.4)
}

/// 0xE044D0: a clear path, regardless of which side of the root the target lies on.
pub fn target_ahead(root: Vec3, target: Option<JumpTarget>, world: &CollisionWorld) -> bool {
    let Some(target) = target.filter(|t| matches!(t.type_flags, 1 | 0x10000 | 2)) else { return false };
    // PORT: immutable jump targets have no native entity lifetime / cell handle to invalidate.
    let delta = target.position - root;
    let distance = delta.length();
    let slack = if target.type_flags == 0x10000 { 0.0005 } else { 0.0 };
    world.ray_distance(root, delta, distance, crate::layers::CHARACTER) >= distance - slack
}

pub fn automatic(elapsed: f32, drop: bool, target_flags: Option<u32>, facing: Vec3, normals: impl Iterator<Item = Vec3>) -> bool {
    !drop && elapsed >= 0.2 && target_flags.is_some_and(|ty| matches!(ty, 1 | 0x10000 | 0x40 | 0x400))
        // PORT: the proxy's static contacts adapt native type-4 controller reports; this actor is always Human.
        && normals.map(|n| Vec3::new(n.x, 0.0, n.z).normalize_or_zero()).any(|n| facing.dot(n) < -std::f32::consts::FRAC_1_SQRT_2)
}

pub fn catch_blend(speed: f32) -> f32 { (1.0 - (speed / 10.0).min(1.0)) * 0.14 + 0.06 }

// ---------------------------------------------------------------- the ledge, climb and free-hang catches

/// `CheckAirCatch` 0xE0BB70's catch actions (HumanInAir block, `xx_fall_tr_*`), [fall < 3 m, ≥ 3 m].
/// Climb catch (type 1): `climb_min_a` / `climb_max_a`.
pub const CLIMB_CATCH: [u32; 2] = [0x1F0C_1C57, 0x1F0C_1C58];
/// The ≥ 3 m free-hang catch to the side (type 4, one hand): `hangfree_max_left_a` / `hangfree_max_right_a`.
pub const FREE_CATCH_SIDE: [u32; 2] = [0x2013_72F8, 0x2013_72F9];
/// The edge landing (InAir sub-state 3), `xx_fall_step_off_{front,back,left,right}_max` then `…_tr_fall_max`.
pub const STEP_OFF: [u32; 4] = [0x2013_6CEF, 0x2013_6CF0, 0x2013_6CF1, 0x2013_6CF2];

/// What the catch search found.
#[derive(Clone, Copy, Debug)]
pub enum CatchFound {
    /// Type 1: both hands and both feet on holds → the Climb context with `CLIMB_CATCH`.
    Climb(super::climb::ClimbEntry),
    /// Types 3 (wall hang), 4 / 5 (free hang): the hands' midpoint, the wall normal, the catch action, and for the
    /// one-handed side catch the hand that holds (`Some(true)` = the right hand).
    Hang { mid: Vec3, normal: Vec3, wall: bool, action: u32, one_hand: Option<bool> },
}

/// `CheckAirCatch` 0xE0BB70 after the ladder (RE/19 §2): the order and the rules, with the port's guidance probes in
/// place of `Guidance__ProbeBox` / `FindHandholdPair` 0xE0A150.
/// 1. `FindLedgeCatch` 0xE0A990: a hand pair on ledge holds around the feet + 1.4 m (a hand bone + 0.2 m on the
///    automatic catch), within 70° (box 0.4 m; mask 306, filter `dword_193417C`). Then
///    - feet holds (`sub_E0A810`: the hands' midpoint − 1.2 m + 0.25 m along the reach, 0.5 m on a manual grab,
///      ±0.25 m sideways, climb filter `dword_19341B0`) → **type 1, the climb catch** (`Human__ComputeClimbPoseFromHolds`);
///    - else feet on the wall (two `sub_11718B0` probes 1 m below) → **type 3, the wall-hang catch**
///      `ACT_CATCH_WALL` (its 3-way angle blend snapped to the dominant class, 30° / 45°);
///    - else nothing here: the search goes on.
/// 2. A fall of 3 m or more: `sub_DFFE30` (one hold, the feet + 1.95 m + 0.25 / 0.5 m along the reach, half
///    0.4 / 0.25 or 0.5 / 0.25, 70°, climb filter) → **type 4**, the free hang: beyond 120° from the facing
///    `hangfree_max` (both hands), else to the side `FREE_CATCH_SIDE` with one hand.
///    Under 3 m: `FindClimbCatch` 0xE0AC70 (a hand pair at the feet + 1.95 m, climb filter) → **type 5**,
///    `hangfree_min`.
/// Every search ignores holds less than 0.5 m below the last released hold (InAirData +0x190 / +0x1F4).
/// PORT: the port's guidance has one edge type, so the ledge / climb filters are the same edges; the feet holds are
/// edges other than the hands' one, 1.2 m below. The bodies' clearance tests (`sub_B2DC80` / `sub_B2DEB0`) are the
/// port's hang boxes.
pub fn ledge_search(
    feet: Vec3,
    reach: Vec3,
    facing: Vec3,
    fall_height: f32,
    manual: bool,
    release_hold: Option<Vec3>,
    guidance: &crate::guidance::GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<CatchFound> {
    use super::ledge::{hang_root_at, hang_type_at, LedgeHangType};
    let long = fall_height >= 3.0;
    let dir = Vec3::new(reach.x, 0.0, reach.z).normalize_or(Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::NEG_Z));
    let below_release = |y: f32| release_hold.is_none_or(|r| r.y - 0.5 > y);
    let angle = 70f32.to_radians();
    // 1. the ledge catch: a hand pair at the feet + 1.4 m
    if let Some(h) = guidance.probe(feet + Vec3::Y * 1.4 + dir * 0.3, 0.4, 0.3, Some(dir), angle).filter(|h| below_release(h.point.y)) {
        let n = h.wall_normal;
        let mid = guidance.fit_hands(h.point, n);
        let r = super::right_of(-n);
        let (hl, hr) = (mid - r * crate::tuning::HAND_SPACING * 0.5, mid + r * crate::tuning::HAND_SPACING * 0.5);
        let into = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        // the feet holds (sub_E0A810)
        let fc = mid - Vec3::Y * 1.2 + dir * if manual { 0.5 } else { 0.25 };
        // PORT: the box depth along the reach is not decoded (FindHandholdPair's caller arguments are garbled); 0.75 m
        // both ways reaches the holds when the reach points into the wall
        let foot = |side: f32| {
            let c = fc + r * 0.25 * side;
            guidance
                .probe_box(c, r, dir, Vec3::Y, Vec3::new(0.4, 0.75, 0.3), c - into * 0.2, angle, 0.1, crate::guidance::MASK_CLIMB_HOLDS)
                .filter(|f| f.edge != h.edge && (f.point.y - (mid.y - 1.2)).abs() <= 0.3 && f.wall_normal.dot(n) > 0.7)
        };
        if let (Some(fl), Some(fr)) = (foot(-1.0), foot(1.0)) {
            let entry = super::climb::ClimbEntry {
                entry_type: super::climb::ClimbEntryType::Default,
                hand_l: hl,
                hand_r: hr,
                foot_l: fl.point,
                foot_r: fr.point,
                normal: n,
                from_feet: feet,
                foot_right: false,
                action: Some(CLIMB_CATCH[long as usize]),
            };
            return Some(CatchFound::Climb(entry));
        }
        if hang_type_at(mid, n, collision) == LedgeHangType::Wall {
            let root = hang_root_at(hl, hr, n, LedgeHangType::Wall, collision);
            if super::climb::hang_boxes_free(collision, root, into, true) {
                let action = super::ledge::ACT_CATCH_WALL[long as usize];
                return Some(CatchFound::Hang { mid, normal: n, wall: true, action, one_hand: None });
            }
        }
    }
    // 2. the free hangs at the feet + 1.95 m
    let free = |h: crate::guidance::GuidanceHit| {
        let n = h.wall_normal;
        let mid = guidance.fit_hands(h.point, n);
        let r = super::right_of(-n);
        let (hl, hr) = (mid - r * crate::tuning::HAND_SPACING * 0.5, mid + r * crate::tuning::HAND_SPACING * 0.5);
        let root = hang_root_at(hl, hr, n, LedgeHangType::Free, collision);
        let into = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        super::climb::hang_boxes_free(collision, root, into, false).then_some((mid, n))
    };
    if long {
        // sub_DFFE30
        let along = if manual { 0.5 } else { 0.25 };
        let centre = feet + Vec3::Y * 1.95 + dir * along;
        let half = Vec3::new(0.4, along, 0.25);
        let reference = centre - dir * 0.4 + Vec3::Y * 0.25;
        let hit = guidance
            .probe_box(centre, super::right_of(dir), dir, Vec3::Y, half, reference, angle, 0.1, crate::guidance::MASK_CLIMB_HOLDS)
            .filter(|h| below_release(h.point.y));
        if let Some((mid, n)) = hit.and_then(free) {
            // the side by the angle between the facing and the hold (0xE0D322)
            let to = Vec3::new(mid.x - feet.x, 0.0, mid.z - feet.z).normalize_or(dir);
            let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or(dir);
            // the game's signed angle about up (counter-clockwise, toward the left, positive)
            let a = (-super::right_of(f).dot(to)).atan2(f.dot(to));
            let (action, one_hand) = if a.abs() > 120f32.to_radians() {
                (super::ledge::ACT_CATCH_FREE[1], None)
            } else if a >= 0.0 {
                (FREE_CATCH_SIDE[1], Some(true))
            } else {
                (FREE_CATCH_SIDE[0], Some(false))
            };
            return Some(CatchFound::Hang { mid, normal: n, wall: false, action, one_hand });
        }
    } else if let Some((mid, n)) = guidance
        .probe(feet + Vec3::Y * 1.95 + dir * 0.3, 0.4, 0.3, Some(dir), angle)
        .filter(|h| below_release(h.point.y))
        .and_then(free)
    {
        // FindClimbCatch 0xE0AC70
        return Some(CatchFound::Hang { mid, normal: n, wall: false, action: super::ledge::ACT_CATCH_FREE[0], one_hand: None });
    }
    None
}

/// InAir sub-state 3, the edge landing (`CheckAirCatch` LABEL_140, 0xE0D684): at a flat ground contact
/// (`HumanInAir__GetGroundContactType` = 1) with guidance edges near the feet (`sub_E008C0`: a box ±0.4 × ±0.4 ×
/// ±0.35 m round the feet + 0.15 m, along the facing; `HumanInAir__AverageNearbyEdges` 0xE01FA0: each edge whose
/// plane the feet are past, or within 0.15 m of, with room past it (`Human__RoomPastEdge`), its flattened wall normal
/// weighted by its length) the character does not land: by the angle a from the facing to the averaged normal,
/// |a| < 45° `step_off_back`, 45°–135° `step_off_left` / `_right` (turned ∓90°), else `step_off_front`; the body faces
/// that way and InAir goes on with the action (request 33), the root held 0.2 s. Returns (action, facing).
pub fn edge_landing(feet: Vec3, facing: Vec3, guidance: &crate::guidance::GuidanceWorld, collision: &CollisionWorld) -> Option<(u32, Vec3)> {
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::NEG_Z);
    let r = super::right_of(f);
    let centre = feet + Vec3::Y * 0.15;
    let mut sum = Vec3::ZERO;
    let mut count = 0;
    for (i, e) in guidance.edges.iter().enumerate() {
        if e.subtype != crate::guidance::GuidanceSubType::LedgeGrab {
            continue;
        }
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z);
        if n.length_squared() < 1e-6 {
            continue;
        }
        let n = n.normalize();
        // the edge within the box (its nearest point to the centre)
        let p = e.closest_point(centre);
        let l = p - centre;
        if l.dot(r).abs() > 0.4 || l.dot(f).abs() > 0.4 || l.y.abs() > 0.35 {
            continue;
        }
        // the feet against the edge's plane: past it (any distance) or within 0.15 m inside
        let past = (feet - p).dot(n);
        if past < -0.15 {
            continue;
        }
        let report = super::jump_candidates::EdgeReport { point: Vec3::new(p.x, feet.y, p.z), normal: n, drop: 0.0, dist: -past, edge: i };
        if !super::jump_candidates::room_past_edge(&report, collision) {
            continue;
        }
        sum += n * (e.p1 - e.p0).length();
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let n = Vec3::new(sum.x, 0.0, sum.z).normalize_or(f);
    // the game's signed angle from the facing to the normal (counter-clockwise positive); its ±90° turns are the
    // port's rotations about +Y
    let a = (-r.dot(n)).atan2(f.dot(n));
    let rot = |deg: f32| Quat::from_rotation_y(deg.to_radians()) * n;
    Some(if a.abs() < 45f32.to_radians() {
        (STEP_OFF[1], n)
    } else if a > -135f32.to_radians() && a < -45f32.to_radians() {
        (STEP_OFF[3], rot(90.0))
    } else if a > 45f32.to_radians() && a < 135f32.to_radians() {
        (STEP_OFF[2], rot(-90.0))
    } else {
        // PORT: the front case's facing (`sub_42FA60`) is not decoded; the body keeps facing away from the normal
        (STEP_OFF[0], -n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn height_timer_and_drop_boundaries() {
        assert!(!manual(true, 0.3, None)); assert!(manual(true, 0.30001, None));
        assert!(!manual(false, 3.0, None)); assert!(!manual(true, 3.0, Some(0)));
        assert!(manual(true, 3.0, Some(2)));
        assert!(!narrow(0.3, 0.399, false)); assert!(narrow(0.3, 0.4, false));
        assert!(narrow(0.30001, 0.0, false)); assert!(!narrow(9.0, 1.0, false));
        assert!(narrow(8.999, 0.0, false)); assert!(!narrow(1.0, 1.0, true));
    }
    #[test]
    fn automatic_needs_target_timer_and_wall_contact() {
        let check = |t, drop, ty, n| automatic(t, drop, ty, Vec3::Z, [n].into_iter());
        assert!(!check(0.199, false, Some(1), Vec3::NEG_Z));
        assert!(check(0.2, false, Some(1), Vec3::NEG_Z));
        assert!(!check(0.2, true, Some(1), Vec3::NEG_Z));
        assert!(!check(0.2, false, Some(2), Vec3::NEG_Z));
        assert!(!check(0.2, false, Some(1), Vec3::X));
        assert!(!check(0.2, false, None, Vec3::NEG_Z));
    }
    #[test]
    fn ladder_blend_clamps_speed() {
        assert!((catch_blend(0.0)-0.2).abs()<1e-6);
        assert!((catch_blend(5.0)-0.13).abs()<1e-6);
        assert!((catch_blend(10.0)-0.06).abs()<1e-6);
        assert!((catch_blend(20.0)-0.06).abs()<1e-6);
    }
}

#[cfg(test)]
mod ray_tests {
    use super::*;
    use crate::collision::Aabb3;
    fn target(position:Vec3,ty:u32)->JumpTarget {JumpTarget{position,type_flags:ty,hang:None,straight:None,pass:None,ladder:None}}
    #[test]
    fn target_guard_is_a_ray_not_a_forward_cone() {
        let mut world=CollisionWorld::default();let root=Vec3::ZERO;
        assert!(target_ahead(root,Some(target(Vec3::NEG_Z*3.0,1)),&world));
        assert!(!target_ahead(root,Some(target(Vec3::Z*3.0,0x40)),&world));
        world.boxes.push(Aabb3{min:Vec3::new(-0.5,-0.5,1.0),max:Vec3::new(0.5,0.5,1.5)});
        assert!(!target_ahead(root,Some(target(Vec3::Z*3.0,1)),&world));
        assert!(target_ahead(root,Some(target(Vec3::Z,1)),&world));
        assert!(!target_ahead(root,Some(target(Vec3::Z*1.0004,1)),&world));
        assert!(target_ahead(root,Some(target(Vec3::Z*1.0004,0x10000)),&world));
        world.layers.push(crate::layers::NOTHING);
        assert!(target_ahead(root,Some(target(Vec3::Z*3.0,1)),&world));
    }
}
