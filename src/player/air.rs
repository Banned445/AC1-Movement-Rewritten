//! In-air context (`HumanInAir`, ActorContextID 8) — RE/04.
//!
//! Jumps are NOT ballistic in AC1: a takeoff item and a flight item play back to back (chosen and
//! weighted by Human__ComputeJumpAnimBlend 0xB1EC40, `jump_blend`), and their blended root motion gets a
//! linear correction `(target − animEnd) · t/(T₁+T₂)`, so the jump ends exactly on the chosen target
//! (Human__SetupJumpToTarget 0xB20200, HumanInAir__UpdateJumpMotion 0xE0DEF0). Arrival is checked within
//! 0.01 m (0xE07D00).
//! Real physics only for: the over-drop tail (target > 5 m below → aim 5 m down, then free fall with
//! g = 9.8, horizontal steering ≤ 15 m/s) and plain falls (drift decays 4 m/s², ≤ 5 m/s).
//! Landing: fall height measured from the apex; player fixed damage table (0xED6280 / 0xE00FE0);
//! landing action by drop / distance / speed bucket, total drop > 3 m → damage or damage-roll + camera
//! shake (0xE05940, `jump_blend::landing`).

use bevy::prelude::*;

use super::jump_blend::{self, ActionBlend, TARGET_FREESTEP};
use super::ledge::{LedgeEntry, LedgeSubState};
use super::ledge_moves::{self, LedgeArrival};
use super::targets::JumpTarget;
use super::{switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceHit, GuidanceWorld};
use crate::input::PadInput;
use crate::tuning::*;

/// `LandingEvent::LandingType` (values from the exe, desc 0x19919D0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandingType {
    Safe = 0,
    SmallDamage = 1,
    HeavyDamage = 2,
    Fatal = 3,
}

#[derive(Clone, Copy, Debug)]
pub struct Landing {
    pub kind: LandingType,
    /// LandingEvent+16 (0xE04E40). PORT: native health/protection component is not yet integrated.
    pub damage: u32,
    /// Fall height from the apex (m).
    pub fall_height: f32,
    /// Total drop from the jump/fall start (m).
    pub total_drop: f32,
    pub roll: bool,
    /// The action the Ground context plays on entry (landing 0xE05940 or free-step reception 0xE07D00);
    /// its root motion moves the character until it ends.
    pub action: Option<ActionBlend>,
}

/// Drop sub-state (4) animations (`HumanInAir__EnterDropState` 0xE064C0, tables 0x1A2EAE0 / 0x1A2EB30 filled by
/// 0xDFF680; HumanGround_Hurt block): the entry item by [drop type][side 0 front, 1 back], then the falling loop
/// `hurt_fall_{front,back}`. Ground loss sends types 0..6 (`HumanGround__TransitionToInAirFall` 0xD8C380).
pub const DROP_ENTRY: [[u32; 2]; 9] = [
    [0xC5DD_34F3, 0xBE67_B24A], // 0 stumble_air medium
    [0xC5DD_34F6, 0xBE67_B24D], // 1 stumble_air strong
    [0xC5DD_34F7, 0xBE67_B23E], // 2 hurt_fall_balanced short
    [0xC5DD_34FA, 0xBE67_B241], // 3 hurt_fall_balanced long
    [0x7E5D_E556, 0x7E5D_E557], // 4 hurt_fall_medium
    [0, 0],                     // 5 (none)
    [0x7F4A_71E4, 0x7F4A_71E5], // 6 hurt_fall_dive
    [0x8111_514B, 0x8111_514B], // 7 hurt_fall_stumble_dive
    [0x8111_7142, 0x8111_7141], // 8 hurt_fall_railing
];
pub const DROP_LOOP: [u32; 2] = [0x8111_514E, 0x8111_514F];

/// Drop phase length by type (0xE064C0): < 2 → 0.3 s, 2–3 → 0.6 s, 4–5 → 1.0 s, 6–7 → the animation's length,
/// 8 → 0.1 s; `None` = the animation's length.
pub fn drop_phase(ty: usize) -> Option<f32> {
    match ty {
        0 | 1 => Some(0.3),
        2 | 3 => Some(0.6),
        4 | 5 => Some(1.0),
        8 => Some(0.1),
        _ => None,
    }
}

/// `HumanGround__TransitionToInAirFall` 0xD8C380: fall type by the drop report's height `h` and the horizontal speed
/// `v`: h < 1 m (or < 8 m on a type-1 probe, PORT: not modelled) → 0 / 1 (2 / 3 when h ≥ 2 m), else 4 / 6; the second
/// of each pair at v ≥ 2.5 m/s.
pub fn fall_type(h: f32, v: f32) -> usize {
    let fast = v >= 2.5;
    if h < 1.0 {
        if h >= 2.0 { 2 + fast as usize } else { fast as usize }
    } else if fast {
        6
    } else {
        4
    }
}

/// `HumanInAirData::FallOrigin` (desc 0x199587C).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FallOrigin {
    #[default]
    Ground = 0,
    Climb = 1,
    HangWall = 2,
    HangFree = 3,
}

