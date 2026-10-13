//! NarrowObject's Movement state: the free-step stay (NarrowObject SubState 6 → inner state 1, RE/05 §2.15).
//!
//! Every free-step landing (`HumanInAir__CheckJumpTargetArrival` 0xE07D00, types 1 / 0x10000) and every climb-up onto a
//! top (`HumanLedge__SwitchToNarrowObject_Enter` 0xDD2A50: `hangknee_tr_freestep_entry`) plays its action here, in
//! context 12, before the Ground, a beam or a pilotis takes over:
//! - `StateMovement_Enter` 0xE4FE20 looks for a beam piece ahead of the support point (flag 0x20);
//! - `StateMovement_Update` 0xE534B0 → `CheckSupportAndFall` 0xE51190: the root interpolation, the haystack past
//!   phase 0.6, and without the beam piece the Ground setup when the action is done (with the stick pointing back
//!   over the edge, from phase 0.5); then `TryPilotisFreeStep` 0xE50190 and `TryMountBeam` 0xE52AD0 once the action is
//!   done;
//! - the player's input (`GoAssassinActionInterpreter__NarrowObjectState` 0xEE8EC0): the wall run (event 15) and the
//!   jump on (event 4, from phase 0.5, guard 0xE4C700) as a free-step jump (kind 1, 0xE4D780).

use bevy::prelude::*;

use super::abilities::{Ability, AbilitySet};
use super::air::InAirEntry;
use super::jump_blend::ActionBlend;
use super::jump_candidates::Query;
use super::{switch_context, Body, HumanDataBundle, Locomotion, TransitionSetup};
use crate::collision::CollisionWorld;
use crate::guidance::GuidanceWorld;
use crate::input::PadInput;

/// cos 140° (flt_1702A68 = 0xBF441B7D, `HumanNarrowObject__StickDotAbove` 0xE4D6B0 at 0xE515BE).
const BACK_COS: f32 = -0.766_044_4;
/// The phase from which a stick pointing back leaves, and the jump is accepted (dword_17028A8, 0xE515D0 / 0xE4C75D).
const HALF: f32 = 0.5;
/// The haystack search starts past this phase (dword_16834C4, 0xE51434).
const HAY_PHASE: f32 = 0.6;
/// The Ground setup's stick test: within 120° of the facing (dword_16D908C, 0xE51857 / 0xE51872).
const MOVE_ANGLE: f32 = 2.094_395_2;
/// The interpreter's stick dead zone (0xEE8F93) and the wall run's 60° (0xEE951C).
const STICK_MIN: f32 = 0.35;
const WALL_RUN_ANGLE: f32 = 1.047_197_6;
/// The wall run from NarrowObject warps onto the wall in 0.13 s (`sub_B263B0`).
const WALL_RUN_WARP: f32 = 0.13;

/// What a context hands the stay: the writers of NarrowObjectData +0xF0 / +0x100 / +0x140 and SubState 6.
#[derive(Clone, Copy, Debug)]
pub struct FreeStepEntry {
    /// The action the stay plays: the free-step reception (24 clips), `hangknee_foot{l,r}_tr_freestep_entry`
    /// (0x248E9730 / 31), the one-hand 0x39193BBB, or the step-up's reception 0x012B33F1.
    pub action: ActionBlend,
    /// Time already played into it.
    pub t: f32,
    /// The root interpolation: `from` → `to` over `root_time` s (the arrival's 0.067 s, 0xE09B26). After it the
    /// action's own root motion moves the body when `follow_clip`.
    pub from: Vec3,
    pub to: Vec3,
    pub root_time: f32,
    pub follow_clip: bool,
    /// +0xF0: the support point (the arrival: the target + 0.5 · the jump direction; the climb-ups: the root).
    pub point: Vec3,
    /// +0x100: the support normal (the target's normal, pointing back at the jumper; the climb-ups: −the facing).
    pub normal: Vec3,
    /// +0x140: the target's hit type is 4 (a beam): the query box reaches only 0.25 m ahead.
    pub small_box: bool,
    /// The heading kept, then turned by the action's own root yaw.
    pub heading: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct FreeStep {
    pub e: FreeStepEntry,
    pub t: f32,
    /// Time since the entry (the root interpolation's clock).
    k: f32,
    applied: [f32; 3],
    /// `StateMovement_Enter` has run (on the first update).
    entered: bool,
    /// Flag 0x20: the beam piece found ahead of the support point (NarrowObject +64 / +80).
    pub edge: Option<(Vec3, Vec3)>,
}

impl FreeStep {
    pub fn new(e: FreeStepEntry) -> Self {
        let d = e.action.duration().max(1e-4);
        FreeStep { e, t: e.t, k: 0.0, applied: e.action.disp((e.t / d).min(1.0)), entered: false, edge: None }
    }

