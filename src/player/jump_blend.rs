//! Jump animation selection and blending — `Human__ComputeJumpAnimBlend` 0xB1EC40 (+ 0xB140B0, 0xB13A60),
//! `Human__SetupJumpToTarget` 0xB20200, the free-step reception (CheckJumpTargetArrival 0xE07D00) and the
//! ground landing (`HumanInAir__SetupToGround_Landing` 0xE05940). RE/04 §6.
//!
//! A jump plays two FROMAI items back to back: a **takeoff** (40 clips: 5 direction groups × {front
//! 050/300/550, down 050/300/550, up 050/300} cm) and a **flight** (9 or 16 clips by height/distance). The
//! root follows their blended DISPLACEMENT plus a linear correction to the target over both items
//! (0xE0DEF0). An item's duration is Σ wᵢ·Tᵢ (0x507650 → 0x5B9480).

use super::jump_clips::{ClipRoot, ACTIONS, CLIPS};

// ---------------------------------------------------------------- action ids (verified in the graph dump)
/// Ground running takeoff (jump kind 0), [footl, footr] (0xB1EC40: 172788750 + (foot != 1)).
pub const TAKEOFF_RUN: [u32; 2] = [0x0A4C_8C0E, 0x0A4C_8C0F];
/// Free-step takeoff (jump kind 1: from a beam / pilotis / roof edge), [footl, footr]: same 40-clip layout,
/// `xx_h_freestep_front_*_foot{l,r}_to_air` (0xB1EC40; pilotis jumps pass kind 1 to 0xB20200, 0xE4D950).
pub const TAKEOFF_FREESTEP: [u32; 2] = [0x0112_B589, 0x0112_B5AC];
/// The rebound takeoff (jump kind 2: `HumanWalling__ReboundJump` 0xE365C0, the ledge / climb rebounds), [footl, footr]
/// (0xB1EC40 0xB1EE09: 0x1157C6A + (foot != 1) + 1; RE/24 AIR-5).
pub const TAKEOFF_REBOUND: [u32; 2] = [0x0115_7C6B, 0x0115_7C6C];
/// Flights by target type, [footl, footr].
pub const FLIGHT_FREESTEP: [u32; 2] = [0x010D_DAFA, 0x010D_F0D8]; // 1, 0x10000, 0x100, 0x200, 0x400, haystack
/// `Human__UseBeamFreestepJumpVariants` 0xB145B0 (from 0xB1EC40): a target more than 0.4 m below the start and a
/// **blocked** sweep toward it swaps the free-step takeoff and flight for these (`xx_h_beam_freestep_*_to_air`,
/// `xx_h_beam_air_*_to_freestep`, same 40-clip layouts). Live: the running jump without a target flies 0x118DA41C/D.
pub const TAKEOFF_FREESTEP_DOWN: [u32; 2] = [0x118D_A41A, 0x118D_A41B];
pub const FLIGHT_FREESTEP_DOWN: [u32; 2] = [0x118D_A41C, 0x118D_A41D];
/// The drop below the start beyond which 0xB145B0 swaps to the down variants.
pub const DOWN_VARIANT_DROP: f32 = 0.4;

/// 0xB145B0's swap: the down variants of the free-step takeoff / flight (others unchanged). The drop and the sweep are
/// tested by `HumanInAirData::apply_down_variants` (RE/24 AIR-1).
pub fn down_variants(takeoff: u32, flight: u32) -> (u32, u32) {
    let t = match TAKEOFF_FREESTEP.iter().position(|x| *x == takeoff) { Some(i) => TAKEOFF_FREESTEP_DOWN[i], None => takeoff };
    let f = match FLIGHT_FREESTEP.iter().position(|x| *x == flight) { Some(i) => FLIGHT_FREESTEP_DOWN[i], None => flight };
    (t, f)
}
pub const FLIGHT_PASSOVER: [u32; 2] = super::passover::FLIGHT_PASSOVER; // 2
pub const FLIGHT_ASSASSINATE: [u32; 2] = [0x21B4_DC3D, 0x21B4_DC3E]; // 0x8000
pub const FLIGHT_SURFACE: [u32; 2] = [0x011E_555B, 0x011E_555C]; // 0x40, 0x1000, 0x2000, 0x4000
pub const FLIGHT_SWING: [u32; 2] = [0x0121_A149, 0x0121_A151]; // every other type
/// Free-step reception on arrival at a type 1 / 0x10000 target, after a [footl, footr] flight
/// (0xE07D00: 0x10DE1FE / 0x10DF150). 24 clips: 12 normal + 12 `_fast`.
pub const RECEPTION_FREESTEP: [u32; 2] = [0x010D_E1FE, 0x010D_F150];
/// Ground landings (0xE05940), drop ≤ 3 m. Moving with the stick within 75° of the motion:
/// speed bucket ≥ 1 → [footl, footr] 6-clip {soft, hard} × {walk, jog, sprint impulsion};
/// bucket 0 → [footl, footr] 2-clip {soft, hard} → wait.
pub const LAND_FORWARD_MOVE: [u32; 2] = [0x6D20_0160, 0x6D20_0161];
pub const LAND_FORWARD_STOP: [u32; 2] = [0x6E9C_754A, 0x6E9C_754C];
/// No stick (or turned > 75°): straight landings, moving / stopping.
pub const LAND_STRAIGHT_MOVE: u32 = 0x6E9C_702A;
pub const LAND_STRAIGHT_STOP: u32 = 0x6E9C_7557;
/// Drop > 3 m: `xx_h_landing_damage_footl` (speed ratio ≤ 0.2) or `…_roll`.
pub const LAND_DAMAGE: u32 = 0x010D_D707;
pub const LAND_DAMAGE_ROLL: u32 = 0x010D_D70B;

