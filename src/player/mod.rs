//! The player ("Human") and its locomotion-context framework.
//!
//! Mirrors the game's architecture (RE/01):
//! - exactly one *locomotion context* is active (AIActor extension slot 1);
//! - contexts are identified by `ActorContextID` (values verbatim from the exe, desc 0x19640EC);
//! - a switch is immediate (`AIActor::SwitchLocomotionContext` 0x55F7E0): old context exits, a
//!   `TransitionSetup` object is applied to the destination's data block, the new context enters,
//!   and the new context skips its first update (the `justSwitched` byte);
//! - every context's runtime data lives in one `HumanDataBundle` (HumanData+0x30 in the game).

pub mod abilities;
pub mod air;
pub mod air_catches;
pub mod anim_gate;
pub mod climb;
pub mod collide;
pub mod crouch;
pub mod crowd;
pub mod dead;
pub mod ground;
pub mod ground_extras;
pub mod ground_tree;
pub mod falls;
pub mod hay;
pub mod item_flags;
pub mod jump_blend;
pub mod jump_candidates;
pub mod jump_clips;
pub mod kiosk;
pub mod ladder;
pub mod ledge;
pub mod ledge_moves;
pub mod move_blend;
pub mod narrow;
pub mod passover;
pub mod swing;
pub mod social;
pub mod targets;
pub mod walling;

use bevy::prelude::*;

use crate::tuning::*;

/// `ActorContextID` (subset used so far; numbering matches the exe).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ActorContextId {
    Ground = 4,
    Ladder = 5,
    Pole = 6,
    Rope = 7,
    InAir = 8,
    Ledge = 9,
    Climb = 10,
    Walling = 11,
    NarrowObject = 12,
    Kiosk = 19,
    Dead = 20,
    HayStack = 21,
}

#[derive(Component)]
pub struct Player;

/// Locomotion slot state (AIActor slot 1: current / previous id + justSwitched).
#[derive(Component, Debug)]
pub struct Locomotion {
    pub current: ActorContextId,
    pub previous: ActorContextId,
    pub just_switched: bool,
}

/// Kinematic body: feet position, facing (yaw, radians; 0 = facing -Z) and velocity.
#[derive(Component, Debug, Default)]
pub struct Body {
    pub feet: Vec3,
    pub heading: f32,
    /// Pitch / roll of the root on top of the heading (world space). Only the climb sets it (its root frame follows a
    /// leaning wall, sub_B1CC20); identity elsewhere.
    pub tilt: Quat,
    /// The character proxy's persistent manifold (controller +232).
    pub proxy: crate::proxy::ProxyState,
    pub velocity: Vec3,
    pub grounded: bool,
    /// CharacterController +384: pre-snap feet height minus support (0x57D240), consumed by IK.
    pub stick_residual: f32,
    pub stick_normal_y: Option<f32>,
}

impl Body {
    pub fn forward(&self) -> Vec3 {
        Vec3::new(-self.heading.sin(), 0.0, -self.heading.cos())
    }
}

pub fn heading_of(dir: Vec3) -> f32 {
    (-dir.x).atan2(-dir.z)
}

/// Right-hand vector for a facing direction (horizontal).
pub fn right_of(forward: Vec3) -> Vec3 {
    Vec3::new(-forward.z, 0.0, forward.x)
}

/// IK targets for the four limbs (the game's limb-IK component at Human+1328: 0 L hand, 1 R hand,
/// 2 L foot, 3 R foot). `None` = limb free (plays animation only).
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct LimbTargets {
    pub hands: Option<(Vec3, Vec3)>,
    pub feet: Option<(Vec3, Vec3)>,
    /// Wall / edge normal of the contacts (out of the wall).
    pub normal: Vec3,
    /// Time a limb takes to travel to a new hold (the running move's duration).
    pub transit: f32,
    /// The limb-IK body adjustments switched on (LimbIK +656, `crate::ik::ADJUST_*`): the climb sets both on entry
    /// (`HumanClimb__EnterCommon` 0xDE97B0) and clears them on exit (`HumanClimb__ExitCleanup` 0xDE9A00).
    pub adjust: u8,
}

