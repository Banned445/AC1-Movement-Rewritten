//! HumanWalling (context 11): the run up a wall (RE/05 §1).
//!
//! Entry: Movement event 49 (guard 0xDA54F0 → 0xDA2B10 → wall test 0xE18390), start 0xDA2C30. Sub-states
//! EntryA → EntryB → Vertical → VerticalEnd (`UpdateWallingSubState` 0xE37590); ledge probes A–D hand over to
//! the Ledge context with the game's `wallingfront_*_tr_*` exit clips; the rebound jump (0xE365C0) leaves to
//! InAir. Only the front (vertical) wall run exists in the game's data (no `wallingside` clips).

use bevy::prelude::*;

use super::air::{FallOrigin, InAirEntry};
use super::jump_blend::{self, ActionBlend};
use super::ledge::{hang_root, LedgeEntry, LedgeHangType, LedgeSubState};
use super::ledge_moves::{self, LedgeMove, MoveKind};
use super::targets::JumpTarget;
use super::{right_of, switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::CollisionWorld;
use crate::guidance::GuidanceWorld;
use crate::input::PadInput;

/// HumanWalling block actions (matched by clip name; the exe requests some through runtime ids 68 / 69).
pub const ENTRY_A: u32 = 0x00D8_2C5C; // xx_h_wallingfront_entry_footl_a (id 68, 0xDA2C30)
pub const ENTRY_B: u32 = 0x0100_4122; // xx_h_wallingfront_entry_footl_b
pub const STEP1: u32 = 0x00D8_2C9F; // xx_h_wallingfront_step1_footr (id 69, SubState 3 Vertical)
pub const STEP1_TO_FALL: u32 = 0x00D8_2CE2;
pub const VERTICAL_END: u32 = 0x0121_847A; // step1_footr_tr_rebound a, b (SubState 6)
pub const ENTRY_TO_FALL: u32 = 0x00FF_7CA1;
pub const ENTRY_TO_KNEE: u32 = 0x0100_759B; // entry_footl_tr_hangknee_{131,200}cm (probe A pull-up)
pub const ENTRY_TO_PASSOVER: u32 = 0x0109_B6CF;
pub const ENTRY_TO_HANGFREE: u32 = 0x0564_BB1D; // 90487581, probe B (4-way)
pub const STEP1_TO_KNEE: u32 = 0x0106_CFC2; // probe D pull-up, 201 / 250 cm
pub const STEP1_TO_HANGWALL: u32 = 0x0106_CFC5; // probe D hang, 251 / 430 cm × 0 / 50 cm
pub const STEP1_TO_HANGFREE: u32 = 0x0564_BB1E; // 90487582, probe C
pub const STEP1_TO_PASSOVER: u32 = 0x0109_BB50;
pub const ENTRY_TO_REBOUND: u32 = 0x0115_7E35;
pub const STEP1_TO_REBOUND: u32 = 0x0115_7E36;
/// `xx_h_rebound_footr_tr_fall`: VerticalEnd's last action, played when its rebound clip ends and the walling is left
/// (0xE37590 SubState 6, flag +0x32).
pub const REBOUND_TO_FALL: u32 = 0x0129_1762;

/// The Ground's lead-in to the wall run from the lean and from the standing FreeRun (event 49 with HG+1492 = 0:
/// `ObstacleCollision_HandleEvent` 0xDA6FF0 / `HumanGround__FreeRun_WallingGuardA` 0xDA2DD0 → 0xD8E740 / 0xD89AA0 →
/// 0xD857C0 / 0xD8A460): `xx_h_impultionstraight_footl_to_wallingfront_step1_footr`, then the wall run.
pub const GROUND_LEAD_IN: u32 = 0x02F4_B310;

pub const DUMPED_ACTIONS: &[u32] = &[
    GROUND_LEAD_IN, ENTRY_A, ENTRY_B, STEP1, STEP1_TO_FALL, VERTICAL_END, ENTRY_TO_FALL, ENTRY_TO_KNEE, ENTRY_TO_PASSOVER, ENTRY_TO_HANGFREE,
    STEP1_TO_KNEE, STEP1_TO_HANGWALL, STEP1_TO_HANGFREE, STEP1_TO_PASSOVER, ENTRY_TO_REBOUND, STEP1_TO_REBOUND, REBOUND_TO_FALL,
];

/// The interpreter's walling state (`GoAssassinActionInterpreter` 0xEE05E0) pushes off straight back when the stick
/// points within 50° of the facing (its angle to the facing, interp +0x1244), else along the stick.
const PUSH_STRAIGHT_BACK: f32 = 0.872_664_6;

/// entity+0x7C: the reference length of every walling band. 1.0 for Altaïr: the entry clip lifts the root
/// exactly 1.0 m, which is the warp target's height (wall hit at 1.3·h minus 0.3·h, 0xE18390).
pub const H: f32 = 1.0;

/// HumanWallingData::SubState (RE/05 §1.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WallingSubState {
    #[default]
    EntryA = 1,
    EntryB = 2,
    Vertical = 3,
    VerticalEnd = 6,
}

#[cfg(test)]
mod beam_warp_tests {
    use super::*;

    #[test]
    fn beam_wall_run_reaches_contact_at_130_ms_then_keeps_root_motion() {
        let mut w = HumanWallingData::default();
        let target = Vec3::new(0.0, 1.0, -0.8);
        w.enter(WallingEntry { contact: target, normal: Vec3::Z, slope: Vec3::Z, from: Vec3::ZERO, warp_duration: Some(0.13) });
        assert!(w.root_at(0.0).length() < 1e-6);
        assert!(w.root_at(0.13).distance(target) < 1e-5);
        assert!(w.root_at(0.14).distance(w.root_at(0.13)) < 0.1);
        assert!(w.root_at(0.3).y > target.y);
    }

