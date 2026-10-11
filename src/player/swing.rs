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

/// `sub_DCFBC0`: sweeps (0.5 m ahead along the facing) from 0.65 m (r 0.35) and 1.6 m (r 0.6) below the hands,
/// pulled back 0.25 m: the legs blocked -> the legs impact (blend by how far they got, PORT: piecewise over
/// [00, 50, 150 cm]); else the shoulders blocked -> the shoulder impact; else the elbow impact.
fn impact(s: &Swing, collision: &CollisionWorld) -> (u32, f32) {
    let back = s.hands_mid - s.fwd * 0.25;
    let upper = collision.sphere_free_distance(back - Vec3::Y * 0.65, s.fwd, 0.35, 0.5) / 0.5;
    let lower = collision.sphere_free_distance(back - Vec3::Y * 1.6, s.fwd, 0.6, 0.5) / 0.5;
    if lower < 1.0 {
        (IMPACT_LEGS, lower)
    } else if upper < 1.0 {
        (IMPACT_SHOULDER, upper)
    } else {
        (IMPACT_ELBOW, 1.0)
    }
}

pub enum SwingOut {
    Stay,
    /// The free hang (Ledge Movement, `sub_DCE900` / the settle's `_tr_hangfree`).
    Hang,
    /// The swing jump (event 7 -> `sub_DCD0B0`: `Human__SetupJumpToTarget` with jump kind 3).
    Jump(JumpTarget),
}

/// `HumanLedge__StateSwingReception_Update` 0xDD24F0 with its events (0xDD2890). The interpreter's ledge state
/// (0xEECCF9–0xEED11F, RE/24 LEDGE-17 / CENSUS-3) drives it with held buttons: `held` = high profile and Legs held
/// (+4397; its negation +4398 sends the stop, event 6), `jump_pressed` = held, Legs pressed and the stick past the dead
/// zone within 75° of the facing (event 7). `find_target` = the jump target along the facing (the guidance search;
/// PORT: the Leap of Faith and the free jump that follow it are not modelled).
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
                    return SwingOut::Jump(t);
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
            *feet = s.root;
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

fn legs_weights(f: f32) -> Vec<f32> {
    if f <= 0.5 { vec![1.0 - f * 2.0, f * 2.0, 0.0] } else { vec![0.0, 2.0 - f * 2.0, f * 2.0 - 1.0] }
}

#[cfg(test)]
mod stop_tests {
    use super::*;

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