pub const DUMPED_ACTIONS: &[u32] = &[
    TAKEOFF_RUN[0], TAKEOFF_RUN[1], TAKEOFF_FREESTEP[0], TAKEOFF_FREESTEP[1], TAKEOFF_REBOUND[0], TAKEOFF_REBOUND[1],
    FLIGHT_FREESTEP[0], FLIGHT_FREESTEP[1],
    // the flights to hang targets (wall hang / ladder: `…_to_surface`; free hang and the rest: `…_to_swing`)
    FLIGHT_SURFACE[0], FLIGHT_SURFACE[1], FLIGHT_SWING[0], FLIGHT_SWING[1],
    TAKEOFF_FREESTEP_DOWN[0], TAKEOFF_FREESTEP_DOWN[1], FLIGHT_FREESTEP_DOWN[0], FLIGHT_FREESTEP_DOWN[1],
    RECEPTION_FREESTEP[0], RECEPTION_FREESTEP[1],
    LAND_FORWARD_MOVE[0], LAND_FORWARD_MOVE[1], LAND_FORWARD_STOP[0], LAND_FORWARD_STOP[1],
    LAND_STRAIGHT_MOVE, LAND_STRAIGHT_STOP, LAND_DAMAGE, LAND_DAMAGE_ROLL,
    // climb start from the ground: wait → climbing (HumanGround block)
    super::climb::CLIMB_FROM_GROUND[0], super::climb::CLIMB_FROM_GROUND[1],
    // run stop (RE/02 §3: RunStop 0xD98E30) and its settle into the wait
    RUN_STOP[0], RUN_STOP[1], RUN_STOP_TO_WAIT[0], RUN_STOP_TO_WAIT[1],
    // Leap of Faith and the haystack (RE/04 §4.1.12)
    TAKEOFF_FAITH[0], TAKEOFF_FAITH[1], FLIGHT_FAITH, FALL_FAITH,
    super::hay::HAYSTACK_FAITH_LANDING, super::hay::HAYSTACK_WAIT, super::hay::HAYSTACK_FROM_AIR, super::hay::HAYSTACK_HOP_OUT,
    // the dives into a haystack from the ground / a free step (0xE42420 / 0xE42190)
    super::hay::HAYSTACK_DIVE[0], super::hay::HAYSTACK_DIVE[1],
    // the step off an edge backwards (event 68, 0xD9D1F0): `xx_fall_ground_back_tr_fall`
    0x935C_E89E,
    // the air catches (CheckAirCatch 0xE0BB70): the climb catch, the ≥ 3 m side free-hang catch, the edge landing
    super::air_catches::CLIMB_CATCH[0], super::air_catches::CLIMB_CATCH[1], super::air_catches::FREE_CATCH_SIDE[0], super::air_catches::FREE_CATCH_SIDE[1],
    super::air_catches::STEP_OFF[0], super::air_catches::STEP_OFF[1], super::air_catches::STEP_OFF[2], super::air_catches::STEP_OFF[3],
    super::air_catches::KNEE_CATCH[0], super::air_catches::KNEE_CATCH[1],
    // the climb's ledge grabs (TryLedgeGrab 0xDF0980): up, down, then left / right by row, [wall, free]
    super::climb::LEDGE_GRAB_UP[0], super::climb::LEDGE_GRAB_UP[1], super::climb::LEDGE_GRAB_DOWN[0], super::climb::LEDGE_GRAB_DOWN[1],
    super::climb::LEDGE_GRAB_SIDE[0][0][0], super::climb::LEDGE_GRAB_SIDE[0][0][1], super::climb::LEDGE_GRAB_SIDE[0][1][0],
    super::climb::LEDGE_GRAB_SIDE[0][1][1], super::climb::LEDGE_GRAB_SIDE[0][2][0], super::climb::LEDGE_GRAB_SIDE[0][2][1],
    super::climb::LEDGE_GRAB_SIDE[1][0][0], super::climb::LEDGE_GRAB_SIDE[1][0][1], super::climb::LEDGE_GRAB_SIDE[1][1][0],
    super::climb::LEDGE_GRAB_SIDE[1][1][1], super::climb::LEDGE_GRAB_SIDE[1][2][0], super::climb::LEDGE_GRAB_SIDE[1][2][1],
    // the kiosk table (0xE3DAE0): entries, monkey bars, their falls
    super::kiosk::KIOSK_ACTIONS[0][0], super::kiosk::KIOSK_ACTIONS[0][1], super::kiosk::KIOSK_ACTIONS[0][2], super::kiosk::KIOSK_ACTIONS[0][3],
    super::kiosk::KIOSK_ACTIONS[1][0], super::kiosk::KIOSK_ACTIONS[1][1], super::kiosk::KIOSK_ACTIONS[2][0], super::kiosk::KIOSK_ACTIONS[2][1],
    super::kiosk::SIDE_ROLL,
];

/// Run stop (HumanGround state 18, enter 0xD98E30): [left-foot item playing, right] = `xx_h_{jog,run,sprint}stop_foot{l,r}`
/// (0.2 / 0.2 / 0.4 s); then `xx_h_runstop_foot{l,r}_tr_h_wait_hipm_foot{r,l}` (0.47 s), the stop's own transition.
pub const RUN_STOP: [u32; 2] = [0x00D8_37AE, 0x00D8_37F1];
pub const RUN_STOP_TO_WAIT: [u32; 2] = [0x00D8_38BA, 0x00D8_38FD];

