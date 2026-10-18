//! HumanHayStack (context 21): landing in a haystack after a Leap of Faith, waiting inside, hopping out
//! (RE/04 §4.1.12).
//!
//! - Entry (`HumanHayStack__Enter` 0xE43140, `HumanHayStackEntryType`, RE/18 §3):
//!   - **Top** (0): `EnterTop_FaithLanding` 0xE41D50 plays `0x23A9666C` `xx_h_faith_jump_landing` (blend 0.3 s).
//!     InAir gives it on every arrival at a haystack target (0xE07D00 clears InAir+340) and on a ballistic hit
//!     whose contact normal is more than 0.9 up (`CheckHayStackEntry` 0xE05490).
//!   - **SideJump** (3): `EnterFromAir` 0xE42700 plays `0x7750D212` `xx_h_air_to_haystack` (0.1 s): a ballistic hit
//!     on the side. Top and SideJump move the root to the haystack's position with the interpolator (`sub_711130`)
//!     over clamp(distance / speed, 0.1, 0.4) s.
//!   - **Ground** (2, 0xE42420) / **FreeStep** (1, 0xE42190): one of `0x244CD280` / `0x244CFACB`
//!     (`xx_h_freestep_footr_to_haystack_01/02`, picked by the game's LCG bit), the root interpolated to the
//!     haystack over the action's length; Ground turns to face the haystack, FreeStep keeps the facing. Ground is
//!     the Ground context's event 122 (IHumanGround vt1592 / vt1596, guard 0xD8F0B0). FreeStep comes from
//!     NarrowObject (`CheckSupportAndFall` 0xE51190: a haystack found by the support search while standing on a
//!     narrow object, once the action is past 0.6). Live (RE/18 §9): walking into a rimmed haystack hops onto its
//!     rim and drops in as FreeStep; the Leap of Faith lands as Top. PORT: the greybox haystack has no rim, so the
//!     port enters it by the Ground rule; the rim route is not ported.
//! - Wait (`ChooseWait` 0xE416B0 → `PlayWaitHigh` 0xE408C0): `0x23A9666D` `xx_h_haystack_wait`.
//! - Hop out: event 3 in the wait (`Wait_HandleEvent` 0xE43BD0), guard `Guard_HopOut` 0xE434E0: the ray along
//!   the wanted direction leaves the haystack's footprint; the exit point (+0.5 m along it, 1.25 m up) must
//!   have room for the body (sphere 0.35 / 0.5 / 0.75). `ToHopOut` 0xE41D00 → `PlayHopOut` 0xE41890 faces the
//!   direction and plays `0x2C4C2431` `xx_l_haystack_hop_out` (root motion), whose transitions lead to wait.

use bevy::prelude::*;

use super::jump_blend::ActionBlend;
use super::{switch_context, ActorContextId, Body, HumanDataBundle, Locomotion, Player, TransitionSetup};
use crate::collision::{CollisionWorld, HayStack};
use crate::guidance::GuidanceWorld;
use crate::input::PadInput;

pub const HAYSTACK_FAITH_LANDING: u32 = 0x23A9_666C;
pub const HAYSTACK_WAIT: u32 = 0x23A9_666D;
/// The low wait (event 1 → `sub_E41B00` → `sub_E40970`: `xx_l_haystack_wait`, blend 0.1 s; RE/24 NARROW-19).
pub const HAYSTACK_LOW_WAIT: u32 = 0x23A9_6674;
pub const HAYSTACK_FROM_AIR: u32 = 0x7750_D212;
pub const HAYSTACK_HOP_OUT: u32 = 0x2C4C_2431;
/// The pass-over from the wait (event 2 → `sub_E43090` → `HumanHayStack__ToPassOverJump` 0xE42B20:
/// `xx_l_haystack_wait_to_passover_handl`; RE/24 NARROW-19).
pub const HAYSTACK_TO_PASSOVER: u32 = 0x23A9_6673;

