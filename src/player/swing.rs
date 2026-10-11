//! Swinging on a bar (RE/03 §7.10): a free hang reached by a running jump (target type 0x80, no foot holds)
//! enters Ledge SubState 8 SwingReception.

/// The landing on the bar (InAir block, 9 clips `xx_h_air_{down 050/300/550/900, front 050/300/650, up 300 footl,
/// up 050}cm_to_swing_tr_swing_front_a`), the transition into the swing (0xE07D00 → sub_B0EF30).
pub const SWING_LANDING: u32 = 0x0292_6510;
/// The swing cycle (HumanLedge block): items 0 front up, 1 front down, 2 back up, 3 back down.
pub const SWING_CYCLE: u32 = 0x023E_0C60;
/// Stops: [front, back], items a / b / c / d.
pub const SWING_STOP: [u32; 2] = [0x023E_0C61, 0x023E_0C62];
/// The landing's settle when no jump follows (`sub_DCFBC0` picks by the space ahead of the hands): legs blocked
/// → `hangwallfree_impact_00cm` / `hangfree_impact_{50,150}cm` (+ `…_tr_hangfree`); legs free, shoulders
/// blocked → `hangfree_impact_00cm_shoulder`; all free → `hangfree_impact_00cm_elbow` (+ `…_tr_hangfree`).
pub const IMPACT_LEGS: u32 = 0x02F4_BF02;
pub const IMPACT_SHOULDER: u32 = 0x1480_9A03;
pub const IMPACT_ELBOW: u32 = 0x8AAE_420E;
/// The swing takeoff (jump kind 3, 0xB1EC40): 8 clips `xx_h_swing_cycle_{front 050/300/550, down 050/300/550,
/// up 100/300}cm_to_air`.
pub const TAKEOFF_SWING: u32 = 0x0292_655B;

pub const DUMPED_ACTIONS: &[u32] = &[SWING_LANDING, SWING_CYCLE, SWING_STOP[0], SWING_STOP[1], IMPACT_LEGS, IMPACT_SHOULDER, IMPACT_ELBOW, TAKEOFF_SWING];

use bevy::prelude::*;

use super::jump_blend::{self, ActionBlend};
use super::targets::JumpTarget;
use crate::collision::CollisionWorld;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwingPhase {
    /// The landing on the bar (root interpolated onto the free-hang root over it).
    Landing,
    /// Swing cycle item 0..3.
    Cycle(usize),
    /// A stop or the landing's impact settle: (action, item); then the free hang.
    Settle(u32, usize),
}

#[derive(Clone, Copy, Debug)]
pub struct Swing {
    pub phase: SwingPhase,
    pub action: ActionBlend,
    pub t: f32,
    from: Vec3,
    /// The free-hang root.
    pub root: Vec3,
    /// Facing (into the bar's far side: the swing / jump direction).
    pub fwd: Vec3,
    pub hands_mid: Vec3,
    /// A jump was requested (PORT: kept until the release point, the end of back_down).
    pub jump_req: bool,
    pub target: Option<JumpTarget>,
    pub seq: u32,
    impact_w: f32,
    /// Both hands hold Pole edges (contact kind 3, +1792 / +1796; RE/24 LEDGE-17): only then does the landing go on into
    /// the cycle and can the swing be started or jumped from.
    pub on_pole: bool,
}

impl Swing {
    pub fn current(&self) -> (ActionBlend, f32) {
        (self.action, (self.t / self.action.duration().max(1e-4)).min(1.0))
    }
}

