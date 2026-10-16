//! Air-catch eligibility (0xE0BB70, timers 0xE01C00; RE/05 §5).
use bevy::prelude::*;
use crate::collision::CollisionWorld;

/// Manual ladder search remains height-gated even after the grasp-blend timer expires.
pub fn manual(hand: bool, fall_height: f32, drop_type: Option<usize>) -> bool {
    hand && fall_height > 0.3 && drop_type.is_none_or(|ty| ty == 2)
}

pub fn narrow(fall_height: f32, elapsed: f32, drop: bool) -> bool {
    !drop && fall_height < 9.0 && (fall_height > 0.3 || elapsed >= 0.4)
}

/// `HumanInAir__IsJumpTargetAhead` 0xE044D0: only during the target-driven part of a jump (Data+505, the port's
/// `AirMode::Jump`: `stored` is its aim), a target of type 1 / 0x10000 / 2, and a clear ray (layer 5) from the root to
/// the **stored** target Data+560 (the aim pulled back 0.5 m toward the jumper, `pull_back`), not the candidate point:
/// a ray to a point on a beam's top grazes the beam. Regardless of which side of the root the target lies on.
pub fn target_ahead(root: Vec3, stored: Option<Vec3>, type_flags: Option<u32>, world: &CollisionWorld) -> bool {
    let (Some(aim), Some(ty)) = (stored, type_flags.filter(|t| matches!(t, 1 | 0x10000 | 2))) else { return false };
    // PORT: immutable jump targets have no native entity lifetime / cell handle to invalidate.
    let delta = aim - root;
    let distance = delta.length();
    let slack = if ty == 0x10000 { 0.0005 } else { 0.0 };
    world.ray_distance(root, delta, distance, crate::layers::CHARACTER) >= distance - slack
}

