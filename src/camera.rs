//! The player camera: the game's default `LowHighProfileCamera` (vtable 0x16EF81C, on the `FreeRoamingCamera` base
//! 0x169A614), with every value from DataPC / Game Bootstrap Settings / Camera Switcher (RE/21 §2).
//!
//! The camera orbits a smoothed follow point (the player root + 1.6 m) at a yaw and an elevation parameter `e`
//! (0..1). `e` gives both the pitch (-40°..80°, curved) and the distance (lerped between two values that change with
//! the camera's mode). Mouse input turns it directly; the right stick (or the arrow keys) goes through the R6V stick
//! acceleration. While the player moves, the yaw swings lazily behind the motion; the 0.2 m collision sphere pulls the
//! eye in front of walls, keeps manual input out of them and swings the camera round obstacles. Climbing and ladders
//! use the climb mode (farther, target pulled off the wall, the movement stick leads the view); a long fall looks
//! down. Angles here are in the game's convention (Z-up, eye offset (sin yaw, cos yaw), yaw grows to the right);
//! `CameraRig::yaw` keeps the port's convention (π − yaw) for the systems that read it.
//! Esc releases the cursor, left click captures it again.

use bevy::input::gamepad::Gamepad;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use std::f32::consts::{PI, TAU};

use crate::player::Player;

#[derive(Resource)]
pub struct CameraRig {
    /// Port convention: the eye sits at (sin yaw, ·, cos yaw) from the target.
    pub yaw: f32,
    /// Port convention: negative = eye above the target.
    pub pitch: f32,
    pub distance: f32,
    /// A landing's camera-shake intensity, consumed by the camera: `min((drop − 3)/7, 1)` (0xE05940 → HumanFXPack+88,
    /// `FX_Camera_Shake_Assassins_Fall`).
    pub shake: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        // yaw = PI: camera behind a player facing +Z (toward the rooftops)
        Self { yaw: PI, pitch: -0.35, distance: 5.5, shake: 0.0 }
    }
}

/// Camera events from gameplay (the game's PresentationEvent / FallEvent handlers 0xC5E820 / 0xC5E980).
#[derive(Resource, Default)]
pub struct CameraEvents {
    /// ActorState 67 CameraReset (sent by e.g. the wall-run rebound jump 0xE365C0).
    pub reset: bool,
}

#[derive(Component)]
pub struct MainCamera;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraRig>()
            .init_resource::<CameraEvents>()
            .add_systems(Startup, spawn_camera)
            // after the player has moved this frame (otherwise the camera reads this frame's or last frame's position
            // depending on the scheduler: a one-frame judder)
            .add_systems(Update, (grab_cursor, follow.after(crate::ik::IkSet), hide_near_camera).chain().after(crate::player::PlayerSet));
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        // CameraSettings+32 = 0.8 rad, smoothed toward by 0xC5E170 (ω 10); constant for this camera.
        // (hypothesis): the value is the vertical field of view.
        Projection::Perspective(PerspectiveProjection { fov: S.fov, ..default() }),
        MainCamera,
        Transform::from_xyz(0.0, 3.0, 6.0),
    ));
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

// ---------------------------------------------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------------------------------------------

/// LowHighProfileCameraSettings (Camera Switcher object 0x1A2A; FreeRoamingCameraSettings__Serialize 0x5A5DB0, then
/// 0xC5C970) and its PadR6VCameraControl (0x1A4B; serializer 0x5FA620). Offsets are into the settings object.
pub struct Settings {
    /// CameraSettings+176 = (0, 0, 1.6): the follow point above the tracked entity's root (0x5DD2A0).
    pub target_height: f32,
    /// +596 / +384: per-axis target lag T (x, y) / z (`Math__LagToward` 0x54D950 in 0x692430).
    pub target_lag_xy: f32,
    pub target_lag_z: f32,
    /// +552: eye lag T (`FreeRoamingCamera__SmoothEye` 0x690780).
    pub eye_lag: f32,
    /// +32: field of view (rad).
    pub fov: f32,
    /// +408 / +412 / +416: pitch = lo + (hi − lo)·e^exp (0xC5E200).
    pub pitch_lo: f32,
    pub pitch_hi: f32,
    pub pitch_exp: f32,
    /// +388 / +392: distance at e = 0 / e = 1 on the ground (0xC5E040).
    pub dist_ground: [f32; 2],
    /// +780 / +784: the same while climbing.
    pub dist_climb: [f32; 2],
    /// +500: e at activation (0x68F8E0, +496 = 1).
    pub e_default: f32,
    /// +288 / +320: look-at offset at e = 0 / e = 1, lerped by e² (0x690A80); smoothed with ω +424.
    pub look_lo: f32,
    pub look_hi: f32,
    pub look_omega: f32,
    /// +848 / +852: the lazy follow starts when the eye is within 0.5 rad of behind, stops beyond 1.0 (0xC5DDE0 /
    /// 0xC5DE20; hysteresis in 0x692D90).
    pub follow_enter: f32,
    pub follow_exit: f32,
    /// +568 / +572 / +580: the follow's angle band and gain (0x54E140).
    pub follow_lo: f32,
    pub follow_hi: f32,
    pub follow_gain: f32,
    /// +576: the follow waits this long after manual input (timer this+2624, 0x691F00).
    pub input_hold: f32,
    /// +436 / +444 / +448: blocked ≥ 0.75 → e ≥ 0.5 and input that makes it worse is undone; ≥ 0.77 → the eye stays
    /// ≥ 1.0 m above the target (0x68FD50 / 0x68FDA0 / 0x690160).
    pub blocked_hold: f32,
    pub raise_min: f32,
    pub e_min_blocked: f32,
    /// +464: the collision distance eases back out with ω 7.5 (+605 = 1).
    pub ease_out_omega: f32,
    /// +476 / +472: blocked ≥ 0.7 while moving and not following → swing toward behind, ω 7.5 (+606 = 1).
    pub swing_blocked: f32,
    pub swing_omega: f32,
    /// +656: climb mode target offset (heading frame, game axes: x right, y forward): 0.3 m off the wall.
    pub climb_target_back: f32,
    /// +688 / +704: the wall-above probe (2.5 m up, 1 m forward) (0xC5EA90).
    pub climb_probe_up: f32,
    pub climb_probe_fwd: f32,
    /// +760 / +764: e while the movement stick is up / down past 0.75 on a wall (0xC5E370).
    pub climb_e_up: f32,
    pub climb_e_down: f32,
    /// +772 / +776: yaw lead off behind while the stick is left / right past 0.75.
    pub climb_lead: [f32; 2],
    /// +792 / +788: at the top of a wall, e → 0.6 and yaw → behind (0xC5EE20).
    pub climb_top_e: f32,
    /// +796 / +800: after climbing, e recentres into 0.6 ± 0.1 (0xC5E370, flag 2796).
    pub recentre_e: f32,
    pub recentre_band: f32,
    /// +804: the CameraReset yaw recentre stops within 5° of behind (flag 2797).
    pub recentre_yaw_stop: f32,
    /// +808 / +672 / +624: climb collision probe offsets (0xC5F080).
    pub climb_probe_down: f32,
    pub climb_probe_back: [f32; 2],
    pub climb_probe_rise: f32,
    /// +812 / +816: a long fall looks down when its drop passes 1.2 m with no floor 1.25 m below (0xC5E980).
    pub fall_probe: f32,
    pub fall_min: f32,
    /// +744: the look-at drops this far in a long fall (0xC5EC50, ω 5).
    pub fall_drop: f32,
    /// +820 / +836: climb mode's blocked thresholds for the e floor and the raise (0xC5DBD0 / 0xC5DC30).
    pub climb_blocked_hold: f32,
    pub climb_raise: f32,
    /// +824: after climbing, the lazy follow resumes after 1 s (0xC5DD60).
    pub climb_follow_delay: f32,
}

pub const S: Settings = Settings {
    target_height: 1.6,
    target_lag_xy: 0.1,
    target_lag_z: 0.3,
    eye_lag: 0.125,
    fov: 0.8,
    pitch_lo: -0.698_132,
    pitch_hi: 1.396_263,
    pitch_exp: 1.45,
    dist_ground: [3.5, 3.5],
    dist_climb: [4.25, 3.75],
    e_default: 0.535,
    look_lo: -0.3,
    look_hi: 0.0,
    look_omega: 5.0,
    follow_enter: 0.5,
    follow_exit: 1.0,
    follow_lo: 0.0,
    follow_hi: 0.523_599,
    follow_gain: 1.0,
    input_hold: 1.0,
    blocked_hold: 0.75,
    raise_min: 1.0,
    e_min_blocked: 0.5,
    ease_out_omega: 7.5,
    swing_blocked: 0.7,
    swing_omega: 7.5,
    climb_target_back: 0.3,
    climb_probe_up: 2.5,
    climb_probe_fwd: 1.0,
    climb_e_up: 0.35,
    climb_e_down: 0.7,
    climb_lead: [-0.5, 0.5],
    climb_top_e: 0.6,
    recentre_e: 0.6,
    recentre_band: 0.1,
    recentre_yaw_stop: 0.087_266,
    climb_probe_down: -0.5,
    climb_probe_back: [-0.8, -0.8],
    climb_probe_rise: 0.8,
    fall_probe: 1.25,
    fall_min: 1.2,
    fall_drop: -2.0,
    climb_blocked_hold: 0.15,
    climb_raise: 0.75,
    climb_follow_delay: 1.0,
};

/// The collision sphere: `CameraBase` creates a 0.2 m probe (0x5D5A40 → 0x4D8DD0) and places a hit eye at the contact
/// plus the normal times that radius (+268 → 0x4D8DC0), i.e. the sphere's centre at contact.
pub const SPHERE: f32 = 0.2;

/// The parts of a FreeRoamingCameraSettings block that differ between the player's cameras (Camera Switcher: the
/// LowHighProfile block at 0x1BE6, the Leap of Faith FreeRoaming block at 0xD67; layout 0x5A5DB0).
#[derive(Debug)]
pub struct Profile {
    /// CameraSettings +8 priority (the switcher shows the active camera with the highest) and +12 blend-in time (s).
    pub priority: i32,
    pub blend_in: f32,
    pub target_lag_xy: f32,
    pub target_lag_z: f32,
    pub eye_lag: f32,
    pub pitch: [f32; 3],
    pub dist: [f32; 2],
    pub e_default: f32,
    /// +556: how the yaw follows the target (0x692D90).
    pub follow: FollowMode,
    /// +272 z: the target's height above the feet.
    pub target_height: f32,
    /// +512 = 1 / 3: activation turns the yaw to the target heading plus +516 (FreeRoamingCamera__InitYaw 0x68F940);
    /// otherwise it comes from the previous camera (0x6910C0).
    pub init_yaw: Option<f32>,
    /// +600: the camera activated after this one copies its yaw (0x69135A) instead of measuring this one's eye.
    pub hands_yaw: bool,
    /// +576: the wait after manual input.
    pub input_hold: f32,
    /// +606 swing toward behind when blocked, +605 ease the distance back out, +444 the raised eye's minimum height.
    pub swing: bool,
    pub ease_out: bool,
    pub raise_min: f32,
    /// The LowHighProfile layer (climb mode, long fall, recentres, its settings tail).
    pub lowhigh: bool,
}

