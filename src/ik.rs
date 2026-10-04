//! Limb IK: pins hands and feet onto the holds / ledge edges chosen by the Ledge and Climb contexts.
//!
//! Game behaviour (RE/11, limb-IK component at Human+1328, `LimbIK__SolveEffectors` 0xE57570):
//! - four limbs (0 L hand, 1 R hand, 2 L foot, 3 R foot), each with a blend weight that rises at
//!   4/s while the limb has a contact (0.25 s fade-in) and falls at 5/s once released (0.2 s);
//! - a limb moving to a new hold travels from the old contact to the new one over the move's
//!   window: goal = lerp(old + anim delta, new, s), where the anim delta is the limb's own motion in
//!   the playing clip (`LimbIK__AccumAnimEffectorDelta` 0xE56450);
//! - the goals are handed to Autodesk HumanIK (`IKRig__Solve` 0x4FADA0 → 0x10C68CC) as wrist / ankle effectors with
//!   translation reach = the weight and **no pull** (the pull setter is never called; all 22 effectors start at reach
//!   0 / pull 0, `IKRig__Init` 0x4FB0E0): the limbs reach, the body is not dragged towards them;
//! - the body moves only through the hips / chest effectors of the post-adjustments (flags LimbIK +656, both set
//!   while climbing): `ADJUST_SLOPE` lowers the hips when the hands lean out beyond the feet, `ADJUST_HEAD` ducks the
//!   head under an overhang and leans the chest back from a wall in front of the face (RE/11 §6).
//!
//! The port solves each limb analytically (two-bone, keeping the animation's bend plane), after which the hand/foot
//! keeps its **animated world orientation** (the grip/finger pose stays as authored). PORT: HumanIK's own chain
//! solve (shoulders, spine share) is not reproduced.

use bevy::prelude::*;

use crate::model::Rig;
use crate::player::{Body, LimbTargets};
use crate::tuning::*;

/// Bone ids (CRC32 of the bone name, RE/data/altair_bone_names.json): (upper, middle, end).
pub const LIMB_CHAINS: [(u32, u32, u32); 4] = [
    (0xeb83_0ada, 0x89b9_3a80, 0xb675_f36c), // LeftArm, LeftForeArm, LeftHand
    (0x6bb3_f727, 0x7257_a1aa, 0x75f9_4d30), // RightArm, RightForeArm, RightHand
    (0x1761_83f0, 0x060d_f401, 0x5898_8870), // LeftUpLeg, LeftLeg, LeftFoot
    (0x757f_1291, 0x863d_09fc, 0x9b14_362c), // RightUpLeg, RightLeg, RightFoot
];

/// Per-limb runtime state (weight + travel between holds).
#[derive(Clone, Copy, Debug, Default)]
pub struct LimbState {
    pub weight: f32,
    /// Current IK goal (contact point, world space).
    pub goal: Vec3,
    from: Vec3,
    to: Vec3,
    /// Animated contact position when the travel started (for the anim delta).
    anim_from: Vec3,
    t: f32,
    dur: f32,
    has_goal: bool,
    /// Tag-driven: inside a travel window.
    travel: bool,
    /// Tag-driven: seconds the hold has differed from the contact without a release.
    wait: f32,
}

/// What the playing clip's contact tag says about one limb this frame (LimbIK__UpdateContactsFromAnimTags
/// 0xE56FC0): in contact, between contacts (travel window [t0, t1], `s` = progress), or released.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tag {
    On,
    Travel { s: f32 },
    Off,
}

/// Contact bit of each limb in `AcuatorsContactsTypes`: L hand 4, R hand 5, L toes 2, R toes 3.
pub const LIMB_CONTACT_BIT: [usize; 4] = [4, 5, 2, 3];

pub fn limb_tag(c: &crate::anim::ContactState, limb: usize) -> Option<Tag> {
    if !c.tagged {
        return None;
    }
    let b = LIMB_CONTACT_BIT[limb];
    if c.bits >> b & 1 != 0 {
        return Some(Tag::On);
    }
    match (c.off_since[b], c.next_on[b]) {
        (Some(t0), Some(t1)) if t1 > t0 => Some(Tag::Travel { s: ((c.t - t0) / (t1 - t0)).clamp(0.0, 1.0) }),
        _ => Some(Tag::Off),
    }
}

