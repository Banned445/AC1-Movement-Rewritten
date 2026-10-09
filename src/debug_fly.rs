//! Ghost mode: the game's development flight, kept as a debug tool for the port's testers (RE/18 §2).
//!
//! The Human **Debug context** (ContextID_Debug = 3, `DebugContext`) is still in v1.02 but unreachable: its input
//! interpreter `GoDebugInputInterpreter` only exists when loaded data references it, and the retail game has none
//! (checked live). The port turns it on with F5 or the game's own chord (L3 + R3).
//!
//! - **Enter** (0xE46190): the capsule goes to collision layer 10 (NOTHING: no collision), stick-to-ground and the
//!   step offset off, action request 7 = 0x0006F88B "Ghost mode" (`Ghost_mode01`, then the looping `Ghost_mode02`),
//!   ActorState 38 (Debug).
//! - **Input** (`GoDebugInputInterpreter__FlyInput` 0xECDC50, pad indices from `PadXenon` 0x98CE60): the move is the
//!   left stick on the ground plane (its direction × its magnitude), up / down the right stick's Y (zero while LB,
//!   pad 11, is held), all × 5 m/s; × 3 with RT (12), × 10 with RB (13). X (1) held: IHumanDebug vt16 0xE45FF0 sets
//!   DebugContextData +36, so the body keeps its facing (a strafe). R3 (15) pressed: `DebugCameraToggleEvent`.
//! - **Update** (`DebugContext__Update` 0xE46DF0): velocity = the move × the multiplier; the body turns toward the
//!   horizontal motion when its square exceeds 0.1 and +36 is clear; +36 is cleared every frame.
//! - **Leave** (chord L3 + R3 0xECD1E0 → vt20 0xE473B0 / vt24 0xE473E0 → `DebugContext__Exit` 0xE467B0): request 33
//!   `xx_h_jump_falling01`, the controller velocity zeroed, the character's layer back, then InAir. With B (3) pressed
//!   in the same frame the InAir setup carries the flight's direction and speed (thrown out of the flight); without,
//!   a fall from rest. A `ResurrectionEvent` follows (the port has no health: nothing to revive).
//!
//! PORT: the player contexts do not run while flying (`PlayerSet` is skipped); the body is moved here. The debug
//! camera's own behaviour is not decoded: in the port it holds its position and keeps looking at Altaïr. While
//! flying, the gamepad's right stick Y drives the height and not the camera pitch, unless LB is held.
//!
//! Keyboard: F5 = enter / leave (hold E while leaving: keep the flight's speed), WASD = move, Space / Left Ctrl = up /
//! down, Left Shift = × 3, Left Alt = × 10, Q held = strafe, F6 = debug camera.
//! Gamepad: L3 + R3 = enter / leave (B with it: keep the speed), left stick = move, right stick Y = up / down, LB =
//! hold the height, RT = × 3, RB = × 10, X held = strafe, R3 = debug camera.

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;

use crate::camera::CameraRig;
use crate::input::PadInput;
use crate::player::air::{FallOrigin, InAirEntry};
use crate::player::{switch_context, Body, HumanDataBundle, LimbTargets, Locomotion, Player, PlayerSet, TransitionSetup};

/// The Debug context's speed: the input × 5 m/s (0xECDC50).
const FLY_SPEED: f32 = 5.0;
/// × 3 and × 10 with RT / RB (pad 12 / 13, 0xECDC50).
const FLY_FAST: f32 = 3.0;
const FLY_FASTER: f32 = 10.0;
/// The body turns toward the motion when its horizontal square exceeds this (0xE46DF0).
const TURN_MIN_SQ: f32 = 0.1;
/// Action request 7 (HumanGround): `Ghost_mode01` (0.93 s), `Ghost_mode02` (2.27 s, looping).
pub const GHOST_MODE: u32 = 0x0006_F88B;

