//! Ledge context (`HumanLedge`, ActorContextID 9) â€” RE/03 Â§7.
//!
//! - Hangs by two hand contacts on guidance edges. Wall hang: root 1.1 m below the hands and 0.5 m
//!   out from the wall; free hang: root 2.4 m below (0xDD6730). Hang type re-evaluated after each
//!   step (TrySwitchHangType 0xDE1060).
//! - All movement is discrete: pick new hand targets, then interpolate the root over the move's
//!   animation length (PLACEHOLDER durations until clips are decoded). SubState HandPlacement (3)
//!   while a step runs, Movement (1) otherwise.
//! - Shimmy (ProbeLateral 0xDD9640 / StartHandStep 0xDDE0C0): alternating hands, the lead reaching the farthest point
//!   of a box beyond it, the trailing hand closing to 0.15 m of it; free-space sphere
//!   sweep; step = min(d âˆ’ 0.4, 1.0 âˆ’ spacing); blocked if d âˆ’ 0.15 < 0.7; min step 0.15; chain ends
//!   handled implicitly (no edge further on â†’ no step).
//! - Up/down: transition to Climb when wall holds exist (TryTransitionToClimb 0xDD46A0), else
//!   hand step to an edge 0.1-0.8 m away (sub_DD8F70), else (up, wall hang) the swing back into a free hang
//!   (TryWallJumpUp 0xDD62A0), else (up) "blocked up" -> pull-up if there is standing space (CanPullup 0xDE2270).
//! - Left/right past the end of the edge (or blocked): inner corner, side jump, outer corner
//!   (`ledge_moves`, RE/03 §7.6b).
//! - Let go (Legs) â†’ InAir; lost ledge (HasLostLedge 0xDD20D0) â†’ InAir.

use bevy::prelude::*;

use super::air::{FallOrigin, InAirEntry};
use super::climb::{ClimbEntry, ClimbEntryType};
use super::ledge_moves::{self, LedgeMove};
use super::{
    heading_of, right_of, switch_context, ActorContextId, Body, HumanDataBundle, LimbTargets, Locomotion, Player,
    RootInterp, TransitionSetup,
};
use crate::collision::CollisionWorld;
use crate::guidance::GuidanceWorld;
use crate::input::PadInput;
use crate::tuning::*;

/// `HumanLedgeData::LedgeSubState` (values from the exe, desc 0x1994E38) â€” subset used here.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LedgeSubState {
    #[default]
    Entry = 0,
    Movement = 1,
    TurnCorner = 2,
    HandPlacement = 3,
    Pullup = 4,
    ReboundTransition = 5,
    HangWallReception = 6,
    HangFreeReception = 7,
    TransitionInFromClimb = 9,
    PullDown = 11,
    Grasp = 13,
    ParallelJump = 14,
}

/// The ledge sub-states whose update runs the limb IK (`sub_DCD910` 0xDCD910: SubState 0, 1, 3, 4, 6, 8, 11, 13; RE/24
/// ANIM-10), by the playing `action`:
/// - Entry (0) only while the climb's swing back (0x1EB21811), the swing back's `_b` (`SWING_BACK_B`) or `step1_to_hangwall`
///   (0x0106CFC5) plays;
/// - Pullup (4) only while `xx_h_hangfree_tr_hangwall` (0x01B702FD) plays;
/// - never while one of `IK_NEVER` plays (the wall run's swing into the wall-free hang, `air_all_to_swing_tr_hangfree_b`,
///   and ids that are not in Game Fix).
/// The swing (8) runs in `LedgeData::swing`.
pub fn ledge_ik_runs(s: LedgeSubState, action: Option<u32>) -> bool {
    if action.is_some_and(|a| IK_NEVER.contains(&a)) {
        return false;
    }
    match s {
        LedgeSubState::Entry => action.is_some_and(|a| IK_ENTRY.contains(&a)),
        LedgeSubState::Pullup => action == Some(0x01B7_02FD),
        LedgeSubState::Movement | LedgeSubState::HandPlacement | LedgeSubState::HangWallReception | LedgeSubState::PullDown | LedgeSubState::Grasp => true,
        _ => false,
    }
}

/// `sub_DCD910`'s Entry actions with the IK on: the climb's swing back, `SWING_BACK_B`, `step1_to_hangwall`.
const IK_ENTRY: [u32; 3] = [0x1EB2_1811, 0x4CD6_8F50, 0x0106_CFC5];
/// `sub_DCD910`'s actions that never run the IK.
const IK_NEVER: [u32; 10] = [0x0106_F36F, 0x0121_A15E, 0x035F_27A1, 0x035F_27A2, 0x035F_27AD, 0x035F_27AF, 0x0292_6C68, 0x0292_6C69, 0x0292_40CA, 0x0292_40CB];

/// `HumanLedgeData::LedgeHangType` (desc 0x1994E68). WallFree is treated as Free by the game.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LedgeHangType {
    #[default]
    Wall = 0,
    Free = 1,
}

#[derive(Clone, Copy, Debug)]
pub struct LedgeEntry {
    pub hand_l: Vec3,
    pub hand_r: Vec3,
    /// Wall normal (pointing out of the solid).
    pub normal: Vec3,
    pub from_feet: Vec3,
    pub sub_state: LedgeSubState,
    /// The reception after a jump at the ledge (`ledge_moves::arrival_move`); replaces the plain grab.
    pub entry_move: Option<LedgeMove>,
    /// Further moves played after `entry_move` (the pull-down's descent and reception).
    pub entry_rest: [Option<LedgeMove>; 2],
    /// Caught from a fall (`HumanInAir__CheckAirCatch` 0xE0BB70): Some(long catch, a fall of 3 m or more).
    pub catch: Option<bool>,
}

impl LedgeEntry {
    /// Hands centred on `mid`, `HAND_SPACING` apart along the edge, facing into the wall.
    pub fn at(mid: Vec3, normal: Vec3, from_feet: Vec3, sub_state: LedgeSubState) -> Self {
        let r = right_of(-normal);
        Self {
            hand_l: mid - r * HAND_SPACING * 0.5,
            hand_r: mid + r * HAND_SPACING * 0.5,
            normal,
            from_feet,
            sub_state,
            entry_move: None,
            entry_rest: [None, None],
            catch: None,
        }
    }
}

/// The air catches (CheckAirCatch): ledge with the wall below (3-way angle blend), free hang; [< 3 m, >= 3 m].
pub const ACT_CATCH_WALL: [u32; 2] = [0x1F0C_0C23, 0x1F0C_0C2D];
pub const ACT_CATCH_FREE: [u32; 2] = [0x1F0C_2EB8, 0x1F0C_2EB9];
/// Actions the clip generator dumps for the ledge (the catches, for their lengths; the wall-free → wall switch).
/// The shimmy actions (`StartHandStep` 0xDDE0C0): [wall, free] × [left open, left close, right open, right close].
pub const ACT_SHIMMY: [[u32; 4]; 2] = [[0x01B7_0B35, 0x01B7_0B36, 0x01B7_0B37, 0x01B7_0B38], [0x01A2_490A, 0x01A2_490B, 0x01A2_490C, 0x01A2_490D]];
/// `HumanLedge__SetAlternationFromHandSpacing` 0xDCBCD0: +133 = the hand targets 0.3 m or more apart.
fn alternation_from_spacing(l: Vec3, r: Vec3) -> bool {
    (l - r).length() >= 0.3
}

/// The `ACT_SHIMMY` column: the move's side and whether this step opened the hands (the alternation flag after it).
pub fn shimmy_index(right: bool, open: bool) -> usize {
    (right as usize) * 2 + (!open) as usize
}

/// The vertical hand steps: [wall, free] × [up: 1m_u_1lu, 1m_u_1ru, 1lu_u_1m, 1ru_u_1m, down: 1m_d_1lu, 1m_d_1ru,
/// 1lu_d_1m, 1ru_d_1m].
pub const ACT_VSTEP: [[u32; 8]; 2] = [
    [0x01B7_0FAA, 0x01B7_0FAB, 0x01B7_0FAC, 0x01B7_0FAD, 0x01B7_0FAE, 0x01B7_0FAF, 0x01B7_0FB0, 0x01B7_0FB1],
    [0x01A2_79B9, 0x01A2_79BA, 0x01A2_79BB, 0x01A2_79BC, 0x01A2_79BD, 0x01A2_79BE, 0x01A2_79BF, 0x01A2_79C0],
];
/// The `ACT_VSTEP` column for a vertical hand step (`PickVerticalHandAndAnim` 0xDCB3E0, tables `StaticInitTables`
/// 0xDCC3E5..0xDCC403): up, left first → `1m_u_1lu`; down, left first → `1m_d_1ru` (the clips are named by the hand left
/// up), and the second halves likewise (RE/24 LEDGE-6). Played the other way round, the clip's raised hand was the one
/// the IK held low and the lowered one could not reach its hold (bug-1791714114).
pub fn vstep_index(down: bool, second: bool, first_left: bool) -> usize {
    if down {
        4 + (second as usize) * 2 + first_left as usize
    } else {
        (second as usize) * 2 + (!first_left) as usize
    }
}

pub const DUMPED_ACTIONS: &[u32] = &[
    ACT_HANGFREE_SWINGBACK,
    ACT_CATCH_WALL[0], ACT_CATCH_WALL[1], ACT_CATCH_FREE[0], ACT_CATCH_FREE[1], super::ledge_moves::WALLFREE_TO_WALL,
    // the hand steps, for their lengths (RE/24 LEDGE-21)
    ACT_SHIMMY[0][0], ACT_SHIMMY[0][1], ACT_SHIMMY[0][2], ACT_SHIMMY[0][3], ACT_SHIMMY[1][0], ACT_SHIMMY[1][1], ACT_SHIMMY[1][2], ACT_SHIMMY[1][3],
    ACT_VSTEP[0][0], ACT_VSTEP[0][1], ACT_VSTEP[0][2], ACT_VSTEP[0][3], ACT_VSTEP[0][4], ACT_VSTEP[0][5], ACT_VSTEP[0][6], ACT_VSTEP[0][7],
    ACT_VSTEP[1][0], ACT_VSTEP[1][1], ACT_VSTEP[1][2], ACT_VSTEP[1][3], ACT_VSTEP[1][4], ACT_VSTEP[1][5], ACT_VSTEP[1][6], ACT_VSTEP[1][7],
];

