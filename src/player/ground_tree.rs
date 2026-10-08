//! The ground state tree's way back into the locomotion (RE/02 §4.1.0, §4.6).
//!
//! The game never cuts from a landing, a run stop, a pivot or a start straight into the 17-clip locomotion action
//! `0x05923BDB`. `HumanGround__Move_EnterBody` 0xD94A60 plays the locomotion, and the animation graph puts the
//! playing item's authored transition action in front of it (`AnimSlot__SetAction` 0x727F70: ActionTransition +4
//! the transition action, +16 the locomotion item it hands over to). While that transition action plays,
//! `HumanGround__UpdateMoveBlend` 0xDA0810 runs its *transition path* (0xDA09B3–0xDA16CC): the speed parameter is
//! steered into the transition's band by the layout HG+0x724 and the transition's own clips are weighted by it.
//! When the transition item ends, the locomotion item `u_b` takes over at that speed parameter.

use super::jump_blend::ActionBlend;
use super::jump_clips::GROUND_TRANSITIONS;
use super::move_blend::ACT_GROUND_LOCOMOTION;
use std::f32::consts::FRAC_PI_2;

/// The ground waits, [low, high profile] x [left, right foot ahead] (table 0x1A2BFE0: legacy 48 / 50, 51 / 53).
pub const WAITS: [[u32; 2]; 2] = [[0x00D8_243F, 0x00D8_24C5], [0x00D8_2508, 0x00D8_258E]];
/// The parallel-feet waits, [low, high] (legacy 49 / 52): Idle's wait table has a third column for the exit foot 3
/// (`HumanGround__PickLocomotionFoot` 0xD86760 → 2), played by the wait's enter 0xD94580.
pub const WAITS_PARALLEL: [u32; 2] = [0x00D8_2482, 0x00D8_254B];

/// The wait Idle plays for a profile and a `PickLocomotionFoot` result (0 left ahead, 1 right ahead, 2 parallel).
pub fn wait_action(high: bool, foot: usize) -> u32 {
    if foot >= 2 { WAITS_PARALLEL[high as usize] } else { WAITS[high as usize][foot] }
}

/// The destinations whose authored item transitions the simulation follows: the locomotion and the four waits.
pub const GROUND_DESTINATIONS: [u32; 7] = [ACT_GROUND_LOCOMOTION, WAITS[0][0], WAITS[0][1], WAITS[1][0], WAITS[1][1], WAITS_PARALLEL[0], WAITS_PARALLEL[1]];

/// Run stop → Move (`HumanGround__PlayRunStopExitMove` 0xD8B3F0): legacy 92 / 93,
/// `xx_h_runstop_foot{l,r}_tr_{walk,jog}_hipm_foot{r,l}` [walk, jog]. 92 follows the left-foot stop (legacy 86) and
/// hands over to locomotion item 1; 93 to item 0.
pub const RUN_STOP_EXIT: [u32; 2] = [0x00D8_3940, 0x00D8_3983];
/// The skid turn (`HumanGround__RunTurn_Enter` 0xD8B8E0, table 0x1A2C084 = legacy 94 / 95): `xx_h_runturn180_foot{l,r}`,
/// locked, the animation turns the body (word 0x3A / 0x35).
pub const RUN_TURN: [u32; 2] = [0x00D8_39D6, 0x00D8_3A1A];
/// Skid turn → Move while still turned (`HumanGround__PlayRunTurnExitMove` 0xD8F2B0): legacy 96 / 97,
/// `xx_h_runturn180_foot{l,r}_tr_{walk,jog}_hipm_foot{l,r}` [walk, jog]; 96 hands over to item 0, 97 to item 1.
pub const RUN_TURN_EXIT: [u32; 2] = [0x00D8_3A5E, 0x00D8_3AA1];
/// The pass-over roll ending (Movement entry mode 12, state 20, `HumanGround__Movement_Enter` 0xDA8890 table
/// 0x1A2C0D4) and its exit into the locomotion `roll_hipm_tr_l_walk_hipm_footr` (0xD8B780).
pub const ROLL_ENDING: [u32; 2] = [0x0A4C_AAC6, 0x0A4C_AAC5];
pub const ROLL_ENDING_EXIT: u32 = 0x082F_B27C;