/// 0xE07D00 (type 0x80, no foot holds): the landing `0x02926510`, weighted like the flight it ends (PORT: the
/// flight's front / down / up x distance weights are matched by clip name), root to the free-hang root over it.
pub fn enter(from: Vec3, root: Vec3, fwd: Vec3, hands_mid: Vec3, flight: Option<ActionBlend>, seq: u32) -> Option<Swing> {
    let items = jump_blend::action_items(SWING_LANDING)?;
    let clips = items.first()?;
    let mut w = vec![0.0f32; clips.len()];
    if let Some(f) = flight {
        for (name, fw) in f.clips().iter().zip(f.weights()) {
            // "xx_h_air_front_300cm_footl_to_swing" -> "front_300cm"
            let key: String = name.trim_start_matches("xx_h_air_").split("_foot").next().unwrap_or("").into();
            if let Some(k) = clips.iter().position(|c| c.contains(&format!("air_{key}"))) {
                w[k] += *fw;
            }
        }
    }
    if w.iter().sum::<f32>() <= 0.0 {
        if let Some(k) = clips.iter().position(|c| c.contains("front_300cm")) {
            w[k] = 1.0;
        }
    }
    let action = ActionBlend::new(SWING_LANDING, 0, &w);
    Some(Swing { phase: SwingPhase::Landing, action, t: 0.0, from, root, fwd, hands_mid, jump_req: false, target: None, seq: seq.wrapping_add(1), impact_w: 0.0, on_pole: false })
}

/// Event 5 (`HumanLedge` guard 0xDD3750, action 0xDD2940 → SwingReception, Human+2704 = 3): start swinging from a still
/// free hang on a pole: the cycle from its first item at the hang root.
pub fn start(root: Vec3, fwd: Vec3, hands_mid: Vec3, seq: u32) -> Option<Swing> {
    let action = single(SWING_CYCLE, 0, &[])?;
    Some(Swing { phase: SwingPhase::Cycle(0), action, t: 0.0, from: root, root, fwd, hands_mid, jump_req: false, target: None, seq: seq.wrapping_add(1), impact_w: 0.0, on_pole: true })
}

fn single(id: u32, item: usize, w: &[f32]) -> Option<ActionBlend> {
    let n = jump_blend::action_items(id)?.get(item)?.len();
    let mut v = vec![0.0; n];
    v[..w.len().min(n)].copy_from_slice(&w[..w.len().min(n)]);
    if w.is_empty() {
        v[0] = 1.0;
    }
    Some(ActionBlend::new(id, item, &v))
}

/// `HumanLedge__PickSwingImpact` 0xDCFBC0: base = the hands (LedgeData+144) + 0.5·forward + 0.5·up; two box queries
/// (`sub_B2A070` 0xB2A070: half 0.2 across, 0.5 along the flat forward, 0.35 / 0.6 up) centred at base lowered 0.65 m
/// (shoulders) and 1.6 m (legs), pulled back 0.25 m along LedgeData+192; each returns the nearest geometry's distance from
/// the box's rear face (local y + 0.5), 1000 with nothing in it. The legs blocked (< 1) → the legs impact (weight = that
/// distance; PORT: piecewise over [00, 50, 150 cm]); else the shoulders → the shoulder impact; else the elbow impact
/// (RE/24 NARROW-17). PORT: LedgeData+192 is taken as the swing's facing.
fn impact(s: &Swing, collision: &CollisionWorld) -> (u32, f32) {
    let base = s.hands_mid + s.fwd * 0.5 + Vec3::Y * 0.5;
    let probe = |drop: f32| base - Vec3::Y * drop - s.fwd * 0.25;
    let upper = box_depth(collision, probe(0.65), s.fwd, 0.35);
    let lower = box_depth(collision, probe(1.6), s.fwd, 0.6);
    if lower < 1.0 {
        (IMPACT_LEGS, lower)
    } else if upper < 1.0 {
        (IMPACT_SHOULDER, upper)
    } else {
        (IMPACT_ELBOW, 1.0)
    }
}