/// RunStop weights (0xD98E30, locomotion action playing): [jog 0, run 1 − f, sprint f], f = clamp((s − 0.75)·4).
pub fn run_stop_weights(speed_param: f32) -> [f32; 3] {
    let f = if speed_param < 0.75 { 0.0 } else { ((speed_param - 0.75) * 4.0).min(1.0) };
    [0.0, 1.0 - f, f]
}

/// Leap of Faith (0xB1EC40, target type 0x800 with the target ≥ 3 m below, jump kinds 0 / 1): takeoff
/// `freestep_footr_to_faith_jump_*` [foot 1, other], flight `faith_jump_*` (4 clips: 100 / 800 cm long ×
/// 300 / 3000 cm down), and `faith_jump_fall` (FROMPHYSICS) for the free-fall tail.
pub const TAKEOFF_FAITH: [u32; 2] = [0x23A9_49B1, 0x23A9_49B7];
pub const FLIGHT_FAITH: u32 = 0x23A9_49B2;
pub const FALL_FAITH: u32 = 0x23A9_49B5;
/// Haystack target type (HumanInAirData+0x290).
pub const TARGET_HAYSTACK: u32 = 0x800;
/// A haystack target this far below (or more) is a Leap of Faith (0xB1EC40: target z − start z ≤ −3).
pub const FAITH_MIN_DROP: f32 = 3.0;
/// Faith jump bands (0xB1EC40 case 0x800, v26 == 2): max down −30 m, near 7.5 m.
pub const FAITH_DOWN: f32 = 30.0;
pub const FAITH_NEAR: f32 = 7.5;

/// 0xB1EC40 tail for the faith jump: l = clamp(dist / 7.5), d = clamp(−dz / 27) (−dz over (−3) − (−30));
/// weights [(1−l)(1−d), l(1−d), d(1−l), d·l]. The takeoff item gets the same weights (**hypothesis**: the
/// function returns before writing the takeoff array; its item default is [0, 0, 1, 0]).
pub fn faith(dz: f32, dist: f32, foot_left: bool) -> JumpBlend {
    let l = (dist / FAITH_NEAR).clamp(0.0, 1.0);
    let d = (-dz / (FAITH_DOWN - FAITH_MIN_DROP)).clamp(0.0, 1.0);
    let w = vec![(1.0 - l) * (1.0 - d), l * (1.0 - d), d * (1.0 - l), d * l];
    JumpBlend { takeoff: TAKEOFF_FAITH[(!foot_left) as usize], flight: FLIGHT_FAITH, takeoff_w: w.clone(), flight_w: w, h: d, d: l, class: 2, down: true }
}

/// Target types (HumanInAirData+0x290) used by the port.
pub const TARGET_FREESTEP: u32 = 1;

/// Height/distance bands per target type (0xB1EC40 switch). (max up, max down, near, mid, far).
pub fn bands(target_type: u32) -> (f32, f32, f32, f32, f32) {
    match target_type {
        0x800 => (1.3, -3.0, 2.5, 5.0, 6.0),
        0x2 | 0x8000 | 0x10000 | 0x1 | 0x100 | 0x200 | 0x400 => (1.3, -3.0, 2.5, 5.0, 7.0),
        0x40 | 0x1000 | 0x2000 | 0x4000 => (2.5, -3.0, 2.5, 5.5, 7.5),
        _ => (3.0, -3.0, 2.5, 6.0, 8.0),
    }
}

/// `sub_B1EAA0` (from 0xB1EC40 0xB1FAB4, stored at InAirData+452 by 0xB20200, RE/24 AIR-6): the arrival reception's
/// weights. The free-step, pass-over, haystack and free-hang types (1, 0x10000, 0x100, 0x200, 0x400, 0x800, 0x80, 2)
/// copy the flight weights (`None`); the others (0x40, 0x1000, 0x2000, 0x4000, the swing types) get an [up, down, long]
/// triplet from the target bands: v = (up − dz) / (up − down), l = dist / (v·far), both clamped to 0..1, weights
/// [1 − v, (1 − l)·v, l·v]. `dz` = the target below / above the start, `dist` = the flat distance to the pulled-back
/// target; the swing jump (kind 3) shifts the bands by −0.7 m.
pub fn arrival_weights(target_type: u32, kind: u8, dz: f32, dist: f32) -> Option<[f32; 3]> {
    if matches!(target_type, 0x10000 | 1 | 0x100 | 0x200 | 0x400 | 0x800 | 0x80 | 2) {
        return None;
    }
    let (up, down, _, _, far) = bands(target_type);
    let o = if kind == 3 { -0.7 } else { 0.0 };
    let (up, down) = (up - o, down - o);
    let v0 = (up - dz) / (up - down);
    let l = dist / (v0 * far);
    let l = if l.is_nan() { 1.0 } else { l.clamp(0.0, 1.0) };
    let v = v0.clamp(0.0, 1.0);
    Some([1.0 - v, (1.0 - l) * v, l * v])
}

pub fn flight_action(target_type: u32, foot_left: bool) -> u32 {
    let f = (!foot_left) as usize;
    match target_type {
        0x2 => FLIGHT_PASSOVER[f],
        0x8000 => FLIGHT_ASSASSINATE[f],
        0x10000 | 0x1 | 0x100 | 0x200 | 0x400 | 0x800 => FLIGHT_FREESTEP[f],
        0x40 | 0x1000 | 0x2000 | 0x4000 => FLIGHT_SURFACE[f],
        _ => FLIGHT_SWING[f],
    }
}