    pub fn phase(&self) -> f32 {
        (self.t / self.e.action.duration().max(1e-4)).min(1.0)
    }

    /// The playing item's exit feet (bits 2–3 of its word): 0 left ahead, 1 right ahead (`(word & 0xC) != 4`, as the
    /// beam and pilotis entries read it, RE/24 NARROW-7).
    pub fn exit_foot(&self) -> usize {
        super::anim_gate::word(&self.e.action).map_or(0, |w| ((w & 0xC) != 4) as usize)
    }
}

/// `StateMovement_Enter` 0xE4FE20 (FreeRunSubState 0): the beam report query `sub_116E1A0` at the support point along
/// −the support normal (flattened), cone 135°, box (−0.5, −0.25, −0.25)…(0.5, 1.25, 0.25) in (across, ahead, up)
/// (ahead at most 0.25 with +0x140), the nearest piece horizontally, clipped to the box (dword_16839B0 / 1690238 /
/// 16BF7D0 / flt_1682378, 0xE4FF23–0xE4FF3E). The edge crossed on the way in faces the jumper and lies outside the
/// cone: a piece here is a narrow top under the feet. PORT: HumanGuidance's runtime beam report is `beam_contacts`
/// (as for the other `sub_116E1A0` queries).
pub fn stay_edge(point: Vec3, normal: Vec3, small_box: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3)> {
    let f = -Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
    if f == Vec3::ZERO {
        return None;
    }
    let r = super::right_of(f);
    let ahead = if small_box { 0.25 } else { 1.25 };
    // the box's centre and half extents (across, ahead, up)
    let centre = point + f * ((ahead - 0.25) * 0.5);
    let half = [0.5, (ahead + 0.25) * 0.5, 0.25];
    let reach = r.abs() * half[0] + f.abs() * half[1] + Vec3::Y * half[2];
    let mut best: Option<(f32, (Vec3, Vec3))> = None;
    for b in crate::guidance::beam_contacts::detect(guidance, collision, centre, reach) {
        let v = b.p1 - b.p0;
        let flat = Vec3::new(v.x, 0.0, v.z);
        if flat.length_squared() < 1e-8 {
            continue;
        }
        // cone 135° around the query direction: the piece oriented the nearer way is always within 90°
        let Some((t0, t1)) = super::narrow::clip_piece(b.p0, b.p1, centre, [r, f, Vec3::Y], half) else { continue };
        let (c0, c1) = (b.p0 + v * t0, b.p0 + v * t1);
        let q = crate::guidance::closest_on_segment(c0, c1, point);
        let d = Vec2::new(q.x - point.x, q.z - point.z).length_squared();
        if best.is_none_or(|x| d < x.0) {
            best = Some((d, (b.p0, b.p1)));
        }
    }
    best.map(|b| b.1)
}

/// IHuman vt144 `Human__FindDropEdgeAhead` 0xB1C100 with the narrow interpreter's arguments (−0.3, 1.1, −0.3, 0.3, 0.5,
/// 0.02, normal): the zone is the vertical plane through `pos` along `dir`, from 0.3 m behind to 1.1 m ahead and 0.3 m
/// below to 0.3 m above; the edges crossing it (`sub_9765E0`), not steep, whose outward normal (the lower face's,
/// flattened) faces along `dir`, with a drop of 0.5 m or more beyond them (`Human__MeasureDropBeyondEdge` 0xB19620 along
/// the normal, 10 m, step 0.02); the nearest to the zone's rear point. Returns its drop.
pub fn drop_edge_ahead(pos: Vec3, dir: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<f32> {
    let f = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if f == Vec3::ZERO {
        return None;
    }
    let side = super::right_of(f);
    let rear = pos + f * -0.3 + Vec3::Y * -0.3;
    let mut best: Option<(f32, f32)> = None;
    for e in &guidance.edges {
        if crate::guidance::steep(e.p1 - e.p0) {
            continue;
        }
        let (s0, s1) = ((e.p0 - pos).dot(side), (e.p1 - pos).dot(side));
        if s0 * s1 > 0.0 || (s0 - s1).abs() < 1e-6 {
            continue;
        }
        let x = e.p0.lerp(e.p1, s0 / (s0 - s1));
        let (along, up) = ((x - pos).dot(f), x.y - pos.y);
        if !(-0.3..=1.1).contains(&along) || !(-0.3..=0.3).contains(&up) {
            continue;
        }
        let n = super::jump_candidates::wall_of(e.n0, e.n1);
        if n == Vec3::ZERO || n.dot(f) < 0.0 {
            continue;
        }
        let mut drop = super::jump_candidates::measure_drop(x, n, 10.0, 0.02, collision);
        // PORT: a haystack is solid to the game's drop rays (Damascus's `Hay_Bale_*` collision, the controller touches
        // it, event 122); the port's haystacks sit on the Dynamic layer the drop rays leave out, so its top ends the drop
        // here (live 2026-10-09: free-running onto a cart's rim drops into the haystack from the stay, it does not jump
        // off the rim)
        let beyond = x + n * 0.25;
        for s in &guidance.haystacks {
            if beyond.x >= s.min.x && beyond.x <= s.max.x && beyond.z >= s.min.z && beyond.z <= s.max.z {
                drop = drop.min((x.y - s.max.y).max(0.0));
            }
        }
        if drop < 0.5 {
            continue;
        }
        let d = (x - rear).length_squared();
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, drop));
        }
    }
    best.map(|b| b.1)
}