/// FreeRoamingCameraSettings +556 (0x692D90's switch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FollowMode {
    /// 0: the yaw follows whenever there is no input.
    Always,
    /// 2: no automatic follow.
    Off,
    /// 3: the lazy follow inside its enter / exit window (rad).
    Lazy(f32, f32),
}

pub const LOW_HIGH: Profile = Profile {
    priority: 10, blend_in: 1.0, target_lag_xy: 0.1, target_lag_z: 0.3, eye_lag: 0.125,
    pitch: [-0.698_132, 1.396_263, 1.45], dist: [3.5, 3.5], e_default: 0.535,
    follow: FollowMode::Lazy(0.5, 1.0), input_hold: 1.0, swing: true, ease_out: true, raise_min: 1.0, lowhigh: true,
    target_height: 1.6, init_yaw: None, hands_yaw: false,
};

/// The Leap of Faith camera (FreeRoamingCameraSettings object 0xB97, priority 3500, activated by
/// LeapOfFaithEventMonitor = ActorState 41, sent at the faith jump's start by the interpreter 0xEDAB20 and ended by
/// InAir's cleanup / haystack entry): 3 m away, pitch fixed at 80° looking down, 0.05 s lags, starting behind the
/// heading (+512 = 1).
pub const LEAP_OF_FAITH: Profile = Profile {
    priority: 3500, blend_in: 1.0, target_lag_xy: 0.05, target_lag_z: 0.05, eye_lag: 0.05,
    pitch: [1.396_263, 1.396_263, 1.5], dist: [3.0, 3.0], e_default: 0.99,
    follow: FollowMode::Always, input_hold: 0.0, swing: false, ease_out: false, raise_min: 0.5, lowhigh: false,
    target_height: 1.65, init_yaw: Some(0.0), hands_yaw: false,
};

/// The StandOnLedge camera (FreeRoamingCameraSettings object 0x11FD, block 0x13CD; priority 18, blend-in 2 s),
/// activated by StandOnLedgeEventMonitor = ActorState 62: sent with +16 = 0 by `HumanGround__LookDown_Enter`
/// 0xD9FC80 and +16 = 1 when the look-down ends (0xD98CC0). Distance 3 → 1.8 m, pitch −8°..84°, e 0.865 (66° down,
/// 2 m away), no lags, no follow, the yaw kept from the previous camera and handed on to the next.
pub const STAND_ON_LEDGE: Profile = Profile {
    priority: 18, blend_in: 2.0, target_lag_xy: 0.0, target_lag_z: 0.0, eye_lag: 0.0,
    pitch: [-0.139_626, 1.466_077, 1.5], dist: [3.0, 1.8], e_default: 0.865,
    follow: FollowMode::Off, input_hold: 0.0, swing: false, ease_out: false, raise_min: 0.5, lowhigh: false,
    target_height: 1.7, init_yaw: None, hands_yaw: true,
};

/// PadR6VCameraControl (Camera Switcher 0x1A4B) and the mouse path of its update 0x5FAFC0.
pub struct R6v {
    /// +108: per-axis dead zone of the stick value (0x5F9C00).
    pub dead_zone: f32,
    /// +112: stick length that allows the full-deflection acceleration.
    pub full: f32,
    /// +116: below the minimum speed, the speed first jumps this fraction toward it.
    pub ramp: f32,
    /// +140: the mouse path's gain, times 0.033 (one 30 Hz frame): rad per count.
    pub mouse: f32,
    /// x: +164 min, +168 max (rad/s), +172 acceleration (rad/s²), +176 filter T, +180 axis edge; y: +144..+160.
    pub x: R6vAxis,
    pub y: R6vAxis,
}

pub struct R6vAxis {
    pub min: f32,
    pub max: f32,
    pub accel: f32,
    pub filter: f32,
    pub edge: f32,
}

pub const R6V: R6v = R6v {
    dead_zone: 0.3,
    full: 0.95,
    ramp: 0.5,
    mouse: 0.05 * 0.033,
    x: R6vAxis { min: 1.396_263, max: 4.363_323, accel: 3.054_326, filter: 0.09, edge: 0.65 },
    y: R6vAxis { min: 0.523_599, max: 0.785_398, accel: 0.523_599, filter: 0.09, edge: 0.65 },
};

/// 0x5FAFC0's pad path: dead zone, the axis filter, then the R6V speed per axis. Returns (Δyaw, Δe, active, hard).
fn r6v_step(r6v: &R6v, prev_v: &mut Vec2, speed_v: &mut Vec2, stick: Vec2, dt: f32) -> (f32, f32, bool, bool) {
    let raw = stick;
    // 0x5F9C00
    let dz = |v: f32| {
        let m = (v.abs() - r6v.dead_zone).max(0.0) / (1.0 - r6v.dead_zone);
        m.copysign(v) * (v != 0.0) as i32 as f32
    };
    let mut cur = Vec2::new(dz(raw.x), dz(raw.y));
    let active = cur.x.abs() > 0.0005 || cur.y.abs() > 0.0005;
    let len = raw.length();
    let hard_x = raw.x.abs() > r6v.x.edge && len >= r6v.full;
    let hard_y = raw.y.abs() > r6v.y.edge && len >= r6v.full;
    // 0x5F9CB0: a sign reversal resets the axis speed, else the value lags toward the stick
    for i in 0..2 {
        let (prev, t) = (prev_v[i], if i == 0 { r6v.x.filter } else { r6v.y.filter });
        if (prev > 0.0 && cur[i] < 0.0) || (prev < 0.0 && cur[i] > 0.0) {
            speed_v[i] = 0.0;
        } else if t > 0.0001 {
            cur[i] = (cur[i] - prev) * (dt / t).min(1.0) + prev;
        }
        if cur[i].abs() < 0.0001 { cur[i] = 0.0; }
        prev_v[i] = cur[i];
    }
    // 0x5FAD80 (SInterpParams type 1: linear)
    let axis = |speed: &mut f32, v: f32, a: &R6vAxis, hard: bool| {
        if hard {
            if *speed < a.max {
                if *speed < a.min {
                    *speed = if r6v.ramp <= 0.0 { a.min } else { (a.min - *speed) * r6v.ramp + *speed };
                }
                *speed = (*speed + a.accel * dt).min(a.max);
            }
        } else {
            let x = if len >= r6v.full && a.edge > 0.0 { v.abs() / a.edge } else { v.abs() };
            *speed = if x <= 0.0 { 0.0 } else if x > 1.0 { a.min } else { a.min * x };
        }
        *speed * dt
    };
    let mut sx = speed_v.x;
    let mut sy = speed_v.y;
    let mut dyaw = axis(&mut sx, cur.x, &r6v.x, hard_x);
    let mut de = axis(&mut sy, cur.y, &r6v.y, hard_y);
    *speed_v = Vec2::new(sx, sy);
    if cur.x < 0.0 { dyaw = -dyaw; }
    // stick up lowers the camera (e falls): it looks up
    if cur.y > 0.0 { de = -de; }
    (dyaw, de, active, hard_x || hard_y)
}

/// The lazy follow's ResponseCurve (FreeRoamingCamera ctor 0x693AE0 → `ResponseCurve__AddKey`; piecewise linear,
/// `ResponseCurve__Evaluate` 0x5631D0).
const FOLLOW_CURVE: [(f32, f32); 5] = [(0.0, 0.1), (0.25, 0.4), (0.5, 0.75), (0.75, 0.9), (1.0, 1.0)];

fn follow_curve(x: f32) -> f32 {
    let mut y = 0.0;
    for w in FOLLOW_CURVE.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x >= x0 && x <= x1 {
            y = y0 + (x - x0) * (y1 - y0) / (x1 - x0);
        }
    }
    y
}

// ---------------------------------------------------------------------------------------------------------------
// Math (the game's helpers)
// ---------------------------------------------------------------------------------------------------------------

/// sub_54DF60: wrap to [-π, π].
pub fn wrap(a: f32) -> f32 {
    let mut a = a % TAU;
    if a > PI { a -= TAU; }
    if a < -PI { a += TAU; }
    a
}

/// `Math__SmoothCD` 0x54D980: critically damped spring, snaps within `eps`, ω = 0 snaps.
pub fn smooth_cd(x: &mut f32, target: f32, vel: &mut f32, omega: f32, dt: f32) {
    if omega == 0.0 { *x = target; return; }
    let d = *x - target;
    if d.abs() < 0.001 { *x = target; *vel = 0.0; return; }
    let w = dt * omega;
    let k = 1.0 / (1.0 + w + 0.48 * w * w + 0.235 * w * w * w);
    let t = (d * omega + *vel) * dt;
    *vel = (*vel - t * omega) * k;
    *x = (t + d) * k + target;
}

/// sub_54E5D0: the angular `Math__SmoothCD` (difference and result wrapped).
pub fn smooth_cd_angle(x: &mut f32, target: f32, vel: &mut f32, omega: f32, dt: f32) {
    if omega == 0.0 { *x = target; return; }
    let d = wrap(*x - target);
    if d.abs() < 0.001 { *x = target; *vel = 0.0; return; }
    let w = dt * omega;
    let k = 1.0 / (1.0 + w + 0.48 * w * w + 0.235 * w * w * w);
    let t = (d * omega + *vel) * dt;
    *vel = (*vel - t * omega) * k;
    *x = wrap((t + d) * k + target);
}

/// sub_54DC50: `Math__SmoothCD` per component of a vector.
fn smooth_cd_vec(x: &mut Vec3, target: Vec3, vel: &mut Vec3, omega: f32, dt: f32) {
    for i in 0..3 {
        smooth_cd(&mut x[i], target[i], &mut vel[i], omega, dt);
    }
}

/// `Math__LagToward` 0x54D950 / `Math__LagTowardVec` 0x54DBF0.
fn lag(x: f32, raw: f32, t: f32, dt: f32) -> f32 {
    (x * t + raw * dt) / (t + dt)
}

/// sub_54DD90 in port axes: the eye offset for a yaw, pitch and distance.
pub fn orbit_offset(yaw: f32, pitch: f32, dist: f32) -> Vec3 {
    Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos()) * dist
}

/// sub_54DE60 in port axes: yaw, pitch and distance of an offset.
pub fn angles_of(v: Vec3) -> (f32, f32, f32) {
    let d = v.length();
    let pitch = if d > 0.0005 { (v.y / d).clamp(-1.0, 1.0).asin() } else { 0.0 };
    (v.x.atan2(-v.z), pitch, d)
}

