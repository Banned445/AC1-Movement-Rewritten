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
//!
//! The player's input (RE/05 §2.10): `GoAssassinActionInterpreter__BeamState` 0xEE9AF0 on a beam,
//! `__NarrowObjectState` 0xEE8EC0 on a pilotis. Main's sub-states (`StateBeam_Update` 0xF808D0): Idle +91,
//! Walk +94, Stop +97, the 90° wait +100 (facing across the beam), Turn180 +124, the turn to / from the 90° wait
//! +127. Exits: the pull-down to a hang under the edge (event 9, the empty hand, 0xE504D0 → Ledge), the wall run
//! (event 15, Walk only, 0xF77DC0 → Walling). The corner hop (event 7, +151 / +154 / +157) jumps onto another beam
//! beside the one walked along (RE/05 §2.11).

use bevy::prelude::*;

use super::air::InAirEntry;
use super::jump_blend::{self, ActionBlend};
use super::targets::{self, JumpTarget};
use super::{switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::CollisionWorld;
use crate::guidance::{GuidanceSubType, GuidanceWorld};
use crate::input::PadInput;

/// Fence the A2 beam completion changes for comparison with the previous port.
pub const BEAM_COMPLETION: bool = true;

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
/// Across-beam crouch transition (0xF738A0).
pub const BEAM_90_TO_IMPULSE: u32 = 0x5288_EF4F;
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
/// The 90° wait (facing across the beam): Idle with the stick to a side (75°–135°, `ClassifyStickDir` 1 left /
/// 2 right) turns to it (`PlayTurnTo90` 0xF77640, tables 0x1A353F0 / 0x1A353FC by foot:
/// `crouchwait_foot{l,r}_turn_{left,right}_to_crouchwait_90`), then waits in `crouchwait_90` (0xF702B0).
pub const BEAM_TO_90: [[u32; 2]; 2] = [[0x350D_9386, 0x350D_9387], [0x350D_9388, 0x350D_9389]];
pub const BEAM_WAIT_90: u32 = 0x02E4_A9DF;
/// From the 90° wait: the stick to a side turns back along the beam (`Play90TurnBack` 0xF78630:
/// `crouchwait_90_turn_{left,right}`), the stick back turns around across it (`PlayTurn180` 0xF78740:
/// `crouchwait_90_turn180`). Pushing forward (off the beam) does nothing for the player (0xF7BA60 is AI only).
pub const BEAM_90_TURN_BACK: [u32; 2] = [0x350D_A35D, 0x350D_A35E];
pub const BEAM_90_TURN180: u32 = 0x4F8A_3DE7;
/// The corner hop (event 7, `StartCornerHop` 0xF7D2F0): start (+151, 0xF7C960), the hop with the root interpolated
/// onto the other beam over its duration (+154, 0xF7CCF0), the landing into the jog (+157, 0xF7CF50). Each action has
/// one item of six clips [left, right, frontleft, frontright, backleft, backright] (`beam_cornerhop_*`). The walking
/// variants (`beam_cornerwalk_*`, 0x5343E72E..30) are picked at walking speed, which event 7's CanHandle (0xF7F150)
/// never accepts.
pub const BEAM_CORNER_HOP: [u32; 3] = [0x340F_1CDE, 0x340F_1CDF, 0x340F_1CE0];

pub const DUMPED_ACTIONS: &[u32] = &[
    BEAM_90_TO_IMPULSE, super::move_blend::ACT_GROUND_LOCOMOTION,
    BEAM_WAIT[0], BEAM_WAIT[1], BEAM_WALK, BEAM_START[0], BEAM_START[1], BEAM_JOG_STOP[0], BEAM_JOG_STOP[1], BEAM_TURN180[0], BEAM_TURN180[1],
    BEAM_SIDE_ENTRY[0], BEAM_SIDE_ENTRY[1], BEAM_SIDE_ENTRY[2], BEAM_SIDE_ENTRY[3],
    BEAM_TO_IMPULSE[0], BEAM_TO_IMPULSE[1], PILOTIS_TO_IMPULSE, BEAM_IMPULSE_WAIT, BEAM_IMPULSE_TO_JUMP, BEAM_IMPULSE_TO_CLEAR,
    BEAM_JUMP_CLEAR, BEAM_JUMP_CLEAR_FALL, PILOTIS_FROM_FREESTEP[0], PILOTIS_FROM_FREESTEP[1], PILOTIS_FROM_AIR, PILOTIS_WAIT,
    0x516D_52DB, 0x516D_52DC, 0x516D_52DD, 0x516D_52DE, 0x516D_52DF,
    BEAM_TO_90[0][0], BEAM_TO_90[0][1], BEAM_TO_90[1][0], BEAM_TO_90[1][1], BEAM_WAIT_90, BEAM_90_TURN_BACK[0], BEAM_90_TURN_BACK[1], BEAM_90_TURN180,
    BEAM_CORNER_HOP[0], BEAM_CORNER_HOP[1], BEAM_CORNER_HOP[2],
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

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[test]
    fn native_beam_completion_actions_are_single_item_one_shots() {
        use crate::assets::{ac_actions::{ActionGraph, CLASS_ACTION_BLOCK}, forge::Forge};
        let Some(game) = crate::assets::find_game_dir() else { eprintln!("skipped: game install absent"); return; };
        if !game.join("DataPC.forge").is_file() { eprintln!("skipped: main archive absent"); return; }
        let mut forge = Forge::open(&game.join("DataPC.forge")).unwrap();
        let entry = forge.find("Game Fix").cloned().unwrap();
        let mut graph = ActionGraph::default();
        for resource in forge.resources(&entry).unwrap().into_iter().filter(|r| r.class_hash == CLASS_ACTION_BLOCK) {
            graph.add_block(&resource.name, &resource.payload).unwrap();
        }
        // The local timing reduction must fail validation if an install needs item seams or queued actions.
        for id in [BEAM_IMPULSE_TO_JUMP, BEAM_IMPULSE_TO_CLEAR, BEAM_CORNER_HOP[0], BEAM_CORNER_HOP[2]] {
            let action = &graph.actions[&id];
            assert_eq!(action.repeat, 1, "{id:#x} must finish rather than loop");
            assert_eq!(action.items.len(), 1, "{id:#x} requires sequence timing");
            assert_eq!(action.items[0].blend.kind, 0, "{id:#x} has an item seam blend");
            assert_eq!(action.items[0].f20, 1.0, "{id:#x} has different item timing data");
            for transition in action.in_transition.iter().chain(action.out_transition.iter()).chain(action.items[0].transitions.iter()) {
                assert_eq!(transition.action_a, 0, "{id:#x} requires a queued transition action");
                assert_eq!(transition.blend_a.b_pos, 0, "{id:#x} requires phase inheritance");
            }
        }
    }

    #[test]
    fn lateral_root_motion_cannot_increase_error_and_correction_is_bounded() {
        let old = Vec3::Z * 0.2;
        assert_eq!(constrain_lateral(old, Vec3::Z * 0.3, false, 0.1), old);
        assert_eq!(constrain_lateral(old, Vec3::Z * 0.1, false, 0.1), Vec3::Z * 0.1);
        assert!((constrain_lateral(old, old, true, 1.0/60.0).length() - (0.2 - 2.0/60.0)).abs() < 1e-5);
        assert_eq!(constrain_lateral(old, old, true, 1.0), Vec3::ZERO);
    }

    #[test]
    fn beam_clearance_rejects_triangle_obstructions() {
        let mut collision = CollisionWorld::default();
        assert!(clear_above(Vec3::ZERO, &collision));
        collision.triangles.push(crate::triangles::Triangle::new([
            Vec3::new(-1.0,1.0,-1.0), Vec3::new(1.0,1.0,-1.0), Vec3::new(0.0,1.0,1.0)], 1).unwrap());
        assert!(!clear_above(Vec3::ZERO, &collision));
        assert!(clear_above(Vec3::X * 3.0, &collision));
    }

    #[test]
    fn step_off_clearance_rejects_obstacles_beside_the_centre_point() {
        let mut n = HumanNarrowObjectData::default();
        n.enter(BeamEntry { p0:Vec3::ZERO,p1:Vec3::X,point:Vec3::X*0.9,from:Vec3::X*0.9,
            toward_p1:true,mode:BeamEntryMode::Straight,foot:0,action:None,facing:Vec3::X });
        let mut collision = CollisionWorld::default();
        collision.boxes.push(crate::collision::Aabb3 { min:Vec3::new(-1.0,-1.0,-1.0),max:Vec3::new(3.0,0.0,1.0) });
        assert!(can_step_off(&n,&collision));
        collision.boxes.push(crate::collision::Aabb3 { min:Vec3::new(1.4,0.9,0.2),max:Vec3::new(1.6,1.5,0.3) });
        assert!(!collision.point_inside(Vec3::new(1.5,1.2,0.0)));
        assert!(!can_step_off(&n,&collision));
    }

    #[test]
    fn segment_detection_clips_long_edges_and_uses_the_rotated_fallback() {
        let mut g = GuidanceWorld::default();
        g.edges.push(crate::guidance::GuidanceEdge { p0: Vec3::X * -20.0, p1: Vec3::X * 20.0,
            n0: Vec3::Y, n1: Vec3::Z, subtype: GuidanceSubType::Beam });
        assert!(detect_beam(Vec3::ZERO, Vec3::X, &g).is_some());
        assert!(detect_beam(Vec3::ZERO, Vec3::Z, &g).is_some());
        assert!(detect_beam(Vec3::Z * 3.0, Vec3::X, &g).is_none());
        assert!(detect_beam(Vec3::Y * 0.6, Vec3::X, &g).is_none());
    }
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
    /// +127: the turn from the wait to the 90° wait (toward `turn_left`).
    TurnTo90,
    /// +100: the 90° wait, facing across the beam (+1040 = 2).
    Wait90,
    /// +127: the turn from the 90° wait back along the beam (+1040 = 1 at its end).
    TurnBack,
    /// +124 with +1040 = 2: the turn-around across the beam (back to the 90° wait).
    Turn90,
    /// +151 / +154 / +157: the corner hop's start, flight onto the other beam, and landing into the jog.
    HopStart,
    Hop,
    HopEnd,
    /// Main mode 8: ground locomotion over 0.8 m (0xF7B720 / 0xF78320).
    StepOff,
}

/// A corner hop target (`HumanNarrowObjectBeam` vt0 0xF7A310 → sub_1173CD0): the other beam's segment, the point
/// to land on (beam +2144) and the direction to go along it (+2160).
#[derive(Clone, Copy, Debug, Default)]
pub struct CornerHop {
    pub p0: Vec3,
    pub p1: Vec3,
    pub point: Vec3,
    pub dir: Vec3,
    /// Clip weights [left, right, frontleft, frontright, backleft, backright] (0xF7C960).
    pub w: [f32; 6],
    pub from: Vec3,
    pub from_heading: f32,
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
    /// Timer at beam +1968: entry sets its deadline to now +0.2 s (RE/05 §2.13, 0xF738A0 / 0x41F440).
    impulse_elapsed: f32,
    /// Leading foot of the next walk item (0 left, 1 right).
    pub foot: usize,
    pub entry_from: Vec3,
    pub entry_mode: BeamEntryMode,
    pub seq: u32,
    /// Root displacement already applied for the playing action (animation space).
    applied: Vec3,
    lateral: Vec3,
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
    /// Facing across the beam in the 90° wait (beam +1040 = 2); `None` = along it.
    pub across: Option<Vec3>,
    /// The playing turn goes to the left (`ClassifyStickDir` 1).
    pub turn_left: bool,
    /// The running corner hop.
    pub hop: Option<CornerHop>,
    /// Mode 8 destination, independent of the beam segment.
    step_off_to: Vec3,
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
        self.lateral = Vec3::ZERO;
        self.entry_mode = e.mode;
        self.foot = e.foot;
        self.hand_target = None;
        self.across = None;
        self.warp_from = e.from;
        self.warp_heading = (super::heading_of(e.facing), super::heading_of(self.facing()));
        match e.mode {
            BeamEntryMode::Ground => self.play(BeamState::Entry, if BEAM_COMPLETION { e.action.or_else(|| self.ground_step(false)) } else { item(BEAM_WAIT[0], 0, &[]) }),
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
        if state == BeamState::ImpulseIn { self.impulse_elapsed = 0.0; }
        self.state = state;
        self.action = a;
        self.t = 0.0;
        self.applied = Vec3::ZERO;
        self.seq = self.seq.wrapping_add(1);
    }

    fn ground_step(&self, jog: bool) -> Option<ActionBlend> {
        let mut w = [0.0; 17];
        w[if jog { super::move_blend::SLOT_JOG } else { super::move_blend::SLOT_WALK } as usize] = 1.0;
        item(super::move_blend::ACT_GROUND_LOCOMOTION, self.foot, &w)
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
            NarrowKind::Beam if self.across.is_some() => self.across.unwrap_or_default(),
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
            NarrowKind::Beam => self.point(self.s) + if BEAM_COMPLETION { self.lateral } else { Vec3::ZERO },
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
            let looping = matches!(self.state, BeamState::ImpulseWait | BeamState::PilotisWait | BeamState::Wait90) || (self.state == BeamState::Wait && a.duration() > 1.0);
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
    // PORT: conservative bounds rejection for the static guidance search; exact point/cone tests follow.
    let centre = support + f * ((fmin + fmax) * 0.5);
    let extent = r.abs() * 0.6 + f.abs() * ((fmax - fmin) * 0.5) + Vec3::Y * 0.5 + Vec3::splat(0.0001);
    let edges: Vec<_> = guidance
        .edges
        .iter()
        .filter(|e| e.subtype == GuidanceSubType::LedgeGrab
            && (!collision.native_query_culling || (e.p0.min(e.p1).cmple(centre + extent).all() && e.p0.max(e.p1).cmpge(centre - extent).all()))
            && in_box(e.closest_point(support)))
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
    if BEAM_COMPLETION {
        return collision.capsule_cast_free(p + Vec3::Y * 0.2, 1.6, 0.4, Vec3::ZERO);
    }
    let (a, b) = (p + Vec3::Y * 0.6, p + Vec3::Y * 1.4);
    !collision.boxes.iter().any(|bx| {
        (0..=8).any(|k| {
            let c = a.lerp(b, k as f32 / 8.0);
            (c - c.clamp(bx.min, bx.max)).length() < 0.4
        })
    })
}

/// Preview `CanStepOffBeamEnd` 0xF77C00 clearance beyond the end. PORT: requires floor support too.
fn can_step_off(n: &HumanNarrowObjectData, collision: &CollisionWorld) -> bool {
    let end = if n.toward_p1 { n.p1 } else { n.p0 };
    let beyond = end + n.facing() * 0.5;
    // 0xF77C00: an oriented clearance box. Floor probing is a PORT guard while native controller
    // support/fall handling for mode 8 is incomplete; it prevents walking in the air.
    let free = if BEAM_COMPLETION {
        collision.obb_free(beyond + Vec3::Y * 1.2, [super::right_of(n.facing()), Vec3::Y, n.facing()], Vec3::new(0.25, 0.4, 0.5))
    } else { !collision.point_inside(beyond + Vec3::Y * 1.2) };
    free && collision.ground_height(beyond + Vec3::Y * 0.3, 0.6).is_some()
}

/// `HumanNarrowObject__CanPullDownToHang` 0xE504D0 (event 9's guard on a beam and a pilotis): guidance in the box
/// ±0.6 m × ±0.6 m × ±0.5 m around the feet facing `dir`; a LedgeGrab edge whose outward normal lies within 45° of
/// `dir`, within 0.6 m (sub_B1C550); more than 2.5 m above the floor below it; then the hang clearance (sub_B2E4F0,
/// using oriented boxes). The search runs along `dir`, then along −`dir`. Returns the edge point, its outward normal and
/// the side for the pull-down table (`FillLedgePullDown` 0xE4EF70: the normal vs `dir`, 0 front < 45°, 1 back
/// > 135°; 2 / 3 left / right are only reached at exactly 45°).
pub fn pull_down_edge(feet: Vec3, dir: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3, usize)> {
    let d0 = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if d0 == Vec3::ZERO {
        return None;
    }
    for (side, d) in [(0usize, d0), (1, -d0)] {
        let r = super::right_of(d);
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for e in &guidance.edges {
            if e.subtype != GuidanceSubType::LedgeGrab {
                continue;
            }
            let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
            if n.dot(d) < 45f32.to_radians().cos() {
                continue;
            }
            let q = e.closest_point(feet);
            let rel = q - feet;
            if rel.dot(r).abs() > 0.6 || rel.dot(d).abs() > 0.6 || rel.y.abs() > 0.5 {
                continue;
            }
            let dist = Vec2::new(rel.x, rel.z).length();
            if dist > 0.6 {
                continue;
            }
            let below = collision.ground_height(q + n * 0.6 - Vec3::Y * 0.05, 50.0).unwrap_or(q.y - 100.0);
            if q.y - below <= 2.5 {
                continue;
            }
            if BEAM_COMPLETION && !pull_down_clear(feet, q, n, collision) { continue; }
            if best.as_ref().is_none_or(|b| dist < b.0) {
                best = Some((dist, q, n));
            }
        }
        if let Some((_, q, n)) = best {
            return Some((q, n, side));
        }
    }
    None
}

/// 0xB2E4F0: upright torso clearance followed by a box tilted 30° for the descent.
fn pull_down_clear(feet: Vec3, edge: Vec3, outward: Vec3, collision: &CollisionWorld) -> bool {
    let right = super::right_of(outward);
    let upright = (edge + outward * 0.25).with_y(feet.y + 1.075);
    if !collision.obb_free(upright, [right, Vec3::Y, outward], Vec3::new(0.3, 0.725, 0.75)) { return false; }
    let rotation = Quat::from_axis_angle(right, 30f32.to_radians());
    let (up, forward) = (rotation * Vec3::Y, rotation * outward);
    collision.obb_free(edge + forward * 0.4 - up * 0.4, [right, up, forward], Vec3::new(0.3, 0.8, 0.3))
}

/// Event 9 (the empty hand) on a beam / pilotis: the Ledge pull-down of type 3 (`FillLedgePullDown` 0xE4EF70,
/// `beam_pilotis_to_pulldown_soft_*`). The direction is the stick (> 0.35); without it the interpreter turns the
/// beam's line 90° toward the camera (0xEE9AF0) and a pilotis uses the camera direction (0xEE8EC0). PORT: without
/// the stick, the facing's right (beam) or the facing (pilotis); the guard tries the opposite side next anyway.
fn try_pull_down(n: &HumanNarrowObjectData, stick: Option<Vec3>, camera: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<super::ledge::LedgeEntry> {
    let feet = n.stand();
    let dir = stick.unwrap_or(if BEAM_COMPLETION { match n.kind {
        NarrowKind::Beam => {
            let side = super::right_of(n.facing());
            if side.dot(camera) >= 0.0 { side } else { -side }
        }
        NarrowKind::Pilotis => camera,
    }} else { match n.kind {
        NarrowKind::Beam => super::right_of(n.facing()),
        NarrowKind::Pilotis => n.facing(),
    }});
    let (p, normal, side) = pull_down_edge(feet, dir, guidance, collision)?;
    let moves = super::ledge_moves::pulldown_with(
        p,
        normal,
        feet,
        super::ledge_moves::PULLDOWN_BEAM_ORIENT[side],
        super::ledge_moves::PULLDOWN_BEAM_DESCENT[side],
        guidance,
        collision,
    )?;
    Some(super::ground::pulldown_ledge_entry(moves, normal, feet))
}

/// Clip a segment to an oriented box (centre, unit axes, half extents); returns the parameter range kept.
fn clip_segment(a: Vec3, b: Vec3, c: Vec3, axes: [Vec3; 3], half: [f32; 3]) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..3 {
        let p = (a - c).dot(axes[k]);
        let d = (b - a).dot(axes[k]);
        if d.abs() < 1e-6 {
            if p.abs() > half[k] {
                return None;
            }
            continue;
        }
        let (mut u0, mut u1) = ((-half[k] - p) / d, (half[k] - p) / d);
        if u0 > u1 {
            std::mem::swap(&mut u0, &mut u1);
        }
        t0 = t0.max(u0);
        t1 = t1.min(u1);
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, t1))
}

/// Current-segment passes of 0xF753A0: 60° cone, rotated 90°, then the ±1 m box without a cone.
fn detect_beam(root: Vec3, forward: Vec3, guidance: &GuidanceWorld) -> Option<(Vec3, Vec3)> {
    for (f, along, side, cone) in [(forward, 2.0, 0.5, 0.5), (super::right_of(forward), 2.0, 0.5, 0.5), (forward, 1.0, 1.0, 0.0)] {
        let right = super::right_of(f);
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for e in &guidance.edges {
            if e.subtype != GuidanceSubType::Beam { continue; }
            let axis = (e.p1 - e.p0).with_y(0.0).normalize_or_zero();
            if axis == Vec3::ZERO || axis.dot(f).abs() < cone { continue; }
            let Some((lo, hi)) = clip_segment(e.p0, e.p1, root, [right, f, Vec3::Y], [side, along, 0.5]) else { continue };
            let (a, b) = (e.p0.lerp(e.p1, lo), e.p0.lerp(e.p1, hi));
            let d = b - a;
            let point = a + d * ((root - a).dot(d) / d.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let distance = point.distance_squared(root);
            if best.is_none_or(|best| distance < best.0) { best = Some((distance, e.p0, e.p1)); }
        }
        if let Some((_, p0, p1)) = best { return Some((p0, p1)); }
    }
    None
}

/// 0xF7C3A0: root motion may reduce lateral error but must not increase it. Its optional
/// correction approaches the line at 2 m/s; current beam callers pass correction=false (0xF7EED0).
fn constrain_lateral(old: Vec3, proposed: Vec3, correction: bool, dt: f32) -> Vec3 {
    let old_len = old.length();
    let mut lateral = if proposed.length() > old_len { old } else { proposed };
    if old_len <= 0.0005 { lateral = Vec3::ZERO; }
    if correction {
        let len = lateral.length();
        lateral *= (1.0 - (2.0 * dt / len.max(1e-6)).min(1.0)).max(0.0);
    }
    lateral
}

/// The corner hop's target search (`HumanNarrowObjectBeam` vt0 0xF7A310 → sub_1173CD0), with the interpreter's
/// inputs (0xEE9AF0): the stick 40°–140° from the facing; `want` = the facing turned 45° (stick ahead) or 135°
/// (stick behind) toward the stick's side.
/// - The box: ±1.8 m along the facing, 0…r to the stick's side (centre at r / 2), ±0.5 m vertically; r = 0.75 m at
///   walking speed, 1.0 m above it (the player only hops above it).
/// - The Beam segments clipped to it (`GuidanceZone__FilterReport`), each oriented along `want` (sub_116DC30) and
///   within 58° of it (sub_1173B10, hypothesis for its angle arguments).
/// - The landing point is the clipped segment's far end; the stick's component toward it must exceed 0.766
///   (as written, without a normalisation: hypothesis). The nearest one wins.
pub fn find_corner_hop(root: Vec3, facing: Vec3, stick: Vec3, current: (Vec3, Vec3), guidance: &GuidanceWorld) -> Option<CornerHop> {
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
    let r_axis = super::right_of(f);
    let to_right = stick.dot(r_axis) >= 0.0;
    let side = if to_right { r_axis } else { -r_axis };
    let behind = stick.dot(f) < 0.0;
    let turn = if behind { 135f32 } else { 45f32 }.to_radians();
    let want = (f * turn.cos() + side * turn.sin()).normalize_or_zero();
    let r = 1.0;
    let c = root + side * (r * 0.5);
    let mut best: Option<(f32, CornerHop)> = None;
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::Beam || (e.p0 == current.0 && e.p1 == current.1) {
            continue;
        }
        let Some((t0, t1)) = clip_segment(e.p0, e.p1, c, [f, side, Vec3::Y], [1.8, r * 0.5, 0.5]) else { continue };
        let (a, b) = (e.p0.lerp(e.p1, t0), e.p0.lerp(e.p1, t1));
        let mut d = Vec3::new(b.x - a.x, 0.0, b.z - a.z).normalize_or_zero();
        let end = if d.dot(want) <= 0.0 { a } else { b };
        if d.dot(want) <= 0.0 {
            d = -d;
        }
        if d == Vec3::ZERO || d.dot(want) < 58f32.to_radians().cos() {
            continue;
        }
        let rel = Vec3::new(end.x - root.x, 0.0, end.z - root.z);
        if stick.dot(rel) <= 0.766 {
            continue;
        }
        let dist = rel.length_squared();
        if best.as_ref().is_none_or(|bst| dist < bst.0) {
            best = Some((dist, CornerHop { p0: e.p0, p1: e.p1, point: end, dir: d, w: corner_hop_weights(root, f, end), from: root, from_heading: 0.0 }));
        }
    }
    best.map(|b| b.1)
}

/// 0xF7C960: the start's six-clip blend. The target's offset along the facing over r (1.5 m above walking speed) is
/// the front / back share k; the side share is 1 − k; the side of the facing picks left or right.
fn corner_hop_weights(root: Vec3, facing: Vec3, target: Vec3) -> [f32; 6] {
    let rel = Vec3::new(target.x - root.x, 0.0, target.z - root.z);
    let k = (rel.dot(facing).abs() / 1.5).clamp(0.0, 1.0);
    let right = rel.dot(super::right_of(facing)) >= 0.0;
    let front = rel.dot(facing) >= 0.0;
    let mut w = [0.0; 6];
    w[right as usize] = 1.0 - k;
    let fb = if front { 2 } else { 4 } + if right { 1 } else { 0 };
    w[fb] = k;
    w
}

/// `DetectBeamSegments` 0xF753A0, the next segment: a Beam edge crossing the box 0.4–1.0 m ahead of the root along
/// the walking direction, ±0.5 m to the sides and vertically, running within 45° of it (sub_116E1A0, cone π/4).
/// Returns its ends.
fn next_segment(root: Vec3, forward: Vec3, current: (Vec3, Vec3), guidance: &GuidanceWorld) -> Option<(Vec3, Vec3)> {
    let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let c = root + f * 0.7;
    for e in &guidance.edges {
        if e.subtype != GuidanceSubType::Beam || (e.p0 == current.0 && e.p1 == current.1) {
            continue;
        }
        let d = Vec3::new(e.p1.x - e.p0.x, 0.0, e.p1.z - e.p0.z).normalize_or_zero();
        if d.dot(f).abs() < 45f32.to_radians().cos() {
            continue;
        }
        if clip_segment(e.p0, e.p1, c, [f, super::right_of(f), Vec3::Y], [0.3, 0.5, 0.5]).is_some() {
            return Some((e.p0, e.p1));
        }
    }
    None
}

/// A free-step jump target from the beam / pilotis in the stick direction (event 4: `Human__SetupJumpToTarget`
/// with jump kind 1, 0xE4D950).
fn jump_target(feet: Vec3, dir: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<JumpTarget> {
    targets::find_jump_target(feet, dir, guidance, collision)
}

/// Beam event 16 (0xF70A60 / 0xF70CD0 → 0xB2E860): foot and hand probes 1.2 m apart,
/// forward then to either side; the handler uses the ordinary climb start (0xF70D00 → 0xB26080).
fn climb_from_beam(feet: Vec3, facing: Vec3, foot_right: bool, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<super::climb::ClimbEntry> {
    for (origin, forward) in [(feet + facing * 0.95, facing), (feet + facing * 0.8, super::right_of(facing)), (feet + facing * 0.8, -super::right_of(facing))] {
        let right = super::right_of(forward);
        for height in [0.0, 0.6, -0.6] {
            let centre = origin + Vec3::Y * height;
            let foot = guidance.probe_box(centre, right, forward, Vec3::Y, Vec3::new(0.25, 0.75, 0.3), centre,
                45f32.to_radians(), 0.1, 1 << GuidanceSubType::LedgeGrab as u32);
            let Some(foot) = foot else { continue };
            let centre = foot.point + Vec3::Y * 1.2;
            let Some(hand) = guidance.probe_box(centre, right, forward, Vec3::Y, Vec3::splat(0.3), centre,
                45f32.to_radians(), 0.1, 1 << GuidanceSubType::LedgeGrab as u32) else { continue };
            let root = super::climb::climb_root_at(hand.point, hand.point, foot.point, foot.point, hand.wall_normal);
            // PORT: capsule casts stand in for the native pose/path helpers 0xB2E160 / 0xB15ED0.
            if !collision.capsule_cast_free(feet + Vec3::Y * 0.02, 1.8, 0.35, root - feet) { continue; }
            return Some(super::climb::ClimbEntry { entry_type: super::climb::ClimbEntryType::FromGround,
                hand_l: hand.point, hand_r: hand.point, foot_l: foot.point, foot_r: foot.point,
                normal: hand.wall_normal, from_feet: feet, foot_right, action: None });
        }
    }
    None
}

pub fn update_narrow(
    time: Res<Time>,
    mut pad: ResMut<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    camera: Res<crate::camera::CameraRig>,
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
        if matches!(n.state, BeamState::ImpulseIn | BeamState::ImpulseWait) { n.impulse_elapsed += dt; }
        // Main loses support when neither the current nor next beam survives detection (0xF808D0).
        // PORT: reuse the existing ballistic fall; the native callback 0xF6ECF0 plays runtime action 33.
        if BEAM_COMPLETION && n.kind == NarrowKind::Beam
            && !matches!(n.state, BeamState::Entry | BeamState::Reception | BeamState::HopStart | BeamState::Hop | BeamState::HopEnd | BeamState::StepOff)
        {
            if let Some((p0, p1)) = detect_beam(body.feet, body.forward(), &guidance) {
                if (p0 == n.p0 && p1 == n.p1) || (p1 == n.p0 && p0 == n.p1) {
                    // The current support remains valid.
                } else {
                let facing = n.facing();
                n.p0 = p0;
                n.p1 = p1;
                n.toward_p1 = n.dir().dot(facing) >= 0.0;
                n.s = (body.feet - p0).dot(n.dir()).clamp(0.0, n.len());
                }
            } else {
                let entry = InAirEntry::Fall { from: body.feet, velocity: body.velocity, origin: super::air::FallOrigin::Ground, speed_param: 0.0 };
                body.grounded = false;
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
                continue;
            }
        }
        let facing = n.facing();
        body.velocity = Vec3::ZERO;
        body.grounded = true;
        // stick vs facing (0xF76150): the interpreter moves only past 0.35 (0xEE9AF0), which speed01 > 0 means
        let stick = pad.speed01 > 0.0;
        let a = if stick { pad.dir.dot(facing).clamp(-1.0, 1.0).acos() } else { 0.0 };
        let forward = stick && a < 75f32.to_radians();
        let back = stick && a > 135f32.to_radians();
        let side = stick && !forward && !back;
        let left = pad.dir.dot(super::right_of(facing)) < 0.0;
        let jog = pad.high_profile;
        let walk_w = if jog { [0.0, 1.0] } else { [1.0, 0.0] };
        let dur = n.action.map(|a| a.duration()).unwrap_or(0.3);
        // RE/05 §2.14: item advance 0x778010 marks completion only with positive leftover time.
        // PORT: these single-item, one-shot actions have no queued evaluator in the movement simulation.
        let slot_done = matches!(n.state, BeamState::JumpOnPlace | BeamState::HopStart | BeamState::HopEnd);
        let done = if BEAM_COMPLETION && slot_done { n.t > dur } else { n.t >= dur };

        // Event 17: walking into an obstacle with stick input searches a hand jump target before
        // pull-down / climb (0xEE9AF0 → 0xF7EF90 / 0xF7DB30 → 0xF70E50 → 0xB21DA0).
        // PORT: the existing standing hand probe and an oriented contact box replace the native
        // report classification/rays; exact obstacle target selection remains a parity task.
        if BEAM_COMPLETION && n.kind == NarrowKind::Beam && matches!(n.state, BeamState::Start | BeamState::Walk)
            && stick && !collision.obb_free(n.stand() + pad.dir * 0.45 + Vec3::Y * 0.8,
                [super::right_of(pad.dir), Vec3::Y, pad.dir], Vec3::new(0.2, 0.3, 0.1)) {
            if let Some(target) = super::ground::straight_hand_target(n.stand(), pad.dir, &guidance, &collision, true) {
                let entry = InAirEntry::JumpToTarget { from: n.stand(), target, speed_param: 0.0, foot_left: n.foot == 0 };
                switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
                continue;
            }
        }

        // ------------------------------------------------ the pull-down to a hang (event 9, the empty hand)
        // Accepted in Idle, Walk and the 90° wait (CanHandle 0xF7A4F0 / 0xF7F150 / 0xF7A650) and in the pilotis wait
        // (0xE50BF0).
        let can_pull_down = matches!(n.state, BeamState::Wait | BeamState::Start | BeamState::Walk | BeamState::Wait90 | BeamState::PilotisWait);
        if can_pull_down && pad.hand_just_pressed() {
            let camera_dir = Vec3::new(camera.yaw.sin(), 0.0, camera.yaw.cos());
            if let Some(entry) = try_pull_down(n, stick.then_some(pad.dir), camera_dir, &guidance, &collision) {
                pad.hand_pressed_ago = f32::INFINITY;
                body.feet = n.stand();
                switch_context(&mut loco, &mut data, TransitionSetup::ToLedge(entry));
                continue;
            }
        }

        // ------------------------------------------------ the wall run (event 15, Walk only)
        // 0xEE9AF0: high profile, Legs pressed, the stick > 0.35 within 60° of the facing; tested before the jumps.
        // Guard `CanWallRun` 0xF77DC0 = the wall test 0xE18390 (1.5·h ahead), fill 0xB263B0 → Walling.
        // 0xB263B0 warps beam entries in 0.13 s; Ground retains its action-length warp.
        if n.kind == NarrowKind::Beam
            && matches!(n.state, BeamState::Start | BeamState::Walk)
            && pad.high_profile
            && pad.jump_buffered()
            && stick
            && a < 60f32.to_radians()
        {
            if let Some((contact, normal)) = super::walling::wall_ahead(n.stand(), facing, &collision) {
                pad.consume_jump();
                let from = n.stand();
                body.feet = from;
                switch_context(&mut loco, &mut data, TransitionSetup::ToWalling(super::walling::WallingEntry { contact, normal, from, warp_duration: BEAM_COMPLETION.then_some(0.13) }));
                continue;
            }
        }

        // ------------------------------------------------ the corner hop (event 7, Walk above walking speed)
        // 0xEE9AF0: vt232 (event 7 accepted: Walk, speed above the walk band, 0xF7F150) and the stick 40°–140° from
        // the facing; the target search (vt288) → vt236 → `StartCornerHop` 0xF7D2F0.
        if n.kind == NarrowKind::Beam
            && matches!(n.state, BeamState::Start | BeamState::Walk)
            && jog
            && stick
            && a > 40f32.to_radians()
            && a < 140f32.to_radians()
        {
            if let Some(mut hop) = find_corner_hop(n.stand(), facing, pad.dir, (n.p0, n.p1), &guidance) {
                hop.from_heading = super::heading_of(facing);
                let w = hop.w;
                n.hop = Some(hop);
                n.play(BeamState::HopStart, item(BEAM_CORNER_HOP[0], 0, &w));
            }
        }
        let n = &mut data.narrow;

        // ------------------------------------------------ jumps (high profile + Legs, 0xEE9AF0 / 0xEE8EC0)
        let jump = pad.high_profile && pad.jump_buffered();
        let impulsion = matches!(n.state, BeamState::ImpulseIn | BeamState::ImpulseWait);
        let impulse_ready = n.impulse_elapsed >= 0.2;
        let can_jump = matches!(n.state, BeamState::Wait | BeamState::Start | BeamState::Walk | BeamState::Wait90 | BeamState::ImpulseWait | BeamState::PilotisWait)
            || (BEAM_COMPLETION && n.state == BeamState::ImpulseIn && impulse_ready)
            || (!BEAM_COMPLETION && n.state == BeamState::Stop);
        // 0xF7A740 gates events 4 / 5 / 6 on the timer, independently of the entry clip.
        if jump && can_jump && (!BEAM_COMPLETION || !impulsion || impulse_ready) {
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
            if n.state == BeamState::ImpulseWait || (BEAM_COMPLETION && n.state == BeamState::ImpulseIn) {
                // events 5 / 6 (+148, 0xF717D0): the jump on the spot, at a hand target when there is one
                pad.consume_jump();
                n.hand_target = super::ground::straight_hand_target(feet, facing, &guidance, &collision, true);
                let id = if n.hand_target.is_some() { BEAM_IMPULSE_TO_JUMP } else { BEAM_IMPULSE_TO_CLEAR };
                n.play(BeamState::JumpOnPlace, item(id, 0, &[]));
            } else if !stick {
                // event 2 (pilotis, 0xE53850 → PilotisToBeam mode 8) / Main: the impulsion crouch (+145, 0xF738A0)
                pad.consume_jump();
                let id = if n.kind == NarrowKind::Pilotis { item(PILOTIS_TO_IMPULSE, 0, &[]) }
                    else if BEAM_COMPLETION && n.across.is_some() { item(BEAM_90_TO_IMPULSE, 0, &[]) }
                    else { item(BEAM_TO_IMPULSE[n.foot], 0, &[]) };
                n.play(BeamState::ImpulseIn, id);
            }
        }
        let n = &mut data.narrow;

        // Event 16 is the interpreter's final movement attempt, after wall runs / hops / jumps (0xEE9AF0).
        if BEAM_COMPLETION && n.kind == NarrowKind::Beam
            && matches!(n.state, BeamState::Wait | BeamState::Start | BeamState::Walk)
            && pad.hand_pressed_ago <= 0.2 {
            if let Some(entry) = climb_from_beam(n.stand(), facing, n.foot == 1, &guidance, &collision) {
                pad.hand_pressed_ago = f32::INFINITY;
                switch_context(&mut loco, &mut data, TransitionSetup::ToClimb(entry));
                continue;
            }
        }
        let n = &mut data.narrow;
        let dur = n.action.map(|a| a.duration()).unwrap_or(0.3);
        let done = done && n.t >= dur;

        // root motion projected on the beam (0xF7C3A0): the action's forward displacement
        if let Some(act) = n.action {
            let d = Vec3::from_array(act.disp((n.t / dur).min(1.0)));
            let step = d - n.applied;
            n.applied = d;
            if n.kind == NarrowKind::Beam && (matches!(n.state, BeamState::Walk | BeamState::Start | BeamState::Stop)
                || (BEAM_COMPLETION && n.state == BeamState::HopEnd)) {
                let sign = if n.toward_p1 { 1.0 } else { -1.0 };
                let step_world = if BEAM_COMPLETION { super::right_of(body.forward()) * step.x + body.forward() * step.y }
                    else { n.facing() * step.y };
                let along = step_world.dot(n.dir());
                if BEAM_COMPLETION {
                    let old = (body.feet - n.point(n.s)).with_y(0.0);
                    let proposed = old + step_world - n.dir() * along;
                    n.lateral = constrain_lateral(old, proposed, false, dt);
                }
                let mut s = n.s + if BEAM_COMPLETION { along } else { step.y * sign };
                // a next segment ahead (0xF753A0) lets the root run to the end and on along it
                let next = next_segment(n.stand(), n.facing(), (n.p0, n.p1), &guidance);
                let stop = if next.is_some() { 0.0 } else if can_step_off(n, &collision) { BEAM_STEP_OFF } else { BEAM_END_STOP };
                let limit_lo = if n.toward_p1 { 0.0 } else { stop };
                let limit_hi = if n.toward_p1 { n.len() - stop } else { n.len() };
                let over = if n.toward_p1 { s - limit_hi } else { limit_lo - s };
                s = s.clamp(limit_lo.min(limit_hi), limit_hi.max(limit_lo));
                n.s = s;
                if let (Some((q0, q1)), true) = (next, over >= 0.0) {
                    // `ConstrainRootMotionToBeam` 0xF7C3A0 switches to the next segment; going along it from the
                    // junction. PORT: the heading turns at once (the game steers to a look-ahead point 0.6 m ahead
                    // on the next segment, beam +288)
                    let junction = n.stand();
                    let fwd = n.facing();
                    n.p0 = q0;
                    n.p1 = q1;
                    let d = n.dir();
                    n.toward_p1 = d.dot(fwd) >= 0.0;
                    let sj = (junction - q0).dot(d).clamp(0.0, n.len());
                    n.s = (sj + over.max(0.0) * if n.toward_p1 { 1.0 } else { -1.0 }).clamp(0.0, n.len());
                    // Re-express the prior lateral offset in the new segment's perpendicular plane.
                    n.lateral -= n.dir() * n.lateral.dot(n.dir());
                }
            }
        }

        let mut to_air: Option<InAirEntry> = None;
        match n.state {
            BeamState::Entry => {
                if BEAM_COMPLETION {
                    // Mode 1 continues incoming root motion until displacement >0.5 m, end clamp or
                    // forward contact (0xF7EBA0 / 0xF76AD0), rather than a fixed entry timer.
                    let delta = n.action.map(|a| a.disp((n.t / dur).min(1.0))[1]
                        - a.disp(((n.t - dt).max(0.0) / dur).min(1.0))[1]).unwrap_or(0.0);
                    let dest = body.feet + facing * delta;
                    let moved = { let b = &mut *body; collision.move_capsule(&mut b.proxy, b.feet, dest - b.feet, true, dt) };
                    body.feet = moved.position;
                    n.s = (body.feet - n.p0).dot(n.dir()).clamp(0.0, n.len());
                    let off = (body.feet - n.point(n.s)).with_y(0.0);
                    n.lateral = off - n.dir() * off.dot(n.dir());
                    if Vec2::new(body.feet.x - n.entry_from.x, body.feet.z - n.entry_from.z).length_squared() > 0.25
                        || n.to_end() <= BEAM_END_STOP || moved.position.distance_squared(dest) > 1e-6 {
                        if forward { n.play(BeamState::Walk, item(BEAM_WALK, n.foot, &walk_w)); }
                        else { n.wait_state(); }
                    } else if done {
                        n.foot ^= 1;
                        n.play(BeamState::Entry, n.ground_step(jog));
                    }
                    body.heading = super::heading_of(facing);
                    continue;
                }
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
                // Idle (0xF808D0 +91), in order: turn around, turn to the 90° wait, walk / step off
                if back {
                    let t = item(BEAM_TURN180[n.foot], 0, &[]);
                    n.play(BeamState::Turn, t);
                } else if side && n.kind == NarrowKind::Beam {
                    n.turn_left = left;
                    let t = item(BEAM_TO_90[!left as usize][n.foot], 0, &[]);
                    n.play(BeamState::TurnTo90, t);
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
            BeamState::TurnTo90 => {
                if done {
                    // +1040 = 2: facing across, to the side turned to
                    let r = super::right_of(facing);
                    n.across = Some(if n.turn_left { -r } else { r });
                    let w = item(BEAM_WAIT_90, 0, &[]);
                    n.play(BeamState::Wait90, w);
                }
            }
            BeamState::Wait90 => {
                // +100 (0xF808D0): the stick to a side turns back along the beam, the stick back turns around
                if side {
                    n.turn_left = left;
                    let t = item(BEAM_90_TURN_BACK[!left as usize], 0, &[]);
                    n.play(BeamState::TurnBack, t);
                } else if back {
                    let t = item(BEAM_90_TURN180, 0, &[]);
                    n.play(BeamState::Turn90, t);
                }
            }
            BeamState::TurnBack => {
                if done {
                    // +1040 = 1: along the beam, toward the side turned to; then the wait (0xF70820)
                    let r = super::right_of(facing);
                    let along = if n.turn_left { -r } else { r };
                    n.toward_p1 = along.dot(n.dir()) >= 0.0;
                    n.across = None;
                    n.wait_state();
                }
            }
            BeamState::Turn90 => {
                if done {
                    n.across = Some(-facing);
                    let w = item(BEAM_WAIT_90, 0, &[]);
                    n.play(BeamState::Wait90, w);
                }
            }
            BeamState::HopStart => {
                // +151: the start in place (PORT: its root motion is not applied), then the hop (+154)
                if done {
                    let w = n.hop.map(|h| h.w).unwrap_or_default();
                    n.play(BeamState::Hop, item(BEAM_CORNER_HOP[1], 0, &w));
                }
            }
            BeamState::Hop => {
                // +154: the root interpolated to the landing point, facing the new beam, over the hop's duration
                // (sub_711130 / RootInterp__Advance)
                if let Some(h) = n.hop {
                    let k = (n.t / dur.max(1e-3)).min(1.0);
                    body.feet = h.from.lerp(h.point, k);
                    let h1 = super::heading_of(h.dir);
                    let dh = (h1 - h.from_heading + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                    body.heading = h.from_heading + dh * k;
                    if done {
                        // now on the other beam (the next segment), going along `dir`
                        n.p0 = h.p0;
                        n.p1 = h.p1;
                        n.s = (h.point - h.p0).dot(n.dir()).clamp(0.0, n.len());
                        n.toward_p1 = h.dir.dot(n.dir()) >= 0.0;
                        n.play(BeamState::HopEnd, item(BEAM_CORNER_HOP[2], 0, &h.w));
                    }
                    continue;
                }
                n.wait_state();
            }
            BeamState::HopEnd => {
                // +157: `cornerhop_*_tr_jog_foot*`, then Main (+320 = 5, +1728 = 3): the jog when the stick still
                // pushes along the beam
                if done {
                    n.hop = None;
                    if forward {
                        n.foot ^= 1;
                        let w = item(BEAM_WALK, n.foot, &walk_w);
                        n.play(BeamState::Walk, w);
                    } else {
                        n.wait_state();
                    }
                }
            }
            BeamState::ImpulseIn => {
                // 0xF80560 can resume Main before the entry clip ends, once the 0.2 s timer expires.
                if BEAM_COMPLETION && n.kind == NarrowKind::Beam && forward && n.impulse_elapsed >= 0.2 {
                    n.across = None;
                    n.play(BeamState::Walk, item(BEAM_WALK, n.foot, &walk_w));
                } else if done {
                    let w = item(BEAM_IMPULSE_WAIT, 0, &[]);
                    n.play(BeamState::ImpulseWait, w);
                }
            }
            BeamState::ImpulseWait => {
                // 0xF80560: only forward input resumes walking; pilotis waits for an event.
                if (BEAM_COMPLETION && n.kind == NarrowKind::Beam && forward && n.impulse_elapsed >= 0.2) || (!BEAM_COMPLETION && stick) {
                    if BEAM_COMPLETION {
                        n.across = None;
                        n.play(BeamState::Walk, item(BEAM_WALK, n.foot, &walk_w));
                    } else { n.wait_state(); }
                }
            }
            BeamState::JumpOnPlace => {
                // 0x5017B0 reads final queued evaluator completion (0x727B40); this action has one item.
                // PORT: native queue/blend scheduling is represented by the isolated action clock above.
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
            BeamState::StepOff => {
                // Main mode 8 moves freely until the target is passed or forward contact (0xF78320).
                let delta = n.action.map(|a| a.disp((n.t / dur).min(1.0))[1]
                    - a.disp(((n.t - dt).max(0.0) / dur).min(1.0))[1]).unwrap_or(0.0);
                let dest = body.feet + facing * delta;
                let moved = { let b = &mut *body; collision.move_capsule(&mut b.proxy, b.feet, dest - b.feet, true, dt) };
                body.feet = moved.position;
                body.heading = super::heading_of(facing);
                if (n.step_off_to - body.feet).dot(facing) <= 0.0 || moved.position.distance_squared(dest) > 1e-6 {
                    let foot = n.foot;
                    let phase = (n.t / dur).min(1.0);
                    let blocked = moved.hit_wall;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
                    // 0xF74BD0 keeps the locomotion action running into Movement. PORT: retain the
                    // shared blend phase without the native graph's transition fade.
                    let g = &mut data.ground;
                    g.speed_param = if blocked { 0.0 } else if jog { 0.5 } else { 0.25 };
                    g.blend.speed_param = g.speed_param;
                    g.blend.foot = foot;
                    g.blend.phase = phase;
                    g.blend.update_weights(g.speed_param, 0.0);
                } else if done {
                    n.foot ^= 1;
                    n.play(BeamState::StepOff, n.ground_step(jog));
                }
                continue;
            }
        }
        if let Some(entry) = to_air {
            body.feet = data.narrow.stand();
            switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
            continue;
        }
        let n = &data.narrow;
        body.feet = n.stand();
        if BEAM_COMPLETION && matches!(n.state, BeamState::Walk | BeamState::Start | BeamState::Stop | BeamState::HopEnd) {
            // Heading toward a point 0.6 m along the next segment, lerp fraction min(4*dt,1)
            // (0xF79B30). Translation continues to use the beam constraint.
            let wanted = next_segment(n.stand(), n.facing(), (n.p0, n.p1), &guidance).map(|(p0, p1)| {
                let axis = (p1 - p0).normalize_or_zero();
                let point = if axis.dot(n.facing()) >= 0.0 { p0 + axis * 0.6 } else { p1 - axis * 0.6 };
                (point - body.feet).with_y(0.0).normalize_or(n.facing())
            }).unwrap_or(n.facing());
            let dh = (super::heading_of(wanted) - body.heading + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            body.heading += dh * (4.0 * dt).min(1.0);
        } else { body.heading = super::heading_of(n.facing()); }
        // step off the end onto the floor beyond (0xF77C00) → Ground
        if n.kind == NarrowKind::Beam
            && matches!(n.state, BeamState::Walk | BeamState::Start)
            && n.to_end() <= BEAM_STEP_OFF + 1e-3
            && next_segment(n.stand(), n.facing(), (n.p0, n.p1), &guidance).is_none()
            && can_step_off(n, &collision)
        {
            if BEAM_COMPLETION {
                let n = &mut data.narrow;
                n.step_off_to = body.feet + n.facing() * 0.8;
                n.play(BeamState::StepOff, n.ground_step(jog));
            } else {
                body.feet += n.facing() * (BEAM_STEP_OFF + 0.2);
                switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
            }
        }
    }
}
