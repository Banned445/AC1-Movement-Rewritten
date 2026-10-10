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

use bevy::input::gamepad::{AxisSettings, Gamepad, GamepadButton, GamepadSettings};
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
    /// The movement stick after the dead zone (pad stick 0; the climb camera reads it, 0xC5E370).
    pub stick: Vec2,
    /// Interp +0x1300: the stick latched by the interpreter (pad axes, x right, y up; `player::interpreter`).
    pub stick_latched: Vec2,
    /// Interp +0x1310: the latched stick with x mirrored when the camera faces the character (`sub_EDB450`). The
    /// ledge, climb and ladder read it in the character's frame (RE/24 INPUT-4).
    pub stick_actor: Vec2,
    /// Interp +0x1129: the empty-hand buffer, kept by `player::interpreter` (RE/24 INPUT-2).
    pub hand_buffer: bool,
    /// Interp +0x1250 (+0x400): the camera-relative direction snapped to straight ahead within 5° (`sub_55E7E0`); the
    /// jump-target searches, the jump-offs and InAir read it (RE/24 INPUT-6). Zero below the dead zone.
    pub dir_snap: Vec3,
    /// Interp +0x1270 (+0x540): the camera-relative direction through the stick's response curve (5° → 1°, 10° → 5°,
    /// 20° → 10°, 30° → 20°, 40° and beyond unchanged, `sub_55EDF0`); the ground steers by it (RE/24 INPUT-6).
    pub dir_move: Vec3,
    /// Interp +0x824 / +0x826: the stick's magnitude band this frame and the last (0: ≤ 0.35, 1: ≤ 0.9, 2: ≤ 1.0;
    /// `sub_EE1710` → 0x717B70, `sub_55EAE0` 0x55ED93; RE/24 INPUT-12).
    pub band: u8,
    pub band_prev: u8,
    /// The camera yaw `dir` was built with (camera manager +0xE0): the live yaw, or the one before a camera cut while
    /// the input latch holds. `None`: the live `CameraRig` yaw (scripted tests).
    pub input_yaw: Option<f32>,
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
    /// The empty-hand buffer (interp +0x1129, `GoAssassinActionInterpreter__ReadInput` 0xEEE0F8 / 0xEEE125): set on
    /// the press frame and kept only while Legs was pressed within 0.3 s, so in practice "pressed this frame"; ignored
    /// for 0.2 s after grabbing (`player::interpreter`).
    pub fn hand_buffered(&self) -> bool {
        self.hand_buffer
    }
    pub fn consume_hand(&mut self) {
        self.hand_pressed_ago = f32::INFINITY;
        self.hand_buffer = false;
    }
    /// The interpreter's stick in the character's frame as a world direction (ledge / climb / ladder:
    /// `GoAssassinActionInterpreter__LedgeState` 0xEEC356, `ClimbState` 0xEE6036, `LadderState` 0xEEB696): stick up =
    /// `facing`, right = the character's right. `unmirrored` takes +0x1300 instead of +0x1310 (the ledge with RB held,
    /// 0xEEC256). Zero below the dead zone.
    pub fn actor_dir(&self, facing: Vec3, unmirrored: bool) -> Vec3 {
        let s = if unmirrored { self.stick_latched } else { self.stick_actor };
        let f = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
        let r = Vec3::new(-f.z, 0.0, f.x);
        (f * s.y + r * s.x).normalize_or_zero()
    }
}

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PadInput { legs_pressed_ago: f32::INFINITY, hand_pressed_ago: f32::INFINITY, ..default() })
            .add_systems(PreUpdate, (raw_gamepad_axes, read_pad).chain());
    }
}

/// The game reads the thumbsticks unfiltered (`PadXenon__Update` 0x98CE60: thumb / 32767) and applies only its own
/// 0.35 radial dead zone. Bevy's default axis settings rescale each axis from a 0.05 dead zone and drop changes under
/// 0.01, which would move the game's bands (0.35 / 0.9); every gamepad gets pass-through settings (RE/24 INPUT-11).
fn raw_gamepad_axes(mut pads: Query<&mut GamepadSettings, Added<GamepadSettings>>) {
    for mut settings in &mut pads {
        if let Ok(raw) = AxisSettings::new(-1.0, 0.0, 0.0, 1.0, 0.0) {
            settings.default_axis_settings = raw;
        }
    }
}