impl LimbState {
    /// Tag-driven update (game rules, RE/11 §2 + RE/13 §5). `target` is the context's hold; `anim` the
    /// clip's own contact point this frame.
    pub fn update_tagged(&mut self, target: Option<Vec3>, anim: Vec3, tag: Tag, dt: f32) -> Option<Vec3> {
        let Some(p) = target else { return self.update(None, anim, 0.0, dt) };
        match tag {
            Tag::Off => {
                // released by the animation: fade out from where the limb let go
                self.weight = (self.weight - dt * IK_WEIGHT_OUT_RATE).max(0.0);
                self.dur = 0.0;
                self.travel = false;
            }
            Tag::On => {
                self.weight = (self.weight + dt * IK_WEIGHT_IN_RATE).min(1.0);
                if !self.has_goal {
                    *self = LimbState { weight: self.weight, goal: p, from: p, to: p, anim_from: anim, has_goal: true, ..Default::default() };
                } else if self.travel {
                    // contact made: the limb is on its new hold
                    self.goal = self.to;
                    self.travel = false;
                    self.wait = 0.0;
                }
                if (p - self.goal).length() > IK_RETARGET_DIST {
                    // a new hold chosen while the limb is still in contact: it stays on the old one until
                    // the animation releases it. PORT: if this clip never releases it, settle after a wait.
                    self.wait += dt;
                    if self.wait > IK_CONTACT_WAIT {
                        self.goal = self.goal.lerp(p, (dt * 8.0).min(1.0));
                    }
                } else {
                    self.goal = p;
                    self.wait = 0.0;
                }
                self.to = self.goal;
            }
            Tag::Travel { s } => {
                if !self.travel {
                    // contact released with a re-contact ahead: travel from here to the new hold (a limb
                    // that never had a contact starts at the hold)
                    self.travel = true;
                    self.from = if self.has_goal { self.goal } else { p };
                    self.anim_from = anim;
                }
                self.to = p;
                self.weight = (self.weight + dt * IK_WEIGHT_IN_RATE).min(1.0);
                self.has_goal = true;
                let followed = self.from + (anim - self.anim_from);
                self.goal = followed.lerp(self.to, s);
            }
        }
        (self.weight > 0.0).then_some(self.goal)
    }
    /// Advance one frame towards `target` (None = released). `anim` is where the clip puts this
    /// limb's contact point this frame. Returns the goal while weighted.
    pub fn update(&mut self, target: Option<Vec3>, anim: Vec3, transit: f32, dt: f32) -> Option<Vec3> {
        match target {
            Some(p) => {
                // 0xE57570: weight += dt * 4, clamped to 1
                self.weight = (self.weight + dt * IK_WEIGHT_IN_RATE).min(1.0);
                if !self.has_goal {
                    // new contact: no travel, the weight fade carries the limb in
                    *self = LimbState { weight: self.weight, goal: p, from: p, to: p, anim_from: anim, t: 0.0, dur: 0.0, has_goal: true, travel: false, wait: 0.0 };
                } else if (p - self.to).length() > IK_RETARGET_DIST {
                    // the limb moves to a new hold: travel from where it is now
                    self.from = self.goal;
                    self.to = p;
                    self.anim_from = anim;
                    self.t = 0.0;
                    self.dur = transit.max(1e-3);
                }
                if self.dur > 0.0 {
                    self.t = (self.t + dt).min(self.dur);
                    let s = self.t / self.dur;
                    // goal = lerp(old + anim delta, new, s): the clip's reach arc, corrected onto the hold
                    let followed = self.from + (anim - self.anim_from);
                    self.goal = followed.lerp(self.to, s);
                    if self.t >= self.dur {
                        self.dur = 0.0;
                        self.goal = self.to;
                    }
                } else {
                    self.goal = self.to;
                }
            }
            None => {
                // released: weight -= dt * 5; the goal stays where the limb let go
                self.weight = (self.weight - dt * IK_WEIGHT_OUT_RATE).max(0.0);
                self.has_goal = false;
            }
        }
        (self.weight > 0.0).then_some(self.goal)
    }
}

/// LimbIK +656 bit 0 (value 1): lower the hips when the hand line leans out (0xE58333).
pub const ADJUST_SLOPE: u8 = 1;
/// LimbIK +656 bit 1 (value 2): head clearance: duck under an overhang, lean the chest back from a wall (0xE58799).
pub const ADJUST_HEAD: u8 = 2;

/// Bone ids: Hips, Spine, Head (CRC32 of the names; the exe queries the same hashes in 0xE57570).
const HIPS: u32 = 0xded1_0611;
const SPINE: u32 = 0x530e_c1cb;
const HEAD: u32 = 0x07c1_59a2;

#[derive(Component, Default)]
pub struct LimbIk {
    pub limbs: [LimbState; 4],
    /// Last hips translation of the post-adjustments (world), for the log.
    pub fit: Vec3,
    /// LimbIK +660: the head-clearance hips drop (m), +664: the chest lean (0..0.5), both moving at 1 /s.
    pub hips_drop: f32,
    pub lean: f32,
    /// Hips, Spine, Head joint indices.
    body_bones: Option<[usize; 3]>,
    /// The standing foot IK (`IKGroundBiped`): left and right foot, and the pelvis drop (m, ≤ 0).
    pub ground: [GroundFoot; 2],
    pub pelvis: f32,
    /// Joint indices of each chain (resolved once from the rig).
    chains: Option<[[usize; 3]; 4]>,
}

impl LimbIk {
    /// Joint indices (upper, middle, end) of each limb chain, once resolved and complete.
    pub fn chains(&self) -> Option<[[usize; 3]; 4]> {
        self.chains.filter(|c| c.iter().all(|l| l[0] != usize::MAX))
    }
}

pub struct IkPlugin;

impl Plugin for IkPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (add_limb_ik, solve_limbs).chain().after(crate::anim::apply_clip));
        if std::env::var_os("AC_IK_LOG").is_some() {
            app.add_systems(PostUpdate, log_ik_error.after(bevy::transform::TransformSystems::Propagate));
        }
    }
}