/// Target types whose flight has the "deep" (down / > 3 m drop) variants (0xB1EC40 a6 flag of 0xB140B0).
fn has_deep(target_type: u32) -> bool {
    matches!(target_type, 0x10000 | 0x1 | 0x100 | 0x200 | 0x400)
}

/// Result of 0xB1EC40 for a ground running jump (kind 0: the side angle is forced to 0, so only the
/// front (`run_*`) group of the takeoff plays).
#[derive(Clone, Debug, PartialEq)]
pub struct JumpBlend {
    pub takeoff: u32,
    pub flight: u32,
    pub takeoff_w: Vec<f32>,
    pub flight_w: Vec<f32>,
    /// Height blend, distance blend, distance class (0 near, 1 up, 2 down, 3 far), going down.
    pub h: f32,
    pub d: f32,
    pub class: u8,
    pub down: bool,
}

/// `dz` = target − start height, `dist` = horizontal distance (m). `scale` = entity+0x7C
/// (hypothesis: character scale, 1 for Altaïr).
#[allow(dead_code)]
pub fn compute(dz: f32, dist: f32, target_type: u32, foot_left: bool, scale: f32) -> JumpBlend {
    compute_kind(dz, dist, target_type, foot_left, scale, 0, 0.0)
}

/// `compute` for a jump kind (0 run, 1 free step, 2 rebound, 3 swing). `angle` = the signed angle from the start
/// facing to the flat direction to the target, positive to the right (0xB1F593); kinds 0 and 4 force it to 0
/// (0xB1F54B), kind 2 clamps it to ±89.94° (0xB1FAD4). It picks and blends the takeoff's side groups (RE/24 AIR-5).
pub fn compute_kind(dz: f32, dist: f32, target_type: u32, foot_left: bool, scale: f32, kind: u8, angle: f32) -> JumpBlend {
    if target_type == TARGET_HAYSTACK && dz <= -FAITH_MIN_DROP {
        return faith(dz, dist, foot_left);
    }
    let (up, down_max, near, mid, far) = bands(target_type);
    // kinds 0, 1, 2, 4: no offset; other kinds (3, the swing jump) shift the bands by -0.7 m (0xB1EC40 v29)
    let o = if kind == 3 { -0.7 } else { 0.0 };
    let (range_up, range_down) = (up - o, down_max - o);
    let v56 = -0.5 - o;
    let v51 = dz.max(-3.0);
    let down = target_type != 2 && dz < v56;
    let range = if v56 <= v51 { range_up } else { range_down };
    let h = ((v51 - v56) / ((range - v56) * scale)).clamp(0.0, 1.0);
    let (class, d) = if dist < near {
        (0u8, dist / near)
    } else if v56 <= v51 {
        (1, ((dist - near) / ((mid - near) * (1.0 - h))).min(1.0))
    } else if dist >= mid {
        (3, ((dist - mid) / ((far - mid) * h)).min(1.0))
    } else {
        (2, (dist - near) / (mid - near))
    };
    let d = if d.is_nan() { 1.0 } else { d.clamp(0.0, 1.0) };

    let flight = flight_action(target_type, foot_left);
    let n_flight = action_items(flight).and_then(|i| i.first().map(|c| c.len())).unwrap_or(16);
    let deep = has_deep(target_type);
    let mut fw = vec![0.0f32; n_flight.max(16)];
    let takeoff = match kind {
        1 => TAKEOFF_FREESTEP[(!foot_left) as usize],
        2 => TAKEOFF_REBOUND[(!foot_left) as usize],
        3 => super::swing::TAKEOFF_SWING,
        _ => TAKEOFF_RUN[(!foot_left) as usize],
    };
    let angle = match kind {
        0 | 4 => 0.0,
        2 => angle.clamp(-1.569_796_3, 1.569_796_3),
        _ => angle,
    };
    let mut tw = vec![0.0f32; 40];
    if target_type == 2 {
        passover_flight_weights(&mut fw, d, h, class);
        passover_takeoff_weights(&mut tw, &fw, class);
        fw.truncate(n_flight);
        return JumpBlend { takeoff, flight, takeoff_w: tw, flight_w: fw, h, d, class, down };
    }
    flight_weights(&mut fw, d, h, down, class, deep);
    takeoff_weights(&mut tw, &fw, down, angle, class, deep, kind);
    if kind == 3 {
        // the swing takeoff is one 8-clip group (0xB1EC40 kind 3 maps both side groups onto the front one)
        tw.truncate(8);
    }
    // > 3 m drop: move weight into the deep flight variants (0xB1EC40 tail, after the takeoff used them)
    if deep && dz < -3.0 {
        let k = ((-dz - 3.0) / 5.0).clamp(0.0, 1.0);
        fw[12] = (fw[9] + fw[7] + fw[3]) * k;
        fw[13] = (fw[10] + fw[8] + fw[4]) * k;
        fw[14] = (fw[11] + fw[5]) * k;
        fw[15] = fw[6] * k;
        for i in [7, 9, 3, 8, 10, 4, 11, 5, 6] {
            fw[i] *= 1.0 - k;
        }
    }
    fw.truncate(n_flight);
    JumpBlend { takeoff, flight, takeoff_w: tw, flight_w: fw, h, d, class, down }
}

/// `Human__ComputeJumpFlightWeightsPassOver` 0xB142A0: the 5-clip pass-over flight [front 050/300/550, up 050/300].
pub fn passover_flight_weights(w: &mut [f32], d: f32, h: f32, class: u8) {
    if class == 0 {
        w[0] = (1.0 - d) * (1.0 - h);
        w[1] = d * (1.0 - h);
        w[3] = (1.0 - d) * h;
        w[4] = d * h;
    } else {
        w[1] = (1.0 - d) * (1.0 - h);
        w[2] = d * (1.0 - h);
        w[4] = h;
    }
}