/// Actions the clip generator dumps for the tree (beyond the ones other modules list).
pub const DUMPED_ACTIONS: &[u32] = &[
    RUN_STOP_EXIT[0], RUN_STOP_EXIT[1], RUN_TURN[0], RUN_TURN[1], RUN_TURN_EXIT[0], RUN_TURN_EXIT[1],
    ROLL_ENDING[0], ROLL_ENDING[1], ROLL_ENDING_EXIT,
];

/// The landings whose authored ending is the wait (`HumanGround__Guard_LandingToIdle` 0xD7F090): forward → wait
/// by foot and straight → wait. Every other landing hands over to the locomotion (0xD7F0F0).
pub const LANDINGS_TO_WAIT: [u32; 3] = [0x6E9C_754A, 0x6E9C_754C, 0x6E9C_7557];
/// The heavy landing whose idle exit keeps the high profile (`HumanGround__DamageLandingToIdle` 0xDA77F0).
pub const LANDING_DAMAGE: u32 = 0x010D_D707;

/// The authored transition from item `item` of `action` into `dest` (the ActionTransition the graph finds first,
/// `sub_5B98B0`): (transition action, its item, the destination item).
pub fn authored_transition(action: u32, item: usize, dest: u32) -> Option<(u32, usize, usize)> {
    GROUND_TRANSITIONS
        .iter()
        .find(|t| t.0 == action && t.1 == item && t.4 == dest)
        .map(|t| (t.2, t.3 as usize, t.5 as usize))
}

/// `HumanGround__PickLocomotionFoot` 0xD86760 (without the Data+556 override): the playing item's authored exit foot
/// (word bits 2–3: 1 left ahead → 0, 2 right ahead → 1, 3 parallel → 2), else the leading foot (0 = left ahead).
pub fn pick_locomotion_foot(word: Option<u16>, leading: usize) -> usize {
    match word.map(|w| (w >> 2) & 3) {
        Some(1) => 0,
        Some(2) => 1,
        Some(3) => 2,
        _ => leading,
    }
}

/// The speed parameter's target on the transition path (0xDA09B3): the normal target `t` (≤ 1) limited by the layout.
/// `cap` is 1.0 with Sprint (Data+0x123), else 0.5. The exe first clamps `t` to [0.25, 0.5] unless the entity's
/// `EntityDescriptorType` (Human+0x100 → HumanData+4 = entity, +168 = the reflected `EntityDescriptor`, low 3 bits) is
/// 1 = Main, the player character (verified 2026-10-08): only NPCs get the clamp, so the player's start reaches the
/// slow-walk start at a light stick and the sprint impulsion with Sprint.
pub fn transition_target(layout: u8, t: f32, s: f32, sprint: bool) -> f32 {
    let t = t.min(1.0);
    let cap = if sprint { 1.0 } else { 0.5 };
    match layout {
        0 => 0.5,
        1 | 4 => t.clamp(0.25, 0.5),
        2 => if s < 0.5 { 0.25 } else { 0.5 },
        3 => t.clamp(0.0, 0.5),
        5 | 6 => t.clamp(0.25, cap),
        _ => t.min(cap),
    }
}

/// The speed step of the transition path: 1/s both ways, no deceleration curve; nothing moves within 0.0005.
pub fn transition_speed(s: f32, target: f32, dt: f32) -> f32 {
    if (target - s).abs() <= 0.000_5 {
        s
    } else if s < target {
        (s + dt).min(target)
    } else {
        (s - dt).max(target)
    }
}

