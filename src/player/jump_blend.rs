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
/// Flights by target type, [footl, footr].
pub const FLIGHT_FREESTEP: [u32; 2] = [0x010D_DAFA, 0x010D_F0D8]; // 1, 0x10000, 0x100, 0x200, 0x400, haystack
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
    TAKEOFF_RUN[0], TAKEOFF_RUN[1], TAKEOFF_FREESTEP[0], TAKEOFF_FREESTEP[1],
    FLIGHT_FREESTEP[0], FLIGHT_FREESTEP[1],
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
    compute_kind(dz, dist, target_type, foot_left, scale, 0)
}

/// `compute` for a jump kind (0 run, 1 free step): only the takeoff action differs for kinds 0 / 1 (both
/// force the side angle to 0 and use the same band offset `o`, 0xB1EC40).
pub fn compute_kind(dz: f32, dist: f32, target_type: u32, foot_left: bool, scale: f32, kind: u8) -> JumpBlend {
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
        3 => super::swing::TAKEOFF_SWING,
        _ => TAKEOFF_RUN[(!foot_left) as usize],
    };
    let mut tw = vec![0.0f32; 40];
    if target_type == 2 {
        passover_flight_weights(&mut fw, d, h, class);
        passover_takeoff_weights(&mut tw, &fw, class);
        fw.truncate(n_flight);
        return JumpBlend { takeoff, flight, takeoff_w: tw, flight_w: fw, h, d, class, down };
    }
    flight_weights(&mut fw, d, h, down, class, deep);
    takeoff_weights(&mut tw, &fw, down, 0.0, class, deep);
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

/// 0xB13A60 for the front/right quadrant pair (angle ∈ [0, π/2)): the takeoff mirrors the flight's
/// height/distance blend on its own 8-clip family, split between the front group (slots 0–7) and the
/// right group (16–23) by the side angle. Group slots: 0–2 front 050/300/550, 3–5 down, 6–7 up.
pub fn takeoff_weights(w: &mut [f32], f: &[f32], down: bool, angle: f32, class: u8, deep: bool) {
    w.iter_mut().for_each(|x| *x = 0.0);
    let mut a = angle.abs();
    if a >= std::f32::consts::FRAC_PI_2 {
        a -= std::f32::consts::FRAC_PI_2;
    }
    let t = a / std::f32::consts::FRAC_PI_2;
    let (ga, gb) = (0usize, 16usize); // front, right (port: kind 0 only)
    let (v18, v19, v20) = if deep && down { (9, 10, 11) } else { (0, 1, 2) };
    let mut put = |slot: usize, v: f32| {
        w[ga + slot] = v * (1.0 - t);
        w[gb + slot] += v * t;
    };
    if class != 0 {
        // A = (front300, front550, up300/up300 or down300/down550)
        put(1, f[v19]);
        put(2, f[v20] + f[6]);
        if class == 1 {
            put(7, f[8]);
        } else {
            put(4, f[4]);
            put(5, f[5]);
        }
    } else {
        put(0, f[v18]);
        put(1, f[v19]);
        if down {
            put(3, f[3]);
            put(4, f[4]);
        } else {
            put(6, f[7]);
            put(7, f[8]);
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

impl ActionBlend {
    pub fn new(id: u32, item: usize, weights: &[f32]) -> Self {
        let mut w = [0.0; 40];
        let n = weights.len().min(40);
        w[..n].copy_from_slice(&weights[..n]);
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
}