/// Linear root interpolation used by all discrete moves (`sub_711130` / `sub_7113F0(human+80)`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RootInterp {
    pub from: Vec3,
    pub to: Vec3,
    pub t: f32,
    pub duration: f32,
}

impl RootInterp {
    pub fn new(from: Vec3, to: Vec3, duration: f32) -> Self {
        Self { from, to, t: 0.0, duration: duration.max(1e-3) }
    }
    /// Advance; returns (position, progress 0..1).
    pub fn advance(&mut self, dt: f32) -> (Vec3, f32) {
        self.t = (self.t + dt).min(self.duration);
        let s = self.t / self.duration;
        (self.from.lerp(self.to, s), s)
    }
    pub fn done(&self) -> bool {
        self.t >= self.duration
    }
}

/// All per-context runtime data of a Human (the game's HumanDataBundle, RE/07 §4b).
#[derive(Component, Debug, Default)]
pub struct HumanDataBundle {
    pub ground: ground::HumanGroundData,
    pub air: air::HumanInAirData,
    pub ledge: ledge::HumanLedgeData,
    pub climb: climb::HumanClimbData,
    pub hay: hay::HumanHayStackData,
    pub walling: walling::HumanWallingData,
    pub narrow: narrow::HumanNarrowObjectData,
    pub ladder: ladder::HumanLadderData,
    pub kiosk: kiosk::HumanKioskData,
    pub dead: dead::HumanDeadData,
}

/// Transition setup objects (`TransitionSetupDataToMovement` / `…ToInAir` …, RE/01 §4.2).
pub enum TransitionSetup {
    ToMovement { landing: Option<air::Landing> },
    ToInAir(air::InAirEntry),
    ToLedge(ledge::LedgeEntry),
    ToClimb(climb::ClimbEntry),
    ToHayStack(hay::HayStackEntry),
    ToWalling(walling::WallingEntry),
    ToBeam(narrow::BeamEntry),
    ToPilotis(narrow::PilotisEntry),
    ToPassOver(passover::PassOverEntry),
    ToLadder(ladder::LadderEntry),
    ToKiosk(kiosk::KioskEntry),
    ToDead(dead::DeadEntry),
}

/// Immediate context switch (0x55F7E0): exit old, apply setup to destination data, enter new.
pub fn switch_context(loco: &mut Locomotion, data: &mut HumanDataBundle, setup: TransitionSetup) {
    loco.previous = loco.current;
    loco.current = match setup {
        TransitionSetup::ToMovement { landing } => {
            data.ground.enter(landing);
            ActorContextId::Ground
        }
        TransitionSetup::ToInAir(entry) => {
            data.air.enter(entry);
            ActorContextId::InAir
        }
        TransitionSetup::ToLedge(entry) => {
            data.ledge.enter(entry);
            ActorContextId::Ledge
        }
        TransitionSetup::ToClimb(entry) => {
            data.climb.enter(entry);
            ActorContextId::Climb
        }
        TransitionSetup::ToHayStack(entry) => {
            data.hay.enter(entry);
            ActorContextId::HayStack
        }
        TransitionSetup::ToWalling(entry) => {
            data.walling.enter(entry);
            ActorContextId::Walling
        }
        TransitionSetup::ToBeam(entry) => {
            data.narrow.enter(entry);
            ActorContextId::NarrowObject
        }
        TransitionSetup::ToLadder(entry) => {
            data.ladder.enter(entry);
            ActorContextId::Ladder
        }
        TransitionSetup::ToPassOver(entry) => {
            let seq = data.ledge.pass_over.map(|p| p.seq).unwrap_or(0);
            data.ledge.pass_over = passover::enter(entry, seq);
            ActorContextId::Ledge
        }
        TransitionSetup::ToPilotis(entry) => {
            data.narrow.enter_pilotis(entry);
            ActorContextId::NarrowObject
        }
        TransitionSetup::ToKiosk(entry) => {
            data.kiosk.enter(entry);
            ActorContextId::Kiosk
        }
        TransitionSetup::ToDead(entry) => {
            data.dead.enter(entry);
            ActorContextId::Dead
        }
    };
    loco.just_switched = true;
}