pub const HAYSTACK_DIVE: [u32; 2] = [0x244C_D280, 0x244C_FACB];

/// `HumanHayStackEntryType` (desc 0x195F010).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HayEntry {
    #[default]
    Top,
    FreeStep,
    Ground,
    SideJump,
}

#[derive(Clone, Copy, Debug)]
pub struct HayStackEntry {
    pub stack: HayStack,
    pub kind: HayEntry,
    pub from: Vec3,
    /// Speed at arrival (the interpolator time is distance / speed).
    pub speed: f32,
}

/// The game's random bit for the dive clip (`dword_1A1FC3C` LCG: x = 1664525·x + 1013904223, bit 0).
fn dive_pick(state: &mut u32) -> usize {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*state & 1) as usize
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HayPhase {
    #[default]
    Entering,
    Waiting,
    /// Event 2: `xx_l_haystack_wait_to_passover_handl` with the root moved linearly onto the pass-over candidate over
    /// the playing item (0xE42CDA); then Ledge HandPassOver (0xE41610 → fill 0xE40EE0: SubState 12, +144 = the
    /// candidate).
    PassOver,
}

#[derive(Debug, Default)]
pub struct HumanHayStackData {
    pub stack: Option<HayStack>,
    pub phase: HayPhase,
    /// The playing action (entry, then wait) and its time.
    pub action: Option<ActionBlend>,
    pub t: f32,
    pub from: Vec3,
    pub to: Vec3,
    /// Root interpolation time (0xE41D50: clamp(dist / speed, 0.1, 0.4)).
    pub interp: f32,
    pub seq: u32,
    /// The entry's facing (Ground: toward the haystack).
    pub face: Option<Vec3>,
    pub kind: HayEntry,
    rng: u32,
    /// In the low wait (+126, event 1).
    pub low: bool,
    /// The pass-over candidate (the wall top's near edge and its outward normal).
    pub pass: Option<(Vec3, Vec3)>,
}

impl HumanHayStackData {
    pub fn enter(&mut self, e: HayStackEntry) {
        self.seq = self.seq.wrapping_add(1);
        self.low = false;
        self.stack = Some(e.stack);
        self.phase = HayPhase::Entering;
        self.kind = e.kind;
        self.t = 0.0;
        self.from = e.from;
        // the haystack entity's position (its origin)
        self.to = e.stack.origin;
        self.face = None;
        match e.kind {
            HayEntry::Top | HayEntry::SideJump => {
                self.action = single(if e.kind == HayEntry::Top { HAYSTACK_FAITH_LANDING } else { HAYSTACK_FROM_AIR });
                let d = (self.to - e.from).length();
                self.interp = if e.speed > 5e-4 { (d / e.speed).clamp(0.1, 0.4) } else { 0.1 };
                // `EnterTop_FaithLanding` 0xE41D50: the target facing is the flat direction to the stack (RE/24 NARROW-20)
                if e.kind == HayEntry::Top {
                    let f = Vec3::new(self.to.x - e.from.x, 0.0, self.to.z - e.from.z).normalize_or_zero();
                    self.face = (f != Vec3::ZERO).then_some(f);
                }
            }
            HayEntry::Ground | HayEntry::FreeStep => {
                let id = HAYSTACK_DIVE[dive_pick(&mut self.rng)];
                self.action = single(id);
                self.interp = self.action.map(|a| a.duration()).unwrap_or(0.8).max(0.1);
                if e.kind == HayEntry::Ground {
                    let f = Vec3::new(self.to.x - e.from.x, 0.0, self.to.z - e.from.z).normalize_or_zero();
                    self.face = (f != Vec3::ZERO).then_some(f);
                }
            }
        }
    }
}

