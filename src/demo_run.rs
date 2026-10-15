//! The demo course's run (`cargo test --release demo_course_run -- --ignored --nocapture`): plays `demo_course` in the
//! headless simulation, frame by frame, deciding the pad from what the character is doing, and writes the run as a
//! recording (`input.txt`, the F9 format) that `AC_REPLAY` plays back in the full game. Output: `AC_DEMO_OUT` or
//! `target/demo-course/`.

use super::*;
use crate::demo_course;
use crate::player::ActorContextId as C;
use crate::recorder::InputFrame;

/// One frame's controls.
#[derive(Clone, Copy, Default)]
struct In {
    dir: Vec3,
    mag: f32,
    high: bool,
    legs: bool,
    hand: bool,
    press_legs: bool,
    press_hand: bool,
}

impl In {
    fn go(dir: Vec3, mag: f32, high: bool, legs: bool) -> Self {
        In { dir: Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero(), mag, high, legs, ..default() }
    }
    fn idle() -> Self {
        In::default()
    }
}

struct Run {
    s: Sim,
    frames: Vec<InputFrame>,
    yaw: f32,
    /// The stick's camera frame: straight behind the character, eased (the shown camera's side offset would bend the
    /// stick through the interpreter's response curve).
    iyaw: f32,
    pitch: f32,
    /// The camera's yaw offset from straight behind (+ = round to the character's left).
    offset: f32,
    t: f32,
    log: Vec<String>,
}

impl Run {
    fn new() -> Self {
        let mut s = Sim::new_raw(demo_course::START, demo_course::START_HEADING);
        s.cam_follow = false;
        {
            let w = s.app.world_mut();
            let mut c = w.resource_mut::<crate::collision::CollisionWorld>();
            let mut c2 = std::mem::take(&mut *c);
            let mut g = std::mem::take(&mut *w.resource_mut::<crate::guidance::GuidanceWorld>());
            demo_course::add(&mut c2, &mut g);
            *w.resource_mut::<crate::collision::CollisionWorld>() = c2;
            *w.resource_mut::<crate::guidance::GuidanceWorld>() = g;
        }
        Run { s, frames: Vec::new(), yaw: demo_course::START_HEADING + 0.5, iyaw: demo_course::START_HEADING, pitch: -0.3, offset: 0.5, t: 0.0, log: Vec::new() }
    }

    fn ctx(&self) -> C {
        self.s.loco().current
    }
    fn feet(&self) -> Vec3 {
        self.s.body().feet
    }

