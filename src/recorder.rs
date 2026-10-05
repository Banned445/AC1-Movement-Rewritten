//! Bug capture and replay.
//!
//! **Recording (always on).** Every frame the recorder keeps:
//! - the frame's exact timestep (nanoseconds) and the final pad state the gameplay systems read, for the whole session;
//! - a state line for the last `RING_FRAMES` frames: context, root, heading, velocity, context actions, the playing
//!   animation, the IK goals and weights, and where the hand / elbow / foot / knee bones ended up after the solve.
//!
//! **F9** writes `bugs/bug-<unix time>/` in the project folder:
//! - `input.txt`: the start body and one line per frame since launch (replayable);
//! - `state.txt`: the last ~10 s of state lines;
//! - `screenshot.png` and `note.txt` (write one line about what looked wrong).
//!
//! **Replay.** `AC_REPLAY=<bug folder>` runs the recording in the full app (same timesteps, same pad, same camera) and
//! writes `replay_state.txt` next to it when it ends, then exits. Gameplay does not read animation or IK state and has
//! no randomness, so the replay is exact; `sim_tests::replay_bug_recordings` (ignored) replays the gameplay headless and
//! reports the first frame that differs from `state.txt`.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::Duration;

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::time::TimeUpdateStrategy;

use crate::anim::AnimPlayer;
use crate::camera::CameraRig;
use crate::ik::LimbIk;
use crate::input::PadInput;
use crate::model::Rig;
use crate::player::{Body, HumanDataBundle, LimbTargets, Locomotion, Player, PlayerSet};

/// Frames of state kept for a bug report (10 s at 60 fps).
pub const RING_FRAMES: usize = 600;

/// Where bug folders go: `bugs/` in the crate root when started by cargo (`cargo run` / `cargo test` set
/// `CARGO_MANIFEST_DIR` at run time), else `bugs/` next to the executable, like `game_dir.txt`. Read at run time so
/// no build-machine path is baked into the binary.
pub fn bugs_dir() -> std::path::PathBuf {
    if let Some(root) = std::env::var_os("CARGO_MANIFEST_DIR") {
        return std::path::PathBuf::from(root).join("bugs");
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("bugs")))
        .unwrap_or_else(|| std::path::PathBuf::from("bugs"))
}

/// One recorded frame of input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InputFrame {
    pub dt_nanos: u64,
    pub dir: Vec3,
    pub magnitude: f32,
    pub speed01: f32,
    pub high_profile: bool,
    pub legs_held: bool,
    pub legs_pressed_ago: f32,
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    /// The empty-hand button's age (12th field; older captures without it read as never pressed).
    pub hand_pressed_ago: f32,
}

impl InputFrame {
    pub fn to_line(&self) -> String {
        format!(
            "{} {:?} {:?} {:?} {:?} {:?} {} {} {:?} {:?} {:?} {:?}",
            self.dt_nanos,
            self.dir.x,
            self.dir.y,
            self.dir.z,
            self.magnitude,
            self.speed01,
            self.high_profile as u8,
            self.legs_held as u8,
            self.legs_pressed_ago,
            self.cam_yaw,
            self.cam_pitch,
            self.hand_pressed_ago
        )
    }

    pub fn parse(line: &str) -> Option<Self> {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() != 11 && t.len() != 12 {
            return None;
        }
        let f = |i: usize| t[i].parse::<f32>().ok();
        Some(InputFrame {
            dt_nanos: t[0].parse().ok()?,
            dir: Vec3::new(f(1)?, f(2)?, f(3)?),
            magnitude: f(4)?,
            speed01: f(5)?,
            high_profile: t[6] == "1",
            legs_held: t[7] == "1",
            legs_pressed_ago: f(8)?,
            cam_yaw: f(9)?,
            cam_pitch: f(10)?,
            hand_pressed_ago: if t.len() == 12 { f(11)? } else { f32::INFINITY },
        })
    }

    pub fn apply(&self, pad: &mut PadInput) {
        pad.dir = self.dir;
        pad.magnitude = self.magnitude;
        pad.speed01 = self.speed01;
        pad.high_profile = self.high_profile;
        pad.legs_held = self.legs_held;
        pad.legs_pressed_ago = self.legs_pressed_ago;
        pad.hand_pressed_ago = self.hand_pressed_ago;
    }
}

/// A parsed `input.txt`.
pub struct Recording {
    pub start_feet: Vec3,
    pub start_heading: f32,
    pub frames: Vec<InputFrame>,
}

