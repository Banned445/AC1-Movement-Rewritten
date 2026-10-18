//! Ground locomotion context (`HumanGround`, ActorContextID 4) — RE/02.
//!
//! - One speed parameter (0..1) split into bands Walk/Jog/Run/Sprint (GetSpeedBand 0xD807B0).
//! - Target = base + 0.25·stick, base 0 (low profile) / 0.5 (high profile) / 0.75 (sprint).
//! - Parameter rises at 1.0/s and falls through the deceleration ResponseCurve (0xDA0810, `move_blend`).
//! - Heading turns toward the wanted direction at 360°/s (0xD95290).
//! - Translation: root motion of the 17-clip locomotion blend (action 0x05923BDB, `move_blend`).
//! - Ground loss → InAir fall (0xD87720 / 0xD8C380); jump request → InAir jump-to-target.

use bevy::prelude::*;

use super::air::{FallOrigin, InAirEntry, Landing, LandingType};
use super::climb::{ClimbEntry, ClimbEntryType};

use super::jump_blend::{self, ActionBlend};
use super::move_blend::MoveBlend;
use super::targets::JumpTarget;
use super::{
    heading_of, switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, SpawnPoint, TransitionSetup,
};
use crate::camera::CameraRig;
use crate::collision::CollisionWorld;
use crate::guidance::GuidanceWorld;
use crate::input::PadInput;
use crate::tuning::*;

/// `HumanGroundData::HumanGroundSubState` (values from the exe, desc 0x19961C8).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HumanGroundSubState {
    #[default]
    Movement = 0,
    Fight = 1,
    FreeRun = 2,
    OrientedMove = 3,
    Hurt = 4,
    ObstacleCollision = 5,
}

/// Speed band as returned by `IHumanGround::GetSpeedBand` (same order as AssassinAbilitySet::MaxSpeed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedBand {
    None,
    Walk,
    Jog,
    Run,
    Sprint,
}

pub fn speed_band(param: f32) -> SpeedBand {
    if param <= 0.0 {
        SpeedBand::None
    } else if param <= BAND_WALK {
        SpeedBand::Walk
    } else if param <= BAND_JOG {
        SpeedBand::Jog
    } else if param <= BAND_RUN {
        SpeedBand::Run
    } else {
        SpeedBand::Sprint
    }
}

/// Runtime data of the ground context (subset of the reflected HumanGroundData, RE/07).
#[derive(Debug, Default)]
pub struct HumanGroundData {
    pub sub_state: HumanGroundSubState,
    /// HG+0x5E8: the 0..1 speed parameter.
    pub speed_param: f32,
    /// Turn attenuation factor (interpreter +0x10D0).
    pub turn_atten: f32,
    /// `Sprint` flag (HumanGroundData+0x123, name recovered by CRC32).
    pub sprint: bool,
    pub high_profile: bool,
    /// The landing / reception action playing after an InAir landing (0xE05940 / 0xE07D00): its root
    /// motion moves the character and input waits until it ends (its transitions lead back to the
    /// locomotion action 0x05923BDB or wait).
    pub oneshot: Option<GroundOneShot>,
    /// Played when `oneshot` ends (the run stop's settle into the wait).
    pub oneshot_next: Option<ActionBlend>,
    /// Landing history for diagnostics; activation effects are owned by `pending_landing`.
    pub last_landing: Option<Landing>,
    /// Entry event, consumed once by the first Ground update and replaced on every entry.
    pending_landing: Option<Landing>,
    /// Incremented on every landing (lets the animator play the landing clip once).
    pub landing_seq: u32,
    /// `HumanGround__UpdateMoveBlend` state: blend weights, timers, lean/bank, step cycle.
    pub blend: MoveBlend,
    /// LedgeStop sub-state (38): the edge it stopped at.
    pub ledge_stop: Option<LedgeStop>,
    /// PORT: no new ledge stop until the stick lets go or turns away from this edge normal. The sender of event 69 is
    /// the interpreter (0xEE8899 → `HumanGround__DoLedgeStop` vt748 0xDBCAD0, RE/02 §9.1), every frame its edge test
    /// holds; whether a held stick stops again after the stop ends is a live check (M5).
    pub ledge_stop_lock: Option<Vec3>,
    /// ObstacleCollision (sub-state 5, event 42): the collide / lean on an obstacle (`collide`).
    pub collide: Option<super::collide::Collide>,
    /// Look-down at an edge (sub-state 9, event 119, `HumanGround__LookDown_Enter` 0xD9FC80).
    pub look_down: Option<LookDown>,
    /// The interpreter's edge-halt latch (+4176) and how long it has held (timer +4144, 0xEE7C5E–0xEE7C8E).
    pub edge_halt_t: f32,
    /// The wall run waiting for its Ground lead-in (`walling::GROUND_LEAD_IN`) to end: the wall's contact and normal
    /// (the payload 0xD857C0 keeps at HG+1344).
    pub wall_start: Option<(Vec3, Vec3, Vec3)>,
    /// The impulsion jump (state +245, entered by event 45): its target, taken off with the free-step jump (kind 1,
    /// `HumanGround__StartFreeStepJumpToTarget` 0xD858A0) when `jump_blend::IMPULSION_JUMP` completes (0xD8E390).
    pub impulsion_jump: Option<ImpulsionJump>,
    /// The interpreter's lean push timer (+0xFC8, 0xEE75F4–0xEE763E): how long the stick has pushed within 45° of the
    /// facing while leaning; any other frame restarts it.
    pub lean_push_t: f32,
    pub pose_seq: u32,
    /// Facing applied when the next one-shot starts (the lean exits end turned, `collide`).
    pub face_next: Option<f32>,
    /// Side of the turn in progress (+1 / -1, 0 = none): the 180 deg tie-break.
    pub turn_sign: f32,
    pub extra: Option<super::ground_extras::ExtraAction>,
    pub crowd_avoid: Option<Vec3>,
    pub crouch: Option<super::crouch::Crouch>,
    /// Data+284 and HG+1784, set by the social helper bit-5 interpreter branch (0xEE7048).
    /// PORT boundary: the producer of that social status remains under RE. No invented crouch button.
    pub crouch_requested: bool,
    pub crouch_hide_hint: bool,
    /// Input timer +4408, reset every frame Legs is held (0xEEE08C).
    pub legs_released_ago: f32,
    /// The transition action playing in front of the locomotion or the wait (MoveBlend's transition path, RE/02 §4.6).
    pub tr: Option<super::ground_tree::MoveTransition>,
    pub tr_seq: u32,
    /// The Movement state that owns the playing one-shot (its update decides what follows it, RE/02 §4.6).
    pub owner: TreeState,
    /// HG+688 (0x2B0): the pivot's turn angle, read by the turn start's layouts 2 / 3.
    pub pivot_turn: f32,
    /// HG+1500 when the pivot started: the turn start's layout follows it (`HumanGround__PlayTurnStartMove` 0xD99710).
    pub pivot_high: bool,
    /// HG+1780: the interpreter's quick-reversal flag (vt976, `player::interpreter`); the run stop turns into the skid
    /// turn on it, the skid turn is not left for Move while it is set, and the crouch walk pivots on it (RE/24 INPUT-7).
    pub quick_turn: bool,
    /// Data+568: k of the turn band (`HumanGround__UpdateHeading` 0xD95290), reset on every Ground entry.
    pub turn_k: f32,
    /// The foot column of Idle's wait (`HumanGround__PickLocomotionFoot` on the item Idle was entered from: 0 left
    /// ahead, 1 right ahead, 2 parallel), RE/02 §4.6.
    pub wait_foot: usize,
    /// HG+1500 while standing: the profile of the wait playing (`HumanGround__IdleWait_Enter` 0xD94580 takes the
    /// wanted one, HG+1504 = `high_profile`).
    pub wait_high: bool,
    /// HG+1500 through the FreeRun sub-state: `FreeRun_Enter` 0xD8B9D0 leaves it as the wait left it, and the FreeRun
    /// update (0xDAAE38) refreshes only HG+1504, so the run start out of it (`HumanGround__FreeRunStartMove` 0xD99A50)
    /// picks its item by the wait's profile.
    pub free_run_high: bool,
    /// HG+1524: the stance-change wait plays (RE/24 GROUND-14); `stance_timer` is its time still to play (HG+736).
    pub stance_change: bool,
    pub stance_timer: f32,
    /// Human+3056 … +3093: the step-up just made, while it holds (RE/24 GUIDE-15).
    pub step_up: Option<StepUp>,
    /// Which of the foot-IK switching states the ground was in last frame (RE/24 GROUND-16).
    pub ik_state: IkState,
    /// The walking upper-body fidget (RE/24 GROUND-18).
    pub fidget: Fidget,
}

/// The walking upper-body fidget, the "cycle breaker" (RE/24 GROUND-18). Only while HumanData +0x201 is set: the
/// HumanData constructor sets it, but the player's behaviour clears it (`BhvAssassin__SetupPlayerHuman` 0xAF29C0), so
/// the player never fidgets (RE/25 #67: +0x201 = 0 and HG+0x588 = 0 through 100 s of walking): `allowed` is false for
/// the player. When set, `HumanGround__Move_EnterBody` 0xD94BFA arms the timer HG+0x588 with Random(HumanData +0x204,
/// +0x208) = Random(6, 15) s (`HumanData__ctor` 0xB30330; confirmed live). `UpdateMoveBlend`'s tail (0xDA1FD4–0xDA20D0), on the
/// locomotion path: when the playing locomotion item becomes item 1 (HG+0x7C0 the last one), in the walk band (HG+0x5DC
/// = 0: s ≤ 0.25) with the timer run out, it posts event 2 to the upper-body FSM (HG+0x580: `sub_D82560`) and re-arms
/// the timer; at s ≥ 0.75 it posts event 4 (`sub_D826F0`), which ends the fidget. Event 2's variant is HumanData +0x202:
/// 0 for the player (an NPC controller, 0xE8DE40, sets 1: the pickpocket walk cycle breakers, **hypothesis** that the
/// player never gets it). The FSM accepts it only while enabled (events 0 / 1 from the same ground states that switch
/// the foot IK: `IkState::On`), with nothing of higher priority playing (+32 ≤ 1: nothing in free roam) and not already
/// fidgeting. The action played (`ground_tree::CYCLE_BREAKER`) is the animation's to pick.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fidget {
    /// HumanData +0x201: the cycle breakers are allowed at all (false for the player).
    pub allowed: bool,
    /// HG+0x588: time until the next fidget may start.
    pub timer: f32,
    /// Bumped at every event 2 (the animation starts a fidget).
    pub seq: u32,
    /// Event 4 or 0 this frame: a running fidget ends.
    pub stop: bool,
    /// HG+0x7C0: the locomotion item of the last MoveBlend run.
    prev_item: usize,
    moving: bool,
    rng: u32,
}

impl Fidget {
    const RANGE: (f32, f32) = (6.0, 15.0);
    const WALK: f32 = 0.25;
    const STOP: f32 = 0.75;

    /// `Random__Range` (the game's LCG, 1664525·x + 1013904223, 28 bits).
    fn random(&mut self, lo: f32, hi: f32) -> f32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        lo + (hi - lo) * (self.rng & 0x0FFF_FFFF) as f32 / 0x1000_0000 as f32
    }

    /// One frame: `moving` = the Move state, `locomotion` = MoveBlend's locomotion path (no transition action in front),
    /// `item` the playing locomotion item, `s` the speed parameter, `enabled` the upper-body FSM's state.
    pub fn update(&mut self, moving: bool, locomotion: bool, item: usize, s: f32, enabled: bool, dt: f32) {
        if moving && !self.moving && self.allowed {
            // `Move_EnterBody`: the timer armed on every Move entry while HumanData +0x201 is set
            self.timer = self.random(Self::RANGE.0, Self::RANGE.1);
        }
        self.moving = moving;
        self.timer = (self.timer - dt).max(0.0);
        self.stop = !enabled;
        if !(moving && locomotion) {
            return;
        }
        if item != self.prev_item && item == 1 && s <= Self::WALK && self.timer <= 0.0 && enabled && self.allowed {
            self.seq = self.seq.wrapping_add(1);
            self.timer = self.random(Self::RANGE.0, Self::RANGE.1);
        }
        self.prev_item = item;
        self.stop |= s >= Self::STOP;
    }
}

/// The ground states that switch the standing foot IK and the stick-to-ground smoothing (`crate::ik::GroundIkSwitch`;
/// RE/24 GROUND-16). Each turns them off over 0.2 s on entry and back on in its exit; the other states (Idle, Move, the
/// run stop, the skid turn, the pivot, the look-down, the ledge stop, the crouch, the static jump's preparation) leave
/// them as they are. Entering the ground turns them on over 0.2 s (`HumanGround__OnActivateSetup` 0xDAE949), leaving it
/// off over 0.2 s (state 1's exit 0xD97BA0, the hand-overs 0xD97920, 0xDAC8B0 …).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IkState {
    #[default]
    On,
    /// States 21 / 22 (`Movement_Enter` 0xDA8890), exits `sub_D85330` / `sub_D99260`: on over 0.35 s (the landing's
    /// `LandingToMove_Layout6` 0xD990A0 asks for 0.3 s after it, too late: only the first call acts).
    Landing,
    /// State 20 (0xDA8890), exit `sub_D85270`: on at once.
    RollEnding,
    /// State 40 (`HumanGround__ObstacleCollision_Enter` 0xD9CBF6), exit `sub_D83DC0`: on at once.
    Lean,
    /// State 36, FreeRun (`sub_D8B9D0`), exit `sub_D853F0`: on over 0.2 s (also on the way into the static jump's
    /// preparation, `sub_D8E690`).
    FreeRun,
    /// Another context.
    Away,
}

impl IkState {
    fn of(g: &HumanGroundData) -> Self {
        match g.owner {
            TreeState::Landing | TreeState::DamageLanding if g.oneshot.is_some() => return Self::Landing,
            TreeState::RollEnding if g.oneshot.is_some() => return Self::RollEnding,
            _ => {}
        }
        if g.collide.is_some() {
            Self::Lean
        } else if g.extra.is_some_and(|e| e.mode == super::ground_extras::ExtraMode::FreeRun) {
            Self::FreeRun
        } else {
            Self::On
        }
    }

    /// The exit's and the entry's calls when the state changes.
    pub fn switch(self, to: Self, sw: &mut crate::ik::GroundIkSwitch, dt: f32) {
        if self == to {
            return;
        }
        match self {
            Self::Landing => sw.on(0.35),
            Self::RollEnding | Self::Lean => sw.on(0.0),
            Self::FreeRun => sw.on(0.2),
            Self::On | Self::Away => {}
        }
        match to {
            Self::Landing | Self::RollEnding | Self::Lean | Self::FreeRun | Self::Away => sw.off(0.2, dt),
            Self::On => {}
        }
    }
}

/// The step-up tracker (`Human__PostUpdate` 0xB2708D → `sub_B26A30`): a stick-to-ground snap up by more than 0.25 m
/// (controller residual +384 < −0.25, step offset on) keeps the cast's contact (+3056 point, +3072 normal) and the flat
/// distance to it (+3088). It holds (`sub_B26940`: +3092 / +3093) while the character is on the normal's side of the
/// contact (horizontally) and no more than 5 cm further from it than its closest; while it holds, the step off an edge
/// (event 68, `HumanGround__Guard_Event68_JumpToEdge` 0xD84190 via `sub_B0EBC0`) does not start.
#[derive(Clone, Copy, Debug)]
pub struct StepUp {
    pub contact: Vec3,
    pub normal: Vec3,
    pub dist: f32,
}

impl StepUp {
    const RESIDUAL: f32 = -0.25;
    const SLACK: f32 = 0.05;

    fn flat(v: Vec3) -> Vec3 {
        Vec3::new(v.x, 0.0, v.z)
    }

    /// The snap of this frame: a new step-up when the feet were lifted more than 0.25 m.
    pub fn capture(residual: f32, contact: Option<(Vec3, Vec3)>, feet: Vec3) -> Option<Self> {
        let (contact, normal) = contact.filter(|_| residual < Self::RESIDUAL)?;
        Some(Self { contact, normal, dist: Self::flat(feet - contact).length() })
    }

    /// One frame of `sub_B26A30` after the capture: the closest distance so far, then whether it still holds.
    pub fn hold(mut self, feet: Vec3) -> Option<Self> {
        let d = Self::flat(feet - self.contact);
        self.dist = self.dist.min(d.length());
        (Self::flat(self.normal).dot(d) >= 0.0 && d.length() <= self.dist + Self::SLACK).then_some(self)
    }
}

/// The Movement child states that end a one-shot by their own rules (`HumanGround__Movement_Update` 0xDAD1C0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TreeState {
    #[default]
    Other,
    /// State 21 (entry mode 9, `HumanGround__Landing_Update` 0xDA8F50).
    Landing,
    /// State 22 (entry mode 10, `HumanGround__DamageLanding_Update` 0xDA8FC0).
    DamageLanding,
    /// State 18 (`HumanGround__RunStop_Update` 0xDA8D70).
    RunStop,
    /// State 24, the skid turn (`HumanGround__RunTurn_Update` 0xDA90A0).
    RunTurn,
    /// State 25 (`HumanGround__Pivot_Update` 0xDA9120).
    Pivot,
    /// State 20 (entry mode 12, `HumanGround__RollEnding_Update` 0xDA8EA0): the pass-over's roll onto the ground.
    RollEnding,
}

/// `HumanGround__LookDown_Enter` 0xD9FC80: `xx_l_ledge_lookdown_{front,left,right}_foot{l,r}` (by the leading foot)
/// blended by the angle between the facing and the point 1 m past the edge (report point + normal): [1 − k, left k,
/// right k], k = min(|a|, 90°) / 90°.
pub const LOOK_DOWN: [u32; 2] = [0x2669_E0F7, 0x2669_E0F8];

/// Pivot actions (table 0x1A2C120, filled by 0xDB6E50): [left, right] x [from low, high] x [to low, high] x [foot l, r],
/// each `*_wait_hipm_foot?_to_?_waitturn_{left,right}_{090,180}_foot?` (2 clips).
pub const PIVOT: [[[[u32; 2]; 2]; 2]; 2] = [
    [[[0x082F_BC7C, 0x082F_BC87], [0x1ABA_2384, 0x1ABA_2385]], [[0x1ABA_2388, 0x1ABA_2389], [0x09A0_9DF1, 0x09A0_A217]]],
    [[[0x082F_BC88, 0x082F_BC89], [0x1ABA_2386, 0x1ABA_2387]], [[0x1ABA_238A, 0x1ABA_238B], [0x09A0_9DF2, 0x09A0_A218]]],
];
/// Start from standing (`HumanGround__PlayStartMove` 0xD98990): the locomotion action entered through these items,
/// [low, high profile] x [foot l, r] (by `Human__GetLeadingFoot` 0xB18850). Clips [wait_tr_walk_slow, wait_tr_walk,
/// wait_tr_jog, impulsion_to_sprint]; the exe weights walk (low) or jog (high) and sets the speed parameter HG+0x5E8 to
/// 0.25 / 0.5 at once.
pub const START_MOVE: [[u32; 2]; 2] = [[0x09A0_8AC6, 0x09A0_A426], [0x09A0_AA2D, 0x09A0_AA2E]];

pub fn is_start(id: u32) -> bool {
    START_MOVE.iter().flatten().any(|&s| s == id)
}

// Probe action list also includes the native Ground extras; kept here for the existing dump tool.
pub const PIVOT_ACTIONS: [u32; 37] = [
    super::ground_extras::FREERUN_WAIT[0], super::ground_extras::FREERUN_WAIT[1],
    super::ground_extras::STATIC_PREPARE[0], super::ground_extras::STATIC_PREPARE[1],
    super::ground_extras::STATIC_JUMP[0], super::ground_extras::STATIC_JUMP[1], super::ground_extras::STATIC_FALL,
    super::ground_extras::CROUCH_WAIT[0][0], super::ground_extras::CROUCH_WAIT[0][1],
    super::ground_extras::CROUCH_WAIT[1][0], super::ground_extras::CROUCH_WAIT[1][1],
    super::ground_extras::CROUCH_START[0], super::ground_extras::CROUCH_START[1],
    super::ground_extras::CROUCH_WALK, super::ground_extras::CROUCH_PIVOT,
    super::ground_extras::CROUCH_HIDE_ENTER[0], super::ground_extras::CROUCH_HIDE_ENTER[1],
    0x09A0_8AC6, 0x09A0_A426, 0x09A0_AA2D, 0x09A0_AA2E,
    0x082F_BC7C, 0x082F_BC87, 0x1ABA_2384, 0x1ABA_2385, 0x1ABA_2388, 0x1ABA_2389, 0x09A0_9DF1, 0x09A0_A217,
    0x082F_BC88, 0x082F_BC89, 0x1ABA_2386, 0x1ABA_2387, 0x1ABA_238A, 0x1ABA_238B, 0x09A0_9DF2, 0x09A0_A218,
];

#[derive(Clone, Copy, Debug)]
pub struct LookDown {
    pub action: ActionBlend,
    pub t: f32,
    pub point: Vec3,
    pub normal: Vec3,
}