#[derive(Resource, Default)]
pub struct DebugFly {
    pub active: bool,
    /// The flight's velocity this frame (carried out by a B / E exit).
    pub velocity: Vec3,
    /// The debug camera (`DebugCameraToggleEvent`): the camera holds still.
    pub free_camera: bool,
    /// The gamepad's LB: the right stick is the camera's, not the height's.
    pub hold_height: bool,
}

pub struct DebugFlyPlugin;

impl Plugin for DebugFlyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugFly>()
            .configure_sets(Update, PlayerSet.run_if(|fly: Res<DebugFly>| !fly.active))
            .add_systems(
                Update,
                (toggle_fly, fly)
                    .chain()
                    .before(PlayerSet)
                    .run_if(|menu: Res<crate::map_menu::MapMenu>| !menu.open),
            );
        // AC_GHOST=1: the flight turns itself on after 1 s (for captures of the Ghost mode animation)
        if std::env::var_os("AC_GHOST").is_some() {
            app.add_systems(Update, ghost_autostart.before(toggle_fly));
        }
    }
}

fn ghost_autostart(time: Res<Time>, mut keys: ResMut<ButtonInput<KeyCode>>, mut done: Local<bool>) {
    if !*done && time.elapsed_secs() >= 1.0 {
        *done = true;
        keys.press(KeyCode::F5);
    }
}

/// `ControlScheme__Chord2` 0xECD1E0: one button pressed this frame while the other is held.
fn chord(gp: &Gamepad, a: GamepadButton, b: GamepadButton) -> bool {
    (gp.just_pressed(a) && gp.pressed(b)) || (gp.just_pressed(b) && gp.pressed(a))
}

fn toggle_fly(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut fly: ResMut<DebugFly>,
    mut pad: ResMut<PadInput>,
    mut q: Query<(&mut Locomotion, &mut HumanDataBundle, &mut Body, &mut LimbTargets, Option<&mut crate::ik::LimbIk>), With<Player>>,
) {
    let pad_chord = gamepads.iter().any(|gp| chord(gp, GamepadButton::LeftThumb, GamepadButton::RightThumb));
    if !keys.just_pressed(KeyCode::F5) && !pad_chord {
        // R3 / F6 alone while flying: the debug camera
        let cam = keys.just_pressed(KeyCode::F6) || gamepads.iter().any(|gp| gp.just_pressed(GamepadButton::RightThumb) && !gp.pressed(GamepadButton::LeftThumb));
        if fly.active && cam {
            fly.free_camera = !fly.free_camera;
        }
        return;
    }
    let Ok((mut loco, mut data, mut body, mut limbs, ik)) = q.single_mut() else { return };
    // the empty hand (E / B) with the toggle: leave with the flight's velocity (0xE467B0, vt24 argument 1)
    let keep = keys.pressed(KeyCode::KeyE) || gamepads.iter().any(|gp| gp.pressed(GamepadButton::East));
    fly.active = !fly.active;
    fly.free_camera = false;
    body.tilt = Quat::IDENTITY;
    body.stick_residual = 0.0;
    body.stick_normal_y = None;
    *limbs = LimbTargets::default();
    if fly.active {
        // enter (0xE46190): from any context
        body.velocity = Vec3::ZERO;
        body.grounded = false;
        fly.velocity = Vec3::ZERO;
        switch_context(&mut loco, &mut data, TransitionSetup::ToDebug);
        return;
    }
    // leave (0xE467B0): the manifold, foot probes and pelvis drop belong to where the flight started
    body.proxy = default();
    if let Some(mut ik) = ik {
        *ik = default();
    }
    // Space / A (up) and E / B also fed the jump and hand buffers while flying
    pad.consume_jump();
    pad.consume_hand();
    let velocity = if keep { fly.velocity } else { Vec3::ZERO };
    body.velocity = velocity;
    body.grounded = false;
    let entry = InAirEntry::Fall { from: body.feet, velocity, origin: FallOrigin::Ground, speed_param: 0.0 };
    switch_context(&mut loco, &mut data, TransitionSetup::ToInAir(entry));
}