/// The transition item's weights for the layout (`sub_502E30` in 0xDA09B3). `prev` is the slot's current weight
/// array: the animation slot keeps one weight array (`sub_725B30` reads the slot's, not the item's), so layouts 4 and
/// 6 keep the authored pair ratio of whatever was set before. `turn` is HG+0x2B0, the pivot's turn angle. Returns
/// `None` for layout 0 (weights left alone).
pub fn transition_weights(layout: u8, s: f32, turn: f32, prev: &[f32]) -> Option<Vec<f32>> {
    let f = (4.0 * (s - 0.25)).clamp(0.0, 1.0);
    let f2 = (2.0 * (s - 0.5)).clamp(0.0, 1.0);
    let a = ((turn.abs() - FRAC_PI_2) / FRAC_PI_2).clamp(0.0, 1.0);
    let w = |i: usize| prev.get(i).copied().unwrap_or(0.0);
    Some(match layout {
        0 => return None,
        1 => vec![1.0 - f, f],
        2 => vec![(1.0 - f) * (1.0 - a), (1.0 - f) * a, f * (1.0 - a), f * a],
        3 => {
            if s <= 0.25 {
                let g = 4.0 * s;
                vec![(1.0 - g) * (1.0 - a), (1.0 - g) * a, g * (1.0 - a), g * a, 0.0, 0.0]
            } else {
                vec![0.0, 0.0, (1.0 - f) * (1.0 - a), (1.0 - f) * a, f * (1.0 - a), f * a]
            }
        }
        4 => {
            let g = (w(0) + w(2)).clamp(0.0, 1.0);
            vec![g * (1.0 - f), (1.0 - g) * (1.0 - f), g * f, (1.0 - g) * f]
        }
        5 => if s <= 0.5 { vec![1.0 - f, f, 0.0] } else { vec![0.0, 1.0 - f2, f2] },
        6 => {
            let g = w(0) + w(1) + w(2);
            if s <= 0.5 {
                vec![g * (1.0 - f), g * f, 0.0, (1.0 - g) * (1.0 - f), (1.0 - g) * f, 0.0]
            } else {
                vec![0.0, g * (1.0 - f2), g * f2, 0.0, (1.0 - g) * (1.0 - f2), (1.0 - g) * f2]
            }
        }
        _ => {
            if s <= 0.25 {
                vec![1.0 - 4.0 * s, 4.0 * s, 0.0, 0.0]
            } else if s <= 0.5 {
                vec![0.0, 1.0 - f, f, 0.0]
            } else {
                vec![0.0, 0.0, 1.0 - f2, f2]
            }
        }
    })
}

/// A transition action playing in front of the locomotion (Move state, MoveBlend's transition path).
#[derive(Clone, Copy, Debug)]
pub struct MoveTransition {
    /// The transition action's item with this frame's weights.
    pub action: ActionBlend,
    /// Normalised time in the item (its clips share one clock; the item lasts Σwᵢ·Tᵢ, 0x507650).
    pub phase: f32,
    /// Root displacement / yaw already applied (animation space), so each frame moves by the difference.
    pub applied: [f32; 3],
    pub yaw_applied: f32,
    /// HG+0x724.
    pub layout: u8,
    /// The locomotion item the transition hands over to (ActionTransition +16): 0 footl, 1 footr.
    pub dest_item: usize,
    /// HG+1832 bit 0 (`HumanGround__PlayTurnStartMove` 0xD99710, a low-profile turn start with the high profile
    /// wanted): the item plays at rate `2 − progress` (`sub_502A90` with `2 − sub_501DF0(0)`; **hypothesis**: vfunc +76
    /// of the item node is its normalised time).
    pub fast_start: bool,
    /// The item the transition was entered from (the animator looks its authored blends up there).
    pub from: Option<(u32, usize)>,
    /// The transition leads into the wait (Idle), not into the locomotion: no MoveBlend, its weights stay as set.
    pub to_wait: bool,
    /// The blend into the transition action when the code passes it explicitly (`ActionParams` of the start, the
    /// run stop and skid-turn exits); `None`: the authored item transition's blend A.
    pub blend_in: Option<crate::assets::ac_actions::ActBlend>,
    /// Restart key for the animator.
    pub seq: u32,
}

impl MoveTransition {
    pub fn new(action: ActionBlend, layout: u8, dest_item: usize, from: Option<(u32, usize)>, seq: u32) -> Self {
        MoveTransition { action, phase: 0.0, applied: [0.0; 3], yaw_applied: 0.0, layout, dest_item, fast_start: false, from, to_wait: false, blend_in: None, seq }
    }