/// The look-down weights for an edge at `p` / outward normal `n` seen from `feet` facing `forward`.
pub fn look_down_weights(feet: Vec3, forward: Vec3, p: Vec3, n: Vec3) -> [f32; 3] {
    let to = Vec3::new(p.x + n.x - feet.x, 0.0, p.z + n.z - feet.z).normalize_or_zero();
    let a = to.dot(forward).clamp(-1.0, 1.0).acos();
    let k = (a.min(std::f32::consts::FRAC_PI_2) / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
    // left when the edge is on the character's left
    if to.dot(super::right_of(forward)) < 0.0 { [1.0 - k, k, 0.0] } else { [1.0 - k, 0.0, k] }
}

/// `HumanGround__LedgeStop_Enter` 0xD93C60 / `HumanGround__LedgeStop_PlayEnd` 0xD7D9D0.
#[derive(Clone, Copy, Debug)]
pub struct LedgeStop {
    /// Edge report: point (+16) and outward normal (+32).
    pub point: Vec3,
    pub normal: Vec3,
    /// The end action is playing (the start action, 0x06E8BD7F, is done).
    pub ending: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct GroundOneShot {
    pub blend: ActionBlend,
    pub t: f32,
    pub duration: f32,
    /// Displacement already applied (animation space).
    pub applied: [f32; 3],
    /// Heading when the action started: its displacement and root yaw are in this frame (set on the first update).
    pub h0: Option<f32>,
}

/// What the impulsion jump takes off to: the stored target, or the free target of a jump without one (IHuman vt68
/// mode 0 -> IHumanGround vt28 `HumanGround__JumpToTargetOfType` 0xD832F0 -> vt24, event 45 like a target: 8 m along
/// the searched direction, 3 m down, type 1).
#[derive(Clone, Copy, Debug)]
pub enum ImpulsionJump {
    Target(JumpTarget),
    Free { dir: Vec3 },
}

/// Which of event 45's guards takes a jump to a target (`HumanGround__Movement_HandleEvent` 0xDB19BE; FreeRun 0xDAB32E).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event45 {
    /// `sub_D7CA90` -> `HumanGround__StartRunJumpToTarget` 0xD837A0 (kind 0).
    Run,
    /// `sub_D83830` / FreeRun's `sub_D85690` -> the impulsion action, then the free-step jump.
    Impulsion,
    /// The playing item is locked (word bit 0x20): neither guard takes it.
    Refused,
}

impl HumanGroundData {
    /// Event 45, the jump to a target (IHuman vt24 from the tap jump, the free run's edge jump and the free-run target
    /// jump). The guards read the playing item:
    /// - locked (word & 0x20) -> refused;
    /// - FreeRun (`free_run`, its guard `sub_D85690`) -> `sub_DA27C0`: the impulsion from its start, blended by the
    ///   target's height v = clamp(dz, 0, 1.3) / 1.3 and its side (the signed angle a from the facing, + right): |a| < 90
    ///   deg blends front with right / left by |a| / 90 deg, beyond it right / left with back-right / back-left;
    /// - Movement (`sub_D83830`): standing (HG+1492 = 0) -> the impulsion at phase 1; the locomotion blend 0x05923BDB ->
    ///   the run jump (`sub_D7CA90`); another item (a start or transition) at normalised time t < 0.75 -> the impulsion
    ///   from phase t / 0.75 (HG+616, `sub_501D10` 0xD9C092), else the run jump. `sub_D9BE90`: front only, by height.
    ///
    /// Live (lab): from the impulsion wait 0.27 s of impulsion then `freestep_front_front_*_to_air`; from a walk start or
    /// a free-step exit 0.05-0.09 s; from the running locomotion `run_front_*_to_air`. PORT: HG+616 is taken as the
    /// impulsion's start phase (`sub_724500`'s node call is not decoded; the timings fit it).
    pub fn event45(&mut self, feet: Vec3, forward: Vec3, jump: ImpulsionJump, free_run: Option<ActionBlend>, leading: Option<usize>) -> Event45 {
        // `free_run`: the FreeRun sub-state's playing item (its wait, `ground_extras::FREERUN_WAIT`): the port starts the
        // run start in the frame the wait hands over, so the item event 45 reads in the game is passed in
        let free_run_item = free_run;
        let free_run = free_run.is_some();
        let target_pos = match jump {
            ImpulsionJump::Target(t) => t.position,
            ImpulsionJump::Free { dir } => feet + Vec3::new(dir.x, 0.0, dir.z).normalize_or(forward) * FREE_JUMP_AHEAD - Vec3::Y * crate::tuning::FREE_JUMP_DOWN,
        };
        let item = self.tr.map(|t| (t.action, t.phase)).or_else(|| self.oneshot.map(|o| (o.blend, o.t / o.duration.max(1e-4))));
        if item.is_some_and(|(b, _)| super::anim_gate::locked(&b)) {
            return Event45::Refused;
        }
        let v = (target_pos.y - feet.y).clamp(0.0, 1.3) / 1.3;
        let mut w = [0.0f32; 10];
        let phase = if free_run {
            let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
            let d = Vec3::new(target_pos.x - feet.x, 0.0, target_pos.z - feet.z).normalize_or(f);
            let a = d.dot(super::right_of(f)).atan2(d.dot(f));
            let q = std::f32::consts::FRAC_PI_2;
            let (slots, k) = if a < -q {
                ([2, 3, 6, 7], (-a - q) / q)
            } else if a < 0.0 {
                ([0, 1, 2, 3], -a / q)
            } else if a < q {
                ([0, 1, 4, 5], a / q)
            } else {
                ([4, 5, 8, 9], (a - q) / q)
            };
            let k = k.clamp(0.0, 1.0);
            w[slots[0]] = (1.0 - v) * (1.0 - k);
            w[slots[1]] = v * (1.0 - k);
            w[slots[2]] = k * (1.0 - v);
            w[slots[3]] = k * v;
            0.0
        } else {
            w[0] = 1.0 - v;
            w[1] = v;
            match item {
                Some((_, t)) if t >= 0.75 => return Event45::Run,
                Some((_, t)) => (t / 0.75).max(0.0),
                None => return Event45::Run,
            }
        };
        // the foot: Movement's `sub_D9BE90` takes `Human__GetLeadingFoot` == 1 -> footl (0xD9BF5A), FreeRun's
        // `HumanGround__PlayImpulsionJumpDirectional` 0xDA2830 the playing item's word bits 2-3 == 1 -> footl
        // (0xDA285E, 0x040C793D; else 0x04A943DE). The playing item is the transition or one-shot in front, else the
        // locomotion's: item 0 (`footl`, word 0x0FC9) ends right ahead → footr, item 1 (0x0FC6) left ahead → footl
        // (live 2026-10-18, RE/33 B1: the impulsion before the stall). Without a pose, Movement takes the locomotion's
        // foot (live: 028 and freerun_north take footl from Movement with foot 1).
        let left = if free_run {
            let word = match (free_run_item, item) {
                (Some(b), _) => super::anim_gate::word(&b),
                (None, Some((b, _))) => super::anim_gate::word(&b),
                (None, None) => super::anim_gate::item_word(super::move_blend::ACT_GROUND_LOCOMOTION, self.blend.foot),
            };
            word.is_some_and(|w| (w >> 2) & 3 == 1)
        } else {
            leading.map_or(self.blend.foot != 0, |f| f == 0)
        };
        let id = jump_blend::IMPULSION_JUMP[(!left) as usize];
        let Some(n) = jump_blend::action_items(id).and_then(|i| i.first().map(|c| c.len())) else { return Event45::Run };
        let b = ActionBlend::new(id, 0, &w[..n.min(10)]);
        self.play_oneshot(b);
        if let Some(os) = self.oneshot.as_mut() {
            os.t = phase * os.duration;
            os.applied = b.disp(phase);
        }
        self.owner = TreeState::Other;
        self.tr = None;
        self.impulsion_jump = Some(jump);
        Event45::Impulsion
    }

    /// Play a ground one-shot action with its root motion (landings, receptions, ledge stop).
    pub fn play_oneshot(&mut self, b: ActionBlend) {
        self.landing_seq = self.landing_seq.wrapping_add(1);
        self.oneshot = Some(GroundOneShot { blend: b, t: 0.0, duration: b.duration(), applied: [0.0; 3], h0: None });
        self.owner = TreeState::Other;
    }

    fn next_tr_seq(&mut self) -> u32 {
        self.tr_seq = self.tr_seq.wrapping_add(1);
        self.tr_seq
    }

    /// RunStop (state 18, `HumanGround__RunStop_Enter` 0xD98E30): the stop action of the leading foot (table 0x1A2C074 =
    /// legacy 86 / 87), weighted by the speed it stopped from. MoveBlend does not run while it plays.
    /// `HumanGround__RunStop_Enter` 0xD98E30: the stop of the foot `PickLocomotionFoot` 0xD86760 reads off the playing
    /// item's word (the locomotion's, or the transition in front of it; RE/24 GROUND-7). Weights: with the locomotion
    /// (or the skid-turn exits, legacy 96 / 97) playing [0, 1 − f, f] by the speed; with another action
    /// [1 − v, v, 0], v = clamp((t − 0.33) / 0.67) of its normalised time (GROUND-8). The stop table dword_1A2C074
    /// (`HumanGround__InitAnimTables` 0xDB6F4E) holds legacy 86 / 87 for feet 0 / 1; its parallel-feet slot (2) stays −1
    /// (0xDB6E6A), so `Anim__PlayAction`(−1) plays no stop. The locomotion's items never report parallel feet (words
    /// 0x0FC9 / 0x0FC6); PORT: behind a transition that does, the right-foot stop plays.
    pub fn start_run_stop(&mut self) {
        let word = match self.tr {
            Some(t) => super::anim_gate::word(&t.action),
            None => super::anim_gate::item_word(super::move_blend::ACT_GROUND_LOCOMOTION, self.blend.foot),
        };
        let foot = super::ground_tree::pick_locomotion_foot(word, self.blend.foot).min(1);
        let weights = match self.tr.filter(|t| !super::ground_tree::RUN_TURN_EXIT.contains(&t.action.id)) {
            Some(t) => {
                let v = ((t.phase - 0.33).max(0.0) / 0.67).clamp(0.0, 1.0);
                [1.0 - v, v, 0.0]
            }
            None => jump_blend::run_stop_weights(self.speed_param),
        };
        if let Some(b) = jump_blend::action_items(jump_blend::RUN_STOP[foot]).map(|_| ActionBlend::new(jump_blend::RUN_STOP[foot], 0, &weights)) {
            self.play_oneshot(b);
            self.oneshot_next = None;
            self.tr = None;
            self.owner = TreeState::RunStop;
            self.speed_param = 0.0;
            self.blend.speed_param = 0.0;
        }
    }

    /// The transition action `action` (its item `item`) in front of the locomotion, handing over to locomotion item
    /// `dest`. `prev`: the weights the slot holds (the item that was playing), kept when the transition has as many
    /// clips (the slot's single weight array, `sub_725B30`).
    fn start_transition(&mut self, action: u32, item: usize, dest: usize, layout: u8, prev: &[f32], from: Option<(u32, usize)>) -> bool {
        let Some(items) = jump_blend::action_items(action) else { return false };
        let n = items.get(item).map(|c| c.len()).unwrap_or(0);
        if n == 0 {
            return false;
        }
        let mut w: Vec<f32> = if prev.len() == n { prev.to_vec() } else { (0..n).map(|k| if k == 0 { 1.0 } else { 0.0 }).collect() };
        // the layout's weights from the first frame (the exe sets them in the same update that plays the action)
        if let Some(lw) = super::ground_tree::transition_weights(layout, self.speed_param, self.pivot_turn, &w) {
            w = (0..n).map(|k| lw.get(k).copied().unwrap_or(0.0).clamp(0.0, 1.0)).collect();
        }
        let seq = self.next_tr_seq();
        self.tr = Some(super::ground_tree::MoveTransition::new(ActionBlend::new(action, item, &w), layout, dest, from, seq));
        true
    }

    /// `HumanGround__Move_EnterBody` 0xD94A60 entered from `from` without a start: the locomotion through the item's
    /// authored transition (the graph's lookup, `sub_5B98B0`), with the given MoveBlend layout. Without one the
    /// locomotion starts at once on the exit foot of the item (`HumanGround__PickLocomotionFoot` 0xD86760).
    fn enter_move(&mut self, from: ActionBlend, layout: u8) {
        use super::ground_tree::{authored_transition, pick_locomotion_foot};
        self.reset_move_entry();
        match authored_transition(from.id, from.item, super::move_blend::ACT_GROUND_LOCOMOTION) {
            Some((ta, ua, ub)) if self.start_transition(ta, ua, ub, layout, from.weights(), Some((from.id, from.item))) => {}
            _ => {
                self.tr = None;
                self.blend.foot = pick_locomotion_foot(super::anim_gate::word(&from), self.blend.foot).min(1);
                self.blend.phase = 0.0;
            }
        }
    }

    /// Idle entered from `from` (`HumanGround__Idle_Enter` 0xDA6AB0): the wait of the profile, through the item's authored
    /// transition into it when there is one (its root motion still moves the body); the foot follows that wait.
    fn enter_wait(&mut self, from: ActionBlend, high: bool) {
        self.speed_param = 0.0;
        self.blend.speed_param = 0.0;
        self.tr = None;
        let waits = super::ground_tree::WAITS[high as usize];
        // Idle's wait column: the exit foot of the item it is entered from (0xD94580 → 0xD86760)
        self.set_wait_foot(super::ground_tree::pick_locomotion_foot(super::anim_gate::word(&from), self.blend.foot));
        let parallel = super::ground_tree::WAITS_PARALLEL[high as usize];
        let found = super::jump_clips::GROUND_TRANSITIONS.iter().find(|t| t.0 == from.id && t.1 == from.item && (waits.contains(&t.4) || t.4 == parallel));
        if let Some(&(_, _, ta, ua, dest, _)) = found {
            if self.start_transition(ta, ua as usize, 0, 0, from.weights(), Some((from.id, from.item))) {
                if let Some(t) = self.tr.as_mut() {
                    t.to_wait = true;
                }
                self.set_wait_foot(if dest == parallel { 2 } else { (dest == waits[1]) as usize });
            }
        }
    }

    /// `HumanGround__Idle_EnterFromMove` 0xD8B220: the wait of the leading foot (`lead`, 0 = left ahead) in the wanted
    /// profile, played through the accel transition the code passes (`ground_tree::IDLE_FROM_MOVE`, blend type 1, 0.2 s,
    /// from B; it has no root motion).
    /// `coast`: the walk's speed, whose displacement fades out over the blend (BLENDAB).
    fn idle_from_move(&mut self, lead: usize, coast: f32) {
        self.set_wait_foot(lead);
        let id = super::ground_tree::IDLE_FROM_MOVE[self.high_profile as usize][lead.min(1)];
        if self.start_transition(id, 0, 0, 0, &[], None) {
            if let Some(t) = self.tr.as_mut() {
                t.to_wait = true;
                t.coast = coast;
                t.blend_in = Some(crate::assets::ac_actions::ActBlend { kind: 1, acuator: 1, time: 0.2, ..crate::assets::ac_actions::ActBlend::DEFAULT });
            }
        }
    }

    /// Idle's wait column; a foot ahead is also the leading foot the next start uses.
    pub fn set_wait_foot(&mut self, foot: usize) {
        self.wait_foot = foot.min(2);
        if foot < 2 {
            self.blend.foot = foot;
        }
    }

    /// Entry mode 12 (`HumanLedge__SetupPassOverToGround` 0xDCFA40 → `Movement_EnterBody` 0xD92C40 case 12): state 20, high
    /// profile, s 0.5, the roll ending of the foot the vault's item word gives (Data+556 = 2 − ((word & 0xC) ≠ 4), read
    /// by `PickLocomotionFoot` 0xD86760: 1 → foot 0, 2 → foot 1; table 0x1A2C0D4) (RE/24 GROUND-4).
    pub fn enter_roll_ending(&mut self, vault_word: Option<u16>) {
        use super::ground_tree::ROLL_ENDING;
        let foot = match vault_word {
            Some(w) if w & 0xC == 4 => 1,
            _ => 0,
        };
        if jump_blend::action_items(ROLL_ENDING[foot]).is_none() {
            return;
        }
        self.high_profile = true;
        self.speed_param = 0.5;
        self.blend.speed_param = 0.5;
        self.play_oneshot(ActionBlend::new(ROLL_ENDING[foot], 0, &[1.0]));
        self.owner = TreeState::RollEnding;
    }

    /// The free-step stay's Ground setup (`HumanNarrowObject__CheckSupportAndFall` 0xE51914 →
    /// `TransitionSetupDataToMovement::Apply` 0xC80310 → `Movement_EnterBody` 0xD92C40, RE/05 §2.15): entry mode 0 / 1 →
    /// Idle in the low / high profile, the wait through the stay's item's authored transition
    /// (`freestep_entry_tr_{l,h}_wait`); 3 / 4 / 6 → Move with s 0.25 / 0.5 / 1.0, the locomotion through
    /// `freestep_entry_tr_{l_walk, h_jog, h_sprint_impultion}` (layout 1, promoted to 5 by mode 6). The transition starts
    /// at once, also from the middle of the stay's action (live RE/25 roof_jogoff_high: mode 1 at its phase 0.5;
    /// crate_probe: mode 0; freerun_north: mode 3).
    pub fn enter_from_free_step(&mut self, from: ActionBlend, mode: u8) {
        match mode {
            0 | 1 => {
                let high = mode == 1;
                self.high_profile = high;
                self.wait_high = high;
                self.enter_wait(from, high);
            }
            _ => {
                let s = match mode {
                    3 => 0.25,
                    4 => 0.5,
                    _ => 1.0,
                };
                self.high_profile = mode != 3;
                self.speed_param = s;
                self.blend.speed_param = s;
                self.enter_move(from, if mode == 6 { 5 } else { 1 });
            }
        }
    }

    /// `HumanGround__Move_EnterBody` 0xD94A60 on every Move entry: the MoveBlend slowdown (HG+1740) and settle (HG+1744)
    /// timers and the lean (Data+212) start from 0 (RE/24 GROUND-13).
    fn reset_move_entry(&mut self) {
        self.blend.slowdown = 0.0;
        self.blend.settle = 0.0;
        self.blend.lean = 0.0;
    }

    /// `HumanGround__PlayStartMove` 0xD98990 (Idle → Move): the locomotion entered through the start transition of the
    /// profile and leading foot (0.2 s AROLLBROLL from B only), weights walk (low) / jog (high), the speed parameter
    /// set to 0.25 / 0.5 and layout 7. A left foot ahead hands over to locomotion item 1.
    pub fn play_start_move(&mut self) {
        self.reset_move_entry();
        let foot = (self.blend.foot != 0) as usize;
        let id = START_MOVE[self.high_profile as usize][foot];
        if jump_blend::action_items(id).is_none() {
            return;
        }
        let w: &[f32] = if self.high_profile { &[0.0, 0.0, 1.0, 0.0] } else { &[0.0, 1.0, 0.0, 0.0] };
        self.speed_param = if self.high_profile { 0.5 } else { 0.25 };
        self.blend.speed_param = self.speed_param;
        if !GAME_GROUND_TREE {
            self.play_oneshot(ActionBlend::new(id, 0, w));
            return;
        }
        let seq = self.next_tr_seq();
        let mut t = super::ground_tree::MoveTransition::new(ActionBlend::new(id, 0, w), 7, 1 - foot, None, seq);
        t.blend_in = Some(crate::assets::ac_actions::ActBlend { kind: 1, disp_src: 2, time: 0.2, ..crate::assets::ac_actions::ActBlend::DEFAULT });
        self.tr = Some(t);
    }

    /// FreeRun → Move (`HumanGround__FreeRunStartMove` 0xD99A50, from the FreeRun update 0xDAAE84 once HG+520 has run
    /// out, 0.2 s after `FreeRun_Enter`, with the wanted move mode HG+1496 = 2): the start item of HG+1500 (the wait's
    /// profile, `free_run_high`) and the leading foot as `PlayStartMove`, the same 0.2 s blend, but weighted all on its
    /// fourth clip, `xx_h_impultionstraight_foot?_to_sprint_impultion` ([0, 0, 0, 1], 0xD99D48), and Movement entered in
    /// mode 6 (`TransitionSetupDataToMovement` 6, layout 7: s := 1.0, HG+1500 := 1, `Movement_EnterBody` 0xD92E1D).
    /// Live (RE/26 §6.7, freerun_east): the low start's action with the sprint impulsion's root speeds, 1.10 → 5.84 m/s.
    /// Its other branch (Data+284 set in the low profile: the social crouch request, not produced here) is not ported.
    pub fn play_free_run_start(&mut self) {
        self.reset_move_entry();
        let foot = (self.blend.foot != 0) as usize;
        let id = START_MOVE[self.free_run_high as usize][foot];
        if jump_blend::action_items(id).is_none() {
            return;
        }
        let w: &[f32] = &[0.0, 0.0, 0.0, 1.0];
        self.speed_param = 1.0;
        self.blend.speed_param = 1.0;
        if !GAME_GROUND_TREE {
            self.play_oneshot(ActionBlend::new(id, 0, w));
            return;
        }
        let seq = self.next_tr_seq();
        let mut t = super::ground_tree::MoveTransition::new(ActionBlend::new(id, 0, w), 7, 1 - foot, None, seq);
        t.blend_in = Some(crate::assets::ac_actions::ActBlend { kind: 1, disp_src: 2, time: 0.2, ..crate::assets::ac_actions::ActBlend::DEFAULT });
        self.tr = Some(t);
    }

    /// RunTurn → Move (`HumanGround__PlayRunTurnExitMove` 0xD8F2B0), s := 0.5. Still turned (more than 120 deg): the
    /// heading is turned round (the skid-turn clip turned the skeleton, not the root) and the turn's own exit plays
    /// (legacy 96 / 97); otherwise the run stop's exit (93 after the left-foot turn, 92 after the right) with a 0.2 s
    /// ASTOPBROLL.
    pub fn run_turn_exit(&mut self, turn_item: ActionBlend, turned: bool, heading: &mut f32) {
        use super::ground_tree::{RUN_STOP_EXIT, RUN_TURN, RUN_TURN_EXIT};
        let footl = turn_item.id == RUN_TURN[0];
        self.speed_param = 0.5;
        self.blend.speed_param = 0.5;
        let (action, dest) = if turned {
            *heading = wrap_angle(*heading + std::f32::consts::PI);
            if footl { (RUN_TURN_EXIT[0], 0) } else { (RUN_TURN_EXIT[1], 1) }
        } else if footl {
            (RUN_STOP_EXIT[1], 0)
        } else {
            (RUN_STOP_EXIT[0], 1)
        };
        if self.start_transition(action, 0, dest, 1, &[], Some((turn_item.id, turn_item.item))) && !turned {
            if let Some(t) = self.tr.as_mut() {
                t.blend_in = Some(crate::assets::ac_actions::ActBlend { kind: 2, time: 0.2, ..crate::assets::ac_actions::ActBlend::DEFAULT });
            }
        }
    }

    /// What follows a ground one-shot that ends, by the Movement state that owns it (RE/02 §4.6). `stick`: a move is
    /// wanted (HG+1496 ≠ 0); `turn`: HG+1532.
    pub fn after_oneshot(&mut self, ended: ActionBlend, stick: bool, turn: f32, heading: &mut f32) {
        use super::ground_tree::{pick_locomotion_foot, LANDINGS_TO_WAIT, LANDING_DAMAGE, RUN_STOP_EXIT, RUN_TURN};
        let owner = std::mem::take(&mut self.owner);
        let high = self.high_profile;
        if std::env::var_os("AC_TREE_LOG").is_some() {
            info!("ground tree: {:#010x} ended, owner {owner:?}, stick {stick}, turn {:.0} deg, high {high}", ended.id, turn.to_degrees());
        }
        match owner {
            // state 21 (0xDA8F50): when the landing completes, the landings that end in the wait go to Idle (high
            // profile, 0xDA26C0); the others enter Move with layout 6 and s 0.25 / 0.5 / 1.0 by the wanted profile and
            // Sprint (0xD990A0). A released stick then leaves Move for Idle at once (0xD7ED30).
            // the landings' authored endings run into the high-profile wait only (e.g. 0x6E9C754A → 0x6E9C754B
            // `…_tr_h_wait_*` → 0x00D82508): the wait is the high one, and in low profile Idle's wait update changes back
            // through the stance-change wait (live RE/25: a low-profile landing off the ladder played `…_tr_h_wait_*_b`,
            // `xx_hl_wait_hipm_footr` 0.5 s, then `…_tr_l_wait`)
            TreeState::Landing if LANDINGS_TO_WAIT.contains(&ended.id) => {
                self.enter_wait(ended, true);
                self.wait_high = true;
            }
            // `Guard_LandingToMove` 0xD7F0F0 has no stick test: a completed landing always enters Move (layout 6); with
            // the stick released the next update's Move guards hand over to Idle (RE/24 GROUND-6)
            TreeState::Landing => {
                self.speed_param = if !high { 0.25 } else if !self.sprint { 0.5 } else { 1.0 };
                self.blend.speed_param = self.speed_param;
                self.enter_move(ended, 6);
            }
            // state 22 (0xDA8FC0): Idle (0xDA77F0) or Move with s 0.25 / 0.5 by profile and layout 1 (0xD7F210)
            TreeState::DamageLanding if stick => {
                self.speed_param = if high { 0.5 } else { 0.25 };
                self.blend.speed_param = self.speed_param;
                self.enter_move(ended, 1);
            }
            TreeState::DamageLanding => {
                let _ = LANDING_DAMAGE;
                self.enter_wait(ended, high);
            }
            // state 18 (0xDA8D70): the item ended with HG+1780 set or the stick more than 120 deg away (no stick test:
            // HG+1532 is 0 only with the stick released) → the skid turn (state 24, 0xD8B8E0, the turn of the exit
            // foot; RunStopToRunTurn 0xD9B933 clears HG+1780); else Idle (0xDA7710) or Move through legacy 92 / 93 with
            // s 0.5 (0xD8B3F0: the left-foot stop hands over to item 1)
            TreeState::RunStop if self.quick_turn || turn.abs() > crate::tuning::RUN_TURN_ANGLE => {
                self.quick_turn = false;
                let foot = pick_locomotion_foot(super::anim_gate::word(&ended), self.blend.foot).min(1);
                if jump_blend::action_items(RUN_TURN[foot]).is_some() {
                    self.play_oneshot(ActionBlend::new(RUN_TURN[foot], 0, &[1.0]));
                    self.owner = TreeState::RunTurn;
                }
            }
            TreeState::RunStop if stick => {
                let footl = ended.id == jump_blend::RUN_STOP[0];
                self.speed_param = 0.5;
                self.blend.speed_param = 0.5;
                let (action, dest) = if footl { (RUN_STOP_EXIT[0], 1) } else { (RUN_STOP_EXIT[1], 0) };
                self.reset_move_entry();
                self.start_transition(action, 0, dest, 1, &[], Some((ended.id, ended.item)));
            }
            // state 24 completed (0xDA90A0)
            TreeState::RunTurn if stick => self.run_turn_exit(ended, turn.abs() > crate::tuning::RUN_TURN_ANGLE, heading),
            TreeState::RunTurn => {
                // 0xD8F210: the heading turned round, Idle
                *heading = wrap_angle(*heading + std::f32::consts::PI);
                self.enter_wait(ended, high);
            }
            // state 25 (0xDA9120) → `HumanGround__PlayTurnStartMove` 0xD99710: the locomotion through the pivot's
            // authored transition, by the profile the pivot started in (HG+1500): high: layout 2, s 0.5; low: layout 3,
            // s 0.25 (slots 2 / 3), or with the high profile now wanted (HG+1504) s 0.5 (slots 4 / 5) at the faster rate
            // 2 − t (HG+1832 bit 0, 0xD99904; RE/24 GROUND-12). The pivot's root yaw has already turned the body (the
            // exe catches CurHeading up by HG+688 here).
            TreeState::Pivot if stick => {
                let start_high = self.pivot_high;
                let layout = if start_high { 2 } else { 3 };
                self.speed_param = if start_high || high { 0.5 } else { 0.25 };
                self.blend.speed_param = self.speed_param;
                self.enter_move(ended, layout);
                if !start_high && high {
                    if let Some(t) = self.tr.as_mut() {
                        t.fast_start = true;
                    }
                }
            }
            // state 20 (0xDA8EA0): moving → `PlayRollEndingExitMove` 0xD8B780 (s 0.5, 0x082FB27C into locomotion item 1,
            // setup mode 4, layout 1); else Idle through the roll ending's own transition into the wait
            TreeState::RollEnding if stick => {
                self.speed_param = 0.5;
                self.blend.speed_param = 0.5;
                self.reset_move_entry();
                self.start_transition(super::ground_tree::ROLL_ENDING_EXIT, 0, 1, 1, &[], Some((ended.id, ended.item)));
            }
            TreeState::Pivot | TreeState::RunStop | TreeState::RollEnding => self.enter_wait(ended, high),
            // the other one-shots (climb / ledge / hay exits, lean exits …): the item's authored transition into the
            // locomotion with the default layout, PORT s ≥ 0.25 / 0.5 (TransitionSetupDataToMovement modes 3 / 4, the
            // exits' own setups are not traced), or into the wait
            // (the ledge stop's end action hands back to Movement when it ends, `Locomotion_Update` 0xDAF2D0)
            TreeState::Other if self.ledge_stop.is_none_or(|l| l.ending) && self.collide.is_none() && self.look_down.is_none() => {
                if stick {
                    if super::ground_tree::authored_transition(ended.id, ended.item, super::move_blend::ACT_GROUND_LOCOMOTION).is_some() {
                        self.speed_param = self.speed_param.max(if high { 0.5 } else { 0.25 });
                        self.blend.speed_param = self.speed_param;
                        self.enter_move(ended, 1);
                    }
                } else {
                    self.enter_wait(ended, high);
                }
            }
            TreeState::Other => {}
        }
    }

    /// `TransitionSetupDataToMovement::Apply` (0xC80310), simplified.
    pub fn enter(&mut self, landing: Option<Landing>) {
        self.pending_landing = landing;
        // the other contexts run without the step offset: the tracker is off (0xB26A30's else branch)
        self.step_up = None;
        self.turn_k = PLAYER_TURN_BAND_K0;
        self.sub_state = HumanGroundSubState::Movement;
        self.ledge_stop = None;
        self.wall_start = None;
        self.impulsion_jump = None;
        self.collide = None;
        self.look_down = None;
        self.extra = None;
        self.crouch = None;
        self.oneshot_next = None;
        self.tr = None;
        // entry mode 9 (landing) / 10 (heavy landing) put Movement in states 21 / 22 (`Movement_EnterBody` 0xD92C40)
        self.owner = match landing.and_then(|l| l.action) {
            Some(a) if a.id == jump_blend::LAND_DAMAGE || a.id == jump_blend::LAND_DAMAGE_ROLL => TreeState::DamageLanding,
            Some(_) => TreeState::Landing,
            None => TreeState::Other,
        };
        // `Movement_EnterBody` 0xD92C40 sets the speed parameter HG+1512 for every entry mode: 0 for the plain entries
        // and the landings (modes 0/1/2/7/9/10/11/13), 0.25 for 3, 0.5 for 4/5/12 (RE/24 GROUND-5). The landing sets
        // the profile's speed when it hands over to Move (`LandingToMove_Layout6` 0xD990A0). PORT: every port entry
        // into the ground is one of the 0 modes (the 0.25 / 0.5 exits are not routed through entry modes).
        self.speed_param = 0.0;
        self.blend.speed_param = 0.0;
        if landing.is_none() {
            self.oneshot = None;
        }
        if let Some(l) = landing {
            self.landing_seq = self.landing_seq.wrapping_add(1);
            // a reception that started under InAir's root interpolation picks up where it is (its root motion so far
            // was replaced by the interpolation, RE/24 AIR-2)
            self.oneshot = l.action.map(|b| {
                let d = b.duration();
                let t = l.skip.min(d);
                GroundOneShot { blend: b, t, duration: d, applied: b.disp(t / d.max(1e-4)), h0: None }
            });
            self.last_landing = Some(l);
        }
    }
}

/// Heading kept in (-pi, pi] (it used to grow without bound with every turn).
fn wrap_angle(a: f32) -> f32 {
    angle_diff(a, 0.0)
}

pub fn heading_delta(a:f32,b:f32)->f32 { angle_diff(a,b) }

fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = (a - b) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    } else if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