fn fly(
    time: Res<Time>,
    mut fly: ResMut<DebugFly>,
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    rig: Res<CameraRig>,
    mut q: Query<(&mut Body, &mut Transform), With<Player>>,
) {
    if !fly.active {
        return;
    }
    let Ok((mut body, mut tr)) = q.single_mut() else { return };
    let key = |k: KeyCode| keys.pressed(k) as i32 as f32;
    let mut stick = Vec2::new(key(KeyCode::KeyD) - key(KeyCode::KeyA), key(KeyCode::KeyW) - key(KeyCode::KeyS));
    let mut lift = key(KeyCode::Space) - key(KeyCode::ControlLeft);
    let mut scale = if keys.pressed(KeyCode::AltLeft) { FLY_FASTER } else if keys.pressed(KeyCode::ShiftLeft) { FLY_FAST } else { 1.0 };
    let mut strafe = keys.pressed(KeyCode::KeyQ);
    let mut hold = false;
    for gp in &gamepads {
        let s = gp.left_stick();
        if s.length() > stick.length() {
            stick = s;
        }
        // LB (pad 11) held: no height change; else the right stick's Y
        if gp.pressed(GamepadButton::LeftTrigger) {
            hold = true;
        } else if gp.right_stick().y.abs() > lift.abs() {
            lift = gp.right_stick().y;
        }
        if gp.pressed(GamepadButton::RightTrigger) {
            scale = FLY_FASTER;
        } else if gp.pressed(GamepadButton::RightTrigger2) {
            scale = scale.max(FLY_FAST);
        }
        strafe |= gp.pressed(GamepadButton::West);
    }
    fly.hold_height = hold;
    // the stick's direction × its magnitude on the ground plane, relative to the camera's heading
    let stick = stick.clamp_length_max(1.0);
    let flat = Vec3::new(-rig.yaw.sin(), 0.0, -rig.yaw.cos());
    let right = crate::player::right_of(flat);
    let wish = flat * stick.y + right * stick.x + Vec3::Y * lift.clamp(-1.0, 1.0);
    let v = wish * FLY_SPEED * scale;
    body.feet += v * time.delta_secs();
    fly.velocity = v;
    // the body turns toward the horizontal motion unless X / Q is held (0xE46DF0, DebugContextData +36)
    let h = Vec3::new(v.x, 0.0, v.z);
    if !strafe && h.length_squared() > TURN_MIN_SQ {
        body.heading = crate::player::heading_of(h.normalize());
    }
    body.velocity = Vec3::ZERO;
    body.grounded = false;
    // `sync_visuals` is in the skipped PlayerSet
    tr.translation = body.feet;
    tr.rotation = Quat::from_rotation_y(body.heading);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::CollisionWorld;
    use crate::player::ActorContextId;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct ContextRuns(u32);

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(100)))
            .insert_resource(ButtonInput::<KeyCode>::default())
            .insert_resource(crate::camera::CameraRig { pitch: -0.8, ..default() })
            .insert_resource(crate::map_menu::neutral_pad())
            .insert_resource(crate::map_menu::MapMenu::default())
            .init_resource::<ContextRuns>()
            .add_plugins(DebugFlyPlugin)
            .add_systems(Update, (|mut runs: ResMut<ContextRuns>| runs.0 += 1).in_set(PlayerSet));
        let mut collision = CollisionWorld::default();
        collision.boxes.push(crate::collision::Aabb3 { min: Vec3::new(-50.0, -1.0, -50.0), max: Vec3::new(50.0, 2.0, 50.0) });
        app.insert_resource(collision);
        app.world_mut().spawn((crate::player::player_components(Vec3::new(0.0, 2.0, 0.0), 0.0), Transform::default()));
        app.update(); // the first update has no delta
        app
    }

    fn step(app: &mut App, keys: &[KeyCode]) {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.release_all();
        input.clear();
        for k in keys {
            input.press(*k);
        }
        app.update();
    }

    fn player<'a>(app: &'a mut App) -> (&'a Locomotion, &'a Body) {
        let world = app.world_mut();
        let e = world.query_filtered::<Entity, With<Player>>().single(world).unwrap();
        let entity = world.entity(e);
        (entity.get::<Locomotion>().unwrap(), entity.get::<Body>().unwrap())
    }

    #[test]
    fn ghost_mode_flies_on_the_ground_plane_through_the_world_and_falls_out() {
        let mut app = app();
        let runs = app.world().resource::<ContextRuns>().0;
        step(&mut app, &[KeyCode::F5]);
        assert!(app.world().resource::<DebugFly>().active);
        assert_eq!(player(&mut app).0.current, ActorContextId::Debug);
        assert_eq!(app.world().resource::<ContextRuns>().0, runs, "PlayerSet is skipped while flying");
        // up 10 frames (1 s)
        for _ in 0..10 {
            step(&mut app, &[KeyCode::Space]);
        }
        let y = player(&mut app).1.feet.y;
        assert!((y - (2.0 + FLY_SPEED)).abs() < 1e-3, "{y}");
        // forward with the camera pitched down: the move stays on the ground plane (0xECDC50), through the solid box
        for _ in 0..10 {
            step(&mut app, &[KeyCode::KeyW]);
        }
        let feet = player(&mut app).1.feet;
        assert!((feet.y - y).abs() < 1e-4 && (feet.z - FLY_SPEED).abs() < 1e-3, "{feet}");
        // leaving falls from rest into InAir (0xE467B0), not onto the floor
        step(&mut app, &[KeyCode::F5]);
        assert!(!app.world().resource::<DebugFly>().active);
        let (loco, body) = player(&mut app);
        assert_eq!(loco.current, ActorContextId::InAir);
        assert!(!body.grounded && body.velocity.length() < 1e-6, "{:?}", body.velocity);
        assert!(app.world().resource::<PadInput>().legs_pressed_ago.is_infinite(), "flying up must not leave a buffered jump");
        step(&mut app, &[]);
        assert!(app.world().resource::<ContextRuns>().0 > runs);
    }

    #[test]
    fn leaving_with_the_empty_hand_keeps_the_flights_velocity() {
        let mut app = app();
        step(&mut app, &[KeyCode::F5]);
        step(&mut app, &[KeyCode::KeyW, KeyCode::ShiftLeft]);
        step(&mut app, &[KeyCode::KeyW, KeyCode::ShiftLeft, KeyCode::KeyE, KeyCode::F5]);
        let (loco, body) = player(&mut app);
        assert_eq!(loco.current, ActorContextId::InAir);
        assert!((body.velocity.z - FLY_SPEED * FLY_FAST).abs() < 1e-3, "thrown out at the flight's speed: {:?}", body.velocity);
    }

    #[test]
    fn strafing_keeps_the_facing() {
        let mut app = app();
        step(&mut app, &[KeyCode::F5]);
        let h0 = player(&mut app).1.heading;
        for _ in 0..5 {
            step(&mut app, &[KeyCode::KeyD, KeyCode::KeyQ]);
        }
        assert!((player(&mut app).1.heading - h0).abs() < 1e-6, "Q held: no turn");
        step(&mut app, &[KeyCode::KeyD]);
        assert!((player(&mut app).1.heading - h0).abs() > 0.5, "without Q the body turns toward the motion");
    }

    #[test]
    fn f6_toggles_the_debug_camera_while_flying() {
        let mut app = app();
        step(&mut app, &[KeyCode::F6]);
        assert!(!app.world().resource::<DebugFly>().free_camera, "only in Ghost mode");
        step(&mut app, &[KeyCode::F5]);
        step(&mut app, &[KeyCode::F6]);
        assert!(app.world().resource::<DebugFly>().free_camera);
        step(&mut app, &[KeyCode::F5]);
        assert!(!app.world().resource::<DebugFly>().free_camera, "leaving resets it");
    }
}