/// The game's heading for a port facing direction: the yaw that puts the eye behind it (sub_5DD250).
pub fn game_heading(forward: Vec3) -> f32 {
    (-forward.x).atan2(forward.z)
}

/// sub_54E140: turn `cur` toward `target` by a gain from the curve of how far `target` is from `reference`
/// (beyond `lo`, over the band `hi − lo`), never past `reference`.
fn follow_step(cur: f32, target: f32, reference: f32, lo: f32, hi: f32, gain: f32) -> f32 {
    let band = hi - lo;
    if band <= 0.0 { return cur; }
    let m = (wrap(target - reference).abs() - lo).clamp(0.0, band);
    let f = follow_curve((m / band).clamp(0.0, 1.0));
    let mut limit = (reference - cur).abs() % TAU;
    if limit > PI { limit = TAU - limit; }
    let d = wrap(target - cur);
    let mut step = d * f * gain;
    if step.abs() > limit { step = limit.copysign(d); }
    wrap(cur + step)
}

// ---------------------------------------------------------------------------------------------------------------
// The camera
// ---------------------------------------------------------------------------------------------------------------

/// Collision queries the camera needs.
pub trait CamWorld {
    /// The camera's sphere cast (+248 0x5D5950, a Havok linear cast of the 0.2 m probe): the sphere centre at the
    /// first contact between `from` and `to`, or None.
    fn sphere_cast(&self, from: Vec3, to: Vec3) -> Option<Vec3>;
}

impl CamWorld for crate::collision::CollisionWorld {
    fn sphere_cast(&self, from: Vec3, to: Vec3) -> Option<Vec3> {
        let v = to - from;
        let len = v.length();
        if len < 1e-4 { return None; }
        let dir = v / len;
        let d = self.sphere_free_distance(from, dir, SPHERE, len);
        (d < len).then(|| from + dir * d)
    }
}

/// One frame of input to the camera.
#[derive(Clone, Copy, Default)]
pub struct CamInput {
    pub dt: f32,
    /// The tracked entity's root (feet) and facing (port).
    pub root: Vec3,
    pub forward: Vec3,
    /// The raw mouse counts this frame, and whether the mouse is driving (byte_1A1E1F9: set by mouse motion, cleared
    /// by the right-stick keys or a pad).
    pub mouse: Vec2,
    pub mouse_mode: bool,
    /// The right stick after the pad's dead zone (arrows or a pad).
    pub stick: Vec2,
    /// The movement stick (the climb mode reads it, 0xC5E370).
    pub move_stick: Vec2,
    /// Climb or Ladder (PresentationEvent 2 / 27), Beam (40).
    pub climbing: bool,
    pub beam: bool,
    /// The InAir fall (FallEvent 47 from 0xEDBAF0): Some(drop below the apex) while in the air.
    pub fall_drop: Option<f32>,
    /// ActorState 67.
    pub reset: bool,
}

/// What a newly active camera takes from the one before it (0x6910C0).
#[derive(Clone, Copy, Debug)]
pub enum Handover {
    /// The previous camera's yaw (+600, or a first-person camera).
    Yaw(f32),
    /// The previous camera's eye.
    Eye(Vec3),
}

/// The camera's output for this frame (port space).
#[derive(Clone, Copy, Default, Debug)]
pub struct CamView {
    pub eye: Vec3,
    pub look_at: Vec3,
}

#[derive(Clone, Debug)]
pub struct NativeCamera {
    pub active: bool,
    pub profile: &'static Profile,
    /// Smoothed / raw follow point (this+192 / +208).
    pub target: Vec3,
    pub target_raw: Vec3,
    /// Raw / smoothed eye (this+48 / +32).
    pub eye_raw: Vec3,
    pub eye: Vec3,
    /// Yaw (this+272), elevation parameter (this+2052), and the distance / pitch it gives (this+284 / +260).
    pub yaw: f32,
    pub e: f32,
    pub dist: f32,
    pub pitch: f32,
    /// The lerped distance pair (this+2704 / +2712).
    pub dist_pair: [f32; 2],
    /// Heading (this+2040) and the yaw / e at the frame's start (this+2032 / +2036).
    pub heading: f32,
    prev_yaw: f32,
    prev_e: f32,
    /// The 20-frame average of the root's displacement (TargetTracker +64, 0x5D8DF0).
    displacement: [Vec3; 20],
    displacement_n: usize,
    displacement_i: usize,
    last_root: Option<Vec3>,
    /// Lazy follow on (this+1940).
    follow: bool,
    /// Seconds until the follow may run after manual input (this+2624).
    input_hold: f32,
    /// Manual input changed the yaw / e this frame (this+2289 / +2290); the stick or mouse is active (+2304); the R6V
    /// full-deflection flag (+2305).
    input_yaw: bool,
    input_e: bool,
    input_active: bool,
    input_hard: bool,
    /// R6V state: filtered values and speeds per axis (this[7], [8], [5], [6]).
    r6v_prev: Vec2,
    r6v_speed: Vec2,
    /// Blocked fraction (this+1936), the collision distance (this+2056) and its velocity.
    pub blocked: f32,
    coll_dist: f32,
    coll_vel: f32,
    swing_vel: f32,
    /// Look offset (this+2224) and the climb / fall target offset (this+2256), with velocities.
    look: Vec3,
    look_vel: Vec3,
    offset: Vec3,
    offset_vel: Vec3,
    /// LowHigh flags: climbing (2792), long fall (2793), wall above (2794), climb top recentre (2795), recentre e /
    /// yaw (2796 / 2797), stick lead off (2800), beam (2799); the reset timer (2720) and the follow delay after
    /// climbing (2768).
    climb: bool,
    long_fall: bool,
    wall_above: bool,
    climb_top: bool,
    recentre_e: bool,
    recentre_yaw: bool,
    lead_off: bool,
    beam: bool,
    reset_timer: Option<f32>,
    climb_follow_hold: f32,
    lead_vel: f32,
    climb_e_vel: f32,
    climb_yaw_vel: f32,
    recentre_e_vel: f32,
    recentre_yaw_vel: f32,
}

impl Default for NativeCamera {
    fn default() -> Self {
        Self {
            active: false,
            profile: &LOW_HIGH,
            target: Vec3::ZERO,
            target_raw: Vec3::ZERO,
            eye_raw: Vec3::ZERO,
            eye: Vec3::ZERO,
            yaw: 0.0,
            e: S.e_default,
            dist: S.dist_ground[0],
            pitch: 0.0,
            dist_pair: S.dist_ground,
            heading: 0.0,
            prev_yaw: 0.0,
            prev_e: S.e_default,
            displacement: [Vec3::ZERO; 20],
            displacement_n: 0,
            displacement_i: 0,
            last_root: None,
            follow: false,
            input_hold: 0.0,
            input_yaw: false,
            input_e: false,
            input_active: false,
            input_hard: false,
            r6v_prev: Vec2::ZERO,
            r6v_speed: Vec2::ZERO,
            blocked: 0.0,
            coll_dist: 0.0,
            coll_vel: 0.0,
            swing_vel: 0.0,
            look: Vec3::ZERO,
            look_vel: Vec3::ZERO,
            offset: Vec3::ZERO,
            offset_vel: Vec3::ZERO,
            climb: false,
            long_fall: false,
            wall_above: false,
            climb_top: false,
            recentre_e: false,
            recentre_yaw: false,
            lead_off: false,
            beam: false,
            reset_timer: None,
            climb_follow_hold: 0.0,
            lead_vel: 0.0,
            climb_e_vel: 0.0,
            climb_yaw_vel: 0.0,
            recentre_e_vel: 0.0,
            recentre_yaw_vel: 0.0,
        }
    }
}

/// Local offset in the heading frame (game axes: x right, y forward, z up) to port space (sub_54EB90).
fn heading_frame(forward: Vec3, local: Vec3) -> Vec3 {
    let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z);
    let r = crate::player::right_of(f);
    r * local.x + f * local.y + Vec3::Y * local.z
}

impl NativeCamera {
    /// 0xC5E200: the distance and pitch for an elevation parameter.
    pub fn dist_pitch(&self, e: f32) -> (f32, f32) {
        let e = e.clamp(0.0, 1.0);
        let d = (self.dist_pair[1] - self.dist_pair[0]) * e + self.dist_pair[0];
        let [lo, hi, exp] = self.profile.pitch;
        let p = lo + (hi - lo) * e.powf(exp);
        (d, p)
    }

    /// Inverse of the pitch curve (port helper for external camera placements).
    pub fn e_for_pitch(pitch: f32) -> f32 {
        ((pitch - S.pitch_lo) / (S.pitch_hi - S.pitch_lo)).clamp(0.0, 1.0).powf(1.0 / S.pitch_exp)
    }

    /// `FreeRoamingCamera__Activate` 0x6910C0: snap the target, yaw behind the heading, e = +500, eye snapped.
    pub fn activate(&mut self, inp: &CamInput) {
        self.target_raw = inp.root + Vec3::Y * self.profile.target_height;
        self.target = self.target_raw;
        self.heading = game_heading(inp.forward);
        self.yaw = self.heading;
        self.e = self.profile.e_default;
        self.dist_pair = if inp.climbing && self.profile.lowhigh { S.dist_climb } else { self.profile.dist };
        let (d, p) = self.dist_pitch(self.e);
        self.dist = d;
        self.pitch = p;
        self.eye_raw = self.target + orbit_offset(self.yaw, self.pitch, self.dist);
        self.eye = self.eye_raw;
        self.coll_dist = self.dist;
        self.look = Vec3::Y * (S.look_lo + (S.look_hi - S.look_lo) * self.e * self.e);
        self.active = true;
        self.last_root = Some(inp.root);
    }

    /// `FreeRoamingCamera__Activate` 0x6910C0 after another camera: e = +500 (InitElevation 0x68F8E0, +496 = 1 on
    /// every block); the yaw from InitYaw (+512), else the previous camera's yaw when it hands it on (+600) or a
    /// first-person camera, else the previous eye's azimuth round this camera's target (0x691297).
    pub fn activate_after(&mut self, inp: &CamInput, prev: Handover) {
        self.activate(inp);
        self.yaw = match (self.profile.init_yaw, prev) {
            (Some(off), _) => wrap(self.heading + off),
            (None, Handover::Yaw(y)) => y,
            (None, Handover::Eye(eye)) => angles_of(eye - self.target).0,
        };
        self.eye_raw = self.target + orbit_offset(self.yaw, self.pitch, self.dist);
        self.eye = self.eye_raw;
    }

    fn move_heading(&self) -> f32 {
        // +344 = 0x691890: the average displacement's direction when any axis passes 0.001, else the facing
        let n = self.displacement_n.max(1) as f32;
        let v = self.displacement[..self.displacement_n].iter().copied().sum::<Vec3>() / n;
        if v.x.abs() > 0.001 || v.z.abs() > 0.001 {
            game_heading(Vec3::new(v.x, 0.0, v.z).normalize())
        } else {
            self.heading
        }
    }