/// The skipped first update of a context just switched in (`AIActor::Update`, the `justSwitched` byte): the context's
/// logic does not run, but the character controller still integrates the velocity it was left with
/// (`CharacterController__Integrate` 0x57C7C0 runs every frame on +0x90). Without this the body would freeze for a
/// frame at every switch (a visible hitch at each takeoff and landing).
pub fn coast(body: &mut Body, collision: &crate::collision::CollisionWorld, dt: f32) {
    let v = if body.grounded { Vec3::new(body.velocity.x, 0.0, body.velocity.z) } else { body.velocity };
    if v.length_squared() < 1e-6 {
        return;
    }
    let r = collision.move_capsule(&mut body.proxy, body.feet, v * dt, body.grounded, dt);
    body.feet = r.position;
}

/// All player context systems (animation, IK and the camera run after them).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlayerSet;

#[derive(Resource)]
pub struct SpawnPoint(pub Vec3);

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SpawnPoint(Vec3::new(0.0, 0.0, 4.0)))
            .init_resource::<abilities::AbilitySet>()
            .add_systems(Startup, spawn_player)
            .add_systems(
                Update,
                (social::update_social, crowd::update_crowd, ground::update_ground, air::update_air, ledge::update_ledge, climb::update_climb, hay::update_hay, kiosk::update_kiosk, dead::update_dead, walling::update_walling, narrow::update_narrow, ladder::update_ladder, release_limbs, proxy_layer, sync_visuals)
                    .chain()
                    .in_set(PlayerSet),
            );
    }
}

/// Limb IK targets belong to the hang / climb contexts only: any other context lets the hands and feet go (the game's
/// contexts release their limb contacts on exit). Without this a context that never touches the targets (InAir after
/// a swing jump, Ground after a pass-over …) kept the hands pinned to the last bar or ledge.
/// The capsule's collision layer by context (each context's entry, through `Human__CharacterLayer` 0xB0EAA0, which turns
/// Character 5 / CharacterNoInteractive 12 / CharacterNoStatic 20 into the main character's 21 / 22 / 23):
/// - Character (→ MainCharacter): `HumanGround__OnActivateSetup` 0xDAE6E0, `HumanLedge__OnEnter` 0xDE3460, the
///   pull-up blend 0xDCBF70, `HumanClimb__State7_Enter` 0xDE91B0, `HumanClimb__ExitCleanup` 0xDE9A00;
/// - CharacterNoStatic (→ MainCharacterNoStatic): `HumanLedge__EnterCommon` 0xDE26D0, `HumanClimb__EnterCommon`
///   0xDE97B0, `HumanLedge__PassOverToPullDown` 0xDE0220: hanging and climbing, the capsule ignores the static world.
/// PORT: by context only (the ledge pull-up keeps NoStatic; the port's ledge / climb moves don't run the proxy).
pub fn proxy_layer(mut q: Query<(&Locomotion, &HumanDataBundle, &mut Body), With<Player>>) {
    for (loco, data, mut body) in &mut q {
        body.proxy.height = (loco.current == ActorContextId::Ground && data.ground.crouch.is_some()).then_some(1.0);
        let layer = match loco.current {
            ActorContextId::Ledge | ActorContextId::Climb => crate::layers::MAIN_CHARACTER_NO_STATIC,
            _ => crate::layers::MAIN_CHARACTER,
        };
        if body.proxy.layer != layer {
            body.proxy.layer = layer;
        }
    }
}