/// `sub_B2A070` 0xB2A070's result: the box (half 0.2 across, 0.5 along `dir`, `half_h` up) centred at `p`; the nearest
/// geometry's distance from its rear face, 1000 when the box holds nothing. PORT: found by bisecting the box's length
/// (the game reads the query's contact points).
fn box_depth(collision: &CollisionWorld, p: Vec3, dir: Vec3, half_h: f32) -> f32 {
    let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
    let axes = [super::right_of(dir), dir, Vec3::Y];
    let rear = p - dir * 0.5;
    let blocked = |l: f32| !collision.obb_free(rear + dir * (l * 0.5), axes, Vec3::new(0.2, l * 0.5, half_h));
    if !blocked(1.0) {
        return 1000.0;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..16 {
        let m = (lo + hi) * 0.5;
        if blocked(m) { hi = m } else { lo = m }
    }
    hi
}

pub enum SwingOut {
    Stay,
    /// The free hang (Ledge Movement, `sub_DCE900` / the settle's `_tr_hangfree`).
    Hang,
    /// The swing jump (event 7 -> `sub_DCD0B0`: `Human__SetupJumpToTarget` with jump kind 3). None: the free jump along
    /// the facing (IHuman vt68's free target, 0xEED0FB).
    Jump(Option<JumpTarget>),
}

/// IHumanLedge vt104 `sub_DD38A0`: may the swing jump go free? `Human__BoxQueryNearestFromRear` 0xB2A070 with a box
/// half 0.2 across, 1.0 along the flat facing and 0.75 up, centred 1.0 m ahead of the root and 0.75 m up, must hold
/// nothing (the result 1000).
pub fn free_jump_room(root: Vec3, fwd: Vec3, collision: &CollisionWorld) -> bool {
    let f = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::NEG_Z);
    collision.obb_free(root + f + Vec3::Y * 0.75, [super::right_of(f), f, Vec3::Y], Vec3::new(0.2, 1.0, 0.75))
}

/// `HumanLedge__StateSwingReception_Update` 0xDD24F0 with its events (0xDD2890). The interpreter's ledge state
/// (0xEECCF9–0xEED11F, RE/24 LEDGE-17 / CENSUS-3) drives it with held buttons: `held` = high profile and Legs held
/// (+4397; its negation +4398 sends the stop, event 6), `jump_pressed` = held, Legs pressed and the stick past the dead
/// zone within 75° of the facing (event 7). `find_target` = the jump target along the facing (the guidance search,
/// then the Leap of Faith, `targets::swing_jump_target`); without one the free jump when there is room ahead
/// (`free_jump_room`, 0xEED0D9), else no jump.
pub fn update(s: &mut Swing, dt: f32, held: bool, jump_pressed: bool, feet: &mut Vec3, collision: &CollisionWorld, find_target: impl Fn(Vec3, Vec3) -> Option<JumpTarget>) -> SwingOut {
    s.t += dt;
    if jump_pressed && s.on_pole {
        s.jump_req = true;
    }
    let dur = s.action.duration();
    let done = s.t >= dur;
    match s.phase {
        SwingPhase::Landing => {
            *feet = s.from.lerp(s.root, (s.t / dur.max(1e-3)).min(1.0));
            if !done {
                return SwingOut::Stay;
            }
            // 0xDD24F0: legs blocked → the legs impact for any bar (`SwingLandingToImpactLegs` 0xDCFE90 runs first); on a
            // pole (both hands, `SwingLandingToSettle` 0xDCD300) the landing goes on into the cycle; on a ledge-type free
            // hang it settles (`PlaySwingImpact` 0xDD0140)
            let (impact_id, impact_f) = impact(s, collision);
            if impact_id != IMPACT_LEGS && s.on_pole {
                if let Some(a) = single(SWING_CYCLE, 0, &[]) {
                    s.phase = SwingPhase::Cycle(0);
                    s.action = a;
                }
            } else {
                let (id, f) = (impact_id, impact_f);
                s.impact_w = f;
                let w = if id == IMPACT_LEGS { legs_weights(f) } else { vec![1.0] };
                if let Some(a) = single(id, 0, &w) {
                    s.phase = SwingPhase::Settle(id, 0);
                    s.action = a;
                } else {
                    return SwingOut::Hang;
                }
            }
            s.t = 0.0;
            s.seq = s.seq.wrapping_add(1);
            SwingOut::Stay
        }
        SwingPhase::Cycle(i) => {
            *feet = s.root;
            if !done {
                return SwingOut::Stay;
            }
            // event 6 (+4398: high profile or Legs released; guard `Guard_SwingStop` 0xDCCF40: not on the up items 0 / 2,
            // and only at the item boundary, `sub_502290`): the graph plays the stop of the other side, front_down →
            // `swing_stop_back` 0x023E0C62, back_down → `swing_stop_front` 0x023E0C61, items a → d (RE/24 NARROW-16)
            if !held && (i == 1 || i == 3) {
                let id = SWING_STOP[(i == 1) as usize];
                if let Some(a) = single(id, 0, &[]) {
                    s.phase = SwingPhase::Settle(id, 0);
                    s.action = a;
                    s.t = 0.0;
                    s.seq = s.seq.wrapping_add(1);
                    return SwingOut::Stay;
                }
                return SwingOut::Hang;
            }
            // event 7 (guard 0xDCD000): released at the end of back_down (item 3), toward the target
            if i == 3 && s.jump_req {
                if let Some(t) = s.target.or_else(|| find_target(s.root, s.fwd)) {
                    return SwingOut::Jump(Some(t));
                }
                if free_jump_room(s.root, s.fwd, collision) {
                    return SwingOut::Jump(None);
                }
            }
            let next = (i + 1) % 4;
            if let Some(a) = single(SWING_CYCLE, next, &[]) {
                s.phase = SwingPhase::Cycle(next);
                s.action = a;
            }
            s.t = 0.0;
            s.seq = s.seq.wrapping_add(1);
            SwingOut::Stay
        }
        SwingPhase::Settle(id, item) => {
            // the legs impact requests the controller root hands + 0.5·forward + 0.5·up − 2.4·up (+1296, flag +1360,
            // `HumanLedge__SwingLandingToImpactLegs` 0xDD0077), reached in one frame (`CharacterController__RequestMoveTo`
            // 0x710B40: the velocity (target − position) / dt) (RE/24 NARROW-17); the others stay on the hang root.
            // PORT: its blend parameters (`sub_4FEBF0` 0 / 1 / 2 = 0, 0.5, 0.5) are not mapped onto the clips; the
            // weights come from the legs box's depth (`legs_weights`)
            *feet = if id == IMPACT_LEGS { legs_impact_root(s) } else { s.root };
            if !done {
                return SwingOut::Stay;
            }
            // the stops play their four items a → d, d leading into the free hang
            if SWING_STOP.contains(&id) && item < 3 {
                if let Some(a) = single(id, item + 1, &[]) {
                    s.phase = SwingPhase::Settle(id, item + 1);
                    s.action = a;
                    s.t = 0.0;
                    s.seq = s.seq.wrapping_add(1);
                    return SwingOut::Stay;
                }
            }
            // the impact actions' second item is their transition into the free hang
            if (id == IMPACT_LEGS || id == IMPACT_ELBOW || id == IMPACT_SHOULDER) && item == 0 {
                let w = if id == IMPACT_LEGS { legs_weights(s.impact_w) } else { vec![1.0] };
                if let Some(a) = single(id, 1, &w) {
                    s.phase = SwingPhase::Settle(id, 1);
                    s.action = a;
                    s.t = 0.0;
                    s.seq = s.seq.wrapping_add(1);
                    return SwingOut::Stay;
                }
            }
            SwingOut::Hang
        }
    }
}

