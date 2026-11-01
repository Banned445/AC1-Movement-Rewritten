//! Pass-over: vaulting a thin wall with a hand plant (RE/04 §4.1.13).

/// Flights to a pass-over target (type 2), [footl, footr]: 5 clips [front 050/300/550, up 050/300]
/// `xx_h_air_*_foot{l,r}_to_passover` (0xB1EC40).
pub const FLIGHT_PASSOVER: [u32; 2] = [0x09A0_A58D, 0x09A0_A58E];
/// Arrival receptions (0xE07D00 case 2) after a [footl, footr] flight: `…_to_passover_tr_passover_hand{r,l}`.
pub const RECEPTION_PASSOVER: [u32; 2] = [0x0A4C_9776, 0x0B56_40DB];
/// The vault (HandPassOver, `HumanLedge__HandPassOver_Update` 0xDDB800): [hand l, hand r], 2 clips
/// `xx_h_passover_hand{l,r}_{030,100}cm` blended by the wall-top thickness.
pub const VAULT: [u32; 2] = [0x09A0_A7E1, 0x09A0_A7E2];
/// No floor beyond: InAir with `passover_hand{l,r}_{030,100}cm_tr_fall` (0xDDBC60).
pub const VAULT_TO_FALL: [u32; 2] = [0x0109_B7C8, 0x0109_BB53];

/// The pass-over pull-down (pull-down type 4, `HumanLedge__PullDown_Enter` 0xDDE4D0 / `PullDown_Update` 0xDDE980), after
/// the [hand l, hand r] vault: orientation `PULLDOWN_ORIENT`, descent `PULLDOWN_DESCENT`, 2 clips each blended
/// [1 − w, w] by the thickness (LedgeData+308).
pub const PULLDOWN_ORIENT: [u32; 2] = [0x09A0_B647, 0x09A0_B649];
pub const PULLDOWN_DESCENT: [u32; 2] = [0x09A0_B648, 0x09A0_B64A];

pub const DUMPED_ACTIONS: &[u32] = &[
    FLIGHT_PASSOVER[0], FLIGHT_PASSOVER[1], RECEPTION_PASSOVER[0], RECEPTION_PASSOVER[1], VAULT[0], VAULT[1], VAULT_TO_FALL[0], VAULT_TO_FALL[1],
    PULLDOWN_ORIENT[0], PULLDOWN_ORIENT[1], PULLDOWN_DESCENT[0], PULLDOWN_DESCENT[1],
];

use bevy::prelude::*;

use super::jump_blend::{self, ActionBlend};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceSubType, GuidanceWorld};

