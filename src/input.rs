//! Player input → "pad", mirroring GoAssassinActionInterpreter (RE/01 §6).
//!
//! The game reads a virtual pad: left stick (camera-relative), High Profile modifier
//! (ShoulderRight1 = RT / right mouse) and the "Legs" button (Button1 = A, with a 0.3 s buffer).
//! Keyboard/mouse: WASD = stick, Right mouse = High Profile, Space = Legs, Left Alt = walk-slow.
//! Gamepad: left stick, RT = High Profile, A = Legs.

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;

use crate::camera::CameraRig;
use crate::tuning::*;

#[derive(Resource, Default, Debug)]
pub struct PadInput {
    /// Camera-relative wanted direction on the ground plane (unit or zero).
    pub dir: Vec3,
    /// Raw stick magnitude 0..1.
    pub magnitude: f32,
    /// speed01 = clamp((|stick| - 0.35) / 0.65, 0, 1)  (0xEE65A0)
    pub speed01: f32,
    pub high_profile: bool,
    pub legs_held: bool,
    /// Seconds since Legs was last pressed (jump buffer, 0.3 s).
    pub legs_pressed_ago: f32,
}

impl PadInput {
    pub fn jump_buffered(&self) -> bool {
        self.legs_pressed_ago <= JUMP_BUFFER
    }
    pub fn consume_jump(&mut self) {
        self.legs_pressed_ago = f32::INFINITY;
    }
}

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PadInput { legs_pressed_ago: f32::INFINITY, ..default() })
            .add_systems(PreUpdate, read_pad);
    }
}

pub fn read_pad(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    rig: Res<CameraRig>,
    mut pad: ResMut<PadInput>,
) {
    let mut stick = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        stick.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        stick.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        stick.x += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        stick.x -= 1.0;
    }
    stick = stick.normalize_or_zero();
    if keys.pressed(KeyCode::AltLeft) {
        stick *= 0.6; // partial stick: lets keyboard players reach the walk-slow range
    }
    let mut high = mouse.pressed(MouseButton::Right);
    let mut legs = keys.pressed(KeyCode::Space);
    let mut legs_just = keys.just_pressed(KeyCode::Space);

    for gp in &gamepads {
        let s = gp.left_stick();
        if s.length() > stick.length() {
            stick = s;
        }
        high |= gp.pressed(GamepadButton::RightTrigger2) || gp.pressed(GamepadButton::RightTrigger);
        legs |= gp.pressed(GamepadButton::South);
        legs_just |= gp.just_pressed(GamepadButton::South);
    }

    let mag = stick.length().min(1.0);
    // camera-relative: stick up = away from the camera
    let forward = Vec3::new(-rig.yaw.sin(), 0.0, -rig.yaw.cos());
    let right = Vec3::new(forward.z * -1.0, 0.0, forward.x);
    let dir = (forward * stick.y + right * stick.x).normalize_or_zero();

    pad.dir = dir;
    pad.magnitude = mag;
    pad.speed01 = if mag <= STICK_DEADZONE { 0.0 } else { ((mag - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).clamp(0.0, 1.0) };
    pad.high_profile = high;
    pad.legs_held = legs;
    if legs_just {
        pad.legs_pressed_ago = 0.0;
    } else {
        pad.legs_pressed_ago += time.delta_secs();
    }
}