/// 0xB13E20 for a pass-over target with the side angle 0 (jump kinds 0 / 4): the front takeoff group's slots
/// (0xB1EC40 picks 0, 1, up 6 / 7 for the near class; 1, 2, up 7 for class 1; 1, 2, down 4 / 5 otherwise).
pub fn passover_takeoff_weights(t: &mut [f32], f: &[f32], class: u8) {
    t.iter_mut().for_each(|x| *x = 0.0);
    match class {
        0 => {
            t[0] = f[0];
            t[1] = f[1];
            t[6] = f[3];
            t[7] = f[4];
        }
        1 => {
            t[1] = f[1];
            t[2] = f[2];
            t[7] = f[4];
        }
        _ => {
            t[1] = f[1];
            t[2] = f[2];
            t[4] = f[4];
            t[5] = f[4];
        }
    }
}

/// 0xB140B0: flight weights from distance blend `d`, height blend `h`, class and going-down flag.
/// Flight slots: 0–2 front 050/300/550, 3–6 down 050/300/550/800, 7–8 up 050/300,
/// 9–11 front …_down, 12–15 down …_deep.
pub fn flight_weights(w: &mut [f32], d: f32, h: f32, down: bool, class: u8, deep: bool) {
    let (s0, s1, s2) = if deep && down { (9, 10, 11) } else { (0, 1, 2) };
    match class {
        0 => {
            w[s0] = (1.0 - h) * (1.0 - d);
            w[s1] = d * (1.0 - h);
            if down {
                w[3] = h * (1.0 - d);
                w[4] = d * h;
            } else {
                w[7] = h * (1.0 - d);
                w[8] = d * h;
            }
        }
        1 => {
            w[1] = (1.0 - d) * (1.0 - h);
            w[2] = d * (1.0 - h);
            w[8] = h;
        }
        2 => {
            w[s1] = (1.0 - h) * (1.0 - d);
            w[s2] = d * (1.0 - h);
            w[4] = h * (1.0 - d);
            w[5] = d * h;
        }
        _ => {
            w[5] = (1.0 - d) * h;
            w[6] = d * h;
            w[s2] = 1.0 - h;
        }
    }
}

/// 0xB1EC40's takeoff slots (0xB1FB09–0xB1FDB4): groups A and B of four slots by the distance class, the side angle's
/// quadrant and the down flag; the 40-clip takeoffs hold five 8-clip groups (front, left, right, back-left, back-right;
/// each front 050 / 300 / 550, down 050 / 300 / 550, up 050 / 300). Kind 2 (the 32-clip rebound takeoff: front-left,
/// front-right, left, right) shifts B by 8, and A too for an angle ≥ 0; kind 3 (the swing) copies A into B.
pub fn takeoff_slots(class: u8, down: bool, angle: f32, kind: u8) -> ([usize; 4], [usize; 4]) {
    use std::f32::consts::FRAC_PI_2;
    let (mut a, mut b): ([usize; 4], [usize; 4]) = match class {
        0 => {
            let (lo, hi) = if down { (3, 4) } else { (6, 7) };
            let g = |base: usize| [base, base + 1, base + lo, base + hi];
            if angle >= 0.0 {
                if angle >= FRAC_PI_2 { (g(16), g(32)) } else { (g(0), g(16)) }
            } else if angle <= -FRAC_PI_2 {
                (g(8), g(24))
            } else {
                (g(0), g(8))
            }
        }
        1 => {
            let g = |base: usize| [base + 1, base + 2, base + 7, base + 7];
            if angle < 0.0 {
                if angle <= -FRAC_PI_2 { (g(8), g(24)) } else { (g(0), g(8)) }
            } else if angle < FRAC_PI_2 {
                (g(0), g(16))
            } else {
                (g(16), g(32))
            }
        }
        _ => {
            let g = |base: usize| [base + 1, base + 2, base + 4, base + 5];
            if angle < 0.0 {
                if angle <= -FRAC_PI_2 { (g(8), g(24)) } else { (g(0), g(8)) }
            } else if angle < FRAC_PI_2 {
                (g(0), g(16))
            } else {
                (g(16), g(32))
            }
        }
    };
    match kind {
        2 => {
            b.iter_mut().for_each(|s| *s += 8);
            if angle >= 0.0 {
                a.iter_mut().for_each(|s| *s += 8);
            }
        }
        3 => b = a,
        _ => {}
    }
    (a, b)
}

/// `Human__ComputeJumpTakeoffWeights` 0xB13A60: the takeoff mirrors the flight's height / distance blend on its own
/// clip family, split between groups A and B (`takeoff_slots`) by t = (|angle| mod 90°) / 90°. A's slots are set, B's
/// added to (so a kind-3 takeoff, B = A, sums back). Class 0: (f0, f1, then down f3 / f4 or up f7 / f8), with f9 / f10
/// for f0 / f1 when deep and down; other classes: (f1, f2 + f6, then class 1 f8 twice, else f4 / f5). RE/24 AIR-5.
pub fn takeoff_weights(w: &mut [f32], f: &[f32], down: bool, angle: f32, class: u8, deep: bool, kind: u8) {
    w.iter_mut().for_each(|x| *x = 0.0);
    let mut a = angle.abs();
    if a >= std::f32::consts::FRAC_PI_2 {
        a -= std::f32::consts::FRAC_PI_2;
    }
    let t = a / std::f32::consts::FRAC_PI_2;
    let (sa, sb) = takeoff_slots(class, down, angle, kind);
    let (v18, v19, v20) = if deep && down { (9, 10, 11) } else { (0, 1, 2) };
    let vals: [f32; 4] = if class != 0 {
        if class == 1 { [f[v19], f[v20] + f[6], f[8], f[8]] } else { [f[v19], f[v20] + f[6], f[4], f[5]] }
    } else if down {
        [f[v18], f[v19], f[3], f[4]]
    } else {
        [f[v18], f[v19], f[7], f[8]]
    };
    for k in 0..4 {
        // class 1 copies slot 2 into slot 3 (the same clip): written once
        if class == 1 && k == 3 {
            break;
        }
        if sa[k] < w.len() {
            w[sa[k]] = vals[k] * (1.0 - t);
        }
        if sb[k] < w.len() {
            w[sb[k]] += vals[k] * t;
        }
    }
}

