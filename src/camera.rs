//! Simple third-person orbit camera. Mouse (cursor locked) or right stick orbits; scroll zooms.
//! Esc releases the cursor, left click captures it again.
//! (The game's NavigationCamera is not reverse engineered yet; this is a stand-in.)

use bevy::input::gamepad::Gamepad;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::player::Player;

#[derive(Resource)]
pub struct CameraRig {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub shake: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        // yaw = PI: camera behind a player facing +Z (toward the rooftops)
        Self { yaw: std::f32::consts::PI, pitch: -0.35, distance: 5.5, shake: 0.0 }
    }
}

#[derive(Component)]
pub struct MainCamera;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraRig>()
            .add_systems(Startup, spawn_camera)
            // after the player has moved this frame (otherwise the camera reads this frame's or last frame's position
            // depending on the scheduler: a one-frame judder)
            .add_systems(Update, (grab_cursor, orbit, follow).chain().after(crate::player::PlayerSet));
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((Camera3d::default(), MainCamera, Transform::from_xyz(0.0, 3.0, 6.0)));
}

fn grab_cursor(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut c) = cursor.single_mut() else { return };
    if mouse.just_pressed(MouseButton::Left) {
        c.grab_mode = CursorGrabMode::Locked;
        c.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        c.grab_mode = CursorGrabMode::None;
        c.visible = true;
    }
}

fn orbit(
    time: Res<Time>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    gamepads: Query<&Gamepad>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut rig: ResMut<CameraRig>,
) {
    let locked = cursor.single().map(|c| c.grab_mode != CursorGrabMode::None).unwrap_or(false);
    if locked {
        rig.yaw -= motion.delta.x * 0.003;
        rig.pitch -= motion.delta.y * 0.003;
    }
    for gp in &gamepads {
        let s = gp.right_stick();
        rig.yaw -= s.x * 2.5 * time.delta_secs();
        rig.pitch += s.y * 1.8 * time.delta_secs();
    }
    rig.pitch = rig.pitch.clamp(-1.3, 0.6);
    rig.distance = (rig.distance - scroll.delta.y * 0.5).clamp(2.0, 14.0);
    rig.shake = (rig.shake - time.delta_secs() * 2.0).max(0.0);
}

fn follow(
    time: Res<Time>,
    rig: Res<CameraRig>,
    player: Query<&Transform, (With<Player>, Without<MainCamera>)>,
    mut cam: Query<&mut Transform, With<MainCamera>>,
) {
    let (Ok(p), Ok(mut c)) = (player.single(), cam.single_mut()) else { return };
    let focus = p.translation + Vec3::Y * 1.5;
    let rot = Quat::from_euler(EulerRot::YXZ, rig.yaw, rig.pitch, 0.0);
    let t = time.elapsed_secs();
    let shake = Vec3::new((t * 53.0).sin(), (t * 71.0).sin(), 0.0) * rig.shake * 0.08;
    let wanted = focus + rot * Vec3::new(0.0, 0.0, rig.distance) + shake;
    let k = 1.0 - (-12.0 * time.delta_secs()).exp();
    c.translation = c.translation.lerp(wanted, k);
    c.look_at(focus, Vec3::Y);
}