#[allow(clippy::too_many_arguments)]
pub fn update_ground(
    time: Res<Time>,
    mut pad: ResMut<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    spawn: Res<SpawnPoint>,
    mut rig: ResMut<CameraRig>,
    abilities: Option<Res<super::abilities::AbilitySet>>,
    skeleton: Option<Res<crate::bone_pose::PoseSkeleton>>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle, Option<&crate::anim::AnimPlayer>, Option<&crate::bone_pose::BonePose>, Option<&mut crate::ik::GroundIkSwitch>), With<Player>>,
) {
    use super::abilities::Ability;
    let ab = super::abilities::of(abilities.as_deref());
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data, anim, pose, mut ik_switch) in &mut q {
        // `Human__GetLeadingFoot` 0xB18850 on last frame's pose (`bone_pose`); None without the install
        let leading_foot = skeleton.as_deref().zip(pose).filter(|(_, p)| p.seq > 0).and_then(|(s, p)| p.leading_foot(s));
        // the Head bone on last frame's pose (`Bone__GetWorldPosition` 0x4EC0E0 on Human+0x280), for the ladder grab test
        let head = skeleton.as_deref().zip(pose).filter(|(_, p)| p.seq > 0).and_then(|(s, p)| p.world_pos(s, crate::bone_pose::HEAD, &body));
        let g = &mut data.ground;
        if loco.current != ActorContextId::Ground {
            // left the ground: the foot IK and the smoothing go off (RE/24 GROUND-16)
            if let Some(sw) = ik_switch.as_deref_mut() {
                g.ik_state.switch(IkState::Away, sw, dt);
            }
            g.ik_state = IkState::Away;
            continue;
        }
        if loco.just_switched {
            // the entry work of the first run; the update itself is skipped only for the rest of the switching frame
            loco.just_switched = false;
            // `HumanGround__OnActivateSetup` 0xDAE949: the foot IK and the smoothing on over 0.2 s
            if let Some(sw) = ik_switch.as_deref_mut() {
                sw.on(0.2);
            }
            g.ik_state = IkState::On;
            if let Some(l) = g.pending_landing.take() {
                // drop > 3 m: camera shake (drop − 3) / 7 (0xE05940)
                if l.total_drop > 3.0 {
                    rig.shake = ((l.total_drop - 3.0) / 7.0).min(1.0);
                }
                if l.kind == LandingType::Fatal {
                    // PORT: immediate respawn until native fatal ragdoll/Dead and desynchronisation are implemented,
                    // as the game's teleport does it (`super::teleport`, RE/24 CENSUS-6): Idle in low profile, no landing
                    body.feet = spawn.0;
                    body.velocity = Vec3::ZERO;
                    body.proxy.manifold.clear();
                    g.enter(None);
                }
            }
            body.grounded = true;
            if loco.switched_this_frame {
                // the controller keeps integrating the velocity the switch left (no one-frame freeze)
                super::coast(&mut body, &collision, dt);
                continue;
            }
        }
        // the wall run's lead-in done: the wall run (PORT: a frame after its end; anything that took over drops it)
        if g.wall_start.is_some() && !g.oneshot.is_some_and(|o| o.blend.id == super::walling::GROUND_LEAD_IN && o.t < o.duration) {
            g.oneshot = None;
            if let Some((contact, normal, slope)) = g.wall_start.take() {
                let from = body.feet;
                switch_context(&mut loco, &mut data, TransitionSetup::ToWalling(super::walling::WallingEntry { contact, normal, slope, from, warp_duration: None }));
                continue;
            }
        }
        // the impulsion done: the free-step takeoff from the stored request (state +245's update 0xD8E390: the action
        // complete, `sub_501760` -> `HumanGround__StartFreeStepJumpToTarget` 0xD858A0, kind 1, the foot from the playing
        // item's word bits 2-3); anything that took over drops it
        if g.impulsion_jump.is_some() && !g.oneshot.is_some_and(|o| jump_blend::IMPULSION_JUMP.contains(&o.blend.id) && o.t < o.duration) {
            let os = g.oneshot.filter(|o| jump_blend::IMPULSION_JUMP.contains(&o.blend.id));
            let jump = g.impulsion_jump.take();
            if let (Some(os), Some(jump)) = (os, jump) {
                g.oneshot = None;
                // the item's foot bits as `Human__SetupJumpToTarget`'s a7: 0 -> `Human__GetLeadingFoot` 0xB18850 on the
                // pose (0xB20435; live 2026-10-18, freestep50_walkon: the impulsion's word has no foot bits and the
                // game takes off `freestep_front_front_050cm_footl` on the pose's leading foot); without a pose the
                // locomotion's foot (PORT)
                let foot_left = super::takeoff_foot_left(super::anim_gate::word(&os.blend), leading_foot, g.blend.foot != 0);
                let entry = match jump {
                    ImpulsionJump::Target(target) => {
                        jump_log("impulsion jump takeoff", body.feet, Some(&target));
                        InAirEntry::FreeStepJump { from: body.feet, target, foot_left }
                    }
                    ImpulsionJump::Free { dir } => {
                        jump_log("impulsion free jump takeoff", body.feet, None);
                        InAirEntry::FreeStepFreeJump { from: body.feet, dir, ahead: FREE_JUMP_AHEAD, down: crate::tuning::FREE_JUMP_DOWN, foot_left }
                    }
                };
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
                continue;
            }
        }
        let g = &mut data.ground;
        // the step-up tracker at last frame's end (`Human__PostUpdate` runs after the contexts; event 68 reads it)
        g.step_up = g.step_up.and_then(|s| s.hold(body.feet));
        // the foot IK switch of the state the ground is in (its entry or exit, RE/24 GROUND-16). PORT: read once per
        // frame, before the update, so a state's calls come at most a frame late
        let ik_now = IkState::of(g);
        if let Some(sw) = ik_switch.as_deref_mut() {
            g.ik_state.switch(ik_now, sw, dt);
        }
        g.ik_state = ik_now;

        if GAME_GROUND_EXTRAS {
            if g.crouch.is_none() && g.crouch_requested && !pad.high_profile && g.extra.is_none()
                && g.oneshot.is_none() && g.collide.is_none() && g.ledge_stop.is_none()
            {
                g.pose_seq=g.pose_seq.wrapping_add(1);
                g.crouch=Some(if pad.speed01>0.0 && g.speed_param>0.0 {
                    super::crouch::Crouch::enter_moving(g.blend.foot,g.crouch_hide_hint,g.pose_seq)
                } else {super::crouch::Crouch::enter(g.blend.foot,g.crouch_hide_hint,g.pose_seq)});
                g.look_down=None;g.speed_param=0.0;g.blend.speed_param=0.0;
            }
            if let Some(mut crouch)=g.crouch {
                let (item_clock,blend_progress)=if let Some(anim)=anim {
                    (Some((anim.contacts.t,anim.phase)),
                        (anim.fade<anim.fade_time && anim.fade_time>0.0).then(||anim.fade/anim.fade_time))
                } else { (Some((crouch.time,crouch.phase())),None) };
                let moving=pad.magnitude>0.35;
                let exit=super::anim_gate::allows_mode_exit(&crouch.action,moving,pad.high_profile);
                let input=super::crouch::CrouchInput{request:g.crouch_requested,hide:g.crouch_hide_hint,
                    high:pad.high_profile,moving,want_heading:if moving && pad.dir_move!=Vec3::ZERO {heading_of(pad.dir_move)} else {body.heading},
                    legs:pad.legs_held,turn_k:g.turn_k,quick_turn:g.quick_turn,
                    can_stand:super::ground_extras::can_stand(body.feet,&collision),anim_allows_exit:exit,
                    blend_progress,item_clock};
                let wait_high=matches!(crouch.mode,super::crouch::CrouchMode::Wait) && !moving && pad.high_profile;
                let mut heading=body.heading;
                // the root displacement ramps in with the blend into a new action (RE/24 ANIM-16)
                let k=crate::anim::root_weight(anim,crouch.action.id);
                let delta=crouch.update(input,g.blend.foot,&mut heading,dt).map(|d|d*k);
                body.heading=heading;
                if wait_high && delta.is_some() {
                    // Wait -> FreeRun 0xDA9510: phase 1 starts on the impulsion wait item (0xD8B9D0).
                    g.pose_seq=g.pose_seq.wrapping_add(1);
                    let mut extra=super::ground_extras::ExtraAction::free_run(g.blend.foot,g.pose_seq);
                    extra.action.item=1;g.extra=Some(extra);g.crouch=None;body.proxy.height=None;
                    g.free_run_high=g.wait_high;
                } else if let Some(delta)=delta {
                    body.proxy.height=Some(1.0);g.crouch=Some(crouch);
                    let d=super::right_of(body.forward())*delta.x+body.forward()*delta.z;
                    let r={let b=&mut *body;collision.move_capsule(&mut b.proxy,b.feet,d,true,dt)};
                    body.feet=r.position;body.velocity=r.velocity;
                    if let Some(support)=collision.ground_support_height(body.feet,body.proxy.height.unwrap_or(CAPSULE_HEIGHT)) {
                        body.stick_residual=body.feet.y-support.y;body.stick_normal_y=Some(support.normal_y);
                        body.feet.y=support.y;body.grounded=true;
                    } else {
                        body.grounded=false;
                        let entry=InAirEntry::Fall{from:body.feet,velocity:body.velocity,origin:FallOrigin::Ground,speed_param:0.0};
                        switch_context(&mut loco,&mut data,TransitionSetup::ToInAir(entry));
                    }
                    continue;
                } else {g.crouch=None;body.proxy.height=None;}
            }
        }

        // FreeRun (state 36) able to take event 45 this frame: its guard `sub_D85690` (game time past HG+520, 0.2 s
        // after the FreeRun start); the stick moving leaves it for Movement in the same frame
        // the FreeRun wait's playing item once event 45 may take it (0.2 s in), for event 45's takeoff foot
        let mut free_run_ready: Option<ActionBlend> = None;
        if GAME_GROUND_EXTRAS {
            g.legs_released_ago = if pad.legs_held { 0.0 } else { g.legs_released_ago+dt };
            // Event43: high profile, stationary, Legs held; native 0xEE8B93 checks button capture.
            // PORT: ability-stack/input-capture ownership is absent in the movement-only input adapter.
            if g.extra.is_none() && g.oneshot.is_none() && g.collide.is_none() && g.ledge_stop.is_none()
                && g.look_down.is_none() && pad.high_profile && pad.magnitude<=0.35 && pad.legs_held
            {
                g.pose_seq=g.pose_seq.wrapping_add(1);
                g.extra=Some(super::ground_extras::ExtraAction::free_run(g.blend.foot,g.pose_seq));
                g.speed_param=0.0;g.blend.speed_param=0.0;
                // HG+1500 stays the wait's through FreeRun (0xD8B9D0 does not touch it)
                g.free_run_high=g.wait_high;
            }
            if g.crouch_requested && !pad.high_profile {g.extra=None;}
            if let Some(mut extra)=g.extra {
                g.sub_state=HumanGroundSubState::FreeRun;
                g.high_profile=pad.high_profile;
                // 0xEE84CC: high, <=0.35 stick, Legs released after being held within the last 0.5 s.
                // 0x6D0F50 is the uncaptured held-button query, not the capture query.
                // Event47 takes a hand report first; event48 is the no-target fallback (0xEE84FE-0xEE858B).
                let request=pad.high_profile && !pad.legs_held && pad.magnitude<=0.35 && g.legs_released_ago<0.5;
                if request && extra.mode==super::ground_extras::ExtraMode::FreeRun && extra.elapsed+dt>=0.2 {
                    extra.target=static_hand_target(body.feet,body.forward(),&guidance,&collision,false);
                }
                // The wall run from the FreeRun sub-state (0xEE7E0F–0xEE7F0D, RE/24 WALL-15): high profile and the last strong
                // stick direction (+0x10E0) within 60° of the facing start it with no Legs, lock or stick test; event 49
                // is refused until 0.2 s after the FreeRun start (0xDA4890 → `sub_DA2DD0` / `sub_DA2BF0`, HG+520).
                if extra.mode==super::ground_extras::ExtraMode::FreeRun && extra.elapsed+dt>=0.2 && g.high_profile
                    && pad.strong_dir.length()>0.75 && pad.strong_dir.angle_between(body.forward())<60f32.to_radians()
                    && ab.allows(Ability::Walling)
                {
                    if let Some((contact,normal,slope))=super::walling::wall_ahead(body.feet,body.forward(),&collision) {
                        g.extra=None;
                        // standing (HG+1492 = 0): `FreeRun_WallingGuardA` 0xDA2DD0 → the Ground lead-in first (0xD89AA0 →
                        // 0xD857C0; live RE/25 #49 `wall41_legs_hold`)
                        if let Some(b)=super::ledge_moves::single_item(super::walling::GROUND_LEAD_IN,0) {
                            g.play_oneshot(b);
                            g.owner=TreeState::Other;
                            g.wall_start=Some((contact,normal,slope));
                            body.velocity=Vec3::ZERO;
                            continue;
                        }
                        let from=body.feet;
                        switch_context(&mut loco,&mut data,TransitionSetup::ToWalling(super::walling::WallingEntry{contact,normal,slope,from,warp_duration:None}));
                        continue;
                    }
                }
                free_run_ready = (extra.mode==super::ground_extras::ExtraMode::FreeRun && extra.elapsed+dt>=0.2).then_some(extra.action);
                let k=crate::anim::root_weight(anim,extra.action.id);
                let (out,delta)=extra.advance(dt,pad.magnitude>0.35,request,g.blend.foot);
                let delta=delta*k;
                match out {
                    super::ground_extras::ExtraResult::Inactive=>{
                        g.extra=None;
                        // the FreeRun's own run start (0xDAAE7D–0xDAAE97): FreeRun_Exit, `FreeRunStartMove`, Movement
                        if extra.mode==super::ground_extras::ExtraMode::FreeRun && GAME_GROUND_TREE {
                            g.play_free_run_start();
                        }
                    },
                    super::ground_extras::ExtraResult::Jump{action,fall}=>{
                        pad.consume_jump();
                        let entry=if let Some(target)=extra.target {
                            InAirEntry::JumpToTarget{from:body.feet,target,speed_param:0.0,foot_left:g.blend.foot==0}
                        } else {InAirEntry::OnPlace { from:body.feet,fwd:body.forward(),
                            action:ActionBlend::new(action,0,&[1.0]),fall:Some(ActionBlend::new(fall,0,&[1.0])) }};
                        body.velocity=Vec3::ZERO;
                        switch_context(&mut loco,&mut data,TransitionSetup::ToInAir(entry));
                        continue;
                    },
                    super::ground_extras::ExtraResult::Stay=>{
                        g.extra=Some(extra);
                        let d=super::right_of(body.forward())*delta.x+body.forward()*delta.z;
                        let r={let b=&mut *body;collision.move_capsule(&mut b.proxy,b.feet,d,true,dt)};
                        body.feet=r.position;body.velocity=r.velocity;
                        if let Some(support)=collision.ground_support_height(body.feet,body.proxy.height.unwrap_or(CAPSULE_HEIGHT)) {
                            body.stick_residual=body.feet.y-support.y;body.stick_normal_y=Some(support.normal_y);
                            body.feet.y=support.y;body.grounded=true;
                        } else {
                            body.grounded=false;
                            let entry=InAirEntry::Fall{from:body.feet,velocity:body.velocity,origin:FallOrigin::Ground,speed_param:0.0};
                            switch_context(&mut loco,&mut data,TransitionSetup::ToInAir(entry));
                        }
                        continue;
                    },
                }
            }
        }

        // ---------------------------------------------------------------- input → wanted motion
        // The interpreter's wall slide (`GoAssassinActionInterpreter` 0xEE6B16 → 0xEDD7C0): a stick pushed into a
        // wall turns along it; pushed within 45 deg of straight into it, the wanted speed is 0 and the run stop allowed.
        // It changes what the interpreter hands the Ground (SetMoveDir / SetDestSpeedRatio); the raw stick keeps the
        // other requests.
        // The interpreter steers by the response-curved direction (+0x1270, RE/24 INPUT-6).
        let (mv_dir, mv_speed) = if GAME_GROUND_TREE { wall_slide(&body, pad.dir_move, pad.speed01) } else { (pad.dir, pad.speed01) };
        // The interpreter's own edge stop (0xEE7BDA–0xEE7CB4), low profile: the stick-direction edge report with +52 below
        // 0.16 (squared: within 0.4 m), a drop of more than 2 m (any deeper drop too: 0xEE7C1A tests only `> 2.0`) and its normal within
        // 70 deg of the facing (and of the stick) sets the wanted speed to 0 (SetMoveSpeed(0)) before the Ground updates:
        // to the Ground it is a released stick, so a walk start still playing leaves for the wait at once and its root
        // motion stops (RE/25 #39: live, the walk start at a 4.1 m drop went to `xx_h_accel_tr_l_wait` with the feet
        // short of the line). Held 0.25 s it becomes the look-down below. Its 0.4 m reach beats the ledge stop's
        // 0.15 m, so in low profile the ledge stop never comes (RE/25 #39: low profile at 1.6 / 4.1 / 7.6 m drops always
        // stopped and looked down; only high profile ledge-stops).
        let halt_edge = (!(pad.high_profile && ab.max_speed() > 1) && mv_speed > 0.0)
            .then(|| edge_report_drop(body.feet, body.forward(), REPORT_PICK_RADIUS, EDGE_HALT_SQ, EDGE_HALT_COS, 2.0, &guidance, &collision))
            .flatten()
            .filter(|&(_, n, _)| pad.dir.dot(Vec3::new(n.x, 0.0, n.z).normalize_or_zero()) > EDGE_HALT_COS);
        let edge_halt = halt_edge.is_some();
        let mv_speed = if edge_halt { 0.0 } else { mv_speed };
        // The stick pushed straight into a wall (wall slide result 2): no speed, but the interpreter still hands the Ground
        // the direction into the wall as DestHeading (0xEE6FB0: SetDestHeading whenever the stick magnitude +4672 > 0),
        // so Idle can pivot round to face it (RE/24 GROUND-10, bug-1791664621). PORT: the stick inside the dead zone
        // (DestSpeedRatio 0 without a wall) does not steer yet; that half of GROUND-10 waits for its live check.
        let wall_stopped = GAME_GROUND_TREE && pad.speed01 > 0.0 && mv_speed <= 0.0;
        // The wall run's input (0xEE65A0): Legs pressed with the stick within 45° of the facing, or (free running,
        // 0xEE7E50) Legs held with the stick past the dead zone and within 60° (`flt_1694AC8`).
        let wall_run_input = {
            let pressed = pad.jump_buffered() && pad.speed01 > 0.0 && pad.dir.dot(body.forward()) >= 45f32.to_radians().cos();
            let held = pad.legs_held && pad.magnitude > crate::tuning::STICK_DEADZONE && pad.dir.dot(body.forward()) >= 60f32.to_radians().cos();
            pressed || held
        };
        // HG+1532: the signed angle from the current heading to the wanted one (Movement_PreUpdate 0xD97E30, no speed test).
        let stick_turn = if mv_speed > 0.0 || wall_stopped { angle_diff(heading_of(mv_dir), body.heading) } else { 0.0 };
        // A transition into the locomotion (the Move state's transition path) keeps steering and the other requests
        // live. A released stick leaves it like any Move: `HumanGround__Move_CanRunStop` 0xD7EC90 (high profile, the
        // run stop allowed, and the playing action is the locomotion, the skid-turn exits legacy 96 / 97, or past 0.33 of its
        // normalised time, `sub_501DF0`)
        // or else `HumanGround__Move_CanStop` 0xD7ED30 (the playing action is not the locomotion): the wait at once.
        if GAME_GROUND_TREE && mv_speed <= 0.0 {
            if let Some(t) = g.tr.filter(|t| !t.to_wait) {
                // HG+1500 is MoveBlend's band flag s > 0.25 (0xDA18FF), not the profile (RE/24 GROUND-21)
                let can_run_stop = g.speed_param > BAND_WALK && !super::anim_gate::locked(&t.action)
                    && (super::ground_tree::RUN_TURN_EXIT.contains(&t.action.id) || t.phase > 0.33);
                if can_run_stop {
                    // RunStop_Enter reads the transition still playing (its word, its normalised time)
                    g.start_run_stop();
                    g.tr = None;
                } else {
                    g.tr = None;
                    // Idle from Move (0xD8B220) as below: the leading foot's wait (RE/24 GROUND-24) through the accel
                    // transition; PORT without a pose: the transition's exit foot
                    let lead = leading_foot.unwrap_or_else(|| super::ground_tree::pick_locomotion_foot(super::anim_gate::word(&t.action), g.blend.foot));
                    g.idle_from_move(lead, body.velocity.with_y(0.0).length());
                    g.speed_param = 0.0;
                    g.blend.speed_param = 0.0;
                }
            }
        }
        // The anim gate of Idle → Move (0xD84AC0) and of the pivot (0xD84B10): the playing item has ended, or its word
        // allows leaving it for moving in this profile and it is not locked (`HumanGround__AnimAllowsModeExit`
        // 0xD80010, `anim_gate`). A one-shot that allows it is left at once for the locomotion (its transition blends,
        // RE/13 §7); locked ones (landings, run stops, pivots) play out. Not in the ground's own sub-states (the ledge
        // stop, sub-state 38; the obstacle collision, 5): they leave by their own rules. A transition into the wait is
        // Idle's: the same gate leaves it for the start.
        // The landing states (21, 22) and the skid turn (24) have no such gate: they leave when the action completes
        // (`sub_501760`). That includes the free-step reception (PORT: the game continues it in NarrowObject); its
        // authored exits (`freestep_entry_tr_*`, entered with a cut) start from its last frame.
        let gated = !GAME_GROUND_TREE || g.owner == TreeState::Other;
        if mv_speed > 0.0
            && gated
            && g.ledge_stop.is_none()
            && g.collide.is_none()
            && g.impulsion_jump.is_none()
            && g.oneshot.is_some_and(|o| super::anim_gate::allows_mode_exit(&o.blend, true, pad.high_profile))
        {
            g.oneshot = None;
            g.oneshot_next = None;
            g.owner = TreeState::Other;
        }
        if mv_speed > 0.0 && g.tr.is_some_and(|t| t.to_wait && super::anim_gate::allows_mode_exit(&t.action, true, pad.high_profile)) {
            g.tr = None;
        }
        // The skid turn (state 24) is left for Move as soon as the stick asks to move within 120 deg again with HG+1780
        // clear, or when it completes (`HumanGround__Guard_RunTurnToMove` 0xD85390, 0xD853BE); its locked item does not
        // hold it.
        if GAME_GROUND_TREE && g.owner == TreeState::RunTurn && mv_speed > 0.0 && !g.quick_turn && stick_turn.abs() <= RUN_TURN_ANGLE {
            if let Some(os) = g.oneshot.take() {
                g.owner = TreeState::Other;
                g.run_turn_exit(os.blend, false, &mut body.heading);
            }
        }
        let busy = g.oneshot.is_some() || g.tr.is_some_and(|t| t.to_wait);
        let moving = mv_speed > 0.0 && !busy;

        // ---------------------------------------------------------------- beam (NarrowObject, event 72)
        // The sender (`GoAssassinActionInterpreter__ProcessGroundMovement`, RE/28 §2): every frame with the Jump ability
        // and the stick past 0.35 it asks the guard (IHumanGround vt756 → CanHandleEvent(72), 0xEE79C3) along the
        // stick's direction (+0x1250), before the edge halt zeroes the wanted speed, and posts the event (vt760,
        // 0xEE860D) ahead of the edge jump, the ledge stop and the obstacle collision; the Ground's own update (the pivot,
        // the start) comes after. So standing on or beside a beam, any stick toward it mounts it. Guard 0xD9F4C0
        // (RE/24 NARROW-3).
        if pad.speed01 > 0.0 && !busy && ab.allows(Ability::Jump) {
            let event_dir = if pad.dir_snap != Vec3::ZERO { pad.dir_snap } else { pad.dir };
            if let Some(mut entry) = super::narrow::try_mount_beam(body.feet, event_dir, &guidance, &collision) {
                // Mode 1 plays the beam walk (`sub_F7B3B0` 0xF7B619: 0x28A3A8E4), keeping the playing locomotion item's
                // index (the foot), jog weights [0, 1] when the Ground runs (NarrowObjectData+228 = Ground+1500), else
                // walk [1, 0] (RE/24 NARROW-2; RE/05 §2.12's "retains the incoming locomotion" was wrong). PORT: the
                // 1.0 s crossfade from the locomotion is the animator's
                if super::narrow::BEAM_COMPLETION {
                    entry.foot = g.blend.foot;
                    let w = if g.speed_param > 0.25 { [0.0, 1.0] } else { [1.0, 0.0] };
                    entry.action = Some(super::jump_blend::ActionBlend::new(super::narrow::BEAM_WALK, g.blend.foot, &w));
                }
                let phase = g.blend.phase;
                switch_context(&mut loco, &mut data, TransitionSetup::ToBeam(entry));
                if super::narrow::BEAM_COMPLETION {
                    // The incoming locomotion action carries on (0xF7AAA0 mode 1): keep its phase, not item start.
                    let n = &mut data.narrow;
                    n.t = phase * n.action.map(|a| a.duration()).unwrap_or(0.0);
                }
                continue;
            }
        }

        let prev_high = g.high_profile;
        // IHumanGround vt12 SetHighProfile: high profile only above MaxSpeed Walk (0xEE70F9, RE/24 INPUT-10)
        g.high_profile = pad.high_profile && ab.max_speed() > 1;
        // vt144 Sprint (Data+291) = MaxSpeed Sprint and Legs held (0xEE709F); MoveBlend applies it only in high profile
        // (RE/24 GROUND-22)
        g.sprint = ab.max_speed() == 4 && pad.legs_held;
        g.sub_state = if !GAME_GROUND_EXTRAS && g.sprint && g.high_profile { HumanGroundSubState::FreeRun } else { HumanGroundSubState::Movement };

        // turn attenuation (interpreter 0xEE65A0): beyond 45° slow down; forced to 1 in low profile. The angle is +0x1264,
        // the curved stick's (+0x1270) before the wall slide (0xEE69DA).
        let want_heading = if moving { heading_of(mv_dir) } else { body.heading };
        let off = if !GAME_GROUND_TREE { angle_diff(want_heading, body.heading).abs() }
            else if moving && pad.dir_move != Vec3::ZERO { angle_diff(heading_of(pad.dir_move), body.heading).abs() } else { 0.0 };
        let atten_target = if !g.high_profile || off <= TURN_ATTEN_START {
            1.0
        } else {
            (1.0 - (off - TURN_ATTEN_START) / TURN_ATTEN_RANGE).max(TURN_ATTEN_FLOOR)
        };
        g.turn_atten = if GAME_GROUND_TREE {
            // exactly as the interpreter steps +4304 (0xEE6A34): at or below 1 − k it rises at 5/s up to 1, above it
            // falls at 10/s down to 0.1, so it hunts around the target instead of settling on it; low profile forces 1
            let k = ((off.min(std::f32::consts::FRAC_PI_2) - TURN_ATTEN_START) / TURN_ATTEN_RANGE).max(0.0);
            let a = if g.turn_atten <= 1.0 - k { (g.turn_atten + TURN_ATTEN_UP_RATE * dt).min(1.0) } else { (g.turn_atten - TURN_ATTEN_DOWN_RATE * dt).max(TURN_ATTEN_FLOOR) };
            if g.high_profile { a } else { 1.0 }
        } else if atten_target > g.turn_atten {
            (g.turn_atten + TURN_ATTEN_UP_RATE * dt).min(atten_target)
        } else {
            (g.turn_atten - TURN_ATTEN_DOWN_RATE * dt).max(atten_target)
        };

        // speed parameter: target = base + 0.25·stick (RE/02 §1)
        let base = if g.sprint && g.high_profile {
            BASE_SPRINT
        } else if g.high_profile {
            BASE_HIGH_PROFILE
        } else {
            BASE_LOW_PROFILE
        };
        // the target follows the stick even while a landing action plays (MoveBlend's transition path keeps
        // ramping toward it, 0xDA0810)
        let target = if mv_speed > 0.0 { (base + STICK_SPAN * mv_speed * g.turn_atten).min(1.0) } else { 0.0 };
        // stick released (desired mode HG+0x5D8 = 0): the game leaves the Move state instead of decelerating
        // (the curve at HG+0x63C only runs while still moving toward a slower band):
        // - jog or faster (HG+0x5DC, speed > 0.25) → RunStop 0xD98E30 (guard 0xD7EC90): the run-stop action by the
        //   leading foot, its root motion, then its settle into the wait;
        // - walk band → Idle 0xD8B220 (guard 0xD7ED30): the wait with a 0.2 s blend, i.e. stopped at once.
        // Stick released while a landing / reception plays: its transition leads into the wait, not into the
        // locomotion (the action's exits, RE/13), so no run stop follows it. Before, the speed kept from the jump
        // started a run stop once the landing ended: a second slide after the landing.
        if mv_speed <= 0.0 && g.speed_param > 0.0 && g.oneshot.is_some_and(|o| !jump_blend::RUN_STOP.contains(&o.blend.id) && !jump_blend::RUN_STOP_TO_WAIT.contains(&o.blend.id)) {
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
        }
        // The run stop is allowed by Data+0x11F, which the input interpreter sets (IHumanGround vt136 0xDB31B0, from
        // 0xEE6783 / 0xEE67EE / 0xEE67FC): 1 with the stick released, and with the stick held more than 135 deg from the
        // facing (dot < -0.7071); 0 otherwise. Its guard (0xD7EC90) also needs HG+1500, MoveBlend's band flag s > 0.25. So
        // pulling the stick back at a run skids (run stop), and the Idle that follows pivots (state 25): the skid turn.
        let reversed = pad.speed01 > 0.0 && pad.dir_move.dot(body.forward()) < -std::f32::consts::FRAC_1_SQRT_2;
        // in front of the locomotion the guard also accepts the skid-turn exits (legacy 96 / 97) and any transition
        // past 0.33 of its normalised time that is not locked (0xD7EC90)
        let tr_allows_stop = g.tr.is_none_or(|t| !t.to_wait && !super::anim_gate::locked(&t.action)
            && (super::ground_tree::RUN_TURN_EXIT.contains(&t.action.id) || t.phase > 0.33));
        if reversed && g.speed_param > BAND_WALK && g.oneshot.is_none() && tr_allows_stop {
            g.start_run_stop();
        }
        if mv_speed <= 0.0 && g.speed_param > 0.0 && g.oneshot.is_none() && g.tr.is_none() {
            // the Move guards try the run stop first (0xD7EC90), Idle only otherwise: the run stop reads the playing
            // locomotion item (RE/24 GROUND-7)
            if GAME_GROUND_TREE && g.speed_param <= 0.25 {
                // Idle from Move (0xD8B220): the wait of the leading foot (`Human__GetLeadingFoot` 0xD8B279: the left
                // foot ahead → the left wait, `dword_1A2BFE0`; else the right one; RE/24 GROUND-24). PORT without a pose:
                // the locomotion item's exit foot (its word, 0xD86760)
                let word = super::anim_gate::item_word(super::move_blend::ACT_GROUND_LOCOMOTION, g.blend.foot);
                let lead = leading_foot.unwrap_or_else(|| super::ground_tree::pick_locomotion_foot(word, g.blend.foot));
                g.idle_from_move(lead, body.velocity.with_y(0.0).length());
            }
            if g.speed_param > 0.25 {
                g.start_run_stop();
                if !GAME_GROUND_TREE && g.oneshot.is_some() {
                    let foot = (g.blend.foot != 0) as usize;
                    g.oneshot_next = jump_blend::action_items(jump_blend::RUN_STOP_TO_WAIT[foot]).map(|_| ActionBlend::new(jump_blend::RUN_STOP_TO_WAIT[foot], 0, &[1.0]));
                }
            }
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
        }
        // moving again during the run stop: the stop item is locked (0x20) and plays out; its settle into the wait
        // allows leaving for moving (0x200 / 0x800), which the anim gate above does
        // pivot (Movement state 25, `HumanGround__Pivot_Enter` 0xDA6150): the wanted heading more than 90 deg (HG+0x720)
        // from the current one, from standing (guard 0xD84B10) or from a low-profile walk (Move guard 0xD84F10: the
        // band flag HG+1500 clear: s ≤ 0.25, RE/24 GROUND-21). The turn action from the table at 0x1A2C120 by side, [from, to] profile and
        // leading foot, blending its 90 / 180 deg clips by (|a| - 90 deg) / 90 deg; the clip's root yaw turns the body.
        // `Idle_CanPivot` needs no move request (only the anim gate, |HG+1532| > π/2 and HG+1485, which is never set:
        // only cleared at 0xDB2CE7 / 0xDA7F6E), so the stick into a wall behind turns the character round.
        let pivot_off = if GAME_GROUND_TREE { stick_turn.abs() } else { off };
        if (moving || wall_stopped) && !busy && g.oneshot.is_none() && g.collide.is_none() && g.ledge_stop.is_none() && pivot_off > std::f32::consts::FRAC_PI_2 && g.speed_param <= BAND_WALK {
            let left = mv_dir.dot(super::right_of(body.forward())) < 0.0;
            // the player's EntityDescriptorType is Main (entity+168 & 7 == 1): `Pivot_Enter` 0xDA6150 then sets the
            // destination profile HG+1504 to the current one HG+1500, so the player only plays the low → low and
            // high → high turns
            let to_high = if GAME_GROUND_TREE { prev_high } else { g.high_profile };
            let id = PIVOT[(!left) as usize][prev_high as usize][to_high as usize][(g.blend.foot != 0) as usize];
            let w = ((pivot_off - std::f32::consts::FRAC_PI_2) / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
            if jump_blend::action_items(id).is_some() {
                g.play_oneshot(ActionBlend::new(id, 0, &[1.0 - w, w]));
                g.tr = None;
                g.owner = TreeState::Pivot;
                g.pivot_high = prev_high;
                // HG+688 := HG+1532 (`HumanGround__Pivot_Enter` 0xDA6150), read by the turn start's layouts
                g.pivot_turn = stick_turn;
                g.speed_param = 0.0;
                g.blend.speed_param = 0.0;
                body.velocity = Vec3::ZERO;
                continue;
            }
        }

        // the halt also latches +4176 (0xEE7C85) and starts its timer +4144 when it rises; any frame without it clears
        // the latch (0xEE7C8E)
        g.edge_halt_t = if edge_halt { g.edge_halt_t + dt } else { 0.0 };
        // start from standing (Idle → Move, 0xD84AC0 → `HumanGround__PlayStartMove` 0xD98990)
        if moving && !busy && !edge_halt && g.oneshot.is_none() && g.tr.is_none() && g.speed_param <= 0.0 && g.collide.is_none() && g.ledge_stop.is_none() && g.ledge_stop_lock.is_none() {
            g.play_start_move();
        }
        // Idle's wait update (0xDA6DC0, RE/24 GROUND-14): when the playing item may be left for standing (`sub_D84D10`:
        // it ends, or `AnimAllowsModeExit` 0xD80010 by the wanted profile) and the wanted profile is not the wait's
        // (HG+1504 ≠ HG+1500), or the stance-change wait has played its time, HG+1524 toggles (`sub_D7E5C0`: 0 → 1 with
        // 0.1 s after a low wait, 0.5 s after a high one; 1 → 0) and the wait is entered again in the wanted profile
        // (0xD94580). The other states clear HG+1524 (`PlayStartMove` 0xD98BBF, `Movement_EnterBody` 0xD92E7D,
        // `Pivot_Enter` 0xDA6412, `LookDown_Enter` 0xD9FE88), and a wait entered from them takes the wanted profile.
        let idle = GAME_GROUND_TREE && g.speed_param <= 0.0 && g.oneshot.is_none() && g.tr.is_none_or(|t| t.to_wait)
            && g.crouch.is_none() && g.extra.is_none() && g.collide.is_none() && g.look_down.is_none() && g.ledge_stop.is_none();
        if idle {
            g.stance_timer = (g.stance_timer - dt).max(0.0);
            // the waits allow standing in both profiles (words 0x0FC5 / 0x0FCA / 0x0FCF); a transition into the wait by
            // its own word
            let gate = g.tr.is_none_or(|t| super::anim_gate::allows_mode_exit(&t.action, false, g.high_profile));
            if gate && (g.wait_high != g.high_profile || (g.stance_change && g.stance_timer <= 0.0)) {
                // leaving the stance-change wait: the wait is played from it, so its authored item transition into the new
                // profile's wait plays first (`IdleWait_Enter` 0xD94580 → `AnimSlot__SetAction`'s lookup, RE/13 §7.1;
                // live RE/25 #63: `xx_lh_wait_*_tr_h_wait_*` 0.66 s, `xx_hl_wait_*_tr_l_wait_*` 1.0 s)
                let stance_wait = g.stance_change.then(|| super::ground_tree::idle_wait(g.wait_high, g.wait_foot, true));
                g.stance_change = !g.stance_change;
                if g.stance_change {
                    g.stance_timer = super::ground_tree::STANCE_TIME[g.wait_high as usize];
                }
                g.wait_high = g.high_profile;
                g.tr = None;
                if let Some(from) = stance_wait.filter(|&id| jump_blend::action_items(id).is_some()) {
                    g.enter_wait(ActionBlend::new(from, 0, &[1.0]), g.high_profile);
                }
            }
        } else {
            g.stance_change = false;
            g.wait_high = g.high_profile;
        }

        // speed parameter, lean/bank and blend weights (MoveBlend 0xDA0810). The heading snapshot is the
        // heading before this frame's turn (HG+0x600, Movement_PreUpdate 0xD97E30). While a transition action plays in
        // front of the locomotion, the transition path runs instead (0xDA09B3): the speed parameter is steered into the
        // layout's band at 1/s and the transition's clips are weighted (`ground_tree`).
        g.blend.speed_param = g.speed_param;
        if let Some(t) = g.tr.filter(|t| !t.to_wait) {
            let goal = super::ground_tree::transition_target(t.layout, target, g.speed_param, g.sprint);
            g.blend.speed_param = super::ground_tree::transition_speed(g.speed_param, goal, dt);
            if matches!(t.layout, 0 | 5 | 6 | 7) {
                g.blend.settle = 1.0;
            }
        } else {
            g.blend.update_speed(target, dt);
        }
        g.blend.update_angles(body.heading, moving.then_some(want_heading), g.crowd_avoid.take().map(heading_of), false, dt);
        if g.tr.is_none() {
            g.blend.update_weights(target, dt);
        }
        // MoveBlend is the Move state's (11): the run stop (18), the landings (21, 22), the skid turn (24) and the
        // pivot (25) do not run it, so the parameter keeps what their exits set (0xDA8C80 is the only caller)
        let tree_owned = GAME_GROUND_TREE && g.oneshot.is_some() && g.owner != TreeState::Other;
        if !tree_owned {
            g.speed_param = g.blend.speed_param;
        } else {
            g.blend.speed_param = g.speed_param;
        }
        // the run stop (state 18) does not run MoveBlend: the parameter stays 0 while it plays
        if g.oneshot.is_some_and(|o| jump_blend::RUN_STOP.contains(&o.blend.id) || jump_blend::RUN_STOP_TO_WAIT.contains(&o.blend.id)) {
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
        }
        // MoveBlend's tail: the walking upper-body fidget (RE/24 GROUND-18)
        let in_move = g.speed_param > 0.0 && g.oneshot.is_none() && g.crouch.is_none() && g.extra.is_none() && g.collide.is_none();
        let (item, s, enabled) = (g.blend.foot, g.speed_param, g.ik_state == IkState::On);
        g.fidget.update(in_move, g.tr.is_none(), item, s, enabled, dt);

        // heading: rotate toward wanted at the player turn rate (0xD95290), unless the playing item turns the body
        // itself (0x10)
        // k of the turn band grows every frame, before the clip-flag test (`HumanGround__UpdateHeading` 0xD95290)
        g.turn_k = (g.turn_k + PLAYER_TURN_BAND_RATE * dt).min(1.0);
        if moving && !g.oneshot.is_some_and(|o| super::anim_gate::anim_turns(&o.blend)) && !g.tr.is_some_and(|t| super::anim_gate::anim_turns(&t.action)) {
            let d = angle_diff(want_heading, body.heading);
            // `RotateTowards` 0xD94F30 turns the shorter way unless HG+1776 forces a side; HG+1776 is written only by
            // `HumanGround__ctor` (0 at 0xDB2DA3) and read by `UpdateMoveBlend` 0xDA1A7D / `UpdateHeading`, so the
            // player always turns the shorter way (2026-10-18; the port kept a started side near 180° before).
            // the player's rates are set every frame by the interpreter (IHumanGround vt952 / vt956 → HG+1752 / +1756,
            // 0xEE6D3F–0xEE6D91): 1.5 / 4.0 rad/s, with Legs held 0.975 / 2.6 (0.375 / 3.0 in the vt1196 state, not
            // reached by the port); `RotateTowards` 0xD94F30 picks between them by the angle band 10° / 60° × k
            // (RE/24 INPUT-5)
            let rate = if GAME_GROUND_TREE { player_turn_rate(d.abs(), pad.legs_held, g.turn_k) } else { PLAYER_TURN_RATE };
            let step = rate * dt;
            let turn = d.clamp(-step, step);
            g.turn_sign = if d.abs() > 1e-3 { turn.signum() } else { 0.0 };
            body.heading = wrap_angle(body.heading + turn);
        } else {
            g.turn_sign = 0.0;
        }

        // ---------------------------------------------------------------- obstacle collision / lean (sub-state 5)
        if g.collide.is_none() {
            g.lean_push_t = 0.0;
        }
        if let Some(mut c) = g.collide {
            g.sub_state = HumanGroundSubState::ObstacleCollision;
            // event 49 in the lean (`ObstacleCollision` handler 0xDA6FF0: HG+1492 clear → FindWallingWall → 0xDA5680,
            // RE/24 WALL-19): the wall run starts from the lean with the same input as from Movement
            if g.high_profile && wall_run_input && ab.allows(Ability::Walling) && c.phase == super::collide::CollidePhase::Wait {
                if let Some((contact, normal, slope)) = super::walling::wall_ahead(body.feet, body.forward(), &collision) {
                    if pad.jump_buffered() {
                        pad.consume_jump();
                    }
                    // the lean takes event 49 with HG+1492 clear only, and plays the Ground lead-in first (0xD8E740 →
                    // 0xD8A460; live RE/25 #49: `impultionstraight_footl_to_wallingfront_step1_footr`, 0.27 s, then
                    // `wallingfront_entry_a`)
                    g.collide = None;
                    g.lean_push_t = 0.0;
                    if let Some(b) = super::ledge_moves::single_item(super::walling::GROUND_LEAD_IN, 0) {
                        g.play_oneshot(b);
                        g.owner = TreeState::Other;
                        g.wall_start = Some((contact, normal, slope));
                        body.velocity = Vec3::ZERO;
                        continue;
                    }
                    let from = body.feet;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToWalling(super::walling::WallingEntry { contact, normal, slope, from, warp_duration: None }));
                    continue;
                }
            }
            // The lean's climb (interpreter 0xEE75E0–0xEE7788, live-traced RE/25 GROUND-32: `DoStraightJumpToTarget`
            // posts event 47 from 0xEE7781): while leaning (IHumanGround vt708 0xD7DB10: sub-state 5) the stick past
            // 0.35 within 45° of the facing runs the push timer. Legs pressed, or pushed for more than 0.5 s (0.1 s in
            // high profile), with the Jump ability and the lean's item unlocked (CanHandleEvent(47), 0xDA6FF0): the static
            // hand target (IHuman vt64 0xB10990 → `Human__EvaluateStaticHandTargets` 0xE165D0, picked by 0xE97720) of a
            // free-step, knee or waist type (flags & 0xD; any type with high profile and Legs) is jumped at (event 47 →
            // `HumanGround__StartStraightJumpToTarget` 0xD8A9C0 → 0xB21DA0). Live: the Bureau crate (1.16 m) was climbed
            // 0.47 s after the collide.
            let pushing = pad.magnitude > crate::tuning::STICK_DEADZONE && pad.stick_facing_angle.abs() < std::f32::consts::FRAC_PI_4;
            g.lean_push_t = if pushing { g.lean_push_t + dt } else { 0.0 };
            let legs = pad.jump_buffered();
            if (legs || g.lean_push_t > LEAN_CLIMB_TIME[pad.high_profile as usize]) && ab.allows(Ability::Jump) && !super::anim_gate::locked(&c.current().0) {
                let target = static_hand_target(body.feet, body.forward(), &guidance, &collision, false)
                    .filter(|t| (pad.high_profile && legs) || t.type_flags & 0xD != 0);
                super::target_debug::record(|| "lean climb (static hand target)".into(), body.feet, body.forward(), &[], target.as_ref());
                if let Some(target) = target {
                    pad.consume_jump();
                    g.collide = None;
                    g.lean_push_t = 0.0;
                    body.velocity = Vec3::ZERO;
                    let foot_left = g.blend.foot == 0;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::JumpToTarget { from: body.feet, target, speed_param: 0.0, foot_left }));
                    continue;
                }
            }
            let stick = (pad.speed01 > 0.0).then(|| super::collide::lean_stick(pad.dir, c.normal));
            let (mut feet, mut heading) = (body.feet, body.heading);
            let out = super::collide::update(&mut c, dt, stick, pad.high_profile, &mut feet, &mut heading);
            body.feet = feet;
            body.heading = heading;
            match out {
                super::collide::CollideOut::Stay => g.collide = Some(c),
                super::collide::CollideOut::Leave { first, then, speed, face } => {
                    g.collide = None;
                    // the exit clips turn the body by their root yaw (back 180 deg, side 90 deg); the stick-based facing
                    // is only a fallback for an exit without one
                    g.face_next = if first.is_some_and(|b| b.yaw(1.0).abs() > 0.05) { None } else { face.map(heading_of) };
                    g.speed_param = speed;
                    g.blend.speed_param = speed;
                    if let Some(b) = first {
                        g.play_oneshot(b);
                        g.oneshot_next = then;
                    }
                }
            }
            body.velocity = Vec3::ZERO;
            continue;
        }

        // ---------------------------------------------------------------- look-down at an edge (event 119)
        // Sender (interpreter 0xEE87DE–0xEE8844): the edge-halt latch (+4176: low profile, the stick past the dead zone
        // and within 70° of a front edge's normal, the edge within 0.4 m (+52 < 0.16), its drop over 2 m) with its drop (+4128)
        // over 2 m, held for more than 0.25 s (+4144, flt_1682378), the LookDown ability, then IHumanGround vt1540
        // (0xDB6AD0, CanHandleEvent(119): the Idle wait sub-state 0xDA7250, guard 0xD7E590) and vt1544 (0xDBEF90).
        // The look-down is Idle's sub-state 149: the player's interpreter sends no end event (120 comes only from the
        // NPC AI, 0xEBAF80 → 0xEB9BD0), so it lasts until the walk starts again (the stick turned off the edge) or
        // another action takes over. Releasing the stick keeps it.
        // (the halt zeroes the wanted speed at the input, +4176; the port zeroes it after MoveBlend, so the parameter
        // read here is not yet 0 while halted)
        // the transitions into the wait are Idle's (Idle_Enter / Idle_EnterFromMove): the look-down takes over from them
        // (live RE/25: 0.25 s into `xx_h_accel_tr_l_wait` the look-down started)
        if g.oneshot.is_some() || (!edge_halt && (moving || g.speed_param > 0.0)) {
            g.look_down = None;
        } else if let Some(mut ld) = g.look_down {
            ld.t += dt;
            g.look_down = Some(ld);
        } else if let Some((p, n)) = halt_edge.filter(|&(_, _, d)| d > 2.0 && g.edge_halt_t > LOOK_DOWN_HOLD && ab.allows(Ability::LookDown)).map(|(p, n, _)| (p, n)) {
            let w = look_down_weights(body.feet, body.forward(), p, n);
            if let Some(a) = jump_blend::action_items(LOOK_DOWN[(g.blend.foot != 0) as usize]).map(|_| ActionBlend::new(LOOK_DOWN[(g.blend.foot != 0) as usize], 0, &w)) {
                g.pose_seq = g.pose_seq.wrapping_add(1);
                g.tr = None;
                g.look_down = Some(LookDown { action: a, t: 0.0, point: p, normal: n });
            }
        }

        // The interpreter tests the ground ladder (0xEE7147) and the empty-hand climb start (0xEE7255) before the wall-run
        // press (0xEE78C9) and the held start (0xEE7E50) (RE/24 WALL-16).
        // ---------------------------------------------------------------- ladder (event 38)
        // The interpreter's automatic ladder entry (0xEE7143–0xEE723C, RE/24 LADDER-1): every ground frame, unless high
        // profile with Legs held (the free-run path); the stick toward a ladder (`FindLadderNearby`) and event 38's guard
        // `Human__CanGrabLadder` (reach 0.6) → event 38, from the foot or from the top; no button. PORT: the ladder
        // entity's reply (0xEE720A) is taken as yes
        if !(g.high_profile && pad.legs_held) && pad.magnitude > 0.0 && !busy && ab.allows(Ability::Ladder) {
            if let Some((base, top, mut n, from_top)) = super::ladder::find_ladder(body.feet, head, pad.dir, body.forward(), &guidance) {
                // the approach side's attach box blocked → the back-approach turn onto the far side (RE/24 LADDER-4)
                let mut back_turn = None;
                if from_top {
                    // the far side's box blocked → the near side (RE/24 LADDER-23)
                    n = super::ladder::top_entry_side(base, top, n, &collision);
                }
                let buried = if from_top { 0.0 } else { super::ladder::buried_depth(base, top, &collision) };
                // the box sits at the entry's root target, the attach rung (0xE26AFB after 0xE26A73–0xE26AE2)
                let attach = super::ladder::ground_attach(base, top, n, body.feet, g.high_profile, buried);
                if !from_top && super::ladder::attach_blocked(attach, -n, &collision) {
                    // the item by the signed angle about up from the ladder → root direction to the ladder's normal on
                    // the root's side (0xE269FC, v74 = angle / 90 deg): above 0 → TURN[0] (`turn_l`), else TURN[1]
                    // (0xE26B26; live, RE/25: the root 0.13 m to the side of the line → `turn_l`, on it → `turn_r`)
                    let to_char = Vec3::new(body.feet.x - base.x, 0.0, body.feet.z - base.z);
                    back_turn = Some(to_char.cross(n).y > 0.0);
                    // the climb stays on the approach side: live (RE/25 #68, probed at 0xE26B00 / 0xE26B57 on the Bureau
                    // ladder) the box was blocked at the near target (y 3.214) and the flipped frame put the target on
                    // the far side (y 2.214), yet the root ended on the near side 0.545 m out, hands on the line, after
                    // the turn (twice, from both sides); the flipped target does not place it (hypothesis: the
                    // entry's root warp is relative to the turn clip). PORT: `n` is kept
                }
                // the entry's column: 2·(`Human__GetLeadingFoot` ≠ 1) (0xE26A4C, RE/24 LADDER-24); PORT without a pose: the
                // locomotion blend's foot
                let foot = leading_foot.unwrap_or((g.blend.foot != 0) as usize);
                let e = super::ladder::LadderEntry { base, top, n, from: body.feet, facing: body.forward(), from_top, high: g.high_profile, foot, from_ledge: false, action: None, back_turn, buried, ..Default::default() };
                switch_context(&mut loco, &mut data, TransitionSetup::ToLadder(e));
                continue;
            }
        }

        // ---------------------------------------------------------------- climb / grab requests
        // Climbing from the ground is the empty-hand press (0xEE7255: the Climb ability, `Pad__JustPressed(3)` ->
        // IHumanGround vt840 = CanHandleEvent(50) -> vt844, event 50 -> the Climb context, fill 0xD83950; RE/02 4.7).
        let forward = body.forward();
        if pad.hand_just_pressed() && ab.allows(Ability::Climb) {
            // leading foot (Human__GetLeadingFoot 0xB18850): the playing locomotion item, footl = left ahead
            // the climb start's foot variant: `Human__GetLeadingFoot` ≠ 1 (0xB261A8); PORT without a pose: the blend's foot
            if let Some(entry) = climb_start_entry(body.feet, forward, leading_foot.map_or(g.blend.foot != 0, |f| f == 1), &guidance, &collision) {
                pad.consume_hand();
                switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(entry));
                continue;
            }
        }
        // ---------------------------------------------------------------- wall run (Walling, event 49)
        // Input handler 0xEE65A0 (tested before the jumps and the grab): high profile, Legs pressed, the stick
        // pushed within 45° of the facing (> 0.35), ability Walling, and IHumanGround vt112 = event 49's guard
        // (0xDA54F0: the playing item allows a mode exit, then the wall test 0xE18390).
        // Free running starts it without a press too (0xEE7E50, verified live): Legs held, the stick past the dead zone
        // and within 60 degrees of the facing (`flt_1694AC8`), then the same guard (RE/02 4.7).
        // Both read the stick itself (+0x1240 > 0.35, its angle +0x1244), not the wanted speed: snug against the wall
        // the wall slide zeroes the speed (0xEDD7C0 result 2) but the wall run still starts (bug-1791665811).
        if g.high_profile && wall_run_input && !busy && ab.allows(Ability::Walling) {
            if let Some((contact, normal, slope)) = super::walling::wall_ahead(body.feet, body.forward(), &collision) {
                if pad.jump_buffered() {
                    pad.consume_jump();
                }
                // in the FreeRun sub-state, standing (HG+1492 = 0), event 49 plays the Ground lead-in first
                // (`FreeRun_WallingGuardA` 0xDA2DD0 → 0xD89AA0; live RE/25 #49 `wall41_legs_hold`); from Movement it
                // switches at once (`Movement_HandleEvent` 0xDB1AB6 → `ToWalling` 0xDA7B90)
                if g.extra.is_some_and(|e| e.mode == super::ground_extras::ExtraMode::FreeRun) {
                    if let Some(b) = super::ledge_moves::single_item(super::walling::GROUND_LEAD_IN, 0) {
                        g.extra = None;
                        g.play_oneshot(b);
                        g.owner = TreeState::Other;
                        g.wall_start = Some((contact, normal, slope));
                        body.velocity = Vec3::ZERO;
                        continue;
                    }
                }
                let from = body.feet;
                switch_context(&mut loco, &mut data, TransitionSetup::ToWalling(super::walling::WallingEntry { contact, normal, slope, from, warp_duration: None }));
                continue;
            }
        }

        // ---------------------------------------------------------------- free-run target jump (0xEE7F7A)
        // No press: high profile, the stick past the dead zone, the Jump ability, IHumanGround vt16 (CanHandleEvent
        // 45) → IHuman vt56 with kind 4 (Legs held) / 3, ladders left out, then the scorer (mode 1) → vt24. Its box
        // only reaches 0.45–1.3 m up and 0.5–2.0 m (1.3 m) ahead: the game's hop onto low obstacles (RE/18 §1.5).
        if g.high_profile && moving && !busy && ab.allows(Ability::Jump) {
            if let Some(target) = super::targets::find_target(body.feet, body.forward(), pad.dir_snap, &super::jump_candidates::Query::free_run(pad.legs_held), false, &guidance, &collision) {
                jump_log("free-run target jump", body.feet, Some(&target));
                match g.event45(body.feet, body.forward(), ImpulsionJump::Target(target), free_run_ready, leading_foot) {
                    Event45::Run => {
                        let entry = InAirEntry::JumpToTarget { from: body.feet, target, speed_param: g.speed_param, foot_left: run_jump_foot_left(g.sub_state, leading_foot, g.blend.foot) };
                        switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
                        continue;
                    }
                    Event45::Impulsion => {
                        body.velocity = Vec3::ZERO;
                        continue;
                    }
                    Event45::Refused => {}
                }
            }
        }

        // ---------------------------------------------------------------- ledge stop (event 69 / sub-state 38)
        if let Some(ls) = g.ledge_stop {
            // pull-down EdgeStop (LedgeStop_HandleEvent 0xDA4D90, event 70). Sender (0xEE8977–0xEE89AF): in the ledge
            // stop (vt752 `HumanGround__IsInLedgeStop` 0xD8A990) the stick past 0.35 and within 70° of the edge's
            // normal, with the Climb ability. The guard (0xD9D580) takes it only on the frame the start action
            // (0x06E8BD7F) is done and still the playing item: still pushing into the edge then hangs from it,
            // having let go the end action steps back.
            let start_done = g.oneshot.is_some_and(|o| o.blend.id == super::ledge_moves::LEDGE_STOP_START && o.t + dt >= o.duration);
            let toward = pad.magnitude > crate::tuning::STICK_DEADZONE && pad.dir.dot(Vec3::new(ls.normal.x, 0.0, ls.normal.z).normalize_or_zero()) > EDGE_HALT_COS;
            if !ls.ending && start_done && toward && ab.allows(Ability::Climb) {
                if let Some(entry) = pulldown_entry(ls.point, ls.normal, body.feet, false, &guidance, &collision) {
                    g.ledge_stop = None;
                    g.oneshot = None;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(entry));
                    continue;
                }
            }
            if g.oneshot.is_none() {
                // end done → Movement (Locomotion_Update 0xDAF2D0, sub_5017B0); start → end is in the move step
                g.ledge_stop = None;
            }
        }
        if let Some(n) = g.ledge_stop_lock {
            if pad.speed01 <= 0.0 || pad.dir.dot(Vec3::new(n.x, 0.0, n.z).normalize_or_zero()) < FRONT_COS {
                g.ledge_stop_lock = None;
            }
        }
        // Movement event 69 (0xDB1470): guard 0xDA5DE0 = front edge (ClassifyEdgeSide 0xD9D7F0 → 1), drop > 2 m,
        // body space → ToLedgeStop 0xDA99F0. Side edges (3/4, guard 0xDA5D90) go to another state (not ported).
        // Sender: the input interpreter (0xEE8899 → IHumanGround vt748 `HumanGround__DoLedgeStop` 0xDBCAD0): a front
        // edge (vt744) within 0.15 m (0.0225 = 0.15²) with a drop of more than 5 m, in either profile, every frame
        // without a stick test: a run stop sliding to the edge turns into the ledge stop (RE/25 #12: live, released
        // late the jog stop became `xx_h_ledge_stop_start` within 0.1 s; it never carries the body off). Free running
        // (high profile + Legs) jumps first (the interpreter's jump branch returns before it). In low profile the edge
        // halt above comes first (its reach is larger) and its look-down returns before the sender.
        let run_stopping = g.oneshot.is_some_and(|o| jump_blend::RUN_STOP.contains(&o.blend.id) || jump_blend::RUN_STOP_TO_WAIT.contains(&o.blend.id));
        let busy = g.oneshot.is_some_and(|o| !is_start(o.blend.id)) && !run_stopping;
        if (moving || run_stopping) && !busy && !edge_halt && g.ledge_stop_lock.is_none() && !(g.high_profile && pad.legs_held) {
            if let Some((p, n)) = edge_report_drop(body.feet, body.forward(), REPORT_PICK_RADIUS, LEDGE_STOP_SQ, FRONT_COS, LEDGE_STOP_DROP, &guidance, &collision).map(|r| (r.0, r.1)) {
                if let Some(b) = super::ledge_moves::single_item(super::ledge_moves::LEDGE_STOP_START, 0) {
                    g.ledge_stop = Some(LedgeStop { point: p, normal: n, ending: false });
                    g.ledge_stop_lock = Some(n);
                    g.speed_param = 0.0;
                    g.blend.speed_param = 0.0;
                    g.play_oneshot(b);
                    continue;
                }
            }
        }

        // ---------------------------------------------------------------- pull-down (event 70)
        // Sender (interpreter 0xEE8934–0xEE8A6D, after the step off, event 68): the edge report, the Climb ability,
        // vt764 (0xDB63D0, CanHandleEvent(70): guard 0xD9D6C0, facing along the edge's outward normal, a drop of more
        // than 2 m, body space) and, out of the ledge stop, the empty hand pressed within the last 0.2 s
        // (`sub_ED4A20(3, 0.2)`, dword_170861C). Either profile.
        if pad.hand_pressed_ago <= PULLDOWN_HAND_WINDOW && !busy && ab.allows(Ability::Climb) {
            // the pull-down report (`sub_DDCB20`: ladders carved, steep removed; RE/24 GUIDE-2)
            if let Some(entry) = try_pulldown(body.feet, body.forward(), &guidance.ledge_report_depth(body.feet, 2.5, true, crate::guidance::PULLDOWN_DEPTH, &collision), &collision) {
                pad.consume_hand();
                switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(entry));
                continue;
            }
        }

        // ---------------------------------------------------------------- jump requests
        // The interpreter's jump branch (0xEE817E): high profile, the Legs buffer (+0x1126: set on the press,
        // `GoAssassinActionInterpreter__ReadInput` 0xEEDF80, cleared 0.3 s after it) **with Legs no longer held**, and
        // the stick past the dead zone: a tap jumps when it is let go, holding Legs free-runs instead. While Legs is held
        // the free run jumps at an edge (`v142`, 0xEE7DCE: an edge report within 0.25 m with a drop over 0.5 m, within
        // 80° of the stick, Legs held or HG+588). The edge test is the vt136 report below.
        let forward = body.forward();
        // the foot-level edge reports (IHuman vt132 0xB1BC50: 0.75 m, drops of 0.45 m or more, with the floor report:
        // a6 = 1, 0xEE7952) and the one along the facing (vt136, any angle): the free run's edge jump `v142`, the edge
        // jump and event 68 below read it
        let reports = super::jump_candidates::edge_reports(body.feet, 0.75, 0.45, Some(forward), &guidance, &collision);
        let facing_report = super::jump_candidates::pick_report(&reports, body.feet, forward, 0.75, std::f32::consts::PI);
        // v142 (0xEE7DCE): a report with a drop over 0.5 m within 0.25 m, its normal within 80 degrees of the stick,
        // Legs held
        let edge_jump = facing_report.is_some_and(|r| r.drop > 0.5 && r.sq() < 0.25 && r.normal.dot(pad.dir) > 0.173_648_18) && pad.legs_held;
        let want_jump = g.high_profile
            && moving
            && ((pad.jump_buffered() && !pad.legs_held) || edge_jump);
        if want_jump && !busy && ab.allows(Ability::Jump) {
            pad.consume_jump();
            // the candidates, then the Leap of Faith search under its ability (0xEE8239: `sub_D32760`, IHuman vt76)
            let want = if moving && pad.dir_snap != Vec3::ZERO { pad.dir_snap } else { forward };
            let found = super::targets::find_target(body.feet, want, want, &super::jump_candidates::Query::TAP, false, &guidance, &collision)
                .or_else(|| ab.allows(Ability::LeapOfFaith).then(|| super::targets::haystack_target(body.feet, want, forward, &guidance, &collision)).flatten());
            jump_log("tap jump", body.feet, found.as_ref());
            // event 45 (vt24; without a target through vt28 with the free target): the impulsion or the run jump by the
            // playing item
            let jump = match found {
                Some(t) => ImpulsionJump::Target(t),
                None => ImpulsionJump::Free { dir: want },
            };
            match g.event45(body.feet, forward, jump, free_run_ready, leading_foot) {
                Event45::Run => {}
                Event45::Impulsion => {
                    body.velocity = Vec3::ZERO;
                    continue;
                }
                Event45::Refused => continue,
            }
            let entry = match found {
                // the takeoff foot (`StartRunJumpToTarget` 0xD837A0: the playing item's word bits 2–3, 1 = the footl
                // takeoff): the foot opposite the locomotion's (live RE/25: three free-run take-offs, the game always
                // on the other foot from the port's former "same foot" reading)
                Some(t) => InAirEntry::JumpToTarget { from: body.feet, target: t, speed_param: g.speed_param, foot_left: run_jump_foot_left(g.sub_state, leading_foot, g.blend.foot) },
                // vt28: free jump without a target, along the direction the candidates were searched in (IHuman vt68
                // gets the query's own direction pointer, 0xEE842D / 0xEE81CE; live RE/25 freerun_east: the stick's
                // (0.999, −0.038) while the body still faced 12° to the right)
                None => InAirEntry::FreeJump { from: body.feet, dir: want, ahead: FREE_JUMP_AHEAD, speed_param: g.speed_param, foot_left: g.blend.foot != 0, tap: true },
            };
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            continue;
        }

        // ---------------------------------------------------------------- at an edge: the edge jump, event 68
        if !busy {
            if let Some(r) = facing_report {
                // The edge jump (0xEE8679): at or past the edge (+52 < 0.01), the stick past the dead zone and both
                // the stick and the facing within 45 degrees of the edge normal → IHuman vt56 kind 2 (high) / 1 along
                // the facing, with beams; scorer mode 1 with the far-behind flag (a7 = 1). It sets +4399, which skips
                // event 68 this frame.
                let flat_fwd = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
                if r.sq() < 0.01 && moving && ab.allows(Ability::Jump) && pad.dir.dot(r.normal) > std::f32::consts::FRAC_1_SQRT_2 && flat_fwd.dot(r.normal) > std::f32::consts::FRAC_1_SQRT_2 {
                    if let Some(target) = super::targets::find_target(body.feet, forward, flat_fwd, &super::jump_candidates::Query::edge(g.high_profile), true, &guidance, &collision) {
                        jump_log("edge jump", body.feet, Some(&target));
                        let entry = InAirEntry::JumpToTarget { from: body.feet, target, speed_param: g.speed_param, foot_left: run_jump_foot_left(g.sub_state, leading_foot, g.blend.foot) };
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
                        continue;
                    }
                }
                // Event 68 `HumanGround__Guard_Event68_JumpToEdge` 0xD84190: a drop of 0.53 m or more, the feet 1 cm
                // past the edge, room for the body beyond it (0xB2E7A0) → `ToInAir_JumpToEdge` 0xDA7A10 / 0xD9D1F0:
                // the step off the edge. PORT: HG+609 bit 0 (set by some state enters) is taken to be clear outside
                // one-shot actions. Not while a step-up holds (`sub_B0EBC0`, RE/24 GUIDE-15).
                // The floor report (+56, no edge) needs instead the box at the feet + 0.7 m up, half 0.25 across, 0.25
                // along the forward and 1.0 up, empty (`Human__BoxQueryNearestFromRear` 0xB2A070 == 1000, 0xD8420E).
                let room = if r.floor {
                    collision.obb_free(r.point + Vec3::Y * 0.7, [super::right_of(r.normal), r.normal, Vec3::Y], Vec3::new(0.25, 0.25, 1.0))
                } else {
                    r.sq() < -0.01 && super::jump_candidates::room_past_edge(&r, &collision)
                };
                if r.drop >= 0.53 && g.step_up.is_none() && room {
                    // the takeoff foot is the one opposite the leading foot: `sub_B27180`(…, 2 − (GetLeadingFoot() != 1))
                    // 0xD9D4B9, so a left foot ahead takes off on `footr` (RE/24 AIR-28). The leading foot is the bone
                    // test of 0xB18850 on the pose; without a pose the locomotion's (MoveBlend foot 0 = left ahead)
                    let takeoff_left = leading_foot.map_or(g.blend.foot != 0, |f| f != 0);
                    if let Some(entry) = step_off_entry(body.feet, flat_fwd, &r, g.high_profile, g.speed_param, takeoff_left) {
                        jump_log("step off (event 68)", body.feet, None);
                        switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
                        continue;
                    }
                }
            }
        }

        // ---------------------------------------------------------------- into a haystack (event 122)
        // The interpreter's last request (0xEE8E08, either profile): the stick past the dead zone and IHumanGround
        // vt1592 (event 122's guard 0xD8F0B0: touching a haystack, facing it within 100 degrees, the stick within 45
        // degrees into it) → vt1596 → HumanHayStack, entry Ground (0xD84F80).
        // the interpreter sends it with the stick past the dead zone (0xEE8E08), also when the haystack has stopped the walk
        if pad.speed01 > 0.0 && !busy {
            if let Some(e) = super::hay::ground_entry(body.feet, body.forward(), pad.dir, &body.proxy.manifold, &guidance.haystacks, &collision) {
                switch_context(&mut loco, &mut data, TransitionSetup::ToHayStack(e));
                continue;
            }
        }

        // ---------------------------------------------------------------- move (blended clip root motion)
        let stopping = g.oneshot.is_some_and(|o| jump_blend::RUN_STOP.contains(&o.blend.id) || jump_blend::RUN_STOP_TO_WAIT.contains(&o.blend.id));
        let (delta, speed) = if let Some(mut os) = g.oneshot {
            // landing / reception action: its blended root motion (FROMANIM)
            os.t += dt;
            // the action's displacement and root yaw are in the heading it started with (FROMANIM)
            // (the start item follows the steering: its frame is the current heading)
            let h0 = if is_start(os.blend.id) { body.heading } else { *os.h0.get_or_insert(body.heading) };
            let yaw = os.blend.yaw(os.t / os.duration.max(1e-4));
            if yaw.abs() > 1e-4 {
                body.heading = wrap_angle(h0 + yaw);
            }
            let d = os.blend.disp(os.t / os.duration.max(1e-4));
            let step = [d[0] - os.applied[0], d[1] - os.applied[1]];
            os.applied = d;
            // the wall run's lead-in holds its last frame until the switch (the next frame's start)
            g.oneshot = (os.t < os.duration || (g.wall_start.is_some() && os.blend.id == super::walling::GROUND_LEAD_IN)
                || (g.impulsion_jump.is_some() && jump_blend::IMPULSION_JUMP.contains(&os.blend.id))).then_some(os);
            if g.oneshot.is_none() {
                if let Some(next) = g.oneshot_next.take() {
                    g.play_oneshot(next);
                }
                if let Some(h) = g.face_next.take() {
                    body.heading = h;
                }
                if let Some(mut ls) = g.ledge_stop.filter(|l| !l.ending) {
                    // start done → end action, same frame (HumanGround__LedgeStop_PlayEnd 0xD7D9D0)
                    ls.ending = true;
                    g.ledge_stop = Some(ls);
                    if let Some(b) = super::ledge_moves::single_item(super::ledge_moves::LEDGE_STOP_END, 0) {
                        g.play_oneshot(b);
                    }
                }
                if GAME_GROUND_TREE && g.oneshot.is_none() && g.wall_start.is_none() {
                    let mut heading = body.heading;
                    g.after_oneshot(os.blend, mv_speed > 0.0, stick_turn, &mut heading);
                    body.heading = heading;
                }
            }
            let f0 = Vec3::new(-h0.sin(), 0.0, -h0.cos());
            let right = super::right_of(f0);
            // while the animation blends into it, the action's displacement ramps in with the weight (BLENDAB under
            // ASTOPBROLL: the frozen outgoing pose adds none; RE/24 ANIM-16, `sub_774350`)
            let k = crate::anim::root_weight(anim, os.blend.id);
            let delta = (right * step[0] + f0 * step[1]) * k;
            (delta, delta.length() / dt.max(1e-4))
        } else if let Some(mut t) = g.tr {
            // the transition action's root motion (FROMANIM), in the current heading: the code turns the body while it
            // plays unless its item word has 0x10 (then its root yaw does)
            let incoming = anim.map_or(1.0, |p| p.root_shares_for(t.action.id).1);
            let (d, yaw, ended) = t.advance(g.speed_param, g.pivot_turn, incoming, dt);
            if yaw.abs() > 1e-6 {
                body.heading = wrap_angle(body.heading + yaw);
            }
            let f0 = body.forward();
            let delta = super::right_of(f0) * d[0] + f0 * d[1];
            g.tr = if ended {
                if !t.to_wait {
                    // the locomotion item the transition hands over to (ActionTransition +16), from its start; on the
                    // item's last frame MoveBlend already runs its locomotion path (0xDA09BD), so the weights are this
                    // frame's
                    g.blend.foot = t.dest_item.min(1);
                    g.blend.phase = 0.0;
                    g.blend.update_weights(target, 0.0);
                }
                None
            } else {
                Some(t)
            };
            (delta, delta.length() / dt.max(1e-4))
        } else {
            let speed = if g.speed_param > 0.0 { g.blend.advance(dt) } else { 0.0 };
            // the locomotion's displacement ramps in the same way when the animation blends into it from another action
            let speed = speed * crate::anim::root_weight(anim, super::move_blend::ACT_GROUND_LOCOMOTION);
            (forward * speed * dt, speed)
        };
        // PORT: after a ledge stop, still pushing into the same edge holds the character at it (the interpreter
        // re-sends event 69 while its edge test holds, 0xEE8899; the port does not loop stop / step back)
        // The interpreter's own edge stop (see the halt above) holds the walk at the edge without a clip.
        let held = g.oneshot.is_none()
            && g.tr.is_none()
            && (edge_halt
                || (g.ledge_stop_lock.is_some() && edge_report_drop(body.feet, forward, REPORT_PICK_RADIUS, LEDGE_STOP_SQ, FRONT_COS, LEDGE_STOP_DROP, &guidance, &collision).is_some()));
        let (delta, speed) = if held {
            g.speed_param = 0.0;
            g.blend.speed_param = 0.0;
            (Vec3::ZERO, 0.0)
        } else {
            (delta, speed)
        };
        let before = body.feet;
        let mut r = { let b = &mut *body; collision.move_capsule(&mut b.proxy, b.feet, delta, true, dt) };
        // PORT: the run stop's root motion (≈0.5 m of slide) does not carry the character off a roof edge; it stops at
        // the last supported position. The game has no guard: its root motion carries the body until the fall-off rule
        // or event 68's floor branch fires (RE/24 GROUND-1 / 2 / 31). LIVE (RE/20 #12): the visual check only.
        if stopping && collision.ground_support(r.position).is_none() {
            r.position = before;
        }
        // ObstacleCollision (sub-state 5, the collide and lean). The sender (live-traced RE/25 GROUND-32: walking into
        // the Bureau crate, `DoObstacleCollision` returns to 0xEE8DB2) is the interpreter's walking path
        // (`ProcessGroundMovement` 0xEE8C83–0xEE8DB0), not its dead jump-branch one (0xEE83B5): the Jump ability, the
        // stick past 0.35, `HumanGround__CanObstacleCollision` (vt104 0xDB5E70 = CanHandleEvent(42) along the stick:
        // a controller wall contact within 45°), then the obstacle query along the stick, reach 0.6
        // (`Human__ObstacleQuery` 0xB27900); a plain obstacle (no guidance edge, flag 1) only in high profile. So a
        // crate with ledge guidance collides in either profile and a plain wall only in high profile (both live).
        // The wall run and the jumps above go first.
        // Movement takes event 42 with only its contact guard (`Movement_CanHandleEvent` 0xDAF731 → 0xD838F0 →
        // `Human__GetObstacleContact` 0xB25230: a controller contact within 45° of the event's direction, ≥ 0.5 m above the
        // feet), no anim gate: it starts again during the push-away's transition into the walk (unlocked), not during the
        // locked push-away itself. The direction is the stick, so standing along the wall and pushing into it turns the
        // character to face it (the entry's 0.15 s warp).
        let unlocked = g.oneshot.is_none_or(|o| !super::anim_gate::locked(&o.blend)) && g.tr.is_none_or(|t| !super::anim_gate::locked(&t.action));
        let event42 = ab.allows(Ability::Jump) && pad.magnitude > crate::tuning::STICK_DEADZONE && unlocked && g.ledge_stop.is_none()
            && body.stick_residual.abs() <= 0.2 && super::collide::has_obstacle_contact(&body.proxy.manifold, pad.dir);
        if event42 {
            if let Some(o) = super::collide::obstacle_query(r.position, pad.dir, OBSTACLE_REACH, &guidance, &collision).filter(|o| o.guided || pad.high_profile) {
                if let Some(c) = super::collide::enter(r.position, body.heading, o.contact, o.normal, o.height, g.pose_seq) {
                    g.pose_seq = c.seq;
                    g.collide = Some(c);
                    g.tr = None;
                    g.oneshot = None;
                    g.oneshot_next = None;
                    g.owner = TreeState::Other;
                    g.speed_param = 0.0;
                    g.blend.speed_param = 0.0;
                    body.feet = r.position;
                    body.velocity = Vec3::ZERO;
                    continue;
                }
            }
        }
        if let Some(ls) = g.ledge_stop {
            // PORT: the stop clip's root motion may not carry the feet past the edge (the game places the edge
            // report the interpreter hands event 69, 0xEE8899, so the clip ends on it)
            let past = (r.position - ls.point).dot(ls.normal) + LEDGE_STOP_MARGIN;
            if past > 0.0 {
                r.position -= Vec3::new(ls.normal.x, 0.0, ls.normal.z) * past;
            }
        }
        // the controller's velocity is what it actually moved (blocked by a wall → slower, sliding → along it)
        let moved = Vec3::new(r.position.x - before.x, 0.0, r.position.z - before.z) / dt.max(1e-4);
        body.velocity = if moved.length() < speed { moved } else { forward * speed };
        body.feet = r.position;

        // stick to ground (0x57D240): the capsule set on the support up to 0.37 m above / 0.58 m below the feet. In
        // Movement the only way off the ground is the fall-off rule (`HumanGround__State1_Update` 0xDB46B0 →
        // `Human__ShouldFallOffSupport` 0xB23CB0): no contact flatter than 45° and no floor 0.8 m below, i.e. the
        // capsule's rounded bottom has rolled off the rim (its centre ≈ r·sin 45° = 0.28 m past the edge). The edge-line
        // ground loss (`ground_loss`, 0xD87720) is gated by GroundData+760, which only the fight's grabbed reaction sets.
        body.stick_residual = 0.0;
        body.stick_normal_y = None;
        match collision.ground_support(body.feet) {
            Some(s) => {
                let residual = body.feet.y - s.y;
                body.stick_residual = if residual.abs() <= 0.0005 { 0.0 } else { residual };
                body.stick_normal_y = Some(s.normal_y);
                // a step up by more than 0.25 m starts the tracker (0xB26A30)
                let contact = (body.stick_residual < -0.25).then(|| collision.support_contact(body.feet)).flatten();
                let feet = Vec3::new(body.feet.x, s.y, body.feet.z);
                g.step_up = StepUp::capture(body.stick_residual, contact, feet).or(g.step_up);
                body.feet.y = s.y;
                body.grounded = true;
            }
            None => {
                body.grounded = false;
                // `HumanGround__TransitionToInAirOffSupport` 0xD8ADB0: InAir kind 3 (sub-state 3 keeps the current clip
                // for its remaining length, 0xE00EF0, then the fall), no drop report, so no drop sub-state or drop steer.
                // The animator completes the active item, then uses the upper-body falling blend (0xE00EF0).
                let entry = InAirEntry::Fall { from: body.feet, velocity: body.velocity, origin: FallOrigin::Ground, speed_param: g.speed_param };
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            }
        }
    }
}