// ---------------------------------------------------------------- landings (0xE05940)

/// Speed-ratio bucket (HumanInAir+0x16C): < 0.2 → 0, < 0.5 → 1, < 0.9 → 2, else 3.
pub fn speed_bucket(ratio: f32) -> usize {
    if ratio < 0.2 {
        0
    } else if ratio < 0.5 {
        1
    } else if ratio < 0.9 {
        2
    } else {
        3
    }
}

/// Landing action and weights for a ground contact. `drop` = jump/fall start height − landing height,
/// `horiz` = horizontal distance from the start, `stick_forward` = the wanted move is non-zero and within
/// 75° of the motion, `speed_ratio` = +0x16C, `foot_left` = sub_B18850 == 1.
pub fn landing(drop: f32, horiz: f32, stick_forward: bool, speed_ratio: f32, foot_left: bool) -> (u32, Vec<f32>) {
    if drop > 3.0 {
        return (if speed_ratio <= 0.2 { LAND_DAMAGE } else { LAND_DAMAGE_ROLL }, vec![1.0]);
    }
    let mut b = if drop < 2.5 { drop / 2.5 } else { 1.0 };
    if horiz > 5.0 {
        b = b.max(((horiz - 5.0) / 7.5).min(1.0));
    }
    let b = b.clamp(0.0, 1.0);
    let bucket = speed_bucket(speed_ratio);
    let f = (!foot_left) as usize;
    let id = match (stick_forward, bucket) {
        (false, 0) => LAND_STRAIGHT_STOP,
        (false, _) => LAND_STRAIGHT_MOVE,
        (true, 0) => LAND_FORWARD_STOP[f],
        (true, _) => LAND_FORWARD_MOVE[f],
    };
    let w = if bucket == 0 {
        vec![1.0 - b, b]
    } else {
        let mut w = vec![0.0; 6];
        w[bucket - 1] = 1.0 - b;
        w[bucket + 2] = b;
        w
    };
    (id, w)
}

/// Free-step reception weights: the flight's first 12 weights, split normal / `_fast` by HumanInAir+0x16C
/// (0xE07D00, defaults to 0 → normal only).
pub fn reception_weights(flight_w: &[f32], fast: f32) -> Vec<f32> {
    let mut w = vec![0.0; 24];
    for i in 0..12.min(flight_w.len()) {
        w[i] = (1.0 - fast) * flight_w[i];
        w[i + 12] = flight_w[i] * fast;
    }
    w
}

// ---------------------------------------------------------------- clip data

/// One item of a graph action with runtime weights (what the exe passes to `sub_502E30`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActionBlend {
    pub id: u32,
    pub item: usize,
    pub w: [f32; 40],
    pub n: usize,
}

/// The item evaluator's weight setup (`AnimItem__NormalizeWeights` 0x777AE0), run whenever an item's weights are set
/// (`AnimGraph__SetItemBlendWeights` 0x503040 → 0x777380 dirty byte): over the weights above 0.0005, if Σ(i ≥ 1) ≤ 1
/// and the remainder is above 0.0005, clip 0 gets the remainder 1 − Σ(i ≥ 1) (whatever it held); otherwise every such
/// weight is divided by Σ(i ≥ 1) + w0. The duration, pose and root displacement use the result (RE/24 ANIM-1). Weights
/// at or below 0.0005 are left out of the evaluator's list (`dword_1A13528`), i.e. they count as 0.
pub fn normalize_item_weights(w: &mut [f32]) {
    const ACTIVE: f32 = 0.000_5;
    if w.is_empty() {
        return;
    }
    for x in w.iter_mut().filter(|x| **x <= ACTIVE) {
        *x = 0.0;
    }
    let rest: f32 = w[1..].iter().sum();
    if rest <= 1.0 {
        if 1.0 - rest > ACTIVE {
            w[0] = 1.0 - rest;
        }
    } else {
        let total = rest + w[0];
        for x in w.iter_mut().filter(|x| **x > ACTIVE) {
            *x /= total;
        }
    }
}