/// `CheckAirCatch` 0xE0BB70's automatic catch (0xE0C385–0xE0C702, RE/24 AIR-12): its timer run out, not a drop,
/// **Data+505** (the target-driven part of a jump: `jumping`), a target of type 1 / 0x10000 / 0x40 / 0x400 and a
/// controller **wall** contact (the controller's contact list, `HumanInAir__GetCharacterController` 0xD4B410 +24, entries
/// of 64 bytes; class +40 == 4, `sub_71B840`: neither ground nor ceiling, `proxy::contact_class`) whose flat normal
/// faces the character (dot < −0.7071). The gate "target owner valid or fall height < 2 m" (`sub_420A10`, 0xE0C3F4)
/// holds for the port's targets: they are world geometry, which never goes away. The actor is the player (entity
/// +168 & 7 == 1, 0xE0C3C2).
pub fn automatic<'a>(elapsed: f32, drop: bool, jumping: bool, target_flags: Option<u32>, facing: Vec3, contacts: impl Iterator<Item = &'a crate::proxy::Contact>) -> bool {
    !drop && jumping && elapsed >= 0.2 && target_flags.is_some_and(|ty| matches!(ty, 1 | 0x10000 | 0x40 | 0x400))
        && contacts
            .filter(|c| crate::proxy::contact_class(c) == 4)
            .map(|c| Vec3::new(c.normal.x, 0.0, c.normal.z).normalize_or_zero())
            .any(|n| facing.dot(n) < -std::f32::consts::FRAC_1_SQRT_2)
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
    /// Types 3 (wall hang), 4 / 5 (free hang): the hand holds the search found ([left, right]; the type 4 catches put
    /// both on the one hold), the wall normal, the catch action, and for type 4 the hand that reaches for the edge once
    /// the catch ends (`Some(true)` = the right hand; `HumanLedge__Grasp_SetupSecondHand` 0xDCDCF0).
    Hang { hands: [Vec3; 2], normal: Vec3, wall: bool, action: u32, reach: Option<bool> },
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
/// The searches read `CheckAirCatch`'s pre-filtered reports (`catch_reports`), not the world.
/// The free hangs' room is `free_catch_room` (`Human__CatchFreeHangBoxEmpty` 0xB2DEB0).
pub fn ledge_search(
    feet: Vec3,
    reach: Vec3,
    facing: Vec3,
    fall_height: f32,
    manual: bool,
    hips: Option<Vec3>,
    release_hold: Option<Vec3>,
    pair_right: &mut Option<Vec3>,
    mut trace: Option<&mut CatchTrace>,
    guidance: &crate::guidance::GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<CatchFound> {
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
    let (hands_report, feet_report) = catch_reports(feet, guidance, collision);
    let pair_along = |report: &crate::guidance::Report, base: Vec3, dir: Vec3, half_z: f32| {
        let refs = [Vec3::new(-0.25, -0.4, 0.3), Vec3::new(0.25, -0.4, 0.3)];
        report.handhold_pair(base + dir * along, dir, Vec3::new(0.4, along, half_z), refs, angle, 0.1, crate::guidance::MASK_CATCH)
    };
    let catch_pair = |base: Vec3| pair_along(&hands_report, base, dir, half_z);
    // 1. the ledge catch: a hand pair at the feet + 1.4 m
    let ledge_base = if manual { feet + Vec3::Y * 1.4 } else { auto(1.4) };
    let ledge_pair = catch_pair(ledge_base);
    if let Some(t) = trace.as_deref_mut() {
        t.push_box(ledge_base + dir * along, dir, Vec3::new(0.4, along, half_z), TraceKind::LedgeHands, ledge_pair.is_some());
        t.holds.extend(ledge_pair.iter().flatten().map(|h| h.point));
    }
    if let Some([_, b]) = ledge_pair {
        *pair_right = Some(b.point);
    }
    if let Some([a, b]) = ledge_pair.filter(|[a, b]| below_release(a.point.y) && below_release(b.point.y)) {
        let n = a.wall_normal;
        let (hl, hr) = (a.point, b.point);
        let into = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        // the feet holds (FindCatchFeetHolds 0xE0A810): the same search from the feet's position at the higher hand's
        // height − 1.2 m, along −(the two hands' wall normals) (into the wall)
        let feet_dir = -Vec3::new(a.wall_normal.x + b.wall_normal.x, 0.0, a.wall_normal.z + b.wall_normal.z).normalize_or(-into);
        let feet_base = Vec3::new(feet.x, hl.y.max(hr.y) - 1.2, feet.z);
        let feet_pair = pair_along(&feet_report, feet_base, feet_dir, 0.3);
        if let Some(t) = trace.as_deref_mut() {
            t.push_box(feet_base + feet_dir * along, feet_dir, Vec3::new(0.4, along, 0.3), TraceKind::Feet, feet_pair.is_some());
            t.holds.extend(feet_pair.iter().flatten().map(|h| h.point));
        }
        if let Some([fl, fr]) = feet_pair {
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
            return Some(CatchFound::Hang { hands: [hl, hr], normal: n, wall: true, action, reach: None });
        }
    }
    // 2. the free hangs at the feet + 1.95 m: both hand records on the one hold (`Human__FreeHangPoseFromHolds`(…, &v137,
    // &v137, …) 0xE0D2CA, RE/24 AIR-13)
    let free = |a: crate::guidance::GuidanceHit, b: crate::guidance::GuidanceHit| {
        let mid = Vec3::new((a.point.x + b.point.x) * 0.5, a.point.y.max(b.point.y), (a.point.z + b.point.z) * 0.5);
        let sum = Vec3::new(a.wall_normal.x + b.wall_normal.x, 0.0, a.wall_normal.z + b.wall_normal.z);
        free_catch_room(mid, sum.normalize_or(a.wall_normal), collision).then_some(([a.point, b.point], mid, a.wall_normal))
    };
    if long {
        // sub_DFFE30: manual, the feet + 1.95 m, half (0.4, 0.5, 0.25); automatic, the hand bone + 0.2 m, half (0.4, 0.5,
        // 0.9) (v21 / v22); 0.5 m along the reach; reference (0, −0.4, 0.25) local; 70°, mask 306 (RE/24 AIR-12 / AIR-13)
        let centre = if manual { feet + Vec3::Y * 1.95 } else { auto(1.4) } + dir * along;
        let half = Vec3::new(0.4, along, if manual { 0.25 } else { 0.9 });
        let reference = centre - dir * 0.4 + Vec3::Y * 0.25;
        let hit = hands_report
            .world
            .probe_box(centre, super::right_of(dir), dir, Vec3::Y, half, reference, angle, 0.1, crate::guidance::MASK_CATCH)
            .filter(|h| below_release(h.point.y));
        if let Some(t) = trace.as_deref_mut() {
            t.push_box(centre, dir, half, TraceKind::FreeHang, hit.is_some());
            if let Some(h) = hit {
                t.holds.push(h.point);
                t.push_room(h.point, h.wall_normal, collision);
            }
        }
        if let Some((hands, mid, n)) = hit.and_then(|h| free(h, h)) {
            // the side by the angle from the facing to v110 = ½(v137 + v140) (0xE0D23E–0xE0D347): the long catch fills
            // only v137 (the hold), so v140 is the right hand of the last pair `FindLedgeCatch` found, left in the
            // same stack slot by the frames before (live 2026-10-14, `catch69_ahead`: a hold straight ahead plays
            // `hangfree_max_left`, RE/05 §5.7); with none, the hold itself (hypothesis)
            let mid = pair_right.map_or(mid, |r| Vec3::new((mid.x + r.x) * 0.5, mid.y.max(r.y), (mid.z + r.z) * 0.5));
            let to = Vec3::new(mid.x - feet.x, 0.0, mid.z - feet.z).normalize_or(dir);
            let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or(dir);
            // the game's signed angle about up (counter-clockwise, toward the left, positive). The hand that reaches
            // afterwards (0xDCDCF0): the right one after `hangfree_max` and after the right-side catch
            // (`hangfree_max_left`, the left hand holding), the left one after the left-side catch
            let a = (-super::right_of(f).dot(to)).atan2(f.dot(to));
            let (action, reach) = if a.abs() > 120f32.to_radians() {
                (super::ledge::ACT_CATCH_FREE[1], true)
            } else if a >= 0.0 {
                (FREE_CATCH_SIDE[1], false)
            } else {
                (FREE_CATCH_SIDE[0], true)
            };
            return Some(CatchFound::Hang { hands, normal: n, wall: false, action, reach: Some(reach) });
        }
    } else if let Some((hands, _, n)) = {
        let base = if manual { feet + Vec3::Y * 1.95 } else { auto(1.75) };
        let pair = catch_pair(base);
        if let Some(t) = trace.as_deref_mut() {
            t.push_box(base + dir * along, dir, Vec3::new(0.4, along, half_z), TraceKind::ClimbCatch, pair.is_some());
            if let Some([a, b]) = pair {
                t.holds.extend([a.point, b.point]);
                let sum = Vec3::new(a.wall_normal.x + b.wall_normal.x, 0.0, a.wall_normal.z + b.wall_normal.z);
                let mid = Vec3::new((a.point.x + b.point.x) * 0.5, a.point.y.max(b.point.y), (a.point.z + b.point.z) * 0.5);
                t.push_room(mid, sum.normalize_or(a.wall_normal), collision);
            }
        }
        pair.filter(|[a, b]| below_release(a.point.y) && below_release(b.point.y)).and_then(|[a, b]| free(a, b))
    } {
        // FindClimbCatch 0xE0AC70
        return Some(CatchFound::Hang { hands, normal: n, wall: false, action: super::ledge::ACT_CATCH_FREE[0], reach: None });
    }
    None
}

/// The free-hang catches' room (types 4 / 5, 0xE0D2EA / 0xE0D1BA): the pose from the holds
/// (`Human__FreeHangPoseFromHolds` 0xB15AD0: the root 2.4 m straight below the holds' midpoint, at the higher hold's
/// height; forward −(the flat sum of their wall normals)), then `Human__CatchFreeHangBoxEmpty` 0xB2DEB0: one box from
/// the root to 1.9 m up, 0.05 m back from it (half 0.375 across / 0.25 along / 0.95 up). Not the hang state's body boxes
/// (`sub_B2DD50`, `climb::hang_boxes_free`), which reach 0.5 m further out.
pub fn free_catch_room(mid: Vec3, n: Vec3, collision: &CollisionWorld) -> bool {
    let fwd = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let root = mid - Vec3::Y * 2.4;
    collision.obb_free(root - fwd * 0.05 + Vec3::Y * 0.95, [super::right_of(fwd), fwd, Vec3::Y], Vec3::new(0.375, 0.25, 0.95))
}

/// `CheckAirCatch`'s two pre-filtered reports (0xE0BC9D–0xE0BF12, RE/24 AIR-35, RE/26 §6): the whole edges a box at the
/// feet meets ((−1, −1, −0.2) … (1, 1, 2.2), no clip; the catch types exempt from the box filter, 0xE0BDD1), ladders
/// out (mask 8), steep edges removed, then the depth filter: detectors 0.27 out and 0.23 below the edge, −45° × 1.2
/// and 135° × 0.38; thresholds 1.2 / 0.38 for the hand searches (the ledge, climb and free-hang catches) and 0.35 / 0.38
/// for the feet holds and the edges at the feet. Returns (hands, feet).
pub fn catch_reports(feet: Vec3, guidance: &crate::guidance::GuidanceWorld, collision: &CollisionWorld) -> (crate::guidance::Report, crate::guidance::Report) {
    use crate::guidance::{DepthDetector, GuidanceSubType, Report, DEPTH_OFFSET_LIP};
    let all = guidance.report_indexed(feet, Vec3::NEG_Z, Vec3::new(-1.0, -1.0, -0.2), Vec3::new(1.0, 1.0, 2.2), false);
    let mut pre = Report::default();
    for (k, e) in all.world.edges.iter().enumerate() {
        if e.subtype == GuidanceSubType::Ladder || crate::guidance::steep(e.p1 - e.p0) {
            continue;
        }
        pre.world.edges.push(e.clone());
        pre.source.push(all.source[k]);
        pre.scratch.push([0.0; 4]);
    }
    let detectors = [DepthDetector::new(DEPTH_OFFSET_LIP, -std::f32::consts::FRAC_PI_4, 1.2), DepthDetector::new(DEPTH_OFFSET_LIP, 2.356_194_5, 0.38)];
    (pre.depth_filter(collision, &detectors, &[1.2, 0.38]), pre.depth_filter(collision, &detectors, &[0.35, 0.38]))
}

/// The wall catch's feet (`CheckAirCatch` 0xE0CD2F–0xE0CEBE, type 3): from the hands' midpoint at the higher hand's
/// height − 1 m − 0.125·n̂ (n̂ the normalised flat sum of the two holds' wall normals), two `Guidance__FindWallSupport`
/// 0x11718B0 searches (`SUPPORT_WIDE`: half 0.4 / 0.625 / 0.2, step 0.1) toward −n̂, offset (−0.15, 0, +0.1) and
/// (+0.15, 0, −0.1) in the wall's frame (x across, z up); both must find the wall (by the ray or the Surface slices). The free types test the body box (`sub_B2DEB0`), the wall catch does
/// not (RE/24 AIR-14).
pub fn wall_catch_feet(hl: Vec3, hr: Vec3, nl: Vec3, nr: Vec3, collision: &CollisionWorld) -> bool {
    let n = Vec3::new(nl.x + nr.x, 0.0, nl.z + nr.z).normalize_or(Vec3::new(nl.x, 0.0, nl.z).normalize_or(Vec3::Z));
    let mid = Vec3::new((hl.x + hr.x) * 0.5, hl.y.max(hr.y), (hl.z + hr.z) * 0.5);
    let into = -n;
    let c = mid - Vec3::Y + into * 0.125;
    use super::ledge::{wall_support, SUPPORT_WIDE};
    wall_support(c, n, -0.15, 0.1, SUPPORT_WIDE, collision) && wall_support(c, n, 0.15, -0.1, SUPPORT_WIDE, collision)
}

/// The knee catch (type 0), `xx_fall_tr_hangknee_min_a` / `_max_a` [fall < 3 m, ≥ 3 m].
pub const KNEE_CATCH: [u32; 2] = [0x1F0C_05A7, 0x1F0C_05A6];

/// The knee catch (`CheckAirCatch` 0xE0D58B, type 0; ordinary falls and jumps only, under 9 m, after the post and beam
/// catches): `sub_E04630` takes the edges near the feet (the `sub_E008C0` box), keeps those within 45° of the facing
/// seen from the feet + 0.15 m − 0.2 m back (`sub_1173B10`), and the nearest point to the feet + 0.5 m − 0.2 m back on
/// one facing the character within 70° (`sub_116FF40`, step 0.1, mask 386); no jump target may be ahead, and the drop
/// in front of the edge, measured from the root (`Human__MeasureDropBeyondEdge`(root, edge, normal, 0.7, 0.3) with its
/// lip and line checks, `jump_candidates::measure_drop_from`), must exceed 0.5 m. `sub_B2E240` then wants room on the
/// top (two sweeps from 1.075 m and 0.95 m above it, 0.3 m clear past 1.0 / 0.6 m). Returns the edge point and its wall
/// normal. No vertical-velocity gate: a rising body catches too (RE/24 AIR-34), and no test of the feet's side of the
/// edge (0xE04630 has none: the 45° cone from behind the feet keeps the edges ahead).
pub fn knee_catch(feet: Vec3, facing: Vec3, guidance: &crate::guidance::GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3)> {
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::NEG_Z);
    let r = super::right_of(f);
    let centre = feet + Vec3::Y * 0.15;
    let eye = feet + Vec3::Y * 0.15 - f * 0.2;
    let reference = feet + Vec3::Y * 0.5 - f * 0.2;
    let mut best: Option<(f32, Vec3, Vec3)> = None;
    // the edges of the catch's feet report (`HumanInAir__GatherEdgesAtFeet` 0xE0C4E7 reads it; `catch_reports`); mask
    // 386: LedgeGrab, Quadruped and Kiosk edges (RE/24 AIR-34)
    let (_, feet_report) = catch_reports(feet, guidance, collision);
    for e in feet_report.world.edges.iter().filter(|e| (1u32 << e.subtype as u32) & crate::guidance::MASK_KNEE != 0) {
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
    if super::jump_candidates::measure_drop_from(Some(feet), p, n, 0.7, 0.3, collision, Some(guidance)) <= 0.5 {
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
        let report = super::jump_candidates::EdgeReport { point: Vec3::new(p.x, feet.y, p.z), normal: n, drop: 0.0, dist: -past, edge: i, floor: false };
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

// ---------------------------------------------------------------- debug draw of the catch search (F7)

/// One box the air catch searched this frame (centre, axes right / along / up, half extents across / along / up).
#[derive(Clone, Copy, Debug)]
pub struct TraceBox {
    pub centre: Vec3,
    pub axes: [Vec3; 3],
    pub half: Vec3,
    pub kind: TraceKind,
    /// The search found something (a hold pair, a hold, a ladder rung); for the room box: the box is empty.
    pub hit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceKind {
    /// `HumanInAir__FindLadderCatch` 0xE04100.
    Ladder,
    /// `HumanInAir__FindLedgeCatch` 0xE0A990: the hand pair at the feet + 1.4 m (the Hips + 0.2 m on the automatic catch).
    LedgeHands,
    /// `HumanInAir__FindCatchFeetHolds` 0xE0A810: the feet holds under that pair (the climb catch).
    Feet,
    /// `HumanInAir__FindFreeHangCatch` 0xDFFE30 (falls of 3 m or more).
    FreeHang,
    /// `HumanInAir__FindClimbCatch` 0xE0AC70 (shorter falls): the free hang on a pair.
    ClimbCatch,
    /// `Human__CatchFreeHangBoxEmpty` 0xB2DEB0: the free hangs' room.
    Room,
}

/// What the air catch did this frame, for the debug draw: whether the grab is armed and why (`manual` = the empty hand
/// held past 0.3 m of fall; `automatic` = a target jump touching a wall), the boxes, the holds found and the outcome.
#[derive(Clone, Debug, Default)]
pub struct CatchTrace {
    pub boxes: Vec<TraceBox>,
    pub holds: Vec<Vec3>,
    pub feet: Vec3,
    pub reach: Vec3,
    pub fall_height: f32,
    pub hand_held: bool,
    pub manual: bool,
    pub automatic: bool,
    pub result: Option<String>,
}

impl CatchTrace {
    pub fn push_box(&mut self, centre: Vec3, along: Vec3, half: Vec3, kind: TraceKind, hit: bool) {
        let f = Vec3::new(along.x, 0.0, along.z).normalize_or(Vec3::NEG_Z);
        self.boxes.push(TraceBox { centre, axes: [super::right_of(f), f, Vec3::Y], half, kind, hit });
    }

    /// The free hangs' room box for a hold (`free_catch_room`), and whether it is empty.
    pub fn push_room(&mut self, mid: Vec3, n: Vec3, collision: &CollisionWorld) {
        let fwd = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        let root = mid - Vec3::Y * 2.4;
        let free = free_catch_room(mid, n, collision);
        let centre = root - fwd * 0.05 + Vec3::Y * 0.95;
        self.boxes.push(TraceBox { centre, axes: [super::right_of(fwd), fwd, Vec3::Y], half: Vec3::new(0.375, 0.25, 0.95), kind: TraceKind::Room, hit: free });
    }

    /// One HUD line: armed or not and why, the fall height, the outcome.
    pub fn summary(&self) -> String {
        let why = match (self.manual, self.automatic) {
            (true, _) => "ARMED (empty hand)".to_string(),
            (false, true) => "ARMED (automatic: target jump, wall contact)".to_string(),
            (false, false) if self.hand_held && self.fall_height <= 0.3 => format!("held, not armed yet (fall {:.2} m, needs > 0.30 m)", self.fall_height),
            (false, false) if self.hand_held => "held, not armed".to_string(),
            _ => "-".to_string(),
        };
        let band = if self.fall_height >= 3.0 { "long" } else { "short" };
        format!("grab: {why}   fall {:.2} m ({band})   {}", self.fall_height, self.result.as_deref().unwrap_or("searching"))
    }
}

/// The air catch's debug draw. F7 toggles it (on by default; `AC_CATCH_DEBUG=0` starts it off). While InAir with the
/// empty hand held or the automatic catch armed, it draws every box the catch searched: yellow the hand pair at the
/// feet + 1.4 m, orange the feet holds, cyan the free hang (falls of 3 m or more), blue the free hang under 3 m, purple
/// the ladder, white / red the free hang's room (empty / blocked); bright when the search found something, dim when
/// not. The holds found are green spheres; the feet and the reach direction (the stick, else the facing) are a sphere and
/// an arrow (green when armed, grey when not). After a catch its frame stays drawn for 2 s. The HUD shows the state.
#[derive(Resource)]
pub struct CatchDebug {
    pub show: bool,
    pub trace: Option<CatchTrace>,
    /// Elapsed time when the trace was written, and how long it stays drawn.
    pub at: f32,
    pub hold: f32,
}

impl Default for CatchDebug {
    fn default() -> Self {
        Self { show: std::env::var("AC_CATCH_DEBUG").map_or(true, |v| v != "0"), trace: None, at: 0.0, hold: 0.0 }
    }
}

impl CatchDebug {
    /// A frame's trace; `caught` keeps it drawn for 2 s (later frames do not replace it meanwhile).
    pub fn write(&mut self, trace: CatchTrace, now: f32, caught: bool) {
        if !caught && now < self.at + self.hold {
            return;
        }
        self.trace = Some(trace);
        self.at = now;
        self.hold = if caught { 2.0 } else { 0.0 };
    }

    /// The trace to draw now: this frame's, or a catch's within its 2 s.
    pub fn current(&self, now: f32) -> Option<&CatchTrace> {
        self.trace.as_ref().filter(|_| now <= self.at + self.hold.max(0.1))
    }
}

pub struct CatchDebugPlugin;

impl Plugin for CatchDebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CatchDebug>().add_systems(Update, (toggle_catch_debug, draw_catch_debug));
    }
}

fn toggle_catch_debug(keys: Res<ButtonInput<KeyCode>>, mut debug: ResMut<CatchDebug>) {
    if keys.just_pressed(KeyCode::F7) {
        debug.show = !debug.show;
    }
}

fn draw_catch_debug(time: Res<Time>, debug: Res<CatchDebug>, mut gizmos: Gizmos) {
    if !debug.show {
        return;
    }
    let Some(t) = debug.current(time.elapsed_secs()) else { return };
    for b in &t.boxes {
        let (bright, dim) = match b.kind {
            TraceKind::LedgeHands => (Color::srgb(1.0, 0.85, 0.1), Color::srgba(1.0, 0.85, 0.1, 0.35)),
            TraceKind::Feet => (Color::srgb(1.0, 0.5, 0.1), Color::srgba(1.0, 0.5, 0.1, 0.35)),
            TraceKind::FreeHang => (Color::srgb(0.1, 0.95, 1.0), Color::srgba(0.1, 0.95, 1.0, 0.35)),
            TraceKind::ClimbCatch => (Color::srgb(0.3, 0.5, 1.0), Color::srgba(0.3, 0.5, 1.0, 0.35)),
            TraceKind::Ladder => (Color::srgb(0.8, 0.3, 1.0), Color::srgba(0.8, 0.3, 1.0, 0.35)),
            TraceKind::Room => (Color::srgb(1.0, 1.0, 1.0), Color::srgb(1.0, 0.15, 0.15)),
        };
        let color = if b.hit { bright } else { dim };
        // the cube gizmo is a unit cube: local x = right, y = up, z = right × up (± along)
        let z = b.axes[0].cross(b.axes[2]);
        let rotation = Quat::from_mat3(&Mat3::from_cols(b.axes[0], b.axes[2], z));
        let transform = Transform { translation: b.centre, rotation, scale: Vec3::new(b.half.x, b.half.z, b.half.y) * 2.0 };
        gizmos.cube(transform, color);
    }
    for h in &t.holds {
        gizmos.sphere(Isometry3d::from_translation(*h), 0.06, Color::srgb(0.2, 1.0, 0.3));
    }
    let armed = t.manual || t.automatic;
    let feet_color = if armed { Color::srgb(0.2, 1.0, 0.3) } else { Color::srgb(0.6, 0.6, 0.6) };
    gizmos.sphere(Isometry3d::from_translation(t.feet), 0.08, feet_color);
    let reach = Vec3::new(t.reach.x, 0.0, t.reach.z).normalize_or_zero();
    gizmos.arrow(t.feet + Vec3::Y * 0.05, t.feet + Vec3::Y * 0.05 + reach * 0.8, feet_color);
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
        let catch = |hips| ledge_search(feet, Vec3::NEG_Z, Vec3::NEG_Z, 1.0, false, hips, None, &mut None, None, &guidance, &collision);
        let found = catch(Some(feet + Vec3::Y * 0.65));
        assert!(matches!(found, Some(CatchFound::Hang { wall: true, .. })), "caught from the Hips: {found:?}");
        if let Some(CatchFound::Hang { hands, .. }) = found {
            let mid = (hands[0] + hands[1]) * 0.5;
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
        let contact = |n: Vec3| crate::proxy::Contact { pos: Vec3::ZERO, normal: n.normalize(), dist: 0.0, fraction: 0.0, body: 0 };
        let check = |t, drop, ty, n| automatic(t, drop, true, ty, Vec3::Z, [contact(n)].iter());
        // only in the target-driven part of the jump (Data+505, RE/24 AIR-12)
        assert!(!automatic(0.2, false, false, Some(1), Vec3::Z, [contact(Vec3::NEG_Z)].iter()));
        // only a wall contact (class 4): a floor sloping toward the character faces it once flattened, but is class 1
        assert!(!check(0.2, false, Some(1), Vec3::new(0.0, 1.0, -0.6)));
        assert!(check(0.2, false, Some(1), Vec3::new(0.0, 0.6, -1.0)));
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
    fn target(position:Vec3,ty:u32)->(Option<Vec3>,Option<u32>) {(Some(position),Some(ty))}
    fn ahead(root:Vec3,(p,t):(Option<Vec3>,Option<u32>),world:&CollisionWorld)->bool {target_ahead(root,p,t,world)}
    #[test]
    fn target_guard_is_a_ray_not_a_forward_cone() {
        let mut world=CollisionWorld::default();let root=Vec3::ZERO;
        assert!(ahead(root,target(Vec3::NEG_Z*3.0,1),&world));
        assert!(!ahead(root,target(Vec3::Z*3.0,0x40),&world));
        // the flight's target-driven part has ended (Data+505 clear): no target ahead
        assert!(!target_ahead(root,None,Some(1),&world));
        world.boxes.push(Aabb3{min:Vec3::new(-0.5,-0.5,1.0),max:Vec3::new(0.5,0.5,1.5)});
        assert!(!ahead(root,target(Vec3::Z*3.0,1),&world));
        assert!(ahead(root,target(Vec3::Z,1),&world));
        assert!(!ahead(root,target(Vec3::Z*1.0004,1),&world));
        assert!(ahead(root,target(Vec3::Z*1.0004,0x10000),&world));
        world.layers.push(crate::layers::NOTHING);
        assert!(ahead(root,target(Vec3::Z*3.0,1),&world));
    }

    /// RE/24 AIR-14: the wall catch needs the wall under both feet (two support boxes 1 m below the hands). A foot whose
    /// ray misses still finds a wall that reaches into its box (±0.4 m about the hands' middle) by the Surface slices.
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
        assert!(feet(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(0.0, 3.0, 0.0)), "the wall under one foot's ray, in the other's box");
        assert!(!feet(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(-0.45, 3.0, 0.0)), "the wall ending outside the boxes");
    }

    /// `Human__CatchFreeHangBoxEmpty` 0xB2DEB0: one box from the root (2.4 m under the hold) to 1.9 m up, 0.05 m out
    /// from it. A ledge 0.3 m out from the wall under the hold blocks it; one 0.6 m below the hold's wall does not (the
    /// box stops 0.5 m under the hold), nor does a wall 0.35 m behind the hold, which the hang's own boxes would hit.
    #[test]
    fn the_free_hang_catch_room_is_one_box_under_the_hold() {
        let hold = Vec3::new(0.0, 5.0, 0.0);
        let room = |min: Vec3, max: Vec3| {
            let mut c = CollisionWorld::default();
            c.boxes.push(Aabb3 { min, max });
            free_catch_room(hold, Vec3::Z, &c)
        };
        assert!(free_catch_room(hold, Vec3::Z, &CollisionWorld::default()));
        assert!(!room(Vec3::new(-1.0, 3.0, -0.5), Vec3::new(1.0, 3.2, 0.3)), "a ledge under the hold");
        assert!(room(Vec3::new(-1.0, 4.55, -0.5), Vec3::new(1.0, 4.7, 0.3)), "above the box's top (root + 1.9 m)");
        assert!(room(Vec3::new(-1.0, 2.0, 0.35), Vec3::new(1.0, 6.0, 1.0)), "behind the body");
        assert!(!crate::player::climb::hang_boxes_free(&{ let mut c = CollisionWorld::default(); c.boxes.push(Aabb3 { min: Vec3::new(-1.0, 2.0, 0.35), max: Vec3::new(1.0, 6.0, 1.0) }); c }, hold - Vec3::Y * 2.4, Vec3::NEG_Z, false), "the hang's own boxes reach it");
    }

    /// `Human__MeasureDropBeyondEdge` 0xB19620's checks before the sweep: a lip standing on the edge, or a wall between
    /// 1 m above the feet and the point past the edge, is the floor.
    #[test]
    fn the_drop_measure_takes_the_lip_and_the_line_from_the_feet() {
        use crate::player::jump_candidates::measure_drop_from;
        // a roof 3 m high, its edge at z 0 facing +Z, the street below
        let world = |extra: &[Aabb3]| {
            let mut c = CollisionWorld::default();
            c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 0.0, -4.0), max: Vec3::new(2.0, 3.0, 0.0) });
            c.boxes.extend_from_slice(extra);
            c
        };
        let (p, n) = (Vec3::new(0.0, 3.0, 0.0), Vec3::Z);
        let feet = Vec3::new(0.0, 2.2, 0.4);
        assert!(measure_drop_from(Some(feet), p, n, 7.0, 0.3, &world(&[]), None) > 2.9, "nothing below: the sweep's maximum");
        // a cornice 0.2 m high sticking 0.25 m out over the edge: the lip ray (50° down from 0.05 m inside the edge, to the
        // point 0.3 m out) hits its top
        let lip = world(&[Aabb3 { min: Vec3::new(-2.0, 3.0, 0.0), max: Vec3::new(2.0, 3.2, 0.25) }]);
        assert!(measure_drop_from(Some(feet), p, n, 7.0, 0.3, &lip, None) < 0.0, "the lip");
        // without the feet (the other callers) only the sweep runs
        assert!(measure_drop_from(None, p, n, 7.0, 0.3, &lip, None) > 2.9);
        // a post between the feet's line and the point past the edge
        let post = world(&[Aabb3 { min: Vec3::new(-0.1, 3.0, 0.32), max: Vec3::new(0.1, 3.4, 0.37) }]);
        let d = measure_drop_from(Some(feet), p, n, 7.0, 0.3, &post, None);
        assert!(d < 0.0 && d > -0.4, "the line from 1 m above the feet: {d}");
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
