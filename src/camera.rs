//! Simple third-person orbit camera. Mouse (cursor locked) or right stick orbits; scroll zooms.
//! Esc releases the cursor, left click captures it again.
//! PORT: orbit, collision and follow point remain stand-ins. Target/eye lag matches the default
//! LowHighProfile camera (RE/15 §6.2–6.3), not the debug NavigationCamera.

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
            .add_systems(Update, (grab_cursor, orbit, follow.after(crate::ik::IkSet)).chain().after(crate::player::PlayerSet));
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((Camera3d::default(), MainCamera, Transform::from_xyz(0.0, 3.0, 6.0)));
}

fn grab_cursor(
    menu: Res<crate::map_menu::MapMenu>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if menu.open { return; }
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
    menu: Res<crate::map_menu::MapMenu>,
    time: Res<Time>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    gamepads: Query<&Gamepad>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut rig: ResMut<CameraRig>,
) {
    if menu.open { return; }
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

/// Math__LagToward 0x54D950 / Math__LagTowardVec 0x54DBF0, independently per axis.
fn lag_toward(x: Vec3, raw: Vec3, lag: Vec3, dt: f32) -> Vec3 {
    (x * lag + raw * dt) / (lag + Vec3::splat(dt))
}

#[derive(Default)]
struct FollowLag {
    target: Vec3,
    eye: Vec3,
    last_raw: Option<Vec3>,
}

impl FollowLag {
    fn target(&mut self, raw: Vec3, dt: f32) -> bool {
        // Activate 0x6910C0 snaps initially. PORT: also snap across >5 m respawn/map-switch jumps.
        let snap = self.last_raw.is_none_or(|last| last.distance(raw) > 5.0);
        self.last_raw = Some(raw);
        // DataPC / Game Bootstrap Settings / Camera Switcher, LowHighProfileCameraSettings +596/+384.
        self.target = if snap { raw } else { lag_toward(self.target, raw, Vec3::new(0.1, 0.3, 0.1), dt) };
        snap
    }

    fn eye(&mut self, raw: Vec3, dt: f32, snap: bool) -> Vec3 {
        // FreeRoamingCamera__SmoothEye 0x690780; LowHighProfileCameraSettings +552.
        self.eye = if snap { raw } else { lag_toward(self.eye, raw, Vec3::splat(0.125), dt) };
        self.eye
    }
}

fn follow(
    collision: Res<crate::collision::CollisionWorld>,
    time: Res<Time>,
    rig: Res<CameraRig>,
    player: Query<&Transform, (With<Player>, Without<MainCamera>)>,
    mut cam: Query<&mut Transform, With<MainCamera>>,
    mut lag: Local<FollowLag>,
) {
    let (Ok(p), Ok(mut c)) = (player.single(), cam.single_mut()) else { return };
    // PORT: tracker bone/rotated offset in Camera Switcher is not decoded (CameraBase__UpdateTarget 0x5DD2A0).
    let raw_focus = p.translation + Vec3::Y * 1.5;
    let dt = time.delta_secs();
    let snap = lag.target(raw_focus, dt);
    let focus = if crate::tuning::GAME_SMOOTHING { lag.target } else { raw_focus };
    let rot = Quat::from_euler(EulerRot::YXZ, rig.yaw, rig.pitch, 0.0);
    let t = time.elapsed_secs();
    let shake = Vec3::new((t * 53.0).sin(), (t * 71.0).sin(), 0.0) * rig.shake * 0.08;
    let direction = rot * Vec3::Z;
    // PORT: source scenes need obstruction avoidance; keep the existing greybox camera unchanged.
    let distance = if collision.triangles.is_empty() { rig.distance }
        else { (collision.camera_distance(focus,direction,rig.distance)-0.20).clamp(0.25,rig.distance) };
    // FreeRoamingCamera__Update 0x68ED60: build the raw eye from the lagged target, then lag the eye.
    let raw_eye = focus + direction * distance;
    c.translation = if crate::tuning::GAME_SMOOTHING {
        lag.eye(raw_eye, dt, snap) + shake
    } else {
        // PORT: previous orbit camera, retained only for comparison with GAME_SMOOTHING disabled.
        c.translation.lerp(raw_eye + shake, 1.0 - (-12.0 * dt).exp())
    };
    // (hypothesis): view orientation was not traced; aim at the smoothed target.
    c.look_at(focus, Vec3::Y);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_lag_matches_the_native_formula_per_axis() {
        let raw = Vec3::splat(1.0);
        let lag = Vec3::new(0.1, 0.3, 0.1);
        let actual = lag_toward(Vec3::ZERO, raw, lag, 0.1);
        assert!((actual - Vec3::new(0.5, 0.25, 0.5)).length() < 1e-6);
        let actual = lag_toward(Vec3::splat(2.0), raw, Vec3::splat(0.125), 0.125);
        assert!((actual - Vec3::splat(1.5)).length() < 1e-6);
        assert_eq!(lag_toward(raw, Vec3::ZERO, lag, 0.0), raw);
    }

    #[test]
    fn camera_lags_the_eye_from_the_smoothed_target_and_snaps_on_activation() {
        let mut lag = FollowLag::default();
        let origin = Vec3::Y * 1.5;
        let snap = lag.target(origin, 0.1);
        assert!(snap);
        assert_eq!(lag.target, origin);
        let eye = origin + Vec3::Z * 5.5;
        assert_eq!(lag.eye(eye, 0.1, snap), eye);
        let snap = lag.target(origin + Vec3::ONE, 0.1);
        assert!(!snap);
        let expected_target = origin + Vec3::new(0.5, 0.25, 0.5);
        assert!((lag.target - expected_target).length() < 1e-6);
        let raw_eye = lag.target + Vec3::Z * 5.5;
        let expected_eye = (eye * 0.125 + raw_eye * 0.1) / 0.225;
        assert!((lag.eye(raw_eye, 0.1, snap) - expected_eye).length() < 1e-6);
        let raw = origin + Vec3::X * 20.0;
        let snap = lag.target(raw, 0.1);
        assert!(snap);
        assert_eq!(lag.target, raw);
        assert_eq!(lag.eye(raw + Vec3::Z * 5.5, 0.1, snap), raw + Vec3::Z * 5.5);
    }

    #[test]
    fn camera_teleport_gate_uses_consecutive_raw_points_and_is_strictly_over_five_metres() {
        let mut lag = FollowLag::default();
        assert!(lag.target(Vec3::ZERO, 0.0));
        assert!(!lag.target(Vec3::X * 5.0, 0.0));
        assert!(!lag.target(Vec3::X * 10.0, 0.0), "lag error is not a teleport");
        assert!(lag.target(Vec3::X * 15.01, 0.0));
    }

    #[test]
    fn camera_plugin_reads_the_visual_after_ik() {
        fn move_visual(mut q: Query<&mut Transform, With<Player>>) {
            q.single_mut().unwrap().translation.y = 0.3;
        }
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<crate::collision::CollisionWorld>()
            .init_resource::<crate::map_menu::MapMenu>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<AccumulatedMouseScroll>()
            .add_plugins(CameraPlugin)
            .add_systems(Update, move_visual.in_set(crate::ik::IkSet));
        app.world_mut().spawn((Player, Transform::default()));
        app.update();
        let world = app.world_mut();
        let rig = world.resource::<CameraRig>();
        let expected = Vec3::Y * 1.8 + Quat::from_euler(EulerRot::YXZ, rig.yaw, rig.pitch, 0.0) * Vec3::Z * rig.distance;
        let eye = world.query_filtered::<&Transform, With<MainCamera>>().single(world).unwrap().translation;
        assert!((eye - expected).length() < 1e-6, "follow must see this frame's IK-adjusted visual");
    }
}
