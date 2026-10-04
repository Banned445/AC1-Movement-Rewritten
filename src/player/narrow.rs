//! HumanNarrowObject (context 12): beams and pilotis (RE/05 §2).
//!
//! Entry from Ground: Movement event 72 (guard 0xD9F4C0 → fill 0xD84A40 → context 12). The beam class state
//! (`HumanNarrowObjectBeam`, 0xF6E000–0xF81000) aligns to the entry point (`AlignToBeamEntry` 0xF7EBA0), then
//! in Main moves by the beam actions' **root motion projected onto the beam line**
//! (`ConstrainRootMotionToBeam` 0xF7C3A0): it stops 0.3 m before an end, and steps off an end that has free
//! space beyond it (`CanStepOffBeamEnd` 0xF77C00: within 0.16 m of the end, free capsule at end + 0.5·dir).
//! Stick classification vs the facing (`ClassifyStickDir` 0xF76150): |a| < 75° forward, > 135° back (turn).
//!
//! From the air (RE/05 §2.8): a free-step jump that arrives on a beam mounts it with the entry mode picked by
//! `TryMountBeam` 0xE52AD0; on a pilotis it plays the free-step → pilotis entry (`TryPilotisFreeStep`
//! 0xE50190). Falling onto either is caught by `CheckAirCatch` 0xE0BB70 (pilotis 0xB2B600, beam 0xE0B890).
//! Jumps: the impulsion crouch (state +145, 0xF738A0), the jump on the spot (+148, 0xF717D0 → 0xF73B80) and
//! free-step jumps to a target (kind 1, 0xE4D950 → `Human__SetupJumpToTarget`).

use bevy::prelude::*;

use super::air::InAirEntry;
use super::jump_blend::{self, ActionBlend};
use super::targets::{self, JumpTarget};
use super::{switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceSubType, GuidanceWorld};
use crate::input::PadInput;

/// HumanNarrowObject block actions (by clip name).
pub const BEAM_WAIT: [u32; 2] = [0x28A3_A8E2, 0x28A3_A8E3]; // xx_l_beam_crouchwait_foot{l,r}
/// Two items (foot l, foot r), each [crouchwalk, crouchjog].
pub const BEAM_WALK: u32 = 0x28A3_A8E4;
pub const BEAM_START: [u32; 2] = [0x3466_2CB4, 0x3466_2CB5]; // crouchwait_foot{l,r}_tr_crouch{walk,jog}
pub const BEAM_JOG_STOP: [u32; 2] = [0x3466_339D, 0x3466_339E]; // crouchjog_stop_foot{l,r}
pub const BEAM_TURN180: [u32; 2] = [0x28A3_A8EE, 0x28A3_A8EF]; // crouchwait_foot{l,r}_turn180
/// Side entries from a free-step arrival (modes 4 / 5, 0xF7AAA0): items swapped into the walk; each 4 clips
/// [crouchwalk 30°, crouchwalk 90°, crouchjog 30°, crouchjog 90°]. Index: mode 5 / foot r, mode 4 / foot r,
/// mode 5 / foot l, mode 4 / foot l (`xx_h_freestep_entry_foot{l,r}_tr_crouch{walk,jog}_foot{r,l}_{left,right}_{30,90}`).
pub const BEAM_SIDE_ENTRY: [u32; 4] = [0xE6E0_E9B8, 0xE6E0_E9B9, 0xE6E0_E9BA, 0xE6E0_E9BB];
/// Impulsion (state +145, enter 0xF738A0): the transition `crouchwait_foot{l,r}_tr_impultionstraight` (items a, b;
/// table 0x1A35420), from a pilotis `beam_pilotis_tr_impultionstraight_a`, then the `impultionstraight_wait`.
pub const BEAM_TO_IMPULSE: [u32; 2] = [0x516D_4D0A, 0x516D_4D0B];
pub const PILOTIS_TO_IMPULSE: u32 = 0x5288_D630;
pub const BEAM_IMPULSE_WAIT: u32 = 0x5288_EF5C;
/// Jump on the spot (state +148, 0xF717D0): `impultionstraight_to_jumpstraight` with a hand target, else
/// `impultionstraight_tr_jumpstraight_clear`; ActorState 25.
pub const BEAM_IMPULSE_TO_JUMP: u32 = 0x516D_521A;
pub const BEAM_IMPULSE_TO_CLEAR: u32 = 0x516D_52EB;
/// No hand target (0xF73B80): InAir plays `beam_jumpstraight_clear`, then `…_clear_tr_fall` (InAir +416).
pub const BEAM_JUMP_CLEAR: u32 = 0x516D_52E7;
pub const BEAM_JUMP_CLEAR_FALL: u32 = 0x516D_52E8;
/// Pilotis: free-step entry by foot (0xE50190), the soft landing from the air (InAir block, items a / b,
/// 0xE0BB70), and the wait (3 clips [wait, left, right], 0xE4CFA0 / 0xE51960).
pub const PILOTIS_FROM_FREESTEP: [u32; 2] = [0x3919_3BBF, 0x3919_3BC0];
pub const PILOTIS_FROM_AIR: u32 = 0x2F3E_9E0C;
pub const PILOTIS_WAIT: u32 = 0x388E_97DA;
/// Beam catch from a fall (0xE0B890 → BeamReception, mode 7): `xx_h_landing_damage_footl`.
pub const BEAM_LANDING: u32 = jump_blend::LAND_DAMAGE;