/// The legs impact's controller root (0xDCFFC0–0xDD0077): the hands (LedgeData+144) + 0.5·forward + 0.5·up − 2.4·up.
pub fn legs_impact_root(s: &Swing) -> Vec3 {
    let f = Vec3::new(s.fwd.x, 0.0, s.fwd.z).normalize_or(Vec3::NEG_Z);
    s.hands_mid + f * 0.5 - Vec3::Y * 1.9
}

fn legs_weights(f: f32) -> Vec<f32> {
    if f <= 0.5 { vec![1.0 - f * 2.0, f * 2.0, 0.0] } else { vec![0.0, 2.0 - f * 2.0, f * 2.0 - 1.0] }
}

#[cfg(test)]
mod stop_tests {
    use super::*;

    /// RE/24 NARROW-17: `sub_B2A070`'s result is the nearest geometry's distance from the box's rear face (the box 1 m
    /// long along the facing, centred on the probe), 1000 with nothing in it.
    #[test]
    fn the_impact_box_reports_the_depth_from_its_rear() {
        use crate::collision::Aabb3;
        let mut c = CollisionWorld::default();
        assert_eq!(box_depth(&c, Vec3::ZERO, Vec3::NEG_Z, 0.35), 1000.0);
        // a wall from z = −0.4 on: the rear face is at z = +0.5
        c.boxes.push(Aabb3 { min: Vec3::new(-2.0, -2.0, -3.0), max: Vec3::new(2.0, 2.0, -0.4) });
        assert!((box_depth(&c, Vec3::ZERO, Vec3::NEG_Z, 0.35) - 0.9).abs() < 1e-3);
    }