    #[test]
    fn ledge_hands_keep_left_and_right_whichever_way_the_edge_runs() {
        // facing +Z into a wall whose outward normal is -Z: the climber's right is -X
        for dir in [Vec3::X, Vec3::NEG_X] {
            let c = LedgeCandidate { point: Vec3::new(0.0, 2.0, 0.0), normal: Vec3::NEG_Z, edge_dir: dir, height: 2.0, dist: 0.0 };
            let (l, r) = hands(&c);
            assert!(l.x > r.x, "left hand on the climber's left for edge direction {dir}");
            assert!((l.distance(r) - 0.4).abs() < 1e-6);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WallingEntry {
    /// Warp target of the entry (0xE18390 output): 0.5·h out from the wall, h above the feet.
    pub contact: Vec3,
    /// Outward wall normal.
    pub normal: Vec3,
    /// The wall's tilted outward normal (`slope_normal`; `normal` on a vertical wall): the entry warp pitches the
    /// character's frame to face along its reverse (RE/24 WALL-3).
    pub slope: Vec3,
    pub from: Vec3,
    /// Beam event 15 uses a 0.13 s warp (0xB263B0); ground entries use the action length.
    pub warp_duration: Option<f32>,
}

#[derive(Debug, Default)]
pub struct HumanWallingData {
    pub sub_state: WallingSubState,
    pub normal: Vec3,
    pub action: Option<ActionBlend>,
    /// Second item of a two-item action (VerticalEnd a → b).
    pub action_next: Option<ActionBlend>,
    pub t: f32,
    pub from: Vec3,
    /// Where the root must be when the action ends (motion warp, `sub_711130`).
    pub to: Vec3,
    pub seq: u32,
    /// HumanWallingData+0x14.
    pub start_height: f32,
    warp_duration: Option<f32>,
    /// The root frame's pitch (RE/24 WALL-3): the entry warp's target frame (forward = −slope normal, up = right ×
    /// forward, 0xDA2D77 → `sub_710C20`), held through EntryB and Vertical, turned back upright in VerticalEnd.
    pub pitch: Quat,
}

/// The pitch that turns the flat facing (−`normal` flattened) onto −`slope` about the wall's horizontal axis.
pub fn wall_pitch(normal: Vec3, slope: Vec3) -> Quat {
    let f = -Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
    let s = slope.normalize_or_zero();
    if f == Vec3::ZERO || s == Vec3::ZERO {
        return Quat::IDENTITY;
    }
    Quat::from_rotation_arc(f, -s)
}

/// VerticalEnd's turn back upright (0xE39274–0xE392E7): the frame's up lerps toward world up by 4·dt each frame
/// (`sub_42ECC0`, normalised), the forward re-squared to it (`sub_52BD40`).
pub fn pitch_toward_upright(pitch: Quat, dt: f32) -> Quat {
    let up = pitch * Vec3::Y;
    let up = (up + (Vec3::Y - up) * (4.0 * dt)).normalize_or(Vec3::Y);
    Quat::from_rotation_arc(Vec3::Y, up)
}

fn single(id: u32, item: usize) -> Option<ActionBlend> {
    let n = jump_blend::action_items(id)?.get(item)?.len();
    let mut w = vec![0.0; n.max(1)];
    w[0] = 1.0;
    Some(ActionBlend::new(id, item, &w))
}

fn blended(id: u32, item: usize, w: &[f32]) -> Option<ActionBlend> {
    let n = jump_blend::action_items(id)?.get(item)?.len();
    let mut v = vec![0.0; n];
    for (k, x) in w.iter().enumerate().take(n) {
        v[k] = *x;
    }
    Some(ActionBlend::new(id, item, &v))
}

impl HumanWallingData {
    /// `HumanGround` 0xDA2C30: SubState EntryA, WallingType Vertical, action 68 with the root warped to the
    /// contact over the action's length.
    pub fn enter(&mut self, e: WallingEntry) {
        self.normal = e.normal;
        self.pitch = wall_pitch(e.normal, e.slope);
        self.start_height = e.from.y;
        self.play(WallingSubState::EntryA, single(ENTRY_A, 0), None, e.from, Some(e.contact));
        self.warp_duration = e.warp_duration;
    }

    fn play(&mut self, sub: WallingSubState, a: Option<ActionBlend>, next: Option<ActionBlend>, from: Vec3, to: Option<Vec3>) {
        self.sub_state = sub;
        self.warp_duration = None;
        self.action = a;
        self.action_next = next;
        self.t = 0.0;
        self.from = from;
        let facing = self.facing();
        let disp = |a: &Option<ActionBlend>| a.map(|a| a.disp(1.0)).unwrap_or([0.0; 3]);
        let (da, db) = (disp(&a), disp(&next));
        let d = [da[0] + db[0], da[1] + db[1], da[2] + db[2]];
        self.to = to.unwrap_or(from + right_of(facing) * d[0] + facing * d[1] + Vec3::Y * d[2]);
        self.seq = self.seq.wrapping_add(1);
    }

    pub fn facing(&self) -> Vec3 {
        -Vec3::new(self.normal.x, 0.0, self.normal.z).normalize_or_zero()
    }

    fn duration(&self) -> f32 {
        self.action.map(|a| a.duration()).unwrap_or(0.1) + self.action_next.map(|a| a.duration()).unwrap_or(0.0)
    }

    /// Root at time t: the actions' displacement plus a linear correction onto `to` (FROMANIM + warp).
    fn root_at(&self, t: f32) -> Vec3 {
        let f = self.facing();
        let w = |d: [f32; 3]| right_of(f) * d[0] + f * d[1] + Vec3::Y * d[2];
        let d1 = self.action.map(|a| a.duration()).unwrap_or(0.1);
        let total = self.duration();
        let disp_at = |t: f32| -> Vec3 {
            let a = self.action.map(|a| w(a.disp((t / d1).min(1.0)))).unwrap_or(Vec3::ZERO);
            let b = match self.action_next {
                Some(n) if t > d1 => w(n.disp(((t - d1) / (total - d1).max(1e-4)).min(1.0))),
                _ => Vec3::ZERO,
            };
            a + b
        };
        let warp = self.warp_duration.unwrap_or(total).max(1e-4);
        let end = self.from + disp_at(warp);
        self.from + disp_at(t) + (self.to - end) * (t / warp).min(1.0)
    }

    /// The action playing now and its phase (for the animator).
    pub fn current(&self) -> Option<(ActionBlend, f32)> {
        let d1 = self.action.map(|a| a.duration()).unwrap_or(0.1);
        match self.action_next {
            Some(n) if self.t > d1 => Some((n, ((self.t - d1) / n.duration().max(1e-4)).min(1.0))),
            _ => self.action.map(|a| (a, (self.t / d1.max(1e-4)).min(1.0))),
        }
    }
}

// ---------------------------------------------------------------- the wall ahead (0xE18390)

/// The walling probes' collision layer (14, `Human__RayClear` / `Human__RayDistance` in 0xE149E0 / 0xE14590).
const WALL_RAY_LAYER: u8 = crate::layers::RAYCAST_NO_CHARACTERS;
/// The fuzzy-zone layers a wall hit must not be on (`sub_E1C1F0`).
const FUZZY_LAYERS: [u8; 5] = [crate::layers::FUZZY_ZONE, crate::layers::FUZZY_ZONE_ALL_CHARACTER, crate::layers::FUZZY_ZONE_NO_CAMERA, crate::layers::FUZZY_ZONE_ALL_CHARACTER_NO_CAMERA, crate::layers::FUZZY_ZONE_NO_CAMERA_ALLOW_ASSASSINATION];

/// `Human__WallingWidthCheck` 0xE149E0 (RE/24 WALL-1): from the hit + 0.001·n, a ray 0.5 m along n turned −45° and then
/// +45° about up. Both clear → accept the hit. A ray that hits found something jutting out beside the contact (an inner
/// corner, a pilaster): the contact becomes that hit (0xE14AED / 0xE14BD4) and `sub_E14790` steps from it along n × (s·up)
/// (s = −1 after the −45° ray, +1 after the +45° one: away from the jut): a ray 0.3 m along it must be clear, then from
/// there a ray back along −n as long as the distance to the plane through the jut's hit (zero: the step runs along it)
/// + 0.25 m must hit (0xE148C8–0xE14934); that hit becomes the contact. A failed step rejects the wall, so a jut standing
/// more than 0.25 m out from the wall behind it does. Narrow walls, posts and a wall's outer end are accepted.
fn width_check(hit: Vec3, n: Vec3, collision: &CollisionWorld) -> Option<Vec3> {
    let start = hit + n * 0.001;
    for a in [-std::f32::consts::FRAC_PI_4, std::f32::consts::FRAC_PI_4] {
        let dir = Quat::from_rotation_y(a) * n;
        let Some((d, _)) = collision.ray_hit(start, dir, 0.5, WALL_RAY_LAYER) else { continue };
        let jut = start + dir * d;
        let along = -(dir - n * dir.dot(n)).normalize_or_zero();
        if collision.ray_hit(jut, along, 0.3, WALL_RAY_LAYER).is_some() {
            return None;
        }
        let from = jut + along * 0.3;
        // the distance to the plane through the jut's hit with normal n (`Vec_ProjectOntoPlane` 0xE148C8), + 0.25
        let back = (from - jut).dot(n).abs() + 0.25;
        let (d2, _) = collision.ray_hit(from, -n, back, WALL_RAY_LAYER)?;
        return Some(from - n * d2);
    }
    Some(hit)
}

/// `Human__WallingSlopeDir` 0xE14590 + `Human__WallingSlopeLimit` 0xE14FB0 (RE/24 WALL-3): a ray from the hit + 0.75·n +
/// 1.0 up back along −n, 1.5 m. Clear → the wall's normal is n. Hit → the line from the hit up to that hit, turned 90°
/// about n × up, is the wall's tilted outward normal. Its signed angle from n about n × up (positive: tilted up, the wall
/// leaning back): |a| ≥ 45° rejects the wall; it is clamped to −10°…+20°. Returns the slope normal.
fn slope_normal(hit: Vec3, n: Vec3, collision: &CollisionWorld) -> Option<Vec3> {
    let from = hit + n * 0.75 + Vec3::Y;
    let Some((d, _)) = collision.ray_hit(from, -n, 1.5, WALL_RAY_LAYER) else { return Some(n) };
    let up_line = (from - n * d - hit).normalize_or_zero();
    let k = n.cross(Vec3::Y).normalize_or_zero();
    let mut s = Quat::from_axis_angle(k, -std::f32::consts::FRAC_PI_2) * up_line;
    if s.dot(n) < 0.0 {
        s = -s;
    }
    let a = n.cross(s).dot(k).atan2(n.dot(s));
    if a.abs() >= std::f32::consts::FRAC_PI_4 {
        return None;
    }
    let a = a.clamp(-10f32.to_radians(), 20f32.to_radians());
    Some(Quat::from_axis_angle(k, a) * n)
}

/// `HumanWalling` entry test (event 49 guard 0xDA2B10 → 0xE18390): a ray from 1.3·h above the feet along the facing,
/// 1.5·h long, on the walling layer; the hit's own normal flattened (0xE184EA, RE/24 WALL-2) must face back within 45°;
/// the width check (`width_check`) may move the contact or reject; the slope (`slope_normal`) may reject. Returns the warp
/// target (contact + 0.5·h along the slope normal, 0.3·h down: feet + h on a vertical wall) and the flat outward normal.
/// Returns (warp target, flat outward normal, slope normal); the slope normal pitches the run (`wall_pitch`).
pub fn wall_ahead(feet: Vec3, forward: Vec3, collision: &CollisionWorld) -> Option<(Vec3, Vec3, Vec3)> {
    let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let o = feet + Vec3::Y * (1.3 * H);
    // the wall ray: filter 47 (`sub_4FB6E0`(47) 0xE18404, RaycastFuzzyZone); a hit on a fuzzy zone (layers 24 / 26 / 25 /
    // 33 / 41, `sub_E1C1F0`) is no wall (`sub_E14410`). PORT: the owner (CharacterComponent) and material rejections
    // (type 2, class 3 / 8–9: the haystacks) have no counterpart: no characters, the haystacks are not solid (RE/24
    // WALL-18)
    let (d, normal, hit_layer) = collision.ray_hit_layer(o, f, 1.5 * H, crate::layers::RAYCAST_FUZZY_ZONE)?;
    if FUZZY_LAYERS.contains(&hit_layer) {
        return None;
    }
    let n = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
    if n == Vec3::ZERO || n.dot(-f) < 45f32.to_radians().cos() {
        return None;
    }
    let hit = width_check(o + f * d, n, collision)?;
    let slope = slope_normal(hit, n, collision)?;
    let contact = hit + slope * (0.5 * H) - Vec3::Y * (0.3 * H);
    Some((contact, n, slope))
}

// ---------------------------------------------------------------- ledge probes (0xE36BD0)

struct LedgeCandidate {
    point: Vec3,
    normal: Vec3,
    edge_dir: Vec3,
    /// Height above the root.
    height: f32,
    /// Horizontal distance from the probe origin.
    dist: f32,
}

/// `HumanWalling__FindLedgeCandidates` 0xE36BD0 (RE/24 WALL-5 / WALL-6): the report of the game's box (±`width` sideways,
/// 0..`height` up, 0..`depth` ahead of `origin`), type mask 0x182 (LedgeGrab, Quadruped, Kiosk), edges steeper than 40°
/// removed (`GuidanceReport__RemoveSteepEdges`); chains (`GuidanceChain__Build`: reference −forward, walls turned within
/// ±45°); per chain the grab point from the **root** along the flat forward (`GuidanceChain__FindGrabPoint`, hand offsets
/// [0.5, 0.1], margin 0.3: a chain leaving less than 0.5 m on a side is dropped); a clear ray (layer 5) from the root −
/// 0.5·forward to the grab + 0.05·n. Sorted by height (`SortCandidates` 0xE36A40): highest first, or lowest first for
/// probe C. Between the steep removal and the chains, the depth filter (`Human__DepthDetectReport` 0xE36FDF /
/// `Human__DepthFilterReport` 0xE37008): detectors 0.27 out and 0.23 below the edge, −75° × `radius` (a9: 0.8 for probe
/// A, 1.2 for the others) and 135° × 0.42, the same thresholds; its scratch is what the grab point's hand offsets may
/// reach past a chain's ends (RE/26 §6).
#[allow(clippy::too_many_arguments)]
fn find_ledge(g: &GuidanceWorld, origin: Vec3, root: Vec3, facing: Vec3, width: f32, height: f32, depth: f32, radius: f32, highest_first: bool, collision: &CollisionWorld) -> Vec<LedgeCandidate> {
    use crate::guidance::ChainParams;
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
    let right = f.cross(Vec3::Y);
    let report = g.report_indexed(origin, f, Vec3::new(-width, 0.0, 0.0), Vec3::new(width, depth, height), true);
    let keep: Vec<usize> = (0..report.world.edges.len())
        .filter(|&i| {
            let e = &report.world.edges[i];
            (1u32 << e.subtype as u32) & 0x182 != 0 && !crate::guidance::steep(e.p1 - e.p0)
        })
        .collect();
    let kept = crate::guidance::Report {
        world: GuidanceWorld { edges: keep.iter().map(|&i| report.world.edges[i].clone()).collect(), ..Default::default() },
        source: keep.clone(),
        scratch: keep.iter().map(|&i| report.scratch[i]).collect(),
    };
    use crate::guidance::{DepthDetector, DEPTH_OFFSET_LIP};
    let detectors = [DepthDetector::new(DEPTH_OFFSET_LIP, -1.308_997, radius), DepthDetector::new(DEPTH_OFFSET_LIP, 2.356_194_5, 0.42)];
    let filtered = kept.depth_filter(collision, &detectors, &[radius, 0.42]);
    let (world, scratch) = (filtered.world, filtered.scratch);
    let q = std::f32::consts::FRAC_PI_4;
    let params = ChainParams { dir: -f, right, ranges: [(-q, 0.0), (0.0, q)], parallel: q, mask: u32::MAX };
    let mut out: Vec<LedgeCandidate> = Vec::new();
    for chain in world.chains(&params) {
        let Some(hit) = world.grab_point_toward(&chain, root, Some(f), Some(f), &[0.5, 0.1], 0.3, &scratch) else { continue };
        let n = Vec3::new(hit.wall_normal.x, 0.0, hit.wall_normal.z).normalize_or_zero();
        if n == Vec3::ZERO {
            continue;
        }
        let (from, to) = (root - f * 0.5, hit.point + n * 0.05);
        let v = to - from;
        let len = v.length();
        if len > 1e-4 && collision.ray_distance(from, v / len, len, crate::layers::CHARACTER) < len - 1e-4 {
            continue;
        }
        let d = hit.point - origin;
        out.push(LedgeCandidate { point: hit.point, normal: n, edge_dir: hit.dir, height: hit.point.y - root.y, dist: Vec2::new(d.x, d.z).length() });
    }
    if highest_first {
        out.sort_by(|a, b| b.point.y.total_cmp(&a.point.y));
    } else {
        out.sort_by(|a, b| a.point.y.total_cmp(&b.point.y));
    }
    out
}

/// Probe D's second blend axis (WALL-8): the wall-hang foot support under the hands (the `sub_B16130` box 1.0 m below,
/// 0.125 m into the wall) carried onto the hands' level; its flat offset d from the hands' midpoint gives o = min(d, 0.75)
/// / 0.75 when d ≈ 0 or it points back toward the character, else 0.
fn foot_offset(mid: Vec3, n: Vec3, f: Vec3, collision: &CollisionWorld) -> f32 {
    let c = mid - Vec3::Y + f * 0.125;
    let from = c - f * 0.625;
    let d = collision.sphere_free_distance(from, f, 0.01, 1.25);
    if d >= 1.25 - 1e-3 {
        return 0.0;
    }
    let foot = from + f * d;
    let off = Vec3::new(foot.x - mid.x, 0.0, foot.z - mid.z);
    let dist = off.length();
    let _ = n;
    if dist < 1e-4 || off.dot(f) < 0.0 { dist.min(0.75) / 0.75 } else { 0.0 }
}

/// `HumanWalling__ClassifyLedgeCandidate` 0xE341D0: the room behind the edge. A cast (`sub_E1C820`, 2 m, layer 5)
/// along the wall's inward direction turned 45° up about the edge (`flt_19BA7F0` = −45°), starting at the edge +
/// 0.05·up + 0.05·n − 0.5·dir (out from the wall and below the edge, so it clears the corner). The flat distance from
/// the edge to the hit (2 m when nothing is hit, or the hit is material class 3 with sub-material 8 / 9): < 0.03 → 0
/// (blocked), < 0.4 → 2 (a hang only: no room to stand), else 1. The game never returns 3, so the pass-over exits
/// (`…_tr_passover`, class 3 with WallingData +0x20) are unreachable. PORT: a 1 cm sphere sweep for the cast.
fn classify(c: &LedgeCandidate, collision: &CollisionWorld) -> u8 {
    let n = Vec3::new(c.normal.x, 0.0, c.normal.z).normalize_or_zero();
    let dir = (Vec3::Y - n) * std::f32::consts::FRAC_1_SQRT_2;
    let start = c.point + Vec3::Y * 0.05 + n * 0.05 - dir * 0.5;
    let d = collision.sphere_free_distance(start, dir, 0.01, 2.0);
    let flat = if d >= 2.0 {
        2.0
    } else {
        let hit = start + dir * d;
        Vec2::new(hit.x - c.point.x, hit.z - c.point.z).length()
    };
    if flat < 0.03 {
        0
    } else if flat < 0.4 {
        2
    } else {
        1
    }
}

/// The two hands 0.2 m either side of the grab point along the edge, the left hand on the climber's left. Edges are
/// authored in either direction, so the edge direction is first turned to the climber's right.
fn hands(c: &LedgeCandidate) -> (Vec3, Vec3) {
    let right = right_of(-Vec3::new(c.normal.x, 0.0, c.normal.z).normalize_or_zero());
    let along = if c.edge_dir == Vec3::ZERO { right } else if c.edge_dir.dot(right) < 0.0 { -c.edge_dir } else { c.edge_dir };
    (c.point - along * 0.2, c.point + along * 0.2)
}

fn exit(seq: [Option<ActionBlend>; 4], from: Vec3, to: Vec3, c: &LedgeCandidate, end: (bool, bool, bool)) -> LedgeEntry {
    exit_with_hands(seq, from, to, c, end, hands(c))
}

/// `exit` with the hand targets given (probe D's wall hang: the hand pair it found).
fn exit_with_hands(seq: [Option<ActionBlend>; 4], from: Vec3, to: Vec3, c: &LedgeCandidate, end: (bool, bool, bool), (hl, hr): (Vec3, Vec3)) -> LedgeEntry {
    let facing = -Vec3::new(c.normal.x, 0.0, c.normal.z).normalize_or_zero();
    let durations = seq.map(|a| a.map(|a| a.duration()).unwrap_or(0.0));
    let mv = LedgeMove {
        kind: MoveKind::Arrival { wallfree_wait: false },
        seq,
        durations,
        t: 0.0,
        from,
        to,
        facing_from: facing,
        facing_to: facing,
        follow_disp: durations.iter().sum::<f32>() > 0.0,
        lead: 0.0,
        table: None,
        root_time: None,
        end_free: end.0,
        end_wall: end.1,
        end_stand: end.2,
        hand_l: hl,
        hand_r: hr,
        normal: c.normal,
    };
    if std::env::var_os("AC_ANIM_LOG").is_some() {
        let ne = mv.natural_end();
        info!("walling exit: from {from:.2} to {to:.2} natural end {ne:.2} correction {:.2} over {:.2}s (edge {:.2}, h {:.2})", (to - ne).length(), durations.iter().sum::<f32>(), c.point, c.height);
    }
    let mut e = LedgeEntry::at(c.point, c.normal, from, LedgeSubState::HangWallReception);
    e.hand_l = hl;
    e.hand_r = hr;
    e.entry_move = Some(mv);
    e
}

/// Pull-up from the wall run (A 0xE37B25 / D 0xE38EC8): `…_tr_hangknee_{low,high}` blended by `b` while the root follows
/// the clip, corrected onto the edge + 0.1 m into the wall at the edge's height, over that clip only (`sub_711130` in anim
/// mode, 0xE37BBD / 0xE38DD6, RE/24 WALL-22); the Ledge then enters Pullup (LedgeData +76 = 4, WALL-21) and stands up:
/// hangknee → free-step entry (0xDE2EE0) onto the top. The entry ends on footl, step1 on footr.
fn pullup_exit(id: u32, b: f32, root: Vec3, c: &LedgeCandidate) -> LedgeEntry {
    let into = -Vec3::new(c.normal.x, 0.0, c.normal.z).normalize_or_zero();
    let knee = c.point + into * 0.1;
    let mut e = exit([blended(id, 0, &[1.0 - b, b]), None, None, None], root, knee, c, (false, false, false));
    e.sub_state = LedgeSubState::Pullup;
    if let Some(mv) = e.entry_move {
        let stand_id = ledge_moves::ACT_KNEE_TO_FREESTEP[(id == STEP1_TO_KNEE) as usize];
        let seq = [single(stand_id, 0), None, None, None];
        let d = seq.map(|a| a.map(|a| a.duration()).unwrap_or(0.0));
        let stand = LedgeMove {
            kind: MoveKind::Pullup,
            seq,
            durations: if d[0] > 0.0 { d } else { [0.4, 0.0, 0.0, 0.0] },
            t: 0.0,
            from: knee,
            // no root motion in `hangknee_tr_freestep_entry` (see `ledge_moves::climb_out_moves`)
            to: knee,
            follow_disp: false,
            end_stand: true,
            ..mv
        };
        e.entry_rest = [Some(stand), None];
    }
    e
}

/// Probes after EntryB (UpdateWallingSubState 0xE37590, command 0): A = pull-up, B = free hang.
fn probe_entry(g: &GuidanceWorld, root: Vec3, facing: Vec3, collision: &CollisionWorld) -> Option<LedgeEntry> {
    // A: FindLedge(pos, 0.3h, 1.05h+0.05, h, 0.8): 0.25h ≤ height ≤ h, room on top
    for c in find_ledge(g, root, root, facing, 0.3 * H, 1.05 * H + 0.05, H, 0.8, true, collision) {
        if (0.25 * H..=H).contains(&c.height) && classify(&c, collision) == 1 {
            let b = ((c.height.max(0.3) - 0.3 * H) / (0.7 * H)).clamp(0.0, 1.0);
            return Some(pullup_exit(ENTRY_TO_KNEE, b, root, &c));
        }
    }
    // B: FindLedge(pos + 1.5·up − 0.6·fwd, 0.3h, h+0.05, 0.25h+0.1, 1.2): an edge overhanging the run, 0.25–0.6 m
    // behind the root (the box reaches 0.35 m forward from 0.6 m behind), not the wall's own top. Free hang, 4-way
    // blend (0xE37A07 … 0xE37F06).
    let o = root + Vec3::Y * 1.5 - facing * 0.6;
    if let Some(c) = find_ledge(g, o, root, facing, 0.3 * H, H + 0.05, 0.25 * H + 0.1, 1.2, true, collision).into_iter().find(|c| classify(c, collision) != 0) {
        let hb = (c.height - 1.5).clamp(0.0, 1.0);
        // the "max" weight: 1 − (flat distance from the probe origin − 0.1) / 0.7
        let near = (1.0 - (c.dist - 0.1) / 0.7).clamp(0.0, 1.0);
        // clips: 250 min, 350 min, 250 max, 350 max (swing back)
        let w = [(1.0 - hb) * (1.0 - near), hb * (1.0 - near), (1.0 - hb) * near, hb * near];
        let (hl, hr) = hands(&c);
        // the root goes straight under the edge, 2.4 m down (0xE37F70); hang type Free (0xE3807F). LedgeData +76 = 0, +108 = 3
        // (HangFreeReceptionType), +112 = w₂ + w₃ (0xE3807F–0xE380D7): the free idle after it blends by them
        // (`HumanLedgeData::swing_strength`, RE/24 WALL-21 / LEDGE-24)
        let to = hang_root(hl, hr, c.normal, LedgeHangType::Free);
        return Some(exit([blended(ENTRY_TO_HANGFREE, 0, &w), None, None, None], root, to, &c, (true, false, false)));
    }
    None
}

/// Probes after the Vertical step: C = free hang (2h ≤ height < 2.7h+0.1), D = pull-up (0.5h–h) or wall hang
/// (h–2.7h+0.1).
fn probe_vertical(g: &GuidanceWorld, root: Vec3, facing: Vec3, collision: &CollisionWorld) -> Option<LedgeEntry> {
    // C: FindLedge(pos − 0.6·fwd + up, 0.3h, 1.7h+0.1, 0.8h, 1.2), within 0.25h+0.1 flat of the probe origin
    // (0xE38586): again an overhanging edge, not the wall's top.
    let oc = root - facing * 0.6 + Vec3::Y;
    for c in find_ledge(g, oc, root, facing, 0.3 * H, 1.7 * H + 0.1, 0.8 * H, 1.2, false, collision) {
        if c.dist < 0.25 * H + 0.1 && (2.0 * H..2.7 * H + 0.1).contains(&c.height) && classify(&c, collision) != 0 {
            // weights (0xE3870C … 0xE38855): height past 2 m × nearness ((0.7 − dist) / 0.7)
            let hb = (c.height - 2.0).clamp(0.0, 1.0);
            let near = ((0.8 - c.dist - 0.1) / 0.7).clamp(0.0, 1.0);
            let w = [(1.0 - hb) * (1.0 - near), hb * (1.0 - near), (1.0 - hb) * near, hb * near];
            let (hl, hr) = hands(&c);
            let to = hang_root(hl, hr, c.normal, LedgeHangType::Free);
            return Some(exit([blended(STEP1_TO_HANGFREE, 0, &w), None, None, None], root, to, &c, (true, false, false)));
        }
    }
    // D: FindLedge(pos − 0.6·fwd, 0.8h, 2.7h+0.1, 1.5h, 1.2) (0xE3863F)
    let od = root - facing * 0.6;
    for c in find_ledge(g, od, root, facing, 0.8 * H, 2.7 * H + 0.1, 1.5 * H, 1.2, true, collision) {
        let class = classify(&c, collision);
        if class == 0 {
            continue;
        }
        if (0.5 * H..H).contains(&c.height) && class == 1 {
            return Some(pullup_exit(STEP1_TO_KNEE, (c.height - 0.5 * H) / (0.5 * H), root, &c));
        }
        if (H..2.7 * H + 0.1).contains(&c.height) {
            // the wall hang (0xE38C4A, RE/24 WALL-9): a hand pair around the candidate (`Guidance__ProbeBoxHandPair`: box at
            // the candidate, axes flat forward / up, half 1.0 / 0.3 / 0.3, refs (∓0.3, −0.15, 0) local, 70°, step 0.4,
            // mask 306) and feet supports at the hands' midpoint (`Human__FindWallHangFeetSupports` 0xB16130); either
            // fails → the next candidate
            let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
            let r = f.cross(Vec3::Y);
            let local = |x: f32| c.point + r * x - f * 0.15;
            let Some([a, b]) = g.probe_facing_pair(c.point, f, Vec3::new(1.0, 0.3, 0.3), [local(-0.3), local(0.3)], 1.221_730_4, 0.4, crate::guidance::MASK_CATCH) else { continue };
            let (hl, hr) = (a.point, b.point);
            let mid = (hl + hr) * 0.5;
            let n = Vec3::new(a.wall_normal.x, 0.0, a.wall_normal.z).normalize_or(c.normal);
            if !super::ledge::feet_supports(mid, n, collision) {
                continue;
            }
            // clips 251 / 430 cm × 000 / 050 cm: v = min((height − h) / 1.5h, 1) (0xE39069, WALL-7); o = the foot support's
            // offset from the hands on the hands' level, min(d, 0.75) / 0.75 when it points back toward the character, else
            // 0 (0xE38F9D…0xE3903B, WALL-8). PORT: LedgeData+112 = o is not carried into the hang.
            let v = ((c.height - H) / (1.5 * H)).clamp(0.0, 1.0);
            let o = foot_offset(mid, n, f, collision);
            let w = [(1.0 - v) * (1.0 - o), v * (1.0 - o), (1.0 - v) * o, v * o];
            let to = hang_root(hl, hr, n, LedgeHangType::Wall);
            let c = LedgeCandidate { normal: n, ..c };
            // the hands are the pair it found (0xE391FB / 0xE3920C: `sub_E55CB0`(0 / 1) with its two points): on short
            // pieces both on the piece's middle, so the hang starts with its hands together (live RE/25 climb survey
            // wall 08: 0.09 m apart, and the first side move goes onto the climb holds, `+133` clear); the port's ±0.2 m
            // spread made every such hang start with a closing step
            return Some(exit_with_hands([blended(STEP1_TO_HANGWALL, 0, &w), blended(STEP1_TO_HANGWALL, 1, &w), None, None], root, to, &c, (false, true, false), (hl, hr)));
        }
    }
    None
}

// ---------------------------------------------------------------- update (UpdateWallingSubState 0xE37590)

pub fn update_walling(
    time: Res<Time>,
    mut pad: ResMut<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    camera: Option<ResMut<crate::camera::CameraEvents>>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>,
) {
    let mut camera = camera;
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data) in &mut q {
        if loco.current != ActorContextId::Walling {
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            if loco.switched_this_frame {
                continue;
            }
        }
        let w = &mut data.walling;
        w.t = (w.t + dt).min(w.duration());
        // `RootInterp__Advance` 0x7113F0 with a4 = 1 (0xDA2D92): the root is a move-to request to the character controller
        // (`CharacterController__RequestMoveTo` 0x710B40: velocity = (target − position) / dt, RE/01 §7.2), integrated
        // with collision, so a lip the capsule meets holds the root back (live RE/26 §6: wall 00's entry stops 0.09 m out
        // and 0.05 m low under a hold at 2.7 m)
        let planned = w.root_at(w.t);
        let feet = body.feet;
        body.feet = collision.move_capsule(&mut body.proxy, feet, planned - feet, false, dt).position;
        body.heading = super::heading_of(w.facing());
        // the root frame's pitch (RE/24 WALL-3): the entry warp slerps the frame from the start frame (upright) onto the
        // pitched target frame with the warp's progress (`RootInterp__Advance` 0x7113F0, `Quat__Slerp`); EntryB and
        // Vertical keep it; VerticalEnd turns it back upright by 4·dt a frame (0xE39295)
        body.tilt = match w.sub_state {
            WallingSubState::EntryA => {
                let warp = w.warp_duration.unwrap_or(w.duration()).max(1e-4);
                Quat::IDENTITY.slerp(w.pitch, (w.t / warp).min(1.0))
            }
            WallingSubState::EntryB | WallingSubState::Vertical => w.pitch,
            WallingSubState::VerticalEnd => {
                w.pitch = pitch_toward_upright(w.pitch, dt);
                w.pitch
            }
        };
        body.velocity = Vec3::ZERO;
        body.grounded = false;
        let facing = w.facing();
        let normal = w.normal;
        let sub = w.sub_state;
        let done = w.t >= w.duration() - 1e-4;

        // The interpreter's walling state (0xEE05E0) sets the input direction (+0x20, `SetInputDirection` 0xE36250)
        // only while Legs was pressed within 0.3 s (interp +0x1126): straight back from the wall when the stick points
        // within 50° of the facing, else along the stick; zero otherwise. It always sends command 0 (+0x38,
        // `SetActionCommand` 0xE337D0): the player's wall run never falls off by command. The angle is +0x1244, written
        // only while the stick is past 0.35, so with the stick released the last angle decides: within 50° straight back,
        // else the (near-)zero stick, which starts no rebound (0xE3930A / 0xE376EB: |dir| ≤ 0.0005) (RE/24 WALL-13).
        let push = pad.jump_buffered().then(|| {
            if pad.stick_facing_angle.abs() < PUSH_STRAIGHT_BACK {
                normal
            } else {
                Vec3::new(pad.dir.x, 0.0, pad.dir.z) * pad.magnitude.min(1.0)
            }
        }).filter(|d| d.length() > 0.0005);
        // A ladder beside the wall (the interpreter's first test, 0xEE05E0 → IHumanWalling slots 7 / 8): taken at the end of a
        // Vertical step with the stick toward it; it comes before the push-off.
        if sub == WallingSubState::Vertical && done && pad.speed01 > 0.0 {
            if let Some(e) = super::ladder::find_wall_run_ladder(body.feet, pad.dir, &guidance) {
                switch_context(&mut loco, &mut data, TransitionSetup::ToLadder(e));
                continue;
            }
        }
        // Rebound (0xE37590): at the end of EntryB, at any time in Vertical and in VerticalEnd (command ≠ 4)
        let rebound = match sub {
            WallingSubState::EntryA => false,
            WallingSubState::EntryB => done,
            WallingSubState::Vertical | WallingSubState::VerticalEnd => true,
        };
        if let Some(push) = push.filter(|_| rebound) {
            pad.consume_jump();
            let from = body.feet;
            // the foot: the playing item's word bits 2–3 (0xE36660, 1 = left ahead; RE/24 WALL-11)
            let foot_left = w.current().and_then(|(a, _)| super::anim_gate::word(&a)).is_none_or(|word| (word >> 2) & 3 != 2);
            let (dir, target) = rebound_jump(from, normal, push, &guidance, &collision);
            if target.is_none() {
                if let Some(c) = camera.as_mut() {
                    // ActorState 67 (CameraReset) only with the fallback target (0xE368D9, RE/24 WALL-12)
                    c.reset = true;
                }
            }
            // turned away from the wall (`sub_52BD40` 0xE366BD), then jump kind 2: the rebound takeoff's side groups
            // turn the character toward the target (RE/24 WALL-10 / AIR-5)
            // the frame snaps upright first (`sub_52BD40`(forward, world up) at 0xE37717 / 0xE382EB / 0xE39322; RE/24 WALL-3)
            body.tilt = Quat::IDENTITY;
            body.heading = super::heading_of(normal);
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::ReboundJump { from, target, dir, foot_left }));
            continue;
        }
        if !done {
            continue;
        }
        let feet = body.feet;
        let fall = InAirEntry::Fall { from: feet, velocity: Vec3::ZERO, origin: FallOrigin::Ground, speed_param: 0.0 };
        match sub {
            WallingSubState::EntryA => {
                let a = single(ENTRY_B, 0);
                data.walling.play(WallingSubState::EntryB, a, None, feet, None);
            }
            WallingSubState::EntryB => {
                // (command ≠ 0 would play entry_footl_tr_fall 0xFF7CA1 and leave; only AI walls send it)
                if let Some(e) = probe_entry(&guidance, feet, facing, &collision) {
                    // the hangs enter the Ledge's Entry state (+76 = 0; the pull-ups 4): its end re-poses the hands (LEDGE-22)
                    let hang = e.sub_state != LedgeSubState::Pullup;
                    // PORT: upright at once; the game's exits interpolate the frame to world up over their warp
                    // (`sub_711130` with up (0, 0, 1), 0xE37C41 / 0xE38057 / 0xE38964 / 0xE38EA0 / 0xE391D9; RE/24 WALL-3)
                    body.tilt = Quat::IDENTITY;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(e));
                    data.ledge.repose_at_entry_end = hang;
                    continue;
                }
                let a = single(STEP1, 0);
                data.walling.play(WallingSubState::Vertical, a, None, feet, None);
            }
            WallingSubState::Vertical => {
                if let Some(e) = probe_vertical(&guidance, feet, facing, &collision) {
                    let hang = e.sub_state != LedgeSubState::Pullup;
                    // PORT: upright at once; the game's exits interpolate the frame to world up over their warp
                    // (`sub_711130` with up (0, 0, 1), 0xE37C41 / 0xE38057 / 0xE38964 / 0xE38EA0 / 0xE391D9; RE/24 WALL-3)
                    body.tilt = Quat::IDENTITY;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(e));
                    data.ledge.repose_at_entry_end = hang;
                    continue;
                }
                let (a, b) = (single(VERTICAL_END, 0), single(VERTICAL_END, 1));
                data.walling.play(WallingSubState::VerticalEnd, a, b, feet, None);
            }
            WallingSubState::VerticalEnd => {
                // the rebound clip done (sub_501760) → `rebound_footr_tr_fall` and leave (flag +0x32): falling back off
                // the wall
                // (the frame snapped upright, 0xE3936F; RE/24 WALL-3)
                body.tilt = Quat::IDENTITY;
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(fall));
                data.air.fall_action = single(REBOUND_TO_FALL, 0);
            }
        }
    }
}