pub const DUMPED_ACTIONS: &[u32] = &[
    BEAM_WAIT[0], BEAM_WAIT[1], BEAM_WALK, BEAM_START[0], BEAM_START[1], BEAM_JOG_STOP[0], BEAM_JOG_STOP[1], BEAM_TURN180[0], BEAM_TURN180[1],
    BEAM_SIDE_ENTRY[0], BEAM_SIDE_ENTRY[1], BEAM_SIDE_ENTRY[2], BEAM_SIDE_ENTRY[3],
    BEAM_TO_IMPULSE[0], BEAM_TO_IMPULSE[1], PILOTIS_TO_IMPULSE, BEAM_IMPULSE_WAIT, BEAM_IMPULSE_TO_JUMP, BEAM_IMPULSE_TO_CLEAR,
    BEAM_JUMP_CLEAR, BEAM_JUMP_CLEAR_FALL, PILOTIS_FROM_FREESTEP[0], PILOTIS_FROM_FREESTEP[1], PILOTIS_FROM_AIR, PILOTIS_WAIT,
    0x516D_52DB, 0x516D_52DC, 0x516D_52DD, 0x516D_52DE, 0x516D_52DF,
];

/// 0xF7C3A0: the root stops this far before a beam end.
pub const BEAM_END_STOP: f32 = 0.3;
/// 0xF77C00: step off within this distance of the end.
pub const BEAM_STEP_OFF: f32 = 0.16;
/// Entry alignment time (PORT: `AlignToBeamEntry` runs until the entry reception is done, not timed).
const ENTRY_TIME: f32 = 0.25;
/// Catch / pilotis receptions interpolate the root over 0.2 s (0xE0BB70 → sub_711130).
const CATCH_WARP: f32 = 0.2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NarrowKind {
    #[default]
    Beam,
    Pilotis,
}

/// `HumanNarrowObjectBeam` entry mode (+0x14), set by the writer before context 12 (0xE528C0 / 0xE52AD0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BeamEntryMode {
    /// 1: mount from Ground (event 72): align to the entry point.
    #[default]
    Ground = 1,
    /// 2: free-step arrival along the beam (|facing · axis| ≥ 0.866).
    Straight = 2,
    /// 3: free-step arrival across it.
    Side = 3,
    /// 4 / 5: arrival across it with the stick along it: walk away through the side-entry clips.
    SideWalk = 4,
    SideWalkBack = 5,
    /// 7: caught falling onto it (BeamReception, 0xE0B890).
    Reception = 7,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BeamState {
    #[default]
    Entry,
    /// Modes 3 / 4 / 5 / 7: the entry action plays (root warped onto the beam), then Main.
    Reception,
    Wait,
    Start,
    Walk,
    Stop,
    Turn,
    /// +145: the transition into the impulsion crouch, then its wait.
    ImpulseIn,
    ImpulseWait,
    /// +148: the jump on the spot (then InAir).
    JumpOnPlace,
    /// Pilotis (inner state 3): sub 4 / 5 entries, sub 6 wait.
    PilotisIn,
    PilotisWait,
}

#[derive(Clone, Copy, Debug)]
pub struct BeamEntry {
    pub p0: Vec3,
    pub p1: Vec3,
    /// Entry point on the beam line.
    pub point: Vec3,
    pub from: Vec3,
    /// Walking toward p1 (true) or p0.
    pub toward_p1: bool,
    pub mode: BeamEntryMode,
    /// The leading foot of the arriving jump (0 left, 1 right).
    pub foot: usize,
    /// Mode 3: the free-step reception the arrival plays; modes 4 / 5: the side entry blend.
    pub action: Option<ActionBlend>,
    /// Character facing on arrival (modes 3 / 7 turn from it onto the beam).
    pub facing: Vec3,
}