    /// RE/24 LEDGE-17: with no target (guidance or haystack) the swing jump goes free when `sub_DD38A0` finds the 2 m
    /// box ahead of the root empty; a wall in it keeps the swing going.
    #[test]
    fn a_swing_jump_without_a_target_goes_free_only_with_room_ahead() {
        use crate::collision::Aabb3;
        let release = |c: &CollisionWorld| {
            let mut s = start(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y * 2.4, 0).expect("swing cycle");
            s.phase = SwingPhase::Cycle(3);
            s.action = single(SWING_CYCLE, 3, &[]).unwrap();
            s.jump_req = true;
            let mut feet = Vec3::ZERO;
            let d = s.action.duration();
            update(&mut s, d + 1e-3, true, false, &mut feet, c, |_, _| None)
        };
        let mut c = CollisionWorld::default();
        assert!(matches!(release(&c), SwingOut::Jump(None)), "free jump into open space");
        // a wall 1.5 m ahead, from the root up to 3 m
        c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 0.0, -3.0), max: Vec3::new(2.0, 3.0, -1.5) });
        assert!(free_jump_room(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z, &c));
        assert!(!free_jump_room(Vec3::ZERO, Vec3::NEG_Z, &c));
        assert!(matches!(release(&c), SwingOut::Stay), "no room: no jump");
    }

    /// RE/24 NARROW-17: the legs impact holds the root at the controller target (0xDD0077): 0.5 m toward the obstacle and
    /// 1.9 m below the hands, not on the hang root.
    #[test]
    fn the_legs_impact_moves_the_root_toward_the_obstacle() {
        let mut s = start(Vec3::new(0.0, 0.6, 0.0), Vec3::NEG_Z, Vec3::new(0.0, 3.0, 0.0), 0).expect("swing cycle");
        let Some(a) = single(IMPACT_LEGS, 0, &[1.0, 0.0, 0.0]) else { return };
        s.phase = SwingPhase::Settle(IMPACT_LEGS, 0);
        s.action = a;
        let mut feet = s.root;
        let _ = update(&mut s, 1.0 / 60.0, false, false, &mut feet, &CollisionWorld::default(), |_, _| None);
        assert!((feet - Vec3::new(0.0, 1.1, -0.5)).length() < 1e-4, "{feet:?}");
    }

    /// RE/24 NARROW-16: let go on front_down (item 1), the swing finishes the item, then plays the back stop's four
    /// items and hangs.
    #[test]
    fn a_stop_waits_for_the_item_and_plays_the_other_sides_four_items() {
        let mut s = start(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y * 2.4, 0).expect("swing cycle");
        s.phase = SwingPhase::Cycle(1);
        s.action = single(SWING_CYCLE, 1, &[]).unwrap();
        let c = CollisionWorld::default();
        let mut feet = Vec3::ZERO;
        let none = |_: Vec3, _: Vec3| None;
        let dur = s.action.duration();
        assert!(matches!(update(&mut s, dur * 0.5, false, false, &mut feet, &c, none), SwingOut::Stay));
        assert!(matches!(s.phase, SwingPhase::Cycle(1)), "the item plays on");
        let _ = update(&mut s, dur, false, false, &mut feet, &c, none);
        assert!(matches!(s.phase, SwingPhase::Settle(id, 0) if id == SWING_STOP[1]), "front_down leads to the back stop: {:?}", s.phase);
        for k in 1..4 {
            let d = s.action.duration();
            let _ = update(&mut s, d + 1e-3, false, false, &mut feet, &c, none);
            assert!(matches!(s.phase, SwingPhase::Settle(_, i) if i == k), "item {k}: {:?}", s.phase);
        }
        let d = s.action.duration();
        assert!(matches!(update(&mut s, d + 1e-3, false, false, &mut feet, &c, none), SwingOut::Hang));
    }
}