impl ActionBlend {
    pub fn new(id: u32, item: usize, weights: &[f32]) -> Self {
        let mut w = [0.0; 40];
        let n = weights.len().min(40);
        w[..n].copy_from_slice(&weights[..n]);
        normalize_item_weights(&mut w[..n]);
        ActionBlend { id, item, w, n }
    }
    pub fn clips(&self) -> &'static [&'static str] {
        action_items(self.id).and_then(|i| i.get(self.item).copied()).unwrap_or(&[])
    }
    pub fn weights(&self) -> &[f32] {
        &self.w[..self.n]
    }
    /// Σ wᵢ·Tᵢ (0x5B9480).
    pub fn duration(&self) -> f32 {
        item_duration(self.clips(), self.weights())
    }
    /// Blended root displacement at `phase` (animation space: x right, y forward, z up).
    pub fn disp(&self, phase: f32) -> [f32; 3] {
        item_disp(self.clips(), self.weights(), phase)
    }
    /// Blended root yaw at `phase` (radians, + = left): the DISPLACEMENT track's rotation, which turns the character
    /// when the item is FROMANIM (the pivots, the lean exits …).
    pub fn yaw(&self, phase: f32) -> f32 {
        let x = phase.clamp(0.0, 1.0) * 8.0;
        let i = (x.floor() as usize).min(7);
        let s = x - i as f32;
        self.clips()
            .iter()
            .zip(self.weights())
            .filter(|(_, w)| **w > 0.0)
            .filter_map(|(n, w)| clip(n).map(|c| (c.yaw[i] + (c.yaw[i + 1] - c.yaw[i]) * s) * w))
            .sum()
    }
}

pub fn action_items(id: u32) -> Option<&'static [&'static [&'static str]]> {
    ACTIONS.iter().find(|a| a.0 == id).map(|a| a.1)
}

pub fn clip(name: &str) -> Option<&'static ClipRoot> {
    CLIPS.iter().find(|c| c.name == name)
}

/// Item duration Σ wᵢ·Tᵢ (0x5B9480).
pub fn item_duration(clips: &[&str], w: &[f32]) -> f32 {
    clips.iter().zip(w).filter_map(|(n, w)| clip(n).map(|c| c.duration * w)).sum()
}

fn sample(c: &ClipRoot, phase: f32) -> [f32; 3] {
    let x = phase.clamp(0.0, 1.0) * 8.0;
    let i = (x.floor() as usize).min(7);
    let s = x - i as f32;
    let (a, b) = (c.disp[i], c.disp[i + 1]);
    [a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s, a[2] + (b[2] - a[2]) * s]
}