/// `GoAssassinActionInterpreter` wall slide (0xEDD7C0). Only with the stick-to-ground residual within 0.2 m: the
/// controller contact whose normal faces the stick most directly, within 90 deg (`sub_4F8EA0`, contact mask 4; PORT:
/// contacts steeper than 45 deg), at least 0.55 m above the feet. Its flat normal n against the stick d: within 135 deg
/// of n the stick turns along the wall; beyond, d is turned straight into the wall and the wanted speed is 0 (result
/// 2: SetDestSpeedRatio(0) and the run stop allowed).
pub fn wall_slide(body: &Body, dir: Vec3, speed01: f32) -> (Vec3, f32) {
    let d = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if speed01 <= 0.0 || d == Vec3::ZERO || body.stick_residual.abs() > 0.2 {
        return (dir, speed01);
    }
    let mut best: Option<(f32, Vec3)> = None;
    for c in &body.proxy.manifold {
        if c.normal.y.abs() >= std::f32::consts::FRAC_1_SQRT_2 || c.pos.y < body.feet.y + 0.55 {
            continue;
        }
        let a = (-d).dot(c.normal).clamp(-1.0, 1.0).acos();
        if a <= std::f32::consts::FRAC_PI_2 && best.is_none_or(|(b, _)| a < b) {
            best = Some((a, c.normal));
        }
    }
    let Some((_, normal)) = best else { return (dir, speed01) };
    let n = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
    if n == Vec3::ZERO {
        return (dir, speed01);
    }
    let into = n.dot(d).clamp(-1.0, 1.0).acos();
    if into <= std::f32::consts::PI - std::f32::consts::FRAC_PI_4 {
        let t = (d - n * d.dot(n)).normalize_or_zero();
        if t != Vec3::ZERO { (t, speed01) } else { (dir, speed01) }
    } else {
        (-n, 0.0)
    }
}

