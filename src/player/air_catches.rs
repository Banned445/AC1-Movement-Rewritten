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

/// `CheckAirCatch` 0xE0BB70's automatic catch (0xE0C385–0xE0C702, RE/24 AIR-12): its timer run out, not a drop,
/// **Data+505** (the target-driven part of a jump: `jumping`), a target of type 1 / 0x10000 / 0x40 / 0x400 and a
/// controller contact whose flat normal faces the character (dot < −0.7071). PORT: the gate "target owner valid or fall
/// height < 2 m" holds for world targets (they have an owner); the actor is always the player (entity +168 & 7 == 1).
pub fn automatic(elapsed: f32, drop: bool, jumping: bool, target_flags: Option<u32>, facing: Vec3, normals: impl Iterator<Item = Vec3>) -> bool {
    !drop && jumping && elapsed >= 0.2 && target_flags.is_some_and(|ty| matches!(ty, 1 | 0x10000 | 0x40 | 0x400))
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

/// `CheckAirCatch` 0xE0BB70 after the ladder (RE/19 §2): the order and the rules.
/// The automatic catch centres all three searches on the Hips bone (0xDED10611, `hips`, from the simulation's pose) +
/// 0.2 m up, half height 0.9 (RE/24 AIR-12); without a pose (no install) the PORT centres below stand in.
/// 1. `FindLedgeCatch` 0xE0A990 → `FindHandholdPair` 0xE0A150 (`GuidanceWorld::handhold_pair`): a hand pair on the
///    chains in a box around the feet + 1.4 m (the Hips + 0.2 m on the automatic catch) + `along` m along the reach
///    (0.25, 0.5 on a manual grab), facing it, half extents 0.4 / along / 0.3; walls within 70°, mask 306; the hand
///    references (∓0.25, −0.4, +0.3) in the box frame, margin 0.1. Then
///    - feet holds (`FindCatchFeetHolds` 0xE0A810: the same search from the feet at the higher hand − 1.2 m, along
///      −(the hands' wall normals)) → **type 1, the
///      climb catch** (`Human__ComputeClimbPoseFromHolds`);
///    - else feet on the wall (`wall_catch_feet`: two `sub_11718B0` boxes 1 m below, both required, no body box)
///      → **type 3, the wall-hang catch** `ACT_CATCH_WALL` (its 3-way angle blend snapped to the dominant class,
///      30° / 45°);
///    - else nothing here: the search goes on.
/// 2. A fall of 3 m or more: `sub_DFFE30` (one hold, the feet + 1.95 m + 0.25 / 0.5 m along the reach, half
///    0.4 / 0.25 or 0.5 / 0.25, 70°, mask 50) → **type 4**, the free hang: beyond 120° from the facing
///    `hangfree_max` (both hands), else to the side `FREE_CATCH_SIDE` with one hand.
///    Under 3 m: `FindClimbCatch` 0xE0AC70 (the ledge catch's search around the feet + 1.95 m) → **type 5**,
///    `hangfree_min`.
/// Every search ignores holds less than 0.5 m below the last released hold (InAirData +0x190 / +0x1F4).
/// PORT: the free hangs' clearance test (`sub_B2DEB0`) is the port's hang boxes.
pub fn ledge_search(
    feet: Vec3,
    reach: Vec3,
    facing: Vec3,
    fall_height: f32,
    manual: bool,
    hips: Option<Vec3>,
    release_hold: Option<Vec3>,
    guidance: &crate::guidance::GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<CatchFound> {
    use super::ledge::{hang_root_at, LedgeHangType};
    let long = fall_height >= 3.0;
    let dir = Vec3::new(reach.x, 0.0, reach.z).normalize_or(Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::NEG_Z));
    let below_release = |y: f32| release_hold.is_none_or(|r| r.y - 0.5 > y);
    let angle = 70f32.to_radians();
    // the box: manual, the feet + 1.4 m, 0.5 m along the reach, half height 0.3; automatic, the Hips bone (0xDED10611)
    // + 0.2 m up + 0.5 m along, half height 0.9 (0xE0A9C8–0xE0AA6D, RE/24 AIR-12). PORT without a pose: the Hips at the
    // feet + 1.4 m (the ledge and free-hang searches) or + 1.75 m (the climb catch's)
    let along = 0.5;
    let half_z = if manual { 0.3 } else { 0.9 };
    let auto = |stand_in: f32| hips.map_or(feet + Vec3::Y * stand_in, |h| h) + Vec3::Y * 0.2;
    let pair_along = |base: Vec3, dir: Vec3, half_z: f32| {
        let refs = [Vec3::new(-0.25, -0.4, 0.3), Vec3::new(0.25, -0.4, 0.3)];
        guidance.handhold_pair(base + dir * along, dir, Vec3::new(0.4, along, half_z), refs, angle, 0.1, crate::guidance::MASK_CATCH)
    };
    let catch_pair = |base: Vec3| pair_along(base, dir, half_z);
    // 1. the ledge catch: a hand pair at the feet + 1.4 m
    let ledge_base = if manual { feet + Vec3::Y * 1.4 } else { auto(1.4) };
    if let Some([a, b]) = catch_pair(ledge_base).filter(|[a, b]| below_release(a.point.y) && below_release(b.point.y)) {
        let n = a.wall_normal;
        let (hl, hr) = (a.point, b.point);
        let mid = (hl + hr) * 0.5;
        let into = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        // the feet holds (FindCatchFeetHolds 0xE0A810): the same search from the feet's position at the higher hand's
        // height − 1.2 m, along −(the two hands' wall normals) (into the wall)
        let feet_dir = -Vec3::new(a.wall_normal.x + b.wall_normal.x, 0.0, a.wall_normal.z + b.wall_normal.z).normalize_or(-into);
        if let Some([fl, fr]) = pair_along(Vec3::new(feet.x, hl.y.max(hr.y) - 1.2, feet.z), feet_dir, 0.3) {
            let entry = super::climb::ClimbEntry {
                // the catch's Ledge fill (SubState 13) enters with type 1 and ClimbData+40 = the hands 0.3 m or more
                // apart (0xE0CCC7; RE/24 AIR-15 / CLIMB-24). PORT: the Ledge step is skipped
                entry_type: super::climb::ClimbEntryType::FromLedge,
                pose_2m: Some((hl - hr).length() >= 0.3), end_action: None,
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
        if wall_catch_feet(hl, hr, a.wall_normal, b.wall_normal, collision) {
            let action = super::ledge::ACT_CATCH_WALL[long as usize];
            return Some(CatchFound::Hang { mid, normal: n, wall: true, action, one_hand: None });
        }
    }
    // 2. the free hangs at the feet + 1.95 m: both hand records on the one hold (`sub_B15AD0`(…, &v137, &v137, …)
    // 0xE0D2CA, RE/24 AIR-13)
    let free = |h: crate::guidance::GuidanceHit| {
        let n = h.wall_normal;
        let mid = h.point;
        let root = hang_root_at(mid, mid, n, LedgeHangType::Free, collision);
        let into = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        super::climb::hang_boxes_free(collision, root, into, false).then_some((mid, n))
    };
    if long {
        // sub_DFFE30: manual, the feet + 1.95 m, half (0.4, 0.5, 0.25); automatic, the hand bone + 0.2 m, half (0.4, 0.5,
        // 0.9) (v21 / v22); 0.5 m along the reach; reference (0, −0.4, 0.25) local; 70°, mask 306 (RE/24 AIR-12 / AIR-13)
        let centre = if manual { feet + Vec3::Y * 1.95 } else { auto(1.4) } + dir * along;
        let half = Vec3::new(0.4, along, if manual { 0.25 } else { 0.9 });
        let reference = centre - dir * 0.4 + Vec3::Y * 0.25;
        let hit = guidance
            .probe_box(centre, super::right_of(dir), dir, Vec3::Y, half, reference, angle, 0.1, crate::guidance::MASK_CATCH)
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
    } else if let Some((mid, n)) = catch_pair(if manual { feet + Vec3::Y * 1.95 } else { auto(1.75) })
        .filter(|[a, b]| below_release(a.point.y) && below_release(b.point.y))
        .and_then(|[a, b]| free(crate::guidance::GuidanceHit { point: (a.point + b.point) * 0.5, ..a }))
    {
        // FindClimbCatch 0xE0AC70
        return Some(CatchFound::Hang { mid, normal: n, wall: false, action: super::ledge::ACT_CATCH_FREE[0], one_hand: None });
    }
    None
}

/// The wall catch's feet (`CheckAirCatch` 0xE0CD2F–0xE0CEBE, type 3): from the hands' midpoint at the higher hand's
/// height − 1 m − 0.125·n̂ (n̂ the normalised flat sum of the two holds' wall normals), two `Guidance__FindWallSupport`
/// 0x11718B0 boxes (half 0.4 / 0.625 / 0.2, step 0.1) toward −n̂, offset (−0.15, 0, +0.1) and (+0.15, 0, −0.1) in the
/// wall's frame (x across, z up); both must hit. The free types test the body box (`sub_B2DEB0`), the wall catch does
/// not (RE/24 AIR-14).
pub fn wall_catch_feet(hl: Vec3, hr: Vec3, nl: Vec3, nr: Vec3, collision: &CollisionWorld) -> bool {
    let n = Vec3::new(nl.x + nr.x, 0.0, nl.z + nr.z).normalize_or(Vec3::new(nl.x, 0.0, nl.z).normalize_or(Vec3::Z));
    let mid = Vec3::new((hl.x + hr.x) * 0.5, hl.y.max(hr.y), (hl.z + hr.z) * 0.5);
    let into = -n;
    let c = mid - Vec3::Y + into * 0.125;
    let across = into.cross(Vec3::Y);
    use super::ledge::support_ray;
    support_ray(c - across * 0.15, n, 0.625, 0.1, collision) && support_ray(c + across * 0.15, n, 0.625, -0.1, collision)
}

/// The knee catch (type 0), `xx_fall_tr_hangknee_min_a` / `_max_a` [fall < 3 m, ≥ 3 m].
pub const KNEE_CATCH: [u32; 2] = [0x1F0C_05A7, 0x1F0C_05A6];

/// The knee catch (`CheckAirCatch` 0xE0D58B, type 0; ordinary falls and jumps only, under 9 m, after the post and beam
/// catches): `sub_E04630` takes the edges near the feet (the `sub_E008C0` box), keeps those within 45° of the facing
/// seen from the feet + 0.15 m − 0.2 m back (`sub_1173B10`), and the nearest point to the feet + 0.5 m − 0.2 m back on
/// one facing the character within 70° (`sub_116FF40`, step 0.1, mask 386); no jump target may be ahead, and the drop
/// in front of the edge (`Human__MeasureDropBeyondEdge` 0.7 / 0.3) must exceed 0.5 m. `sub_B2E240` then wants room on
/// the top (two sweeps from 1.075 m and 0.95 m above it, 0.3 m clear past 1.0 / 0.6 m). Returns the edge point and its
/// wall normal. No vertical-velocity gate: a rising body catches too (RE/24 AIR-34). PORT: the drop is one ray down
/// 0.3 m out from the edge.
pub fn knee_catch(feet: Vec3, facing: Vec3, guidance: &crate::guidance::GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3)> {
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::NEG_Z);
    let r = super::right_of(f);
    let centre = feet + Vec3::Y * 0.15;
    let eye = feet + Vec3::Y * 0.15 - f * 0.2;
    let reference = feet + Vec3::Y * 0.5 - f * 0.2;
    let mut best: Option<(f32, Vec3, Vec3)> = None;
    // mask 386: LedgeGrab, Quadruped and Kiosk edges (RE/24 AIR-34)
    for e in guidance.edges.iter().filter(|e| (1u32 << e.subtype as u32) & crate::guidance::MASK_KNEE != 0) {
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z);
        if n.length_squared() < 1e-6 {
            continue;
        }
        let n = n.normalize();
        if n.dot(f) > -70f32.to_radians().cos() {
            continue;
        }
        let p = e.closest_point(reference);
        let l = p - centre;
        if l.dot(r).abs() > 0.4 || l.dot(f).abs() > 0.4 || l.y.abs() > 0.35 {
            continue;
        }
        // the feet in front of the face, not over its top (hypothesis: the drop is measured from the feet's side)
        if (feet - p).dot(n) <= 0.0 {
            continue;
        }
        let to = Vec3::new(p.x - eye.x, 0.0, p.z - eye.z).normalize_or_zero();
        if to.dot(f) < 45f32.to_radians().cos() {
            continue;
        }
        let d = p.distance(reference);
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, p, n));
        }
    }
    let (_, p, n) = best?;
    let below = collision.ground_height(p + n * 0.3 + Vec3::Y * 0.05, 10.0).map(|h| p.y - h).unwrap_or(10.0);
    if below <= 0.5 {
        return None;
    }
    // `sub_B2E240` = `Human__KneeCatchRoomOnTop` (room above the edge, no floor test; RE/24 CLIMB-5)
    super::ledge_moves::knee_room_on_top(p, -n, collision).then_some((p, n))
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
        // the front case faces the negated averaged normal (`sub_42FA60` 0x42FA60, RE/24 AIR-17)
        (STEP_OFF[0], -n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// RE/24 AIR-12: the automatic catch searches around the Hips bone + 0.2 m (half height 0.9), so a wall top 0.5 m
    /// above the feet is caught from the Hips (0.65 m above the feet in a jump) and missed from the old stand-in centre.
    #[test]
    fn the_automatic_catch_searches_around_the_hips() {
        let (min, max) = (Vec3::new(-2.0, 0.0, -1.5), Vec3::new(2.0, 1.6, -0.5));
        let collision = CollisionWorld { boxes: vec![crate::collision::Aabb3 { min, max }], ..Default::default() };
        let mut guidance = crate::guidance::GuidanceWorld::default();
        crate::level::add_roof_edges(&mut guidance, min, max);
        let feet = Vec3::new(0.0, 1.1, -0.1);
        let catch = |hips| ledge_search(feet, Vec3::NEG_Z, Vec3::NEG_Z, 1.0, false, hips, None, &guidance, &collision);
        let found = catch(Some(feet + Vec3::Y * 0.65));
        assert!(matches!(found, Some(CatchFound::Hang { wall: true, .. })), "caught from the Hips: {found:?}");
        if let Some(CatchFound::Hang { mid, .. }) = found {
            assert!((mid.y - 1.6).abs() < 1e-3 && (mid.z + 0.5).abs() < 1e-3, "on the wall top's front edge: {mid}");
        }
        assert!(catch(None).is_none(), "the stand-in centre (feet + 1.6 m) misses it");
    }

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
        let check = |t, drop, ty, n| automatic(t, drop, true, ty, Vec3::Z, [n].into_iter());
        // only in the target-driven part of the jump (Data+505, RE/24 AIR-12)
        assert!(!automatic(0.2, false, false, Some(1), Vec3::Z, [Vec3::NEG_Z].into_iter()));
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
    fn target(position:Vec3,ty:u32)->JumpTarget {JumpTarget{position,type_flags:ty,hang:None,straight:None,pass:None,ladder:None,support:None}}
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

    /// RE/24 AIR-14: the wall catch needs the wall under both feet (two support boxes 1 m below the hands).
    #[test]
    fn the_wall_catch_needs_both_feet_on_the_wall() {
        let (hl, hr) = (Vec3::new(-0.3, 3.0, 0.0), Vec3::new(0.3, 3.0, 0.0));
        let feet = |min: Vec3, max: Vec3| {
            let mut c = CollisionWorld::default();
            c.boxes.push(Aabb3 { min, max });
            wall_catch_feet(hl, hr, Vec3::Z, Vec3::Z, &c)
        };
        assert!(feet(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 3.0, 0.0)), "a wall under the hands");
        assert!(!feet(Vec3::new(-2.0, 2.5, -2.0), Vec3::new(2.0, 3.0, 0.0)), "an overhang");
        assert!(!feet(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(0.0, 3.0, 0.0)), "the wall under one foot only");
    }

    /// RE/24 AIR-34: the knee catch takes the edges of mask 386 (LedgeGrab, Quadruped, Kiosk), not a beam's.
    #[test]
    fn the_knee_catch_reads_mask_386() {
        use crate::guidance::{GuidanceEdge, GuidanceSubType, GuidanceWorld};
        let mut c = CollisionWorld::default();
        c.boxes.push(Aabb3 { min: Vec3::new(-1.0, 0.0, -2.0), max: Vec3::new(1.0, 1.0, 0.0) });
        let found = |subtype| {
            let mut g = GuidanceWorld::default();
            g.edges.push(GuidanceEdge { p0: Vec3::new(-1.0, 1.0, 0.0), p1: Vec3::new(1.0, 1.0, 0.0), n0: Vec3::Y, n1: Vec3::Z, subtype });
            knee_catch(Vec3::new(0.0, 0.9, 0.3), Vec3::NEG_Z, &g, &c).is_some()
        };
        for s in [GuidanceSubType::LedgeGrab, GuidanceSubType::Quadruped, GuidanceSubType::Kiosk] {
            assert!(found(s), "{s:?}");
        }
        assert!(!found(GuidanceSubType::Beam));
    }
}