/// The arrival on a pass-over target (0xE07D00 case 2).
#[derive(Clone, Copy, Debug)]
pub struct PassOverEntry {
    /// Near edge point on the wall top, and its outward normal (toward where the jump came from).
    pub edge: Vec3,
    pub normal: Vec3,
    pub from: Vec3,
    pub flight_foot_left: bool,
    pub flight_w: [f32; 5],
    /// The reception already played by the caller (the haystack's `xx_l_haystack_wait_to_passover_handl`, whose root
    /// ends on the edge, 0xE42B20): the Ledge goes straight on to the vault. None: InAir's arrival reception.
    pub reception: Option<ActionBlend>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassOverPhase {
    /// The reception (root to target + 0.5 m forward over 0.067 s).
    Reception,
    /// HandPassOverSubState 1 (PassOver): the vault across the top.
    Vault,
}

#[derive(Clone, Copy, Debug)]
pub struct HandPassOver {
    pub edge: Vec3,
    pub fwd: Vec3,
    pub phase: PassOverPhase,
    pub action: ActionBlend,
    pub t: f32,
    from: Vec3,
    to: Vec3,
    warp: f32,
    /// Thickness blend (LedgeData+308): 30 cm ↔ 100 cm.
    pub w: f32,
    /// The vault's hand (0 left, 1 right).
    pub hand: usize,
    pub flight_foot_left: bool,
    pub seq: u32,
    /// The wall top's far edge record (HumanLedge +1248 point, +1264 normal; +1312 set when the probe found it,
    /// 0xDDB800): none → the vault ends standing.
    pub far: Option<(Vec3, Vec3)>,
}

/// 0xE07D00 case 2 → Ledge SubState 12: the reception after a [footl, footr] flight with the flight's weights; the
/// root goes to the target + 0.5·forward = the edge (the target sits 0.5 m before it, 0xB1EC40) over 0.067 s.
pub fn enter(e: PassOverEntry, seq: u32) -> Option<HandPassOver> {
    let fwd = -Vec3::new(e.normal.x, 0.0, e.normal.z).normalize_or_zero();
    if let Some(action) = e.reception {
        return Some(HandPassOver { edge: e.edge, fwd, phase: PassOverPhase::Reception, action, t: 0.0, from: e.from, to: e.from, warp: 1e-3, w: 0.0, hand: e.flight_foot_left as usize, flight_foot_left: e.flight_foot_left, seq: seq.wrapping_add(1), far: None });
    }
    let id = RECEPTION_PASSOVER[(!e.flight_foot_left) as usize];
    jump_blend::action_items(id)?;
    let action = ActionBlend::new(id, 0, &e.flight_w);
    Some(HandPassOver { edge: e.edge, fwd, phase: PassOverPhase::Reception, action, t: 0.0, from: e.from, to: e.edge, warp: 0.067, w: 0.0, hand: e.flight_foot_left as usize, flight_foot_left: e.flight_foot_left, seq: seq.wrapping_add(1), far: None })
}

/// PORT (the ladder's top test, `ladder::wall_top_behind`): the far LedgeGrab edge (normal along +fwd, same height) within
/// 1.8 m of `edge`. Returns (far edge point, thickness). The vault itself uses the game's probe, `probe_wall_top`.
pub fn far_edge(edge: Vec3, fwd: Vec3, guidance: &GuidanceWorld) -> Option<(Vec3, f32)> {
    far_edge_record(edge, fwd, guidance).map(|(q, d, _)| (q, d))
}

/// `far_edge` with the far edge's outward normal: (point, thickness, normal).
pub fn far_edge_record(edge: Vec3, fwd: Vec3, guidance: &GuidanceWorld) -> Option<(Vec3, f32, Vec3)> {
    guidance
        .edges
        .iter()
        .filter(|f| f.subtype == GuidanceSubType::LedgeGrab && Vec3::new(f.n1.x, 0.0, f.n1.z).normalize_or_zero().dot(fwd) > 0.9 && (f.p0.y - edge.y).abs() < 0.1)
        .map(|f| (f.closest_point(edge + fwd * 0.9), Vec3::new(f.n1.x, 0.0, f.n1.z).normalize_or(fwd)))
        .map(|(q, n)| (q, Vec3::new(q.x - edge.x, 0.0, q.z - edge.z).dot(fwd), n))
        .filter(|(q, d, _)| *d > 0.02 && *d <= 1.8 && Vec2::new(q.x - (edge.x + fwd.x * d), q.z - (edge.z + fwd.z * d)).length() < 0.1)
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// The wall top `Human__ProbeWallTopThickness` 0xB18FB0 finds: the near and far edges where the facing line crosses
/// them, the far edge's wall normal (HumanLedge +1264), and the distance between the two crossings (+0xA0).
#[derive(Clone, Copy, Debug)]
pub struct WallTop {
    pub near: Vec3,
    pub far: Vec3,
    pub far_normal: Vec3,
    pub thickness: f32,
}

/// `Human__ProbeWallTopThickness` 0xB18FB0 as `HumanLedge__HandPassOver_Update` 0xDDB800 calls it (verified
/// 2026-10-31): from `root` − 0.3·fwd − 0.25 up, a vertical `GuidanceZonePlane` (0x658780; its AABB 0x658C50 runs
/// 1.8 m along the facing and 0.6 m up from that corner, no width) takes every guidance edge (type mask −1) that
/// crosses it. Each edge no steeper than 40° (`Guidance__IsNotSteep` 0xB12480: |dir.up| ≤ 0.6428) is cut by the
/// vertical plane through the facing line (`Math__SegmentPlaneIntersect` 0x9765E0); its wall normal is the edge
/// normal with the lower up component, flattened. The edge counts only with 0.45 m or more of drop on that side:
/// `Human__RayDistance` from the crossing + 0.5 up + 0.15·normal, 2.0 m down on the player's layer 21, less 0.5.
/// A normal against the facing is a near edge, along it (dot ≥ 0) a far edge; the nearest of each to the start
/// (horizontally, 0xB193D1 / 0xB193A5). Both must be found. The far edge's record (`GuidanceHit__Set` 0xB18600 into
/// out+0x50) is the crossing point and its unflattened normal; out+0xA0 the crossings' distance.
pub fn probe_wall_top(root: Vec3, fwd: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<WallTop> {
    let fwd = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
    let start = root - fwd * 0.3 - Vec3::Y * 0.25;
    // the plane through the facing line: normal = fwd × up (0xB19178)
    let side = fwd.cross(Vec3::Y);
    let mut near: Option<(f32, Vec3)> = None;
    let mut far: Option<(f32, Vec3, Vec3)> = None;
    for e in &guidance.edges {
        let d = e.p1 - e.p0;
        if d.abs().max_element() <= 0.0005 || d.normalize().y.abs() > 0.642_787_6 {
            continue;
        }
        let (a, b) = ((e.p0 - start).dot(side), (e.p1 - start).dot(side));
        if a * b > 0.0 || (a - b).abs() < 1e-9 {
            continue;
        }
        let x = e.p0 + d * (a / (a - b));
        // inside the zone: 0..1.8 along the facing, 0..0.6 up from the start
        let (along, up) = ((x - start).dot(fwd), x.y - start.y);
        if !(0.0..=1.8).contains(&along) || !(0.0..=0.6).contains(&up) {
            continue;
        }
        let wall = if e.n1.y <= e.n0.y { e.n1 } else { e.n0 };
        let n = Vec3::new(wall.x, 0.0, wall.z).normalize_or_zero();
        let from = Vec3::new(x.x, x.y + 0.5, x.z) + n * 0.15;
        if collision.ray_distance(from, -Vec3::Y, 2.0, crate::layers::MAIN_CHARACTER) - 0.5 < 0.45 {
            continue;
        }
        let dist2 = Vec3::new(x.x - start.x, 0.0, x.z - start.z).length_squared();
        if n.dot(fwd) >= 0.0 {
            if far.is_none_or(|(best, _, _)| dist2 < best) {
                far = Some((dist2, x, wall));
            }
        } else if near.is_none_or(|(best, _)| dist2 < best) {
            near = Some((dist2, x));
        }
    }
    let ((_, near), (_, far, far_normal)) = (near?, far?);
    Some(WallTop { near, far, far_normal, thickness: (far - near).length() })
}

pub enum PassOverOut {
    Stay,
    /// Vault done (release, 0xDE0720) with no far edge (+1312 = 0): Ground at the root (fill 0xDCFA40).
    Ground,
    /// A far edge with room at it and 3 m or more of drop beyond (`PassOverWantsPullDown` 0xDD2BD0 →
    /// `PassOverToPullDown` 0xDE0220): pull-down type 4 from the far edge (point, outward normal).
    PullDown { far: Vec3, normal: Vec3 },
    /// Otherwise (`PassOverWantsInAir` 0xDCC0E0): InAir with `passover_*_tr_fall` [1 − w, w] (0xDDBC60).
    Fall(ActionBlend),
}

/// `HumanLedge__HandPassOver_Update` 0xDDB800 + `StateHandPassOver_Update` 0xDE0720.
pub fn update(p: &mut HandPassOver, dt: f32, feet: &mut Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> PassOverOut {
    p.t += dt;
    let k = (p.t / p.warp.max(1e-3)).min(1.0);
    *feet = p.from.lerp(p.to, k);
    match p.phase {
        PassOverPhase::Reception => {
            if k < 1.0 {
                return PassOverOut::Stay;
            }
            // warp done → the vault by the reception's item foot (0xDDB8C6: word & 0xC == 4 → hand l 0x09A0A7E1, else
            // hand r): the footl flight's `tr_passover_handr` ends 8 → hand r, the footr flight's and the haystack's
            // `…_handl` end 4 → hand l (verified 2026-10-31). Thickness blend w = min(thickness, 1); the root goes over
            // the vault to the far edge's crossing point (`ClimbHoldRecord__Copy` 0xB0FC50 copies the probe's far record,
            // out+0x50, into HumanLedge +1248) + 0.3·(1 − w)·forward; no wall top → root + 0.3·fwd, w = 0 (verified)
            let top = probe_wall_top(*feet, p.fwd, guidance, collision);
            p.far = top.map(|t| (t.far, t.far_normal));
            let (to, w) = match top {
                Some(t) => {
                    let w = t.thickness.min(1.0);
                    (t.far + p.fwd * 0.3 * (1.0 - w), w)
                }
                None => (*feet + p.fwd * 0.3, 0.0),
            };
            let hand = if p.flight_foot_left { 1 } else { 0 };
            let Some(_) = jump_blend::action_items(VAULT[hand]) else { return PassOverOut::Ground };
            p.action = ActionBlend::new(VAULT[hand], 0, &[1.0 - w, w]);
            p.hand = hand;
            p.w = w;
            p.phase = PassOverPhase::Vault;
            p.from = *feet;
            p.to = to;
            p.warp = p.action.duration().max(0.05);
            p.t = 0.0;
            p.seq = p.seq.wrapping_add(1);
            PassOverOut::Stay
        }
        PassOverPhase::Vault => {
            // the release point: `StateHandPassOver_Update` 0xDE0720 (Ground), `PassOverWantsPullDown` 0xDD2BD0 and
            // `PassOverWantsInAir` 0xDCC0E0 all wait for `AnimSlot__IsActionDone` 0x5017B0 (the slot's running state +40
            // bit 0) in stage 1; the vault actions have one item (0x09A0A7E3 / 0x09A0A7E4), so it is the vault's end
            if p.t < p.action.duration() {
                return PassOverOut::Stay;
            }
            // the release (0xDE0720): no far edge → Ground; a far edge with room at it (`sub_B2E4F0`) and a drop of
            // 3 m or more beyond (`Human__MeasureDropBeyondEdge`(3.5, 0.02)) → the pull-down; else InAir
            let Some((far, normal)) = p.far else { return PassOverOut::Ground };
            let drop = super::jump_candidates::measure_drop(far, normal, 3.5, 0.02, collision);
            if drop >= 3.0 && super::narrow::pull_down_clear(*feet, far, normal, collision) {
                PassOverOut::PullDown { far, normal }
            } else {
                PassOverOut::Fall(ActionBlend::new(VAULT_TO_FALL[p.hand], 0, &[1.0 - p.w, p.w]))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::Aabb3;
    use crate::guidance::GuidanceEdge;

    /// A wall top facing −Z: near edge at z 0, far edge at z −thick (height `far_y`, tilted by `slope` end to end),
    /// over a floor at y 0 (raised to `far_floor` beyond it). The root stands on the near edge after the reception.
    fn wall(thick: f32, far_y: f32, far_floor: Option<f32>, slope: f32) -> (GuidanceWorld, CollisionWorld) {
        let aabb = |min: Vec3, max: Vec3| Aabb3 { min, max };
        let mut boxes = vec![aabb(Vec3::new(-20.0, -1.0, -20.0), Vec3::new(20.0, 0.0, 20.0)), aabb(Vec3::new(-2.0, 0.0, -thick), Vec3::new(2.0, 1.0, 0.0))];
        if let Some(f) = far_floor {
            boxes.push(aabb(Vec3::new(-2.0, 0.0, -6.0), Vec3::new(2.0, f, -thick)));
        }
        let edge = |p0: Vec3, p1: Vec3, n1: Vec3| GuidanceEdge { p0, p1, n0: Vec3::Y, n1, subtype: GuidanceSubType::LedgeGrab };
        let edges = vec![
            edge(Vec3::new(-2.0, 1.0, 0.0), Vec3::new(2.0, 1.0, 0.0), Vec3::Z),
            edge(Vec3::new(-2.0, far_y - slope, -thick), Vec3::new(2.0, far_y + slope, -thick), Vec3::NEG_Z),
        ];
        (GuidanceWorld { edges, ..Default::default() }, CollisionWorld { boxes, ..Default::default() })
    }

    fn probe(w: &(GuidanceWorld, CollisionWorld)) -> Option<WallTop> {
        probe_wall_top(Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_Z, &w.0, &w.1)
    }

    #[test]
    fn a_30cm_wall_top_gives_the_far_crossing_and_its_thickness() {
        let t = probe(&wall(0.3, 1.0, None, 0.0)).expect("a wall top");
        assert!((t.thickness - 0.3).abs() < 1e-4 && (t.far - Vec3::new(0.0, 1.0, -0.3)).length() < 1e-4, "{t:?}");
        assert!((t.near - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-4 && t.far_normal == Vec3::NEG_Z, "{t:?}");
    }

    #[test]
    fn the_zone_reaches_1_8m_from_0_3m_behind_the_near_edge() {
        assert!(probe(&wall(1.45, 1.0, None, 0.0)).is_some());
        assert!(probe(&wall(1.55, 1.0, None, 0.0)).is_none());
    }

    #[test]
    fn the_zone_is_0_6m_tall_from_0_25m_below_the_root() {
        assert!(probe(&wall(0.5, 1.3, None, 0.0)).is_some());
        assert!(probe(&wall(0.5, 1.4, None, 0.0)).is_none());
    }

    #[test]
    fn a_far_edge_needs_0_45m_of_drop_beyond_it() {
        assert!(probe(&wall(0.3, 1.0, Some(0.5), 0.0)).is_some());
        assert!(probe(&wall(0.3, 1.0, Some(0.6), 0.0)).is_none());
    }

    #[test]
    fn an_edge_steeper_than_40_degrees_is_no_wall_top() {
        assert!(probe(&wall(0.3, 1.0, None, 1.5)).is_some());
        assert!(probe(&wall(0.3, 1.0, None, 1.8)).is_none());
    }
}