fn add_limb_ik(mut commands: Commands, q: Query<Entity, (With<Rig>, Without<LimbIk>)>) {
    for e in &q {
        commands.entity(e).insert(LimbIk::default());
    }
}

/// Rigid transform (the rig has unit scale everywhere).
#[derive(Clone, Copy)]
struct Iso {
    rot: Quat,
    pos: Vec3,
}

impl Iso {
    fn mul(&self, t: &Transform) -> Iso {
        Iso { rot: self.rot * t.rotation, pos: self.pos + self.rot * t.translation }
    }
}

/// Where the IK end bone (wrist / ankle) goes for a contact point, in world space. The offsets come
/// from the game's hang/climb clips (see tuning.rs): the wrist hangs 0.1 m below the edge point.
pub fn effector_goal(limb: usize, contact: Vec3, normal: Vec3) -> Vec3 {
    if limb < 2 {
        contact + Vec3::Y * IK_HAND_DROP + normal * IK_HAND_OUT
    } else {
        contact + Vec3::Y * IK_FOOT_UP + normal * IK_FOOT_OUT
    }
}

/// Inverse of `effector_goal`: the contact point an end bone at `p` corresponds to.
fn contact_of(limb: usize, p: Vec3, normal: Vec3) -> Vec3 {
    p - (effector_goal(limb, Vec3::ZERO, normal))
}

/// PORT: ankle height above the sole (xx_h_wait_hipm: ankles ~0.09 m above the animation origin).
const ANKLE_HEIGHT: f32 = 0.09;

/// One foot of the game's standing foot IK (`scimitar::FootIK`, 720 bytes, two of them in `IKGroundBiped` 0x430CA0).
/// `state` follows the exe's FootIK +4: 0 off, 1 fading in, 2 running, 3 fading out (timer +64 / length +68).
#[derive(Clone, Copy, Debug, Default)]
pub struct GroundFoot {
    pub state: u8,
    t: f32,
    dur: f32,
    /// Ankle height where the fade started (+56 fade in, +88 fade out).
    from: f32,
    /// The ankle height on the ground under the foot (+40), and the smoothed one used while running.
    pub target: f32,
    cur: f32,
    /// The ankle height handed to the leg solve this frame.
    pub goal: f32,
}

/// `IKGroundBiped` fade time (`sub_42D5E0(0.2)`, `sub_42F290(…, 0.2)`).
const GROUND_IK_FADE: f32 = 0.2;

impl GroundFoot {
    fn fade_in(&mut self, from: f32) {
        // sub_42F290: from off or while fading out
        if matches!(self.state, 0 | 3) {
            self.state = 1;
            (self.t, self.dur, self.from) = (0.0, GROUND_IK_FADE, from);
        }
    }
    fn fade_out(&mut self) {
        // sub_42D5E0: unless off, or already fading out with less than the new time left
        if self.state != 0 && (self.state != 3 || self.dur - self.t > GROUND_IK_FADE) {
            (self.t, self.dur, self.from) = (0.0, GROUND_IK_FADE, self.goal);
            self.state = 3;
        }
    }
    /// `sub_42F130` + `sub_42EFC0`: advance the timer and give the ankle height for this frame (None when off).
    fn advance(&mut self, anim: f32, dt: f32) -> Option<f32> {
        if self.state != 0 {
            self.t += dt;
            if self.t >= self.dur {
                (self.t, self.dur) = (0.0, 0.0);
                self.state = match self.state {
                    1 => {
                        self.cur = self.target;
                        2
                    }
                    3 => 0,
                    s => s,
                };
            }
        }
        let s = if self.dur > 0.0 { self.t / self.dur } else { 1.0 };
        self.goal = match self.state {
            1 => self.from + (self.target - self.from) * s,
            // running: the ankle follows the ground under it, smoothed (sub_42E7E0 rate 2; PORT: exponential)
            2 => {
                self.cur += (self.target - self.cur) * (1.0 - (-2.0 * dt).exp());
                self.cur
            }
            3 => self.from + (anim - self.from) * s,
            _ => return None,
        };
        Some(self.goal)
    }
}

/// `sub_432940`: the ground under an animated ankle, from 0.5 m above it to 0.5 m below (`sub_432570`, straight down),
/// as an ankle height (+ the ankle's height above the root). None when nothing is hit, or when the knee would end up
/// less than 0.15 m above the new ankle.
fn ground_socket(c: &crate::collision::CollisionWorld, ankle: Vec3, knee: Vec3, ankle_height: f32) -> Option<f32> {
    // a ray (not the controller's footprint): the highest top under the ankle's own x / z
    let (top, bottom) = (ankle.y + 0.5, ankle.y - 0.5);
    let h = c
        .boxes
        .iter()
        .filter(|b| ankle.x >= b.min.x && ankle.x <= b.max.x && ankle.z >= b.min.z && ankle.z <= b.max.z)
        .map(|b| b.max.y)
        .filter(|y| *y <= top && *y >= bottom)
        .fold(None, |m: Option<f32>, y| Some(m.map_or(y, |m| m.max(y))))?;
    let target = h + ankle_height;
    (knee.y - target >= 0.15).then_some(target)
}