/// `PilotisEntryType` (NarrowObjectData+0xCC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PilotisEntryType {
    FromFreeStep = 0,
    FromInAir = 1,
}

#[derive(Clone, Copy, Debug)]
pub struct PilotisEntry {
    /// Centre of the post top.
    pub top: Vec3,
    pub from: Vec3,
    pub facing: Vec3,
    pub kind: PilotisEntryType,
    pub foot: usize,
}

#[derive(Debug, Default)]
pub struct HumanNarrowObjectData {
    pub kind: NarrowKind,
    pub p0: Vec3,
    pub p1: Vec3,
    /// Distance along the beam from p0.
    pub s: f32,
    pub toward_p1: bool,
    pub state: BeamState,
    pub action: Option<ActionBlend>,
    pub t: f32,
    /// Leading foot of the next walk item (0 left, 1 right).
    pub foot: usize,
    pub entry_from: Vec3,
    pub entry_mode: BeamEntryMode,
    pub seq: u32,
    /// Root displacement already applied for the playing action (forward, animation space).
    applied: f32,
    /// Pilotis: top centre and facing. Entry / reception warps: from → to over `warp` s.
    pub top: Vec3,
    pub pilotis_facing: Vec3,
    warp_from: Vec3,
    warp_heading: (f32, f32),
    warp: f32,
    /// Pilotis wait lean (NarrowObject+372): −1 left … 1 right.
    pub lean: f32,
    /// Hand target found for the jump on the spot (beam +1840 ≠ 0x80000000).
    pub hand_target: Option<JumpTarget>,
}

fn item(id: u32, item: usize, w: &[f32]) -> Option<ActionBlend> {
    let n = jump_blend::action_items(id)?.get(item)?.len();
    let mut v = vec![0.0; n];
    for (k, x) in w.iter().enumerate().take(n) {
        v[k] = *x;
    }
    if w.is_empty() {
        v[0] = 1.0;
    }
    Some(ActionBlend::new(id, item, &v))
}

impl HumanNarrowObjectData {
    pub fn enter(&mut self, e: BeamEntry) {
        self.kind = NarrowKind::Beam;
        self.p0 = e.p0;
        self.p1 = e.p1;
        self.s = (e.point - e.p0).dot(self.dir());
        self.toward_p1 = e.toward_p1;
        self.entry_from = e.from;
        self.entry_mode = e.mode;
        self.foot = e.foot;
        self.hand_target = None;
        self.warp_from = e.from;
        self.warp_heading = (super::heading_of(e.facing), super::heading_of(self.facing()));
        match e.mode {
            BeamEntryMode::Ground => self.play(BeamState::Entry, item(BEAM_WAIT[0], 0, &[])),
            // 0xF7AAA0 mode 2: standing → the wait by foot (table 0x1A353C0), moving → Main (the walk)
            BeamEntryMode::Straight => self.play(BeamState::Wait, item(BEAM_WAIT[e.foot], 0, &[])),
            BeamEntryMode::Side | BeamEntryMode::SideWalk | BeamEntryMode::SideWalkBack => {
                self.warp = e.action.map(|a| a.duration()).unwrap_or(0.3);
                self.play(BeamState::Reception, e.action);
            }
            BeamEntryMode::Reception => {
                self.warp = CATCH_WARP;
                self.play(BeamState::Reception, item(BEAM_LANDING, 0, &[]));
            }
        }
    }

    pub fn enter_pilotis(&mut self, e: PilotisEntry) {
        self.kind = NarrowKind::Pilotis;
        self.top = e.top;
        self.pilotis_facing = Vec3::new(e.facing.x, 0.0, e.facing.z).normalize_or(Vec3::NEG_Z);
        self.entry_from = e.from;
        self.warp_from = e.from;
        self.foot = e.foot;
        self.lean = 0.0;
        self.hand_target = None;
        let h = super::heading_of(self.pilotis_facing);
        self.warp_heading = (h, h);
        // 0xE50190: the entry clip by foot, the root interpolated to the top over its duration; 0xE0BB70: the soft
        // landing, interpolated over 0.2 s
        let a = match e.kind {
            PilotisEntryType::FromFreeStep => item(PILOTIS_FROM_FREESTEP[e.foot], 0, &[]),
            PilotisEntryType::FromInAir => item(PILOTIS_FROM_AIR, 0, &[]),
        };
        self.warp = match e.kind {
            PilotisEntryType::FromFreeStep => a.map(|a| a.duration()).unwrap_or(0.3),
            PilotisEntryType::FromInAir => CATCH_WARP,
        };
        self.play(BeamState::PilotisIn, a);
    }