pub fn release_limbs(mut q: Query<(&Locomotion, &mut LimbTargets), With<Player>>) {
    for (loco, mut limbs) in &mut q {
        if !matches!(loco.current, ActorContextId::Ledge | ActorContextId::Climb) && (limbs.hands.is_some() || limbs.feet.is_some()) {
            limbs.hands = None;
            limbs.feet = None;
        }
        if loco.current != ActorContextId::Climb {
            limbs.adjust = 0;
        }
    }
}

#[derive(Component)]
struct HandMarker(usize);

pub fn player_components(feet: Vec3, heading: f32) -> impl Bundle {
    (
        Player,
        social::SocialHelper::default(),
        Locomotion { current: ActorContextId::Ground, previous: ActorContextId::Ground, just_switched: false },
        Body { feet, heading, grounded: true, ..default() },
        HumanDataBundle::default(),
        LimbTargets::default(),
    )
}

fn spawn_player(
    mut commands: Commands,
    spawn: Res<SpawnPoint>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // PLACEHOLDER body: capsule + a "nose" so facing is visible. Replaced by Altaïr's Mesh/Skeleton
    // once those .forge resources are decoded (stage 3).
    let body_mat = materials.add(StandardMaterial { base_color: Color::srgb(0.93, 0.93, 0.95), ..default() });
    let sash_mat = materials.add(StandardMaterial { base_color: Color::srgb(0.65, 0.08, 0.08), ..default() });
    commands
        .spawn((player_components(spawn.0, std::f32::consts::PI), Transform::from_translation(spawn.0), Visibility::default()))
        .with_children(|p| {
            let h = CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS;
            p.spawn((
                crate::model::PlaceholderBody,
                Mesh3d(meshes.add(Capsule3d::new(CAPSULE_RADIUS, h))),
                MeshMaterial3d(body_mat),
                Transform::from_xyz(0.0, CAPSULE_HEIGHT * 0.5, 0.0),
            ));
            p.spawn((
                crate::model::PlaceholderBody,
                Mesh3d(meshes.add(Cuboid::new(0.12, 0.12, 0.3))),
                MeshMaterial3d(sash_mat.clone()),
                Transform::from_xyz(0.0, 1.55, -0.3),
            ));
        });
    // limb IK target markers (hands red, feet dark)
    let hand_mat = materials.add(StandardMaterial { base_color: Color::srgb(0.85, 0.15, 0.1), unlit: true, ..default() });
    let foot_mat = materials.add(StandardMaterial { base_color: Color::srgb(0.2, 0.2, 0.25), unlit: true, ..default() });
    for i in 0..4 {
        commands.spawn((
            HandMarker(i),
            Mesh3d(meshes.add(Sphere::new(0.07))),
            MeshMaterial3d(if i < 2 { hand_mat.clone() } else { foot_mat.clone() }),
            Transform::default(),
            Visibility::Hidden,
        ));
    }
}

fn sync_visuals(
    q: Query<(&Body, &LimbTargets), With<Player>>,
    mut t: Query<&mut Transform, With<Player>>,
    mut markers: Query<(&HandMarker, &mut Transform, &mut Visibility), Without<Player>>,
    rig: Query<(), With<crate::model::Rig>>,
) {
    let Ok((b, limbs)) = q.single() else { return };
    // with the real model the IK puts the hands on the holds; markers only on request
    let show = rig.is_empty() || std::env::var_os("AC_IK_MARKERS").is_some();
    if let Ok(mut tr) = t.single_mut() {
        tr.translation = b.feet;
        tr.rotation = b.tilt * Quat::from_rotation_y(b.heading);
    }
    for (m, mut tr, mut vis) in &mut markers {
        let p = match m.0 {
            0 => limbs.hands.map(|h| h.0),
            1 => limbs.hands.map(|h| h.1),
            2 => limbs.feet.map(|f| f.0),
            _ => limbs.feet.map(|f| f.1),
        };
        match p {
            Some(p) if show => {
                tr.translation = p;
                *vis = Visibility::Visible;
            }
            _ => *vis = Visibility::Hidden,
        }
    }
}