    /// The eye for a yaw / distance / pitch, with the collision (+252: 0xC5F080 climbing, else 0x692930).
    /// Returns the eye and whether it was blocked.
    fn build_eye(&self, world: &dyn CamWorld, forward: Vec3, yaw: f32, dist: f32, pitch: f32) -> (Vec3, bool) {
        let free = self.target + orbit_offset(yaw, pitch, dist);
        let dir = (free - self.target).normalize_or_zero();
        let (a, b) = if self.climb {
            // two probes from 0.8 m back off the wall: 1.3 m below the target and level with it
            let back = heading_frame(forward, Vec3::new(0.0, S.climb_probe_back[0], S.climb_probe_back[1]));
            let base = self.target + back;
            (base + Vec3::Y * S.climb_probe_down, base + heading_frame(forward, Vec3::new(0.0, 0.0, S.climb_probe_rise)))
        } else {
            // from 0.2 m out along the eye direction, and from 1 m below the target (+336 = (0, 0, −1))
            (self.target + dir * 0.2, self.target - Vec3::Y)
        };
        match world.sphere_cast(a, free) {
            Some(hit) if world.sphere_cast(b, free).is_some() => (hit, true),
            _ => (free, false),
        }
    }

    /// +388 = 0x691940: 1 − horizontal distance of the collided eye / of the free eye.
    fn blocked_fraction(&self, eye: Vec3) -> f32 {
        let free = self.target + orbit_offset(self.yaw, self.pitch, self.dist);
        let h = |v: Vec3| Vec2::new(v.x, v.z).length();
        let full = h(free - self.target);
        if full <= 1e-5 { return 0.0; }
        1.0 - h(eye - self.target) / full
    }

    /// One camera frame (`LowHighProfileCamera` +108 = 0xC5ED70).
    pub fn update(&mut self, inp: &CamInput, world: &dyn CamWorld) -> CamView {
        let dt = inp.dt.max(1e-5);
        if !self.active {
            self.activate(inp);
        }
        self.events(inp, world);
        // 0xC5E040: the distance pair lags (T 0.3) toward the mode's values
        let want = if self.climb { S.dist_climb } else { self.profile.dist };
        for i in 0..2 {
            self.dist_pair[i] = lag(self.dist_pair[i], want[i], 0.3, dt);
        }
        if let Some(t) = &mut self.reset_timer {
            *t -= dt;
            if *t <= 0.0 {
                self.reset_timer = None;
                self.recentre_e = true;
                self.recentre_e_vel = 0.0;
                self.recentre_yaw = true;
                self.recentre_yaw_vel = 0.0;
            }
        }
        self.climb_follow_hold = (self.climb_follow_hold - dt).max(0.0);
        self.input_hold = (self.input_hold - dt).max(0.0);
        // the tracker's displacement history
        if let Some(last) = self.last_root {
            self.displacement[self.displacement_i] = inp.root - last;
            self.displacement_i = (self.displacement_i + 1) % 20;
            self.displacement_n = (self.displacement_n + 1).min(20);
        }
        let speed = self.last_root.map_or(0.0, |l| (inp.root - l).length());
        self.last_root = Some(inp.root);

        // FreeRoamingCamera__Update 0x68ED60
        self.prev_yaw = self.yaw;
        self.prev_e = self.e;
        self.update_target(inp, dt);
        self.climb_top_recentre(inp, world, dt);
        self.climb_lead_and_recentre(inp, dt);
        self.orbit_input(inp, dt);
        self.raw_eye(inp, world, speed, dt);
        // +292 = 0x690610: the eye lag, none while the mouse drives
        self.eye = if inp.mouse_mode {
            self.eye_raw
        } else {
            (self.eye * self.profile.eye_lag + self.eye_raw * dt) / (self.profile.eye_lag + dt)
        };
        // +296 = 0x690C30: the look offset (rotated by π − heading, which leaves the vertical one unchanged)
        let look = Vec3::Y * (S.look_lo + (S.look_hi - S.look_lo) * self.e * self.e);
        smooth_cd_vec(&mut self.look, look, &mut self.look_vel, S.look_omega, dt);
        // 0xC5EC50: the long-fall / climb target offset
        let (want, omega) = if self.long_fall {
            (Vec3::Y * S.fall_drop, 5.0)
        } else if self.climb {
            (heading_frame(inp.forward, Vec3::new(0.0, -S.climb_target_back, 0.0)), 10.0)
        } else {
            (Vec3::ZERO, 10.0)
        };
        smooth_cd_vec(&mut self.offset, want, &mut self.offset_vel, omega, dt);
        // +128 = 0x690A40: the look-at point
        CamView { eye: self.eye, look_at: self.target + self.look + self.offset }
    }

    /// PresentationEvent / FallEvent handlers (0xC5E820 / 0xC5E980) from this frame's state.
    fn events(&mut self, inp: &CamInput, world: &dyn CamWorld) {
        if !self.profile.lowhigh {
            return;
        }
        if inp.climbing && !self.climb {
            self.climb = true;
            self.recentre_e = false;
        } else if !inp.climbing && self.climb {
            self.climb = false;
            self.recentre_e = true;
            self.recentre_e_vel = 0.0;
            self.climb_follow_hold = S.climb_follow_delay;
        }
        self.beam = inp.beam;
        match inp.fall_drop {
            Some(drop) if drop > S.fall_min && !self.long_fall => {
                // a ray down from the target: a floor within 1.25 m cancels the look-down
                let floor = world.sphere_cast(self.target, self.target - Vec3::Y * S.fall_probe).is_some();
                self.long_fall = !floor;
            }
            None => self.long_fall = false,
            _ => {}
        }
        if inp.reset && self.reset_timer.is_none() {
            self.reset_timer = Some(0.18);
        }
    }

    /// `FreeRoamingCamera__UpdateTarget` 0x692430 (lag mode, +612 = 0).
    fn update_target(&mut self, inp: &CamInput, dt: f32) {
        self.target_raw = inp.root + Vec3::Y * self.profile.target_height;
        if self.target_raw.distance(self.target) > 5.0 {
            // PORT: a respawn / map switch snaps like an activation
            self.activate(inp);
            return;
        }
        let pr = self.profile;
        self.target.x = lag(self.target.x, self.target_raw.x, pr.target_lag_xy, dt);
        self.target.z = lag(self.target.z, self.target_raw.z, pr.target_lag_xy, dt);
        self.target.y = lag(self.target.y, self.target_raw.y, pr.target_lag_z, dt);
    }

    /// 0xC5EE20 (+308): climbing, the wall-above probe; reaching the top of the wall recentres e → 0.6 and the yaw.
    fn climb_top_recentre(&mut self, inp: &CamInput, world: &dyn CamWorld, dt: f32) {
        if !self.climb {
            self.wall_above = false;
            self.climb_top = false;
            self.climb_e_vel = 0.0;
            self.climb_yaw_vel = 0.0;
            return;
        }
        let a = self.target + heading_frame(inp.forward, Vec3::new(0.0, 0.0, S.climb_probe_up));
        let b = self.target + heading_frame(inp.forward, Vec3::new(0.0, S.climb_probe_fwd, S.climb_probe_up));
        self.wall_above = world.sphere_cast(a, b).is_some();
        if self.wall_above {
            self.climb_top = true;
        }
        if self.input_yaw || self.input_e {
            self.climb_top = false;
        }
        if !self.wall_above && self.climb_top {
            // the wall is still there just below the probe: wait
            let back = heading_frame(inp.forward, Vec3::new(0.0, S.climb_probe_back[0], S.climb_probe_back[1]));
            if world.sphere_cast(self.target + back + Vec3::Y * S.climb_probe_down, b).is_some() {
                self.climb_top = false;
            }
            if self.climb_top {
                smooth_cd(&mut self.e, S.climb_top_e, &mut self.climb_e_vel, 10.0, dt);
                smooth_cd_angle(&mut self.yaw, self.heading, &mut self.climb_yaw_vel, 10.0, dt);
            }
        }
    }

    /// 0xC5E370 (+60): the climb's stick lead and the recentres, then the orbit input (0x691F00).
    fn climb_lead_and_recentre(&mut self, inp: &CamInput, dt: f32) {
        if !self.climb || !self.wall_above {
            self.lead_off = false;
        } else if self.input_hard {
            self.lead_off = true;
        }
        if !inp.mouse_mode && !self.lead_off && self.wall_above && self.climb && !self.input_yaw && !self.input_e {
            let s = inp.move_stick;
            if s.y > 0.75 {
                smooth_cd(&mut self.e, S.climb_e_up, &mut self.lead_vel, 10.0, dt);
            } else if s.y < -0.75 {
                smooth_cd(&mut self.e, S.climb_e_down, &mut self.lead_vel, 10.0, dt);
            } else {
                self.lead_vel = 0.0;
            }
            if s.x.abs() > 0.75 {
                let lead = if s.x >= 0.0 { S.climb_lead[1] } else { S.climb_lead[0] };
                smooth_cd_angle(&mut self.yaw, self.heading + lead, &mut self.climb_yaw_vel, 10.0, dt);
            }
        }
        if self.recentre_e {
            let (lo, hi) = (S.recentre_e - S.recentre_band, S.recentre_e + S.recentre_band);
            if self.input_active || (self.e >= lo && self.e <= hi) {
                self.recentre_e = false;
            } else {
                smooth_cd(&mut self.e, S.recentre_e, &mut self.recentre_e_vel, 10.0, dt);
            }
        }
        if self.recentre_yaw {
            if self.input_active || wrap(self.heading - self.yaw).abs() < S.recentre_yaw_stop {
                self.recentre_yaw = false;
            } else {
                smooth_cd_angle(&mut self.yaw, self.heading, &mut self.recentre_yaw_vel, 10.0, dt);
            }
        }
    }

    /// 0x691F00 with PadR6VCameraControl +36 = 0x5FAFC0: manual yaw / e, the e floor and the input flags.
    fn orbit_input(&mut self, inp: &CamInput, dt: f32) {
        let (yaw0, e0) = (self.yaw, self.e);
        let (dyaw, de, active, hard) = if inp.mouse_mode {
            let d = inp.mouse * R6V.mouse;
            (d.x, d.y, d.x.abs() > 0.0005 || d.y.abs() > 0.0005, false)
        } else {
            self.r6v(inp.stick, dt)
        };
        self.input_active = active;
        self.input_hard = hard;
        self.yaw = wrap(self.yaw + dyaw);
        self.e += de;
        // the floor: e ≥ 0.5 while ≥ 75 % blocked on the ground (+280 / +348), else 0; never above 1
        let blocked_floor = if self.climb { self.blocked >= S.climb_blocked_hold } else { self.blocked >= S.blocked_hold };
        let floor = if blocked_floor && !self.climb { S.e_min_blocked } else { 0.0 };
        if floor > self.e { self.e = floor; }
        if self.e > 1.0 { self.e = 1.0; }
        self.input_yaw = self.yaw != yaw0;
        self.input_e = self.e != e0;
        if self.input_yaw || self.input_e {
            self.input_hold = self.profile.input_hold;
        }
    }