/// PORT stand-in, outside the standing foot IK: a foot the animation puts below the surface under it is lifted onto
/// it. Without it the flight clips' reaching leg went through the roof on landings.
fn ground_foot_target(ankle: Vec3, collision: &crate::collision::CollisionWorld) -> Option<Vec3> {
    let h = collision.ground_height(Vec3::new(ankle.x, ankle.y + 0.5, ankle.z), 1.0)?;
    let want = h + ANKLE_HEIGHT;
    (ankle.y < want - 1e-3).then_some(Vec3::new(ankle.x, want, ankle.z))
}

fn solve_limbs(
    time: Res<Time>,
    collision: Option<Res<crate::collision::CollisionWorld>>,
    mut q: Query<(&Rig, &Body, &LimbTargets, &mut LimbIk, Option<&crate::anim::AnimPlayer>)>,
    mut joints: Query<&mut Transform>,
) {
    let dt = time.delta_secs().min(1.0 / 20.0);
    for (rig, body, targets, mut ik, player) in &mut q {
        let contacts = player.map(|p| p.contacts).unwrap_or_default();
        if ik.chains.is_none() {
            let find = |id: u32| rig.bone_ids.iter().position(|b| *b == id);
            let mut chains = [[usize::MAX; 3]; 4];
            for (i, (a, b, c)) in LIMB_CHAINS.iter().enumerate() {
                if let (Some(a), Some(b), Some(c)) = (find(*a), find(*b), find(*c)) {
                    chains[i] = [a, b, c];
                }
            }
            if chains.iter().any(|c| c[0] == usize::MAX) {
                warn!("limb IK: limb bones not found in the skeleton; IK disabled");
            }
            ik.chains = Some(chains);
        }
        let chains = ik.chains.unwrap();
        if chains.iter().any(|c| c[0] == usize::MAX) {
            continue;
        }
        if std::env::var_os("AC_NO_IK").is_some() {
            continue;
        }

        // ---------------------------------------------------------------- world pose of the rig (animated)
        let player = Iso { rot: body.tilt * Quat::from_rotation_y(body.heading), pos: body.feet };
        let Ok(root_t) = joints.get(rig.root).copied() else { continue };
        let root = player.mul(&root_t);
        let mut global: Vec<Iso> = Vec::with_capacity(rig.joints.len());
        for (i, e) in rig.joints.iter().enumerate() {
            let l = joints.get(*e).copied().unwrap_or_default();
            let parent = rig.parents[i].map(|p| global[p]).unwrap_or(root);
            global.push(parent.mul(&l));
        }

        // ---------------------------------------------------------------- limb goals
        let n = targets.normal;
        let wanted = [
            targets.hands.map(|h| h.0),
            targets.hands.map(|h| h.1),
            targets.feet.map(|f| f.0),
            targets.feet.map(|f| f.1),
        ];
        let mut goals: [Option<(Vec3, f32)>; 4] = [None; 4];
        for i in 0..4 {
            let anim_contact = contact_of(i, global[chains[i][2]].pos, n);
            // the clip's contact tags decide attach / travel / release when the clip has them (game);
            // otherwise the context's timing is used
            let g = match limb_tag(&contacts, i) {
                Some(tag) => ik.limbs[i].update_tagged(wanted[i], anim_contact, tag, dt),
                None => ik.limbs[i].update(wanted[i], anim_contact, targets.transit, dt),
            };
            goals[i] = g.map(|g| (g, ik.limbs[i].weight));
        }
        // feet without a hold: kept above the surface under them
        let mut ground_fix: [Option<Vec3>; 4] = [None; 4];
        if let Some(c) = collision.as_deref() {
            for limb in 2..4 {
                if goals[limb].is_none() {
                    ground_fix[limb] = ground_foot_target(global[chains[limb][2]].pos, c);
                }
            }
        }
        // ---------------------------------------------------------------- standing foot IK (IKGroundBiped 0x432AC0)
        // Only while standing still on the ground (speed ≤ 0.001; moving fades both feet out over 0.2 s). Each foot
        // looks for the ground under it; the IK runs only when the ground is uneven (feet ≥ 5 cm apart in height, or
        // the left foot ≥ 5 cm off the root), the pelvis drops for the lower foot (≤ 1.5 m/s) and each leg is solved
        // onto its ankle height.
        let standing = body.grounded && body.velocity.length() <= 0.001 && targets.feet.is_none() && targets.hands.is_none();
        let mut ground_goal: [Option<f32>; 2] = [None; 2];
        if let Some(c) = collision.as_deref() {
            let ankle = [global[chains[2][2]].pos, global[chains[3][2]].pos];
            let knee = [global[chains[2][1]].pos, global[chains[3][1]].pos];
            if standing {
                let sockets: [Option<f32>; 2] = std::array::from_fn(|f| ground_socket(c, ankle[f], knee[f], (ankle[f].y - body.feet.y).max(0.0)));
                let z = [sockets[0].unwrap_or(ankle[0].y), sockets[1].unwrap_or(ankle[1].y)];
                let (ah0, ah1) = (ankle[0].y - body.feet.y, ankle[1].y - body.feet.y);
                let too_far = (ankle[0].y - ankle[1].y).abs() > 0.6;
                if !too_far && ((z[0] - ah0 - (z[1] - ah1)).abs() >= 0.05 || (z[0] - ah0 - body.feet.y).abs() >= 0.05) {
                    for f in 0..2 {
                        if let Some(t) = sockets[f] {
                            ik.ground[f].target = t;
                            let from = if ik.ground[f].state == 0 { ankle[f].y } else { ik.ground[f].goal };
                            ik.ground[f].fade_in(from);
                        }
                    }
                } else {
                    ik.ground.iter_mut().for_each(GroundFoot::fade_out);
                }
            } else {
                ik.ground.iter_mut().for_each(GroundFoot::fade_out);
            }
            for f in 0..2 {
                ground_goal[f] = ik.ground[f].advance(ankle[f].y, dt);
            }
            // the pelvis goes down by how far the lower foot's ankle sits below its animated height (sub_431240)
            let want = (0..2).filter_map(|f| ground_goal[f].map(|g| g - ankle[f].y)).fold(0.0f32, f32::min);
            ik.pelvis += (want - ik.pelvis).clamp(-1.5 * dt, 1.5 * dt);
            if ground_goal.iter().any(Option::is_some) || ik.pelvis != 0.0 {
                for f in 0..2 {
                    if let Some(g) = ground_goal[f] {
                        ground_fix[2 + f] = Some(Vec3::new(ankle[f].x, g, ankle[f].z));
                    }
                }
                let offset = Vec3::Y * ik.pelvis;
                for g in &mut global {
                    g.pos += offset;
                }
                if let Ok(mut t) = joints.get_mut(rig.root) {
                    t.translation += player.rot.inverse() * offset;
                }
            }
        }
        if goals.iter().all(|g| g.is_none()) && ground_fix.iter().all(|g| g.is_none()) {
            ik.fit = Vec3::ZERO;
            continue;
        }

        // ---------------------------------------------------------------- body post-adjustments (0xE58333 …)
        if ik.body_bones.is_none() {
            let find = |id: u32| rig.bone_ids.iter().position(|b| *b == id).unwrap_or(usize::MAX);
            ik.body_bones = Some([find(HIPS), find(SPINE), find(HEAD)]);
        }
        let [_, spine, head] = ik.body_bones.unwrap();
        let forward = body_forward(body.heading);
        let ends: [Vec3; 4] = std::array::from_fn(|l| goals[l].map(|(c, _)| effector_goal(l, c, n)).unwrap_or(global[chains[l][2]].pos));
        let mut drop = 0.0;
        if targets.adjust & ADJUST_SLOPE != 0 {
            drop += slope_hips_drop(ends, body.feet, forward);
        }
        let (mut hips_want, mut lean_want) = (0.0, 0.0);
        if targets.adjust & ADJUST_HEAD != 0 && head != usize::MAX {
            if let Some(c) = collision.as_deref() {
                (hips_want, lean_want) = head_clearance(c, global[head].pos - Vec3::Y * drop, forward);
            }
        }
        // +660 / +664 move towards their targets at 1 /s (and back once the flags are off)
        ik.hips_drop = approach(ik.hips_drop, hips_want, dt);
        ik.lean = approach(ik.lean, lean_want, dt);
        drop += ik.hips_drop;
        let offset = -Vec3::Y * drop;
        ik.fit = offset;
        if offset != Vec3::ZERO {
            for g in &mut global {
                g.pos += offset;
            }
            if let Ok(mut t) = joints.get_mut(rig.root) {
                t.translation += player.rot.inverse() * offset;
            }
        }
        // the chest-origin effector's rotation goal: Spine turned back about the body's right by up to 45°
        if ik.lean > 0.0 && spine != usize::MAX {
            let r = Quat::from_axis_angle(forward.cross(Vec3::Y).normalize(), (ik.lean * 2.0).clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_4);
            let parent = rig.parents[spine].map(|p| global[p].rot).unwrap_or(root.rot);
            if let Ok(mut t) = joints.get_mut(rig.joints[spine]) {
                t.rotation = (parent.inverse() * (r * global[spine].rot)).normalize();
            }
            // the bones under the spine move with it (parents come before their children)
            let pivot = global[spine].pos;
            let mut moved = vec![false; global.len()];
            for i in 0..global.len() {
                moved[i] = i == spine || rig.parents[i].is_some_and(|pp| moved[pp]);
                if moved[i] {
                    global[i].rot = r * global[i].rot;
                    global[i].pos = pivot + r * (global[i].pos - pivot);
                }
            }
        }

        // ---------------------------------------------------------------- per-limb two-bone solve
        for limb in 0..4 {
            let [ia, ib, ic] = chains[limb];
            let (a, b, c) = (global[ia].pos, global[ib].pos, global[ic].pos);
            let (target, w) = match (goals[limb], ground_fix[limb]) {
                (Some((contact, w)), _) => (c.lerp(effector_goal(limb, contact, n), w), w),
                // the standing foot IK's ankle height
                (None, Some(t)) if ground_goal[limb - 2].is_some() => (t, 1.0),
                // the hips may have moved: re-check the foot against the surface
                (None, Some(_)) => match collision.as_deref().and_then(|col| ground_foot_target(c, col)) {
                    Some(t) => (t, 1.0),
                    None => continue,
                },
                _ => continue,
            };
            // elbows bend back, knees forward (used only when the limb is straight)
            let bend_hint = if limb < 2 { -forward } else { forward };
            let (da, db) = two_bone(a, b, c, target, bend_hint);
            let parent_a = rig.parents[ia].map(|p| global[p].rot).unwrap_or(root.rot);
            let parent_b = rig.parents[ib].map(|p| global[p].rot).unwrap_or(root.rot);
            let parent_c = rig.parents[ic].map(|p| global[p].rot).unwrap_or(root.rot);
            // new globals: A' = da·A, B' = db·B (db already includes da); everything under B moves with db
            if let Ok(mut t) = joints.get_mut(rig.joints[ia]) {
                t.rotation = (parent_a.inverse() * (da * global[ia].rot)).normalize();
            }
            if let Ok(mut t) = joints.get_mut(rig.joints[ib]) {
                t.rotation = ((da * parent_b).inverse() * (db * global[ib].rot)).normalize();
            }
            // the hand/foot keeps its animated world orientation (grip and toe placement as authored)
            if let Ok(mut t) = joints.get_mut(rig.joints[ic]) {
                let keep = global[ic].rot;
                let new_parent = db * parent_c;
                let solved = (new_parent.inverse() * keep).normalize();
                t.rotation = t.rotation.slerp(solved, w);
            }
        }
    }
}