/// The parameters of `Human__sub_E15AA0` 0xE15AA0, the haystack search behind IHuman vt76 / vt80 / vt84 (RE/19 §1.2,
/// RE/24 CENSUS-1 / CENSUS-2 / AIR-26): the box half width and half depth (near, far), the height window (dz_min < dz <
/// dz_max, dz_far for the blend), the cone (near, far), the clearance rays' midpoint fraction and the forward ray.
#[derive(Clone, Copy, Debug)]
pub struct HaySearch {
    pub width: (f32, f32),
    pub depth: (f32, f32),
    pub dz_min: f32,
    pub dz_far: f32,
    pub dz_max: f32,
    pub cone: (f32, f32),
    pub frac: f32,
    pub pre_ray: bool,
}

/// vt76, the Leap of Faith search (`sub_B10A10` → 0xE17C90; floats 0xE17C94–0xE17D0A): 2 → 8 m wide, 9 → 20 m deep,
/// −100 < dz < 1.2 (blend to −30), 40° → 70°, 0.66, the 3 m forward ray.
pub const LEAP_OF_FAITH: HaySearch = HaySearch { width: (2.0, 8.0), depth: (9.0, 20.0), dz_min: -100.0, dz_far: -30.0, dz_max: 1.2, cone: (0.698_131_7, 1.221_730_5), frac: 0.66, pre_ray: true };
/// vt84, the ledge let-go's search (`sub_B10AB0` → 0xE17DA0): 1 → 5 m wide, 2 → 10 m deep, −100 < dz < 0 (blend to
/// −30), 90°, 0.1, no forward ray (RE/24 CENSUS-2).
pub const LET_GO: HaySearch = HaySearch { width: (1.0, 5.0), depth: (2.0, 10.0), dz_min: -100.0, dz_far: -30.0, dz_max: 0.0, cone: (std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2), frac: 0.1, pre_ray: false };
/// vt80, the free-step arrival's support search (`IHuman__FindHayStackSupport` 0xB10A60 → 0xE17D20): 3 m wide, 2.5 m
/// deep, 0 < dz < 1.5, 80°, 0.66, no forward ray.
pub const SUPPORT: HaySearch = HaySearch { width: (3.0, 3.0), depth: (2.5, 2.5), dz_min: 0.0, dz_far: 0.0, dz_max: 1.5, cone: (1.396_263_4, 1.396_263_4), frac: 0.66, pre_ray: false };

/// `Human__sub_E15AA0` 0xE15AA0. A haystack (or hiding place) qualifies when its top, the entity origin + 1.6 m, is
/// - within the height window: dz_min < dz < dz_max; the blend t = clamp((dz_max − dz) / (dz_max − dz_far), 0, 1) lerps
///   the limits from near to far;
/// - within ±width sideways and ±depth along `dir` in the frame of `dir`;
/// - in front of `facing` (the flat direction to it and the facing at least 90° apart is a reject, 0xE160BA);
/// - within the cone of `dir` seen from 1.5 m behind the feet (0xE1619B);
/// - reachable: a clear ray from 0.4 m above the feet to the feet + frac·(the flat offset to the top) + 0.4 m (RE/24
///   CENSUS-5: the midpoint has no height share), then to 0.25 m above the top (`sub_E1C470`, layer 5 / 21).
/// With `pre_ray`, a 3 m ray from the feet + 1 m along `dir` pitched 20° down must be clear first, else nothing
/// (0xE15DCD; the pitch's sign is a hypothesis). The first one that qualifies is taken (the entity list's order).
pub fn search(feet: Vec3, dir: Vec3, facing: Vec3, p: &HaySearch, stacks: &[HayStack], collision: &CollisionWorld) -> Option<HayStack> {
    let d = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
    if d == Vec3::ZERO {
        return None;
    }
    let right = super::right_of(d);
    let clear = |a: Vec3, b: Vec3| {
        let v = b - a;
        let len = v.length();
        len < 1e-4 || collision.ray_distance(a, v / len, len, crate::layers::MAIN_CHARACTER) >= len - 1e-4
    };
    if p.pre_ray {
        let down = Quat::from_axis_angle(right, -20f32.to_radians()) * d;
        let o = feet + Vec3::Y;
        if collision.ray_distance(o, down, 3.0, crate::layers::MAIN_CHARACTER) < 3.0 - 1e-4 {
            return None;
        }
    }
    let lerp = |(a, b): (f32, f32), t: f32| a + (b - a) * t;
    stacks.iter().copied().find(|s| {
        let top = s.top();
        let dz = top.y - feet.y;
        if !(dz > p.dz_min && dz < p.dz_max) {
            return false;
        }
        let span = p.dz_max - p.dz_far;
        let t = if span.abs() < 1e-6 { 0.0 } else { ((p.dz_max - dz) / span).clamp(0.0, 1.0) };
        let rel = top - feet;
        if right.dot(rel).abs() >= lerp(p.width, t) || d.dot(rel).abs() >= lerp(p.depth, t) {
            return false;
        }
        let flat = Vec3::new(rel.x, 0.0, rel.z);
        if flat.normalize_or_zero().dot(f) < 0.0 {
            return false;
        }
        let from_behind = Vec3::new(top.x - (feet.x - d.x * 1.5), 0.0, top.z - (feet.z - d.z * 1.5)).normalize_or_zero();
        if from_behind == Vec3::ZERO || from_behind.dot(d).clamp(-1.0, 1.0).acos() >= lerp(p.cone, t) {
            return false;
        }
        let a = feet + Vec3::Y * 0.4;
        let mid = feet + flat * p.frac + Vec3::Y * 0.4;
        clear(a, mid) && clear(mid, top + Vec3::Y * 0.25)
    })
}