/// What the switching context asks InAir to do (TransitionSetupDataToInAir, RE/01 §4.2).
#[derive(Clone, Copy, Debug)]
pub enum InAirEntry {
    /// PORT: `speed_param` seeds the entry; 0xEDBAF0 updates the live air ratio on subsequent updates;
    /// `foot_left` = the leading foot (byte+60 bits 2–3 of the playing item, or sub_B18850).
    JumpToTarget { from: Vec3, target: JumpTarget, speed_param: f32, foot_left: bool },
    /// No target found: the free-jump target (`Human__MakeFreeJumpTarget` 0xB13630 → 0xB1E7F0, `FREE_JUMP_AHEAD` along
    /// `dir`, `FREE_JUMP_DOWN` below), jumped at like any target (type 1) and ended by the ground contact.
    FreeJump { from: Vec3, dir: Vec3, speed_param: f32, foot_left: bool },
    /// A free-step jump (jump kind 1: the `freestep_*_to_air` takeoff) from a beam or a pilotis to a target
    /// (NarrowObject event 4 → `Human__SetupJumpToTarget` 0xB20200 with kind 1, 0xE4D950).
    FreeStepJump { from: Vec3, target: JumpTarget, foot_left: bool },
    /// A jump without a target that plays one action's root motion, then falls (the beam's jump on the spot,
    /// 0xF73B80: `beam_jumpstraight_clear`, then InAir +416 = `…_clear_tr_fall`).
    OnPlace { from: Vec3, fwd: Vec3, action: ActionBlend, fall: Option<ActionBlend> },
    /// The swing jump from a bar (Ledge SwingReception event 7 -> `sub_DCD0B0`: `Human__SetupJumpToTarget` with jump
    /// kind 3, the `swing_cycle_*_to_air` takeoff).
    SwingJump { from: Vec3, target: JumpTarget },
    Fall { from: Vec3, velocity: Vec3, origin: FallOrigin, speed_param: f32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AirMode {
    #[default]
    Idle,
    /// Target-driven jump with linear correction.
    Jump {
        from: Vec3,
        clip_end: Vec3,
        aim: Vec3,
        /// PLACEHOLDER arc height (ledge jumps only, see `real`).
        apex: f32,
        duration: f32,
        t: f32,
        /// After the clip, continue as a free fall toward this point (over-drop or free jump).
        then_fall_to: Option<Vec3>,
        /// The game's takeoff + flight items (`HumanInAirData::takeoff/flight`) drive the root. False for
        /// jumps at a ledge (their flight comes from the ledge code, not 0xB20200): PLACEHOLDER arc.
        real: bool,
        /// Takeoff item duration T₁ (HumanInAirData+0x1E4).
        t_takeoff: f32,
        /// Character forward at takeoff (the jump origin matrix +0x10).
        fwd: Vec3,
    },
    /// Ballistic fall (optionally steered toward `steer_to`).
    Fall { steer_to: Option<Vec3> },
}

/// Runtime data of the InAir context (subset of reflected HumanInAirData, RE/07).
#[derive(Debug, Default)]
pub struct HumanInAirData {
    /// Incremented on every entry (lets the animator restart the jump clip).
    pub seq: u32,
    /// The jump target (+0x230), kept so arrival can hand off to the right context.
    pub target: Option<JumpTarget>,
    /// Seconds since entering InAir (used to avoid re-catching the ledge just released).
    pub time_in_air: f32,
    /// Last catch (fall height ≥ 3 m selects the long catch animations in the game, 0xE0BB70).
    pub long_catch: bool,
    pub mode: AirMode,
    /// JumpApexPosition (+0xD0): where descent started — the fall-height reference.
    pub apex_y: f32,
    pub apex_reached: bool,
    /// Height at the jump/fall start (total-drop reference).
    pub start_y: f32,
    pub fall_origin: FallOrigin,
    /// Jump target type flags (+0x290).
    pub target_flags: u32,
    pub prev_y: f32,
    /// PORT adapter: entry velocity is applied by the body-owning system before the skipped update.
    pub entry_velocity: Option<Vec3>,
    /// HumanInAir+0x16C: live air-control speed ratio (0xEDBAF0 -> 0xE102B0).
    pub speed_ratio: f32,
    /// Takeoff and flight items of the current jump (+0x1A4 / +0x1A8 and the weight arrays +0x1BC..).
    pub takeoff: Option<ActionBlend>,
    pub flight: Option<ActionBlend>,
    pub foot_left: bool,
    /// Jump / fall start (JumpOrigin +0x10 translation).
    pub start: Vec3,
    /// The action played once the jump turns into a fall (HumanInAirData+416), and the time since.
    pub fall_action: Option<ActionBlend>,
    pub fall_t: f32,
    /// The jump is at the free-jump target (no target found): the ground contact ends it, as it does in the game
    /// (0xE05200 on the controller contact) long before the target 3 m down is reached.
    pub free_target: bool,
    /// Ground loss: the drop sub-state's (type, side) (InAirData+944 +64 / +68) and the edge's outward normal.
    pub drop: Option<(usize, usize, Option<Vec3>)>,
    /// First steep contact and rag-fall requirement (0xE038A0 / 0xE05200).
    pub slope_slide: super::ground_extras::SlopeSlide,
}

impl HumanInAirData {
    pub fn enter(&mut self, entry: InAirEntry) {
        self.seq = self.seq.wrapping_add(1);
        self.apex_reached = false;
        self.fall_origin = FallOrigin::Ground;
        self.target_flags = 0;
        self.target = None;
        self.time_in_air = 0.0;
        self.fall_action = None;
        self.fall_t = 0.0;
        self.free_target = false;
        self.drop = None;
        self.slope_slide = default();
        self.entry_velocity = None;
        match entry {
            InAirEntry::JumpToTarget { from, target, speed_param, foot_left } => {
                self.speed_ratio = speed_param;
                self.foot_left = foot_left;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.target_flags = target.type_flags;
                self.target = Some(target);
                // over-drop rule (0xB1B8C0): target > 5 m below → aim 5 m down, then free-fall.
                let mut aim = target.position;
                let mut then_fall_to = None;
                // haystack: −3 m, or −30 m when deeper (Leap of Faith)
                let thr = if target.type_flags == jump_blend::TARGET_HAYSTACK {
                    if target.position.y - from.y <= -jump_blend::FAITH_MIN_DROP { jump_blend::FAITH_DOWN } else { jump_blend::FAITH_MIN_DROP }
                } else {
                    OVERDROP
                };
                if target.position.y < from.y - thr {
                    aim.y = from.y - thr;
                    then_fall_to = Some(target.position);
                }
                self.mode = match (target.straight, target.hang) {
                    (Some(j), Some((_, n))) => self.straight_jump(from, aim, j, n),
                    _ => self.real_jump(from, aim, target.type_flags, then_fall_to, 0),
                };
            }
            InAirEntry::FreeStepJump { from, target, foot_left } => {
                self.speed_ratio = 0.0;
                self.foot_left = foot_left;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.target_flags = target.type_flags;
                self.target = Some(target);
                let mut aim = target.position;
                let mut then_fall_to = None;
                if target.position.y < from.y - OVERDROP {
                    aim.y = from.y - OVERDROP;
                    then_fall_to = Some(target.position);
                }
                self.mode = self.real_jump(from, aim, target.type_flags, then_fall_to, 1);
            }
            InAirEntry::SwingJump { from, target } => {
                self.speed_ratio = 0.0;
                self.foot_left = true;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.target_flags = target.type_flags;
                self.target = Some(target);
                let mut aim = target.position;
                let mut then_fall_to = None;
                if target.position.y < from.y - OVERDROP {
                    aim.y = from.y - OVERDROP;
                    then_fall_to = Some(target.position);
                }
                self.mode = self.real_jump(from, aim, target.type_flags, then_fall_to, 3);
            }
            InAirEntry::OnPlace { from, fwd, action, fall } => {
                self.speed_ratio = 0.0;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.takeoff = None;
                self.flight = Some(action);
                self.fall_action = fall;
                let fwd = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::NEG_Z);
                let clip_end = from + to_world(action.disp(1.0), fwd);
                let d = action.duration().max(0.1);
                self.mode = AirMode::Jump { from, clip_end, aim: clip_end, apex: 0.0, duration: d, t: 0.0, then_fall_to: Some(clip_end), real: true, t_takeoff: 0.0, fwd };
            }
            InAirEntry::FreeJump { from, dir, speed_param, foot_left } => {
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.speed_ratio = speed_param;
                self.foot_left = foot_left;
                // the game's free-jump target (0xB1E7F0): 8 m ahead, 3 m down, type 1, jumped at through
                // `Human__SetupJumpToTarget` like any target; on open ground the contact ends the jump long before
                // it (live: 0.66 s, 4.8 m), off a roof it falls on from there
                let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
                let aim = from + flat * FREE_JUMP_AHEAD - Vec3::Y * FREE_JUMP_DOWN;
                self.mode = self.real_jump(from, aim, TARGET_FREESTEP, Some(aim), 0);
                self.free_target = true;
            }
            InAirEntry::Fall { from, velocity, origin, speed_param } => {
                self.entry_velocity = Some(velocity);
                self.takeoff = None;
                self.flight = None;
                self.speed_ratio = speed_param;
                self.start_y = from.y;
                self.start = from;
                self.apex_y = from.y;
                self.prev_y = from.y;
                self.fall_origin = origin;
                self.mode = AirMode::Fall { steer_to: None };
            }
        }
    }
}

impl HumanInAirData {
    /// Human__SetupJumpToTarget 0xB20200 for a ground running jump (kind 0): pick and weight the takeoff and
    /// flight items (0xB1EC40), T₁/T₂ = their Σw·T durations, anim end = their summed displacement.
    fn real_jump(&mut self, from: Vec3, aim: Vec3, target_type: u32, then_fall_to: Option<Vec3>, kind: u8) -> AirMode {
        let flat = Vec3::new(aim.x - from.x, 0.0, aim.z - from.z);
        let fwd = flat.normalize_or(Vec3::NEG_Z);
        let b = jump_blend::compute_kind(aim.y - from.y, flat.length(), target_type, self.foot_left, 1.0, kind);
        let (takeoff_id, flight_id) = jump_blend::down_variants(b.takeoff, b.flight, aim.y - from.y);
        let takeoff = ActionBlend::new(takeoff_id, 0, &b.takeoff_w);
        let flight = ActionBlend::new(flight_id, 0, &b.flight_w);
        let (t1, t2) = (takeoff.duration(), flight.duration());
        self.takeoff = Some(takeoff);
        self.flight = Some(flight);
        let end = add(takeoff.disp(1.0), flight.disp(1.0));
        let clip_end = from + to_world(end, fwd);
        if std::env::var_os("AC_ANIM_LOG").is_some() {
            info!("target jump: type {target_type:#x} kind {kind} dz {:.2} dist {:.2}: takeoff {takeoff_id:#010x} {t1:.2}s, flight {flight_id:#010x} {t2:.2}s, clip end {:.2} vs aim {:.2}",
                aim.y - from.y, flat.length(), (clip_end - from).length(), (aim - from).length());
        }
        AirMode::Jump { from, clip_end, aim, apex: 0.0, duration: (t1 + t2).max(1e-3), t: 0.0, then_fall_to, real: true, t_takeoff: t1, fwd }
    }
}

impl HumanInAirData {
    /// Human__SetupJumpToHandTarget 0xB21DA0: no takeoff item (+0x1E4 = 0); the band's 2-clip flight, its Σw·T
    /// as the duration (+0x1E8), JumpType 0, and the linear correction to the hang / knee root.
    fn straight_jump(&mut self, from: Vec3, aim: Vec3, j: super::ledge_moves::HangJumpIn, n: Vec3) -> AirMode {
        let fwd = -Vec3::new(n.x, 0.0, n.z).normalize_or(Vec3::NEG_Z);
        let flight = ActionBlend::new(j.flight, 0, &[1.0 - j.b, j.b]);
        self.takeoff = None;
        self.flight = Some(flight);
        let d = flight.duration();
        let clip_end = from + to_world(flight.disp(1.0), fwd);
        if std::env::var_os("AC_ANIM_LOG").is_some() {
            info!("straight jump: flight {:#010x} b {:.2} {d:.2}s, clip end {:.2} vs aim {:.2}", j.flight, j.b, (clip_end - from).length(), (aim - from).length());
        }
        AirMode::Jump { from, clip_end, aim, apex: 0.0, duration: d.max(0.2), t: 0.0, then_fall_to: None, real: true, t_takeoff: 0.0, fwd }
    }
}

/// PORT: how fast a target jump closes a lag behind its path (1/s), and the lag at which it gives up and falls.
const JUMP_LAG_RECOVERY: f32 = 6.0;
const JUMP_BLOCKED_LAG: f32 = 0.6;

/// Signed angle from `f` to `to` about up (+ = to the left, the heading's positive sense).
fn right_of_signed(f: Vec3, to: Vec3) -> f32 {
    let r = super::right_of(f);
    (-to.dot(r)).atan2(to.dot(f))
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Animation space (x right, y forward, z up) → world, for a character facing `fwd`.
fn to_world(d: [f32; 3], fwd: Vec3) -> Vec3 {
    super::right_of(fwd) * d[0] + fwd * d[1] + Vec3::Y * d[2]
}

/// Blended root displacement of the jump at time `t` (takeoff item, then the flight item from its end).
fn jump_disp(air: &HumanInAirData, t: f32, t1: f32, duration: f32) -> [f32; 3] {
    jump_disp_clips(air, t, t1, duration)
}

fn jump_disp_clips(air: &HumanInAirData, t: f32, t1: f32, duration: f32) -> [f32; 3] {
    let Some(fl) = air.flight else { return [0.0; 3] };
    let Some(to) = air.takeoff else { return fl.disp(t / duration.max(1e-4)) };
    if t < t1 {
        to.disp(t / t1.max(1e-4))
    } else {
        add(to.disp(1.0), fl.disp((t - t1) / (duration - t1).max(1e-4)))
    }
}

fn classify_landing(apex_y: f32, apex_reached: bool, start_y: f32, land_y: f32) -> Landing {
    let fall_height = if apex_reached { (apex_y - land_y).max(0.0) } else { 0.0 };
    let total_drop = (start_y - land_y).max(0.0);
    let kind = if fall_height > FALL_FATAL {
        LandingType::Fatal
    } else if fall_height > FALL_HEAVY {
        LandingType::HeavyDamage
    } else {
        LandingType::Safe
    };
    // PORT: the native CanSurviveDamage/protection overrides (0xE00FE0) require the missing
    // synchronization/damage component. This returns the verified base table, not final health outcomes.
    let (kind, damage) = if GAME_FALLS { super::falls::landing_damage(fall_height) } else { (kind, if kind == LandingType::Fatal { 200 } else if kind == LandingType::HeavyDamage { 10 } else { 0 }) };
    Landing { kind, damage, fall_height, total_drop, roll: total_drop > ROLL_DROP && kind != LandingType::Fatal, action: None }
}

/// CheckAirCatch 0xE0BB70 / FindLedgeCatch 0xE0A990: look for an edge in the hand box at the reach
/// height, facing the character within 70°.
/// PORT: approximate feet-height probes; native hand-bone pairs and origin/release clearance are not represented.
fn find_air_catch(feet: Vec3, facing: Vec3, guidance: &GuidanceWorld) -> Option<GuidanceHit> {
    for reach in [CATCH_REACH_WALL, CATCH_REACH_LEDGE] {
        let p = feet + Vec3::Y * reach + facing * 0.3;
        if let Some(h) = guidance.probe(p, CATCH_BOX_R, CATCH_BOX_V, Some(facing), CATCH_MAX_ANGLE) {
            return Some(h);
        }
    }
    None
}

/// The narrow-object part of `CheckAirCatch` 0xE0BB70 (fall height < 9 m, not while a target is still ahead):
/// first a pilotis under the feet (a support within 0.5 m, then 0xB2B600 from the air → soft landing,
/// PilotisReception), then a beam in the 0xE0B890 box (→ landing, BeamReception mode 7).
fn narrow_catch(feet: Vec3, facing: Vec3, foot: usize, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<TransitionSetup> {
    use super::narrow::*;
    if let Some(h) = collision.ground_height(feet + Vec3::Y * 0.01, 0.5) {
        let support = Vec3::new(feet.x, h, feet.z);
        if let Some(top) = find_pilotis(feet, support, facing, true, guidance, collision) {
            return Some(TransitionSetup::ToPilotis(PilotisEntry { top, from: feet, facing, kind: PilotisEntryType::FromInAir, foot }));
        }
    }
    let (p0, p1, point) = beam_at(feet, facing, guidance, collision)?;
    let axis = Vec3::new(p1.x - p0.x, 0.0, p1.z - p0.z).normalize_or_zero();
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::NEG_Z);
    Some(TransitionSetup::ToBeam(BeamEntry { p0, p1, point, from: feet, toward_p1: axis.dot(f) >= 0.0, mode: BeamEntryMode::Reception, foot, action: None, facing: f }))
}

pub fn update_air(
    time: Res<Time>,
    pad: Res<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data) in &mut q {
        if loco.current != ActorContextId::InAir {
            continue;
        }
        if loco.just_switched {
            if let Some(v) = data.air.entry_velocity.take() { body.velocity = v; }
            loco.just_switched = false;
            // the controller keeps integrating the take-off velocity; the jump's path shifts with it so the
            // linear correction (0xE0DEF0) still ends on the target
            let before = body.feet;
            super::coast(&mut body, &collision, dt);
            let d = body.feet - before;
            if let AirMode::Jump { from, clip_end, .. } = &mut data.air.mode {
                *from += d;
                *clip_end += d;
            }
            continue;
        }
        let air = &mut data.air;
        air.time_in_air += dt;
        if GAME_FALLS {
            air.speed_ratio = super::falls::speed_ratio(&pad, body.forward());
            // Record current feet before integration, on the first descending update (0xE0DFC7 / 0xE0DD15).
            if !air.apex_reached && body.velocity.y < -0.001 {
                air.apex_reached = true;
                air.apex_y = body.feet.y;
            }
        }

        let mut landed_at: Option<f32> = None;
        // landed by arriving on a jump target (0xE07D00) rather than by ground contact (0xE05200)
        let mut on_target = false;
        let mut hang_on: Option<LedgeEntry> = None;
        let mut hay_on: Option<super::hay::HayStackEntry> = None;
        let mut narrow_on: Option<TransitionSetup> = None;
        let mut pass_on: Option<super::passover::PassOverEntry> = None;
        let mut swing_on = false;
        let mut ladder_on: Option<super::ladder::LadderEntry> = None;
        let foot = (!air.foot_left) as usize;
        match air.mode {
            AirMode::Jump { from, clip_end, aim, apex, duration, t, then_fall_to, real, t_takeoff, fwd } => {
                let t1 = (t + dt).min(duration);
                let s = t1 / duration;
                // the takeoff + flight items' blended root motion (0xE0DEF0); `real` is always set now
                let clip_pos = if real {
                    from + to_world(jump_disp(air, t1, t_takeoff, duration), fwd)
                } else {
                    from.lerp(clip_end, s) + Vec3::Y * (4.0 * apex * s * (1.0 - s))
                };
                // linear correction toward the real target (0xE0DEF0)
                let path = |tt: f32| -> Vec3 {
                    let ss = (tt / duration).clamp(0.0, 1.0);
                    let cp = if real { from + to_world(jump_disp(air, tt.clamp(0.0, duration), t_takeoff, duration), fwd) } else { from.lerp(clip_end, ss) + Vec3::Y * (4.0 * apex * ss * (1.0 - ss)) };
                    cp + (aim - clip_end) * ss
                };
                let next = clip_pos + (aim - clip_end) * s;
                // the path's own velocity (not the collision-resolved displacement: a depenetration push is not
                // a velocity, using it flung the character up when a foot clipped the lip of a higher roof)
                let path_vel = (next - path(t1 - dt.max(1e-3))) / dt.max(1e-3);
                // The game drives the jump as a velocity through the character proxy (0xE0DEF0 → controller +0x90,
                // 0x57C7C0): the body moves by the path's step and slides along what it touches (a lip's rounded
                // contact lifts it over). PORT: a lag behind the path (a contact held the body back) is closed at
                // `JUMP_LAG_RECOVERY`/s so the arrival still lands on the target; driving the absolute position instead
                // made the body stall on a lip, then jump ahead to catch up.
                let lag = path(t) - body.feet;
                let mv = (next - path(t)) + lag * (dt * JUMP_LAG_RECOVERY).min(1.0);
                let r = { let b = &mut *body; collision.move_capsule(&mut b.proxy, b.feet, mv, false, dt) };
                body.velocity = path_vel;
                body.heading = super::heading_of(fwd);
                body.feet = r.position;
                let blocked = (r.position - next).length() > JUMP_BLOCKED_LAG;
                if air.free_target && r.landed && path_vel.y <= 0.0 {
                    // the free jump comes down on the ground before its target (0xE05200, controller contact)
                    landed_at = Some(body.feet.y);
                } else if blocked {
                    // held back by a wall (anti-stuck 0xE0B1E0, simplified): fall from here with the path's
                    // horizontal velocity damped and no upward push
                    air.mode = AirMode::Fall { steer_to: None };
                    body.velocity.x *= 0.3;
                    body.velocity.z *= 0.3;
                    body.velocity.y = body.velocity.y.min(0.0);
                } else if t1 >= duration {
                    // arrival (0xE07D00, within 0.01 m): the rest of a contact's lag is absorbed here
                    if (body.feet - aim).length() > ARRIVAL_TOLERANCE && collision.capsule_fits(aim) {
                        body.feet = aim;
                    }
                    // hand-off by target type (ledge targets → Ledge context)
                    if let Some((mid, normal)) = air.target.and_then(|t| t.hang) {
                        // the reception by the flight that arrived (CheckJumpTargetArrival 0xE07D00)
                        let t = air.target.unwrap();
                        let arrival = match t.straight {
                            Some(j) => LedgeArrival::Straight(j),
                            None => LedgeArrival::Surface { free: t.type_flags == super::targets::TARGET_LEDGE_FREE },
                        };
                        let mut e = LedgeEntry::at(mid, normal, body.feet, LedgeSubState::HangWallReception);
                        if matches!(arrival, LedgeArrival::Surface { free: true }) {
                            // no foot holds: SwingReception (Ledge SubState 8), `swing`
                            swing_on = true;
                        } else {
                            e.entry_move = Some(ledge_moves::arrival_move(arrival, body.feet, e.hand_l, e.hand_r, normal, &collision));
                        }
                        hang_on = Some(e);
                    }
                    // ladder target (0x1000): 0xE07D00 plays `ARRIVE_TARGET`, interpolates the root onto it (0.2 s) and
                    // sets Ladder EntryType 1 (state 2 waits for the action, then the low wait)
                    if let Some(mut e) = air.target.and_then(|t| t.ladder) {
                        e.from = body.feet;
                        // the arrival's first item blends [up, down, long] by the flight's weights (sub_502E30, +452)
                        if let Some(f) = air.flight {
                            for (k, x) in f.weights().iter().enumerate().take(3) {
                                e.w[k] = *x;
                            }
                        }
                        ladder_on = Some(e);
                    }
                    // pass-over target (type 2): 0xE07D00 case 2 → Ledge HandPassOver
                    if let Some((edge, normal)) = air.target.and_then(|t| t.pass) {
                        let mut fw = [0.0f32; 5];
                        if let Some(f) = air.flight {
                            for (k, x) in f.weights().iter().enumerate().take(5) {
                                fw[k] = *x;
                            }
                        }
                        pass_on = Some(super::passover::PassOverEntry { edge, normal, from: body.feet, flight_foot_left: air.foot_left, flight_w: fw });
                    }
                    if air.target_flags == jump_blend::TARGET_HAYSTACK && then_fall_to.is_none() {
                        if let Some(stack) = guidance.haystacks.iter().find(|s| {
                            let p = body.feet;
                            p.x >= s.min.x - 0.3 && p.x <= s.max.x + 0.3 && p.z >= s.min.z - 0.3 && p.z <= s.max.z + 0.3
                        }) {
                            // arrival on a haystack target (0xE07D00 → RequestHayStack 0xE01AF0)
                            let faith = air.flight.is_some_and(|f| f.id == jump_blend::FLIGHT_FAITH);
                            hay_on = Some(super::hay::HayStackEntry { stack: *stack, faith, from: body.feet, speed: body.velocity.length() });
                        }
                    }
                    // a jump without a real target (on the spot / free jump) can come down on a beam or a pilotis
                    if !AIR_CATCHES && air.target.is_none() && hang_on.is_none() {
                        narrow_on = narrow_catch(body.feet, fwd, foot, &guidance, &collision);
                    }
                    match then_fall_to {
                        _ if hang_on.is_some() || hay_on.is_some() || narrow_on.is_some() || pass_on.is_some() || ladder_on.is_some() => {}
                        Some(p) if collision.ground_height(body.feet + Vec3::Y * 0.05, 0.1).is_none() => {
                            air.mode = AirMode::Fall { steer_to: if p == aim { None } else { Some(p) } };
                        }
                        None => {
                            landed_at = Some(body.feet.y);
                            on_target = real;
                        }
                        _ => landed_at = Some(body.feet.y),
                    }
                } else {
                    air.mode = AirMode::Jump { from, clip_end, aim, apex, duration, t: t1, then_fall_to, real, t_takeoff, fwd };
                }
            }
            AirMode::Fall { steer_to } => {
                body.velocity.y -= GRAVITY * dt;
                let mut h = Vec3::new(body.velocity.x, 0.0, body.velocity.z);
                match steer_to {
                    Some(p) => {
                        // steer horizontally onto the real target, ≤ 15 m/s (0xE0DEF0 / 0xE00730)
                        if GAME_FALLS {
                            let t_left = super::falls::time_to_height(body.feet.y, p.y, body.velocity.y);
                            if t_left > 0.0 {
                                // PORT: Body stores m/s and integrates variable dt; native tail stores m/frame.
                                let target = Vec3::new(p.x, body.feet.y, p.z);
                                h += super::falls::correction(body.feet, target, h, t_left, dt);
                            }
                            h = h.clamp_length_max(FREEFALL_MAX_HORIZONTAL);
                        } else {
                            let t_left = (2.0 * (body.feet.y - p.y).max(0.01) / GRAVITY).sqrt().max(dt);
                            h = (Vec3::new(p.x - body.feet.x, 0.0, p.z - body.feet.z) / t_left).clamp_length_max(FREEFALL_MAX_HORIZONTAL);
                        }
                    }
                    None => {
                        // Non-target drift 0xE0F643–0xE0F707, constants 0x19BA434/438.
                        // 0x4F8C40: decay only without controller contacts. PORT: use the proxy manifold
                        // as the native contact-list adapter.
                        // PORT: a fall entered faster than the cap (a free jump at sprint speed, 6.3 m/s) decays onto it
                        // at the same 4 m/s² instead of being clamped in one frame (a visible lurch). The game reaches
                        // this path from its own entries only (LIVE: horizontal speed of a sprint off an edge).
                        let len = h.length();
                        let new_len = (len - DRIFT_DECEL * dt).max(0.0);
                        let new_len = if len > DRIFT_MAX { new_len.max(DRIFT_MAX) } else { new_len.min(DRIFT_MAX) };
                        h = if GAME_FALLS { super::falls::drift(h, dt, !body.proxy.manifold.is_empty()) } else if len > 1e-4 { h * (new_len / len) } else { Vec3::ZERO };
                    }
                }
                body.velocity.x = h.x;
                body.velocity.z = h.z;
                // `HumanInAir__DropSteerAwayFromWall` 0xE04B60: for the first 0.3 s of a drop the facing turns toward the
                // edge normal (away from it with the back to the drop) at 3 rad/s until within 30 deg (fully, with no edge)
                if let Some((_, side, n)) = air.drop.filter(|_| air.fall_t < 0.3) {
                    let want = n.map(|n| if side == 1 { -n } else { n }).unwrap_or(body.forward());
                    let want = Vec3::new(want.x, 0.0, want.z).normalize_or(body.forward());
                    let cone = if n.is_some() { 30f32.to_radians() } else { 0.0 };
                    let f = body.forward();
                    let a = right_of_signed(f, want);
                    if a.abs() > cone {
                        let step = (a.abs() - cone).min(3.0 * dt) * a.signum();
                        body.heading += step;
                    }
                }
                air.fall_t += dt;
                // narrow objects are caught before the ground contact (0xE0BB70 runs before 0xE05200)
                if !AIR_CATCHES && body.velocity.y <= 0.0 && air.apex_y - body.feet.y < 9.0 && air.time_in_air > 0.1 {
                    narrow_on = narrow_catch(body.feet + body.velocity * dt, body.forward(), foot, &guidance, &collision);
                }
                let r = { let b = &mut *body; collision.move_capsule(&mut b.proxy, b.feet, b.velocity * dt, false, dt) };
                body.feet = r.position;
                // PORT: static proxy contacts have no character bodies. The 0.3 m / Data+528
                // rejection gate (0xE038A0) is inactive until native AntiStuckNudge (0xE0B1C0)
                // arms it; that recovery path is not yet ported.
                let contact = if GAME_GROUND_EXTRAS {
                    super::ground_extras::ground_contact(body.proxy.manifold.iter().map(|c| c.normal.y))
                } else { super::ground_extras::GroundContact::None };
                if contact == super::ground_extras::GroundContact::Steep {
                    air.slope_slide.contact(contact, body.feet.y);
                    let height = if air.apex_reached { air.apex_y-body.feet.y } else { 0.0 };
                    air.slope_slide.check(height,body.feet.y,false);
                    // PORT: articulated RagFall is a logged dependency; retain the required flag instead of
                    // inventing a ragdoll/recovery animation (native 0xE05200 -> 0xDCE480).
                    body.velocity = r.velocity;
                }
                if narrow_on.is_some() {
                    // caught: the entry warps the root onto the beam / top
                } else if r.hit_wall && contact != super::ground_extras::GroundContact::Steep {
                    body.velocity.x = 0.0;
                    body.velocity.z = 0.0;
                }
                if r.hit_ceiling && body.velocity.y > 0.0 {
                    body.velocity.y = 0.0;
                }
                if GAME_FALLS && narrow_on.is_none() {
                    // 0xE05490 checks hay before ordinary landing. PORT: greybox AABB top crossing replaces
                    // native controller contacts with type-3, subtype-8/9 entities; side entry is not reconstructed.
                    hay_on = guidance.haystacks.iter().find(|stack| {
                        let next = body.feet;
                        let previous = next - body.velocity * dt;
                        body.velocity.y < 0.0 && previous.y >= stack.max.y && next.y <= stack.max.y
                            && next.x >= stack.min.x && next.x <= stack.max.x
                            && next.z >= stack.min.z && next.z <= stack.max.z
                    }).map(|stack| super::hay::HayStackEntry { stack: *stack, faith: false, from: body.feet, speed: body.velocity.length() });
                }
                if narrow_on.is_some() || hay_on.is_some() {
                } else if r.landed && body.velocity.y <= 0.0 {
                    landed_at = Some(body.feet.y);
                } else if !AIR_CATCHES && if GAME_FALLS { pad.hand_held && air.apex_reached && air.apex_y - body.feet.y > 0.3 } else { pad.legs_held && body.velocity.y <= 0.5 && air.time_in_air > 0.3 } {
                    // grab requested (SetGrabRequested 0xE102D0) → a ladder first (FindLadderCatch 0xE04100), then a ledge
                    // in reach
                    if let Some(e) = super::ladder::find_ladder_catch(body.feet, body.forward(), true, &guidance, &collision) {
                        ladder_on = Some(e);
                    } else if let Some(h) = find_air_catch(body.feet, body.forward(), &guidance) {
                        air.long_catch = air.apex_y - body.feet.y >= 3.0;
                        hang_on = Some(LedgeEntry { catch: Some(air.long_catch), ..LedgeEntry::at(guidance.fit_hands(h.point, h.wall_normal), h.wall_normal, body.feet, LedgeSubState::HangWallReception) });
                    }
                }
            }
            AirMode::Idle => {
                air.mode = AirMode::Fall { steer_to: None };
            }
        }

        if AIR_CATCHES && !on_target && ladder_on.is_none() && hang_on.is_none() && pass_on.is_none() {
            let fall_height = if air.apex_reached { air.apex_y - body.feet.y } else { 0.0 };
            let manual = super::air_catches::manual(pad.hand_held, fall_height, air.drop.map(|d| d.0));
            let automatic = super::air_catches::automatic(air.time_in_air, air.drop.is_some(), air.target.map(|t| t.type_flags), body.forward(), body.proxy.manifold.iter().map(|c| c.normal));
            // 0xE0BB70: raw reach direction (+32), seeded from facing when the stick is zero.
            let reach = if manual && pad.speed01 > 0.0 { pad.dir } else { body.forward() };
            if manual || automatic {
                if let Some(mut e) = super::ladder::find_ladder_catch(body.feet, reach, manual, &guidance, &collision) {
                    e.facing = body.forward();
                    e.catch_speed = body.velocity.length();
                    ladder_on = Some(e);
                } else if manual {
                    // PORT: the pre-existing generic ledge adapter is outside this catch-geometry pass.
                    if let Some(h) = find_air_catch(body.feet, reach, &guidance) {
                        air.long_catch = fall_height >= 3.0;
                        hang_on = Some(LedgeEntry { catch: Some(air.long_catch), ..LedgeEntry::at(guidance.fit_hands(h.point, h.wall_normal), h.wall_normal, body.feet, LedgeSubState::HangWallReception) });
                    }
                }
            }
            if ladder_on.is_none() && hang_on.is_none() && super::air_catches::narrow(fall_height, air.time_in_air, air.drop.is_some()) {
                let ahead = super::air_catches::target_ahead(body.feet, air.target, &collision);
                if !ahead { narrow_on = narrow_catch(body.feet, body.forward(), foot, &guidance, &collision); }
            }
        }

        // apex tracking: first frame vertical velocity < -0.001 (RE/04 §1)
        if !GAME_FALLS && !air.apex_reached && body.feet.y - air.prev_y < -0.001 * dt {
            air.apex_reached = true;
            air.apex_y = air.prev_y;
        }
        if !air.apex_reached {
            air.apex_y = air.apex_y.max(body.feet.y);
        }
        air.prev_y = body.feet.y;
        body.grounded = false;

        if let Some(e) = ladder_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, TransitionSetup::ToLadder(e));
            if AIR_CATCHES { data.ladder.advance_catch_root(&mut body, dt); }
        } else if let Some(entry) = hang_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            let flight = air.flight;
            switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(entry));
            if swing_on {
                let n = entry.normal;
                let root = super::ledge::hang_root_at(entry.hand_l, entry.hand_r, n, super::ledge::LedgeHangType::Free, &collision);
                let fwd = -Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
                let seq = data.ledge.swing.map(|s| s.seq).unwrap_or(0);
                data.ledge.swing = super::swing::enter(body.feet, root, fwd, (entry.hand_l + entry.hand_r) * 0.5, flight, seq);
                data.ledge.hang_type = super::ledge::LedgeHangType::Free;
                data.ledge.hang_set = true;
            }
        } else if let Some(e) = pass_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, TransitionSetup::ToPassOver(e));
        } else if let Some(setup) = narrow_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, setup);
            if AIR_CATCHES { data.narrow.advance_catch_root(&mut body, dt); }
        } else if let Some(e) = hay_on {
            air.mode = AirMode::Idle;
            body.velocity = Vec3::ZERO;
            switch_context(&mut loco, &mut data, TransitionSetup::ToHayStack(e));
        } else if let Some(y) = landed_at {
            body.grounded = true;
            let impact_velocity = body.velocity;
            body.velocity.y = 0.0;
            let mut landing = classify_landing(air.apex_y, air.apex_reached, air.start_y, y);
            landing.action = Some(if on_target && matches!(air.target_flags, 1 | 0x10000) {
                // free-step reception: the flight's weights, normal / `_fast` by +0x16C (0xE07D00).
                // PORT: the game continues in NarrowObject (on the edge); the port stays in Ground.
                let fw = air.flight.map(|f| f.weights().to_vec()).unwrap_or_default();
                let id = jump_blend::RECEPTION_FREESTEP[(!air.foot_left) as usize];
                ActionBlend::new(id, 0, &jump_blend::reception_weights(&fw, air.speed_ratio))
            } else {
                // ground landing (0xE05940): drop and horizontal distance from the jump/fall start; the
                // forward/straight test uses the trajectory pitch, not the pad (0xE05B64).
                let horiz = Vec2::new(body.feet.x - air.start.x, body.feet.z - air.start.z).length();
                let stick_forward = if GAME_FALLS { super::falls::forward_landing(impact_velocity) } else { pad.speed01 > 0.0 && pad.dir.dot(body.forward()) >= 75f32.to_radians().cos() };
                let (id, w) = jump_blend::landing(air.start_y - y, horiz, stick_forward, air.speed_ratio, air.foot_left);
                ActionBlend::new(id, 0, &w)
            });
            landing.roll = landing.action.is_some_and(|a| a.id == jump_blend::LAND_DAMAGE_ROLL);
            air.mode = AirMode::Idle;
            // A free-step arrival continues in NarrowObject (0xE07D00: SubState 6 → Movement); its Movement state
            // tries a pilotis first (`TryPilotisFreeStep` 0xE50190), then a beam (`TryMountBeam` 0xE52AD0), and
            // returns to Ground on wide support (0xE51190, the port's default).
            if on_target && matches!(air.target_flags, 1 | 0x10000) {
                let fwd = body.forward();
                let foot = (!air.foot_left) as usize;
                let stick = (pad.speed01 > 0.0).then_some(pad.dir);
                if let Some(top) = super::narrow::find_pilotis(body.feet, body.feet, fwd, false, &guidance, &collision) {
                    let e = super::narrow::PilotisEntry { top, from: body.feet, facing: fwd, kind: super::narrow::PilotisEntryType::FromFreeStep, foot };
                    switch_context(&mut loco, &mut data, TransitionSetup::ToPilotis(e));
                    continue;
                }
                if let Some(e) = super::narrow::free_step_beam_entry(body.feet, fwd, stick, foot, landing.action, pad.high_profile, &guidance, &collision) {
                    switch_context(&mut loco, &mut data, TransitionSetup::ToBeam(e));
                    continue;
                }
            }
            switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: Some(landing) });
        }
    }
}

#[cfg(test)]
mod fall_tests {
    use super::*;
    #[test]
    fn fall_entry_clears_previous_flight_and_keeps_all_native_origins() {
        for origin in [FallOrigin::Ground,FallOrigin::Climb,FallOrigin::HangWall,FallOrigin::HangFree] {
            let mut air=HumanInAirData::default();
            air.flight=Some(ActionBlend::new(1,0,&[1.0]));
            air.apex_reached=true;
            let from=Vec3::new(2.0,10.0,4.0); let velocity=Vec3::new(3.0,2.0,0.0);
            air.enter(InAirEntry::Fall{from,velocity,origin,speed_param:0.5});
            assert_eq!(air.fall_origin,origin);
            assert_eq!(air.entry_velocity,Some(velocity));
            assert!(!air.apex_reached && air.flight.is_none() && air.takeoff.is_none());
            assert_eq!(air.apex_y,from.y);
        }
    }
    #[test]
    fn fall_height_is_zero_until_the_apex_flag() {
        assert_eq!(classify_landing(25.0,false,25.0,0.0).kind,LandingType::Safe);
        assert_eq!(classify_landing(25.0,true,25.0,0.0).kind,LandingType::Fatal);
    }
}