fn approach(v: f32, want: f32, dt: f32) -> f32 {
    v + (want - v).clamp(-dt, dt)
}

/// `ADJUST_SLOPE` (0xE58333): the four effectors projected onto the body's vertical plane (through `root`, along
/// `forward`); the line from the higher foot to the higher hand leans `a` from up about the body's right (positive =
/// the hands further out than the feet). k = (clamp(a, 10°, 40°) − 10°) / 30° · (1 − min(|Δ feet height|, 0.6) / 0.6);
/// the hips go down 0.4·k.
pub fn slope_hips_drop(ends: [Vec3; 4], root: Vec3, forward: Vec3) -> f32 {
    let side = forward.cross(Vec3::Y).normalize_or_zero();
    let proj = |p: Vec3| p - side * (p - root).dot(side);
    let [lh, rh, lf, rf] = ends.map(proj);
    let feet = 1.0 - (lf.y - rf.y).abs().min(0.6) / 0.6;
    let foot = if lf.y <= rf.y { rf } else { lf };
    let hand = if lh.y <= rh.y { rh } else { lh };
    let d = hand - foot;
    if d.abs().max_element() <= 0.0005 {
        return 0.0;
    }
    let d = d.normalize();
    let mut a = d.dot(Vec3::Y).clamp(-1.0, 1.0).acos();
    if Vec3::Y.cross(d).dot(side) < 0.0 {
        a = -a;
    }
    let (lo, hi) = (10f32.to_radians(), 40f32.to_radians());
    let k = (a.clamp(lo, hi) - lo) / (hi - lo) * feet;
    if k > 0.0005 { 0.4 * k } else { 0.0 }
}