/// The free-step arrival's haystack search (`HumanNarrowObject__CheckSupportAndFall` 0xE51190 → vt80): `SUPPORT`.
pub fn find_support(feet: Vec3, dir: Vec3, facing: Vec3, stacks: &[HayStack], collision: &CollisionWorld) -> Option<HayStack> {
    search(feet, dir, facing, &SUPPORT, stacks, collision)
}

/// The haystack's top as the search reports it (the entity origin + 1.6 m).
pub fn top_of(s: &HayStack) -> Vec3 {
    s.top()
}

/// The controller's contacts with a haystack: the haystack's collision (a box on layer Dynamic) whose contact point lies
/// in a haystack volume (the game: the contact's entity is type 3 with descriptor 8, a haystack, or 9, a hiding place).
/// Returns the stack and the contact normal (from the haystack toward the body) of each.
pub fn haystack_contacts<'a>(manifold: &'a [crate::proxy::Contact], stacks: &'a [HayStack], collision: &'a CollisionWorld) -> impl Iterator<Item = (HayStack, Vec3)> + 'a {
    manifold.iter().filter(|c| collision.layer_of(c.body) == crate::layers::DYNAMIC).filter_map(|c| {
        let s = stacks.iter().find(|s| s.contains(c.pos, 0.1))?;
        Some((*s, c.normal))
    })
}

/// `HumanInAir__CheckHayStackEntry` 0xE05490's dive branch (the faith dive playing, IHumanInAir vt72): a ray from the
/// controller's last position along this frame's step, three times its length (`Human__RayClear`, layer Character); a
/// hit near the known haystack enters it. PORT: the ray is tested against the haystacks' collision boxes.
pub fn dive_hit(from: Vec3, to: Vec3, stacks: &[HayStack]) -> Option<HayStack> {
    let d = (to - from) * 3.0;
    if d.length_squared() < 1e-10 {
        return None;
    }
    stacks.iter().copied().find(|s| s.segment_hits(from, d))
}