    /// One frame of the transition path after the speed parameter `s` was updated: new weights, then the clock.
    /// Returns (root displacement this frame, root yaw this frame, the item ended). The step is taken with this
    /// frame's weights over the phase interval, as the blended item is sampled at the new time.
    pub fn advance(&mut self, s: f32, turn: f32, dt: f32) -> ([f32; 3], f32, bool) {
        if let Some(w) = transition_weights(self.layout, s, turn, self.action.weights()) {
            // `SetItemBlendWeights` writes the slots the item has; the rest of the array is not read
            let n = match self.action.clips().len() { 0 => self.action.n, c => c }.min(40);
            let mut nw = [0.0f32; 40];
            for (k, v) in nw.iter_mut().enumerate().take(n) {
                *v = w.get(k).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            }
            self.action.w = nw;
            self.action.n = n;
        }
        let d0 = self.action.disp(self.phase);
        let y0 = self.action.yaw(self.phase);
        let duration = self.action.duration().max(1e-4);
        let rate = if self.fast_start { 2.0 - self.phase } else { 1.0 };
        self.phase = (self.phase + dt * rate / duration).min(1.0);
        let d1 = self.action.disp(self.phase);
        let y1 = self.action.yaw(self.phase);
        ([d1[0] - d0[0], d1[1] - d0[1], d1[2] - d0[2]], y1 - y0, self.phase >= 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_targets_follow_the_table() {
        assert_eq!(transition_target(0, 1.0, 0.0, true), 0.5);
        assert_eq!(transition_target(1, 1.0, 0.0, true), 0.5);
        assert_eq!(transition_target(1, 0.1, 0.0, true), 0.25);
        assert_eq!(transition_target(2, 1.0, 0.3, false), 0.25);
        assert_eq!(transition_target(2, 1.0, 0.5, false), 0.5);
        assert_eq!(transition_target(3, 0.1, 0.0, false), 0.1);
        assert_eq!(transition_target(6, 1.0, 0.0, false), 0.5);
        assert_eq!(transition_target(6, 1.0, 0.0, true), 1.0);
        assert_eq!(transition_target(7, 0.1, 0.0, true), 0.1);
    }

    #[test]
    fn layout_weights_follow_the_table() {
        // layout 6 keeps the soft / hard share of the landing that set the slot's weights
        let w = transition_weights(6, 0.375, 0.0, &[0.3, 0.0, 0.0, 0.7, 0.0, 0.0]).unwrap();
        let want = [0.15, 0.15, 0.0, 0.35, 0.35, 0.0];
        for (a, b) in w.iter().zip(want) {
            assert!((a - b).abs() < 1e-5, "{w:?}");
        }
        // layout 7 (start from standing): slow walk → walk → jog → impulsion by s
        assert_eq!(transition_weights(7, 0.25, 0.0, &[]).unwrap(), vec![0.0, 1.0, 0.0, 0.0]);
        assert_eq!(transition_weights(7, 0.5, 0.0, &[]).unwrap(), vec![0.0, 0.0, 1.0, 0.0]);
        assert_eq!(transition_weights(7, 1.0, 0.0, &[]).unwrap(), vec![0.0, 0.0, 0.0, 1.0]);
        // layout 2 (high-profile turn start): the 90 / 180 share by the turn
        let w2 = transition_weights(2, 0.5, std::f32::consts::PI * 0.75, &[]).unwrap();
        assert!((w2[2] - 0.5).abs() < 1e-5 && (w2[3] - 0.5).abs() < 1e-5, "{w2:?}");
        assert!(transition_weights(0, 0.3, 0.0, &[]).is_none());
    }

    #[test]
    fn speed_moves_at_one_per_second_both_ways() {
        assert!((transition_speed(0.5, 1.0, 0.1) - 0.6).abs() < 1e-6);
        assert!((transition_speed(0.5, 0.25, 0.1) - 0.4).abs() < 1e-6);
        assert_eq!(transition_speed(0.5, 0.5004, 0.1), 0.5);
    }

    #[test]
    fn exit_foot_comes_from_the_item_word() {
        assert_eq!(pick_locomotion_foot(Some(0x0fc4), 1), 0);
        assert_eq!(pick_locomotion_foot(Some(0x0fc8), 0), 1);
        assert_eq!(pick_locomotion_foot(Some(0x0fc0), 1), 1);
        assert_eq!(pick_locomotion_foot(None, 0), 0);
    }
}