/// Blended displacement Σ wᵢ·dispᵢ(phase), animation space (x right, y forward, z up).
pub fn item_disp(clips: &[&str], w: &[f32], phase: f32) -> [f32; 3] {
    let mut o = [0.0f32; 3];
    for (n, w) in clips.iter().zip(w) {
        if *w <= 0.0 {
            continue;
        }
        if let Some(c) = clip(n) {
            let d = sample(c, phase);
            for k in 0..3 {
                o[k] += d[k] * w;
            }
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RE/24 AIR-5: 0xB1EC40's takeoff groups. A run jump (angle 0) blends front and right, a side angle to the left
    /// picks front and left, beyond 90° the back groups; the rebound (kind 2) shifts to its front-right / right groups for
    /// a target to the right and keeps front-left / left (B + 8) to the left; the swing (kind 3) copies A into B.
    #[test]
    fn the_takeoff_groups_follow_the_side_angle_and_the_kind() {
        assert_eq!(takeoff_slots(0, false, 0.0, 0), ([0, 1, 6, 7], [16, 17, 22, 23]));
        assert_eq!(takeoff_slots(0, true, -0.5, 1), ([0, 1, 3, 4], [8, 9, 11, 12]));
        assert_eq!(takeoff_slots(2, false, 2.0, 1), ([17, 18, 20, 21], [33, 34, 36, 37]));
        assert_eq!(takeoff_slots(1, false, -2.0, 1), ([9, 10, 15, 15], [25, 26, 31, 31]));
        assert_eq!(takeoff_slots(0, false, 0.5, 2), ([8, 9, 14, 15], [24, 25, 30, 31]));
        assert_eq!(takeoff_slots(0, false, -0.5, 2), ([0, 1, 6, 7], [16, 17, 22, 23]));
        assert_eq!(takeoff_slots(2, false, 0.5, 3), ([1, 2, 4, 5], [1, 2, 4, 5]));
        // the weights split by t = |angle| / 90°: 30° to the right puts a third on the right group
        let f = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut w = vec![0.0f32; 40];
        takeoff_weights(&mut w, &f, false, 30f32.to_radians(), 0, false, 1);
        assert!((w[1] - 2.0 / 3.0).abs() < 1e-5 && (w[17] - 1.0 / 3.0).abs() < 1e-5, "{w:?}");
        // kind 3 (B = A) sums back to the flight weight
        takeoff_weights(&mut w, &f, false, 30f32.to_radians(), 0, false, 3);
        assert!((w[1] - 1.0).abs() < 1e-5);
    }

    /// RE/24 AIR-6: the ladder arrival's triplet: a ladder 2 m below and 4 m away (after the pull-back) blends
    /// v = (2.5 + 2) / 5.5, l = 4 / (v · 7.5); free-step targets keep the flight's weights.
    #[test]
    fn the_ladder_arrival_weighs_up_down_and_long() {
        let w = arrival_weights(0x1000, 0, -2.0, 4.0).unwrap();
        let v = 4.5f32 / 5.5;
        let l = 4.0 / (v * 7.5);
        for (a, b) in w.iter().zip([1.0 - v, (1.0 - l) * v, l * v]) {
            assert!((a - b).abs() < 1e-5, "{w:?}");
        }
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(arrival_weights(1, 0, -2.0, 4.0).is_none());
        assert!(arrival_weights(0x80, 0, -2.0, 4.0).is_none());
    }

    #[test]
    fn flight_weights_sum_to_one() {
        for &(dz, dist) in &[(0.0, 1.0), (0.0, 4.0), (1.0, 3.0), (-1.0, 3.0), (-2.0, 6.0), (-4.0, 6.5), (-0.6, 2.0)] {
            let b = compute(dz, dist, TARGET_FREESTEP, true, 1.0);
            let s: f32 = b.flight_w.iter().sum();
            assert!((s - 1.0).abs() < 1e-4, "dz {dz} dist {dist}: {s} {:?}", b);
            let st: f32 = b.takeoff_w.iter().sum();
            assert!((st - 1.0).abs() < 1e-4, "takeoff sum {st}");
        }
    }

    #[test]
    fn every_target_type_has_a_flight_with_clips() {
        // bugs 1791501015 / 1791501115: the hang-target flights were missing from the clip table, so the jump to a
        // ledge lasted only its takeoff (0.2 s over 4 m) and played no flight clip
        for ty in [TARGET_FREESTEP, 0x40, 0x80, 0x1000, 2] {
            for foot_left in [true, false] {
                for &(dz, dist) in &[(0.7, 2.6), (0.7, 4.3), (2.3, 1.6), (0.0, 3.0)] {
                    let b = compute(dz, dist, ty, foot_left, 1.0);
                    let d = ActionBlend::new(b.flight, 0, &b.flight_w).duration();
                    assert!(action_items(b.flight).is_some(), "flight {:#x} of type {ty:#x} not in the clip table", b.flight);
                    assert!(d > 0.1, "type {ty:#x} dz {dz} dist {dist}: flight {:#x} lasts {d}", b.flight);
                }
            }
        }
    }

    #[test]
    fn classes_follow_the_bands() {
        assert_eq!(compute(0.0, 2.0, TARGET_FREESTEP, true, 1.0).class, 0);
        assert_eq!(compute(0.0, 3.5, TARGET_FREESTEP, true, 1.0).class, 1);
        assert_eq!(compute(-1.5, 3.5, TARGET_FREESTEP, true, 1.0).class, 2);
        assert_eq!(compute(-1.5, 6.0, TARGET_FREESTEP, true, 1.0).class, 3);
        // flat 3.5 m gap: height blend 0.5/1.8, distance (3.5−2.5)/(2.5·(1−h))
        let b = compute(0.0, 3.5, TARGET_FREESTEP, true, 1.0);
        assert!((b.h - 0.5 / 1.8).abs() < 1e-5);
        assert!((b.d - 1.0 / (2.5 * (1.0 - 0.5 / 1.8))).abs() < 1e-5);
    }

    #[test]
    fn landing_choice() {
        assert_eq!(landing(4.0, 1.0, true, 0.1, true).0, LAND_DAMAGE);
        assert_eq!(landing(4.0, 1.0, true, 0.6, true).0, LAND_DAMAGE_ROLL);
        let (id, w) = landing(1.25, 1.0, true, 0.6, true);
        assert_eq!(id, LAND_FORWARD_MOVE[0]);
        assert_eq!(w, vec![0.0, 0.5, 0.0, 0.0, 0.5, 0.0]);
        let (id, w) = landing(0.0, 0.0, false, 0.0, false);
        assert_eq!((id, w), (LAND_STRAIGHT_STOP, vec![1.0, 0.0]));
    }
    #[test]
    fn landing_roll_and_speed_bucket_boundaries_are_strict() {
        // 0xE05940: drop >3; damage instead of roll at ratio <=0.2.
        assert_ne!(landing(3.0,0.0,false,1.0,true).0,LAND_DAMAGE_ROLL);
        assert_eq!(landing(3.0f32.next_up(),0.0,false,0.2,true).0,LAND_DAMAGE);
        assert_eq!(landing(3.0f32.next_up(),0.0,false,0.2f32.next_up(),true).0,LAND_DAMAGE_ROLL);
        for (ratio,bucket) in [(0.2f32.next_down(),0),(0.2,1),(0.5f32.next_down(),1),(0.5,2),(0.9f32.next_down(),2),(0.9,3)] {
            assert_eq!(speed_bucket(ratio),bucket);
        }
        assert_eq!(landing(2.5,0.0,true,0.0,true).1,vec![0.0,1.0]);
        assert_eq!(landing(0.0,5.0,true,0.0,true).1,vec![1.0,0.0]);
        assert_eq!(landing(0.0,12.5,true,0.0,true).1,vec![0.0,1.0]);
    }

}

#[cfg(test)]
mod normalize_tests {
    use super::normalize_item_weights;
    fn n(w: &[f32]) -> Vec<f32> {
        let mut v = w.to_vec();
        normalize_item_weights(&mut v);
        v
    }
    fn eq(a: Vec<f32>, b: &[f32]) {
        assert!(a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6), "{a:?} vs {b:?}");
    }
    #[test]
    fn clip_0_takes_the_remainder_or_all_are_divided() {
        // 0x777AE0: Σ(i ≥ 1) ≤ 1 → w0 = 1 − Σ(i ≥ 1), whatever w0 held
        eq(n(&[0.0, 0.3, 0.3]), &[0.4, 0.3, 0.3]);
        eq(n(&[0.7, 0.7]), &[0.3, 0.7]);
        eq(n(&[1.0]), &[1.0]);
        eq(n(&[0.0]), &[1.0]);
        // a remainder of 0.0005 or less leaves w0 alone
        eq(n(&[0.0, 0.9998]), &[0.0, 0.9998]);
        // Σ(i ≥ 1) > 1 → all divided by Σ(i ≥ 1) + w0
        let v = n(&[0.5, 0.8, 0.6]);
        assert!((v[0] - 0.5 / 1.9).abs() < 1e-6 && (v[1] - 0.8 / 1.9).abs() < 1e-6 && (v[2] - 0.6 / 1.9).abs() < 1e-6);
        // weights of 0.0005 or less are dropped
        eq(n(&[0.0, 0.0004, 0.5]), &[0.5, 0.0, 0.5]);
    }
}