    fn play(&mut self, state: BeamState, a: Option<ActionBlend>) {
        self.state = state;
        self.action = a;
        self.t = 0.0;
        self.applied = 0.0;
        self.seq = self.seq.wrapping_add(1);
    }

    /// Beam direction p0 → p1 (horizontal).
    pub fn dir(&self) -> Vec3 {
        let d = self.p1 - self.p0;
        Vec3::new(d.x, 0.0, d.z).normalize_or_zero()
    }

    pub fn len(&self) -> f32 {
        let d = self.p1 - self.p0;
        Vec3::new(d.x, 0.0, d.z).length()
    }

    pub fn facing(&self) -> Vec3 {
        match self.kind {
            NarrowKind::Pilotis => self.pilotis_facing,
            NarrowKind::Beam if self.toward_p1 => self.dir(),
            NarrowKind::Beam => -self.dir(),
        }
    }

    /// Root on the beam line at `s`.
    pub fn point(&self, s: f32) -> Vec3 {
        let u = (s / self.len().max(1e-4)).clamp(0.0, 1.0);
        self.p0.lerp(self.p1, u)
    }

    /// Where the root stands: the beam point or the pilotis top.
    pub fn stand(&self) -> Vec3 {
        match self.kind {
            NarrowKind::Beam => self.point(self.s),
            NarrowKind::Pilotis => self.top,
        }
    }

    /// Distance left to the end the character faces.
    fn to_end(&self) -> f32 {
        if self.toward_p1 { self.len() - self.s } else { self.s }
    }

    pub fn current(&self) -> Option<(ActionBlend, f32)> {
        self.action.map(|a| {
            let ph = self.t / a.duration().max(1e-4);
            // loops: the waits
            let looping = matches!(self.state, BeamState::ImpulseWait | BeamState::PilotisWait) || (self.state == BeamState::Wait && a.duration() > 1.0);
            (a, if looping { ph.fract() } else { ph.min(1.0) })
        })
    }

    fn wait_state(&mut self) {
        match self.kind {
            NarrowKind::Beam => {
                let w = item(BEAM_WAIT[self.foot], 0, &[]);
                self.play(BeamState::Wait, w);
            }
            NarrowKind::Pilotis => {
                let w = item(PILOTIS_WAIT, 0, &pilotis_weights(self.lean));
                self.play(BeamState::PilotisWait, w);
            }
        }
    }
}

/// 0xE51960: [wait, left, right] from the lean l ∈ [−1, 1].
fn pilotis_weights(l: f32) -> [f32; 3] {
    if l >= 0.0 { [1.0 - l, 0.0, l] } else { [1.0 + l, -l, 0.0] }
}

/// Movement event 72's guard 0xD9F4C0: a beam (guidance) in the box ahead of the feet: ±0.75 m sideways,
/// 0–1.0 m ahead, ±0.53 m vertically (`sub_116E1A0`); the facing within 30° of the beam axis (straight entry,
/// mode 2: |dot| ≥ 0.866, `TryMountBeam` 0xE52AD0). Side entries (mode 3) are not ported.
pub fn try_mount_beam(feet: Vec3, forward: Vec3, guidance: &GuidanceWorld) -> Option<BeamEntry> {
    let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let r = super::right_of(f);
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::Beam {
            continue;
        }
        let axis = Vec3::new(e.p1.x - e.p0.x, 0.0, e.p1.z - e.p0.z).normalize_or_zero();
        let along = axis.dot(f);
        if along.abs() < 0.866 {
            continue;
        }
        // the beam end nearest to the feet, a little onto the beam
        let (start, toward_p1) = if along > 0.0 { (e.p0, true) } else { (e.p1, false) };
        let d = start - feet;
        let ahead = d.dot(f);
        if !(0.0..=1.0).contains(&ahead) || d.dot(r).abs() > 0.75 || d.y.abs() > 0.53 {
            continue;
        }
        let point = start + if toward_p1 { axis } else { -axis } * 0.2;
        return Some(BeamEntry { p0: e.p0, p1: e.p1, point, from: feet, toward_p1, mode: BeamEntryMode::Ground, foot: 0, action: None, facing: f });
    }
    None
}