/// IHuman vt148 0xB15DE0: a beam piece ahead (`sub_116E1A0` on HumanGuidance's report at `pos` + 0.35·`dir`, box
/// (−0.5, 0, −0.5)…(0.5, 0.7, 0.5) in (across, ahead, up), within 45° of `dir`).
pub fn beam_ahead(pos: Vec3, dir: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> bool {
    let f = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if f == Vec3::ZERO {
        return false;
    }
    let r = super::right_of(f);
    let centre = pos + f * (0.35 + 0.35);
    let half = [0.5, 0.35, 0.5];
    let reach = r.abs() * half[0] + f.abs() * half[1] + Vec3::Y * half[2];
    crate::guidance::beam_contacts::detect(guidance, collision, centre, reach).into_iter().any(|b| {
        let along = Vec3::new(b.p1.x - b.p0.x, 0.0, b.p1.z - b.p0.z).normalize_or_zero();
        along.dot(f).abs() >= std::f32::consts::FRAC_1_SQRT_2
            && super::narrow::clip_piece(b.p0, b.p1, centre, [r, f, Vec3::Y], half).is_some()
    })
}

/// The narrow interpreter's jump request (0xEE9564–0xEE96C6): the candidate kind (`None` = 6, no search) and the
/// free-jump mode with the drop edge's drop (`None` = v51 6):
/// - the stick past 0.35 with a drop edge ahead (vt144) of less than 2 m and no beam ahead (vt148): kind and mode 1
///   (low) / 2 (high);
/// - high profile with the stick: Legs held → kind 0 and mode 0 with a drop edge and no beam ahead, else kind 4;
///   without Legs kind 3;
/// - high profile, a pending press with Legs let go → kind 0, mode 0.
#[allow(clippy::too_many_arguments)]
pub fn jump_kind(pos: Vec3, stick: Vec3, high: bool, moving: bool, legs: bool, pressed: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> (Option<u32>, Option<(u32, Option<f32>)>) {
    let drop = moving.then(|| drop_edge_ahead(pos, stick, guidance, collision)).flatten();
    let beam = moving && beam_ahead(pos, stick, guidance, collision);
    let (mut kind, mut free) = (None, None);
    if let Some(d) = drop.filter(|d| *d < 2.0 && !beam) {
        let k = if high { 2 } else { 1 };
        kind = Some(k);
        free = Some((k, Some(d)));
    }
    if high {
        if moving {
            if !beam && drop.is_some() && legs {
                kind = Some(0);
                free = Some((0, drop));
            } else if legs {
                kind = Some(4);
                free = None;
            } else {
                kind = Some(3);
            }
        }
        if pressed && !legs {
            kind = Some(0);
            free = Some((0, None));
        }
    }
    (kind, free)
}

/// The narrow interpreters' jump once its kind is chosen (NarrowObjectState 0xEE96C6–0xEE99F0, BeamState
/// 0xEEAC38–0xEEAF8D): IHuman vt56 with that kind, with beams and no subtype left out, along the interpreter's stored
/// stick (+4320, `dir`), then the scorer (mode 1; PORT: the second target of mode 2, vt20, is not kept); the Leap of
/// Faith for kind 0; then, with no candidate, the free jump by its mode (IHuman vt68 `Human__BuildFreeJumpTarget`
/// 0xB1E7F0: 0 → 8 m ahead, 1 → 1 m, 2 → 3 m; 3 m below, or min(drop + 0.25, 3) under the drop edge's report). Both post
/// event 4, the free-step takeoff (jump kind 1).
#[allow(clippy::too_many_arguments)]
pub fn narrow_jump(feet: Vec3, forward: Vec3, dir: Vec3, kind: Option<u32>, free: Option<(u32, Option<f32>)>, ab: &AbilitySet, foot_left: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<InAirEntry> {
    let kind = kind?;
    let q = Query { kind, mode: 0, long: false, beams: true, exclude: 0 };
    let target = super::targets::find_target(feet, forward, dir, &q, false, guidance, collision)
        .or_else(|| (kind == 0 && ab.allows(Ability::LeapOfFaith)).then(|| super::targets::haystack_target(feet, dir, forward, guidance, collision)).flatten());
    if std::env::var_os("AC_JUMP_LOG").is_some() {
        eprintln!("narrow jump: kind {kind} free {free:?} -> {:?}", target.map(|t| (t.type_flags, t.position)));
    }
    if let Some(target) = target {
        return Some(InAirEntry::FreeStepJump { from: feet, target, foot_left });
    }
    let (mode, drop) = free?;
    let ahead = match mode {
        0 => crate::tuning::FREE_JUMP_AHEAD,
        1 => 1.0,
        _ => 3.0,
    };
    let down = drop.map_or(3.0, |d| (d + 0.25).min(3.0));
    Some(InAirEntry::FreeStepFreeJump { from: feet, dir, ahead, down, foot_left })
}

/// The Ground setup the stay builds (0xE517C8–0xE51934): `TransitionSetupDataToMovement` mode by the profile
/// (+0xB8 bit 7), moving (`GetMoveModeFromStick` 0xC7F000) with the stick within 120° of the facing, and Legs held
/// (+0xB9 bit 1, IHumanNarrowObject vt64 from the interpreter):
/// high → 6 (Legs, ActionParams [0, 0, 1]) / 4 / 1 (not moving); low → 3 / 0.
pub fn ground_mode(high: bool, moving: bool, stick: Vec3, forward: Vec3, legs: bool) -> u8 {
    let toward = moving && stick.angle_between(forward) <= MOVE_ANGLE;
    match (high, toward) {
        (true, true) if legs => 6,
        (true, true) => 4,
        (true, false) => 1,
        (false, true) => 3,
        (false, false) => 0,
    }
}

/// One update of the stay (the interpreter's requests first, then `StateMovement_Update`). Switches context itself.
#[allow(clippy::too_many_arguments)]
pub fn update(
    loco: &mut Locomotion,
    body: &mut Body,
    data: &mut HumanDataBundle,
    pad: &mut PadInput,
    ab: &AbilitySet,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
    anim: Option<&crate::anim::AnimPlayer>,
    dt: f32,
) {
    let Some(mut fs) = data.narrow.free_step else { return };
    if !fs.entered {
        fs.entered = true;
        fs.edge = stay_edge(fs.e.point, fs.e.normal, fs.e.small_box, guidance, collision);
    }
    let a = fs.e.action;
    let dur = a.duration().max(1e-4);
    fs.t += dt;
    fs.k += dt;
    let phase = fs.phase();
    let done = fs.t >= dur;

    // ------------------------------------------------ the root (`RootInterp__Advance`, then the action's root motion)
    if fs.k <= fs.e.root_time && fs.e.root_time > 0.0 {
        let next = fs.e.from.lerp(fs.e.to, (fs.k / fs.e.root_time).min(1.0));
        let r = collision.move_capsule(&mut body.proxy, body.feet, next - body.feet, false, dt);
        body.feet = r.position;
        // the action's root motion meanwhile is replaced by the interpolation (RE/24 AIR-2)
        fs.applied = a.disp(phase);
    } else if fs.e.follow_clip {
        let d = a.disp(phase);
        let step = [d[0] - fs.applied[0], d[1] - fs.applied[1]];
        fs.applied = d;
        let yaw = a.yaw(phase);
        body.heading = (fs.e.heading + yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        let f0 = Vec3::new(-fs.e.heading.sin(), 0.0, -fs.e.heading.cos());
        // ramped in with the blend into the action (RE/24 ANIM-16)
        let delta = (super::right_of(f0) * step[0] + f0 * step[1]) * crate::anim::root_weight(anim, a.id);
        let r = collision.move_capsule(&mut body.proxy, body.feet, delta, true, dt);
        body.feet = r.position;
    }
    body.velocity = Vec3::ZERO;
    body.grounded = true;
    data.narrow.free_step = Some(fs);

    let forward = body.forward();
    let high = pad.high_profile;
    let moving = pad.magnitude >= STICK_MIN;
    // NarrowObject +0x30: the stick's direction past 0.35, else the facing (vt0, 0xEE8FBD); the interpreter's +4320
    // keeps the last strong stick for the jump search
    let stick = if moving && pad.dir_snap != Vec3::ZERO { pad.dir_snap } else if moving { pad.dir } else { forward };

    // ------------------------------------------------ the wall run (event 15), first in the interpreter (0xEE94B0)
    // high profile, Legs held, the stick past 0.35 within 60° of the facing, the Walling ability; the guard is the wall
    // test 1.5 · h ahead (`StateMovement_GuardWallRun` 0xE503D0 → `Human__WallingWallTest` 0xE18390), the fill 0xB263B0
    if high && moving && pad.legs_held && pad.stick_facing_angle.abs() < WALL_RUN_ANGLE && ab.allows(Ability::Walling) {
        if let Some((contact, normal)) = super::walling::wall_ahead(body.feet, forward, collision) {
            pad.consume_jump();
            let from = body.feet;
            switch_context(loco, data, TransitionSetup::ToWalling(super::walling::WallingEntry { contact, normal, from, warp_duration: Some(WALL_RUN_WARP) }));
            return;
        }
    }

    // ------------------------------------------------ the jump on (event 4)
    // Only while event 4 is accepted (vt16): from phase 0.5 (`StateMovement_GuardJump` 0xE4C700, +0xE0 = 0).
    if moving {
        data.narrow.jump_dir = stick;
    }
    let pressed = pad.jump_buffered() && data.narrow.jump_dir != Vec3::ZERO;
    let (kind, free) = if phase >= HALF { jump_kind(body.feet, stick, high, moving, pad.legs_held, pressed, guidance, collision) } else { (None, None) };
    if ab.allows(Ability::Jump) && phase >= HALF {
        // `SetupFreeStepJumpToTarget` 0xE4D780: kind 1 with the playing item's exit feet (live RE/25 freerun_north: the
        // reception ending right foot ahead takes off `freestep_front_front_050cm_footr_to_air`)
        let foot_left = fs.exit_foot() == 0;
        let dir = if data.narrow.jump_dir != Vec3::ZERO { data.narrow.jump_dir } else { stick };
        if let Some(entry) = narrow_jump(body.feet, forward, dir, kind, free, ab, foot_left, guidance, collision) {
            pad.consume_jump();
            switch_context(loco, data, TransitionSetup::ToInAir(entry));
            return;
        }
    }

    // ------------------------------------------------ `CheckSupportAndFall` 0xE51190
    // (IHuman vt276 from phase 0.5: the hit-reaction exits, not ported, RE/05 §2.15)
    // the haystack under the feet past phase 0.6, searched along −the support normal (0xE51308, RE/24 NARROW-21)
    if phase > HAY_PHASE {
        let into = -Vec3::new(fs.e.normal.x, 0.0, fs.e.normal.z).normalize_or(forward);
        if let Some(stack) = super::hay::find_support(body.feet, into, forward, &guidance.haystacks, collision) {
            let e = super::hay::HayStackEntry { stack, kind: super::hay::HayEntry::FreeStep, from: body.feet, speed: 0.0 };
            switch_context(loco, data, TransitionSetup::ToHayStack(e));
            return;
        }
    }
    // no beam piece ahead: the Ground once the action is done; with the stick pointing back over the edge (more than
    // 140° from −the support normal) from phase 0.5 (live RE/25 roof_jogoff_high: 0.13 s of the 0.27 s climb-up)
    let into = -Vec3::new(fs.e.normal.x, 0.0, fs.e.normal.z).normalize_or_zero();
    let back = moving && stick.dot(into) <= BACK_COS;
    let to_ground = fs.edge.is_none() && if back { phase >= HALF } else { done };

    // ------------------------------------------------ the mounts, once the action is done (vt152 0xE4C670)
    if done {
        let foot = fs.exit_foot();
        // `TryPilotisFreeStep` 0xE50190: the from-air box along −the support normal (RE/24 NARROW-8)
        if let Some(top) = super::narrow::find_pilotis(body.feet, body.feet, into.normalize_or(forward), true, guidance, collision) {
            let e = super::narrow::PilotisEntry { top, from: body.feet, facing: forward, kind: super::narrow::PilotisEntryType::FromFreeStep, foot };
            switch_context(loco, data, TransitionSetup::ToPilotis(e));
            return;
        }
        // `TryMountBeam` 0xE52AD0: the piece StateMovement_Enter found, entered at the root's closest point
        if let Some((p0, p1)) = fs.edge {
            let point = crate::guidance::closest_on_segment(p0, p1, body.feet);
            let st = moving.then_some(stick);
            let e = super::narrow::beam_entry_on(p0, p1, point, body.feet, forward, st, foot, Some(a), high);
            switch_context(loco, data, TransitionSetup::ToBeam(e));
            return;
        }
    }

    if to_ground {
        let mode = ground_mode(high, moving, stick, forward, pad.legs_held);
        let from = a;
        switch_context(loco, data, TransitionSetup::ToMovement { landing: None });
        data.ground.enter_from_free_step(from, mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0xE517C8–0xE518D0: the Ground setup's modes.
    #[test]
    fn the_ground_setup_mode_follows_the_profile_the_stick_and_legs() {
        let f = Vec3::NEG_Z;
        assert_eq!(ground_mode(true, true, f, f, true), 6);
        assert_eq!(ground_mode(true, true, f, f, false), 4);
        assert_eq!(ground_mode(true, false, f, f, false), 1);
        assert_eq!(ground_mode(false, true, f, f, false), 3);
        assert_eq!(ground_mode(false, false, f, f, false), 0);
        // more than 120° from the facing counts as not moving
        assert_eq!(ground_mode(true, true, Vec3::Z, f, false), 1);
        assert_eq!(ground_mode(false, true, Vec3::Z, f, false), 0);
    }
}
