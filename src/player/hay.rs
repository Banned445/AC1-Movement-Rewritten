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
use crate::collision::{Aabb3, CollisionWorld};
use crate::input::PadInput;

pub const HAYSTACK_FAITH_LANDING: u32 = 0x23A9_666C;
pub const HAYSTACK_WAIT: u32 = 0x23A9_666D;
pub const HAYSTACK_FROM_AIR: u32 = 0x7750_D212;
pub const HAYSTACK_HOP_OUT: u32 = 0x2C4C_2431;

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
    pub stack: Aabb3,
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
}

#[derive(Debug, Default)]
pub struct HumanHayStackData {
    pub stack: Option<Aabb3>,
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
}

impl HumanHayStackData {
    pub fn enter(&mut self, e: HayStackEntry) {
        self.seq = self.seq.wrapping_add(1);
        self.stack = Some(e.stack);
        self.phase = HayPhase::Entering;
        self.kind = e.kind;
        self.t = 0.0;
        self.from = e.from;
        // the haystack entity's position: its base centre
        self.to = Vec3::new((e.stack.min.x + e.stack.max.x) * 0.5, e.stack.min.y, (e.stack.min.z + e.stack.max.z) * 0.5);
        self.face = None;
        match e.kind {
            HayEntry::Top | HayEntry::SideJump => {
                self.action = single(if e.kind == HayEntry::Top { HAYSTACK_FAITH_LANDING } else { HAYSTACK_FROM_AIR });
                let d = (self.to - e.from).length();
                self.interp = if e.speed > 5e-4 { (d / e.speed).clamp(0.1, 0.4) } else { 0.1 };
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
pub fn search(feet: Vec3, dir: Vec3, facing: Vec3, p: &HaySearch, stacks: &[Aabb3], collision: &CollisionWorld) -> Option<Aabb3> {
    const TOP: f32 = 1.6;
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
        let top = Vec3::new((s.min.x + s.max.x) * 0.5, s.min.y + TOP, (s.min.z + s.max.z) * 0.5);
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
pub fn find_support(feet: Vec3, dir: Vec3, facing: Vec3, stacks: &[Aabb3], collision: &CollisionWorld) -> Option<Aabb3> {
    search(feet, dir, facing, &SUPPORT, stacks, collision)
}

/// The haystack's top as the search reports it (the entity origin + 1.6 m).
pub fn top_of(s: &Aabb3) -> Vec3 {
    Vec3::new((s.min.x + s.max.x) * 0.5, s.min.y + 1.6, (s.min.z + s.max.z) * 0.5)
}

/// Ground event 122's guard (0xD8F0B0): a controller contact with a haystack (entity descriptor 8; 9 = hiding place),
/// the facing within 100 degrees of the contact (dot > -0.1736) and the stick within 45 degrees into it (dot >
/// 0.7071). PORT: haystacks are not solid in the port, so the contact is the capsule (radius 0.4) touching the
/// haystack's side at foot height.
pub fn ground_entry(feet: Vec3, facing: Vec3, stick: Vec3, stacks: &[Aabb3]) -> Option<HayStackEntry> {
    for s in stacks {
        if feet.y < s.min.y - 0.3 || feet.y > s.max.y - 0.3 {
            continue;
        }
        let q = Vec3::new(feet.x.clamp(s.min.x, s.max.x), feet.y, feet.z.clamp(s.min.z, s.max.z));
        let d = Vec3::new(feet.x - q.x, 0.0, feet.z - q.z);
        // a contact on the haystack's side: the body outside its footprint, within the capsule's radius
        if d.length() > 0.45 || d.length_squared() < 1e-8 {
            continue;
        }
        // the contact normal points from the haystack to the body
        let n = d.normalize();
        if (-n).dot(facing) <= -0.173_648_18 || (-n).dot(stick) <= std::f32::consts::FRAC_1_SQRT_2 {
            continue;
        }
        return Some(HayStackEntry { stack: *s, kind: HayEntry::Ground, from: feet, speed: 0.0 });
    }
    None
}

fn single(id: u32) -> Option<ActionBlend> {
    super::jump_blend::action_items(id).filter(|i| !i.is_empty()).map(|_| ActionBlend::new(id, 0, &[1.0]))
}

/// `Guard_HopOut` 0xE434E0 (reduced): where a hop out along `dir` leaves the stack, if there is room.
pub fn hop_out_exit(stack: &Aabb3, centre: Vec3, dir: Vec3, collision: &CollisionWorld) -> Option<Vec3> {
    let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    // ray from the centre to the footprint's boundary
    let tx = if dir.x.abs() > 1e-5 { ((if dir.x > 0.0 { stack.max.x } else { stack.min.x }) - centre.x) / dir.x } else { f32::INFINITY };
    let tz = if dir.z.abs() > 1e-5 { ((if dir.z > 0.0 { stack.max.z } else { stack.min.z }) - centre.z) / dir.z } else { f32::INFINITY };
    let edge = centre + dir * tx.min(tz);
    let probe = edge + dir * 0.5 + Vec3::Y * 1.25;
    if collision.point_inside(probe) || collision.point_inside(probe - Vec3::Y * 0.75) {
        return None;
    }
    let h = collision.ground_height(probe, 3.0)?;
    Some(Vec3::new(probe.x, h, probe.z))
}

pub fn update_hay(
    time: Res<Time>,
    pad: Res<PadInput>,
    collision: Res<CollisionWorld>,
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
                let dur = h.action.map(|a| a.duration()).unwrap_or(0.5);
                if h.t >= dur.max(h.interp) {
                    h.phase = HayPhase::Waiting;
                    h.action = single(HAYSTACK_WAIT);
                    h.t = 0.0;
                    h.seq = h.seq.wrapping_add(1);
                }
            }
            HayPhase::Waiting => {
                body.feet = h.to;
                // PORT: event 3 (hop out) comes from the untraced decision layer; the port sends it when the
                // stick is pushed after the wait started
                if pad.speed01 > 0.0 && h.t > 0.2 {
                    let Some(stack) = h.stack else { continue };
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
        }
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    /// RE/24 AIR-26 / CENSUS-1: the Leap of Faith search finds a haystack 10 m below and 6 m ahead, not one 2 m above
    /// (dz < 1.2), not one behind; a wall in the 3 m forward ray refuses everything.
    #[test]
    fn the_leap_of_faith_search_takes_the_box_the_window_and_the_rays() {
        let stack = |x: f32, y: f32, z: f32| Aabb3 { min: Vec3::new(x - 1.5, y, z - 1.5), max: Vec3::new(x + 1.5, y + 1.5, z + 1.5) };
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
