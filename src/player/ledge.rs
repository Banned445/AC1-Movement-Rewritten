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
/// - Entry (0) only while the climb's swing back (0x1EB21811), the hop's `_b` (`HOP_UP_B`) or `step1_to_hangwall`
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

/// `sub_DCD910`'s Entry actions with the IK on: the climb's swing back, `HOP_UP_B`, `step1_to_hangwall`.
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
pub const DUMPED_ACTIONS: &[u32] = &[ACT_CATCH_WALL[0], ACT_CATCH_WALL[1], ACT_CATCH_FREE[0], ACT_CATCH_FREE[1], super::ledge_moves::WALLFREE_TO_WALL];

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
        .sum();
    if t > 0.0 { t } else { GRAB_TIME }
}

/// How long a catch holds the hang: both items of the catch action are locked (word 0x0FE0), and
/// `HumanLedge__Movement_ChooseAction` 0xDE29E0 starts no move while the playing item is locked. The wall catch
/// takes its straight clips (PORT: the port's ledges are straight, as the animator plays them).
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
    /// Running vertical hand step: (first hand was the left one, this is the second half).
    pub vstep: Option<(bool, bool)>,
    /// A vertical step's second hand still to move (+1848, `ContinuePendingVerticalStep` 0xDCF3C0): (that hand's index,
    /// the pending direction is up, +1852). The hands stay split until the stick asks for it (RE/24 LEDGE-4).
    pub pending_vstep: Option<(usize, bool)>,
    /// The jump off's stored target (event 2 → 0xDD2CA0 copies it to LedgeData+64), jumped at when the rebound ends.
    pub jump_off: Option<super::air::InAirEntry>,
    /// The last lateral move that was blocked went right (picks `PlayIdleOrBlocked`'s blocked action, 0xDD3A10).
    pub blocked_right: bool,
    moves: Vec<RootInterp>,
    after: Option<After>,
    /// Running corner turn / ledge jump / hop up (`ledge_moves`).
    pub mv: Option<LedgeMove>,
    /// Moves queued after `mv` (pull-down stages); each starts from where the previous one ended.
    pub queue: Vec<LedgeMove>,
    /// The Climb context takes over when `mv` ends (TryJumpUpToClimb 0xDD5E10: landing category 0).
    pub then_climb: Option<ClimbEntry>,
    /// The Ladder context takes over when `mv` ends (TrySideMoveToLadder 0xDD4CD0: flag 1860 | 0x10).
    pub then_ladder: Option<super::ladder::LadderEntry>,
    /// Free hang forced by the hop (0xDDAB00) until the next move re-evaluates the wall below.
    /// PORT: the game switches back through TrySwitchHangType's free → wall anims (not ported).
    pub force_free: bool,
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
    pub fn enter(&mut self, e: LedgeEntry) {
        self.hand_l = e.hand_l;
        self.hand_r = e.hand_r;
        self.normal = e.normal;
        self.sub_state = e.sub_state;
        self.pass_over = None;
        self.swing = None;
        self.alt_flag = false; // EnterCommon 0xDE26D0 clears +0x85
        self.blocked_up = false;
        self.pending_vstep = None;
        self.jump_off = None;
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

/// The ray part of `sub_11718B0` (a wall support for a foot): from `centre` + `up`·z, `half_depth` m out from the wall,
/// a ray of 2·`half_depth` toward it, on layer 43 (`sub_11718B0`). A hit on a body owned by a component (flag
/// 0x10000000, the same tag `ContactManager__GroundMaterial` 0x55A580 tests) is a support, which is all the port's
/// static collision; a hit on a FuzzyZone layer (24 / 26 / 25 / 33 / 41, `sub_E1C1F0`) aborts with no support (fuzzy
/// zones are not imported). PORT: only a miss or an unowned body falls back to Surface guidance edges facing within 45°
/// (sliced every `step`), which is not searched (RE/24 GUIDE-12).
pub fn support_ray(centre: Vec3, n: Vec3, half_depth: f32, z: f32, collision: &CollisionWorld) -> bool {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let start = centre + Vec3::Y * z - facing * half_depth;
    collision.sphere_free_distance(start, facing, 0.01, 2.0 * half_depth) < 2.0 * half_depth - 1e-3
}

/// `sub_B16130`: the wall-hang feet supports under a hold point `hand` (the free-hang drop 0xDDF390, the corners
/// 0xDD3BB0, the climb's ledge grab 0xDF0980): two `sub_11718B0` searches 1.0 m below it and 0.125 m into the wall, one
/// 0.2 m to the left raised 0.1 m and one under the point lowered 0.1 m (box half extents 0.25 / 0.625 / 0.15); either
/// is enough (the other foot then shares it).
pub fn feet_supports(hand: Vec3, n: Vec3, collision: &CollisionWorld) -> bool {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let c = hand - Vec3::Y + facing * 0.125;
    support_ray(c - right_of(facing) * 0.2, n, 0.625, 0.1, collision) || support_ray(c, n, 0.625, -0.1, collision)
}

/// `sub_B15C30`: the jump arrival's foot holds under a hold point `hand` (`HumanInAir__CheckJumpTargetArrival`
/// 0xE07D00, generic branch 0xE093FC): two `sub_11718B0` searches 1.0 m below it and 0.15 m into the wall, one
/// 0.15 m to the left raised 0.05 m and one 0.15 m to the right lowered 0.05 m (box half extents 0.4 / 0.625 / 0.2;
/// the offset is in the wall's frame, x across, z up). Both feet must find the wall.
pub fn reception_feet(hand: Vec3, n: Vec3, collision: &CollisionWorld) -> bool {
    let facing = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let c = hand - Vec3::Y + facing * 0.15;
    let across = facing.cross(Vec3::Y);
    support_ray(c - across * 0.15, n, 0.625, 0.05, collision) && support_ray(c + across * 0.15, n, 0.625, -0.05, collision)
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
    d.force_free = false;
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
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle, &mut LimbTargets), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data, mut limbs) in &mut q {
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
            let out = super::swing::update(&mut s, dt, held, jump, &mut feet, &collision, |p, f| super::targets::find_jump_target(p, f, &guidance, &collision));
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
                    switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(super::air::InAirEntry::SwingJump { from, target }));
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
                    let entry = match ledge_moves::pulldown_passover(far, normal, body.feet, p.hand, p.w, &guidance.ledge_report(far, 2.5, true), &collision) {
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
            let t = match d.catch_action {
                Some(id) => action_time(id),
                None => catch_time(d.hang_type == LedgeHangType::Wall, long),
            };
            if let Some(m) = d.moves.first_mut() {
                m.duration = t;
            }
        }
        // the limb IK runs only in some sub-states (`sub_DCD910` from `HumanLedge__Update` 0xDE3EC8): elsewhere the
        // animation alone shows, released at once (RE/24 ANIM-10)
        let playing = d.mv.and_then(|m| m.current()).map(|(b, _)| b.id).or(d.catch_action);
        let ik_runs = ledge_ik_runs(d.sub_state, playing);
        limbs.suspended = !ik_runs;
        limbs.hands = ik_runs.then_some((d.hand_l, d.hand_r));
        limbs.normal = n;
        limbs.transit = d.moves.first().map(|m| m.duration).unwrap_or(SHIMMY_OPEN_TIME) * 0.8;
        // the feet follow the hang clip (braced against the wall, or dangling); only the hands are pinned
        limbs.feet = None;

        // ---------------------------------------------------------------- corner / ledge jump / hop
        if let Some(mut mv) = d.mv {
            let (p, done) = mv.advance(dt);
            body.feet = p;
            body.heading = heading_of(mv.facing());
            limbs.hands = None;
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
                // knee / waist arrivals end standing on top (Ledge SubState 4 → pull-up). Game: NarrowObject
                // on the edge; PORT: Ground
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
                d.force_free = false;
                d.mv = None;
                d.sub_state = LedgeSubState::Movement;
                if let ledge_moves::MoveKind::SideJump { .. } = mv.kind {
                    // the table jump into a hang: +133 = the hand targets 0.3 m or more apart (`sub_DCBCD0`, RE/24 LEDGE-14)
                    d.alt_flag = (mv.hand_l - mv.hand_r).length() >= 0.3;
                } else if !matches!(mv.kind, ledge_moves::MoveKind::SwitchHang { .. }) {
                    d.alt_flag = false;
                }
            } else {
                d.mv = Some(mv);
            }
            continue;
        }

        // ---------------------------------------------------------------- running move
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
            if !d.moves.is_empty() {
                continue;
            }
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
        let spacing = (d.hand_l - d.hand_r).length();
        let tol = LOST_LEDGE_R + spacing * 0.5;
        if guidance.on_edge(d.hand_l, n, tol).is_none() || guidance.on_edge(d.hand_r, n, tol).is_none() {
            let origin = if d.hang_type == LedgeHangType::Wall { FallOrigin::HangWall } else { FallOrigin::HangFree };
            let hold = hand_mid(d);
            limbs.hands = None;
            limbs.feet = None;
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::Fall { from: body.feet, velocity: Vec3::ZERO, origin, speed_param: 0.0 }));
            data.air.release_hold = Some(hold);
            continue;
        }
        // the interpreter's latched stick in the character's frame (`GoAssassinActionInterpreter__LedgeState` 0xEEC356:
        // the angle of +0x1310 from stick-up; RB held reads the unmirrored +0x1300, 0xEEC256; RE/24 INPUT-4)
        let dir = if pad.speed01 > 0.0 { Some(quantize(pad.actor_dir(facing, pad.buttons[13]), facing)) } else { None };
        // the ledge report (`HumanLedge__BuildLedgeReport` 0xDDC620 → HumanData +116): ladder zones carved, steep edges
        // removed; ProbeLateral, FindCornerEdges, FindVerticalRows, the side jumps and TryJumpUpToClimb read it
        // (RE/24 GUIDE-2). Its largest box reaches 3 m sideways from the hands.
        let report = guidance.ledge_report(hand_mid(d), 3.5, true);
        // the interpreter's ledge state (`GoAssassinActionInterpreter__LedgeState` 0xEEBFD0, RE/03 7.12): the
        // empty-hand buffer lets go (event 4, 0xDDB450 -> LetGoToInAir) ...
        if pad.hand_buffered() {
            pad.consume_hand();
            limbs.hands = None;
            limbs.feet = None;
            let origin = if d.hang_type == LedgeHangType::Wall { FallOrigin::HangWall } else { FallOrigin::HangFree };
            let hold = hand_mid(d);
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::Fall { from: body.feet, velocity: Vec3::ZERO, origin, speed_param: 0.0 }));
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
            let (first_p, second_p) = if second == 0 { (d.hand_r, d.hand_l) } else { (d.hand_l, d.hand_r) };
            let along = right * (second_p - first_p).dot(right);
            let target = match rows.step {
                Some(h) => h.point + right * (second_p - hand_mid(d)).dot(right),
                None => first_p + along,
            };
            if second == 0 { d.hand_l = target } else { d.hand_r = target }
            d.pending_vstep = None;
            let to = hang_root_at(d.hand_l, d.hand_r, n, d.hang_type, &collision);
            d.vstep = Some((second == 1, true));
            d.moves.push(RootInterp::new(body.feet, to, VSTEP_SECOND_TIME));
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
                // StartHandStep 0xDDE0C0 (vertical): the hand step onto the step row's edge (0.1–0.8 m along the step)
                // PORT: each hand keeps its lateral offset from the hands' midpoint on the new edge (the game moves one
                // hand onto the hit, PickVerticalHandAndAnim 0xDCB3E0, and the other follows with its own search,
                // ContinuePendingVerticalStep 0xDCF3C0); the target's hang body box is not tested.
                if let Some(h) = rows.step {
                    let mid = hand_mid(d);
                    let along = |p: Vec3| right * (p - mid).dot(right);
                    let tl = h.point + along(d.hand_l);
                    let tr = h.point + along(d.hand_r);
                    // first hand moves now, the second one next (pending vertical step)
                    let first_left = !d.alt_flag;
                    d.alt_flag = !d.alt_flag;
                    if first_left { d.hand_l = tl } else { d.hand_r = tr }
                    let mid_root = (body.feet + hang_root_at(tl, tr, n, d.hang_type, &collision)) * 0.5;
                    d.vstep = Some((first_left, false));
                    let first_time = match (d.hang_type, sign > 0.0) {
                        (LedgeHangType::Wall, true) => VSTEP_TIME,
                        (LedgeHangType::Wall, false) => VSTEP_DOWN_TIME,
                        _ => VSTEP_SECOND_TIME,
                    };
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
                    // TryWallJumpUp 0xDD62A0: hop up to a ledge above the reach of a hand step (wall hang).
                    if let Some(mv) = ledge_moves::try_hop_up(d.hand_l, d.hand_r, n, body.feet, d.hang_type, &guidance, &collision) {
                        start_move(d, mv, "hop up");
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
                // TrySideMoveToLadder 0xDD4CD0 (`sub_DD4CD0(dir, 8, 0)`): a ladder at the side
                if let Some((mv, e)) = ledge_moves::try_side_to_ladder(right_dir, n, body.feet, d.hang_type, &guidance) {
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
                    d.alt_flag = !d.alt_flag;
                    d.force_free = false;
                    let to = hang_root_at(d.hand_l, d.hand_r, n, d.hang_type, &collision);
                    // after the toggle: alt_flag set = this was the lead hand's reach (open), clear = closing step
                    d.moves.push(RootInterp::new(body.feet, to, if d.alt_flag { SHIMMY_OPEN_TIME } else { SHIMMY_CLOSE_TIME }));
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