/// 0xE0B890 (the beam test of `CheckAirCatch`, also used for the free-step arrival): a Beam edge in the box
/// ±0.55 m sideways, ±0.4 m along the facing, ±0.35 m vertically around the feet + 0.15 m (sub_116E1A0, cone π);
/// the root goes to the closest point of the beam (sub_946AC0) when a capsule fits there (sub_116D960).
/// Returns (p0, p1, point).
pub fn beam_at(feet: Vec3, facing: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3, Vec3)> {
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::NEG_Z);
    let r = super::right_of(f);
    let c = feet + Vec3::Y * 0.15;
    let mut best: Option<(f32, (Vec3, Vec3, Vec3))> = None;
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::Beam {
            continue;
        }
        // the beam point closest to the box centre must lie in the box
        let q = e.closest_point(c);
        let d = q - c;
        if d.dot(r).abs() > 0.55 || d.dot(f).abs() > 0.4 || d.y.abs() > 0.35 {
            continue;
        }
        let point = e.closest_point(feet);
        if !clear_above(point, collision) {
            continue;
        }
        let dist = (point - feet).length();
        if best.as_ref().is_none_or(|b| dist < b.0) {
            best = Some((dist, (e.p0, e.p1, point)));
        }
    }
    best.map(|b| b.1)
}

/// The entry mode of a free-step arrival on a beam (`TryMountBeam` 0xE52AD0): with the stick (> 0.25) the
/// angle between the character's back and the beam axis (axis flipped toward the right) picks the side walks:
/// 80°–150° with the stick within 45° of the axis → 4; 30°–100° with the stick > 135° from it → 5. Otherwise
/// |axis · forward| ≥ 0.866 → 2 (straight), else 3 (side).
pub fn beam_entry_mode(axis: Vec3, forward: Vec3, stick: Option<Vec3>) -> BeamEntryMode {
    let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z);
    let mut a = Vec3::new(axis.x, 0.0, axis.z).normalize_or_zero();
    if let Some(st) = stick {
        if a.dot(super::right_of(f)) < 0.0 {
            a = -a;
        }
        let back_angle = (-f).dot(a).clamp(-1.0, 1.0).acos();
        let stick_angle = st.dot(a).clamp(-1.0, 1.0).acos();
        if back_angle > 80f32.to_radians() && back_angle < 150f32.to_radians() && stick_angle < 45f32.to_radians() {
            return BeamEntryMode::SideWalk;
        }
        if back_angle > 30f32.to_radians() && back_angle < 100f32.to_radians() && stick_angle > 135f32.to_radians() {
            return BeamEntryMode::SideWalkBack;
        }
    }
    if axis.dot(f).abs() >= 0.866 { BeamEntryMode::Straight } else { BeamEntryMode::Side }
}

/// The beam entry for a free-step arrival at `feet` facing `forward` (0xE07D00 → NarrowObject Movement →
/// `TryMountBeam` 0xE52AD0 / `HumanNarrowObjectBeam` enter 0xF7AAA0). `reception` = the free-step reception the
/// arrival plays (mode 3 keeps it); `jog` picks the jog clips of the side entries.
pub fn free_step_beam_entry(
    feet: Vec3,
    forward: Vec3,
    stick: Option<Vec3>,
    foot: usize,
    reception: Option<ActionBlend>,
    jog: bool,
    guidance: &GuidanceWorld,
    collision: &CollisionWorld,
) -> Option<BeamEntry> {
    let (p0, p1, point) = beam_at(feet, forward, guidance, collision)?;
    let axis = Vec3::new(p1.x - p0.x, 0.0, p1.z - p0.z).normalize_or_zero();
    let mode = beam_entry_mode(axis, forward, stick);
    let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z);
    let (toward_p1, action) = match mode {
        BeamEntryMode::SideWalk | BeamEntryMode::SideWalkBack => {
            let st = stick.unwrap_or(f);
            // walk away along the stick; 0xF7AAA0 blends the 30° and 90° clips by the angle to the beam
            let toward = st.dot(axis) >= 0.0;
            let walk = if toward { axis } else { -axis };
            let ang = (-f).dot(walk).clamp(-1.0, 1.0).acos();
            let w90 = if mode == BeamEntryMode::SideWalk {
                let a = (ang + 30f32.to_radians()).clamp(120f32.to_radians(), 180f32.to_radians());
                ((60f32.to_radians() - (a - 120f32.to_radians())) / 60f32.to_radians()).clamp(0.0, 1.0)
            } else {
                let a = (ang - 30f32.to_radians()).clamp(0.0, 90f32.to_radians());
                (a / 60f32.to_radians()).clamp(0.0, 1.0)
            };
            let k = match (mode, foot) {
                (BeamEntryMode::SideWalkBack, 1) => 0,
                (BeamEntryMode::SideWalk, 1) => 1,
                (BeamEntryMode::SideWalkBack, _) => 2,
                _ => 3,
            };
            let w = if jog { [0.0, 0.0, 1.0 - w90, w90] } else { [1.0 - w90, w90, 0.0, 0.0] };
            (toward, item(BEAM_SIDE_ENTRY[k], 0, &w))
        }
        // the beam direction nearest the facing
        _ => (axis.dot(f) >= 0.0, if mode == BeamEntryMode::Side { reception } else { None }),
    };
    Some(BeamEntry { p0, p1, point, from: feet, toward_p1, mode, foot, action, facing: f })
}

