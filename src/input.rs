//! Player input → "pad", mirroring GoAssassinActionInterpreter (RE/01 §6).
//!
//! The game reads a virtual pad (RE/21 §1). The keyboard and mouse fill it from the user's binding profile
//! (`bindings.rs`: DefaultBindings.map, by default KeyboardMouse2: WASD = left stick, Space = Legs (A), LMB = weapon hand
//! (X), E = head (Y), Left Shift = empty hand (B), RMB = high profile (RT), arrows = right stick). A gamepad fills it as
//! PadXenon (0x98CE60): A / X / Y / B = 0–3, D-pad 4–7, Back / Start 8 / 9, LT / LB / RT / RB 10–13 (triggers above
//! 30/255), L3 / R3 14 / 15.
//! Sticks (`sub_93FC80`): a radial dead zone of 0.35 (Pad +288, 0x940340) zeroes the stick; above it the value is kept
//! as is, and a length over 1 is normalised. Keys give ±1 per axis (0x90A780), so diagonals are unit length.
//! PORT: Left Alt scales the keyboard stick by 0.6 so keyboard players can reach the walk-slow band (no game key).

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
    /// Empty-hand held (pad button 3): falling grab, 0xEDBAF0 -> 0xE102D0.
    pub hand_held: bool,
    /// Seconds since Legs was last pressed (jump buffer, 0.3 s).
    pub legs_pressed_ago: f32,
    /// Seconds since the empty-hand button (pad button 3) was last pressed.
    pub hand_pressed_ago: f32,
    /// The pad's 16 buttons this frame (PadXenon order, RE/21 §1).
    pub buttons: [bool; 16],
    /// The right stick (camera), after the dead zone: arrows or a gamepad's right stick. The mouse delta goes to the
    /// camera directly (0x90A780: the mouse is the right stick's raw counts when no arrow key is down).
    pub cam_stick: Vec2,
}

impl PadInput {
    pub fn jump_buffered(&self) -> bool {
        self.legs_pressed_ago <= JUMP_BUFFER
    }
    pub fn consume_jump(&mut self) {
        self.legs_pressed_ago = f32::INFINITY;
    }
    /// The empty-hand button pressed this frame (interp +0x1129, `Pad__JustPressed(3)`).
    pub fn hand_just_pressed(&self) -> bool {
        self.hand_pressed_ago <= 0.0
    }
    /// The empty-hand buffer (interp +0x1129): set on the press, cleared 0.3 s after it
    /// (`GoAssassinActionInterpreter__ReadInput` 0xEEE0F8 / 0xEEE125), like the Legs buffer.
    pub fn hand_buffered(&self) -> bool {
        self.hand_pressed_ago <= JUMP_BUFFER
    }
    pub fn consume_hand(&mut self) {
        self.hand_pressed_ago = f32::INFINITY;
    }
}

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PadInput { legs_pressed_ago: f32::INFINITY, hand_pressed_ago: f32::INFINITY, ..default() })
            .add_systems(PreUpdate, read_pad);
    }
}

/// `sub_93FC80`: the radial dead zone (0.35) and the normalisation above 1.
pub fn stick_response(s: Vec2) -> Vec2 {
    let l2 = s.length_squared();
    if l2 < STICK_DEADZONE * STICK_DEADZONE {
        Vec2::ZERO
    } else if l2 > 1.0 {
        s / l2.sqrt()
    } else {
        s
    }
}