/// Ground event 122's guard (`HumanGround__Guard_Event122_HayStack` 0xD8F0B0): a wall-class contact (`sub_4F8C40`(4):
/// |normal z| <= 0.707) with a haystack, the facing within 100 degrees of the flattened contact (-n . facing > -0.1736)
/// and the stick within 45 degrees into it (-n . stick > 0.7071). Standing on top is a ground contact: no entry there.
pub fn ground_entry(feet: Vec3, facing: Vec3, stick: Vec3, manifold: &[crate::proxy::Contact], stacks: &[HayStack], collision: &CollisionWorld) -> Option<HayStackEntry> {
    haystack_contacts(manifold, stacks, collision).find_map(|(s, n)| {
        if n.y.abs() > std::f32::consts::FRAC_1_SQRT_2 {
            return None;
        }
        let n = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        if (-n).dot(facing) <= -0.173_648_18 || (-n).dot(stick) <= std::f32::consts::FRAC_1_SQRT_2 {
            return None;
        }
        Some(HayStackEntry { stack: s, kind: HayEntry::Ground, from: feet, speed: 0.0 })
    })
}

fn single(id: u32) -> Option<ActionBlend> {
    super::jump_blend::action_items(id).filter(|i| !i.is_empty()).map(|_| ActionBlend::new(id, 0, &[1.0]))
}

/// `Guard_HopOut` 0xE434E0: the ray along `dir` from the root leaves the stack's footprint; the room is
/// `Human__BoxQueryEmpty` at the exit + 0.5·dir, 1.25 m above the root, half extents (0.35, 0.5, 0.75) across / along /
/// up, the stack itself ignored (the port's haystacks are not solid), filter 8; no ground needed (RE/24 NARROW-20).
/// Returns where the hop leaves (at the root's height). The footprint is the collision shape's local box placed by
/// the body's transform (0xE434E0: `sub_6D1D40` / `sub_6D1DB0` / `sub_6D22A0`, four `sub_DEE760` edges).
pub fn hop_out_exit(stack: &HayStack, centre: Vec3, dir: Vec3, collision: &CollisionWorld) -> Option<Vec3> {
    let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    // ray from the centre to the footprint's boundary
    let edge = stack.footprint_exit(centre, dir);
    let probe = Vec3::new(edge.x, centre.y, edge.z) + dir * 0.5;
    // the query leaves the stack itself out (its collision: boxes and triangles inside its bounds)
    let (lo, hi) = (stack.bounds.min - Vec3::splat(0.01), stack.bounds.max + Vec3::splat(0.01));
    let room = collision.obb_free_except(probe + Vec3::Y * 1.25, [super::right_of(dir), dir, Vec3::Y], Vec3::new(0.35, 0.5, 0.75), |b| b.min.cmpge(lo).all() && b.max.cmple(hi).all());
    room.then_some(probe)
}