/// `ADJUST_HEAD` (0xE58799), from the head `h`: (hips drop, chest lean) targets.
/// - a slab 0.3 m behind the head (centre h − 0.2 up − 0.3 forward; 0.1 across, ±0.5 up, 0.01 thick): its lowest solid
///   point z above the head gives a drop of 0.3 − z (the head keeps 0.3 m under an overhang);
/// - a box at h − (drop + 0.02) up − 0.05 forward (0.1 across, ±0.25 along forward, ±0.3 up): its rearmost solid point
///   y in front of the head gives a lean of 0.2 − y (a wall closer than 0.2 m to the face).
pub fn head_clearance(c: &crate::collision::CollisionWorld, h: Vec3, forward: Vec3) -> (f32, f32) {
    let up = Vec3::Y;
    let back = -forward;
    let drop = c
        .obb_lowest(h - up * 0.2 - forward * 0.3, [up.cross(back).normalize(), up, back], Vec3::new(0.1, 0.5, 0.01), 1)
        .map(|p| (0.3 - (p - h).dot(up)).max(0.0))
        .unwrap_or(0.0);
    let lean = c
        .obb_lowest(h - up * (drop + 0.02) - forward * 0.05, [forward.cross(up).normalize(), forward, up], Vec3::new(0.1, 0.25, 0.3), 1)
        .map(|p| (0.2 - (p - h).dot(forward)).max(0.0))
        .unwrap_or(0.0);
    (drop, lean)
}

fn body_forward(heading: f32) -> Vec3 {
    Vec3::new(-heading.sin(), 0.0, -heading.cos())
}