    /// One frame: the pad as `read_pad` would leave it (the buttons' ages count up, a press sets 0), the camera behind
    /// the character at `offset`, eased; applied the way a replay applies it, then recorded.
    fn frame(&mut self, i: In) {
        let dt = 1.0 / 60.0;
        let (legs_ago, hand_ago) = {
            let p = self.s.app.world().resource::<PadInput>();
            let first = self.frames.is_empty();
            let age = |a: f32| if first { f32::INFINITY } else { a + dt };
            (if i.press_legs { 0.0 } else { age(p.legs_pressed_ago) }, if i.press_hand { 0.0 } else { age(p.hand_pressed_ago) })
        };
        let want = self.s.body().heading + self.offset;
        let d = (want - self.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        self.yaw += d * (2.5 * dt).min(1.0);
        let yaw = (self.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        let h = self.s.body().heading;
        let di = (h - self.iyaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        self.iyaw += di * (6.0 * dt).min(1.0);
        let iyaw = (self.iyaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        let mag = if i.dir == Vec3::ZERO { 0.0 } else { i.mag };
        let mut f = InputFrame {
            dt_nanos: 0,
            dir: if mag > 0.0 { i.dir } else { Vec3::ZERO },
            magnitude: mag,
            speed01: if mag <= 0.35 { 0.0 } else { ((mag - 0.35) / 0.65).min(1.0) },
            high_profile: i.high,
            legs_held: i.legs,
            legs_pressed_ago: legs_ago,
            cam_yaw: yaw,
            cam_pitch: self.pitch,
            hand_pressed_ago: hand_ago,
            hand_held: i.hand,
            input_yaw: iyaw,
        };
        {
            let w = self.s.app.world_mut();
            f.apply(&mut w.resource_mut::<PadInput>());
            let mut rig = w.resource_mut::<CameraRig>();
            rig.yaw = f.cam_yaw;
            rig.pitch = f.cam_pitch;
        }
        self.s.app.update();
        f.dt_nanos = self.s.app.world().resource::<Time>().delta().as_nanos() as u64;
        self.t += f.dt_nanos as f32 * 1e-9;
        self.frames.push(f);
    }

    fn hold(&mut self, secs: f32, i: In) {
        for _ in 0..(secs * 60.0).round() as usize {
            self.frame(i);
        }
    }

    /// Frames with `input` until `done`, at most `max` seconds (then the run fails with what it was doing).
    fn until(&mut self, what: &str, max: f32, mut input: impl FnMut(&Run) -> In, done: impl Fn(&Run) -> bool) {
        for _ in 0..(max * 60.0) as usize {
            let i = input(self);
            self.frame(i);
            if done(self) {
                self.log.push(format!("{:7.2}s  {what}  ({:?} at {:.2?})", self.t, self.ctx(), self.feet()));
                return;
            }
        }
        panic!("{what}: not done within {max} s: {:?} at {:?}, ledge {:?} climb {:?} narrow {:?}\n{}", self.ctx(), self.feet(),
            self.s.data().ledge.last_action, self.s.data().climb.last_action, self.s.data().narrow.state, self.log.join("\n"));
    }

    /// Walk into a wall along `dir` (stick 0.6, high profile, as the tests' `walk_to_wall`) until the feet have stood
    /// still for 6 frames.
    fn walk_to_wall(&mut self, what: &str, dir: Vec3, high: bool, max: f32) {
        let mut prev = self.feet();
        let mut still = 0;
        for _ in 0..(max * 60.0) as usize {
            self.frame(In::go(dir, 0.6, high, false));
            let f = self.feet();
            still = if (f - prev).with_y(0.0).length() < 0.002 { still + 1 } else { 0 };
            prev = f;
            if still >= 6 {
                self.log.push(format!("{:7.2}s  {what}  ({:?} at {:.2?})", self.t, self.ctx(), self.feet()));
                return;
            }
        }
        panic!("{what}: never stopped at the wall: {:?}", self.feet());
    }

    /// The stick toward a point on the ground plane.
    fn toward(&self, to: Vec3) -> Vec3 {
        Vec3::new(to.x - self.feet().x, 0.0, to.z - self.feet().z).normalize_or_zero()
    }

    fn write(&self) -> std::path::PathBuf {
        let dir = std::env::var_os("AC_DEMO_OUT").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("demo-course"));
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = String::from("# ac_port recording v1 (demo course): dt_nanos dir.x dir.y dir.z magnitude speed01 high legs legs_pressed_ago cam_yaw cam_pitch hand_pressed_ago hand_held input_yaw\n");
        let (p, h) = (demo_course::START, demo_course::START_HEADING);
        text += &format!("start {:?} {:?} {:?} {:?}\n", p.x, p.y, p.z, h);
        for f in &self.frames {
            text += &f.to_line();
            text.push('\n');
        }
        std::fs::write(dir.join("input.txt"), text).unwrap();
        std::fs::write(dir.join("log.txt"), self.log.join("\n") + "\n").unwrap();
        dir
    }
}

#[test]
#[ignore]
fn demo_course_run() {
    let mut r = Run::new();
    let x = Vec3::X;
    r.hold(1.0, In::idle());

    // ---- walk, run, hop over the railing (face x -112.15)
    r.hold(2.0, In::go(x, 1.0, false, false));
    r.until("run up to the railing", 6.0, |_| In::go(x, 1.0, true, false), |r| r.feet().x >= -115.4);
    r.frame(In { press_legs: true, ..In::go(x, 1.0, true, false) });
    r.until("hop over the railing", 4.0, |_| In::go(x, 1.0, true, false), |r| r.ctx() == C::Ground && r.feet().x > -111.0);

    // ---- wall run onto the 1.8 m block (face x -95), then the free run up the two roofs
    r.until("run at the wall", 6.0, |r| In::go(r.toward(Vec3::new(-95.0, 0.0, 130.0)), 1.0, true, false), |r| r.feet().x >= -96.25);
    r.frame(In { press_legs: true, ..In::go(x, 1.0, true, true) });
    r.until("wall run", 1.5, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::Walling);
    r.until("pull up onto the block", 4.0, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::Ground && r.feet().y > 1.7);
    r.until("free run up the roofs", 8.0, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::Ground && r.feet().y > 3.9 && r.feet().x > -82.0);

    // ---- the beam onto the platform
    r.until("walk onto the beam", 4.0, |_| In::go(x, 1.0, false, false), |r| r.ctx() == C::NarrowObject);
    r.until("cross the beam", 12.0, |_| In::go(x, 1.0, false, false), |r| r.ctx() == C::Ground && r.feet().x > -70.8);

    // ---- free run across the gap onto the 3.5 m roof, then the pull-down from its +X edge
    r.until("free run over the gap", 6.0, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::Ground && r.feet().x > -63.0 && (r.feet().y - 3.5).abs() < 0.05);
    r.until("walk to the edge", 5.0, |_| In::go(x, 1.0, false, false), |r| r.feet().x > -58.3 && r.s.body().velocity.length() < 0.05);
    r.hold(0.3, In::idle());
    r.frame(In { press_hand: true, ..In::idle() });
    r.until("pull down into a hang", 1.0, |_| In::idle(), |r| r.ctx() == C::Ledge);
    r.until("hang", 4.0, |_| In::idle(), |r| r.s.data().ledge.mv.is_none());
    r.hold(0.4, In::idle());
    r.hold(2.4, In::go(Vec3::Z, 1.0, false, false)); // shimmy (the character faces -X: +Z is its left)
    r.hold(0.4, In::idle());
    r.frame(In { press_hand: true, ..In::idle() });
    r.until("let go and land", 3.0, |_| In::idle(), |r| r.ctx() == C::Ground);

    // ---- jump up to the ledge (C, face x -45), shimmy, side jump to D, pull up
    r.until("walk to the ledge wall", 6.0, |r| In::go(r.toward(Vec3::new(-45.0, 0.0, 128.0)), 1.0, true, false), |r| r.feet().x > -47.0);
    r.until("line up with the ledge wall", 3.0, |r| In::go(r.toward(Vec3::new(-46.2, 0.0, 128.0)), 0.6, false, false), |r| (r.feet() - Vec3::new(-46.2, 0.0, 128.0)).with_y(0.0).length() < 0.2);
    r.walk_to_wall("up against the wall", x, true, 3.0);
    r.hold(0.35, In { high: true, legs: true, ..In::idle() });
    r.until("jump up to the ledge", 3.0, |_| In { high: true, ..In::idle() }, |r| r.ctx() == C::Ledge);
    r.until("grab", 3.0, |_| In::idle(), |r| r.s.data().ledge.mv.is_none());
    r.hold(0.3, In::idle());
    r.until("shimmy to the gap and side jump", 10.0, |_| In::go(Vec3::Z, 1.0, false, false), |r| r.s.data().ledge.last_action == "side jump");
    r.until("land the side jump", 3.0, |_| In::go(Vec3::Z, 1.0, false, false), |r| r.s.data().ledge.mv.is_none());
    r.hold(0.3, In::idle());
    r.until("pull up onto D", 4.0, |_| In::go(x, 1.0, false, false), |r| r.ctx() == C::Ground && r.feet().y > 2.5);
    r.until("run off D", 3.0, |_| In::go(x, 1.0, true, false), |r| r.ctx() == C::InAir);
    r.until("land", 3.0, |_| In::go(x, 1.0, true, false), |r| r.ctx() == C::Ground);

    // ---- the ladder (face x -25) onto the platform, the pilotis, the drop
    r.until("line up with the ladder", 6.0, |r| In::go(r.toward(Vec3::new(-29.0, 0.0, 130.0)), 1.0, true, false), |r| (r.feet() - Vec3::new(-29.0, 0.0, 130.0)).with_y(0.0).length() < 0.4);
    r.until("walk onto the ladder", 4.0, |_| In::go(x, 1.0, false, false), |r| r.ctx() == C::Ladder);
    r.until("climb the ladder", 15.0, |_| In::go(x, 1.0, false, false), |r| r.ctx() == C::Ground && r.feet().y > 4.9);
    r.until("free run onto a post", 5.0, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::NarrowObject);
    r.until("stand on the post", 3.0, |_| In { high: true, ..In::idle() }, |r| r.s.data().narrow.state == crate::player::narrow::BeamState::PilotisWait);
    for _ in 0..2 {
        r.frame(In { press_legs: true, ..In::go(x, 1.0, true, false) });
        r.until("hop", 0.5, |_| In::go(x, 1.0, true, false), |r| r.ctx() == C::InAir);
        r.until("land on the next post", 3.0, |_| In { high: true, ..In::idle() }, |r| r.ctx() == C::NarrowObject && r.s.data().narrow.state == crate::player::narrow::BeamState::PilotisWait);
    }
    r.frame(In { press_legs: true, ..In::go(x, 1.0, true, false) });
    r.until("hop onto the platform", 3.0, |_| In::go(x, 1.0, true, false), |r| r.ctx() == C::Ground);
    r.until("free run off the platform", 4.0, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::InAir);
    r.until("land the drop", 4.0, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::Ground && r.feet().y < 0.1);

    // ---- the swing bars
    r.until("hop onto the swing platform", 6.0, |r| In::go(r.toward(Vec3::new(7.5, 0.0, 130.0)), 1.0, true, false), |r| r.ctx() == C::Ground && r.feet().y > 1.1);
    r.until("step to the start", 3.0, |r| In::go(r.toward(Vec3::new(7.0, 1.2, 130.0)), 0.6, false, false), |r| (r.feet() - Vec3::new(7.0, 1.2, 130.0)).with_y(0.0).length() < 0.3);
    r.until("face the bars", 2.0, |_| In::go(x, 0.4, false, false), |r| r.s.body().forward().dot(Vec3::X) > 0.98);
    r.hold(0.35, In::go(x, 1.0, true, false));
    r.frame(In { press_legs: true, ..In::go(x, 1.0, true, false) });
    r.until("jump to the first bar", 3.0, |_| In::go(x, 1.0, true, false), |r| r.s.data().ledge.swing.is_some());
    r.until("swing", 2.0, |_| In::go(x, 1.0, true, true), |r| r.s.data().ledge.swing.is_some_and(|w| matches!(w.phase, crate::player::swing::SwingPhase::Cycle(_))));
    for _ in 0..2 {
        r.frame(In { press_legs: true, ..In::go(x, 1.0, true, true) });
        r.until("swing jump", 3.0, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::InAir);
        r.until("catch the next bar", 3.0, |_| In::go(x, 1.0, true, true), |r| r.s.data().ledge.swing.is_some() && r.ctx() == C::Ledge);
        r.until("swing", 2.0, |_| In::go(x, 1.0, true, true), |r| r.s.data().ledge.swing.is_some_and(|w| matches!(w.phase, crate::player::swing::SwingPhase::Cycle(_))));
    }
    r.frame(In { press_legs: true, ..In::go(x, 1.0, true, true) });
    r.until("jump off the last bar", 3.0, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::InAir);
    r.until("land on the far platform", 3.0, |_| In::go(x, 1.0, true, false), |r| r.ctx() == C::Ground);
    r.hold(0.3, In::idle());

    // ---- climb the tower (face x 35)
    r.until("walk off the platform", 4.0, |_| In::go(x, 1.0, true, false), |r| r.ctx() == C::Ground && r.feet().y < 0.1 && r.feet().x > 25.5);
    r.until("run to the tower", 6.0, |r| In::go(r.toward(Vec3::new(35.0, 0.0, 130.0)), 1.0, true, false), |r| r.feet().x > 30.5);
    r.until("slow down", 2.0, |_| In::go(x, 0.6, false, false), |r| r.feet().x > 32.5);
    r.walk_to_wall("up against the tower", x, false, 3.0);
    if std::env::var_os("DEMO_DEBUG").is_some() {
        let g = r.s.app.world().resource::<crate::guidance::GuidanceWorld>();
        let (feet, fwd) = (r.feet(), r.s.body().forward());
        let report = g.report(feet, fwd, Vec3::new(-0.75, 0.0, -0.9), Vec3::new(0.75, 1.2, 2.1), false);
        eprintln!("feet {feet:?} fwd {fwd:?} report edges {}", report.edges.len());
        for k in [0.0f32, 0.6, -0.6] {
            let c = feet + fwd * 0.95 + Vec3::Y * k;
            let foot = report.probe_facing(c, fwd, Vec3::new(0.25, 0.75, 0.3), 0.2, 0.785_398_2, crate::guidance::MASK_CLIMB_HOLDS);
            eprintln!("k {k}: foot {:?}", foot.map(|h| h.point));
        }
        eprintln!("ctx {:?} ground {:?}", r.ctx(), r.s.data().ground.sub_state);
    }
    r.frame(In { press_hand: true, ..In::go(x, 1.0, true, false) });
    r.until("start climbing", 1.0, |_| In::go(x, 1.0, true, false), |r| r.ctx() == C::Climb);
    r.until("climb to the top", 40.0, |_| In::go(x, 1.0, true, false), |r| r.ctx() == C::Ground && r.feet().y > 9.5);
    r.hold(0.5, In::idle());

    // ---- Leap of Faith into the haystack, hop out
    r.until("Leap of Faith", 4.0, |_| In::go(x, 1.0, true, true), |r| r.ctx() == C::InAir);
    r.until("into the haystack", 5.0, |_| In::idle(), |r| r.ctx() == C::HayStack);
    r.until("hide", 3.0, |_| In::idle(), |r| r.s.data().hay.phase == crate::player::hay::HayPhase::Waiting);
    r.hold(1.5, In::idle());
    r.until("hop out", 2.0, |_| In::go(x, 1.0, false, false), |r| r.ctx() == C::Ground);
    r.hold(1.5, In::go(x, 1.0, false, false));
    r.hold(1.5, In::idle());

    let dir = r.write();
    eprintln!("{}\n{} frames, {:.1} s → {}", r.log.join("\n"), r.frames.len(), r.t, dir.display());
}