    /// 0x5FAFC0's pad path with this camera's control (`R6V`).
    fn r6v(&mut self, stick: Vec2, dt: f32) -> (f32, f32, bool, bool) {
        r6v_step(&R6V, &mut self.r6v_prev, &mut self.r6v_speed, stick, dt)
    }

    /// +332 = 0x692D90: heading, lazy follow, e → distance / pitch, the swing round obstacles and the collision
    /// distance (+408 = 0x690160).
    fn raw_eye(&mut self, inp: &CamInput, world: &dyn CamWorld, speed: f32, dt: f32) {
        self.heading = game_heading(inp.forward);
        // the lazy follow (+556 = 3): the yaw follows where the last raw eye sits around the raw target
        if !inp.mouse_mode && !self.climb {
            let (eye_az, _, _) = angles_of(self.eye_raw - self.target_raw);
            let off = wrap(eye_az - self.heading).abs();
            match self.profile.follow {
                FollowMode::Lazy(enter, exit) => {
                    if !self.follow {
                        if enter >= off { self.follow = true; }
                    } else if off > exit {
                        self.follow = false;
                    }
                }
                FollowMode::Always => self.follow = true,
                FollowMode::Off => self.follow = false,
            }
            let allowed = self.climb_follow_hold <= 0.0 && (!self.beam);
            if self.follow && allowed && !self.input_yaw && !self.input_e && !self.input_active && self.input_hold <= 0.0 {
                self.yaw = follow_step(self.yaw, eye_az, self.move_heading(), S.follow_lo, S.follow_hi, S.follow_gain);
            }
        }
        self.yaw = wrap(self.yaw);
        let (d, p) = self.dist_pitch(self.e);
        self.dist = d;
        self.pitch = p;
        // the swing round obstacles: moving, no input, the pad driving
        if speed > 0.0 && !self.input_yaw && !self.input_e && !self.input_active && !inp.mouse_mode {
            let probe = |yaw: f32| world.sphere_cast(self.target_raw + orbit_offset(yaw, self.pitch, self.dist), self.target_raw).is_some();
            let blocked_now = world.sphere_cast(self.eye_raw, self.target_raw).is_some() || probe(self.yaw);
            if blocked_now {
                let mut half = wrap(self.yaw - self.heading) * 0.5;
                let mut cand = wrap(self.yaw - half);
                for _ in 0..4 {
                    half *= 0.5;
                    if probe(cand) {
                        cand = wrap(cand - half);
                    } else {
                        self.yaw = cand;
                        cand = wrap(cand + half);
                    }
                }
            }
        }
        self.collision(inp, world, speed, dt);
    }

