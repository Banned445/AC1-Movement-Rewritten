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

/// `HumanLedgeData::LedgeHangType` (+104, desc 0x1994E68).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LedgeHangType {
    #[default]
    Wall = 0,
    Free = 1,
    /// WallFree (2). Its only writer in v1.02 is walling's post-run grab (`HumanWalling__TryGrabLedgeAfterWallRun`
    /// 0xE399D5), which is unreachable (RE/05 §1.9): every other entry writes 0 or 1, so the game never holds it. Its
    /// readers are ported: the table lookups take it as Free (`!= Wall`), `ComputeRootFromHandTargets` 0xDD6899 sends
    /// it down the wall branch (`hang_root`), the entry's end plays the wall-free wait 0x0106F2E9 (0xDE20C3), and
    /// `Pullup_Start` 0xDDC2E3 plays `hangwallfree_tr_hangwall` into the wall hang (+104 := 0). The straight jump's
    /// wall-free reception is a Free hang (0xE08EE9) that plays the wall-free wait (`HumanLedgeData::wallfree_wait`).
    WallFree = 2,
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
    /// Both hand records on the hold `mid`, facing into the wall. The game's fills set the hand records each their own
    /// way (`LimbIK` contacts 0 / 1): the climb's ledge grab (`HumanClimb__TryLedgeGrab` 0xDF0980), the side jumps
    /// (`HumanLedge__TrySideJumpToLedge` 0xDDD490) and the free-hang drop (`TryFreeHangDropToClimb` 0xDDF6E0) put both on
    /// the one hold; the jump arrivals (`arrival_hands`), the air catches, the pull-downs, the ladder's side hang and the
    /// wall run's hangs set two (their callers overwrite these). EnterCommon 0xDE26D0 re-poses SubStates 1 / 2 / 3 / 5 /
    /// 7 / 10 / 12 from the hand bones, the Entry state at its end (0xDE1970, RE/24 LEDGE-22).
    pub fn at(mid: Vec3, normal: Vec3, from_feet: Vec3, sub_state: LedgeSubState) -> Self {
        Self {
            hand_l: mid,
            hand_r: mid,
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

/// How long the wall pull-down's reception holds the hang after its `_a` (the root's interpolation): the idle request
/// plays the graph's transition `0x082F820C` first, whose first item (`_b`, word 0x0FF0) is locked
/// (`Movement_ChooseAction` 0xDE29E0 → 0xDE2B1C), with the reception's angle weights. PORT: the transition plays with
/// its authored weights (the port's ledges are straight).
pub fn reception_lock_time(w: &[f32]) -> f32 {
    use super::jump_blend::{action_items, ActionBlend};
    let id = ledge_moves::PULLDOWN_WALL[1];
    action_items(id).filter(|i| !i.is_empty()).map(|_| ActionBlend::new(id, 0, w).duration()).unwrap_or(0.0)
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
    /// hang (the hang type follows it) and, for the long free-hang catches (type 4), the hand that reaches for the edge
    /// once the catch ends (true = right; `ledge_moves::catch_reach`).
    pub catch_action: Option<u32>,
    pub catch_free: bool,
    pub catch_reach: Option<bool>,
    /// The straight jump's wall-free reception (`0x0121B072`, after `jumpstraight_to_hangwallfree`) plays the wall-free
    /// wait 0x0106F2E9 behind it (0xE09055) on a free hang (+104 = 1, 0xE08EE9 / 0xE08C2D); it lasts until the next move.
    /// The `step_seq` it was set at.
    pub wallfree_wait: Option<u32>,
}

impl HumanLedgeData {
    /// The swing strength for the free idle, while no move has started since it was stored.
    pub fn swing_idle(&self) -> Option<f32> {
        self.swing_strength.filter(|(_, seq)| *seq == self.step_seq).map(|(s, _)| s)
    }

    /// The wall-free wait plays: the straight jump's wall-free reception with no move since, or the WallFree type.
    pub fn wallfree_idle(&self) -> bool {
        self.hang_type == LedgeHangType::WallFree || self.wallfree_wait == Some(self.step_seq)
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
        self.wallfree_wait = None;
        self.swing_lock = 0.0;
        self.moves.clear();
        self.mv = None;
        self.hang_set = false;
        self.catch = e.catch.filter(|_| e.entry_move.is_none());
        self.catch_action = None;
        self.catch_free = false;
        self.catch_reach = None;
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

/// The released-hold record of every Ledge → InAir fill (`HumanLedge__FillInAirData_LetGo` 0xDD7810: the let-go, the
/// lost ledge, the pull-down's release …): InAirData+400 = the root + 1.1 m up from a wall hang (+500 = 2), + 2.4 m from
/// a free hang (+500 = 3). The air catches ignore holds less than 0.5 m below it (`HumanInAir__FindLedgeCatch` 0xE0A990).
pub fn release_record(root: Vec3, hang: LedgeHangType) -> Vec3 {
    root + Vec3::Y * if hang == LedgeHangType::Wall { 1.1 } else { 2.4 }
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

/// The jump arrival's hand records (`HumanInAir__CheckJumpTargetArrival` 0xE07D00): `Guidance__ProbeBoxHandPair`
/// 0x1171050 at the target point, the box facing into the wall, mask 306, step 0.1, hand references (∓0.3, −0.15, 0) in
/// the box frame (right, forward, up): 0.3 m to either side, 0.15 m out from the wall. The generic receptions (the wall
/// reception and the swing, `sub_E00C50`) use half extents 0.4 / 0.3 / 0.3 and 70°; the straight jump's (`sub_E00B10`)
/// 0.4 / 0.15 / 0.07 and 45°. No pair: the arrival returns 0 (the jump ends without a reception, the air catch takes
/// over). Hands 0 / 1 are the left / right (`sub_E55CB0`(0 / 1, …), 0xE08BFB / 0xE09520 / 0xE096F2); the wall and free
/// receptions keep them (EnterCommon 0xDE26D0 re-poses neither SubState 6 nor 8).
pub fn arrival_hands(target: Vec3, n: Vec3, straight: bool, guidance: &crate::guidance::GuidanceWorld) -> Option<(Vec3, Vec3)> {
    let forward = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let right = right_of(forward);
    let (half, angle) = if straight { (Vec3::new(0.4, 0.15, 0.07), std::f32::consts::FRAC_PI_4) } else { (Vec3::new(0.4, 0.3, 0.3), 70f32.to_radians()) };
    let refs = [target - right * 0.3 - forward * 0.15, target + right * 0.3 - forward * 0.15];
    guidance.probe_facing_pair(target, forward, half, refs, angle, 0.1, crate::guidance::MASK_CATCH).map(|[l, r]| (l.point, r.point))
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

/// Root (feet) for a hang (ComputeRootFromHandTargets 0xDD6730): only type 1 is the free pose (0xDD6899), the holds'
/// midpoint 2.4 m below the higher one (`Human__FreeHangPoseFromHolds` 0xB15AD0); types 0 and 2 take the wall branch.
/// The entries and every step hang here, at the distance from the wall of their type; only the straight jump's hang
/// receptions keep their band's root (+out·n − down, 0xB21DA0) until the next move.
pub fn hang_root(hand_l: Vec3, hand_r: Vec3, n: Vec3, hang: LedgeHangType) -> Vec3 {
    let mid = (hand_l + hand_r) * 0.5;
    match hang {
        LedgeHangType::Wall | LedgeHangType::WallFree => {
            Vec3::new(mid.x, hand_l.y.min(hand_r.y) - WALL_HANG_DROP, mid.z) + n * WALL_HANG_OUT
        }
        LedgeHangType::Free => {
            Vec3::new(mid.x, hand_l.y.max(hand_r.y) - FREE_HANG_DROP, mid.z) + n * FREE_HANG_OUT
        }
    }
}

/// The end of `HumanLedge__FindVerticalRows` 0xDD8F70: the root from the hand targets with the stepping hand (`hand`) on
/// the step row (`ComputeRootFromHandTargets` 0xDD6730); in a free hang (+104 = 1) the body boxes there
/// (`Human__FreeHangBodyBoxesFree` 0xB2DD50) must be free, else the row is dropped (+912 = 0) and no step starts
/// (`StartHandStep` 0xDDE0C0 needs it). The pending second hand's target, the row's or its own point, is tested the same.
/// Live (2026-10-13, wall 23): the hold 0.155 m above the walling free hang is found, then dropped here, and the game
/// hangs on. PORT: the wall hangs' box (`Human__WallHangBodyBoxFree` 0xB2DC80, its root moved by the hold's lean,
/// 0xDD9591–0xDD9617) is not tested.
fn step_fits(d: &HumanLedgeData, hand: usize, target: Vec3, n: Vec3, collision: &CollisionWorld) -> bool {
    if d.hang_type != LedgeHangType::Free {
        return true;
    }
    let (mut hl, mut hr) = (d.hand_l, d.hand_r);
    if hand == 0 {
        hl = target;
    } else {
        hr = target;
    }
    let root = hang_root(hl, hr, n, d.hang_type);
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    super::climb::hang_boxes_free(collision, root, facing, false)
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
            // the stick past the dead zone within 75° of the facing (1.3089969) and Legs held (`sub_6D0F50(0)` =
            // `Pad__IsPressed`, 0xEECE5E; RE/24 LEDGE-17, RE/27 C1): holding Legs with the stick forward jumps off at the
            // release point, no press needed
            let held = pad.high_profile && pad.legs_held;
            let toward = pad.speed01 > 0.0 && pad.dir_snap.dot(s.fwd) > 1.308_997f32.cos();
            let jump = held && toward;
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

        // ---------------------------------------------------------------- the jump off during an Entry-state grab
        // The Entry state (LedgeData +76 = 0: the wall run's hangs, the climb's jump up; `repose_at_entry_end` marks
        // them) takes event 2 too (`HumanLedge__CanHandleEvent` 0xDE3A30, slot +2792 = `StateEntry` → guard
        // `HumanLedge__Guard_EntryJumpOff` 0xDCBF00): at any time while `step1_tr_hangwall` (0x0106CFC5, the wall run's kick up
        // into a wall hang) plays, else while the playing action's normalised time is under 0.25 (the swings into a free
        // hang). No hang-type test (the Movement guard 0xDCCE10 refuses a free hang). The interpreter sends it as in
        // Movement (IHumanLedge vt24 = CanHandleEvent(2), 0xDE4170; high profile and the Legs buffer, 0xEEC89F), and the
        // state's handler `sub_DDFF30` starts the same rebound jump (`HumanLedge__StartReboundJump` 0xDD2CA0).
        if let Some(mv) = d.mv.filter(|_| d.repose_at_entry_end && pad.high_profile && pad.jump_buffered()) {
            let accepts = mv.action_time().is_some_and(|(id, t)| id == super::walling::STEP1_TO_HANGWALL || t < 0.25);
            let n = mv.normal;
            if let Some(jd) = accepts.then(|| super::targets::jump_off_wall_dir((pad.speed01 > 0.0).then_some(pad.dir_snap), n)).flatten() {
                pad.consume_jump();
                d.repose_at_entry_end = false;
                d.queue.clear();
                d.hand_l = mv.hand_l;
                d.hand_r = mv.hand_r;
                d.normal = n;
                d.jump_off = Some(super::targets::jump_off_wall_entry(body.feet, jd, &guidance, &collision));
                start_move(d, ledge_moves::rebound_move(body.feet, n, jd, mv.hand_l, mv.hand_r), "rebound");
                d.sub_state = LedgeSubState::ReboundTransition;
                continue;
            }
        }

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
                let hold = release_record(body.feet, LedgeHangType::Wall);
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
                // the straight jump's wall-free reception plays the wall-free wait behind it (0xE09055) on a free hang
                // (+104 = 1, 0xE08EE9) until the next move
                if mv.kind == (ledge_moves::MoveKind::Arrival { wallfree_wait: true }) {
                    d.wallfree_wait = Some(d.step_seq);
                }
                // the wall pull-down's locked `_b` (the free one is not)
                if mv.kind == (ledge_moves::MoveKind::PullDown { stage: 3 }) && d.hang_type == LedgeHangType::Wall {
                    d.swing_lock = reception_lock_time(mv.seq[0].map(|a| a.w).as_ref().map(|w| &w[..3]).unwrap_or(&[1.0, 0.0, 0.0]));
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
            let hold = release_record(body.feet, d.hang_type);
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
                m.to = hang_root(d.hand_l, d.hand_r, n, d.hang_type);
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
            // a long free-hang catch (type 4) ends in SecondHandGrab, not in the wait (`HumanLedge__StateGrasp_Update`
            // 0xDDFDB0 → `HumanLedge__Grasp_SetupSecondHand` 0xDCDCF0): the other hand reaches for the edge
            if let (Some(right), Some(catch)) = (d.catch_reach.take(), d.catch_action) {
                let hold = if right { d.hand_l } else { d.hand_r };
                if let Some(mv) = ledge_moves::catch_reach(catch, right, hold, n, body.feet, &guidance, &collision) {
                    start_move(d, mv, "catch: second hand");
                    continue;
                }
            }
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
            let hold = release_record(body.feet, d.hang_type);
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
            if !step_fits(d, second, target, n, &collision) {
                d.last_action = "idle";
                continue;
            }
            if second == 0 { d.hand_l = target } else { d.hand_r = target }
            d.pending_vstep = None;
            d.last_hand = second;
            d.alt_flag = false;
            let to = hang_root(d.hand_l, d.hand_r, n, d.hang_type);
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
        // Ledge event 10, the turn round under a beam (RE/28 §7): the interpreter sends it for the stick more than 135°
        // from the facing (0xEECCCA) when its guard passes. PORT (order): taken before the vertical moves the same stick
        // asks for (the game posts the event from the interpreter and the Movement choice runs in the ledge update).
        if dir == LedgeDir::Down {
            if let Some(mv) = ledge_moves::try_turn_under_beam(d.hand_l, d.hand_r, n, body.feet, d.hang_type, &guidance, &collision) {
                start_move(d, mv, "turn under beam");
                continue;
            }
        }
        match dir {
            LedgeDir::Up | LedgeDir::Down => {
                let sign = if dir == LedgeDir::Up { 1.0 } else { -1.0 };
                // sub_DD8F70: the feet, hands and hand-step rows for this direction
                let rows = ledge_moves::vertical_rows(d.hand_l, d.hand_r, n, dir == LedgeDir::Up, &report);
                // the step row is dropped where the body would not fit (`step_fits`); the hand not moved last steps
                let mut rows = rows;
                if rows.step.is_some_and(|h| !step_fits(d, if d.last_hand == 1 { 0 } else { 1 }, h.point, n, &collision)) {
                    rows.step = None;
                }
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
                // A free hang's body boxes at the target were tested with the rows (`step_fits`).
                if let Some(h) = rows.step {
                    // first hand moves now, the second one next (pending vertical step)
                    let first_left = d.last_hand == 1;
                    d.alt_flag = false;
                    d.last_hand = (!first_left) as usize;
                    if first_left { d.hand_l = h.point } else { d.hand_r = h.point }
                    // the root from the two hand targets as they stand after the first hand (`ComputeRootFromHandTargets`
                    // 0xDD6730): a free hang hangs from the higher hand (max − 2.4), so a step up lifts the body the
                    // whole step and the reaching hand gets to its hold; a wall hang's vertical step reads the lower hand
                    // (min − 1.1) and waits for the second. The halfway root left the reaching arm 0.4 m short of its hold
                    // (bug-1791711547). Its distance from the wall is the type's: a step from the straight jump's wall-free
                    // reception (root 0.5 m out) brings it under the hands
                    let mid_root = hang_root(d.hand_l, d.hand_r, n, d.hang_type);
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
                    // a free hang (+104 = 1) with no pending step whose two foot rays find the wall and whose wall-hang body
                    // box is free (`HumanLedge__FreeHangFeetFindWall` 0xDE0E30) is accepted before the room test, flag 0x20
                    // (0xDE22FD); `Pullup_Start` 0xDDC32C then switches it to the wall hang instead of pulling up
                    // (`HumanLedge__PullupSwitchFreeToWall` 0xDD4160), and the next push up is the wall pull-up
                    if d.hang_type == LedgeHangType::Free && d.pending_vstep.is_none() {
                        let (a, b) = feet_on_wall(hand_mid(d), n, &collision);
                        let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
                        if a && b && super::climb::hang_boxes_free(&collision, hang_root(d.hand_l, d.hand_r, n, LedgeHangType::Wall), facing, true) {
                            start_move(d, ledge_moves::pullup_switch_to_wall_move(body.feet, d.hand_l, d.hand_r, n), "switch to wall hang (pull-up)");
                            continue;
                        }
                    }
                    // `HumanLedge__CanPullup` 0xDE2270's room test (RE/28 §1): `Human__KneeCatchRoomOnTop` 0xB2E240 at the
                    // hands' midpoint along the facing, room for the body on top with no floor test; else a top edge
                    // probed at the higher hand (box half 0.25 / 0.35 / 0.25, reference 0.2 up, 45°, mask 50) with room
                    // at its point, both hands then on that point. A beam or a thin wall top passes.
                    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
                    let mut mid = hand_mid(d);
                    let mut room = ledge_moves::knee_room_on_top(mid, facing, &collision);
                    if !room {
                        let hi = if d.hand_l.y >= d.hand_r.y { d.hand_l } else { d.hand_r };
                        let centre = Vec3::new(hi.x, mid.y, hi.z);
                        if let Some(hit) = guidance.probe_box(centre, right_of(facing), facing, Vec3::Y, Vec3::new(0.25, 0.35, 0.25),
                            centre + Vec3::Y * 0.2, std::f32::consts::FRAC_PI_4, 0.1, crate::guidance::MASK_CLIMB_HOLDS) {
                            let into = -Vec3::new(hit.wall_normal.x, 0.0, hit.wall_normal.z).normalize_or(facing);
                            if ledge_moves::knee_room_on_top(hit.point, into, &collision) {
                                d.hand_l = hit.point;
                                d.hand_r = hit.point;
                                mid = hit.point;
                                room = true;
                            }
                        }
                    }
                    if room {
                        // root: along the pull-up clips' own displacement (up, then in over the lip; FROMANIM). PORT: with a
                        // floor 0.5 m in (Pullup_Start 0xDDBE80's stand point) the root is also corrected onto it; on a
                        // narrow top (a beam) the clips alone place it, and the free-step stay mounts what is there
                        let top = Vec3::new(mid.x, mid.y, mid.z) - n * PULLUP_IN;
                        let standable = collision
                            .ground_height(top + Vec3::Y * 0.05, 0.3)
                            .filter(|h| (h - mid.y).abs() < 0.25)
                            .is_some()
                            && collision.capsule_fits(Vec3::new(top.x, mid.y, top.z));
                        let top_feet = standable.then(|| Vec3::new(top.x, mid.y, top.z));
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
                        // the WallFree type (+104 = 2; never held in v1.02, see `LedgeHangType::WallFree`) does not pull up:
                        // `Pullup_Start` 0xDDC2E3 plays `hangwallfree_tr_hangwall` into the wall hang (+104 := 0)
                        if d.hang_type == LedgeHangType::WallFree {
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
                    let to = hang_root(d.hand_l, d.hand_r, n, d.hang_type);
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
    /// `FindVerticalRows` 0xDD8F70 drops a free hang's step row when the body boxes at the stepped root are not free (live
    /// wall 23); a wall hang's are not tested (PORT).
    #[test]
    fn a_free_hang_steps_only_where_the_body_fits() {
        let n = Vec3::Z; // the wall faces +Z, the hanging body is in front of it
        let (hl, hr) = (Vec3::new(-0.2, 3.0, 0.0), Vec3::new(0.2, 3.0, 0.0));
        let d = HumanLedgeData { hang_type: LedgeHangType::Free, hand_l: hl, hand_r: hr, normal: n, ..Default::default() };
        let target = Vec3::new(0.2, 3.15, 0.0);
        let mut c = CollisionWorld::default();
        assert!(step_fits(&d, 1, target, n, &c), "nothing in the way");
        // a slab sticking out of the wall where the raised body's legs would be
        c.boxes.push(Aabb3 { min: Vec3::new(-1.0, 1.0, 0.0), max: Vec3::new(1.0, 1.2, 0.6) });
        assert!(!step_fits(&d, 1, target, n, &c), "the body would hit the slab");
        let wall = HumanLedgeData { hang_type: LedgeHangType::Wall, ..d };
        assert!(step_fits(&wall, 1, target, n, &c), "wall hangs untested");
    }

    #[test]
    fn the_swing_into_the_free_idle_lasts_its_first_item() {
        assert!((swing_lock_time(0.0) - 0.5333).abs() < 1e-3);
        assert!((swing_lock_time(1.0) - 0.7333).abs() < 1e-3);
        assert!((swing_lock_time(0.835) - 0.70).abs() < 0.01);
    }

    /// Bug 1792058748: a wall hang on a bay whose face ends 0.9 m below the holds switched to a free hang 0.5 m out from
    /// the wall (the wall-free pose, picked by the wall still under the hands), and every later step kept that distance.
    /// `TrySwitchHangType` 0xDE1060 sets the plain free hang (+104 = 1, 0xDE1822) on `Human__FreeHangPoseFromHolds`
    /// 0xB15AD0: the holds' midpoint, 2.4 m below.
    #[test]
    fn a_switch_to_the_free_hang_hangs_under_the_holds() {
        let mut c = CollisionWorld::default();
        // face z 0 (normal -Z) from 3.3 m to 7 m, nothing below
        c.boxes.push(Aabb3 { min: Vec3::new(-2.0, 3.3, 0.0), max: Vec3::new(2.0, 7.0, 0.9) });
        let (n, hold) = (Vec3::NEG_Z, Vec3::new(0.0, 4.2, -0.08));
        let mv = ledge_moves::switch_move(2, false, Vec3::new(0.0, 3.1, -0.58), hold, hold, n, &c);
        assert!(mv.end_free);
        assert!(mv.to.distance(Vec3::new(0.0, 1.8, hold.z)) < 0.02, "root {} should be under the holds", mv.to);
    }

    /// `ComputeRootFromHandTargets` 0xDD6899: only +104 == 1 is the free pose; the WallFree type (2) takes the wall
    /// branch (1.1 m below the lower hand, 0.5 m out).
    #[test]
    fn the_wall_free_type_hangs_from_the_wall_branch() {
        let (hl, hr, n) = (Vec3::new(-0.2, 3.0, 0.0), Vec3::new(0.2, 3.2, 0.0), Vec3::NEG_Z);
        assert_eq!(hang_root(hl, hr, n, LedgeHangType::WallFree), hang_root(hl, hr, n, LedgeHangType::Wall));
        let free = hang_root(hl, hr, n, LedgeHangType::Free);
        assert!((free - Vec3::new(0.0, 0.8, -crate::tuning::FREE_HANG_OUT)).length() < 1e-5, "{free}");
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
