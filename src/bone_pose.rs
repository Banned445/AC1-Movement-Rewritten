//! The animated skeleton as data, so the simulation can read bone positions without the renderer (bone positions in
//! the simulation, phase 1).
//!
//! The game reads a bone through a handle (`Human__SetupBoneHandles` 0xB11020: Head, Spine, the hands and the feet at
//! Human +0x280 …) with `Bone__GetWorldPosition` 0x4EC0E0: the skeleton's last evaluated pose placed at the entity's
//! current matrix (a moved entity marks the skeleton dirty, `SkeletonComponent__MarkDirty` 0x4DF4B0, and the read
//! rebuilds the global pose, `SkeletonComponent__RebuildGlobalPose` 0x4E75D0). The contexts run before the frame's
//! animation (`Game__SetupPostAITasks` 0x45C900), so they read the previous frame's pose at the current root;
//! `BonePose::world_pos` does the same with the body's current feet, heading and tilt.
use bevy::prelude::*;

use crate::assets::ac_formats::SkelBone;
use crate::player::Body;

/// Bone ids (CRC32 of the bone name, RE/data/altair_bone_names.json).
pub const HEAD: u32 = 0x07C1_59A2;
pub const SPINE: u32 = 0x530E_C1CB;
pub const HIPS: u32 = 0xDED1_0611;
pub const LEFT_HAND: u32 = 0xB675_F36C;
pub const RIGHT_HAND: u32 = 0x75F9_4D30;
pub const LEFT_FOOT: u32 = 0x5898_8870;
pub const RIGHT_FOOT: u32 = 0x9B14_362C;

/// Altaïr's movement skeleton (`UCMA_Altair`): bone ids, parents and the rest pose, in skeleton order. Empty when the
/// install is not found.
#[derive(Resource, Default, Clone, Debug)]
pub struct PoseSkeleton {
    pub bone_ids: Vec<u32>,
    pub parents: Vec<Option<usize>>,
    pub rest: Vec<Transform>,
}

impl PoseSkeleton {
    pub fn from_bones(bones: &[SkelBone]) -> Self {
        Self {
            bone_ids: bones.iter().map(|b| b.bone_id).collect(),
            parents: bones.iter().map(|b| b.parent).collect(),
            rest: bones
                .iter()
                .map(|b| Transform { translation: Vec3::from_array(b.local_pos), rotation: Quat::from_array(b.local_rot).normalize(), scale: Vec3::ONE })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bone_ids.is_empty()
    }

    pub fn index(&self, bone: u32) -> Option<usize> {
        self.bone_ids.iter().position(|b| *b == bone)
    }
}

/// Loads the skeleton alone from the install, for runs without the renderer (the game fills it from the model it
/// loads, `model::attach_altair`, so the Rig and the pose share the same bones).
pub fn load_pose_skeleton(mut skel: ResMut<PoseSkeleton>) {
    if std::env::var_os("AC_NO_MODEL").is_some() {
        return;
    }
    match crate::assets::altair::load_skeleton(&crate::assets::game_dir()) {
        Ok(bones) => *skel = PoseSkeleton::from_bones(&bones),
        Err(e) => warn!("pose skeleton not loaded: {e}"),
    }
}

/// One skeleton per process for the runs without the renderer (tests, replays). None without the install.
pub fn shared_pose_skeleton() -> Option<PoseSkeleton> {
    static SKEL: std::sync::OnceLock<Option<PoseSkeleton>> = std::sync::OnceLock::new();
    SKEL.get_or_init(|| crate::assets::altair::load_skeleton(&crate::assets::game_dir()).ok().map(|b| PoseSkeleton::from_bones(&b))).clone()
}

/// Readies a schedule without the renderer (the tests' `Sim`, the replays, `CitySim`) for the animation: inserts the
/// shared library and skeleton (empty ones without the install). The harness chains `anim::headless_animation_systems`
/// after its contexts, so the player's `BonePose` follows its actions as in the game. Returns whether the install was
/// found: give the player an `AnimPlayer` only then. `AC_SIM_NO_ANIM=1` keeps the runs without animation (to compare).
pub fn add_headless_animation(app: &mut App) -> bool {
    let shared = if std::env::var_os("AC_SIM_NO_ANIM").is_some() { None } else { crate::anim::shared_library().zip(shared_pose_skeleton()) };
    let on = shared.is_some();
    let (lib, skel) = shared.unwrap_or_default();
    app.insert_resource(lib).insert_resource(skel);
    on
}

/// The player's pose of the last animation step: each bone's local transform, the skeleton root's transform in the
/// player's space, and each bone in the player's space (forward kinematics). Written by `anim::animate_pose`; the
/// renderer copies `locals` and `root` onto its joints.
#[derive(Component, Default, Clone, Debug)]
pub struct BonePose {
    pub locals: Vec<(Quat, Vec3)>,
    pub root: Transform,
    /// Bone rotations and positions in the player's space (feet at the origin, facing −Z, before heading and tilt).
    pub model: Vec<(Quat, Vec3)>,
    /// Counts the animation steps that wrote the pose (0: never posed).
    pub seq: u64,
}

impl BonePose {
    /// Forward kinematics of `locals` under `root`, in skeleton order (parents come before their children).
    pub fn compute_model(&mut self, skel: &PoseSkeleton) {
        self.model.clear();
        let root = (self.root.rotation, self.root.translation);
        for (i, &(r, t)) in self.locals.iter().enumerate() {
            let (pr, pt) = skel.parents.get(i).copied().flatten().map(|p| self.model[p]).unwrap_or(root);
            self.model.push((pr * r, pt + pr * t));
        }
    }