/// Analytic two-bone IK (law of cosines). Returns the global rotation deltas `(da, db)`: the upper
/// bone's new global rotation is `da·A`, the middle bone's is `db·B`, and the end lands on `t`
/// (clamped to reach). The bend plane of the current pose is kept.
pub fn two_bone(a: Vec3, b: Vec3, c: Vec3, t: Vec3, bend_hint: Vec3) -> (Quat, Quat) {
    let eps = 1e-4;
    let lab = (b - a).length();
    let lcb = (c - b).length();
    if lab < eps || lcb < eps {
        return (Quat::IDENTITY, Quat::IDENTITY);
    }
    let lat = (t - a).length().clamp(eps.max((lab - lcb).abs() + 1e-3), lab + lcb - 1e-3);
    let acos = |x: f32| x.clamp(-1.0, 1.0).acos();

    let ac = (c - a).normalize_or_zero();
    let ab = (b - a).normalize_or_zero();
    let at = (t - a).normalize_or_zero();
    let ac_ab_0 = acos(ac.dot(ab));
    let ba_bc_0 = acos((a - b).normalize_or_zero().dot((c - b).normalize_or_zero()));
    let ac_ab_1 = acos((lcb * lcb - lab * lab - lat * lat) / (-2.0 * lab * lat));
    let ba_bc_1 = acos((lat * lat - lab * lab - lcb * lcb) / (-2.0 * lab * lcb));

    // bend-plane normal; falls back to the hint when the limb is straight
    let mut axis0 = ac.cross(ab);
    if axis0.length_squared() < 1e-8 {
        axis0 = ac.cross((ab + bend_hint * 0.1).normalize_or_zero());
    }
    let axis0 = axis0.normalize_or_zero();
    if axis0 == Vec3::ZERO {
        return (Quat::IDENTITY, Quat::IDENTITY);
    }
    let r0 = Quat::from_axis_angle(axis0, ac_ab_1 - ac_ab_0);
    let r1 = Quat::from_axis_angle(axis0, ba_bc_1 - ba_bc_0);
    let axis1 = ac.cross(at);
    let r2 = if axis1.length_squared() < 1e-10 { Quat::IDENTITY } else { Quat::from_axis_angle(axis1.normalize(), acos(ac.dot(at))) };
    let da = r2 * r0;
    // B's new global = da · r1 · B (r1 acts about B in the original frame)
    (da, da * r1)
}