/// The summed length of an action's items (their first clips).
pub fn action_time(id: u32) -> f32 {
    use super::jump_blend::{action_items, ActionBlend};
    let Some(items) = action_items(id) else { return GRAB_TIME };
    let t: f32 = (0..items.len())
        .map(|i| {
            let n = items[i].len();
            let w: Vec<f32> = (0..n).map(|k| if k == 0 { 1.0 } else { 0.0 }).collect();
            ActionBlend::new(id, i, &w).duration()
        })
        .sum::<f32>()
        + super::jump_blend::item_blend_adjust(id);
    if t > 0.0 { t } else { GRAB_TIME }
}

/// `xx_h_hangfree_swingback_{min,max}_a` + `_b`: the graph's transition action into the free idle 0x012719F1 from walling's
/// free hangs (items 0x0564BB1F / 0x0564BB20), the climb's jump up (0x1EB21812) and the swing back's `_b` (0x4CD68F52).
/// Its first item is locked (word 0x0FF0), the second not (0x0FC0).
pub const ACT_HANGFREE_SWINGBACK: u32 = 0x0564_BB0E;

/// How long the swing into the free idle holds the hang: `HumanLedge__EntryEnd_PlayIdle` 0xDE1FE0 requests the free idle,
/// the graph plays `ACT_HANGFREE_SWINGBACK` first, and the weights [1 − s, s] (`sub_502E30`, s = SwingStrength +112) go to
/// its playing first item (min / max); `HumanLedge__Movement_ChooseAction` 0xDE29E0 starts no move while that item plays
/// (locked, 0xDE2B1C). Live (2026-10-13, wall 00): the swing from 1.15 s, the climb at 1.85 s.
pub fn swing_lock_time(s: f32) -> f32 {
    super::jump_blend::ActionBlend::new(ACT_HANGFREE_SWINGBACK, 0, &[1.0 - s, s]).duration()
}

/// How long a catch holds the hang: both items of the catch action are locked (word 0x0FE0), and
/// `HumanLedge__Movement_ChooseAction` 0xDE29E0 starts no move while the playing item is locked. The wall catch
/// takes its straight clips (PORT: the port's ledges are straight, as the animator plays them).
/// The catches' root interpolation (`HumanInAir__CheckAirCatch` 0xE0DB5C): the length of the item playing once the catch
/// starts (`sub_502590`(0), item 0) + 0.2 s, linear (mode 0). The catch action itself plays on to its end.
pub fn catch_root_time(id: u32) -> f32 {
    use super::jump_blend::{action_items, ActionBlend};
    let t = action_items(id).and_then(|items| items.first().map(|c| {
        let w: Vec<f32> = (0..c.len()).map(|k| if k == 0 { 1.0 } else { 0.0 }).collect();
        ActionBlend::new(id, 0, &w).duration()
    })).unwrap_or(0.0);
    if t > 0.0 { t + 0.2 } else { GRAB_TIME }
}