/// `sub_B2B600`: is there a pilotis (a small post) here? Guidance in the box around `support` (from the air
/// ±0.6 sideways, −0.3…1.1 along `dir`, else ±0.6 / ±0.6; ±0.5 vertically); LedgeGrab edges facing −dir
/// (≤ 0.3 m), +dir (≤ 1.3 m), and both sides (≤ 0.6 m from the middle of those two), 30° / 45° cones; the
/// character within 0.375 m of one of them, front–back and left–right ≤ 0.7 m apart; the top centre (the mean
/// of the four points) free for a 0.4 m capsule from 0.6 to 1.4 m above it. Returns the top centre.
pub fn find_pilotis(pos: Vec3, support: Vec3, dir: Vec3, from_air: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<Vec3> {
    let f = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
    let r = super::right_of(f);
    let (fmin, fmax) = if from_air { (-0.3, 1.1) } else { (-0.6, 0.6) };
    let in_box = |q: Vec3| {
        let d = q - support;
        d.dot(r).abs() <= 0.6 && (fmin..=fmax).contains(&d.dot(f)) && d.y.abs() <= 0.5
    };
    let edges: Vec<_> = guidance
        .edges
        .iter()
        .filter(|e| e.subtype == GuidanceSubType::LedgeGrab && in_box(e.closest_point(support)))
        .collect();
    // sub_B1C550: the nearest edge whose outward normal is within 45° of `d`, within `max` of `from`
    let find = |from: Vec3, d: Vec3, max: f32| -> Option<Vec3> {
        edges
            .iter()
            .filter(|e| Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero().dot(d) >= 45f32.to_radians().cos())
            .map(|e| e.closest_point(from))
            .filter(|q| Vec2::new(q.x - from.x, q.z - from.z).length() <= max && (q.y - from.y).abs() <= 0.5)
            .min_by(|a, b| (*a - from).length().total_cmp(&(*b - from).length()))
    };
    let back = find(support, -f, 0.3)?;
    let front = find(support, f, 1.3)?;
    let mid = (back + front) * 0.5;
    let left = find(mid, -r, 0.6)?;
    let right = find(mid, r, 0.6)?;
    let flat = |a: Vec3, b: Vec3| Vec2::new(a.x - b.x, a.z - b.z).length();
    let near = [back, front, left, right].iter().map(|q| flat(*q, pos)).fold(f32::INFINITY, f32::min);
    if near > 0.375 || flat(back, front) > 0.7 || flat(left, right) > 0.7 {
        return None;
    }
    let top = (back + front + left + right) * 0.25;
    clear_above(top, collision).then_some(top)
}

/// Clearance test of 0xB2B600 / 0xE0B890 (sub_B2A290 / sub_116D960): no solid within 0.4 m of the segment
/// 0.6–1.4 m above `p`.
fn clear_above(p: Vec3, collision: &CollisionWorld) -> bool {
    let (a, b) = (p + Vec3::Y * 0.6, p + Vec3::Y * 1.4);
    !collision.boxes.iter().any(|bx| {
        (0..=8).any(|k| {
            let c = a.lerp(b, k as f32 / 8.0);
            (c - c.clamp(bx.min, bx.max)).length() < 0.4
        })
    })
}

/// `CanStepOffBeamEnd` 0xF77C00: free capsule (r 0.25) at end + 0.5·dir, 1.2 m above the feet, and floor there.
fn can_step_off(n: &HumanNarrowObjectData, collision: &CollisionWorld) -> bool {
    let end = if n.toward_p1 { n.p1 } else { n.p0 };
    let beyond = end + n.facing() * 0.5;
    !collision.point_inside(beyond + Vec3::Y * 1.2) && collision.ground_height(beyond + Vec3::Y * 0.3, 0.6).is_some()
}

/// A free-step jump target from the beam / pilotis in the stick direction (event 4: `Human__SetupJumpToTarget`
/// with jump kind 1, 0xE4D950).
fn jump_target(feet: Vec3, dir: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<JumpTarget> {
    targets::find_jump_target(feet, dir, guidance, collision)
}

pub fn update_narrow(
    time: Res<Time>,
    mut pad: ResMut<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data) in &mut q {
        if loco.current != ActorContextId::NarrowObject {
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            continue;
        }
        let n = &mut data.narrow;
        n.t += dt;
        let facing = n.facing();
        body.velocity = Vec3::ZERO;
        body.grounded = true;
        // stick vs facing (0xF76150)
        let stick = pad.speed01 > 0.0;
        let a = if stick { pad.dir.dot(facing).clamp(-1.0, 1.0).acos() } else { 0.0 };
        let forward = stick && a < 75f32.to_radians();
        let back = stick && a > 135f32.to_radians();
        let jog = pad.high_profile;
        let walk_w = if jog { [0.0, 1.0] } else { [1.0, 0.0] };
        let dur = n.action.map(|a| a.duration()).unwrap_or(0.3);
        let done = n.t >= dur;

        // ------------------------------------------------ jumps (PORT trigger: high profile + Legs, as on the ground)
        let jump = pad.high_profile && pad.jump_buffered();
        let can_jump = matches!(n.state, BeamState::Wait | BeamState::Start | BeamState::Walk | BeamState::Stop | BeamState::ImpulseWait | BeamState::PilotisWait);
        if jump && can_jump {
            let feet = n.stand();
            // event 4: a target in the stick direction → free-step jump (kind 1)
            if stick {
                if let Some(target) = jump_target(feet, pad.dir, &guidance, &collision) {
                    pad.consume_jump();
                    let foot_left = n.foot == 0;
                    body.feet = feet;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(InAirEntry::FreeStepJump { from: feet, target, foot_left }));
                    continue;
                }
            }
            if n.state == BeamState::ImpulseWait {
                // events 5 / 6 (+148, 0xF717D0): the jump on the spot, at a hand target when there is one
                pad.consume_jump();
                n.hand_target = super::ground::straight_hand_target(feet, facing, &guidance, &collision, true);
                let id = if n.hand_target.is_some() { BEAM_IMPULSE_TO_JUMP } else { BEAM_IMPULSE_TO_CLEAR };
                n.play(BeamState::JumpOnPlace, item(id, 0, &[]));
            } else if !stick {
                // event 2 (pilotis, 0xE53850 → PilotisToBeam mode 8) / Main: the impulsion crouch (+145, 0xF738A0)
                pad.consume_jump();
                let id = if n.kind == NarrowKind::Pilotis { item(PILOTIS_TO_IMPULSE, 0, &[]) } else { item(BEAM_TO_IMPULSE[n.foot], 0, &[]) };
                n.play(BeamState::ImpulseIn, id);
            }
        }
        let n = &mut data.narrow;
        let dur = n.action.map(|a| a.duration()).unwrap_or(0.3);
        let done = done && n.t >= dur;

        // root motion projected on the beam (0xF7C3A0): the action's forward displacement
        if let Some(act) = n.action {
            let d = act.disp((n.t / dur).min(1.0))[1];
            let step = d - n.applied;
            n.applied = d;
            if n.kind == NarrowKind::Beam && matches!(n.state, BeamState::Walk | BeamState::Start | BeamState::Stop) {
                let sign = if n.toward_p1 { 1.0 } else { -1.0 };
                let mut s = n.s + step * sign;
                let stop = if can_step_off(n, &collision) { BEAM_STEP_OFF } else { BEAM_END_STOP };
                let limit_lo = if n.toward_p1 { 0.0 } else { stop };
                let limit_hi = if n.toward_p1 { n.len() - stop } else { n.len() };
                s = s.clamp(limit_lo, limit_hi);
                n.s = s;
            }
        }

        let mut to_air: Option<InAirEntry> = None;
        match n.state {
            BeamState::Entry => {
                let k = (n.t / ENTRY_TIME).min(1.0);
                body.feet = n.entry_from.lerp(n.point(n.s), k);
                body.heading = super::heading_of(facing);
                if k >= 1.0 {
                    n.wait_state();
                }
                continue;
            }
            BeamState::Reception | BeamState::PilotisIn => {
                // the entry action plays while the root is interpolated onto the beam / top (sub_711130 /
                // sub_7113F0); modes 3 / 7 turn onto the beam meanwhile
                let k = (n.t / n.warp.max(1e-3)).min(1.0);
                body.feet = n.warp_from.lerp(n.stand(), k);
                let (h0, h1) = n.warp_heading;
                let dh = (h1 - h0 + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                body.heading = h0 + dh * k;
                if k >= 1.0 && done {
                    let walking = matches!(n.entry_mode, BeamEntryMode::SideWalk | BeamEntryMode::SideWalkBack) && n.kind == NarrowKind::Beam;
                    if walking && forward {
                        let w = item(BEAM_WALK, n.foot, &walk_w);
                        n.play(BeamState::Walk, w);
                    } else {
                        n.wait_state();
                    }
                }
                continue;
            }
            BeamState::Wait => {
                if back {
                    let t = item(BEAM_TURN180[n.foot], 0, &[]);
                    n.play(BeamState::Turn, t);
                } else if forward && n.to_end() > BEAM_END_STOP + 0.05 {
                    let st = item(BEAM_START[n.foot], 0, &walk_w);
                    n.play(BeamState::Start, st);
                } else if forward && can_step_off(n, &collision) {
                    let st = item(BEAM_START[n.foot], 0, &walk_w);
                    n.play(BeamState::Start, st);
                } else if done {
                    n.wait_state();
                }
            }
            BeamState::Start | BeamState::Walk => {
                if !forward {
                    // released: the jog stops with its stop action, the walk settles into the wait
                    if jog {
                        let st = item(BEAM_JOG_STOP[n.foot], 0, &[]);
                        n.play(BeamState::Stop, st);
                    } else {
                        n.wait_state();
                    }
                } else if done {
                    if n.state == BeamState::Walk {
                        n.foot ^= 1;
                    }
                    let w = item(BEAM_WALK, n.foot, &walk_w);
                    n.play(BeamState::Walk, w);
                }
            }
            BeamState::Stop => {
                if done {
                    n.wait_state();
                }
            }
            BeamState::Turn => {
                if done {
                    n.toward_p1 = !n.toward_p1;
                    n.wait_state();
                }
            }
            BeamState::ImpulseIn => {
                if done {
                    let w = item(BEAM_IMPULSE_WAIT, 0, &[]);
                    n.play(BeamState::ImpulseWait, w);
                }
            }
            BeamState::ImpulseWait => {
                // PORT: pushing the stick away from any target leaves the crouch (the game's exit event is not traced)
                if stick {
                    n.wait_state();
                }
            }
            BeamState::JumpOnPlace => {
                // PORT: the game leaves at the action's release point (sub_5017B0); the port at its end
                if done {
                    let from = n.stand();
                    to_air = Some(match n.hand_target {
                        // 0xF73B80 → Human__SetupJumpToHandTarget with the beam flights
                        Some(target) => InAirEntry::JumpToTarget { from, target, speed_param: 0.0, foot_left: n.foot == 0 },
                        None => InAirEntry::OnPlace {
                            from,
                            fwd: facing,
                            action: ActionBlend::new(BEAM_JUMP_CLEAR, 0, &[1.0]),
                            fall: Some(ActionBlend::new(BEAM_JUMP_CLEAR_FALL, 0, &[1.0])),
                        },
                    });
                }
            }
            BeamState::PilotisWait => {
                // 0xE51960: the lean follows the stick's signed angle to the facing (±120° → ±1) at 3/s
                let want = if stick {
                    let side = pad.dir.dot(super::right_of(facing));
                    let ang = pad.dir.dot(facing).clamp(-1.0, 1.0).acos().min(120f32.to_radians());
                    side.signum() * ang / 120f32.to_radians()
                } else {
                    0.0
                };
                n.lean += (want - n.lean) * (dt * 3.0).min(1.0);
                if let Some(a) = n.action.as_mut() {
                    *a = ActionBlend::new(PILOTIS_WAIT, 0, &pilotis_weights(n.lean));
                }
            }
        }
        if let Some(entry) = to_air {
            body.feet = data.narrow.stand();
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            continue;
        }
        let n = &data.narrow;
        body.feet = n.stand();
        body.heading = super::heading_of(n.facing());
        // step off the end onto the floor beyond (0xF77C00) → Ground
        if n.kind == NarrowKind::Beam && matches!(n.state, BeamState::Walk | BeamState::Start) && n.to_end() <= BEAM_STEP_OFF + 1e-3 && can_step_off(n, &collision) {
            body.feet += n.facing() * (BEAM_STEP_OFF + 0.2);
            switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
        }
    }
}