impl Recording {
    pub fn load(dir: &std::path::Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join("input.txt")).ok()?;
        let mut start = None;
        let mut frames = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("start ") {
                let v: Vec<f32> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                if v.len() == 4 {
                    start = Some((Vec3::new(v[0], v[1], v[2]), v[3]));
                }
                continue;
            }
            frames.push(InputFrame::parse(line)?);
        }
        let (start_feet, start_heading) = start?;
        Some(Recording { start_feet, start_heading, frames })
    }
}

/// The gameplay part of a state line: `frame ctx x y z heading` (exact floats), compared by the headless replay.
pub fn gameplay_key(frame: u64, loco: &Locomotion, body: &Body) -> String {
    format!("{} {:?} {:?} {:?} {:?} {:?}", frame, loco.current, body.feet.x, body.feet.y, body.feet.z, body.heading)
}

#[derive(Resource, Default)]
struct Recorder {
    frame: u64,
    start: Option<(Vec3, f32)>,
    inputs: Vec<InputFrame>,
    ring: VecDeque<String>,
}

#[derive(Resource)]
struct Replay {
    dir: std::path::PathBuf,
    rec: Recording,
    next: usize,
}

pub struct RecorderPlugin;

impl Plugin for RecorderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Recorder>()
            .add_systems(Update, record_input.before(PlayerSet))
            .add_systems(PostUpdate, (record_state, save_bug).chain().after(bevy::transform::TransformSystems::Propagate));
        if let Ok(dir) = std::env::var("AC_REPLAY") {
            let dir = std::path::PathBuf::from(dir);
            let Some(rec) = Recording::load(&dir) else {
                error!("AC_REPLAY: no readable input.txt in {}", dir.display());
                return;
            };
            info!("replaying {} frames from {}", rec.frames.len(), dir.display());
            let first = rec.frames.first().map(|f| f.dt_nanos).unwrap_or(0);
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_nanos(first)))
                .insert_resource(Replay { dir, rec, next: 0 })
                .add_systems(PostStartup, replay_place)
                .add_systems(Update, replay_input.before(record_input))
                .add_systems(Last, replay_advance);
        }
    }
}

fn replay_place(replay: Res<Replay>, mut q: Query<&mut Body, With<Player>>) {
    for mut b in &mut q {
        b.feet = replay.rec.start_feet;
        b.heading = replay.rec.start_heading;
    }
}

/// Overrides the pad (and the camera) with the recorded frame, right before the gameplay systems.
fn replay_input(replay: Res<Replay>, mut pad: ResMut<PadInput>, mut rig: ResMut<CameraRig>) {
    if let Some(f) = replay.rec.frames.get(replay.next) {
        f.apply(&mut pad);
        rig.yaw = f.cam_yaw;
        rig.pitch = f.cam_pitch;
    }
}

/// Sets the next frame's timestep; at the end writes `replay_state.txt` and exits.
fn replay_advance(mut replay: ResMut<Replay>, rec: Res<Recorder>, mut strategy: ResMut<TimeUpdateStrategy>, mut exit: MessageWriter<AppExit>) {
    replay.next += 1;
    match replay.rec.frames.get(replay.next) {
        Some(f) => *strategy = TimeUpdateStrategy::ManualDuration(Duration::from_nanos(f.dt_nanos)),
        None => {
            let path = replay.dir.join("replay_state.txt");
            let text: String = rec.ring.iter().map(|l| format!("{l}\n")).collect();
            match std::fs::write(&path, text) {
                Ok(()) => info!("replay done: {}", path.display()),
                Err(e) => error!("replay: cannot write {}: {e}", path.display()),
            }
            exit.write(AppExit::Success);
        }
    }
}

fn record_input(time: Res<Time>, pad: Res<PadInput>, rig: Res<CameraRig>, mut rec: ResMut<Recorder>, q: Query<&Body, With<Player>>) {
    if rec.start.is_none() {
        if let Ok(b) = q.single() {
            rec.start = Some((b.feet, b.heading));
        }
    }
    let f = InputFrame {
        dt_nanos: time.delta().as_nanos() as u64,
        dir: pad.dir,
        magnitude: pad.magnitude,
        speed01: pad.speed01,
        high_profile: pad.high_profile,
        legs_held: pad.legs_held,
        legs_pressed_ago: pad.legs_pressed_ago,
        cam_yaw: rig.yaw,
        cam_pitch: rig.pitch,
        hand_pressed_ago: pad.hand_pressed_ago,
    };
    rec.inputs.push(f);
}

fn v3(v: Vec3) -> String {
    format!("({:.3},{:.3},{:.3})", v.x, v.y, v.z)
}