pub fn read_pad(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    rig: Res<CameraRig>,
    abilities: Option<Res<crate::player::abilities::AbilitySet>>,
    bindings: Option<Res<crate::bindings::Bindings>>,
    mut pad: ResMut<PadInput>,
    mut toggle: Local<(bool, f32)>,
    menu: Option<Res<crate::map_menu::MapMenu>>,
) {
    use crate::bindings::Slot;
    if menu.is_some_and(|m| m.open) { *pad = crate::map_menu::neutral_pad(); return; }
    let default_bindings = crate::bindings::Bindings::default();
    let b = bindings.as_deref().unwrap_or(&default_bindings);
    let held = |s: Slot| b.pressed(s, &keys, &mouse);
    // the keyboard sticks: ±1 per axis (0x90A780)
    let axis = |pos: Slot, neg: Slot| held(pos) as i32 as f32 - held(neg) as i32 as f32;
    let mut stick = Vec2::new(axis(Slot::LeftStickRight, Slot::LeftStickLeft), axis(Slot::LeftStickUp, Slot::LeftStickDown));
    let mut cam = Vec2::new(axis(Slot::RightStickRight, Slot::RightStickLeft), axis(Slot::RightStickUp, Slot::RightStickDown));
    stick = stick_response(stick);
    cam = stick_response(cam);
    if keys.pressed(KeyCode::AltLeft) {
        stick *= 0.6;
    }
    let mut buttons = [false; 16];
    let mut just = [false; 16];
    for i in 0..16usize {
        // slot i of the profile is pad button i
        let slot = crate::bindings::PAD_SLOTS[i];
        buttons[i] = b.pressed(slot, &keys, &mouse);
        just[i] = b.just_pressed(slot, &keys, &mouse);
    }
    // the high-profile toggle option (0x90B080): a tap under 0.5 s flips it, holding still works
    if b.high_profile_toggle {
        let (on, t) = &mut *toggle;
        if buttons[12] {
            *t += time.delta_secs();
        } else {
            if b.just_released(Slot::HighProfile, &keys, &mouse) && *t > 0.0 && *t < 0.5 {
                *on = !*on;
            }
            *t = 0.0;
        }
        buttons[12] |= *on;
    }

    const PAD: [GamepadButton; 16] = [
        GamepadButton::South, GamepadButton::West, GamepadButton::North, GamepadButton::East,
        GamepadButton::DPadDown, GamepadButton::DPadLeft, GamepadButton::DPadUp, GamepadButton::DPadRight,
        GamepadButton::Select, GamepadButton::Start, GamepadButton::LeftTrigger2, GamepadButton::LeftTrigger,
        GamepadButton::RightTrigger2, GamepadButton::RightTrigger, GamepadButton::LeftThumb, GamepadButton::RightThumb,
    ];
    for gp in &gamepads {
        let s = stick_response(gp.left_stick());
        if s.length() > stick.length() {
            stick = s;
        }
        let c = stick_response(gp.right_stick());
        if c.length() > cam.length() {
            cam = c;
        }
        for (i, btn) in PAD.iter().enumerate() {
            // the triggers count above 30/255 (0x98CE60)
            let down = match btn {
                GamepadButton::LeftTrigger2 | GamepadButton::RightTrigger2 => gp.get(*btn).unwrap_or(0.0) > 30.0 / 255.0,
                _ => gp.pressed(*btn),
            };
            buttons[i] |= down;
            just[i] |= match btn {
                GamepadButton::LeftTrigger2 | GamepadButton::RightTrigger2 => gp.just_pressed(*btn),
                _ => gp.just_pressed(*btn),
            };
        }
    }

    let mag = stick.length().min(1.0);
    // camera-relative: stick up = away from the camera
    let forward = Vec3::new(-rig.yaw.sin(), 0.0, -rig.yaw.cos());
    let right = Vec3::new(forward.z * -1.0, 0.0, forward.x);
    let dir = (forward * stick.y + right * stick.x).normalize_or_zero();
    let high = buttons[12];

    pad.dir = dir;
    pad.magnitude = mag;
    pad.speed01 = if mag <= STICK_DEADZONE { 0.0 } else { ((mag - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).clamp(0.0, 1.0) };
    pad.high_profile = high;
    // the ability stack's MaxSpeed (0xEE65A0: none → 0, Jog in high profile → 0.3)
    pad.speed01 = crate::player::abilities::of(abilities.as_deref()).clamp_speed(pad.speed01, high);
    pad.legs_held = buttons[0];
    pad.hand_held = buttons[3];
    pad.buttons = buttons;
    pad.cam_stick = cam;
    if just[0] {
        pad.legs_pressed_ago = 0.0;
    } else {
        pad.legs_pressed_ago += time.delta_secs();
    }
    if just[3] {
        pad.hand_pressed_ago = 0.0;
    } else {
        pad.hand_pressed_ago += time.delta_secs();
    }
}