    /// +408 = 0x690160: the collided eye, input that pushes into walls undone, the raise and the distance easing.
    fn collision(&mut self, inp: &CamInput, world: &dyn CamWorld, speed: f32, dt: f32) {
        let prev_blocked = self.blocked;
        let (mut eye, hit) = self.build_eye(world, inp.forward, self.yaw, self.dist, self.pitch);
        self.blocked = if hit { self.blocked_fraction(eye) } else { 0.0 };
        let mut rebuild = false;
        // +606: swing toward behind when blocked ≥ 0.7 while moving and not following
        if self.profile.swing && self.blocked >= S.swing_blocked && !self.follow && speed > 0.0 {
            self.yaw = self.prev_yaw;
            smooth_cd_angle(&mut self.yaw, self.heading, &mut self.swing_vel, S.swing_omega, dt);
            rebuild = true;
        } else {
            let hold = if self.climb { S.climb_blocked_hold } else { S.blocked_hold };
            if self.blocked >= hold && self.blocked >= prev_blocked {
                // undo this frame's yaw (e is restored below)
                self.yaw = self.prev_yaw;
                rebuild = true;
            }
        }
        if rebuild {
            self.e = self.prev_e;
            let (d, p) = self.dist_pitch(self.e);
            self.dist = d;
            self.pitch = p;
            let (e2, hit2) = self.build_eye(world, inp.forward, self.yaw, self.dist, self.pitch);
            eye = e2;
            self.blocked = if hit2 { self.blocked_fraction(eye) } else { 0.0 };
        }
        // +284: blocked past 0.77 (climbing 0.75) keeps the eye at least 1 m above the target, then re-probes (+352)
        let raise = if self.climb { self.blocked >= S.climb_raise } else { self.blocked >= S.blocked_hold + 0.02 };
        if raise {
            eye.y = eye.y.max(self.target.y + self.profile.raise_min);
            let from = if self.climb || self.beam {
                let back = heading_frame(inp.forward, Vec3::new(0.0, S.climb_probe_back[0], S.climb_probe_back[1]));
                let base = self.target + back + Vec3::Y * S.climb_probe_down;
                base + (eye - self.target).normalize_or_zero() * 0.2
            } else {
                self.target + (eye - self.target).normalize_or_zero() * 0.2
            };
            if let Some(h) = world.sphere_cast(from, eye) {
                eye = h;
            }
        }
        let (az, pt, d) = angles_of(eye - self.target);
        if self.blocked <= prev_blocked && self.profile.ease_out {
            // opening up (+605): the distance eases back out with ω 7.5
            smooth_cd(&mut self.coll_dist, d, &mut self.coll_vel, S.ease_out_omega, dt);
            eye = self.target + orbit_offset(az, pt, self.coll_dist);
        } else if inp.mouse_mode {
            // closing in with the mouse driving: lag in with T 0.03
            self.coll_dist = lag(self.coll_dist, d, 0.03, dt);
            self.coll_vel = 0.0;
            eye = self.target + orbit_offset(az, pt, self.coll_dist);
        } else {
            self.coll_dist = d;
            self.coll_vel = 0.0;
        }
        self.eye_raw = eye;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// First-person camera
// ---------------------------------------------------------------------------------------------------------------

/// The first-person camera's PadR6VCameraControl (AssassinFirstPersonCameraSettings, Camera Switcher 0x262, its +28;
/// serializer 0x5FA620): slower than LowHigh's, filter 0.07 s.
pub const FIRST_PERSON_R6V: R6v = R6v {
    dead_zone: 0.3,
    full: 0.95,
    ramp: 0.5,
    mouse: 0.05 * 0.033,
    x: R6vAxis { min: 1.396_263, max: 2.356_194, accel: 1.745_329, filter: 0.07, edge: 0.65 },
    y: R6vAxis { min: 1.308_997, max: 1.954_769, accel: 1.308_997, filter: 0.07, edge: 0.65 },
};

/// AssassinFirstPersonCameraSettings (Camera Switcher 0x262; CameraSettings 0x6AEB50 / 0x5DCD90 / 0x5DB460,
/// FirstPersonCameraSettings 0x6961E0).
pub struct FirstPersonSettings {
    /// +8 priority (over LowHigh's 10, under the Leap of Faith's 3500) and +12 blend-in (s).
    pub priority: i32,
    pub blend_in: f32,
    /// +32: the view's field of view (rad).
    pub fov: f32,
    /// +224 / +228 pitch and +232 / +236 yaw limits (rad).
    pub pitch: [f32; 2],
    pub yaw: [f32; 2],
    /// +272 / +276: the yaw and pitch SmoothCDAngle rates toward the input (0x696E90).
    pub omega_yaw: f32,
    pub omega_pitch: f32,
}

pub const FIRST_PERSON: FirstPersonSettings = FirstPersonSettings {
    priority: 26,
    blend_in: 0.375,
    fov: 0.872_665,
    pitch: [-1.308_997, 1.308_997],
    yaw: [-PI, PI],
    omega_yaw: 50.0,
    omega_pitch: 50.0,
};

/// The head bone the first-person TargetHeadBoneMonitor (Camera Switcher 0x3C7DF4E9) tracks.
const FP_HEAD_BONE: u32 = 0x07C1_59A2;

/// FirstPersonCameraActivator (+68 of the settings; 0xCD3360): pad button 2 (Head), enter and exit after holding it
/// 0 s, stick flag +16 = 0 (moving the stick leaves).
const FP_BUTTON: usize = 2;

/// The first-person camera: the activator (0xCD3360 / 0xCD3070) and FirstPersonCamera__Update 0x696E90.
#[derive(Default, Clone, Debug)]
pub struct FirstPerson {
    pub active: bool,
    /// The activator may toggle again (+57): set once the button is released.
    armed: bool,
    /// Where the player stood when it started (+64).
    entry: Vec3,
    prev_buttons: [bool; 16],
    /// Yaw / pitch (game convention, as NativeCamera) and the input's targets, with velocities (+256..+288).
    pub yaw: f32,
    pub pitch: f32,
    yaw_target: f32,
    pitch_target: f32,
    yaw_vel: f32,
    pitch_vel: f32,
    r6v_prev: Vec2,
    r6v_speed: Vec2,
}

/// What the activator reads.
pub struct FpInput {
    pub buttons: [bool; 16],
    /// The movement stick after the pad's dead zone (the ControlScheme's stick 0, a4 +1472).
    pub move_stick: Vec2,
    pub root: Vec3,
    /// Standing on the ground (the activator's actor checks, 0xCD31F5.., leave on anything else).
    /// PORT (hypothesis): the AI interface tests (10, 18, 9) stand for "not in a normal ground state".
    pub on_ground: bool,
}

impl FirstPerson {
    /// 0xCD3070: leave on another button (pad 0, 1, 3, 14, 15 pressed, or 0 held), the movement stick past 0.0005,
    /// leaving the ground, or 0.1 m from where it started.
    fn should_exit(&self, inp: &FpInput) -> bool {
        let pressed = |i: usize| inp.buttons[i] && !self.prev_buttons[i];
        inp.buttons[0]
            || [1, 3, 14, 15].iter().any(|&i| pressed(i))
            || inp.move_stick.length() > 0.0005
            || !inp.on_ground
            || (self.active && inp.root.distance(self.entry) >= 0.1)
    }

    /// 0xCD3360. Returns true when it switched on or off this frame.
    pub fn activator(&mut self, inp: &FpInput) -> bool {
        let held = inp.buttons[FP_BUTTON];
        let was = self.active;
        if self.active && self.should_exit(inp) {
            self.active = false;
        } else if self.armed && held && !self.should_exit(inp) {
            // hold time 0 (+8 / +12): the press toggles it
            self.armed = false;
            if !self.active { self.entry = inp.root; }
            self.active = !self.active;
        }
        if !self.armed && !held {
            self.armed = true;
        }
        self.prev_buttons = inp.buttons;
        self.active != was
    }

    /// Starting: the view keeps the previous camera's heading.
    /// PORT (hypothesis): the yaw comes from the camera it replaces and the pitch starts level.
    pub fn activate(&mut self, yaw: f32) {
        self.yaw = yaw;
        self.yaw_target = yaw;
        self.pitch = 0.0;
        self.pitch_target = 0.0;
        self.yaw_vel = 0.0;
        self.pitch_vel = 0.0;
        self.r6v_prev = Vec2::ZERO;
        self.r6v_speed = Vec2::ZERO;
    }

    /// 0x696E90: the eye at the tracked head (TargetHeadBoneMonitor, offset +256 = 0), yaw and pitch eased toward the
    /// input with ω 50 (snapped in mouse mode).
    pub fn update(&mut self, inp: &CamInput, head: Vec3) -> CamView {
        let (dyaw, dpitch) = if inp.mouse_mode {
            let d = inp.mouse * FIRST_PERSON_R6V.mouse;
            (d.x, d.y)
        } else {
            let (dx, dy, _, _) = r6v_step(&FIRST_PERSON_R6V, &mut self.r6v_prev, &mut self.r6v_speed, inp.stick, inp.dt);
            (dx, dy)
        };
        let s = &FIRST_PERSON;
        self.yaw_target = wrap(self.yaw_target + dyaw).clamp(s.yaw[0], s.yaw[1]);
        self.pitch_target = (self.pitch_target + dpitch).clamp(s.pitch[0], s.pitch[1]);
        if inp.mouse_mode {
            self.yaw = self.yaw_target;
            self.pitch = self.pitch_target;
        } else {
            smooth_cd_angle(&mut self.yaw, self.yaw_target, &mut self.yaw_vel, s.omega_yaw, inp.dt);
            smooth_cd_angle(&mut self.pitch, self.pitch_target, &mut self.pitch_vel, s.omega_pitch, inp.dt);
        }
        CamView { eye: head, look_at: head - orbit_offset(self.yaw, self.pitch, 1.0) }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Bevy glue
// ---------------------------------------------------------------------------------------------------------------

/// The camera switcher's blend into a newly active camera (BlendCamera update 0x71A350): the eye goes from where the
/// view was toward the new camera's eye by w = curve(t / blend-in) (0x719A30). (hypothesis): the cosine curve
/// (type 1); BlendCameraSettings' curve type is not decoded. The view direction is turned with the same weight.
#[derive(Clone, Copy)]
struct SwitchBlend {
    from_eye: Vec3,
    from_rot: Quat,
    t: f32,
    duration: f32,
}

impl SwitchBlend {
    fn weight(&self) -> f32 {
        let p = (self.t / self.duration.max(1e-4)).clamp(0.0, 1.0);
        1.0 - ((p * PI).cos() + 1.0) * 0.5
    }
}

/// The cameras the switcher picks from, by priority: Leap of Faith 3500, first person 26, StandOnLedge 18, LowHigh 10.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Shown {
    #[default]
    LowHigh,
    StandOnLedge,
    FirstPerson,
    LeapOfFaith,
}

#[derive(Default)]
struct FollowState {
    cam: NativeCamera,
    /// The Leap of Faith and StandOnLedge cameras.
    lof: NativeCamera,
    ledge: NativeCamera,
    fp: FirstPerson,
    shown: Shown,
    blend: Option<SwitchBlend>,
    mouse_mode: bool,
    /// The rig values this system wrote last frame, to see placements by other systems (scenarios, replays).
    /// PORT: a placement's distance is ignored (the camera's e sets it).
    written: Option<(f32, f32, f32)>,
    shake: ShakeState,
}

#[allow(clippy::too_many_arguments)]
fn follow(
    collision: Res<crate::collision::CollisionWorld>,
    time: Res<Time>,
    menu: Res<crate::map_menu::MapMenu>,
    motion: Res<AccumulatedMouseMotion>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    gamepads: Query<&Gamepad>,
    pad: Option<Res<crate::input::PadInput>>,
    mut events: ResMut<CameraEvents>,
    mut rig: ResMut<CameraRig>,
    player: Query<(&Transform, Option<&crate::player::Body>, Option<&crate::player::Locomotion>, Option<&crate::player::HumanDataBundle>), (With<Player>, Without<MainCamera>)>,
    fly: Option<Res<crate::debug_fly::DebugFly>>,
    mut cam: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    rigs: Query<&crate::model::Rig, With<Player>>,
    joints: Query<&GlobalTransform>,
    mut state: Local<FollowState>,
) {
    use crate::player::ActorContextId as C;
    let (Ok((p, body, loco, data)), Ok((mut c, mut projection))) = (player.single(), cam.single_mut()) else { return };
    // Ghost mode's debug camera (`DebugCameraToggleEvent`; PORT: it holds still and keeps Altaïr in view)
    if fly.as_ref().is_some_and(|f| f.active && f.free_camera) {
        c.look_at(p.translation + Vec3::Y * 1.5, Vec3::Y);
        state.cam.active = false;
        return;
    }
    let st = &mut *state;
    // placements by other systems (scenario captures, replays, map switches) take over the yaw and e
    if let Some((wy, wp, wd)) = st.written {
        if rig.yaw != wy {
            st.cam.yaw = wrap(PI - rig.yaw);
            st.cam.follow = false;
        }
        if rig.pitch != wp {
            st.cam.e = NativeCamera::e_for_pitch(-rig.pitch);
        }
        let _ = wd;
    }
    let forward = body.map_or(p.forward().as_vec3(), |b| b.forward());
    let locked = cursor.single().map(|c| c.grab_mode != CursorGrabMode::None).unwrap_or(false);
    let mouse = if locked && !menu.open { motion.delta } else { Vec2::ZERO };
    let stick = if menu.open { Vec2::ZERO } else { pad.as_ref().map_or(Vec2::ZERO, |p| p.cam_stick) };
    // byte_1A1E1F9 (0x90A780): the mouse takes over when it moves, the stick keys or a pad's stick hand it back
    let pad_stick = gamepads.iter().any(|g| g.right_stick().length() > crate::tuning::STICK_DEADZONE);
    if mouse != Vec2::ZERO {
        st.mouse_mode = true;
    }
    if stick != Vec2::ZERO || pad_stick {
        st.mouse_mode = false;
    }
    // Ghost mode: the right stick's Y is the height unless LB holds it (0xECDC50)
    let ghost_height = fly.as_ref().is_some_and(|f| f.active && !f.hold_height);
    // the climb lead reads the pad's movement stick (0xC5E370 → sub_587B90(0))
    let move_stick = pad.as_ref().map_or(Vec2::ZERO, |p| p.stick);
    let ctx = loco.map(|l| l.current);
    // ActorState 41 LeapOfFaith: the faith jump at a haystack 3 m or more below (0xEDAB20) until InAir ends
    let leap = matches!(ctx, Some(C::InAir)) && data.is_some_and(|d| d.air.target.as_ref().is_some_and(|t|
        t.type_flags == crate::player::jump_blend::TARGET_HAYSTACK && t.position.y - d.air.start.y <= -crate::player::jump_blend::FAITH_MIN_DROP));
    let fall_drop = match (ctx, data) {
        (Some(C::InAir), Some(d)) if d.air.apex_reached => Some((d.air.apex_y - body.map_or(p.translation.y, |b| b.feet.y)).max(0.0)),
        (Some(C::InAir), Some(_)) => Some(0.0),
        _ => None,
    };
    let inp = CamInput {
        dt: time.delta_secs(),
        root: p.translation,
        forward,
        mouse,
        mouse_mode: st.mouse_mode,
        stick: if ghost_height { Vec2::new(stick.x, 0.0) } else { stick },
        move_stick,
        climbing: matches!(ctx, Some(C::Climb | C::Ladder)),
        beam: matches!(ctx, Some(C::NarrowObject)),
        fall_drop,
        reset: std::mem::take(&mut events.reset),
    };
    // the first-person activator (0xCD3360): Head toggles it while standing still
    let buttons = if menu.open { [false; 16] } else { pad.as_ref().map_or([false; 16], |p| p.buttons) };
    st.fp.activator(&FpInput { buttons, move_stick, root: p.translation, on_ground: matches!(ctx, Some(C::Ground)) });
    // ActorState 62 (StandOnLedge) while the look-down at an edge runs
    let look_down = matches!(ctx, Some(C::Ground)) && data.is_some_and(|d| d.ground.look_down.is_some());
    // the switcher: the active camera with the highest priority, activated after the one shown before (0x6910C0)
    // and blended in over its blend-in time
    let want = if leap { Shown::LeapOfFaith } else if st.fp.active { Shown::FirstPerson } else if look_down { Shown::StandOnLedge } else { Shown::LowHigh };
    if want != st.shown {
        let prev = match st.shown {
            Shown::FirstPerson => Handover::Yaw(st.fp.yaw),
            s => {
                let c = match s { Shown::LeapOfFaith => &st.lof, Shown::StandOnLedge => &st.ledge, _ => &st.cam };
                if c.profile.hands_yaw { Handover::Yaw(c.yaw) } else { Handover::Eye(c.eye) }
            }
        };
        let blend_in = match want {
            Shown::FirstPerson => {
                let yaw = match prev { Handover::Yaw(y) => y, Handover::Eye(e) => angles_of(e - head_point(&rigs, &joints, p.translation)).0 };
                st.fp.activate(yaw);
                FIRST_PERSON.blend_in
            }
            w => {
                let (cam, profile) = match w {
                    Shown::LeapOfFaith => (&mut st.lof, &LEAP_OF_FAITH),
                    Shown::StandOnLedge => (&mut st.ledge, &STAND_ON_LEDGE),
                    _ => (&mut st.cam, &LOW_HIGH),
                };
                cam.profile = profile;
                cam.activate_after(&inp, prev);
                profile.blend_in
            }
        };
        st.blend = Some(SwitchBlend { from_eye: c.translation, from_rot: c.rotation, t: 0.0, duration: blend_in });
        st.shown = want;
    }
    let head = head_point(&rigs, &joints, p.translation);
    let first_person = st.shown == Shown::FirstPerson;
    let view = match st.shown {
        Shown::LeapOfFaith => st.lof.update(&inp, &*collision),
        Shown::StandOnLedge => st.ledge.update(&inp, &*collision),
        Shown::FirstPerson => st.fp.update(&inp, head),
        Shown::LowHigh => st.cam.update(&inp, &*collision),
    };
    // the active camera's field of view (+32)
    if let Projection::Perspective(pp) = &mut *projection {
        let fov = if first_person { FIRST_PERSON.fov } else { S.fov };
        if pp.fov != fov { pp.fov = fov; }
    }
    // the landing camera shake (FX_Camera_Shake_Assassins_Fall)
    if rig.shake > 0.0 {
        st.shake.start(rig.shake);
        rig.shake = 0.0;
    }
    let (shake_pos, shake_rot) = st.shake.step(time.delta_secs());
    c.translation = view.eye;
    // the view (0x694080): yaw and pitch of the look-at point (+128) seen from the smoothed eye
    c.look_at(view.look_at, Vec3::Y);
    if let Some(b) = st.blend.as_mut() {
        b.t += time.delta_secs();
        let w = b.weight();
        c.translation = b.from_eye.lerp(view.eye, w);
        c.rotation = b.from_rot.slerp(c.rotation, w);
        if b.t >= b.duration { st.blend = None; }
    }
    let r = c.rotation;
    c.translation += r * shake_pos;
    c.rotation *= shake_rot;
    let shown = match st.shown { Shown::LeapOfFaith => &st.lof, Shown::StandOnLedge => &st.ledge, _ => &st.cam };
    let (yaw, pitch) = if first_person { (st.fp.yaw, st.fp.pitch) } else { (shown.yaw, shown.pitch) };
    rig.yaw = wrap(PI - yaw);
    rig.pitch = -pitch;
    rig.distance = if first_person { 0.0 } else { shown.dist };
    st.written = Some((rig.yaw, rig.pitch, rig.distance));
}

/// The tracked head for the first-person camera: the head bone of the last propagated pose (PORT: 1.6 m above the
/// feet without a skeleton).
fn head_point(rigs: &Query<&crate::model::Rig, With<Player>>, joints: &Query<&GlobalTransform>, root: Vec3) -> Vec3 {
    rigs.single().ok()
        .and_then(|r| r.bone_ids.iter().position(|&b| b == FP_HEAD_BONE).and_then(|i| joints.get(r.joints[i]).ok()))
        .map_or(root + Vec3::Y * S.target_height, |g| g.translation())
}

/// CameraSettings +24 = 0.45 (0x4C0B80): characters whose bounds meet a 0.45 m box round the eye are hidden when the eye
/// is inside one of their body capsules (0x4BE440): pelvis–head 0.4 m, each leg's hip–knee and knee–foot 0.3 m, the
/// forearms 0.2 m. The joints are read from the last propagated pose.
fn hide_near_camera(
    cam: Query<&Transform, With<MainCamera>>,
    mut players: Query<(&crate::model::Rig, &mut Visibility), With<Player>>,
    joints: Query<&GlobalTransform>,
) {
    const HEAD: u32 = 0x07C1_59A2;
    const SEGMENTS: [(u32, u32, f32); 6] = [
        (0x060D_F401, 0x5898_8870, 0.3), (0x863D_09FC, 0x9B14_362C, 0.3),
        (0xB898_B609, 0xB675_F36C, 0.2), (0x63D8_9144, 0x75F9_4D30, 0.2), (0, 0x060D_F401, 0.3), (0, 0x863D_09FC, 0.3),
    ];
    let Ok(eye) = cam.single().map(|t| t.translation) else { return };
    for (rig, mut vis) in &mut players {
        let at = |id: u32| -> Option<Vec3> {
            let i = if id == 0 { Some(0) } else { rig.bone_ids.iter().position(|&b| b == id) }?;
            joints.get(rig.joints[i]).ok().map(|g| g.translation())
        };
        let points: Vec<Vec3> = rig.joints.iter().filter_map(|&j| joints.get(j).ok().map(|g| g.translation())).collect();
        if points.is_empty() { continue; }
        let (min, max) = points.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), &p| (a.min(p), b.max(p)));
        let near_box = (eye - Vec3::splat(0.45)).cmple(max).all() && (eye + Vec3::splat(0.45)).cmpge(min).all();
        let within = |a: Vec3, b: Vec3, r: f32| {
            let ab = b - a;
            let t = ((eye - a).dot(ab) / ab.length_squared().max(1e-8)).clamp(0.0, 1.0);
            eye.distance(a + ab * t) <= r
        };
        let mut hide = false;
        if near_box {
            if let (Some(root), Some(head)) = (at(0), at(HEAD)) { hide |= within(root, head, 0.4); }
            for (a, b, r) in SEGMENTS {
                if let (Some(a), Some(b)) = (at(a), at(b)) { hide |= within(a, b, r); }
            }
        }
        let want = if hide { Visibility::Hidden } else { Visibility::Inherited };
        if *vis != want && !(*vis == Visibility::Visible && !hide) { *vis = want; }
    }
}

/// The landing shake `FX_Camera_Shake_Assassins_Fall` (HumanFXPack_AlTair +88, FX 0x17C9A6D2; RE/21 §3), started by
/// a > 3 m landing with its parameter 0xBE93B29C = min((drop − 3)/7, 1) (0xE05940). Its FXCameraOperator drives the
/// six CameraFXInterface channels; five have zero amplitude and
/// ShakePitch = k · env(t) · 30 · fbm(t, 20·(200 + 200·t)) over t ∈ [0, 1] s (tracks in 1/120 s, 0x597D20), with
/// env = 1.5 → 0.1 at 0.4 s → 0 at 1 s and a one-octave fBm (H 0.1, lacunarity 0.1) of Noise__Perlin2D.
/// The camera switcher adds the channels to the final view (0x4BF9D0, through its CameraFXInterface pointer +304):
/// angles in degrees × π/180, each scaled by the active camera's CameraSettings +44..+64 (all 1.0 for LowHighProfile).
pub const APPLY_FALL_SHAKE: bool = true;

#[derive(Default)]
pub struct ShakeState {
    intensity: f32,
    t: f32,
    noise: Option<crate::wind::Noise2Tables>,
}

impl ShakeState {
    pub fn start(&mut self, intensity: f32) {
        self.intensity = intensity.clamp(0.0, 1.0);
        self.t = 0.0;
    }