pub fn update_hay(
    time: Res<Time>,
    pad: Res<PadInput>,
    collision: Res<CollisionWorld>,
    guidance: Res<GuidanceWorld>,
    mut q: Query<(&mut Locomotion, &mut Body, &mut HumanDataBundle), With<Player>>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (mut loco, mut body, mut data) in &mut q {
        if loco.current != ActorContextId::HayStack {
            continue;
        }
        if loco.just_switched {
            loco.just_switched = false;
            if loco.switched_this_frame {
                continue;
            }
        }
        let h = &mut data.hay;
        h.t += dt;
        body.velocity = Vec3::ZERO;
        body.grounded = true;
        match h.phase {
            HayPhase::Entering => {
                body.feet = h.from.lerp(h.to, (h.t / h.interp).min(1.0));
                if let Some(f) = h.face {
                    body.heading = super::heading_of(f);
                }
                // the Top landing's timer runs the action + 0.2 s (0xE41D50, RE/24 NARROW-20)
                let extra = if h.kind == HayEntry::Top { 0.2 } else { 0.0 };
                let dur = h.action.map(|a| a.duration()).unwrap_or(0.5) + extra;
                if h.t >= dur.max(h.interp) {
                    // `HumanHayStack__ChooseWait` 0xE416B0: with HumanData +871 set (the player's, 1 from
                    // `BhvAssassin__SetupPlayerHuman` 0xAF2A7F) the wait faces the haystack entity's X row, flattened
                    // (0xE416F0–0xE41795, `sub_52BD40` at once); else IHumanHayStack vt144's direction. Lab hay43 let-go /
                    // faith landing: whatever the entry's heading, the wait faces (0.724, 0.690)
                    if let Some(stack) = h.stack {
                        let x = Vec3::new(stack.axes[0].x, 0.0, stack.axes[0].z).normalize_or_zero();
                        if x != Vec3::ZERO {
                            h.face = Some(x);
                            body.heading = super::heading_of(x);
                        }
                    }
                    h.phase = HayPhase::Waiting;
                    h.action = single(HAYSTACK_WAIT);
                    h.t = 0.0;
                    h.seq = h.seq.wrapping_add(1);
                }
            }
            HayPhase::Waiting => {
                body.feet = h.to;
                // the interpreter's haystack state (`sub_EE08E0` from 0xEEE4E0, RE/24 NARROW-19): the stick released in
                // the high wait → event 1, the low wait (0xE44020 / 0xE44080); the stick past 0.35 → the pass-over to a
                // pass-over candidate (event 2) if there is one, else the hop out (event 3, `Guard_HopOut`). Event 0 (back
                // to the high wait, `sub_E41B60`) is IHumanHayStack vt108 / vt112 (0xE43FB0 / 0xE44010, vtable 0x17022E4);
                // the player's interpreter calls only vt116 / vt120 (event 1) and vt144, so the player never sends it
                // (the lab's haystack waits never return to the high wait)
                if pad.magnitude <= 0.35 && !h.low {
                    h.low = true;
                    h.action = single(HAYSTACK_LOW_WAIT).or(h.action);
                    h.t = 0.0;
                    h.seq = h.seq.wrapping_add(1);
                }
                if pad.magnitude > 0.35 {
                    let Some(stack) = h.stack else { continue };
                    // event 2 (`sub_EDD5E0`): IHuman vt56 along the stick with request type 4 (the free run's, beams on)
                    // and the scorer in mode 2 (the nearest ledge-like candidate in front, jumped as a pass-over, type 2)
                    if let Some((edge, normal)) = pass_over_candidate(h.to, pad.dir, &guidance, &collision) {
                        h.phase = HayPhase::PassOver;
                        h.pass = Some((edge, normal));
                        h.action = single(HAYSTACK_TO_PASSOVER);
                        h.from = body.feet;
                        h.interp = h.action.map(|a| a.duration()).unwrap_or(0.5).max(0.05);
                        h.t = 0.0;
                        h.seq = h.seq.wrapping_add(1);
                        let f = Vec3::new(edge.x - body.feet.x, 0.0, edge.z - body.feet.z).normalize_or_zero();
                        h.face = (f != Vec3::ZERO).then_some(f);
                        continue;
                    }
                    if hop_out_exit(&stack, h.to, pad.dir, &collision).is_some() {
                        body.heading = super::heading_of(pad.dir);
                        let hop = single(HAYSTACK_HOP_OUT);
                        switch_context(&mut loco, &mut data, TransitionSetup::ToMovement { landing: None });
                        if let Some(b) = hop {
                            data.ground.play_oneshot(b);
                        }
                    }
                }
            }
            HayPhase::PassOver => {
                // 0xE42CDA: the root linear onto the candidate over the playing item, facing it; done → Ledge
                // HandPassOver (0xE41610 → 0xE40EE0) with the vault of the planted left hand
                let Some((edge, normal)) = h.pass else { continue };
                body.feet = h.from.lerp(edge, (h.t / h.interp).min(1.0));
                if let Some(f) = h.face {
                    body.heading = super::heading_of(f);
                }
                if h.t >= h.interp {
                    let reception = h.action;
                    let e = super::passover::PassOverEntry { edge, normal, from: edge, flight_foot_left: false, flight_w: [0.0; 5], reception };
                    body.feet = edge;
                    switch_context(&mut loco, &mut data, TransitionSetup::ToPassOver(e));
                }
            }
        }
    }
}