pub fn catch_time(wall: bool, long: bool) -> f32 {
    use super::jump_blend::{action_items, ActionBlend};
    let id = if wall { ACT_CATCH_WALL[long as usize] } else { ACT_CATCH_FREE[long as usize] };
    let Some(items) = action_items(id) else { return GRAB_TIME };
    let t: f32 = (0..items.len())
        .map(|i| {
            let n = items[i].len();
            let w: Vec<f32> = (0..n).map(|k| if k == 0 { 1.0 } else { 0.0 }).collect();
            ActionBlend::new(id, i, &w).duration()
        })
        .sum();
    if t > 0.0 { t } else { GRAB_TIME }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum After {
    Hang,
    /// The first half of a vertical hand-over-hand step ended: the second hand (index) waits for the stick (up?).
    SecondHand(usize, bool),
    StandOnTop,
}

/// Runtime data of the Ledge context (subset of reflected HumanLedgeData, RE/07 / RE/03 Â§2.3).
#[derive(Debug, Default)]
pub struct HumanLedgeData {
    /// HandPassOver (SubState 12): the pass-over vault (`passover`).
    pub pass_over: Option<super::passover::HandPassOver>,
    /// SwingReception (SubState 8): swinging on a bar (`swing`).
    pub swing: Option<super::swing::Swing>,
    pub sub_state: LedgeSubState,
    pub hang_type: LedgeHangType,
    pub hand_l: Vec3,
    pub hand_r: Vec3,
    pub normal: Vec3,
    /// +0x85: shimmy alternation flag. false = lead hand reaches out next, true = trailing hand closes.
    pub alt_flag: bool,
    /// +0x746: stick up and nothing else possible â†’ pull-up allowed.
    pub blocked_up: bool,
    pub last_action: &'static str,
    /// Incremented whenever a discrete move starts (lets the animator restart one-shot clips).
    pub step_seq: u32,
    /// The entry ends in the Entry state (LedgeData +76 = 0: walling's hangs, the climb's jump up): its end re-poses the
    /// hands from the hand bones (`HumanLedge__EntryEnd_PlayIdle` 0xDE1FE0; RE/24 LEDGE-22). Set by the caller after the
    /// switch; `enter` clears it.
    pub repose_at_entry_end: bool,
    /// +1844: the hand the last step moved (0 left, 1 right; `StartHandStep` 0xDDE0C0). `EnterCommon` 0xDE26D0 sets 0, so
    /// the first vertical step after an entry moves the right hand (`PickVerticalHandAndAnim` 0xDCB3E0, RE/24 LEDGE-5).
    pub last_hand: usize,
    /// Running vertical hand step: (first hand was the left one, this is the second half).
    pub vstep: Option<(bool, bool)>,
    /// A vertical step's second hand still to move (+1848, `ContinuePendingVerticalStep` 0xDCF3C0): (that hand's index,
    /// the pending direction is up, +1852). The hands stay split until the stick asks for it (RE/24 LEDGE-4).
    pub pending_vstep: Option<(usize, bool)>,
    /// The jump off's stored target (event 2 → 0xDD2CA0 copies it to LedgeData+64), jumped at when the rebound ends.
    pub jump_off: Option<super::air::InAirEntry>,
    /// The last lateral move that was blocked went right (picks `PlayIdleOrBlocked`'s blocked action, 0xDD3A10).
    pub blocked_right: bool,
    /// LedgeData +108 = 3 (HangFreeReceptionType) and +112 (SwingStrength), with the `step_seq` it was stored at: the swing
    /// back's `_b` (`PlayHopUpSecondPart` 0xDDAB00), walling's free hangs B / C (0xE3807F–0xE380D7, 0xE3898C–0xE389E8)
    /// and the climb's jump up (0xDF9650) store h = w[2] + w[3]; the free idle that follows (`HumanLedge__EntryEnd_PlayIdle` 0xDE1FE0) plays its two clips
    /// (`xx_h_hangfree_wait`, `xx_h_hangwallfree_wait`) at [1 − h, h] (`sub_502E30`) until the next move (RE/24 LEDGE-24,
    /// WALL-21).
    pub swing_strength: Option<(f32, u32)>,
    /// The time left on the swing into the free idle (`swing_lock_time`): no move starts meanwhile; let-go, the jump off
    /// and the swing start are events and still run (event 4's guard 0xDCCDD0 accepts during the swing, `sub_DCCD50`).
    pub swing_lock: f32,
    moves: Vec<RootInterp>,
    after: Option<After>,
    /// Running corner turn / ledge jump / swing back (`ledge_moves`).
    pub mv: Option<LedgeMove>,
    /// Moves queued after `mv` (pull-down stages); each starts from where the previous one ended.
    pub queue: Vec<LedgeMove>,
    /// The Climb context takes over when `mv` ends (TryJumpUpToClimb 0xDD5E10: landing category 0).
    pub then_climb: Option<ClimbEntry>,
    /// The Ladder context takes over when `mv` ends (TrySideMoveToLadder 0xDD4CD0: flag 1860 | 0x10).
    pub then_ladder: Option<super::ladder::LadderEntry>,
    /// The hang type has been evaluated for this hang (it then changes only through moves: switches,
    /// corners, jumps — 0xDE1060).
    pub hang_set: bool,
    /// The entry was an air catch (long or not): the grab lasts the catch action, set once the hang type is known.
    pub catch: Option<bool>,
    /// The catch action chosen by the air search (`air_catches::ledge_search`, RE/19 §2), and whether it is a free
    /// hang (the hang type follows it) and, for the one-handed side catch, the hand that holds (true = right).
    pub catch_action: Option<u32>,
    pub catch_free: bool,
    pub catch_one_hand: Option<bool>,
}

impl HumanLedgeData {
    /// The swing strength for the free idle, while no move has started since it was stored.
    pub fn swing_idle(&self) -> Option<f32> {
        self.swing_strength.filter(|(_, seq)| *seq == self.step_seq).map(|(s, _)| s)
    }

    pub fn enter(&mut self, e: LedgeEntry) {
        self.hand_l = e.hand_l;
        self.hand_r = e.hand_r;
        self.normal = e.normal;
        self.sub_state = e.sub_state;
        self.pass_over = None;
        self.swing = None;
        // EnterCommon 0xDE26D0 clears +0x85, then `HumanLedge__OnEnter` 0xDE3460 sets it from the hand targets
        // (`HumanLedge__SetAlternationFromHandSpacing` 0xDCBCD0: 0.3 m or more apart → the trailing hand closes first)
        self.alt_flag = alternation_from_spacing(e.hand_l, e.hand_r);
        self.repose_at_entry_end = false;
        self.last_hand = 0; // and +1844
        self.blocked_up = false;
        self.pending_vstep = None;
        self.jump_off = None;
        self.swing_strength = None;
        self.swing_lock = 0.0;
        self.moves.clear();
        self.mv = None;
        self.hang_set = false;
        self.catch = e.catch.filter(|_| e.entry_move.is_none());
        self.catch_action = None;
        self.catch_free = false;
        self.catch_one_hand = None;
        self.after = Some(After::Hang);
        self.last_action = "grab";
        self.step_seq += 1;
        self.queue = e.entry_rest.iter().flatten().copied().collect();
        self.then_climb = None;
        self.then_ladder = None;
        // walling's free hangs B / C: +108 = 3, +112 = w₂ + w₃ of their 4-way blend (WALL-21)
        let walling_free = |a: &super::jump_blend::ActionBlend| [super::walling::ENTRY_TO_HANGFREE, super::walling::STEP1_TO_HANGFREE].contains(&a.id);
        if let Some(a) = e.entry_move.and_then(|m| m.seq[0]).filter(walling_free) {
            self.swing_strength = Some((a.w[2] + a.w[3], self.step_seq));
        }
        // the climb's jump up the same way: +108 = 3, +112 = w₂ + w₃ of the swingback's 4-way blend
        // (`HumanClimb__FillLedge_JumpToOverhang` 0xDF9A9A–0xDF9AD1)
        if let Some(a) = e.entry_move.and_then(|m| m.seq[1]).filter(|a| a.id == super::climb::CLIMB_JUMP_SWINGBACK) {
            self.swing_strength = Some((a.w[2] + a.w[3], self.step_seq));
        }
        // `sub_DCDA10` (`HumanLedge__EnterCommon` 0xDE284E): entered while the climb's overhang move plays, the hands are a
        // row apart and the lower one's step is pending, upward (+1848 / +1852 = 0), the higher one moved last (+1844):
        // `1lu` (0x1DBDB3D8) leaves the right hand pending, `1ru` (0x1DBDB3D9) the left. The next stick up finishes it
        // (`xx_h_hangfree_climb_1lu_u_1m`, live wall 00)
        if let Some(k) = e.entry_move.and_then(|m| m.seq[0]).and_then(|a| super::climb::OVERHANG_TO_HANG.iter().position(|&id| id == a.id)) {
            self.pending_vstep = Some((1 - k, true));
            self.last_hand = k;
        }
        if let Some(m) = e.entry_move {
            // the game's reception for the jump that arrived (0xE07D00), or the pull-down's first stage
            self.mv = Some(m);
            self.sub_state = LedgeSubState::HandPlacement;
        } else {
            // the root moves onto the hang pose (reception/grasp anims in the game)
            self.moves.push(RootInterp::new(e.from_feet, Vec3::NAN, GRAB_TIME));
        }
    }
}

impl HumanLedgeData {
    /// A discrete move (grab, step, pull-up) is running.
    pub fn moving(&self) -> bool {
        !self.moves.is_empty() || self.mv.is_some()
    }
}

/// `HumanLedge__RePoseHandsFromBones` 0xDE1970 (RE/24 LEDGE-22): the hands onto the holds under the animated hand bones
/// (`bones`: the LeftHand / RightHand handles, Human +0x290 / +0x298, read through `bone_pose`). The non-steep edges
/// (`Guidance__IsNotSteep`) within a 1.5 m sphere at the root + 1.5 m whose wall (the lower of the edge's two normals,
/// flattened) faces the character (its dot with the facing < −0.785, compared as written, 0xDE1C3B); each hand goes onto
/// the nearest point of those edges to its own bone. A hand with no such edge keeps its hold.
pub fn hands_from_bones(bones: [Vec3; 2], root: Vec3, facing: Vec3, guidance: &GuidanceWorld) -> [Option<Vec3>; 2] {
    let centre = root + Vec3::Y * 1.5;
    let facing = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
    let edges: Vec<_> = guidance
        .edges
        .iter()
        .filter(|e| !crate::guidance::steep(e.p1 - e.p0) && e.closest_point(centre).distance(centre) <= 1.5)
        .filter(|e| {
            let n = if e.n0.y <= e.n1.y { e.n0 } else { e.n1 };
            Vec3::new(n.x, 0.0, n.z).normalize_or_zero().dot(facing) < -0.785_398_2
        })
        .collect();
    bones.map(|b| edges.iter().map(|e| e.closest_point(b)).min_by(|p, q| p.distance_squared(b).total_cmp(&q.distance_squared(b))))
}

fn hand_mid(d: &HumanLedgeData) -> Vec3 {
    (d.hand_l + d.hand_r) * 0.5
}

/// `TrySwitchHangType` 0xDE1060 with a corner variant (a3 1 near / 2 far): the corner row's point (row 3) for both
/// hands, the foot rays in the frame turned onto the corner's face; the switch clips by the move direction, the root
/// interpolated to the new hang turning to the new face. Refused on a closing step (+133) and while a step is pending.
fn corner_switch(d: &HumanLedgeData, rows: &ledge_moves::EdgeRows, right_dir: bool, feet: Vec3, facing: Vec3, collision: &CollisionWorld) -> Option<ledge_moves::LedgeMove> {
    if d.alt_flag || d.pending_vstep.is_some() {
        return None;
    }
    let h = rows[3]?;
    let nt = needs_switch(d.hang_type, h.point, h.wall_normal, collision)?;
    let mut mv = ledge_moves::switch_move(if right_dir { 3 } else { 2 }, nt == LedgeHangType::Wall, feet, h.point, h.point, h.wall_normal, collision);
    mv.facing_from = facing;
    Some(mv)
}

/// Both hands hold Pole edges (the swing bars; contact kinds +1792 / +1796 == 3, `SyncLimbContacts` 0xDD0390).
pub fn hands_on_pole(d: &HumanLedgeData, n: Vec3, guidance: &GuidanceWorld) -> bool {
    [d.hand_l, d.hand_r].iter().all(|h| guidance.on_edge(*h, n, 0.15).is_some_and(|hit| guidance.edges[hit.edge].subtype == crate::guidance::GuidanceSubType::Pole))
}

/// The ray part of `Guidance__FindWallSupport` 0x11718B0 (a wall support for a foot): from `centre` + `up`·z,
/// `half_depth` m out from the wall, a ray of 2·`half_depth` toward it, on layer 43. A hit on a body owned by a component
/// (flag 0x10000000, the same tag `ContactManager__GroundMaterial` 0x55A580 tests) is a support, which is all the port's
/// static collision; a hit on a FuzzyZone layer (24 / 26 / 25 / 33 / 41, `sub_E1C1F0`) aborts with no support (fuzzy
/// zones are not imported). A miss goes on to the Surface search (`wall_support`).
pub fn support_ray(centre: Vec3, n: Vec3, half_depth: f32, z: f32, collision: &CollisionWorld) -> bool {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let start = centre + Vec3::Y * z - facing * half_depth;
    collision.sphere_free_distance(start, facing, 0.01, 2.0 * half_depth) < 2.0 * half_depth - 1e-3
}

/// A `Guidance__FindWallSupport` 0x11718B0 box: half extents across the wall (a6), out of it (a7, also the ray's half
/// length) and up (a7 + 4), and the step between its z planes (a9).
#[derive(Clone, Copy, Debug)]
pub struct SupportBox {
    pub across: f32,
    pub depth: f32,
    pub height: f32,
    pub step: f32,
}

/// The air catch's wall-hang feet (`CheckAirCatch` 0xE0CDFF / 0xE0CEAC) and the jump reception's
/// (`Human__FindReceptionFeetHolds` 0xB15D1C / 0xB15DAD).
pub const SUPPORT_WIDE: SupportBox = SupportBox { across: 0.4, depth: 0.625, height: 0.2, step: 0.1 };
/// The wall-hang feet (`Human__FindWallHangFeetSupports` 0xB1626C / 0xB16305) and the climb's side grab
/// (`HumanClimb__TrySideLedgeGrab` 0xDF2541 / 0xDF2649).
pub const SUPPORT_FEET: SupportBox = SupportBox { across: 0.25, depth: 0.625, height: 0.15, step: 0.07 };

/// `Guidance__FindWallSupport` 0x11718B0: a foot's wall support at `origin` + (x across, z up) in the wall's frame
/// (x along into × up, the wall's normal `n`). First the ray (`support_ray`); on a miss the Surface search
/// (`GuidanceWorkspaceCastZPlanes`, type 6): the collision in the box (centred on `origin`, not the offset point) cut by
/// the planes at z, z ± step, … within its height (`CollisionWorld::surface_slices`), and any piece whose triangle faces
/// the character within 45° (its normal flattened, not renormalised, · into < −0.7071) is a support; the game returns the
/// point on the nearest piece (shortened 5 cm at each end, `Guidance__NearestOnShortenedSegment`) to the offset point.
pub fn wall_support(origin: Vec3, n: Vec3, x: f32, z: f32, b: SupportBox, collision: &CollisionWorld) -> bool {
    let into = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let across = into.cross(Vec3::Y);
    if support_ray(origin + across * x, n, b.depth, z, collision) {
        return true;
    }
    let mut heights = vec![z];
    let mut h = z + b.step;
    while h <= b.height {
        heights.push(h);
        h += b.step;
    }
    let mut h = z - b.step;
    while h >= -b.height {
        heights.push(h);
        h -= b.step;
    }
    collision
        .surface_slices(origin, across, into, Vec3::new(b.across, b.depth, b.height), &heights)
        .iter()
        .any(|&(_, _, sn)| into.dot(Vec3::new(sn.x, 0.0, sn.z)) < -std::f32::consts::FRAC_1_SQRT_2)
}

/// `Human__FindWallHangFeetSupports` 0xB16130: the wall-hang feet supports under a hold point `hand` (the free-hang drop
/// 0xDDF390, the corners 0xDD3BB0, the climb's ledge grab 0xDF0980): two `wall_support` searches 1.0 m below it and
/// 0.125 m into the wall, one 0.2 m to the left raised 0.1 m and one under the point lowered 0.1 m (`SUPPORT_FEET`);
/// either is enough (the other foot then shares it).
pub fn feet_supports(hand: Vec3, n: Vec3, collision: &CollisionWorld) -> bool {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let c = hand - Vec3::Y + facing * 0.125;
    wall_support(c, n, -0.2, 0.1, SUPPORT_FEET, collision) || wall_support(c, n, 0.0, -0.1, SUPPORT_FEET, collision)
}

/// `Human__FindReceptionFeetHolds` 0xB15C30: the jump arrival's foot holds under a hold point `hand`
/// (`HumanInAir__CheckJumpTargetArrival` 0xE07D00, generic branch 0xE093FC): two `wall_support` searches 1.0 m below it
/// and 0.15 m into the wall, one 0.15 m to the left raised 0.05 m and one 0.15 m to the right lowered 0.05 m
/// (`SUPPORT_WIDE`). Both feet must find the wall.
pub fn reception_feet(hand: Vec3, n: Vec3, collision: &CollisionWorld) -> bool {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let c = hand - Vec3::Y + facing * 0.15;
    wall_support(c, n, -0.15, 0.05, SUPPORT_WIDE, collision) && wall_support(c, n, 0.15, -0.05, SUPPORT_WIDE, collision)
}

/// Foot supports for a hang at `mid` (`sub_DE0A60` from TrySwitchHangType 0xDE1060): two rays of 1.2 m toward
/// the wall, starting 1 m below the hands, 0.5 m back from the wall and 0.1 m to either side (collision layer
/// 43). Returns (left foot hits, right foot hits).
pub fn feet_on_wall(mid: Vec3, n: Vec3, collision: &CollisionWorld) -> (bool, bool) {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let r = right_of(facing);
    let start = mid - Vec3::Y - facing * 0.5;
    let hit = |o: Vec3| collision.sphere_free_distance(o, facing, 0.01, 1.2) < 1.2 - 1e-3;
    (hit(start - r * 0.1), hit(start + r * 0.1))
}

/// A wall under the hands within leg length (the free hang becomes WallFree: legs held off the wall).
pub fn wall_below_hands(mid: Vec3, n: Vec3, collision: &CollisionWorld) -> bool {
    [0.6f32, 1.2, 1.8].iter().any(|&d| collision.point_inside(mid - n * 0.15 - Vec3::Y * d))
}

/// Hang type a hang at `mid` gets: wall if a foot finds support (the free → wall rule of 0xDE1060).
pub fn hang_type_at(mid: Vec3, n: Vec3, collision: &CollisionWorld) -> LedgeHangType {
    let (a, b) = feet_on_wall(mid, n, collision);
    if a || b {
        LedgeHangType::Wall
    } else {
        LedgeHangType::Free
    }
}

/// The hang type a step to the target `mid` needs, if it differs from `cur` (0xDE1060): free → wall when a foot finds
/// support, wall → free unless both feet do. The rays start at the target point (RE/24 LEDGE-9).
fn needs_switch(cur: LedgeHangType, mid: Vec3, n: Vec3, collision: &CollisionWorld) -> Option<LedgeHangType> {
    let (a, b) = feet_on_wall(mid, n, collision);
    match cur {
        LedgeHangType::Free if a || b => Some(LedgeHangType::Wall),
        LedgeHangType::Wall if !(a && b) => Some(LedgeHangType::Free),
        _ => None,
    }
}

/// Root (feet) for a hang (ComputeRootFromHandTargets 0xDD6730).
pub fn hang_root(hand_l: Vec3, hand_r: Vec3, n: Vec3, hang: LedgeHangType) -> Vec3 {
    let mid = (hand_l + hand_r) * 0.5;
    match hang {
        LedgeHangType::Wall => {
            Vec3::new(mid.x, hand_l.y.min(hand_r.y) - WALL_HANG_DROP, mid.z) + n * WALL_HANG_OUT
        }
        LedgeHangType::Free => {
            Vec3::new(mid.x, hand_l.y.max(hand_r.y) - FREE_HANG_DROP, mid.z) + n * FREE_HANG_OUT
        }
    }
}

/// Root for a hang, with the WallFree case (LedgeHangType 2): a free hang with a wall under the hands plays
/// `xx_h_hangwallfree_wait`, whose hands are 0.46 m in front of the root (fingertips 0.52 m, 2.41 m up), so its
/// root sits 0.5 m out from the wall and 2.4 m below the hands, as the game's band offsets for the jump into it
/// (+0.5·n, −2.4 m, 0xB21DA0). `hangfree_wait` has the hands right above the root (FREE_HANG_OUT).
/// The root for a hand step's new hand targets: its height from the hang (`hang_root_at`), its distance from the wall kept
/// from the root as it hangs now (`out`, measured from the old hands). The game changes how far out a hang sits only
/// through a hang-type switch (`TrySwitchHangType` 0xDE1060, its own actions), which the step paths try first; the
/// wall-under-the-hands test of `hang_root_at` would otherwise move a free hang 0.5 m out once a hand reaches a bar with
/// wall below it, and both hands fell short of their holds (bug-1791714696).
pub fn step_root(hand_l: Vec3, hand_r: Vec3, n: Vec3, hang: LedgeHangType, out: f32, collision: &CollisionWorld) -> Vec3 {
    let nf = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    let y = hang_root_at(hand_l, hand_r, n, hang, collision).y;
    Vec3::new(mid.x + nf.x * out, y, mid.z + nf.z * out)
}

/// How far out from its hands' line a root hangs (along the flat wall normal), for `step_root`.
pub fn root_out(feet: Vec3, hand_l: Vec3, hand_r: Vec3, n: Vec3) -> f32 {
    let nf = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let mid = (hand_l + hand_r) * 0.5;
    Vec3::new(feet.x - mid.x, 0.0, feet.z - mid.z).dot(nf)
}

pub fn hang_root_at(hand_l: Vec3, hand_r: Vec3, n: Vec3, hang: LedgeHangType, collision: &CollisionWorld) -> Vec3 {
    let mid = (hand_l + hand_r) * 0.5;
    if hang == LedgeHangType::Free && wall_below_hands(mid, n, collision) {
        Vec3::new(mid.x, hand_l.y.max(hand_r.y) - FREE_HANG_DROP, mid.z) + n * WALLFREE_HANG_OUT
    } else {
        hang_root(hand_l, hand_r, n, hang)
    }
}

/// Ledge stick quantization (QuantizeStickDirection 0xDD1920): |a| â‰¤ 45Â° up, beyond 135Â° down,
/// otherwise left/right. `a` is measured from the character facing (into the wall).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgeDir {
    Up,
    Down,
    Left,
    Right,
}