    /// A bone's position in the player's space.
    pub fn model_pos(&self, skel: &PoseSkeleton, bone: u32) -> Option<Vec3> {
        self.model.get(skel.index(bone)?).map(|m| m.1)
    }

    /// `Human__GetLeadingFoot` 0xB18850: the LeftFoot and RightFoot bones (+0x2A0 / +0x2A8) compared along the
    /// character's forward; 0 (the game's 1) when the left one is strictly further forward, else 1 (2: the right one,
    /// ties included). None before the first pose.
    pub fn leading_foot(&self, skel: &PoseSkeleton) -> Option<usize> {
        // the player's space faces −Z
        let (l, r) = (self.model_pos(skel, LEFT_FOOT)?, self.model_pos(skel, RIGHT_FOOT)?);
        Some(if -l.z > -r.z { 0 } else { 1 })
    }

    /// A bone's world position with the body where it stands now (the game's `Bone__GetWorldPosition`).
    pub fn world_pos(&self, skel: &PoseSkeleton, bone: u32, body: &Body) -> Option<Vec3> {
        Some(body.feet + body.tilt * Quat::from_rotation_y(body.heading) * self.model_pos(skel, bone)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chain root → a → b: each bone sits under its parent's rotation and position, the root under the pose's root.
    #[test]
    fn forward_kinematics_composes_the_chain_under_the_root() {
        let skel = PoseSkeleton { bone_ids: vec![1, 2, 3], parents: vec![None, Some(0), Some(1)], rest: vec![Transform::IDENTITY; 3] };
        let quarter = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let mut pose = BonePose {
            locals: vec![(Quat::IDENTITY, Vec3::Y), (quarter, Vec3::X), (Quat::IDENTITY, Vec3::X)],
            root: Transform::from_translation(Vec3::new(0.0, 0.5, 0.0)),
            ..Default::default()
        };
        pose.compute_model(&skel);
        assert!(pose.model_pos(&skel, 1).unwrap().distance(Vec3::new(0.0, 1.5, 0.0)) < 1e-6);
        assert!(pose.model_pos(&skel, 2).unwrap().distance(Vec3::new(1.0, 1.5, 0.0)) < 1e-6);
        // b's offset turns with a's rotation: +X becomes −Z
        assert!(pose.model_pos(&skel, 3).unwrap().distance(Vec3::new(1.0, 1.5, -1.0)) < 1e-6);
        // the world position follows the body: feet, then heading
        let body = Body { feet: Vec3::new(10.0, 0.0, 0.0), heading: std::f32::consts::PI, tilt: Quat::IDENTITY, ..Default::default() };
        assert!(pose.world_pos(&skel, 2, &body).unwrap().distance(Vec3::new(9.0, 1.5, 0.0)) < 1e-5);
        assert_eq!(pose.world_pos(&skel, 9, &body), None);
    }
}