/// `HumanWalling__ReboundJump` 0xE365C0: the character turns away from the wall; the push-off direction is kept within
/// ±89° of the wall normal (beyond ±90° it is turned to ±89°). A jump target along it (IHuman vt56, the ground jumps'
/// guidance search, picked by `sub_E96BF0`), else the fallback pos + 7·dir − 3·up; jump `sub_B27180`(…, 2), flag +0x31,
/// ActorState 67 (camera reset, fallback only). The scorer sees the character turned away from the wall (facing = the
/// normal) and the clamped push as the wanted direction (0xE367CC, RE/24 WALL-11). Returns the direction and the target
/// (`None`: the fallback, `InAirEntry::ReboundJump` aims 7 m along the direction, 3 m down).
fn rebound_jump(from: Vec3, normal: Vec3, push: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> (Vec3, Option<JumpTarget>) {
    let n = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
    let mut dir = Vec3::new(push.x, 0.0, push.z).normalize_or(n);
    if dir.dot(n) <= 0.0 {
        let max = 1.553_343_1;
        let side = if n.cross(dir).y >= 0.0 { 1.0 } else { -1.0 };
        dir = Quat::from_rotation_y(max * side) * n;
    }
    (dir, super::targets::find_target(from, n, dir, &super::jump_candidates::Query::TAP, false, guidance, collision))
}

#[cfg(test)]
mod wall_entry_tests {
    use super::*;
    use crate::collision::Aabb3;
    use crate::triangles::Triangle;

    /// A quad a–b–c–d as two triangles on the walling layer, outward normal (b − a) × (d − a).
    fn quad(c: &mut CollisionWorld, a: Vec3, b: Vec3, cc: Vec3, d: Vec3) {
        for t in [[a, b, cc], [a, cc, d]] {
            c.triangles.push(Triangle::new(t, crate::layers::STATIC).unwrap());
        }
    }

    /// RE/24 WALL-22 / WALL-21: the wall run's pull-up warps over the walling clip only, onto the edge 0.1 m in at the
    /// edge's height, as Ledge Pullup; the stand-up follows as its own move onto the top.
    #[test]
    fn the_wall_run_pull_up_ends_its_clip_at_the_edge() {
        let c = LedgeCandidate { point: Vec3::new(0.0, 2.0, 0.0), normal: Vec3::Z, edge_dir: Vec3::X, height: 2.0, dist: 0.3 };
        let e = pullup_exit(ENTRY_TO_KNEE, 0.5, Vec3::new(0.0, 0.0, 0.6), &c);
        assert_eq!(e.sub_state, LedgeSubState::Pullup);
        let mv = e.entry_move.expect("the walling clip");
        assert!((mv.to - Vec3::new(0.0, 2.0, -0.1)).length() < 1e-5, "0.1 m in at the edge: {}", mv.to);
        assert!(mv.seq[1].is_none() && !mv.end_stand, "the walling clip alone");
        let stand = e.entry_rest[0].expect("the stand-up");
        assert!(stand.end_stand && stand.from == mv.to);
    }

    /// RE/24 WALL-21 / LEDGE-24: walling's free hang B stores +112 = w₂ + w₃ of its 4-way blend; the free idle after it
    /// blends by it until the next move.
    #[test]
    fn a_walling_free_hang_hands_its_swing_strength_to_the_idle() {
        let c = LedgeCandidate { point: Vec3::new(0.0, 3.0, 0.0), normal: Vec3::Z, edge_dir: Vec3::X, height: 3.0, dist: 0.3 };
        let w = [0.1, 0.2, 0.3, 0.4];
        let e = exit([blended(ENTRY_TO_HANGFREE, 0, &w), None, None, None], Vec3::new(0.0, 0.0, 0.6), Vec3::new(0.0, 0.6, 0.3), &c, (true, false, false));
        let a = e.entry_move.and_then(|m| m.seq[0]).expect("the 4-way blend");
        let mut l = crate::player::ledge::HumanLedgeData::default();
        l.enter(e);
        assert_eq!(l.swing_idle(), Some(a.w[2] + a.w[3]));
        l.step_seq += 1;
        assert_eq!(l.swing_idle(), None, "a new move ends it");
    }

    /// RE/24 WALL-18: the wall ray (filter 47) sees fuzzy zones, and a fuzzy zone in front of a wall is no wall
    /// (`sub_E14410`); a static body there is.
    #[test]
    fn a_fuzzy_zone_is_no_wall_to_run_up() {
        let feet = Vec3::new(0.0, 0.0, 1.0);
        let wall = |front: u8| {
            let mut c = CollisionWorld::default();
            c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 0.0, -0.3), max: Vec3::new(2.0, 4.0, 0.3) });
            c.layers.push(front);
            c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 0.0, -2.0), max: Vec3::new(2.0, 4.0, -0.6) });
            c.layers.push(crate::layers::STATIC);
            wall_ahead(feet, Vec3::NEG_Z, &c).is_some()
        };
        assert!(wall(crate::layers::STATIC));
        assert!(!wall(crate::layers::FUZZY_ZONE), "the fuzzy zone in front hides the wall behind it");
    }

    /// RE/24 WALL-1: the width check never needs the wall to reach 0.5 m to both sides: a 0.3 m post and the last 0.2 m
    /// of a wall run up; a pilaster jutting out beside the contact moves it past the pilaster.
    #[test]
    fn narrow_walls_and_wall_ends_are_run_up_and_a_pilaster_is_stepped_past() {
        let feet = Vec3::new(0.0, 0.0, 1.0);
        let mut post = CollisionWorld::default();
        post.boxes.push(Aabb3 { min: Vec3::new(-0.15, 0.0, -0.3), max: Vec3::new(0.15, 4.0, 0.0) });
        let (contact, n, _) = wall_ahead(feet, Vec3::NEG_Z, &post).expect("a 0.3 m post runs up");
        assert!(n.dot(Vec3::Z) > 0.999 && (contact - Vec3::new(0.0, 1.0, 0.5)).length() < 1e-3, "{contact:?} {n:?}");
        let mut end = CollisionWorld::default();
        end.boxes.push(Aabb3 { min: Vec3::new(-0.2, 0.0, -1.0), max: Vec3::new(5.0, 4.0, 0.0) });
        assert!(wall_ahead(feet, Vec3::NEG_Z, &end).is_some(), "the last 0.2 m of a wall runs up");
        // a pilaster 0.15 m to the right (x 0.15..0.35), jutting 0.5 m out: the right 45° ray meets its side 0.15 m out,
        // the contact steps 0.3 m away from it along the wall and back onto the wall within 0.25 m (0xE14790)
        let mut pil = CollisionWorld::default();
        pil.boxes.push(Aabb3 { min: Vec3::new(-5.0, 0.0, -1.0), max: Vec3::new(5.0, 4.0, 0.0) });
        pil.boxes.push(Aabb3 { min: Vec3::new(0.15, 0.0, 0.0), max: Vec3::new(0.35, 4.0, 0.5) });
        let (contact, _, _) = wall_ahead(feet, Vec3::NEG_Z, &pil).expect("stepped away from the pilaster");
        assert!(contact.x < -0.01 && (contact.z - 0.5).abs() < 1e-3, "the contact moved away from the pilaster: {contact:?}");
        // 0.3 m to the right the 45° ray meets the side 0.3 m out: the back ray (0.25 m from the jut's hit) misses the
        // wall and the wall is refused
        let mut far = CollisionWorld::default();
        far.boxes.push(Aabb3 { min: Vec3::new(-5.0, 0.0, -1.0), max: Vec3::new(5.0, 4.0, 0.0) });
        far.boxes.push(Aabb3 { min: Vec3::new(0.3, 0.0, 0.0), max: Vec3::new(0.5, 4.0, 0.5) });
        assert!(wall_ahead(feet, Vec3::NEG_Z, &far).is_none(), "a jut 0.3 m out beside the contact refuses the wall");
    }

    /// RE/24 WALL-2: the hit's own normal, not the nearest axis: a wall at 45° keeps its 45° normal.
    #[test]
    fn an_oblique_wall_keeps_its_own_normal() {
        let mut c = CollisionWorld::default();
        let (u, v) = (Vec3::new(1.0, 0.0, -1.0).normalize(), Vec3::Y);
        let o = Vec3::new(0.0, 0.0, 0.0);
        quad(&mut c, o - u * 3.0, o + u * 3.0, o + u * 3.0 + v * 4.0, o - u * 3.0 + v * 4.0);
        let out = Vec3::new(-1.0, 0.0, -1.0).normalize();
        let n_quad = c.triangles[0].normal;
        let (a, b, cc, d) = if n_quad.dot(out) > 0.0 { (o - u * 3.0, o + u * 3.0, o + u * 3.0 + v * 4.0, o - u * 3.0 + v * 4.0) } else { (o + u * 3.0, o - u * 3.0, o - u * 3.0 + v * 4.0, o + u * 3.0 + v * 4.0) };
        c.triangles.clear();
        quad(&mut c, a, b, cc, d);
        let feet = out * 0.8;
        let (_, n, _) = wall_ahead(feet, -out, &c).expect("an oblique wall");
        assert!(n.dot(out) > 0.999, "not snapped to an axis: {n:?}");
    }

    /// RE/24 WALL-6 / WALL-9: probe D tries the highest candidate first; a slab jutting out over open space (its edge at
    /// 3.4 m, 0.6 m out from the wall: beyond probe C's 0.35 m, whose free hang would take it) has no feet supports under
    /// it, so the next one down, the wall's own top at 2.0 m, is hung from instead.
    #[test]
    fn probe_d_skips_a_lip_without_feet_for_the_next_band_down() {
        use crate::guidance::{GuidanceEdge, GuidanceSubType};
        let edge = |y: f32, z: f32| GuidanceEdge { p0: Vec3::new(-3.0, y, z), p1: Vec3::new(3.0, y, z), n0: Vec3::Y, n1: Vec3::NEG_Z, subtype: GuidanceSubType::LedgeGrab };
        let mut c = CollisionWorld::default();
        c.boxes.push(Aabb3 { min: Vec3::new(-3.0, 0.0, 0.0), max: Vec3::new(3.0, 2.0, 1.0) });
        c.boxes.push(Aabb3 { min: Vec3::new(-3.0, 3.2, -0.6), max: Vec3::new(3.0, 3.4, 0.0) });
        let mut g = GuidanceWorld::default();
        g.edges.push(edge(2.0, 0.0));
        g.edges.push(edge(3.4, -0.6));
        let root = Vec3::new(0.0, 1.0, -0.5);
        let e = probe_vertical(&g, root, Vec3::Z, &c).expect("a wall hang");
        assert!((e.hand_l.y - 2.0).abs() < 0.05 && (e.hand_r.y - 2.0).abs() < 0.05, "the wall top, not the lip: {:?} {:?}", e.hand_l, e.hand_r);
    }

    /// RE/24 WALL-3: a wall leaning back 15° moves the contact along its tilted normal; one leaning 50° is refused.
    #[test]
    fn the_wall_slope_moves_the_contact_and_too_steep_a_lean_refuses() {
        let lean = |deg: f32| {
            let mut c = CollisionWorld::default();
            let t = deg.to_radians();
            // the face rises from z 0 at the floor, leaning back toward −z
            let top = Vec3::new(0.0, 4.0 * t.cos(), -4.0 * t.sin());
            let (a, b) = (Vec3::new(-3.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.0));
            quad(&mut c, a, b, b + top, a + top);
            if c.triangles[0].normal.z < 0.0 {
                c.triangles.clear();
                quad(&mut c, b, a, a + top, b + top);
            }
            c
        };
        let feet = Vec3::new(0.0, 0.0, 0.8);
        let (vert, _, _) = wall_ahead(feet, Vec3::NEG_Z, &lean(0.0)).expect("vertical");
        let (leaning, _, slope) = wall_ahead(feet, Vec3::NEG_Z, &lean(15.0)).expect("15 degrees back");
        assert!(leaning.y > vert.y + 0.05, "the contact follows the tilted normal: {leaning:?} vs {vert:?}");
        assert!(wall_ahead(feet, Vec3::NEG_Z, &lean(50.0)).is_none(), "a 50-degree lean is refused");
        // the run is pitched with the wall: the frame's forward points along −slope, 15° down into the wall, its up
        // leaning back with the face
        let p = wall_pitch(Vec3::Z, slope);
        let (fwd, up) = (p * Vec3::NEG_Z, p * Vec3::Y);
        assert!((fwd.y + 15f32.to_radians().sin()).abs() < 0.02, "forward pitched 15° down: {fwd}");
        assert!(up.z < -0.2 && (up.z + 15f32.to_radians().sin()).abs() < 0.02, "up leans back with the wall: {up}");
        assert_eq!(wall_pitch(Vec3::Z, Vec3::Z), Quat::IDENTITY, "a vertical wall leaves the run upright");
    }

    /// RE/24 WALL-3: VerticalEnd turns the frame's up toward world up by 4·dt a frame (0xE39295): 1 − 4·dt of the
    /// lean is left after a 60 Hz frame (to first order), about a third after 0.25 s, nearly none after a second.
    #[test]
    fn vertical_end_turns_the_pitch_back_upright() {
        let lean = |q: Quat| (q * Vec3::Y).angle_between(Vec3::Y);
        let mut p = Quat::from_rotation_x(-15f32.to_radians());
        let a0 = lean(p);
        p = pitch_toward_upright(p, 1.0 / 60.0);
        assert!((lean(p) / a0 - (1.0 - 4.0 / 60.0)).abs() < 0.01, "one frame: {} of {a0}", lean(p));
        for _ in 1..15 {
            p = pitch_toward_upright(p, 1.0 / 60.0);
        }
        assert!((0.3..0.4).contains(&(lean(p) / a0)), "after 0.25 s: {}", lean(p) / a0);
        for _ in 15..60 {
            p = pitch_toward_upright(p, 1.0 / 60.0);
        }
        assert!(lean(p) < a0 * 0.03, "after a second: {}", lean(p));
    }
}