/// The drop report of `IHuman` vt104 (`Human__ReportDropAtFeet` 0xB248B0): the edges of a 0.75 m sphere zone at the
/// feet (every type, mask −1), none steep (`Guidance__IsNotSteep`), each at its point nearest the feet with |dz| < 0.3
/// and a wall normal; of those whose drop (`Human__MeasureDropBeyondEdge`(feet, point, normal, 7, 0.02) with its lip and
/// line checks) is at least `min_drop`, the one nearest the feet: its point (+16), normal (+32), drop (+48) and signed
/// distance (+52: the SQUARED distance, `sub_42EC70`, negative once the feet are past the edge; +56 = 0). With no such
/// edge, unless an edge was refused for its drop, the drop straight under the feet (+52 = 0, +56 = 1, `normal` None).
/// Callers: `HumanGround__CheckGroundLoss` 0xD87720 (minimum 0.5) and the pilotis air catch (0xE0C583, minimum 0.5).
#[derive(Clone, Copy, Debug)]
pub struct DropReport {
    pub point: Vec3,
    pub normal: Option<Vec3>,
    pub drop: f32,
    pub signed: f32,
}

pub fn drop_report(feet: Vec3, min_drop: f32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<DropReport> {
    let mut best: Option<(f32, DropReport)> = None;
    let mut refused = false;
    for e in guidance.edges.iter().filter(|e| !crate::guidance::steep(e.p1 - e.p0)) {
        let q = e.closest_point(feet);
        if q.distance(feet) > 0.75 || (q.y - feet.y).abs() >= 0.3 {
            continue;
        }
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        if n == Vec3::ZERO {
            continue;
        }
        let drop = super::jump_candidates::measure_drop_from(Some(feet), q, n, 7.0, 0.02, collision, Some(guidance));
        if drop < min_drop {
            refused = true;
            continue;
        }
        let d2 = q.distance_squared(feet);
        if best.is_none_or(|b| d2 < b.0) {
            let signed = if n.dot(feet - q) > 0.0 { -d2 } else { d2 };
            best = Some((d2, DropReport { point: q, normal: Some(n), drop, signed }));
        }
    }
    if let Some((_, r)) = best {
        return Some(r);
    }
    if refused {
        return None;
    }
    // no edge: the drop straight under the feet (flag +56)
    let drop = super::jump_candidates::measure_drop_from(Some(feet), feet, Vec3::ZERO, 7.0, 0.02, collision, Some(guidance));
    (drop >= min_drop).then_some(DropReport { point: feet, normal: None, drop, signed: 0.0 })
}

/// `HumanGround__CheckGroundLoss` 0xD87720: with the drop report (vt104, minimum 0.5 m), the ground is lost once the
/// feet are on or past the edge (signed distance < 0.01) and either there was no edge (a drop right under the feet) or
/// the horizontal velocity does not point back from the edge (dot(normal, v) ≥ 0). So the fall starts at the edge
/// line, not when the capsule's rounded bottom slides off the rim. Its guard (`HumanGroundData__IsGrabbedGroundLoss`
/// 0xC7F150, GroundData+760) is only set while grabbed in a fight (`HumanGround__Fight_EnterGrabbed` 0xD83B80), so
/// normal movement never uses it; the fight is not ported. (The cached report of Human+2801 is not modelled.)
#[allow(dead_code)]
pub fn ground_loss(feet: Vec3, velocity: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> bool {
    let Some(DropReport { normal: n, signed: dist, .. }) = drop_report(feet, 0.5, guidance, collision) else { return false };
    if dist >= 0.01 {
        return false;
    }
    let v = Vec3::new(velocity.x, 0.0, velocity.z).normalize_or_zero();
    n.is_none_or(|n| n.dot(v) >= 0.0)
}

/// Ground → Climb / Ledge / jump-to-ledge when pushing into a wall with high profile + Legs.
/// Order (hypothesis from the interpreter's request order, RE/01 §6.3):
/// 1. climb start: hand holds 1.8–2.4 m up and foot holds 1.2 m below them (vt764/768, FromGround);
/// 2. a ledge with the hands 0.7–3.0 m up → the standing straight jump at it (0xB21DA0 bands: knee / waist
///    heights pull straight up onto the top, higher ones end hanging).
/// The climb start from the ground (event 50, guard `sub_D83920` → `sub_B2E860`). The edges of a report box at the
/// root facing the character (±0.75 m to the side, 0 … 1.2 m ahead, −0.9 … +2.1 m up, whole edges); three passes, the
/// box 0.95 m ahead facing forward, then 0.8 m ahead facing 90° left and 90° right (a side wall), each at the feet's
/// height + 0, +0.6, −0.6 m: a feet hold (box half extents 0.25 / 0.75 / 0.3, the reference 0.2 m back, 45°, mask 50),
/// then a hand hold 1.2 m above it (0.25 / 0.3 / 0.3). Both hands on the hand hold, both feet on the feet hold.
/// Then (0xB2E860): the climb pose from the holds (`Human__ComputeClimbPoseFromHolds` 0xB1CC20) must have room
/// (`climb_pose_room`, `sub_B2E160`); on the side passes the way 1.2 m back from the side wall must be clear
/// (`sub_B15ED0` from the root to the root − 1.2·facing at 0.2 and 1.2 m up), and from there (the root on the front
/// pass) the rays to the feet and hand holds 0.1 m out from them (`sub_B15ED0`).
pub(super) fn climb_start_entry(feet: Vec3, forward: Vec3, foot_right: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<ClimbEntry> {
    let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let report = guidance.report(feet, forward, Vec3::new(-0.75, 0.0, -0.9), Vec3::new(0.75, 1.2, 2.1), false);
    let right = super::right_of(forward);
    let mask = crate::guidance::MASK_CLIMB_HOLDS;
    for (pass, (ahead, facing)) in [(0.95f32, forward), (0.8, -right), (0.8, right)].into_iter().enumerate() {
        for k in [0.0f32, 0.6, -0.6] {
            let c = feet + forward * ahead + Vec3::Y * k;
            let Some(foot) = report.probe_facing(c, facing, Vec3::new(0.25, 0.75, 0.3), 0.2, 0.785_398_2, mask) else { continue };
            let Some(hand) = report.probe_facing(foot.point + Vec3::Y * 1.2, facing, Vec3::new(0.25, 0.3, 0.3), 0.2, 0.785_398_2, mask) else { continue };
            let pose = super::climb::climb_pose([hand.point, hand.point, foot.point, foot.point],
                [hand.wall_normal, hand.wall_normal, foot.wall_normal, foot.wall_normal], facing);
            if !climb_pose_room(pose.root, pose.forward, pose.up, collision) {
                continue;
            }
            let back = feet - facing * 1.2;
            if pass != 0 && !two_rays_clear(feet, back + Vec3::Y * 0.2, back + Vec3::Y * 1.2, collision) {
                continue;
            }
            let from = if pass != 0 { back } else { feet };
            if !two_rays_clear(from, foot.point + foot.wall_normal * 0.1, hand.point + hand.wall_normal * 0.1, collision) {
                continue;
            }
            return Some(ClimbEntry {
                entry_type: ClimbEntryType::FromGround,
                pose_2m: None, end_action: None,
                hand_l: hand.point,
                hand_r: hand.point,
                foot_l: foot.point,
                foot_r: foot.point,
                normal: hand.wall_normal,
                from_feet: feet,
                foot_right,
                action: None,
            });
        }
    }
    None
}

/// `sub_B2E160`: room for a climb pose (root, forward F, up U): two boxes along U and F (`Human__BoxQueryEmpty`), the
/// body at the root + 0.3·U (half extents 0.375 across, 0.25 along F, 0.7 along U) and the head at the root + 1.4·U
/// (0.15 / 0.15 / 0.4).
fn climb_pose_room(root: Vec3, forward: Vec3, up: Vec3, collision: &CollisionWorld) -> bool {
    let right = forward.cross(up).normalize_or(super::right_of(forward));
    let axes = [right, forward, up];
    collision.obb_free(root + up * 0.3, axes, Vec3::new(0.375, 0.25, 0.7)) && collision.obb_free(root + up * 1.4, axes, Vec3::new(0.15, 0.15, 0.4))
}

/// `sub_B15ED0`: from 0.2 m above `from` to `low` and from 1.2 m above it to `high`, both rays clear (`Human__RayClear`,
/// the main character's layer).
fn two_rays_clear(from: Vec3, low: Vec3, high: Vec3, collision: &CollisionWorld) -> bool {
    let clear = |a: Vec3, b: Vec3| {
        let d = b - a;
        let len = d.length();
        len < 1e-4 || collision.ray_distance(a, d / len, len, crate::layers::MAIN_CHARACTER) >= len
    };
    clear(from + Vec3::Y * 0.2, low) && clear(from + Vec3::Y * 1.2, high)
}

/// The beam's obstacle hand target (`sub_F7DB30`, from event 17 on a beam: 0xEE9AF0 → 0xF7EF90 → 0xF70E50 →
/// 0xB21DA0): the edges of a report box at the stand point facing `dir` (±0.3 m to the side, 0 … 0.7 m ahead,
/// 0.3 … 1.4 m up, whole edges), not Quadruped, Kiosk or Rope, the wall facing `dir` within 45° and no steeper than
/// 40° (0xB12480). The hold: the stand point carried across onto the edge line, then the nearest point of the edge
/// shortened 0.5 m at both ends. A wall right over the edge (the top ray of `static_target_type`: from the hold + 0.5·n
/// + 0.3 m up back toward the wall, 1.75 m, less 0.5 under 0.3) rejects it. `Human__PickStaticReport` 0xE97720 picks
/// one (`choose_static_report`), and the straight jump takes it.
/// The obstacle contact the search needs first (`Human__GetObstacleContact` 0xB25230) is the caller's test.
pub fn beam_obstacle_target(stand: Vec3, dir: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<JumpTarget> {
    use crate::guidance::GuidanceSubType as S;
    let dir = dir.with_y(0.0).normalize_or_zero();
    let report = guidance.report(stand, dir, Vec3::new(-0.3, 0.0, 0.3), Vec3::new(0.3, 0.7, 1.4), false);
    let mut reports = Vec::new();
    for e in &report.edges {
        if matches!(e.subtype, S::Quadruped | S::Kiosk | S::Rope) {
            continue;
        }
        let low = if e.n1.y <= e.n0.y { e.n1 } else { e.n0 };
        let low = low.with_y(0.0).normalize_or_zero();
        if low == Vec3::ZERO || dir.angle_between(-low) >= std::f32::consts::FRAC_PI_4 {
            continue;
        }
        let d = e.p1 - e.p0;
        let len = d.length();
        if crate::guidance::steep(d) {
            continue;
        }
        let u = d / len;
        let on_line = e.p0 + u * (stand - e.p0).dot(u);
        let hold = if len <= 1.0 { (e.p0 + e.p1) * 0.5 } else { crate::guidance::closest_on_segment(e.p0 + u * 0.5, e.p1 - u * 0.5, on_line) };
        let top = collision.ray_distance(hold + low * 0.5 + Vec3::Y * 0.3, -low, 1.75, crate::layers::MAIN_CHARACTER) - 0.5;
        if top < 0.3 {
            continue;
        }
        reports.push((hold, low));
    }
    let i = super::ground_extras::choose_static_report(stand, dir, &reports)?;
    straight_target(stand, reports[i].0, reports[i].1, collision, true)
}

fn straight_target(feet:Vec3,point:Vec3,n:Vec3,collision:&CollisionWorld,beam:bool)->Option<JumpTarget> {
    let wall = super::ledge::hang_type_at(point, n, collision) == super::ledge::LedgeHangType::Wall;
    let dz = point.y - feet.y;
    let j = if beam { super::ledge_moves::hang_jump_in_beam(dz, wall)? } else { super::ledge_moves::hang_jump_in(dz, wall)? };
    if std::env::var_os("AC_ANIM_LOG").is_some() {
        info!("straight target: hands {point:.2} n {n:.2} dz {dz:.2} wall {wall} -> out {:.2} down {:.2}", j.out, j.down);
    }
    Some(JumpTarget { position: point + n * j.out - Vec3::Y * j.down, type_flags: j.flags, hang: Some((point, n)), straight: Some(j), pass: None, ladder: None, support: None })
}

/// The standing request's hand targets (`Human__EvaluateStaticHandTargets` 0xE165D0; the box initialised by
/// 0x1610920 / 0x1610970 and consumed by 0xB10990: the feet + 0.5 m ahead + 1.85 m up, half extents 0.5 / 1.0 / 1.35).
/// The box's edges of mask 306 cut to it, none steeper than 40°, then the depth filter (the player passes the entity
/// and a10 = 1, 0xB10990, so 0xE16717 / 0xE16749 run: detectors 2 cm out and up, −60° × 0.3 and 135° × 0.05, the same
/// thresholds; without them every end's scratch would be 0.5, `GuidanceReport__SetScratch` 0xE1677C); three
/// chain sets (`sub_116CD50`): walls facing the jumper (within 45° of −forward), and the two sides (walls turned up to
/// 45° from right / left, edges along the forward axis). On each chain the grab point toward the facing from the feet
/// (`GuidanceChain__FindGrabPoint` with the forward as the preferred direction, hand offsets 0.5 / 0.5, margin 0.25: the
/// depth filter's scratch is what the offsets may reach past a chain's ends), not Quadruped, the wall within 120° of the
/// facing; then its type and path rays (`ground_extras::static_target_type`), and 0xE97720 picks one
/// (`choose_static_report`).
/// PORT: the owner entity's validity (0xE165D0's +100 / +102 checks) is not modelled.
pub fn static_hand_target(feet: Vec3, forward: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld, beam: bool) -> Option<JumpTarget> {
    use crate::guidance::{ChainParams, GuidanceSubType as S, MASK_CATCH};
    let fwd = forward.with_y(0.0).normalize_or_zero();
    let right = fwd.cross(Vec3::Y);
    let half = Vec3::new(0.5, 1.0, 1.35);
    let report = guidance.report_indexed(feet + fwd * 0.5 + Vec3::Y * 1.85, fwd, -half, half, true);
    // the query's mask and GuidanceReport__RemoveSteepEdges 0x66C350
    let keep: Vec<usize> = (0..report.world.edges.len())
        .filter(|&i| {
            let e = &report.world.edges[i];
            (1u32 << e.subtype as u32) & MASK_CATCH != 0 && !crate::guidance::steep(e.p1 - e.p0)
        })
        .collect();
    let kept = crate::guidance::Report {
        world: GuidanceWorld { edges: keep.iter().map(|&i| report.world.edges[i].clone()).collect(), ..Default::default() },
        source: keep.clone(),
        scratch: vec![[0.0; 4]; keep.len()],
    };
    use crate::guidance::{DepthDetector, DEPTH_OFFSET_EDGE};
    let detectors = [DepthDetector::new(DEPTH_OFFSET_EDGE, -1.047_197_6, 0.3), DepthDetector::new(DEPTH_OFFSET_EDGE, 2.356_194_5, 0.05)];
    let filtered = kept.depth_filter(collision, &detectors, &[0.3, 0.05]);
    let (world, cuts) = (filtered.world, filtered.scratch);
    let q = std::f32::consts::FRAC_PI_4;
    let sets = [
        ChainParams { dir: -fwd, right, ranges: [(-q, 0.0), (0.0, q)], parallel: q, mask: u32::MAX },
        ChainParams { dir: right, right: fwd, ranges: [(0.0, 0.0), (0.0, q)], parallel: q, mask: u32::MAX },
        ChainParams { dir: -right, right: fwd, ranges: [(-q, 0.0), (0.0, 0.0)], parallel: q, mask: u32::MAX },
    ];
    let mut candidates = Vec::new();
    for p in &sets {
        for chain in world.chains(p) {
            // a4 = a5 = forward (0xE16E37 / 0xE16E38)
            let Some(hit) = world.grab_point_toward(&chain, feet, Some(fwd), Some(fwd), &[0.5, 0.5], 0.25, &cuts) else { continue };
            if world.edges[hit.edge].subtype == S::Quadruped || hit.wall_normal == Vec3::ZERO {
                continue;
            }
            if fwd.angle_between(-hit.wall_normal) >= 120f32.to_radians() {
                continue;
            }
            let point = hit.point;
            if !(GRAB_MIN_HEIGHT..=STRAIGHT_JUMP_MAX).contains(&(point.y - feet.y)) {
                continue;
            }
            if super::ground_extras::static_target_type(feet, fwd, point, hit.wall_normal, collision).is_none() {
                continue;
            }
            if let Some(target) = straight_target(feet, point, hit.wall_normal, collision, beam) {
                candidates.push((point, hit.wall_normal, target));
            }
        }
    }
    let reports: Vec<_> = candidates.iter().map(|c| (c.0, c.1)).collect();
    super::ground_extras::choose_static_report(feet, fwd, &reports).map(|i| candidates[i].2)
}

/// Pull-down type Wait (1) from Movement (0xDB1470 event 70 → fill 0xD843E0 → PullDown_Enter 0xDDE4D0): a
/// LedgeGrab edge at the feet within 0.8 m ahead whose outward normal points along the facing (dot > 0),
/// with more than 2.0 m of drop below it (edge report +48).
fn try_pulldown(feet: Vec3, forward: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<super::ledge::LedgeEntry> {
    let (p, n) = edge_report(feet, forward, 0.5, 0.0, guidance, collision)?;
    pulldown_entry(p, n, feet, true, guidance, collision)
}

/// Grab / climb probe radius around the character (0xEE65A0: IHuman vt136 radius 0.75).
const GRAB_PROBE_RADIUS: f32 = 0.75;
/// Event 68 guard 0xD84190: the edge must be at least 0.53 m above the feet.
const GRAB_MIN_HEIGHT: f32 = 0.53;

/// The ledge stop's squared edge distance: the interpreter passes 0.0225 (0.15 m) to `ClassifyEdgeSide` (vt744,
/// 0xEE8899), which compares it with the signed squared distance to the report point (0xD9D7F0).
const LEDGE_STOP_SQ: f32 = 0.0225;
/// The interpreter's report picks (`Human__PickEdgeReport` 0xB17EF0, `dword_17085E0`): within 0.75 m.
const REPORT_PICK_RADIUS: f32 = 0.75;
/// Event 69 is only sent for a drop of more than 5 m (0xEE88A9).
const LEDGE_STOP_DROP: f32 = 5.0;
/// The interpreter's low-profile edge halt: the report's +52 below 0.16 (a squared distance: within 0.4 m; live RE/25
/// the halt fired with the feet 0.393 m from the edge), normal within 70 deg (0xEE7C1A / 0xEE7C5C).
const EDGE_HALT_SQ: f32 = 0.16;
/// The lean's push time before it climbs, [low, high profile] (0xEE7672: `dword_1683384` 0.5 / `dword_1708648` 0.1).
const LEAN_CLIMB_TIME: [f32; 2] = [0.5, 0.1];
/// The event-42 sender's obstacle query reach (`dword_16834C4`, 0xEE8CEA).
const OBSTACLE_REACH: f32 = 0.6;
const EDGE_HALT_COS: f32 = 0.342_020_1;
/// PORT: the feet stay this far behind the edge line during the ledge stop (live: 0.12–0.14 m, RE/25 #39).
const LEDGE_STOP_MARGIN: f32 = 0.13;
/// ClassifyEdgeSide 0xD9D7F0: front = within 60° (120° with the report flag +68).
const FRONT_COS: f32 = 0.5;

/// An edge report for a front edge (`HumanGround__ClassifyEdgeSide` 0xD9D7F0 → 1): a LedgeGrab edge within
/// `reach` ahead whose outward normal is within 60° of the facing, with more than 2.0 m of drop
/// (guards 0xDA5DE0 / 0xD9D6C0; the classifier itself needs > 1.3 m). Returns (point, outward normal).
fn edge_report(feet: Vec3, forward: Vec3, reach: f32, min_cos: f32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3)> {
    edge_report_drop(feet, forward, reach, reach * reach, min_cos, 2.0, guidance, collision).map(|r| (r.0, r.1))
}

/// `edge_report` with the drop limit and the drop. The interpreter's report: the foot-level edge reports (IHuman vt132
/// `Human__BuildEdgeReports` 0xB1BC50: 0.75 m, drops of 0.45 m or more), the one within `reach` whose normal is closest
/// to the facing (IHuman vt136 0xB17EF0, within acos(`min_cos`)); then `HumanGround__ClassifyEdgeSide` 0xD9D7F0: a drop
/// over 1.3 m and the signed distance within `reach` (front), and the caller's drop limit. Returns (point, outward
/// normal, drop).
fn edge_report_drop(feet: Vec3, forward: Vec3, pick_radius: f32, max_sq: f32, min_cos: f32, min_drop: f32, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3, f32)> {
    use super::jump_candidates::{edge_reports, pick_report};
    // (the floor report, which `ClassifyEdgeSide` refuses, only stands when no edge report does: none needed here)
    let reports = edge_reports(feet, 0.75, 0.45, None, guidance, collision);
    let r = pick_report(&reports, feet, forward, pick_radius + 1e-3, min_cos.clamp(-1.0, 1.0).acos())?;
    (r.drop > 1.3 && r.drop > min_drop && r.sq() <= max_sq + 1e-4).then_some((r.point, r.normal, r.drop))
}

/// The step off an edge (event 68's action 0xD9D1F0, the parameter = 1.0 in high profile): facing more than 120 degrees
/// from the edge normal plays `xx_fall_ground_back_tr_fall` 0x935CE89E (InAirData+520 = 0.4 s); otherwise a free jump
/// at pos + (1 + 2·a·HG+1512)·dir − 3 m (HG+0x5E8 = the speed parameter), the direction the facing turned to within
/// 60 degrees of the normal (`sub_B27180` kind 0). Live (RE/18 §9): every walk-off and high-profile run-off fires it;
/// 1.80 m takes 0.44–0.49 s and 3.16 m 0.58–0.68 s, rising only 0.01–0.05 m first.
fn step_off_entry(feet: Vec3, facing: Vec3, r: &super::jump_candidates::EdgeReport, high: bool, speed_param: f32, foot_left: bool) -> Option<InAirEntry> {
    let angle = signed_angle_y(r.normal, facing);
    if angle.abs() >= 120f32.to_radians() {
        let action = jump_blend::action_items(STEP_OFF_BACK).map(|_| ActionBlend::new(STEP_OFF_BACK, 0, &[1.0]))?;
        return Some(InAirEntry::OnPlace { from: feet, fwd: facing, action, fall: None });
    }
    let dir = if angle.abs() <= 60f32.to_radians() { facing } else { Quat::from_rotation_y(60f32.to_radians() * angle.signum()) * r.normal };
    let a = if high { 1.0 } else { 0.0 };
    Some(InAirEntry::FreeJump { from: feet, dir, ahead: 1.0 + 2.0 * a * speed_param, speed_param, foot_left, tap: false })
}

/// `AC_JUMP_LOG=1`: which of the ground's jump paths fired, from where, at what target.
/// The takeoff foot of a ground target jump (`HumanGround__StartRunJumpToTarget` 0xD837A0): in the Movement sub-state
/// (HG data +176 = 0) it hands `Human__SetupJumpToTarget` foot 0, which takes `Human__GetLeadingFoot` 0xB18850 on the
/// pose (0xB20435: the left foot ahead → the `footl` takeoff); in the others the playing item's word bits 2–3. PORT:
/// the item word stands as the foot opposite the locomotion's (live RE/25), also without a pose.
fn run_jump_foot_left(sub_state: HumanGroundSubState, leading_foot: Option<usize>, blend_foot: usize) -> bool {
    match (sub_state, leading_foot) {
        (HumanGroundSubState::Movement, Some(lead)) => lead == 0,
        _ => blend_foot != 0,
    }
}

fn jump_log(what: &str, feet: Vec3, target: Option<&JumpTarget>) {
    if std::env::var_os("AC_JUMP_LOG").is_some() {
        eprintln!("jump: {what} from {feet:.2} -> {:?}", target.map(|t| (t.type_flags, t.position)));
    }
}

/// The signed angle about +Y from `from` to `to` (both flat).
fn signed_angle_y(from: Vec3, to: Vec3) -> f32 {
    let c = from.cross(to).y;
    c.atan2(from.dot(to))
}

/// `xx_fall_ground_back_tr_fall` (0xD9D1F0, backing off an edge).
const STEP_OFF_BACK: u32 = 0x935C_E89E;

/// The look-down's latch time (interpreter 0xEE8803, flt_1682378).
const LOOK_DOWN_HOLD: f32 = 0.25;
/// The pull-down's empty-hand window (interpreter 0xEE8A33, dword_170861C).
const PULLDOWN_HAND_WINDOW: f32 = 0.2;

/// Pull-down from the ground at edge `p` / normal `n`: type Wait (from Movement) or EdgeStop (from the ledge stop).
fn pulldown_entry(p: Vec3, n: Vec3, feet: Vec3, wait: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<super::ledge::LedgeEntry> {
    Some(match super::ledge_moves::pulldown(p, n, feet, wait, guidance, collision) {
        Ok(moves) => pulldown_ledge_entry(moves, n, feet),
        Err(orient) => pulldown_release_entry(orient, n, feet),
    })
}

/// The pull-down that finds no hands at the descent (PullDownSubState 4, ReleaseToInAir): the orientation alone.
pub(crate) fn pulldown_release_entry(orient: super::ledge_moves::LedgeMove, n: Vec3, feet: Vec3) -> super::ledge::LedgeEntry {
    let mut e = super::ledge::LedgeEntry::at((orient.hand_l + orient.hand_r) * 0.5, n, feet, super::ledge::LedgeSubState::PullDown);
    e.hand_l = orient.hand_l;
    e.hand_r = orient.hand_r;
    e.entry_move = Some(orient);
    e
}

/// The Ledge entry (SubState 11 PullDown, `PullDown_Enter` 0xDDE4D0) that plays the pull-down's three stages.
pub(super) fn pulldown_ledge_entry([orient, descent, reception]: [super::ledge_moves::LedgeMove; 3], n: Vec3, feet: Vec3) -> super::ledge::LedgeEntry {
    let mut e = super::ledge::LedgeEntry::at((orient.hand_l + orient.hand_r) * 0.5, n, feet, super::ledge::LedgeSubState::PullDown);
    e.hand_l = orient.hand_l;
    e.hand_r = orient.hand_r;
    e.entry_move = Some(orient);
    e.entry_rest = [Some(descent), Some(reception)];
    e
}

trait AnyHit {
    fn any_hit(self, f: impl Fn(f32) -> bool) -> bool;
}
impl AnyHit for std::ops::RangeInclusive<f32> {
    fn any_hit(self, f: impl Fn(f32) -> bool) -> bool {
        let (a, b) = (*self.start(), *self.end());
        (0..=5).any(|i| f(a + (b - a) * i as f32 / 5.0))
    }
}

#[cfg(test)]
mod reanalysis_tests {
    use super::*;

    /// Event 45's guards (0xDB19BE): from the locomotion blend the run jump; from a start item early in its play the
    /// impulsion, entered at phase t / 0.75 (HG+616), with the target kept for the free-step takeoff; late in it the run
    /// jump; FreeRun's impulsion from its start, blended toward the target's side.
    #[test]
    fn event_45_picks_the_impulsion_or_the_run_jump_by_the_playing_item() {
        let target = JumpTarget { position: Vec3::new(0.0, 0.0, -3.0), type_flags: 1, hang: None, straight: None, pass: None, ladder: None, support: None };
        let start = START_MOVE.iter().flatten().copied()
            .filter_map(|id| jump_blend::action_items(id).and_then(|i| i.first().map(|c| ActionBlend::new(id, 0, &vec![1.0 / c.len() as f32; c.len()]))))
            .find(|b| !super::super::anim_gate::locked(b))
            .expect("an unlocked start item");
        let mut g = HumanGroundData::default();
        assert_eq!(g.event45(Vec3::ZERO, Vec3::NEG_Z, ImpulsionJump::Target(target), None, None), Event45::Run, "the locomotion");
        for (t, want) in [(0.3f32, Event45::Impulsion), (0.8, Event45::Run)] {
            let mut g = HumanGroundData::default();
            g.play_oneshot(start);
            if let Some(os) = g.oneshot.as_mut() {
                os.t = t * os.duration;
            }
            assert_eq!(g.event45(Vec3::ZERO, Vec3::NEG_Z, ImpulsionJump::Target(target), None, None), want, "t {t}");
            if want == Event45::Impulsion {
                let os = g.oneshot.unwrap();
                assert!(jump_blend::IMPULSION_JUMP.contains(&os.blend.id));
                assert!((os.t / os.duration - 0.4).abs() < 1e-3, "phase {}", os.t / os.duration);
                assert!(g.impulsion_jump.is_some());
            }
        }
        // FreeRun: from the start, the target 45 degrees to the right blends front and right half and half
        let mut g = HumanGroundData::default();
        let right = JumpTarget { position: Vec3::new(2.0, 0.0, -2.0), ..target };
        assert_eq!(g.event45(Vec3::ZERO, Vec3::NEG_Z, ImpulsionJump::Target(right), Some(ActionBlend::new(super::super::ground_extras::FREERUN_WAIT[0], 0, &[1.0])), None), Event45::Impulsion);
        let os = g.oneshot.unwrap();
        let w = os.blend.weights();
        assert!(os.t == 0.0 && (w[0] - 0.5).abs() < 1e-3 && (w[4] - 0.5).abs() < 1e-3, "{w:?}");
    }

    /// RE/25 #67: the player's cycle breakers are off (HumanData +0x201 = 0): no timer, no fidget.
    #[test]
    fn the_player_never_fidgets() {
        let mut f = Fidget::default();
        for i in 0..400 {
            f.update(true, true, i % 2, 0.2, true, 0.05);
        }
        assert_eq!((f.timer, f.seq), (0.0, 0));
    }

    /// RE/24 GROUND-18: the fidget waits for the timer armed at the Move entry (6–15 s), then starts when the walk's
    /// locomotion item turns to item 1; not when running, not while the upper body is locked; a run ends it.
    #[test]
    fn the_walk_fidgets_when_the_timer_runs_out_on_the_right_step() {
        let dt = 0.1;
        // allowed (HumanData +0x201): an NPC's setting; the player's is off (RE/25 #67)
        let mut f = Fidget { allowed: true, ..Default::default() };
        f.update(true, true, 0, 0.2, true, dt);
        assert!((6.0..=15.0).contains(&f.timer), "armed at the Move entry: {}", f.timer);
        f.timer = 0.0;
        f.update(true, true, 0, 0.2, true, dt);
        assert_eq!(f.seq, 0, "item 0: no fidget");
        f.update(true, true, 1, 0.2, true, dt);
        assert_eq!(f.seq, 1, "the step onto item 1 starts it");
        assert!((6.0..=15.0).contains(&f.timer), "re-armed: {}", f.timer);
        // jogging, or the upper body locked: no fidget
        for (s, enabled) in [(0.5, true), (0.2, false)] {
            let mut g = Fidget { timer: 0.0, prev_item: 0, moving: true, ..Default::default() };
            g.update(true, true, 1, s, enabled, dt);
            assert_eq!(g.seq, 0, "s {s} enabled {enabled}");
        }
        // a run or a lock ends a fidget
        f.update(true, true, 0, 0.8, true, dt);
        assert!(f.stop);
        f.update(true, true, 0, 0.2, false, dt);
        assert!(f.stop);
        f.update(true, true, 0, 0.2, true, dt);
        assert!(!f.stop);
    }

    /// RE/24 GROUND-16: each switching state turns the foot IK off over 0.2 s on entry and on in its exit: the landings
    /// over 0.35 s, the roll ending and the lean at once, FreeRun over 0.2 s; the others leave it alone.
    #[test]
    fn each_ground_state_switches_the_foot_ik_with_its_own_fade() {
        let dt = 1.0 / 60.0;
        for (state, back) in [(IkState::Landing, 0.35), (IkState::RollEnding, 0.0), (IkState::Lean, 0.0), (IkState::FreeRun, 0.2)] {
            let mut sw = crate::ik::GroundIkSwitch::default();
            IkState::On.switch(state, &mut sw, dt);
            assert_eq!((sw.state, sw.smooth, sw.fade()), (3, false, 0.2), "{state:?} entry");
            state.switch(IkState::On, &mut sw, dt);
            assert_eq!((sw.state, sw.smooth, sw.fade()), (0, true, back), "{state:?} exit");
        }
        let mut sw = crate::ik::GroundIkSwitch::default();
        IkState::On.switch(IkState::On, &mut sw, dt);
        assert_eq!(sw.state, 2);
        IkState::Landing.switch(IkState::Away, &mut sw, dt);
        assert_eq!((sw.state, sw.smooth), (3, false), "the landing's exit finds it on; leaving the ground turns it off");
    }

    /// RE/24 GUIDE-15: a step up of more than 0.25 m holds while the character stays on the contact's side, within 5 cm
    /// of its closest approach.
    #[test]
    fn a_step_up_holds_until_the_character_moves_away_from_it() {
        let contact = Some((Vec3::new(0.0, 0.3, 0.0), Vec3::new(0.0, 0.6, 0.8).normalize()));
        assert!(StepUp::capture(-0.2, contact, Vec3::new(0.0, 0.3, 0.3)).is_none(), "0.2 m is no step-up");
        let s = StepUp::capture(-0.3, contact, Vec3::new(0.0, 0.3, 0.3)).expect("0.3 m");
        assert!((s.dist - 0.3).abs() < 1e-6);
        // closer: still holding, the closest distance shrinks
        let s = s.hold(Vec3::new(0.0, 0.3, 0.2)).expect("closer");
        assert!((s.dist - 0.2).abs() < 1e-6);
        // back out by 4 cm: holds; by 6 cm: ends
        assert!(s.hold(Vec3::new(0.0, 0.3, 0.24)).is_some());
        assert!(s.hold(Vec3::new(0.0, 0.3, 0.26)).is_none());
        // across the contact against its normal: ends
        assert!(s.hold(Vec3::new(0.0, 0.3, -0.01)).is_none());
    }

    #[test]
    fn a_completed_landing_enters_move_without_the_stick() {
        // Guard_LandingToMove 0xD7F0F0 has no stick test (RE/24 GROUND-6); the landings that end in the wait keep theirs
        let mut g = HumanGroundData { owner: TreeState::Landing, high_profile: true, ..Default::default() };
        let mut h = 0.0;
        g.after_oneshot(ActionBlend::new(jump_blend::LAND_FORWARD_MOVE[0], 0, &[1.0]), false, 0.0, &mut h);
        assert!(g.speed_param >= 0.5, "entered Move at the profile's speed: {}", g.speed_param);
        let mut g = HumanGroundData { owner: TreeState::Landing, high_profile: true, ..Default::default() };
        g.after_oneshot(ActionBlend::new(jump_blend::LAND_FORWARD_STOP[0], 0, &[1.0]), false, 0.0, &mut h);
        assert_eq!(g.speed_param, 0.0, "a stop landing goes to the wait");
    }

    #[test]
    fn the_pass_over_rolls_onto_the_ground() {
        // entry mode 12 → state 20 (RE/24 GROUND-4): the roll ending by the vault's word, then Move through 0x082FB27C
        use super::super::ground_tree::{ROLL_ENDING, ROLL_ENDING_EXIT};
        if jump_blend::action_items(ROLL_ENDING[0]).is_none() {
            return;
        }
        let mut g = HumanGroundData::default();
        g.enter_roll_ending(Some(0x0004));
        assert_eq!(g.owner, TreeState::RollEnding);
        assert_eq!(g.oneshot.map(|o| o.blend.id), Some(ROLL_ENDING[1]), "word bits 2-3 = 1 → Data+556 = 2 → foot 1");
        assert_eq!(g.speed_param, 0.5);
        let ended = g.oneshot.unwrap().blend;
        let mut h = 0.0;
        g.after_oneshot(ended, true, 0.0, &mut h);
        assert_eq!(g.tr.map(|t| t.action.id), Some(ROLL_ENDING_EXIT));
        let mut g = HumanGroundData::default();
        g.enter_roll_ending(Some(0x0008));
        assert_eq!(g.oneshot.map(|o| o.blend.id), Some(ROLL_ENDING[0]));
        let ended = g.oneshot.unwrap().blend;
        g.after_oneshot(ended, false, 0.0, &mut h);
        assert_eq!(g.speed_param, 0.0, "no stick: the wait");
    }

    #[test]
    fn a_move_entry_resets_the_blend_timers_and_the_lean() {
        // HumanGround__Move_EnterBody 0xD94A60 (RE/24 GROUND-13)
        let mut g = HumanGroundData { high_profile: true, ..Default::default() };
        g.blend.slowdown = 0.7;
        g.blend.settle = 0.4;
        g.blend.lean = 0.3;
        g.play_start_move();
        assert_eq!((g.blend.slowdown, g.blend.settle, g.blend.lean), (0.0, 0.0, 0.0));
    }

    #[test]
    fn the_free_run_start_rides_the_sprint_impulsion() {
        // `HumanGround__FreeRunStartMove` 0xD99A50: the start item of the wait's profile (HG+1500, low here), all its
        // weight on the sprint impulsion clip, s = 1.0 and layout 7 (live: freerun courses, 1.10 → 5.84 m/s)
        if jump_blend::action_items(START_MOVE[0][1]).is_none() {
            return;
        }
        let mut g = HumanGroundData { high_profile: true, free_run_high: false, ..Default::default() };
        g.blend.foot = 1;
        g.blend.slowdown = 0.7;
        g.play_free_run_start();
        let t = g.tr.expect("the start transition");
        assert_eq!(t.action.id, START_MOVE[0][1]);
        assert_eq!(t.action.weights(), &[0.0, 0.0, 0.0, 1.0]);
        assert_eq!(t.layout, 7);
        assert_eq!((g.speed_param, g.blend.speed_param, g.blend.slowdown), (1.0, 1.0, 0.0));
    }
}