#[allow(clippy::type_complexity)]
fn record_state(
    time: Res<Time>,
    mut rec: ResMut<Recorder>,
    q: Query<(&Locomotion, &Body, &HumanDataBundle, &LimbTargets), With<Player>>,
    anim: Query<&AnimPlayer>,
    ik: Query<(&Rig, &LimbIk)>,
    globals: Query<&GlobalTransform>,
) {
    let frame = rec.frame;
    rec.frame += 1;
    let Ok((loco, body, data, targets)) = q.single() else { return };
    let mut s = gameplay_key(frame, loco, body);
    let _ = write!(s, " | t={:.3} dt={:.4} vel={} gnd={}", time.elapsed_secs(), time.delta_secs(), v3(body.velocity), body.grounded as u8);
    let _ = write!(s, " ground={:?}", data.ground.sub_state);
    match loco.current {
        crate::player::ActorContextId::Ledge => {
            let l = &data.ledge;
            let _ = write!(s, " ledge={:?}/{:?}/\"{}\"", l.sub_state, l.hang_type, l.last_action);
        }
        crate::player::ActorContextId::Climb => {
            let c = &data.climb;
            let _ = write!(s, " climb=pose{}/\"{}\"", c.pose, c.last_action);
        }
        _ => {}
    }
    if let Ok(a) = anim.single() {
        let _ = write!(s, " anim={}#{}@{:.3} fade={:.2}", a.clip.as_deref().unwrap_or("-"), a.item, a.phase, a.fade);
    }
    if let Some((l, r)) = targets.hands {
        let _ = write!(s, " tgtH={}{}", v3(l), v3(r));
    }
    if let Some((l, r)) = targets.feet {
        let _ = write!(s, " tgtF={}{}", v3(l), v3(r));
    }
    if let Ok((rig, ik)) = ik.single() {
        const NAMES: [&str; 4] = ["LH", "RH", "LF", "RF"];
        for (i, l) in ik.limbs.iter().enumerate() {
            if l.weight > 0.0 {
                let _ = write!(s, " ik{}=w{:.2}{}", NAMES[i], l.weight, v3(l.goal));
            }
        }
        if let Some(chains) = ik.chains() {
            // solved positions: hand / foot (end) and elbow / knee (middle)
            for (i, c) in chains.iter().enumerate() {
                let pos = |j: usize| rig.joints.get(j).and_then(|e| globals.get(*e).ok()).map(|g| g.translation());
                if let (Some(m), Some(e)) = (pos(c[1]), pos(c[2])) {
                    let _ = write!(s, " {}={}mid{}", NAMES[i], v3(e), v3(m));
                }
            }
        }
    }
    rec.ring.push_back(s);
    while rec.ring.len() > RING_FRAMES {
        rec.ring.pop_front();
    }
}

/// F9, or `AC_BUG_AT=<seconds>` (once, for scripted captures with `AC_AUTOPILOT`).
fn save_bug(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    rec: Res<Recorder>,
    replay: Option<Res<Replay>>,
    mut auto_done: Local<bool>,
) {
    if replay.is_some() {
        return;
    }
    let auto = !*auto_done
        && std::env::var("AC_BUG_AT").ok().and_then(|v| v.parse::<f32>().ok()).is_some_and(|at| time.elapsed_secs() >= at);
    if auto {
        *auto_done = true;
    }
    if !keys.just_pressed(KeyCode::F9) && !auto {
        return;
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let dir = bugs_dir().join(format!("bug-{stamp}"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        error!("F9: cannot create {}: {e}", dir.display());
        return;
    }
    let (feet, heading) = rec.start.unwrap_or((Vec3::ZERO, 0.0));
    let mut input = String::from("# ac_port recording v1: dt_nanos dir.x dir.y dir.z magnitude speed01 high legs legs_pressed_ago cam_yaw cam_pitch\n");
    let _ = writeln!(input, "start {:?} {:?} {:?} {:?}", feet.x, feet.y, feet.z, heading);
    for f in &rec.inputs {
        input.push_str(&f.to_line());
        input.push('\n');
    }
    let state: String = rec.ring.iter().map(|l| format!("{l}\n")).collect();
    let note = "Describe what looked wrong, and roughly when (seconds before pressing F9):\n\n";
    let ok = std::fs::write(dir.join("input.txt"), input).is_ok()
        && std::fs::write(dir.join("state.txt"), state).is_ok()
        && std::fs::write(dir.join("note.txt"), note).is_ok();
    if !ok {
        error!("F9: cannot write the bug files in {}", dir.display());
        return;
    }
    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.join("screenshot.png")));
    info!("bug saved: {} ({} frames of input)", dir.display(), rec.inputs.len());
}