/// `sub_EDD5E0` (the haystack's event 2): IHuman vt56 `Human__FindJumpCandidates` along `dir` with request type 4 and
/// beams (vt56 a5 → `Human__QueryJumpCandidates` a8), then `Human__SelectJumpCandidate` in mode 2 (the nearest
/// ledge-like candidate in front, jumped as type 2). Returns the pass-over edge and its outward normal.
pub fn pass_over_candidate(feet: Vec3, dir: Vec3, guidance: &GuidanceWorld, collision: &CollisionWorld) -> Option<(Vec3, Vec3)> {
    use super::jump_candidates::{query, select, Query};
    let q = Query { kind: 4, mode: 0, long: false, beams: true, exclude: 0 };
    let cands = query(feet, dir, &q, guidance, collision);
    let (i, ty) = select(&cands, feet, dir, dir, 2, false)?;
    super::targets::to_target(&cands[i], ty, feet, guidance, collision)?.pass
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use crate::collision::Aabb3;

    /// RE/24 NARROW-20: the hop out needs only the room box past the stack's side, no ground: an open drop is fine, a
    /// wall right beside the stack is not.
    #[test]
    fn the_hop_out_needs_the_room_box_not_ground() {
        let stack = HayStack::from_box(Aabb3 { min: Vec3::new(-1.0, 0.0, -1.0), max: Vec3::new(1.0, 1.5, 1.0) });
        let centre = Vec3::ZERO;
        assert!(hop_out_exit(&stack, centre, Vec3::X, &CollisionWorld::default()).is_some(), "nothing beyond: room");
        let mut c = CollisionWorld::default();
        c.boxes.push(Aabb3 { min: Vec3::new(1.3, 0.0, -2.0), max: Vec3::new(2.0, 3.0, 2.0) });
        assert!(hop_out_exit(&stack, centre, Vec3::X, &c).is_none(), "a wall 0.3 m past the side");
        assert!(hop_out_exit(&stack, centre, Vec3::NEG_X, &c).is_some(), "the other side is open");
    }

    /// RE/24 AIR-26 / CENSUS-1: the Leap of Faith search finds a haystack 10 m below and 6 m ahead, not one 2 m above
    /// (dz < 1.2), not one behind; a wall in the 3 m forward ray refuses everything.
    #[test]
    fn the_leap_of_faith_search_takes_the_box_the_window_and_the_rays() {
        let stack = |x: f32, y: f32, z: f32| HayStack::from_box(Aabb3 { min: Vec3::new(x - 1.5, y, z - 1.5), max: Vec3::new(x + 1.5, y + 1.5, z + 1.5) });
        let c = CollisionWorld::default();
        let feet = Vec3::new(0.0, 10.0, 0.0);
        let below = [stack(0.0, 0.0, -6.0)];
        assert!(search(feet, Vec3::NEG_Z, Vec3::NEG_Z, &LEAP_OF_FAITH, &below, &c).is_some());
        assert!(search(feet, Vec3::NEG_Z, Vec3::NEG_Z, &LEAP_OF_FAITH, &[stack(0.0, 10.4, -4.0)], &c).is_none(), "too high");
        assert!(search(feet, Vec3::Z, Vec3::Z, &LEAP_OF_FAITH, &below, &c).is_none(), "behind");
        let mut wall = CollisionWorld::default();
        wall.boxes.push(Aabb3 { min: Vec3::new(-3.0, 10.0, -1.5), max: Vec3::new(3.0, 14.0, -1.0) });
        assert!(search(feet, Vec3::NEG_Z, Vec3::NEG_Z, &LEAP_OF_FAITH, &below, &wall).is_none(), "the forward ray is blocked");
        // the first clearance ray's end has no height share (CENSUS-5): a haystack level with the feet just ahead
        let level = [stack(0.0, -1.0, -2.0)];
        assert!(search(Vec3::ZERO, Vec3::NEG_Z, Vec3::NEG_Z, &SUPPORT, &level, &c).is_some());
    }
}