    /// The shake's pitch at FX time `t`, degrees.
    pub fn pitch(&mut self, t: f32) -> f32 {
        if !(0.0..=1.0).contains(&t) { return 0.0; }
        let env = if t < 0.4 { 1.5 + (0.1 - 1.5) * t / 0.4 } else { 0.1 * (1.0 - (t - 0.4) / 0.6) };
        let noise = self.noise.get_or_insert_with(crate::wind::Noise2Tables::new);
        self.intensity * env * 30.0 * noise.noise(t, 20.0 * (200.0 + 200.0 * t))
    }

    pub fn step(&mut self, dt: f32) -> (Vec3, Quat) {
        if self.intensity <= 0.0 { return (Vec3::ZERO, Quat::IDENTITY); }
        self.t += dt;
        if self.t > 1.0 { self.intensity = 0.0; return (Vec3::ZERO, Quat::IDENTITY); }
        let pitch = self.pitch(self.t);
        if !APPLY_FALL_SHAKE { return (Vec3::ZERO, Quat::IDENTITY); }
        // (hypothesis): a positive ShakePitch tilts the view up, about the camera's own right axis
        (Vec3::ZERO, Quat::from_rotation_x(pitch.to_radians()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Open;
    impl CamWorld for Open {
        fn sphere_cast(&self, _: Vec3, _: Vec3) -> Option<Vec3> { None }
    }

    /// A wall plane at z = `z` (port), facing −z.
    struct Wall(f32);
    impl CamWorld for Wall {
        fn sphere_cast(&self, from: Vec3, to: Vec3) -> Option<Vec3> {
            let stop = self.0 - SPHERE;
            if to.z <= stop || from.z >= stop { return if from.z >= stop && to.z > stop { Some(from) } else { None }; }
            let t = (stop - from.z) / (to.z - from.z);
            Some(from + (to - from) * t)
        }
    }

    fn input(dt: f32) -> CamInput {
        CamInput { dt, forward: Vec3::NEG_Z, ..default() }
    }

    #[test]
    fn camera_activation_puts_the_eye_behind_at_the_default_elevation() {
        let mut cam = NativeCamera::default();
        let v = cam.update(&input(1.0 / 60.0), &Open);
        // facing −Z: the game heading is π and the eye sits behind on +Z
        assert!((cam.yaw.abs() - PI).abs() < 1e-5);
        let (d, p) = cam.dist_pitch(S.e_default);
        assert!((d - 3.5).abs() < 1e-5);
        // e = 0.535: −40° + 120°·0.535^1.45
        assert!((p - (-0.698_132 + 2.094_395 * 0.535f32.powf(1.45))).abs() < 1e-4);
        assert!(v.eye.z > 0.0 && v.eye.y > S.target_height, "behind and above: {:?}", v.eye);
        assert!((v.eye - Vec3::Y * S.target_height).length() - 3.5 < 1e-3);
    }

    #[test]
    fn camera_pitch_curve_and_distance_follow_the_settings() {
        let cam = NativeCamera::default();
        assert!((cam.dist_pitch(0.0).1 - S.pitch_lo).abs() < 1e-6);
        assert!((cam.dist_pitch(1.0).1 - S.pitch_hi).abs() < 1e-6);
        let e = NativeCamera::e_for_pitch(0.3);
        assert!((cam.dist_pitch(e).1 - 0.3).abs() < 1e-4);
        let mut c = NativeCamera { dist_pair: S.dist_climb, ..default() };
        assert!((c.dist_pitch(0.0).0 - 4.25).abs() < 1e-6);
        assert!((c.dist_pitch(1.0).0 - 3.75).abs() < 1e-6);
        c.dist_pair = S.dist_ground;
        assert!((c.dist_pitch(0.4).0 - 3.5).abs() < 1e-6);
    }

    #[test]
    fn camera_mouse_turns_directly_and_the_stick_accelerates() {
        let mut cam = NativeCamera::default();
        cam.update(&input(1.0 / 60.0), &Open);
        let yaw0 = cam.yaw;
        let mut inp = input(1.0 / 60.0);
        inp.mouse_mode = true;
        inp.mouse = Vec2::new(100.0, 0.0);
        cam.update(&inp, &Open);
        // 100 counts · 0.05 · 0.033 rad, to the right (yaw grows)
        assert!((wrap(cam.yaw - yaw0) - 0.165).abs() < 1e-4, "{}", wrap(cam.yaw - yaw0));
        // mouse down raises the camera
        let e0 = cam.e;
        inp.mouse = Vec2::new(0.0, 50.0);
        cam.update(&inp, &Open);
        assert!(cam.e > e0);

        // full right stick: starts at 80°/s, accelerates by 175°/s² toward 250°/s
        let mut cam = NativeCamera::default();
        cam.update(&input(1.0 / 60.0), &Open);
        let mut inp = input(1.0 / 60.0);
        inp.stick = Vec2::new(1.0, 0.0);
        let mut last = 0.0;
        let mut rates = Vec::new();
        for _ in 0..240 {
            let y0 = cam.yaw;
            cam.update(&inp, &Open);
            let r = wrap(cam.yaw - y0) * 60.0;
            assert!(r >= last - 1e-3, "speed never drops while held");
            last = r;
            rates.push(r);
        }
        assert!(rates[0] > 0.5 && rates[0] < 1.4, "first frame from the minimum: {}", rates[0]);
        assert!((rates[239] - R6V.x.max).abs() < 0.01, "reaches the maximum: {}", rates[239]);
        // stick up looks up: e falls
        let e0 = cam.e;
        inp.stick = Vec2::new(0.0, 1.0);
        for _ in 0..10 { cam.update(&inp, &Open); }
        assert!(cam.e < e0);
    }

    #[test]
    fn camera_lazy_follow_swings_behind_a_sideways_run() {
        let mut cam = NativeCamera::default();
        let dt = 1.0 / 60.0;
        let mut inp = input(dt);
        cam.update(&inp, &Open);
        // run 20° off the camera's axis (inside the 0.5 rad follow window): the yaw comes round behind the run
        let a = 20f32.to_radians();
        inp.forward = Vec3::new(-a.sin(), 0.0, -a.cos());
        for i in 0..300 {
            inp.root = inp.forward * (i as f32 * 5.0 * dt);
            cam.update(&inp, &Open);
        }
        let off = wrap(cam.yaw - game_heading(inp.forward)).abs();
        assert!(off < 2f32.to_radians(), "behind the run: {off}");
        // 90° off: outside the window, the camera stays put
        let mut cam = NativeCamera::default();
        let mut inp = input(dt);
        cam.update(&inp, &Open);
        let yaw0 = cam.yaw;
        inp.forward = Vec3::X;
        for i in 0..300 {
            inp.root = Vec3::X * (i as f32 * 5.0 * dt);
            cam.update(&inp, &Open);
        }
        assert!(wrap(cam.yaw - yaw0).abs() < 0.05, "no follow beyond 1.0 rad: {}", wrap(cam.yaw - yaw0));
    }

    #[test]
    fn camera_collision_pulls_the_eye_in_front_of_a_wall_and_eases_back_out() {
        let mut cam = NativeCamera::default();
        let dt = 1.0 / 60.0;
        let inp = input(dt);
        cam.update(&inp, &Open);
        // a wall 1.5 m behind the target
        let wall = Wall(1.5);
        cam.update(&inp, &wall);
        assert!(cam.eye_raw.z <= 1.5 - SPHERE + 1e-3, "in front of the wall: {:?}", cam.eye_raw);
        assert!(cam.blocked > 0.3);
        let near = (cam.eye_raw - cam.target).length();
        for _ in 0..5 { cam.update(&inp, &Open); }
        let d2 = (cam.eye_raw - cam.target).length();
        assert!(d2 > near && d2 < 3.4, "eases back out (ω 7.5), not a snap: {near} -> {d2}");
    }

    #[test]
    fn camera_climb_mode_backs_off_and_a_long_fall_looks_down() {
        let mut cam = NativeCamera::default();
        let dt = 1.0 / 60.0;
        let mut inp = input(dt);
        cam.update(&inp, &Open);
        inp.climbing = true;
        for _ in 0..240 { cam.update(&inp, &Open); }
        assert!((cam.dist_pair[0] - 4.25).abs() < 0.01 && (cam.dist_pair[1] - 3.75).abs() < 0.01);
        // the target offset: 0.3 m back from the wall (facing −Z: back is +Z)
        assert!((cam.offset - Vec3::Z * 0.3).length() < 0.01, "{:?}", cam.offset);
        inp.climbing = false;
        inp.fall_drop = Some(2.0);
        let mut v = CamView::default();
        for _ in 0..240 { v = cam.update(&inp, &Open); }
        assert!((v.look_at.y - (cam.target.y + cam.look.y + S.fall_drop)).abs() < 0.02, "the look-at dropped 2 m");
        inp.fall_drop = None;
        for _ in 0..240 { cam.update(&inp, &Open); }
        assert!(cam.offset.length() < 0.01);
    }

    #[test]
    fn camera_leap_of_faith_profile_looks_down_from_three_metres() {
        let mut cam = NativeCamera { profile: &LEAP_OF_FAITH, ..default() };
        cam.activate(&input(1.0 / 60.0));
        for e in [0.0, 0.5, 0.99] {
            let (d, p) = cam.dist_pitch(e);
            assert!((d - 3.0).abs() < 1e-6 && (p - 1.396_263).abs() < 1e-5);
        }
        let v = cam.update(&input(1.0 / 60.0), &Open);
        assert!(v.eye.y - cam.target.y > 2.9, "80 degrees above the target: {:?}", v.eye - cam.target);
        // the switch blend's cosine weight
        let b = SwitchBlend { from_eye: Vec3::ZERO, from_rot: Quat::IDENTITY, t: 0.5, duration: 1.0 };
        assert!((b.weight() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn camera_fall_shake_follows_the_fx_envelope() {
        let mut s = ShakeState::default();
        s.start(1.0);
        // bounded by k · env · 30 · |noise| and gone after one second
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            let env = if t < 0.4 { 1.5 - 1.4 * t / 0.4 } else { 0.1 * (1.0 - (t - 0.4) / 0.6) };
            assert!(s.pitch(t).abs() <= env * 30.0 + 1e-3);
        }
        assert_eq!(s.pitch(1.5), 0.0);
        s.start(0.0);
        assert_eq!(s.step(0.016), (Vec3::ZERO, Quat::IDENTITY));
    }

    #[test]
    fn camera_follow_curve_matches_the_ctor_keys() {
        assert!((follow_curve(0.0) - 0.1).abs() < 1e-6);
        assert!((follow_curve(0.125) - 0.25).abs() < 1e-6);
        assert!((follow_curve(1.0) - 1.0).abs() < 1e-6);
        // never past the reference: cur 0, target 1, reference 0.2 → at most 0.2
        assert!((follow_step(0.0, 1.0, 0.2, 0.0, 0.5236, 1.0) - 0.2).abs() < 1e-5);
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
            .add_plugins(CameraPlugin)
            .add_systems(Update, move_visual.in_set(crate::ik::IkSet));
        app.world_mut().spawn((Player, Transform::default()));
        app.update();
        let world = app.world_mut();
        let eye = world.query_filtered::<&Transform, With<MainCamera>>().single(world).unwrap().translation;
        // activation snaps the target to this frame's visual root + 1.6 m
        let target = Vec3::Y * (0.3 + S.target_height);
        assert!(((eye - target).length() - 3.5).abs() < 1e-3, "follow must see this frame's IK-adjusted visual: {eye:?}");
    }

    fn fp_input(buttons: &[usize], stick: Vec2, root: Vec3) -> FpInput {
        let mut b = [false; 16];
        for &i in buttons { b[i] = true; }
        FpInput { buttons: b, move_stick: stick, root, on_ground: true }
    }

    #[test]
    fn head_toggles_first_person_and_moving_leaves_it() {
        let mut fp = FirstPerson::default();
        // armed once the button has been up
        assert!(!fp.activator(&fp_input(&[], Vec2::ZERO, Vec3::ZERO)));
        assert!(fp.activator(&fp_input(&[FP_BUTTON], Vec2::ZERO, Vec3::ZERO)) && fp.active, "press enters");
        assert!(!fp.activator(&fp_input(&[FP_BUTTON], Vec2::ZERO, Vec3::ZERO)), "held: no repeat");
        fp.activator(&fp_input(&[], Vec2::ZERO, Vec3::ZERO));
        assert!(fp.activator(&fp_input(&[FP_BUTTON], Vec2::ZERO, Vec3::ZERO)) && !fp.active, "a second press leaves");
        // the movement stick, another button or 0.1 m of drift leave (0xCD3070)
        for exit in [
            fp_input(&[], Vec2::new(0.0, 0.01), Vec3::ZERO),
            fp_input(&[3], Vec2::ZERO, Vec3::ZERO),
            fp_input(&[], Vec2::ZERO, Vec3::new(0.1, 0.0, 0.0)),
        ] {
            let mut fp = FirstPerson::default();
            fp.activator(&fp_input(&[], Vec2::ZERO, Vec3::ZERO));
            fp.activator(&fp_input(&[FP_BUTTON], Vec2::ZERO, Vec3::ZERO));
            fp.activator(&fp_input(&[], Vec2::ZERO, Vec3::ZERO));
            assert!(fp.active);
            assert!(fp.activator(&exit) && !fp.active);
        }
        // it does not start while the stick is moving
        let mut fp = FirstPerson::default();
        fp.activator(&fp_input(&[], Vec2::ZERO, Vec3::ZERO));
        assert!(!fp.activator(&fp_input(&[FP_BUTTON], Vec2::new(0.5, 0.0), Vec3::ZERO)));
    }

    #[test]
    fn first_person_looks_from_the_head_and_clamps_the_pitch() {
        let mut fp = FirstPerson::default();
        fp.activate(0.0);
        let head = Vec3::new(1.0, 1.7, 2.0);
        let v = fp.update(&input(1.0 / 60.0), head);
        assert_eq!(v.eye, head);
        // yaw 0 looks the way the orbit camera at yaw 0 looks (its eye on -Z of the target, facing +Z)
        assert!((v.look_at - head - Vec3::Z).length() < 1e-4, "{:?}", v.look_at - head);
        // the stick held down for 3 s pitches down to the 75° limit
        let mut inp = input(1.0 / 60.0);
        inp.stick = Vec2::new(0.0, -1.0);
        for _ in 0..180 { fp.update(&inp, head); }
        assert!((fp.pitch - FIRST_PERSON.pitch[1]).abs() < 0.01, "{}", fp.pitch);
    }

    #[test]
    fn activation_takes_the_previous_view_or_the_heading() {
        let inp = input(1.0 / 60.0);
        // LowHigh turned 1 rad off the heading
        let mut low = NativeCamera { profile: &LOW_HIGH, ..default() };
        low.activate(&inp);
        low.yaw = 1.0;
        low.eye = low.target + orbit_offset(low.yaw, low.pitch, low.dist);
        // StandOnLedge measures the LowHigh eye round its own (higher) target: same azimuth, its own e
        let mut ledge = NativeCamera { profile: &STAND_ON_LEDGE, ..default() };
        ledge.activate_after(&inp, Handover::Eye(low.eye));
        assert!((ledge.yaw - 1.0).abs() < 1e-4, "{}", ledge.yaw);
        assert_eq!(ledge.e, 0.865);
        let (d, pitch) = ledge.dist_pitch(ledge.e);
        assert!((d - 1.962).abs() < 0.01 && (pitch.to_degrees() - 66.0).abs() < 0.5, "{d} {}", pitch.to_degrees());
        assert!((ledge.target.y - 1.7).abs() < 1e-5);
        // it hands its yaw on (+600); the Leap of Faith turns behind the heading instead (+512 = 1)
        ledge.yaw = 2.0;
        let prev = if ledge.profile.hands_yaw { Handover::Yaw(ledge.yaw) } else { Handover::Eye(ledge.eye) };
        let mut back = NativeCamera { profile: &LOW_HIGH, ..default() };
        back.activate_after(&inp, prev);
        assert_eq!(back.yaw, 2.0);
        assert_eq!(back.e, LOW_HIGH.e_default, "InitElevation resets e (+496 = 1)");
        let mut lof = NativeCamera { profile: &LEAP_OF_FAITH, ..default() };
        lof.activate_after(&inp, prev);
        assert!((lof.yaw - game_heading(inp.forward)).abs() < 1e-5);
    }
}