/// The camera manager's input latch after a camera cut (+0xD4, +0xE0 the input orientation, +0xF0 the stored stick).
#[derive(Default)]
pub struct CutLatch {
    on: bool,
    yaw: Option<f32>,
    stick: Vec2,
}

impl CutLatch {
    /// The yaw the stick is turned by this frame. A cut latches the yaw of the camera before it (`sub_4BC360`); the
    /// latch drops when the stick's direction changes by more than acos 0.9 or it is released (`sub_4BFD80`:
    /// 0x4BFE80), or when the live camera's yaw comes within 0.1 rad of the latched one (`sub_4BF7A0` 0x4BF893;
    /// `Quat__ToAngles` 0x4BCB10's third angle, 2(wz + xy), is the rotation about the game's up axis).
    pub fn update(&mut self, cut: bool, live_yaw: f32, stick: Vec2) -> f32 {
        if cut {
            self.on = true;
        }
        if stick.length_squared() > 0.0005 * 0.0005 {
            if self.stick.normalize_or_zero().dot(stick.normalize_or_zero()) <= 0.9 {
                self.on = false;
            }
            if !self.on {
                self.stick = stick;
            }
        } else {
            self.on = false;
            self.stick = Vec2::ZERO;
        }
        let held = self.yaw.unwrap_or(live_yaw);
        if self.on && crate::camera::wrap(live_yaw - held).abs() < 0.1 {
            self.on = false;
        }
        if !self.on {
            self.yaw = Some(live_yaw);
        }
        self.yaw.unwrap_or(live_yaw)
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
    abilities: Option<Res<crate::player::abilities::AbilitySet>>,
    bindings: Option<Res<crate::bindings::Bindings>>,
    mut pad: ResMut<PadInput>,
    mut toggle: Local<(bool, f32)>,
    mut cut: Local<CutLatch>,
    mut rig_cut: ResMut<CameraRig>,
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
    // camera-relative: stick up = away from the camera, in the input frame (held across a camera cut)
    let yaw = cut.update(std::mem::take(&mut rig_cut.cut), rig_cut.yaw, stick);
    pad.input_yaw = Some(yaw);
    let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let right = Vec3::new(forward.z * -1.0, 0.0, forward.x);
    let dir = (forward * stick.y + right * stick.x).normalize_or_zero();
    // without the HighProfile ability the flag is forced off (`ReadInput` 0xEEDEC4)
    let high = buttons[12] && crate::player::abilities::of(abilities.as_deref()).allows_high_profile();

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
    pad.stick = stick;
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_gamepad_axes_pass_through_unfiltered() {
        // no per-axis dead zone, no change threshold: only the game's 0.35 radial dead zone applies
        assert!(AxisSettings::new(-1.0, 0.0, 0.0, 1.0, 0.0).is_ok());
        assert_eq!(stick_response(Vec2::new(0.3, 0.1)), Vec2::ZERO);
        assert_eq!(stick_response(Vec2::new(0.4, 0.0)), Vec2::new(0.4, 0.0));
    }

    #[test]
    fn a_camera_cut_keeps_the_held_stick_in_the_old_frame() {
        let mut l = CutLatch::default();
        let up = Vec2::new(0.0, 1.0);
        assert_eq!(l.update(false, 0.0, up), 0.0);
        // the cut: the new camera is 90° round; the held stick keeps the old frame
        assert_eq!(l.update(true, 1.57, up), 0.0);
        assert_eq!(l.update(false, 1.57, up * 0.9), 0.0);
        // a turn of the stick beyond acos 0.9 releases it
        assert_eq!(l.update(false, 1.57, Vec2::new(0.6, 0.8)), 1.57);
        // a released stick releases it too
        l.update(true, 3.0, up);
        assert_eq!(l.update(false, 3.0, Vec2::ZERO), 3.0);
        // and so does the camera coming back within 0.1 rad
        l.update(false, 0.0, up);
        assert_eq!(l.update(true, 1.0, up), 0.0);
        assert_eq!(l.update(false, 0.05, up), 0.05);
    }
}