/// `AC_IK_LOG=1`: after transform propagation, log how far each weighted end bone is from its goal
/// (verifies the solve on the rendered skeleton), plus the contact-fit translation.
pub fn log_ik_error(q: Query<(&Rig, &LimbIk, &LimbTargets)>, globals: Query<&GlobalTransform>, mut frame: Local<u32>) {
    *frame += 1;
    let every: u32 = std::env::var("AC_IK_LOG").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 1).unwrap_or(30);
    if *frame % every != 0 {
        return;
    }
    for (rig, ik, targets) in &q {
        let Some(chains) = ik.chains.filter(|c| c.iter().all(|l| l[0] != usize::MAX)) else { continue };
        let mut parts = Vec::new();
        for (i, l) in ik.limbs.iter().enumerate() {
            if l.weight <= 0.0 {
                continue;
            }
            let Ok(g) = globals.get(rig.joints[chains[i][2]]) else { continue };
            let want = effector_goal(i, l.goal, targets.normal);
            parts.push(format!("{}={:.3}m(w{:.2})", ["LH", "RH", "LF", "RF"][i], (g.translation() - want).length(), l.weight));
        }
        if !parts.is_empty() {
            info!("ik error: {} | hips {:.3}m lean {:.2}", parts.join(" "), ik.fit.length(), ik.lean);
        }
        if ik.ground.iter().any(|f| f.state != 0) || ik.pelvis != 0.0 {
            let foot = |f: &GroundFoot, i: usize| {
                let err = globals.get(rig.joints[chains[i][2]]).map(|g| (g.translation().y - f.goal).abs()).unwrap_or(-1.0);
                format!("state {} goal {:.3} err {:.3}", f.state, f.goal, err)
            };
            info!("ground ik: L {} | R {} | pelvis {:.3}", foot(&ik.ground[0], 2), foot(&ik.ground[1], 3), ik.pelvis);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forward kinematics of the solved chain must reach the target.
    fn check(a: Vec3, b: Vec3, c: Vec3, t: Vec3) -> f32 {
        let (da, db) = two_bone(a, b, c, t, Vec3::Z);
        let b2 = a + da * (b - a);
        let c2 = b2 + db * (c - b);
        (c2 - t).length()
    }

    #[test]
    fn two_bone_reaches_targets() {
        let (a, b, c) = (Vec3::ZERO, Vec3::new(0.0, -0.3, 0.05), Vec3::new(0.0, -0.58, 0.0));
        for t in [Vec3::new(0.2, -0.3, 0.2), Vec3::new(-0.1, 0.4, 0.1), Vec3::new(0.0, -0.2, 0.3)] {
            assert!(check(a, b, c, t) < 1e-3, "target {t} missed");
        }
        // straight chain uses the hint
        assert!(check(Vec3::ZERO, Vec3::new(0.0, -0.3, 0.0), Vec3::new(0.0, -0.6, 0.0), Vec3::new(0.0, -0.4, 0.1)) < 1e-3);
        // out of reach: the chain straightens towards the target
        let t = Vec3::new(0.0, -2.0, 0.0);
        let (da, db) = two_bone(a, b, c, t, Vec3::Z);
        let c2 = a + da * (b - a) + db * (c - b);
        assert!(c2.normalize().dot(t.normalize()) > 0.999);
    }

    #[test]
    fn ground_foot_fades_in_and_out_over_0_2_s() {
        let dt = 1.0 / 60.0;
        let mut f = GroundFoot { target: 0.1, ..Default::default() };
        assert_eq!(f.advance(0.4, dt), None, "off: no goal");
        f.fade_in(0.4);
        let mut frames = 0;
        while f.state == 1 {
            let g = f.advance(0.4, dt).unwrap();
            assert!((0.1..=0.4).contains(&g));
            frames += 1;
        }
        assert_eq!(f.state, 2);
        assert!((11..=13).contains(&frames), "{frames}"); // 0.2 s
        assert!((f.advance(0.4, dt).unwrap() - 0.1).abs() < 1e-4, "running: on the ground");
        f.fade_out();
        assert_eq!(f.state, 3);
        let mut last = 0.0;
        while let Some(g) = f.advance(0.4, dt) {
            last = g;
        }
        assert_eq!(f.state, 0);
        assert!(last > 0.35, "fades back to the animated ankle: {last}");
    }

    #[test]
    fn hips_drop_only_when_the_hands_lean_out() {
        let fwd = Vec3::NEG_Z;
        let feet = [Vec3::new(-0.1, 0.0, -0.3), Vec3::new(0.1, 0.0, -0.3)];
        // vertical wall: hands straight above the feet
        let flat = slope_hips_drop([Vec3::new(-0.2, 1.2, -0.3), Vec3::new(0.2, 1.2, -0.3), feet[0], feet[1]], Vec3::ZERO, fwd);
        assert_eq!(flat, 0.0);
        // hands 0.7 m further out (back) over 1.2 m: atan(0.7/1.2) = 30° → k = 2/3 → 0.267 m
        let out = slope_hips_drop([Vec3::new(-0.2, 1.2, 0.4), Vec3::new(0.2, 1.2, 0.4), feet[0], feet[1]], Vec3::ZERO, fwd);
        assert!((out - 0.4 * (30.26 - 10.0) / 30.0).abs() < 0.01, "{out}");
        // hands further in (an overhanging face leans the other way): nothing
        let inward = slope_hips_drop([Vec3::new(-0.2, 1.2, -1.0), Vec3::new(0.2, 1.2, -1.0), feet[0], feet[1]], Vec3::ZERO, fwd);
        assert_eq!(inward, 0.0);
    }

    #[test]
    fn head_clearance_ducks_and_leans() {
        use crate::collision::{Aabb3, CollisionWorld};
        let h = Vec3::new(0.0, 2.0, 0.0);
        // a slab 0.1 m above the head reaching 1 m out from the wall in front (−Z): duck 0.2 m
        let c = CollisionWorld { boxes: vec![Aabb3 { min: Vec3::new(-1.0, 2.1, -1.0), max: Vec3::new(1.0, 2.4, 0.5) }], ..Default::default() };
        let (drop, lean) = head_clearance(&c, h, Vec3::NEG_Z);
        assert!((drop - 0.2).abs() < 0.03, "{drop}");
        assert_eq!(lean, 0.0);
        // a wall 0.1 m in front of the face: lean 0.1
        let c = CollisionWorld { boxes: vec![Aabb3 { min: Vec3::new(-1.0, 0.0, -1.0), max: Vec3::new(1.0, 3.0, -0.1) }], ..Default::default() };
        let (drop, lean) = head_clearance(&c, h, Vec3::NEG_Z);
        assert_eq!(drop, 0.0);
        assert!((lean - 0.1).abs() < 0.03, "{lean}");
    }

    #[test]
    fn limb_weight_and_travel_follow_the_game_rates() {
        let mut l = LimbState::default();
        let dt = 1.0 / 60.0;
        let mut frames = 0;
        while l.weight < 1.0 {
            l.update(Some(Vec3::ZERO), Vec3::ZERO, 0.5, dt);
            frames += 1;
        }
        assert!((15..=16).contains(&frames), "{frames}"); // 0.25 s at 4/s
        // retarget: the goal follows the clip's own limb motion, corrected linearly onto the new hold
        let mut anim = Vec3::ZERO;
        let mut mid = Vec3::ZERO;
        for k in 0..30 {
            anim += Vec3::new(0.0, 0.02, 0.0); // the clip lifts the limb
            let g = l.update(Some(Vec3::X), anim, 0.5, dt).unwrap();
            if k == 14 {
                mid = g;
            }
        }
        assert!((l.goal - Vec3::X).length() < 1e-5);
        assert!(mid.y > 0.1, "follows the clip's arc mid-way: {mid}");
        let mut frames = 0;
        while l.update(None, Vec3::ZERO, 0.5, dt).is_some() {
            frames += 1;
        }
        assert!((11..=12).contains(&frames), "{frames}"); // 0.2 s at 5/s
    }
}