pub fn quantize(stick: Vec3, facing: Vec3) -> LedgeDir {
    let r = right_of(facing);
    let a = stick.dot(r).atan2(stick.dot(facing));
    let deg = a.to_degrees();
    if deg.abs() <= 45.0 {
        LedgeDir::Up
    } else if deg.abs() >= 135.0 {
        LedgeDir::Down
    } else if deg > 0.0 {
        LedgeDir::Right
    } else {
        LedgeDir::Left
    }
}

fn start_move(d: &mut HumanLedgeData, mv: LedgeMove, what: &'static str) {
    d.mv = Some(mv);
    d.step_seq += 1;
    d.sub_state = LedgeSubState::HandPlacement;
    d.blocked_up = false;
    d.last_action = what;
}

#[allow(clippy::too_many_arguments)]
pub fn update_ledge(
    time: Res<Time>,
    mut pad: ResMut<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    abilities: Option<Res<super::abilities::AbilitySet>>,
    skeleton: Option<Res<crate::bone_pose::PoseSkeleton>>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle, &mut LimbTargets, Option<&crate::bone_pose::BonePose>), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    let ab = super::abilities::of(abilities.as_deref());
    for (mut loco, mut body, mut data, mut limbs, pose) in &mut q {
        if loco.current != ActorContextId::Ledge {
            continue;
        }
        limbs.suspended = false;
        if loco.just_switched {
            loco.just_switched = false;
            limbs.feet = None;
            if loco.switched_this_frame {
                continue;
            }
        }
        // ---------------------------------------------------------------- SwingReception (SubState 8)
        if let Some(mut s) = data.ledge.swing {
            limbs.hands = Some((data.ledge.hand_l, data.ledge.hand_r));
            limbs.feet = None;
            body.velocity = Vec3::ZERO;
            body.heading = heading_of(s.fwd);
            let mut feet = body.feet;
            // the interpreter's ledge state (0xEECCF9–0xEED11F): +4397 = high profile and Legs held; the jump (event 7) with
            // the stick past the dead zone within 75° of the facing (1.3089969) and Legs pressed (RE/24 LEDGE-17)
            let held = pad.high_profile && pad.legs_held;
            let toward = pad.speed01 > 0.0 && pad.dir_snap.dot(s.fwd) > 1.308_997f32.cos();
            let jump = held && toward && pad.jump_buffered();
            if jump {
                pad.consume_jump();
            }
            let out = super::swing::update(&mut s, dt, held, jump, &mut feet, &collision, |p, f| super::targets::swing_jump_target(p, f, &guidance, &collision));
            body.feet = feet;
            match out {
                super::swing::SwingOut::Stay => data.ledge.swing = Some(s),
                super::swing::SwingOut::Hang => {
                    data.ledge.swing = None;
                    // the swing replaced the plain grab: the hang starts here
                    data.ledge.moves.clear();
                    data.ledge.after = None;
                    data.ledge.sub_state = LedgeSubState::Movement;
                    data.ledge.hang_type = LedgeHangType::Free;
                    data.ledge.hang_set = true;
                }
                super::swing::SwingOut::Jump(target) => {
                    data.ledge.swing = None;
                    let from = body.feet;
                    let dir = s.fwd;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(super::air::InAirEntry::SwingJump { from, target, dir }));
                }
            }
            continue;
        }
        // ---------------------------------------------------------------- HandPassOver (SubState 12)
        if let Some(mut p) = data.ledge.pass_over {
            limbs.hands = None;
            limbs.feet = None;
            body.velocity = Vec3::ZERO;
            body.heading = heading_of(p.fwd);
            let mut feet = body.feet;
            let out = super::passover::update(&mut p, dt, &mut feet, &guidance, &collision);
            body.feet = feet;
            match out {
                super::passover::PassOverOut::Stay => data.ledge.pass_over = Some(p),
                super::passover::PassOverOut::Ground => {
                    data.ledge.pass_over = None;
                    body.grounded = true;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
                    // entry mode 12: the roll ending (0xDCFA40, RE/24 GROUND-4)
                    data.ground.enter_roll_ending(super::anim_gate::word(&p.action));
                }
                super::passover::PassOverOut::PullDown { far, normal } => {
                    // pull-down type 4 (0xDE0220): Ledge SubState 11 from the far edge
                    data.ledge.pass_over = None;
                    let entry = match ledge_moves::pulldown_passover(far, normal, body.feet, p.hand, p.w, &guidance.ledge_report_depth(far, 2.5, true, crate::guidance::PULLDOWN_DEPTH, &collision), &collision) {
                        Ok(moves) => super::ground::pulldown_ledge_entry(moves, normal, body.feet),
                        Err(orient) => super::ground::pulldown_release_entry(orient, normal, body.feet),
                    };
                    switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(entry));
                }
                super::passover::PassOverOut::Fall(action) => {
                    data.ledge.pass_over = None;
                    let entry = super::air::InAirEntry::Fall { from: body.feet, velocity: Vec3::ZERO, origin: super::air::FallOrigin::Ground, speed_param: 0.0 };
                    switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
                    data.air.fall_action = Some(action);
                }
            }
            continue;
        }
        let d = &mut data.ledge;
        let n = d.normal;
        let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        body.heading = heading_of(facing);
        body.velocity = Vec3::ZERO;
        body.grounded = false;
        if !d.hang_set {
            // a free-hang catch keeps the free hang (sub_B15AD0), whatever lies below
            d.hang_type = if d.catch_action.is_some() && d.catch_free { LedgeHangType::Free } else { hang_type_at(hand_mid(d), n, &collision) };
            d.hang_set = true;
        }
        if let Some(long) = d.catch.take() {
            let id = d.catch_action.unwrap_or(if d.hang_type == LedgeHangType::Wall { ACT_CATCH_WALL[long as usize] } else { ACT_CATCH_FREE[long as usize] });
            let t = match d.catch_action {
                Some(id) => action_time(id),
                None => catch_time(d.hang_type == LedgeHangType::Wall, long),
            };
            if let Some(m) = d.moves.first_mut() {
                // the catch lasts its action; the root arrives after item 0 + 0.2 s (0xE0DB5C), not over the whole action
                // (it slid onto the hold for a second, bug-1791711526)
                m.duration = t;
                m.reach = catch_root_time(id).min(t);
            }
        }
        // the limb IK runs only in some sub-states (`sub_DCD910` from `HumanLedge__Update` 0xDE3EC8): elsewhere the
        // animation alone shows, released at once (RE/24 ANIM-10)
        let playing = d.mv.and_then(|m| m.current()).map(|(b, _)| b.id).or(d.catch_action);
        let ik_runs = ledge_ik_runs(d.sub_state, playing);
        limbs.suspended = !ik_runs;
        limbs.hands = ik_runs.then_some((d.hand_l, d.hand_r));
        limbs.normal = n;
        // PORT: the travel time of a clip without contact tags (tagged clips use their window; RE/24 CLIMB-27)
        limbs.transit = d.moves.first().map(|m| m.duration).unwrap_or(SHIMMY_OPEN_TIME) * 0.8;
        // the feet follow the hang clip (braced against the wall, or dangling); only the hands are pinned
        limbs.feet = None;

        // ---------------------------------------------------------------- corner / ledge jump / hop
        if let Some(mut mv) = d.mv {
            let (p, done) = mv.advance(dt);
            body.feet = p;
            body.heading = heading_of(mv.facing());
            limbs.hands = None;
            // the stand-up's last item plays in NarrowObject: at the hangknee item's release `Pullup_Tick` 0xDE2EE0 calls
            // `HumanLedge__SwitchToNarrowObject_Enter` 0xDD2A50 (`hangknee_tr_freestep_entry`, SubState 6, +0xF0 = the root,
            // +0x100 = −the facing); the step-up's reception 0x012B33F1 likewise (0xE09752). The free-step stay
            // (RE/05 §2.15) then hands over to the Ground. PORT: the root keeps the move's path onto the top over the rest
            // of the item (the game's follows the clip; the pull-up's root path is RE/25 #47)
            if mv.end_stand {
                if let Some((b, ph)) = mv.current().filter(|(b, _)| ledge_moves::FREE_STEP_STAY.contains(&b.id)) {
                    let facing = mv.facing();
                    let e = super::free_step::FreeStepEntry {
                        action: b,
                        t: ph * b.duration(),
                        from: body.feet,
                        to: mv.to,
                        root_time: (mv.duration() - mv.t).max(0.0),
                        follow_clip: false,
                        point: body.feet,
                        normal: -Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero(),
                        small_box: false,
                        heading: body.heading,
                    };
                    d.mv = None;
                    d.queue.clear();
                    limbs.hands = None;
                    limbs.feet = None;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToFreeStep(e));
                    continue;
                }
            }
            if done && !d.queue.is_empty() {
                // next queued stage (pull-down: orientation → descent → reception)
                let mut next = d.queue.remove(0);
                next.from = body.feet;
                d.mv = Some(next);
                continue;
            }
            if done {
                if let Some(e) = d.then_ladder.take() {
                    // pending flag 1860&0x10 → Ladder context (0xDD8800 path)
                    d.mv = None;
                    limbs.hands = None;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToLadder(e));
                    continue;
                }
                if let Some(mut e) = d.then_climb.take() {
                    // landing category 0 (0xDDFC30 → 0xDD8CF0): the end item plays in the Climb context
                    d.mv = None;
                    e.from_feet = body.feet;
                    limbs.hands = None;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(e));
                    continue;
                }
            }
            if done && mv.kind == (ledge_moves::MoveKind::PullDown { stage: 4 }) {
                // ReleaseToInAir (0xDDFCF0 → 0xDDA100 → the let-go 0xDD7810, hang type 0: origin HangWall, the
                // released hold 1.1 m above the root)
                d.mv = None;
                limbs.hands = None;
                limbs.feet = None;
                let hold = body.feet + Vec3::Y * 1.1;
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::Fall { from: body.feet, velocity: Vec3::ZERO, origin: FallOrigin::HangWall, speed_param: 0.0 }));
                data.air.release_hold = Some(hold);
                continue;
            }
            if done && mv.kind == ledge_moves::MoveKind::Rebound {
                // `HumanLedge__ReboundEndJump` 0xDD0A40: the rebound done → face away from the wall and jump (`sub_B27180`
                // kind 2, the `xx_h_rebound_*_to_air` takeoff, RE/24 AIR-5), presentation event 67. PORT: the left foot
                // leads (the game reads the playing item's word).
                d.mv = None;
                limbs.hands = None;
                limbs.feet = None;
                body.heading = super::heading_of(mv.normal);
                if let Some(entry) = d.jump_off.take() {
                    switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry.starting_at(body.feet)));
                }
                continue;
            }
            if done && mv.end_stand {
                // a stand-up without its free-step item (not loaded): Ground
                d.mv = None;
                limbs.hands = None;
                limbs.feet = None;
                body.grounded = true;
                switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
                continue;
            }
            if done {
                d.hand_l = mv.hand_l;
                d.hand_r = mv.hand_r;
                d.normal = mv.normal;
                d.hang_type = if mv.end_free {
                    LedgeHangType::Free
                } else if mv.end_wall {
                    LedgeHangType::Wall
                } else {
                    hang_type_at(hand_mid(d), mv.normal, &collision)
                };
                d.hang_set = true;
                d.mv = None;
                d.sub_state = LedgeSubState::Movement;
                // the Entry state's end (`HumanLedge__EntryEnd_PlayIdle` 0xDE1FE0, RE/24 LEDGE-22): the entries in the
                // Entry state (+76 = 0: walling's hangs, the climb's jump up, the swing back's second part) re-pose the
                // hands from the hand bones, unless `step1_to_hangwall` plays. The bones are last frame's pose at the root
                // where it stands now (`bone_pose`); without the install the move's own hand points stay (PORT).
                // `HumanLedge__Pullup_Tick` 0xDE2EE0 re-poses the same way when `hangwallfree_tr_hangwall` is released
                // into the wall hang (its pass-over twins 0x0109BB58 / 59 follow walling pass-overs, which never happen)
                let last = mv.seq.iter().flatten().last().map(|a| a.id);
                let entry_end = std::mem::take(&mut d.repose_at_entry_end) || mv.kind == ledge_moves::MoveKind::SwingBack || last == Some(ledge_moves::WALLFREE_TO_WALL);
                if entry_end && last != Some(super::walling::STEP1_TO_HANGWALL) {
                    use crate::bone_pose::{LEFT_HAND, RIGHT_HAND};
                    let bones = skeleton.as_deref().zip(pose).and_then(|(s, p)| Some([p.world_pos(s, LEFT_HAND, &body)?, p.world_pos(s, RIGHT_HAND, &body)?]));
                    if let Some(bones) = bones {
                        let [l, r] = hands_from_bones(bones, body.feet, body.forward(), &guidance);
                        d.hand_l = l.unwrap_or(d.hand_l);
                        d.hand_r = r.unwrap_or(d.hand_r);
                        // `LimbIK__ReleaseAll`, then the contacts at weight 1 (0xDE19B8, 0xDE1D80)
                        limbs.contact_set = limbs.contact_set.wrapping_add(1);
                    }
                }
                if mv.kind == ledge_moves::MoveKind::SwingBack {
                    d.swing_strength = Some((mv.seq[1].map(|b| b.w[2] + b.w[3]).unwrap_or(0.0), d.step_seq));
                }
                // the free idle after a swing plays the graph's swing first (`ACT_HANGFREE_SWINGBACK`)
                if d.hang_type != LedgeHangType::Wall {
                    if let Some(s) = d.swing_idle() {
                        d.swing_lock = swing_lock_time(s);
                    }
                }
                // Every hang settle sets +133 from the hand targets (`HumanLedge__SetAlternationFromHandSpacing` 0xDCBCD0,
                // called by OnEnter, the entry, the receptions, the grasp, the ledge jump, the swing stops and the free-hang
                // drop): a hang whose hands end apart closes the trailing hand first (RE/24 LEDGE-14; live RE/25 #36: after
                // the pull-down, hands 0.51 m apart, the first shimmy step is `strafe_*_close`). The switch does not touch
                // it (LEDGE-9).
                if let ledge_moves::MoveKind::SideJump { .. } = mv.kind {
                    d.alt_flag = alternation_from_spacing(mv.hand_l, mv.hand_r);
                } else if !matches!(mv.kind, ledge_moves::MoveKind::SwitchHang { .. }) {
                    d.alt_flag = alternation_from_spacing(d.hand_l, d.hand_r);
                }
            } else {
                d.mv = Some(mv);
            }
            continue;
        }

        // ---------------------------------------------------------------- lost ledge
        // `HasLostLedge` 0xDD20D0, every frame before the state dispatch (`HumanLedge__Update` 0xDE3E86), steps included:
        // per hand a `Guidance__ProbeBox` at the hand, half 0.1 m, facing the wall, the wall within 90°, mask 50
        // (`sub_DCE0D0`, on the sphere report r 0.25 + half the hand spacing, which holds that box) (RE/24 LEDGE-19).
        // PORT: not during the `mv` moves (corners, ledge jumps, receptions; the game skips only the ledge jumps,
        // 1861 & 4): their hand records move at the move's end in the port
        let lost = |p: Vec3| guidance.probe_facing(p, facing, Vec3::splat(0.1), 0.0, std::f32::consts::FRAC_PI_2, crate::guidance::MASK_CLIMB_HOLDS).is_none();
        if lost(d.hand_l) || lost(d.hand_r) {
            let origin = if d.hang_type == LedgeHangType::Wall { FallOrigin::HangWall } else { FallOrigin::HangFree };
            let hold = hand_mid(d);
            limbs.hands = None;
            limbs.feet = None;
            d.moves.clear();
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::Fall { from: body.feet, velocity: Vec3::ZERO, origin, speed_param: 0.0 }));
            data.air.release_hold = Some(hold);
            continue;
        }

        // ---------------------------------------------------------------- running move
        let mut move_done = false;
        if let Some(m) = d.moves.first_mut() {
            if m.to.is_nan() {
                // deferred target (entry): the hang root for the current hands
                m.to = hang_root_at(d.hand_l, d.hand_r, n, d.hang_type, &collision);
            }
            let (p, _) = m.advance(dt);
            body.feet = p;
            if m.done() {
                d.moves.remove(0);
            }
            // event 4 (let go) is accepted mid-step: its guard `sub_DCCDD0` 0xDCCDD0 refuses only a locked playing item, and
            // the shimmy / vertical step items are not locked (word 0x0FC0); the catches' are (RE/24 LEDGE-26)
            let hand_step = d.last_action.starts_with("shimmy ") || d.last_action.starts_with("hand step ");
            if !d.moves.is_empty() {
                if !(hand_step && pad.hand_buffered()) {
                    continue;
                }
            } else {
                move_done = true;
            }
        }
        if move_done {
            match d.after.take() {
                Some(After::StandOnTop) => {
                    limbs.hands = None;
                    limbs.feet = None;
                    body.grounded = true;
                    // game: ends in NarrowObject (standing on the ledge edge, not ported yet) â†’ Ground
                    switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
                    continue;
                }
                Some(After::SecondHand(i, up)) => {
                    // the first half ends in Movement with the step pending (+1848); `PlayIdleOrBlocked` plays nothing
                    // while it is set (0xDD3A2B), so the half-step pose holds until the stick asks for the second hand
                    d.pending_vstep = Some((i, up));
                }
                _ => {}
            }
            d.sub_state = LedgeSubState::Movement;
        }

        // ---------------------------------------------------------------- checks (Movement state)
        // the interpreter's latched stick in the character's frame (`GoAssassinActionInterpreter__LedgeState` 0xEEC356:
        // the angle of +0x1310 from stick-up; RB held reads the unmirrored +0x1300, 0xEEC256; RE/24 INPUT-4)
        let dir = if pad.speed01 > 0.0 { Some(quantize(pad.actor_dir(facing, pad.buttons[13]), facing)) } else { None };
        // the ledge report (`HumanLedge__BuildLedgeReport` 0xDDC620 → HumanData +116): ladder zones carved, steep edges
        // removed; ProbeLateral, FindCornerEdges, FindVerticalRows, the side jumps and TryJumpUpToClimb read it
        // (RE/24 GUIDE-2). Its largest box reaches 3 m sideways from the hands.
        let report = guidance.ledge_report_depth(hand_mid(d), 3.5, true, crate::guidance::LEDGE_DEPTH, &collision);
        // the interpreter's ledge state (`GoAssassinActionInterpreter__LedgeState` 0xEEBFD0, RE/03 7.12): the
        // empty-hand buffer lets go (event 4, 0xDDB450 -> LetGoToInAir) ...
        if pad.hand_buffered() {
            pad.consume_hand();
            limbs.hands = None;
            limbs.feet = None;
            let origin = if d.hang_type == LedgeHangType::Wall { FallOrigin::HangWall } else { FallOrigin::HangFree };
            let hold = hand_mid(d);
            // with the Leap of Faith ability, a haystack below (IHuman vt84, `hay::LET_GO`, along the facing away from
            // the wall) turns the let-go into the dive (0xEEC580–0xEEC63B, RE/24 CENSUS-2)
            let away = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
            let dive = ab.allows(super::abilities::Ability::LeapOfFaith)
                .then(|| super::hay::search(body.feet, away, away, &super::hay::LET_GO, &guidance.haystacks, &collision))
                .flatten();
            let entry = match dive {
                Some(s) => {
                    let target = super::targets::JumpTarget { position: super::hay::top_of(&s), type_flags: super::jump_blend::TARGET_HAYSTACK, hang: None, straight: None, pass: None, ladder: None, support: None };
                    InAirEntry::LetGoDive { from: body.feet, target, origin }
                }
                None => InAirEntry::Fall { from: body.feet, velocity: Vec3::ZERO, origin, speed_param: 0.0 },
            };
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            data.air.release_hold = Some(hold);
            continue;
        }
        // ... and high profile with the Legs buffer jumps off (event 2): away from the wall, or along the stick. Event 2's
        // guard (0xDCCE10) refuses a free hang; its action (0xDD2CA0) plays the rebound turn in the Ledge context (SubState
        // 5, state 10) with the target stored, and the jump starts when it ends (0xDD0A40, RE/24 LEDGE-16)
        if pad.high_profile && pad.jump_buffered() && d.hang_type == LedgeHangType::Wall {
            if let Some(jd) = super::targets::jump_off_wall_dir((pad.speed01 > 0.0).then_some(pad.dir_snap), n) {
                pad.consume_jump();
                d.jump_off = Some(super::targets::jump_off_wall_entry(body.feet, jd, &guidance, &collision));
                start_move(d, ledge_moves::rebound_move(body.feet, n, jd, d.hand_l, d.hand_r), "rebound");
                d.sub_state = LedgeSubState::ReboundTransition;
                continue;
            }
        }
        // event 5 (+4397 = high profile and Legs held, CanHandleEvent(5) = guard 0xDD3750): a still free hang with both hands
        // on Pole edges, no pending step, and the box 0.5 m ahead / 1.0 m up (half extents 0.3 / 0.5 / 1.0) free → start
        // swinging (0xDD2940, RE/24 LEDGE-17 / CENSUS-3)
        if pad.high_profile && pad.legs_held && d.hang_type == LedgeHangType::Free && d.pending_vstep.is_none() && hands_on_pole(d, n, &guidance) {
            let centre = body.feet + facing * 0.5 + Vec3::Y * 1.0;
            if collision.obb_free(centre, [right_of(facing), facing, Vec3::Y], Vec3::new(0.3, 0.5, 1.0)) {
                if let Some(s) = super::swing::start(body.feet, facing, hand_mid(d), d.swing.map(|s| s.seq).unwrap_or(0)) {
                    d.swing = Some(s);
                    d.last_action = "swing";
                    continue;
                }
            }
        }
        // `Movement_ChooseAction` 0xDE29E0 returns while the playing item is locked (0xDE2B1C): the swing into the free idle
        if d.swing_lock > 0.0 {
            d.swing_lock -= dt;
            // the idle request stands: the graph plays the swing in front of it
            d.last_action = "idle";
            continue;
        }
        let Some(dir) = dir else {
            d.blocked_up = false;
            if d.pending_vstep.is_none() {
                d.last_action = "idle";
            }
            continue;
        };
        let right = right_of(facing);
        // ContinuePendingVerticalStep 0xDCF3C0: with a step pending a lateral stick becomes the pending direction; the
        // second step then runs the vertical path again (`FindVerticalRows` 0xDD8F70) with the climb transition, the hang
        // switch, the jump up and the wall jump up refusing while it is pending. No hit: the first hand's own point
        // (0xDD94A6). `PickVerticalHandAndAnim` 0xDCB3E0 moves the pending hand with the clip of the stick's direction,
        // so a reversed stick steps that hand the other way (RE/24 LEDGE-4).
        if let Some((second, pending_up)) = d.pending_vstep {
            let up = match dir {
                LedgeDir::Left | LedgeDir::Right => pending_up,
                LedgeDir::Up => true,
                LedgeDir::Down => false,
            };
            let rows = ledge_moves::vertical_rows(d.hand_l, d.hand_r, n, up, &report);
            let first_p = if second == 0 { d.hand_r } else { d.hand_l };
            // the step row's point nearest the hands' midpoint line, else the first hand's own point (0xDD94A6): the hands
            // end together (RE/24 LEDGE-5)
            let target = rows.step.map_or(first_p, |h| h.point);
            let out = root_out(body.feet, d.hand_l, d.hand_r, n);
            if second == 0 { d.hand_l = target } else { d.hand_r = target }
            d.pending_vstep = None;
            d.last_hand = second;
            d.alt_flag = false;
            let to = step_root(d.hand_l, d.hand_r, n, d.hang_type, out, &collision);
            d.vstep = Some((second == 1, true));
            // the step lasts its action (`StartHandStep` 0xDDE0C0 interpolates over `sub_501CB0`(0), RE/24 LEDGE-21)
            let wi = (d.hang_type != LedgeHangType::Wall) as usize;
            d.moves.push(RootInterp::new(body.feet, to, action_time(ACT_VSTEP[wi][vstep_index(!up, true, second == 1)])));
            d.step_seq += 1;
            d.after = Some(After::Hang);
            d.sub_state = LedgeSubState::HandPlacement;
            d.last_action = if up { "hand step up" } else { "hand step down" };
            continue;
        }
        match dir {
            LedgeDir::Up | LedgeDir::Down => {
                let sign = if dir == LedgeDir::Up { 1.0 } else { -1.0 };
                // sub_DD8F70: the feet, hands and hand-step rows for this direction
                let rows = ledge_moves::vertical_rows(d.hand_l, d.hand_r, n, dir == LedgeDir::Up, &report);
                // TryTransitionToClimb 0xDD46A0: both rows → the climb pose of those holds (both hands on the hands row,
                // both feet on the feet row). PORT: its body boxes (0xB2DF90) are not tested.
                if let (Some(feet_row), Some(hands_row)) = (rows.feet, rows.hands) {
                    let entry = ClimbEntry {
                        entry_type: ClimbEntryType::FromLedge,
                        pose_2m: None, end_action: None,
                        hand_l: hands_row.point,
                        hand_r: hands_row.point,
                        foot_l: feet_row.point,
                        foot_r: feet_row.point,
                        normal: hands_row.wall_normal,
                        from_feet: body.feet,
                        foot_right: false,
                        action: Some(super::climb::LEDGE_TO_CLIMB[(dir == LedgeDir::Down) as usize][(d.hang_type != LedgeHangType::Wall) as usize]),
                    };
                    switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(entry));
                    continue;
                }
                // TrySwitchHangType 0xDE1060 (vertical) comes right after the climb transition, before the free-hang drop
                // and the hand step (0xDE2D77..0xDE2E12, RE/24 LEDGE-10): both hand targets are the step row point, the
                // foot rays start there (LEDGE-9). PORT: the game decides with the step clip's end-frame moving foot (+ a
                // ±0.15 m jitter, `QuickWallTestForStep` 0xDD1DC0); the port uses the switch's own rays.
                if let Some(h) = rows.step {
                    if let Some(nt) = needs_switch(d.hang_type, h.point, n, &collision) {
                        let mv = ledge_moves::switch_move(if sign > 0.0 { 0 } else { 1 }, nt == LedgeHangType::Wall, body.feet, h.point, h.point, n, &collision);
                        start_move(d, mv, if nt == LedgeHangType::Wall { "switch to wall hang" } else { "switch to free hang" });
                        continue;
                    }
                }
                // TryFreeHangDropToClimb 0xDDF390 (free hang, down): drop onto holds below and ahead
                if dir == LedgeDir::Down {
                    if let Some((mv, e)) = ledge_moves::try_free_hang_drop(d.hand_l, d.hand_r, n, body.feet, d.hang_type, &guidance, &collision) {
                        start_move(d, mv, if e.is_some() { "drop to climb" } else { "drop to wall hang" });
                        d.then_climb = e;
                        continue;
                    }
                }
                // StartHandStep 0xDDE0C0 (vertical): the hand step onto the step row's edge (0.1–0.8 m along the step). The
                // hand not moved last moves first (`PickVerticalHandAndAnim` 0xDCB3E0: +1844 == 1 → left), onto the row's
                // point nearest the hands' midpoint line (`FindVerticalRows` x_ref 0); the other follows with its own search
                // (ContinuePendingVerticalStep 0xDCF3C0). A vertical step clears +133 (0xDDE24D) (RE/24 LEDGE-5).
                // PORT: the target's hang body box is not tested.
                if let Some(h) = rows.step {
                    // first hand moves now, the second one next (pending vertical step)
                    let first_left = d.last_hand == 1;
                    d.alt_flag = false;
                    d.last_hand = (!first_left) as usize;
                    let out = root_out(body.feet, d.hand_l, d.hand_r, n);
                    if first_left { d.hand_l = h.point } else { d.hand_r = h.point }
                    // the root from the two hand targets as they stand after the first hand (`ComputeRootFromHandTargets`
                    // 0xDD6730): a free hang hangs from the higher hand (max − 2.4), so a step up lifts the body the
                    // whole step and the reaching hand gets to its hold; a wall hang's vertical step reads the lower hand
                    // (min − 1.1) and waits for the second. The halfway root left the reaching arm 0.4 m short of its hold
                    // (bug-1791711547). The distance from the wall stays (`step_root`, bug-1791714696)
                    let mid_root = step_root(d.hand_l, d.hand_r, n, d.hang_type, out, &collision);
                    d.vstep = Some((first_left, false));
                    let wi = (d.hang_type != LedgeHangType::Wall) as usize;
                    let first_time = action_time(ACT_VSTEP[wi][vstep_index(sign < 0.0, false, first_left)]);
                    d.moves.push(RootInterp::new(body.feet, mid_root, first_time));
                    d.step_seq += 1;
                    d.after = Some(if first_left { After::SecondHand(1, sign > 0.0) } else { After::SecondHand(0, sign > 0.0) });
                    d.sub_state = LedgeSubState::HandPlacement;
                    d.blocked_up = false;
                    d.last_action = if sign > 0.0 { "hand step up" } else { "hand step down" };
                    continue;
                }
                if dir == LedgeDir::Up {
                    // TryJumpUpOrSideJump 0xDE05E0 (up) → TryJumpUpToClimb 0xDD5E10: the jump up to climb holds.
                    // (The game's +1848 pending-step check is implicit: the decision only runs between moves.)
                    if let Some((mv, e)) = ledge_moves::try_jump_up_to_climb(d.hand_l, d.hand_r, n, body.feet, d.hang_type, &report, &collision) {
                        start_move(d, mv, "jump up to climb");
                        d.then_climb = Some(e);
                        continue;
                    }
                    // TryWallJumpUp 0xDD62A0: the swing back to an overhang's front edge behind (wall hang).
                    if let Some(mv) = ledge_moves::try_swing_back(d.hand_l, d.hand_r, n, body.feet, d.hang_type, &guidance, &collision) {
                        start_move(d, mv, "swing back");
                        continue;
                    }
                    // the pull-up (event 0) only when nothing else is possible up: `HumanLedge__CanPullup` 0xDE2270 needs
                    // +1862, which `Movement_ChooseAction` clears first (0xDE2A2C) and sets only when the climb transition,
                    // the hang switch, the free-hang drop, the hand step, the jump up and the wall jump up all failed (0xDE2E12;
                    // RE/24 LEDGE-2). PORT: the game sends it from the next frame's interpreter pass; the port pulls up at once.
                    d.blocked_up = true;
                    let mid = hand_mid(d);
                    let top = Vec3::new(mid.x, mid.y, mid.z) - n * PULLUP_IN;
                    let standable = collision
                        .ground_height(top + Vec3::Y * 0.05, 0.3)
                        .filter(|h| (h - mid.y).abs() < 0.25)
                        .is_some();
                    if standable && collision.capsule_fits(Vec3::new(top.x, mid.y, top.z)) {
                        // root: onto the top, 0.5 m in (Pullup_Start 0xDDBE80), along the pull-up clips' own
                        // displacement (up, then in over the lip) plus a correction onto that point
                        let top_feet = Vec3::new(top.x, mid.y, top.z);
                        // HumanLedge__PullupNeedsOneHand 0xDD1120: from the hands' midpoint (higher hand) 0.15 m in, the
                        // drop beyond the top 0.35 m to either side (Human__MeasureDropBeyondEdge 0xB19620: offset 0.35,
                        // search 0.5 down); more than 0.2 m on a side → the one-hand pull-up. PORT: no solid within 0.2 m
                        // below that point (point tests; the game sweeps a tilted capsule).
                        let p = Vec3::new(mid.x, d.hand_l.y.max(d.hand_r.y), mid.z) - n * 0.15;
                        let drops = |side: Vec3| {
                            let q = p + side * 0.35;
                            ![0.05f32, 0.1, 0.15, 0.2].iter().any(|dy| collision.point_inside(q - Vec3::Y * *dy))
                        };
                        let one_hand = drops(right) || drops(-right);
                        // the wall-free hang (Free with a wall below, LedgeHangType 2) does not pull up: Pullup_Start
                        // 0xDDBE80 switches it to the wall hang first (`hangwallfree_tr_hangwall`)
                        if d.hang_type == LedgeHangType::Free && wall_below_hands(mid, n, &collision) {
                            start_move(d, ledge_moves::wallfree_to_wall_move(body.feet, d.hand_l, d.hand_r, n), "wall-free to wall hang");
                            continue;
                        }
                        let (mv, rest) = ledge_moves::pullup_move(d.hang_type, one_hand, body.feet, d.hand_l, d.hand_r, n, top_feet);
                        start_move(d, mv, if one_hand { "pull-up (one hand)" } else { "pull-up" });
                        d.queue = rest.into_iter().collect();
                        d.sub_state = LedgeSubState::Pullup;
                        limbs.suspended = !ledge_ik_runs(d.sub_state, d.mv.and_then(|m| m.current()).map(|(b, _)| b.id));
                        limbs.hands = None;
                        continue;
                    }
                    d.last_action = "blocked up";
                } else {
                    d.last_action = "blocked down";
                }
            }
            LedgeDir::Left | LedgeDir::Right => {
                d.blocked_up = false;
                let right_dir = dir == LedgeDir::Right;
                let dv = if right_dir { right } else { -right };
                // lead hand = hand on the move side; roles swap when the alternation flag is set
                let move_right_hand = right_dir != d.alt_flag;
                let (mover, other) = if move_right_hand { (d.hand_r, d.hand_l) } else { (d.hand_l, d.hand_r) };
                // Movement_ChooseAction 0xDE29E0, left / right, with the preference +1856 = 0 (the ctor's value; nothing
                // else sets it): ProbeLateral 0xDD9640 first (the rows and the shimmy target)
                let (rows, target) = ledge_moves::probe_lateral(mover, other, n, dv, d.alt_flag, d.hang_type, &report, &collision);
                // TrySideMoveToLadder 0xDD4CD0 (`sub_DD4CD0(dir, 8, 0)`): a ladder at the side, skipped during a closing
                // step (+133 set, RE/03 §7.6e; live RE/25: the shimmy closes the trailing hand before the ladder move)
                if let Some((mv, e)) = ledge_moves::try_side_to_ladder(right_dir, n, body.feet, d.hang_type, &guidance).filter(|_| !d.alt_flag) {
                    start_move(d, mv, "side to ladder");
                    d.then_ladder = Some(e);
                    continue;
                }
                // TrySideMoveToClimbHolds 0xDD48B0 on a row source, the bands in order (none during a closing step)
                let (alt, hang_type, feet) = (d.alt_flag, d.hang_type, body.feet);
                let climb_holds = |rows: &ledge_moves::EdgeRows, bands: &[usize]| {
                    if alt { None } else { bands.iter().find_map(|&b| ledge_moves::side_move_to_climb_holds(rows, b, right_dir, hang_type, feet)) }
                };
                // climb holds down, then level (16 / 8 on ProbeLateral's rows)
                if let Some(e) = climb_holds(&rows, &[2, 1]) {
                    switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(e));
                    continue;
                }
                // StartHandStep 0xDDE0C0: the shimmy step onto row 3's target
                if let Some(t) = target {
                    // `TrySwitchHangType` 0xDE1060 (lateral): the lead hand's ProbeLateral target, refused on a closing step;
                    // both hands go onto it and the foot rays start there; +133 is not touched (RE/24 LEDGE-9). PORT: the
                    // decision uses the switch's rays, not the step clip's end-frame foot (0xDD1DC0).
                    let switch = if d.alt_flag { None } else { needs_switch(d.hang_type, t, n, &collision) };
                    if let Some(nt) = switch {
                        let mv = ledge_moves::switch_move(if right_dir { 3 } else { 2 }, nt == LedgeHangType::Wall, body.feet, t, t, n, &collision);
                        start_move(d, mv, if nt == LedgeHangType::Wall { "switch to wall hang" } else { "switch to free hang" });
                        continue;
                    }
                    if move_right_hand { d.hand_r = t } else { d.hand_l = t }
                    d.last_hand = move_right_hand as usize;
                    d.alt_flag = !d.alt_flag;
                    let to = hang_root_at(d.hand_l, d.hand_r, n, d.hang_type, &collision);
                    // after the toggle: alt_flag set = this was the lead hand's reach (open), clear = closing step; the step
                    // lasts its action (RE/24 LEDGE-21: the free hang's closing step is 0.533 s, not the wall's 0.6 s)
                    let wi = (d.hang_type != LedgeHangType::Wall) as usize;
                    d.moves.push(RootInterp::new(body.feet, to, action_time(ACT_SHIMMY[wi][shimmy_index(right_dir, d.alt_flag)])));
                    d.step_seq += 1;
                    d.after = Some(After::Hang);
                    d.sub_state = LedgeSubState::HandPlacement;
                    d.last_action = if right_dir { "shimmy right" } else { "shimmy left" };
                    continue;
                }
                // climb holds up (4)
                if let Some(e) = climb_holds(&rows, &[0]) {
                    switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(e));
                    continue;
                }
                // the near corner: FindCornerEdges(dir, 1) 0xDD0600 → climb holds 16 / 8 / 4 round it, TryTurnCorner(dir,
                // 1) 0xDD3BB0
                let near = ledge_moves::corner_rows(true, right_dir, d.hand_l, d.hand_r, n, &report);
                if let Some(e) = climb_holds(&near, &[2, 1, 0]) {
                    switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(e));
                    continue;
                }
                if let Some(mv) = ledge_moves::try_corner(true, right_dir, d.hand_l, d.hand_r, n, body.feet, d.hang_type, &report, &collision) {
                    start_move(d, mv, "corner (inner)");
                    continue;
                }
                // TrySwitchHangType(dir, 1) 0xDE1060: the near corner's row point, the foot rays in the frame turned onto
                // the corner's face (0xDE12CE), refused on a closing step (RE/24 LEDGE-10)
                if let Some(mv) = corner_switch(d, &near, right_dir, body.feet, facing, &collision) {
                    start_move(d, mv, if mv.end_wall { "switch to wall hang (corner)" } else { "switch to free hang (corner)" });
                    continue;
                }
                // TryJumpUpOrSideJump 0xDE05E0: a ladder (0xDD55F0), then a ledge (0xDDD490)
                if let Some((mv, e)) = ledge_moves::try_side_jump_to_ladder(right_dir, n, body.feet, d.hang_type, &report, &collision) {
                    start_move(d, mv, "side jump to ladder");
                    d.then_ladder = Some(e);
                    continue;
                }
                if let Some((mv, climb)) = ledge_moves::try_side_jump(right_dir, d.hand_l, d.hand_r, n, body.feet, d.hang_type, &report, &collision) {
                    if climb.is_some() {
                        // onto climb holds (type 0): the Climb context takes over at the end
                        start_move(d, mv, "side jump to climb");
                        d.then_climb = climb;
                    } else {
                        // a long jump into a free hang lands on one hand, then the second hand reaches (state 17)
                        let (mv, tail) = ledge_moves::second_hand_grab(mv, &guidance, &collision);
                        start_move(d, mv, "side jump");
                        d.queue = tail.into_iter().collect();
                    }
                    continue;
                }
                // the far corner: FindCornerEdges(dir, 0) → climb holds round it, TryTurnCorner(dir, 0)
                let far = ledge_moves::corner_rows(false, right_dir, d.hand_l, d.hand_r, n, &report);
                if let Some(e) = climb_holds(&far, &[2, 1, 0]) {
                    switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(e));
                    continue;
                }
                if let Some(mv) = ledge_moves::try_corner(false, right_dir, d.hand_l, d.hand_r, n, body.feet, d.hang_type, &report, &collision) {
                    start_move(d, mv, "corner (outer)");
                    continue;
                }
                // TrySwitchHangType(dir, 2): the far corner's row point (0xDE1347, RE/24 LEDGE-10)
                if let Some(mv) = corner_switch(d, &far, right_dir, body.feet, facing, &collision) {
                    start_move(d, mv, if mv.end_wall { "switch to wall hang (corner)" } else { "switch to free hang (corner)" });
                    continue;
                }
                d.blocked_right = right_dir;
                d.last_action = if target.is_none() && rows.iter().all(|r| r.is_none()) { "shimmy blocked (obstacle / inner corner)" } else { "end of ledge" };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::Aabb3;
    use crate::guidance::GuidanceHit;

    /// The swing into the free idle holds the hang for its first item, min / max blended by the swing strength
    /// (`xx_h_hangfree_swingback_min_a` 0.533 s, `max_a` 0.733 s); live, wall 00's walling free hang held 0.70 s.
    #[test]
    fn the_swing_into_the_free_idle_lasts_its_first_item() {
        assert!((swing_lock_time(0.0) - 0.5333).abs() < 1e-3);
        assert!((swing_lock_time(1.0) - 0.7333).abs() < 1e-3);
        assert!((swing_lock_time(0.835) - 0.70).abs() < 0.01);
    }

    /// RE/24 LEDGE-10: past a corner the turn fails when the new face needs the other hang type; `TrySwitchHangType`
    /// with the corner variant switches onto that face instead. A wall hang (face z 0) whose corner face (x 0, normal +X)
    /// is an overhang slab with nothing below switches to a free hang turned onto it.
    #[test]
    fn a_corner_onto_an_overhang_switches_a_wall_hang_to_a_free_hang() {
        let mut c = CollisionWorld::default();
        c.boxes.push(Aabb3 { min: Vec3::new(-5.0, 0.0, 0.0), max: Vec3::new(0.0, 2.6, 1.0) });
        c.boxes.push(Aabb3 { min: Vec3::new(0.0, 2.3, 0.0), max: Vec3::new(1.0, 2.6, 1.0) });
        let d = HumanLedgeData { hang_type: LedgeHangType::Wall, ..Default::default() };
        let corner = GuidanceHit { point: Vec3::new(1.0, 2.6, 0.4), edge: 0, wall_normal: Vec3::X, dir: Vec3::Z };
        let rows: ledge_moves::EdgeRows = [None, None, None, Some(corner), None];
        let mv = corner_switch(&d, &rows, false, Vec3::new(-0.2, 1.5, -0.4), Vec3::Z, &c).expect("a switch onto the corner face");
        assert!(mv.end_free && !mv.end_wall, "to a free hang");
        assert!((mv.hand_l - corner.point).length() < 1e-5 && (mv.hand_r - corner.point).length() < 1e-5, "both hands on the corner point");
        assert!(mv.facing_from.dot(Vec3::Z) > 0.99 && mv.facing_to.dot(Vec3::NEG_X) > 0.99, "turns from the old face to the corner face");
        // refused on a closing step
        let closing = HumanLedgeData { hang_type: LedgeHangType::Wall, alt_flag: true, ..Default::default() };
        assert!(corner_switch(&closing, &rows, false, Vec3::ZERO, Vec3::Z, &c).is_none());
    }
}
