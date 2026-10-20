//! City streaming tests (RE/17 §7). Tests that need the install skip when it is absent.
use bevy::prelude::*;

use crate::collision::CollisionWorld;

fn village() -> Option<CollisionWorld> {
    crate::native_map::village_simulation_fixture().map(|(c, _, _)| c)
}

/// Deterministic pseudo-random points around the village spawn.
fn points(n: usize, centre: Vec3, spread: Vec3) -> Vec<Vec3> {
    let mut s = 0x1234_5678u32;
    let mut r = || { s ^= s << 13; s ^= s >> 17; s ^= s << 5; s as f32 / u32::MAX as f32 * 2.0 - 1.0 };
    (0..n).map(|_| centre + Vec3::new(r(), r(), r()) * spread).collect()
}

#[test]
fn triangle_grid_answers_like_the_linear_scan() {
    let Some(linear) = village() else { return };
    let mut indexed = CollisionWorld { triangles: linear.triangles.clone(), ..Default::default() };
    indexed.enable_index(8.0);
    let centre = Vec3::new(19.0, 36.0, 36.0);
    for p in points(400, centre, Vec3::new(40.0, 8.0, 40.0)) {
        for dir in [Vec3::NEG_Y, Vec3::X, Vec3::new(0.3, -0.2, 0.9)] {
            assert_eq!(linear.ray_distance(p, dir, 30.0, crate::layers::MAIN_CHARACTER), indexed.ray_distance(p, dir, 30.0, crate::layers::MAIN_CHARACTER), "ray at {p}");
        }
        assert_eq!(linear.floor_height_below(p, 20.0), indexed.floor_height_below(p, 20.0));
        assert_eq!(linear.capsule_fits(p), indexed.capsule_fits(p));
        assert_eq!(linear.point_inside(p), indexed.point_inside(p), "inside at {p}");
        assert_eq!(linear.support(p).map(|s| (s.y, s.normal_y)), indexed.support(p).map(|s| (s.y, s.normal_y)));
        assert_eq!(linear.sphere_free_distance(p, Vec3::X, 0.3, 4.0), indexed.sphere_free_distance(p, Vec3::X, 0.3, 4.0));
        assert_eq!(linear.capsule_cast_free(p, 1.8, 0.4, Vec3::new(1.0, 0.5, -1.0)), indexed.capsule_cast_free(p, 1.8, 0.4, Vec3::new(1.0, 0.5, -1.0)));
        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        assert_eq!(linear.obb_free(p, axes, Vec3::new(0.5, 1.0, 0.3)), indexed.obb_free(p, axes, Vec3::new(0.5, 1.0, 0.3)));
        assert_eq!(linear.obb_lowest(p, axes, Vec3::splat(1.0), 1), indexed.obb_lowest(p, axes, Vec3::splat(1.0), 1));
        let (lo, hi) = (p - Vec3::splat(2.0), p + Vec3::splat(2.0));
        assert_eq!(linear.triangle_candidates(lo, hi).collect::<Vec<_>>(), indexed.triangle_candidates(lo, hi).collect::<Vec<_>>());
    }
}

#[test]
fn removed_triangles_free_their_slots_and_leave_the_others_in_place() {
    let Some(linear) = village() else { return };
    let mut w = CollisionWorld::default();
    w.enable_index(8.0);
    let half = linear.triangles.len() / 2;
    let a = w.add_triangles(linear.triangles[..half].to_vec());
    let b = w.add_triangles(linear.triangles[half..].to_vec());
    assert_eq!(a.len() + b.len(), linear.triangles.len());
    let before: Vec<_> = b.iter().map(|&s| w.triangles[s as usize].vertices).collect();
    w.remove_triangles(&a);
    // the second half keeps its slots and data; the first half no longer answers queries
    assert!(b.iter().zip(&before).all(|(&s, v)| w.triangles[s as usize].vertices == *v));
    let p = Vec3::new(19.0, 40.0, 36.0);
    let only_b = CollisionWorld { triangles: linear.triangles[half..].to_vec(), ..Default::default() };
    assert_eq!(w.floor_height_below(p, 20.0), only_b.floor_height_below(p, 20.0));
    // re-adding reuses the freed slots
    let again = w.add_triangles(linear.triangles[..half].to_vec());
    let mut s1 = again.clone(); s1.sort(); let mut s2 = a.clone(); s2.sort();
    assert_eq!(s1, s2);
    assert_eq!(w.floor_height_below(p, 20.0), linear.floor_height_below(p, 20.0));
}

// ---- headless Damascus: the real streaming system next to the movement contexts ----

use std::time::Duration;
use bevy::{ecs::system::RunSystemOnce, time::TimeUpdateStrategy};
use crate::{input::PadInput, player::{ActorContextId, Body, Locomotion}};

pub(crate) struct CitySim {
    pub app: App,
    pub player: Entity,
}

impl CitySim {
    pub fn new(spawn: &str) -> Option<Self> {
        let game = crate::assets::find_game_dir()?;
        if !game.join(super::DAMASCUS.archive).is_file() { return None; }
        let prepared = super::prepare(&game, &super::DAMASCUS, false, spawn).expect("Damascus must load when the archive is present");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)))
            .insert_resource(crate::map_menu::neutral_pad())
            .insert_resource(crate::player::SpawnPoint(Vec3::ZERO))
            .init_resource::<crate::camera::CameraRig>()
            .init_resource::<Assets<Mesh>>().init_resource::<Assets<StandardMaterial>>().init_resource::<Assets<super::ground::GroundSurface>>().init_resource::<Assets<Image>>()
            .init_resource::<CollisionWorld>().init_resource::<crate::guidance::GuidanceWorld>()
            .init_resource::<super::stream::City>()
            .add_systems(Update, (super::update_city, crate::player::interpreter::update_interpreter, crate::player::social::update_social, crate::player::crowd::update_crowd, crate::player::ground::update_ground,
                crate::player::air::update_air, crate::player::ledge::update_ledge, crate::player::climb::update_climb, crate::player::hay::update_hay, crate::player::kiosk::update_kiosk,
                crate::player::walling::update_walling, crate::player::narrow::update_narrow, crate::player::ladder::update_ladder, crate::player::release_limbs, super::hold_player).chain());
        let lab_ik = std::env::var_os("AC_LAB_IK").is_some();
        if lab_ik {
            app.add_systems(Update, (crate::anim::headless_animation_systems(), crate::ik::headless_ik_systems()).chain().after(super::hold_player));
        } else {
            app.add_systems(Update, crate::anim::headless_animation_systems().after(super::hold_player));
        }
        let animated = crate::bone_pose::add_headless_animation(&mut app);
        let player = app.world_mut().spawn((crate::player::player_components(prepared.spawn, prepared.heading), Transform::default())).id();
        if animated {
            app.world_mut().entity_mut(player).insert(crate::anim::AnimPlayer::default());
            if lab_ik {
                crate::ik::add_headless_rig(app.world_mut(), player);
            }
        }
        app.world_mut().run_system_once_with(super::spawn, prepared).unwrap();
        app.update();
        Some(Self { app, player })
    }
    pub fn pad(&mut self, dir: Vec3, stick: f32, high: bool, legs: bool) {
        let mut p = self.app.world_mut().resource_mut::<PadInput>();
        p.dir = dir.normalize_or_zero(); p.magnitude = stick;
        p.speed01 = if stick <= 0.35 { 0.0 } else { ((stick - 0.35) / 0.65).min(1.0) };
        p.high_profile = high; p.legs_held = legs;
    }
    pub fn press_legs(&mut self) { self.app.world_mut().resource_mut::<PadInput>().legs_pressed_ago = 0.0; }
    pub fn step(&mut self) {
        { let mut p = self.app.world_mut().resource_mut::<PadInput>(); p.legs_pressed_ago += 1.0 / 60.0; p.hand_pressed_ago += 1.0 / 60.0; }
        // the camera behind the character: world-direction scripts keep their meaning for the actor-frame stick
        let h = self.body().heading;
        self.app.world_mut().resource_mut::<crate::camera::CameraRig>().yaw = h;
        self.app.update();
    }
    pub fn run(&mut self, seconds: f32) { for _ in 0..(seconds * 60.0) as usize { self.step(); } }
    pub fn body(&self) -> &Body { self.app.world().get::<Body>(self.player).unwrap() }
    pub fn loco(&self) -> &Locomotion { self.app.world().get::<Locomotion>(self.player).unwrap() }
    pub fn stats(&self) -> super::stream::CityStats { self.app.world().resource::<super::stream::City>().0.as_ref().unwrap().stats.clone() }
    /// A fresh player in place of the old one (all its components rebuilt: contexts, data, animation), and the pad,
    /// the camera rig and the player's resources reset: a batch replay starts each recording as a new process would.
    pub fn respawn_player(&mut self, feet: Vec3, heading: f32) {
        let animated = self.app.world().get::<crate::anim::AnimPlayer>(self.player).is_some();
        self.app.world_mut().despawn(self.player);
        let player = self.app.world_mut().spawn((crate::player::player_components(feet, heading), Transform::default())).id();
        if animated {
            self.app.world_mut().entity_mut(player).insert(crate::anim::AnimPlayer::default());
            if std::env::var_os("AC_LAB_IK").is_some() {
                crate::ik::add_headless_rig(self.app.world_mut(), player);
            }
        }
        self.player = player;
        self.app.insert_resource(crate::map_menu::neutral_pad());
        self.app.insert_resource(crate::camera::CameraRig::default());
    }

    pub fn teleport(&mut self, feet: Vec3) {
        let mut b = self.app.world_mut().get_mut::<Body>(self.player).unwrap();
        b.feet = feet; b.velocity = Vec3::ZERO; b.proxy.manifold.clear();
    }
}

/// Live-analysis course finder around the Bureau (game coordinates, Z up): flat open squares (12 × 12 m, every 1 m
/// sample within 0.15 m of the mean) for walking tests, then roof edges (ledge edges with a horizontal wall normal)
/// with the drop beyond them, for walk-off / edge tests. `LAB_RADIUS` (default 60) m.
#[test]
#[ignore = "live-analysis course finder; requires the local game install"]
fn probe_lab_course_finder() {
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    s.run(0.5);
    let radius: f32 = std::env::var("LAB_RADIUS").ok().and_then(|v| v.parse().ok()).unwrap_or(60.0);
    let centre = s.body().feet;
    let game = |p: Vec3| Vec3::new(p.x, -p.z, p.y);
    let world = s.app.world();
    let col = world.resource::<CollisionWorld>();
    let height = |x: f32, z: f32| col.floor_height_below(Vec3::new(x, 60.0, z), 90.0);
    // flat squares, best (highest, so roofs first) without overlaps
    let n = (radius as i32) * 2;
    let mut squares: Vec<(Vec3, f32)> = Vec::new();
    for i in (0..n).step_by(3) {
        for j in (0..n).step_by(3) {
            let (x0, z0) = (centre.x - radius + i as f32, centre.z - radius + j as f32);
            let mut hs = Vec::with_capacity(144);
            for a in 0..12 {
                for b in 0..12 {
                    if let Some(h) = height(x0 + a as f32, z0 + b as f32) { hs.push(h) } else { break }
                }
            }
            if hs.len() < 144 { continue; }
            let mean = hs.iter().sum::<f32>() / hs.len() as f32;
            if hs.iter().all(|h| (h - mean).abs() < 0.15) {
                let c = Vec3::new(x0 + 5.5, mean, z0 + 5.5);
                if squares.iter().all(|(o, _)| o.distance(c) > 12.0) { squares.push((c, mean)); }
            }
        }
    }
    squares.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (c, h) in squares.iter().take(15) {
        println!("flat 12x12 centre {:.2?} height {h:.2}", game(*c));
    }
    // roof edges and their drops
    let guidance = world.resource::<crate::guidance::GuidanceWorld>();
    let mut edges = 0;
    for e in guidance.edges.iter().filter(|e| e.subtype == crate::guidance::GuidanceSubType::LedgeGrab) {
        let mid = (e.p0 + e.p1) * 0.5;
        if mid.distance(centre) > radius || e.p0.distance(e.p1) < 3.0 || e.n1.y.abs() > 0.3 || e.n0.y < 0.7 { continue; }
        let out = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        let inside = height(mid.x - out.x * 0.8, mid.z - out.z * 0.8);
        let beyond = height(mid.x + out.x * 0.8, mid.z + out.z * 0.8);
        let (Some(top), Some(below)) = (inside, beyond) else { continue };
        let drop = mid.y - below;
        if (top - mid.y).abs() > 0.2 || !(1.0..15.0).contains(&drop) { continue; }
        println!("edge mid {:.2?} out {:.2?} len {:.1} drop {drop:.2}", game(mid), game(out), e.p0.distance(e.p1));
        edges += 1;
        if edges >= 80 { break; }
    }
}

/// A live-game recording made in Damascus replayed in the port's Damascus at the same coordinates:
/// `AC_LAB_TRACE=<frames.csv>` (see `sim_tests::load_lab_trace`). The player starts where the recording starts
/// (the streaming loads around it first), and `port_trace.csv` is written next to the input.
#[test]
#[ignore = "live-analysis replay; requires the local game install"]
fn replay_lab_trace_city() {
    let Ok(path) = std::env::var("AC_LAB_TRACE") else {
        eprintln!("set AC_LAB_TRACE to a lab frames.csv");
        return;
    };
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    replay_trace(&mut s, &path, false);
}

/// Every recording listed in the file `AC_LAB_TRACES` (one frames.csv per line) in one process: the city is loaded once
/// and each replay starts with a fresh player at its recording's start (`CitySim::respawn_player`), so nothing carries
/// over from the one before. Writes each `port_trace.csv` as `replay_lab_trace_city` does (livelab/tools/compare_all.py).
#[test]
#[ignore = "live-analysis replay; requires the local game install"]
fn replay_lab_traces_city() {
    let Ok(list) = std::env::var("AC_LAB_TRACES") else {
        eprintln!("set AC_LAB_TRACES to a file listing frames.csv paths");
        return;
    };
    let paths: Vec<String> = std::fs::read_to_string(&list).expect("read AC_LAB_TRACES").lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    for (i, path) in paths.iter().enumerate() {
        let started = std::time::Instant::now();
        replay_trace(&mut s, path, i > 0);
        eprintln!("replayed {}/{} in {:.1} s: {path}", i + 1, paths.len(), started.elapsed().as_secs_f32());
    }
}

fn replay_trace(s: &mut CitySim, path: &str, respawn: bool) {
    let trace = crate::sim_tests::load_lab_trace(path);
    // game (x, y, z) -> Bevy (x, z, -y)
    let start = Vec3::new(trace.start.x, trace.start.z, -trace.start.y);
    if respawn {
        s.respawn_player(start, trace.heading);
    }
    s.teleport(start);
    s.app.world_mut().get_mut::<Body>(s.player).unwrap().heading = trace.heading;
    // let the cells around the start stream in, standing (a recording far from the spawn waited on the streaming hold
    // while its inputs ran on)
    s.app.insert_resource(crate::map_menu::neutral_pad());
    s.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)));
    s.settle_streaming(120.0, Some(start));
    s.teleport(start);
    s.app.world_mut().get_mut::<Body>(s.player).unwrap().heading = trace.heading;
    let player = s.player;
    trace.apply_start(s.app.world_mut(), player, start);
    // `AC_LAB_EXPORT=<folder>`: the recording's inputs as a bug folder's `input.txt` (the F9 format), for the windowed
    // replay (`AC_REPLAY`, `AC_VIDEO`) with `AC_NATIVE_MAP=damascus AC_CITY_AT=<the recording's start>`
    if let Ok(dir) = std::env::var("AC_LAB_EXPORT") {
        let mut txt = String::from("# ac_port recording v1 (exported from a lab recording)\n");
        txt.push_str(&format!("start {:?} {:?} {:?} {:?}\n", start.x, start.y, start.z, trace.heading));
        for f in &trace.frames {
            txt.push_str(&f.to_line());
            txt.push('\n');
        }
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(std::path::Path::new(&dir).join("input.txt"), txt).expect("write the exported input.txt");
        eprintln!("exported {} frames to {dir}; start game ({:.3} {:.3} {:.3})", trace.frames.len(), trace.start.x, trace.start.y, trace.start.z);
    }
    let mut out = String::from(crate::sim_tests::LAB_TRACE_HEADER);
    let mut t = 0.0f32;
    for (i, frame) in trace.frames.iter().enumerate() {
        s.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_nanos(frame.dt_nanos)));
        crate::sim_tests::apply_lab_frame(frame, &mut s.app.world_mut().resource_mut::<PadInput>());
        s.app.world_mut().resource_mut::<crate::camera::CameraRig>().yaw = frame.cam_yaw;
        s.app.update();
        if s.app.world().resource::<super::stream::City>().0.as_ref().is_some_and(|c| c.hold.is_some()) {
            eprintln!("REPLAY WARNING: the streaming hold froze the player at frame {i} (the replay no longer follows the recording)");
        }
        t += frame.dt_nanos as f32 * 1e-9;
        let clip = crate::sim_tests::lab_trace_clip(s.app.world(), s.player);
        out.push_str(&crate::sim_tests::lab_trace_row(i, t, s.loco().current, &clip, s.body().feet - start, s.body().velocity, s.app.world(), s.player));
    }
    let dst = std::path::Path::new(path).with_file_name("port_trace.csv");
    std::fs::write(&dst, out).expect("write port_trace.csv");
    eprintln!("{} frames from {:?} -> {}", trace.frames.len(), trace.start, dst.display());
}

#[test]
#[ignore = "manual Damascus CPU profile; requires the local game install"]
fn profile_damascus_souk_walk_and_freerun() {
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Souk") else { return };
    eprintln!("start {:?}: {:?}", s.body().feet, s.stats());
    for (name, high, legs, dir) in [("walk", false, false, Vec3::X), ("freerun", true, true, Vec3::NEG_Z), ("freerun back", true, true, Vec3::Z)] {
        s.pad(dir, 1.0, high, legs);
        if legs { s.press_legs(); }
        let mut times = Vec::new();
        for _ in 0..240 {
            let t = std::time::Instant::now();
            s.step();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            if ms > 5.0 { eprintln!("  slow frame {ms:.2} ms, streaming phases {:?}", s.stats().phase_ms); }
            times.push(ms);
        }
        times.sort_by(f64::total_cmp);
        eprintln!("{name}: CPU frame median {:.3} ms, p95 {:.3} ms, max {:.3} ms; {:?} at {:?}; {:?}", times[120], times[228], times[239], s.loco().current, s.body().feet, s.stats());
    }
}

impl CitySim {
    pub fn guidance(&self) -> &crate::guidance::GuidanceWorld { self.app.world().resource::<crate::guidance::GuidanceWorld>() }
    pub fn collision(&self) -> &CollisionWorld { self.app.world().resource::<CollisionWorld>() }
    pub fn data(&self) -> &crate::player::HumanDataBundle { self.app.world().get::<crate::player::HumanDataBundle>(self.player).unwrap() }
    pub fn place(&mut self, feet: Vec3, facing: Vec3) {
        let mut b = self.app.world_mut().get_mut::<Body>(self.player).unwrap();
        b.feet = feet; b.heading = crate::player::heading_of(facing); b.velocity = Vec3::ZERO; b.grounded = true; b.proxy.manifold.clear();
        // back on the ground: a hang or climb left from an earlier attempt would otherwise keep its holds
        let w = self.app.world_mut();
        let mut q = w.query::<(&mut crate::player::Locomotion, &mut crate::player::HumanDataBundle)>();
        let (mut loco, mut data) = q.get_mut(w, self.player).unwrap();
        if loco.current != ActorContextId::Ground {
            crate::player::switch_context(&mut loco, &mut data, crate::player::TransitionSetup::ToMovement { landing: None });
        }
    }
    /// Steps standing until the cells around the player have streamed in: no hold and nothing loading for half a second
    /// (a teleport across the city holds the player while the near box loads, RE/17 §1.1).
    /// `hold`: kept there meanwhile (a recording starting in mid-air must not fall during the wait).
    /// Steps until the streaming is settled around the player (or `hold`, where the player is kept), waiting up to
    /// `max_s` seconds of **wall-clock** time: the loader threads run in real time, and headless frames take
    /// milliseconds, so a frame count gives them almost no time when the CPU is busy.
    pub fn settle_streaming(&mut self, max_s: f32, hold: Option<Vec3>) {
        let mut quiet = 0;
        let started = std::time::Instant::now();
        while started.elapsed().as_secs_f32() < max_s {
            self.step();
            if let Some(p) = hold { self.teleport(p); }
            let feet = hold.unwrap_or(self.body().feet);
            let city = self.app.world().resource::<super::stream::City>();
            // every cell the planner wants around the player loaded, not only a quiet spell: under CPU load (the game
            // running beside a lab replay) a quiet 30 frames came before the last cells and the hold caught the replay
            let busy = city.0.as_ref().is_some_and(|c| c.hold.is_some() || c.stats.loading > 0 || !c.settled(feet));
            quiet = if busy { 0 } else { quiet + 1 };
            if quiet >= 30 { return; }
            if busy { std::thread::sleep(std::time::Duration::from_millis(5)); }
        }
        eprintln!("settle_streaming: not settled after {max_s} s");
    }
    pub fn run_until(&mut self, max_s: f32, pred: impl Fn(&CitySim) -> bool) -> bool {
        for _ in 0..(max_s * 60.0) as usize { self.step(); if pred(self) { return true; } }
        false
    }
}

const SPAWNS: [&str; 5] = ["PlayerSpawner_DEBUG_Souk", "PlayerSpawner_DEBUG_Bureau", "PlayerSpawner_DEBUG_Palace", "PlayerSpawner_DEBUG_Academy", "PlayerSpawner_Kingdom"];

#[test]
fn damascus_authored_ladder_takes_the_player_to_the_roof() {
    use crate::guidance::GuidanceSubType;
    let Some(mut s) = CitySim::new(SPAWNS[0]) else { return };
    let spawn = s.body().feet;
    let mut ladders: Vec<_> = s.guidance().edges.iter().filter(|e| e.subtype == GuidanceSubType::Ladder && (e.p1.y - e.p0.y).abs() > 3.0).cloned().collect();
    ladders.sort_by(|a, b| a.p0.distance(spawn).total_cmp(&b.p0.distance(spawn)));
    assert!(!ladders.is_empty(), "no authored ladders near the Souk");
    let mut tried = Vec::new();
    for edge in ladders.iter().take(6) {
        let (base, top) = if edge.p0.y < edge.p1.y { (edge.p0, edge.p1) } else { (edge.p1, edge.p0) };
        let n = Vec3::new(edge.n1.x, 0.0, edge.n1.z).normalize_or_zero();
        let mut feet = base + n * 0.8;
        let Some(y) = s.collision().floor_height_below(feet + Vec3::Y * 0.3, 1.0) else { tried.push(format!("{base}: no floor")); continue };
        feet.y = y;
        if !s.collision().capsule_fits(feet) { tried.push(format!("{base}: blocked")); continue; }
        s.place(feet, -n);
        s.pad(-n, 1.0, false, false);
        if !s.run_until(3.0, |s| s.loco().current == ActorContextId::Ladder) { tried.push(format!("{base}: no mount ({:?})", s.loco().current)); s.pad(Vec3::Z, 0.0, false, false); s.run(0.5); continue; }
        let mounted = s.run_until(40.0, |s| s.loco().current == ActorContextId::Ground);
        s.pad(-n, 0.0, false, false);
        s.run(1.0);
        let f = s.body().feet;
        assert!(mounted && s.loco().current == ActorContextId::Ground, "ladder at {base}: no exit, {:?} at {f}", s.loco().current);
        assert!(f.y > top.y - 1.5, "ladder at {base} (top {top}): ended at {f}");
        return;
    }
    panic!("no usable ladder: {tried:?}");
}

/// A roof edge 1.9–2.5 m above the street in front of its wall: the high-profile jump grabs it, the stick pulls up.
#[test]
fn damascus_climbs_a_street_wall_onto_the_roof() {
    use crate::guidance::GuidanceSubType;
    let Some(mut s) = CitySim::new(SPAWNS[0]) else { return };
    let spawn = s.body().feet;
    let c = s.collision();
    let mut spots = Vec::new();
    for e in s.guidance().edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab && e.n0.y > 0.9 && e.n1.y.abs() < 0.3 && e.p0.distance(e.p1) > 1.5) {
        let q = (e.p0 + e.p1) * 0.5;
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        let stand = q + n * 0.75;
        let Some(floor) = c.floor_height_below(Vec3::new(stand.x, q.y - 1.0, stand.z), 3.0) else { continue };
        let h = q.y - floor;
        if !(1.9..=2.5).contains(&h) { continue; }
        let feet = Vec3::new(stand.x, floor, stand.z);
        let roof = c.floor_height_below(q - n * 0.6 + Vec3::Y * 0.3, 0.6);
        if roof.is_none_or(|r| (r - q.y).abs() > 0.3) || !c.capsule_fits(feet) || !c.capsule_fits(q - n * 0.8 + Vec3::Y * 0.05) { continue; }
        spots.push((feet.distance(spawn), feet, n, q));
    }
    spots.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert!(!spots.is_empty(), "no climbable street wall near the Souk");
    let mut tried = Vec::new();
    for &(_, feet, n, q) in spots.iter().take(5) {
        s.place(feet, -n);
        s.run(0.2);
        s.pad(-n, 1.0, true, true);
        s.press_legs();
        let grabbed = s.run_until(3.0, |s| matches!(s.loco().current, ActorContextId::Ledge | ActorContextId::Climb));
        if !grabbed { tried.push(format!("{q}: no grab ({:?} at {})", s.loco().current, s.body().feet)); s.pad(n, 0.0, false, false); s.run(1.0); continue; }
        s.pad(-n, 1.0, false, false);
        // the climb-up ends in the free-step stay; let go there (pushing on over a narrow top jumps down its far side,
        // RE/05 §2.15), then the Ground or the beam the stay mounted
        let stay = s.run_until(8.0, |s| s.loco().current == ActorContextId::NarrowObject || s.loco().current == ActorContextId::Ground);
        s.pad(-n, 0.0, false, false);
        let up = stay && s.run_until(3.0, |s| s.loco().current == ActorContextId::Ground
            || (s.loco().current == ActorContextId::NarrowObject && s.data().narrow.kind != crate::player::narrow::NarrowKind::FreeStep));
        s.pad(-n, 0.0, false, false);
        s.run(0.5);
        let f = s.body().feet;
        assert!(up && f.y > q.y - 0.3, "edge {q}: did not end on the roof: {:?} at {f}", s.loco().current);
        return;
    }
    panic!("no wall climb succeeded: {tried:?}");
}

/// Leap of Faith from a city roof into one of the World's haystacks (`BhvHayStack`).
#[test]
fn damascus_leap_of_faith_lands_in_a_city_haystack() {
    use crate::guidance::GuidanceSubType;
    let mut tried = Vec::new();
    for spawn in SPAWNS {
        let Some(mut s) = CitySim::new(spawn) else { return };
        let stacks = s.guidance().haystacks.clone();
        let c = s.collision();
        let mut spots = Vec::new();
        for st in &stacks {
            let top = Vec3::new((st.bounds.min.x + st.bounds.max.x) * 0.5, st.bounds.max.y, (st.bounds.min.z + st.bounds.max.z) * 0.5);
            for e in s.guidance().edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab && e.n0.y > 0.9 && e.n1.y.abs() < 0.3) {
                let q = e.closest_point(Vec3::new(top.x, e.p0.y, top.z));
                let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
                let flat = Vec3::new(top.x - q.x, 0.0, top.z - q.z);
                if !(1.5..=6.0).contains(&flat.length()) || q.y - top.y < 4.0 || flat.normalize().dot(n) < 0.8 { continue; }
                let feet = q - n * 1.2;
                let Some(roof) = c.floor_height_below(feet + Vec3::Y * 0.3, 0.6) else { continue };
                let feet = Vec3::new(feet.x, roof, feet.z);
                if (roof - q.y).abs() > 0.3 || !c.capsule_fits(feet) { continue; }
                spots.push((feet, flat.normalize(), top));
            }
        }
        for &(feet, dir, top) in spots.iter().take(4) {
            s.place(feet, dir);
            s.run(0.2);
            s.pad(dir, 1.0, true, true);
            let jumped = s.run_until(3.0, |s| s.loco().current == ActorContextId::InAir);
            let target = s.data().air.target_flags;
            s.pad(dir, 0.0, false, false);
            let hay = jumped && s.run_until(6.0, |s| s.loco().current == ActorContextId::HayStack);
            if hay {
                let f = s.body().feet;
                assert!(Vec2::new(f.x - top.x, f.z - top.z).length() < 1.5, "inside the stack at {top}: {f}");
                return;
            }
            tried.push(format!("{spawn} {feet}->{top}: jumped {jumped} target {target:#x} ended {:?} at {}", s.loco().current, s.body().feet));
            s.run(2.0);
        }
        if spots.is_empty() { tried.push(format!("{spawn}: {} haystacks, no roof edge above one", stacks.len())); }
    }
    panic!("no Leap of Faith reached a haystack: {tried:#?}");
}

/// Walking from a roof onto one of the city's authored beams (guidance subtype Beam) mounts it and walks along it.
#[test]
fn damascus_walks_onto_a_city_beam() {
    use crate::guidance::GuidanceSubType;
    let mut tried = Vec::new();
    for spawn in SPAWNS {
        let Some(mut s) = CitySim::new(spawn) else { return };
        let c = s.collision();
        let mut spots = Vec::new();
        // beams are HumanGuidance's runtime report: paired LedgeGrab contacts (RE/05, beam_contacts::detect)
        let g = s.guidance();
        let mut beams: Vec<(Vec3, Vec3)> = Vec::new();
        for e in g.edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab && e.p0.distance(e.p1) > 2.5 && e.n0.y > 0.9) {
            for b in crate::guidance::beam_contacts::detect(g, c, (e.p0 + e.p1) * 0.5, Vec3::new(1.5, 0.6, 1.5)) {
                if b.p0.distance(b.p1) > 2.5 && (b.p1.y - b.p0.y).abs() < 0.3 && !beams.iter().any(|x| x.0.distance(b.p0) < 0.5) { beams.push((b.p0, b.p1)); }
            }
        }
        for &(p0, p1) in &beams {
            for (a, b) in [(p0, p1), (p1, p0)] {
                let dir = (b - a).with_y(0.0).normalize();
                let from = a - dir * 1.2;
                let Some(floor) = c.floor_height_below(from + Vec3::Y * 0.4, 0.8) else { continue };
                let feet = Vec3::new(from.x, floor, from.z);
                if (floor - a.y).abs() > 0.3 || !c.capsule_fits(feet) || !c.capsule_fits(a.lerp(b, 0.5) + Vec3::Y * 0.05) { continue; }
                if c.floor_height_below(a.lerp(b, 0.5) - Vec3::Y * 0.1, 1.5).is_some() { continue; } // a real gap under the beam
                spots.push((feet, dir, a, b));
            }
        }
        for &(feet, dir, a, b) in spots.iter().take(4) {
            s.place(feet, dir);
            s.run(0.2);
            s.pad(dir, 1.0, false, false);
            if s.run_until(3.0, |s| s.loco().current == ActorContextId::NarrowObject) {
                let mut trace = Vec::new();
                let walked = s.run_until(5.0, |s| (s.body().feet - a).dot(dir) > 1.0 || s.loco().current != ActorContextId::NarrowObject);
                for _ in 0..3 { trace.push(format!("{:?} {:?} {}", s.loco().current, s.data().narrow.state, s.body().feet)); s.run(0.3); }
                assert!(walked, "beam {a}->{b}: {trace:?}");
                let f = s.body().feet;
                let along = (f - a).dot(dir);
                assert!(along > 0.3 && (f.y - a.lerp(b, (along / a.distance(b)).clamp(0.0, 1.0)).y).abs() < 0.15, "beam {a}->{b}: at {f}");
                return;
            }
            tried.push(format!("{spawn} {a}->{b}: {:?} at {}", s.loco().current, s.body().feet));
            s.pad(dir, 0.0, false, false); s.run(1.0);
        }
        if spots.is_empty() { tried.push(format!("{spawn}: {} beams, none with a roof at its end", beams.len())); }
    }
    panic!("no beam mounted: {tried:#?}");
}

/// Free running across a street gap: from one roof edge to the facing roof edge 1.5–3.5 m away.
#[test]
fn damascus_free_run_jumps_a_gap_between_roofs() {
    use crate::guidance::GuidanceSubType;
    let mut tried = Vec::new();
    for spawn in SPAWNS {
        let Some(mut s) = CitySim::new(spawn) else { return };
        let c = s.collision();
        let roofs: Vec<_> = s.guidance().edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab && e.n0.y > 0.9 && e.n1.y.abs() < 0.3 && e.p0.distance(e.p1) > 2.0).cloned().collect();
        let mut spots = Vec::new();
        for a in &roofs {
            let na = Vec3::new(a.n1.x, 0.0, a.n1.z).normalize_or_zero();
            let qa = (a.p0 + a.p1) * 0.5;
            for b in &roofs {
                let nb = Vec3::new(b.n1.x, 0.0, b.n1.z).normalize_or_zero();
                if na.dot(nb) > -0.95 { continue; }
                let qb = b.closest_point(qa);
                let gap = (qb - qa).dot(na);
                if !(1.5..=3.5).contains(&gap) || (qb - qa - na * gap).with_y(0.0).length() > 0.5 || (qb.y - qa.y).abs() > 1.0 { continue; }
                let start = qa - na * 5.0;
                let Some(floor) = c.floor_height_below(start + Vec3::Y * 0.4, 0.8) else { continue };
                let feet = Vec3::new(start.x, floor, start.z);
                let path_ok = (1..10).all(|k| c.floor_height_below(qa - na * (k as f32 * 0.5) + Vec3::Y * 0.4, 0.8).is_some_and(|y| (y - qa.y).abs() < 0.3));
                let land = c.floor_height_below(qb - na * -1.0 + Vec3::Y * 0.4, 0.8).is_some_and(|y| (y - qb.y).abs() < 0.3);
                if (floor - qa.y).abs() > 0.3 || !path_ok || !land || !c.capsule_fits(feet) { continue; }
                spots.push((feet, na, qb));
            }
        }
        for &(feet, dir, far) in spots.iter().take(4) {
            s.place(feet, dir);
            s.run(0.2);
            s.pad(dir, 1.0, true, true);
            let jumped = s.run_until(4.0, |s| s.loco().current == ActorContextId::InAir);
            let crossed = jumped && s.run_until(4.0, |s| {
                let f = s.body().feet;
                (f - far).dot(dir) > -0.2 && matches!(s.loco().current, ActorContextId::Ground | ActorContextId::Ledge | ActorContextId::Climb)
            });
            if crossed { return; }
            tried.push(format!("{spawn} to {far}: jumped {jumped}, {:?} at {}", s.loco().current, s.body().feet));
            s.pad(dir, 0.0, false, false); s.run(2.0);
        }
        if spots.is_empty() { tried.push(format!("{spawn}: no facing roofs")); }
    }
    panic!("no gap jump crossed: {tried:#?}");
}

#[test]
#[ignore]
fn damascus_guidance_census() {
    let Some(s) = CitySim::new(SPAWNS[0]) else { return };
    let mut hist = std::collections::BTreeMap::<String, usize>::new();
    for e in &s.guidance().edges { *hist.entry(format!("{:?}", e.subtype)).or_default() += 1; }
    eprintln!("{hist:?} haystacks {}", s.guidance().haystacks.len());
    let c = s.collision();
    for e in s.guidance().edges.iter().filter(|e| e.subtype == crate::guidance::GuidanceSubType::Beam).take(15) {
        let mid = e.p0.lerp(e.p1, 0.5);
        let f = |p: Vec3| c.floor_height_below(p + Vec3::Y * 0.4, 3.0).map(|y| y - p.y);
        let dir = (e.p1 - e.p0).with_y(0.0).normalize_or_zero();
        eprintln!("beam {} -> {} len {:.2} n0 {} n1 {} floor below mid {:?} before p0 {:?} after p1 {:?}", e.p0, e.p1, e.p0.distance(e.p1), e.n0, e.n1,
            c.floor_height_below(mid - Vec3::Y * 0.05, 10.0).map(|y| y - mid.y), f(e.p0 - dir * 1.0), f(e.p1 + dir * 1.0));
    }
}

/// Free running into a tall facade: the wall run or the climb takes over, and holding up climbs the authored holds.
#[test]
fn damascus_runs_up_and_climbs_a_tall_facade() {
    use crate::guidance::GuidanceSubType;
    let Some(mut s) = CitySim::new(SPAWNS[0]) else { return };
    let spawn = s.body().feet;
    let c = s.collision();
    let mut spots = Vec::new();
    for e in s.guidance().edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab && e.n1.y.abs() < 0.3 && e.p0.distance(e.p1) > 1.0) {
        let q = (e.p0 + e.p1) * 0.5;
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        let stand = q + n * 4.0;
        let Some(floor) = c.floor_height_below(Vec3::new(stand.x, q.y - 1.5, stand.z), 4.0) else { continue };
        let h = q.y - floor;
        if !(2.6..=4.0).contains(&h) { continue; }
        let feet = Vec3::new(stand.x, floor, stand.z);
        // a clear run-up on level ground and a wall reaching well above the hold
        let level = (0..8).all(|k| c.floor_height_below(Vec3::new(stand.x, floor + 0.4, stand.z) - n * (k as f32 * 0.45), 0.8).is_some_and(|y| (y - floor).abs() < 0.25));
        let tall = c.ray_distance(q - n * 0.0 + n * 0.3 + Vec3::Y * 2.0, -n, 1.0, crate::layers::MAIN_CHARACTER) < 1.0;
        if !level || !tall || !c.capsule_fits(feet) { continue; }
        spots.push((feet.distance(spawn), feet, n, q));
    }
    spots.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert!(!spots.is_empty(), "no tall facade near the Souk");
    let mut tried = Vec::new();
    for &(_, feet, n, q) in spots.iter().take(6) {
        s.place(feet, -n);
        s.run(0.2);
        s.pad(-n, 1.0, true, true);
        s.press_legs();
        let start_y = feet.y;
        let on_wall = s.run_until(4.0, |s| matches!(s.loco().current, ActorContextId::Walling | ActorContextId::Climb | ActorContextId::Ledge));
        if on_wall && s.run_until(6.0, |s| s.body().feet.y > start_y + 2.0) {
            return;
        }
        tried.push(format!("{q}: {:?} at {} ledge {} hands {} {} walling {:?}", s.loco().current, s.body().feet, s.data().ledge.last_action, s.data().ledge.hand_l, s.data().ledge.hand_r, s.data().walling.sub_state));
        s.pad(n, 0.0, false, false); s.run(2.0);
    }
    panic!("no facade climbed: {tried:#?}");
}

/// Touring the districts: cells load and unload around the player, physics slots are reused rather than piling up,
/// and the player always has native ground underfoot after the cells around them arrive.
#[test]
fn damascus_streaming_tour_loads_unloads_and_reuses_slots() {
    let Some(mut s) = CitySim::new(SPAWNS[0]) else { return };
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let stops: Vec<(Vec3, f32)> = SPAWNS[1..].iter().chain([SPAWNS[0]].iter()).filter_map(|n| super::spawner(&w.globals, n)).collect();
    let mut peak_slots = 0;
    let mut peak_cells = 0;
    let mut peak_fading = 0;
    let first_slots = s.collision().triangles.len();
    for round in 0..2 {
        for &(p, _) in &stops {
            s.teleport(p + Vec3::Y * 0.05);
            // the loader threads run on the wall clock, the sim on its own: wait in real time
            let started = std::time::Instant::now();
            let mut settled = false;
            while started.elapsed().as_secs() < 60 {
                s.step();
                if s.app.world().resource::<super::stream::City>().0.as_ref().unwrap().settled(p) { settled = true; break; }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(settled, "cells around {p} never finished loading: {:?}", s.stats());
            s.run(1.0);
            let st = s.stats();
            assert!(st.errors == 0, "{st:?}");
            assert!(s.collision().support(s.body().feet + Vec3::Y * 0.3).is_some() || s.loco().current != ActorContextId::Ground, "no ground at {p}: {:?}", s.body().feet);
            peak_slots = peak_slots.max(s.collision().triangles.len());
            peak_cells = peak_cells.max(st.loaded);
            peak_fading = peak_fading.max(st.fading);
            // every loaded cell is near the player
            let city = s.app.world().resource::<super::stream::City>();
            let state = city.0.as_ref().unwrap();
            // exactly the cells the exe's box rule wants (0x55D810), nothing else
            let wanted = state.wanted(p, None);
            assert_eq!(wanted.len(), st.loaded, "loaded cells differ from the wanted box at {p}: {st:?}");
            assert!(wanted.iter().all(|&c| state.is_loaded(c)));
            eprintln!("round {round} at {p}: {st:?}, {} triangle slots", s.collision().triangles.len());
        }
    }
    // slot storage is bounded by the largest window, not the sum of every window visited
    assert!(peak_slots < first_slots * 4 + 400_000, "triangle slots grew to {peak_slots} (first window {first_slots})");
    assert!(peak_cells < 260, "{peak_cells} cells loaded at once");
    // the retail exe never draws the LOD cross-fade (RE/17 §3.2)
    assert_eq!(peak_fading, 0);
}

#[test]
fn damascus_lod_cross_fade_draws_the_next_lod_on_top() {
    // AC_LOD_FADE: objects inside a fade width draw their next LOD on top (0xA90160); leaving the width removes it
    let Some(mut s) = CitySim::new(SPAWNS[0]) else { return };
    s.app.world_mut().resource_mut::<super::stream::City>().0.as_mut().unwrap().lod_fade = true;
    s.run(1.0);
    let st = s.stats();
    assert!(st.errors == 0 && st.fading > 0 && st.fading <= st.drawn, "{st:?}");
    s.app.world_mut().resource_mut::<super::stream::City>().0.as_mut().unwrap().lod_fade = false;
    s.run(0.5);
    assert_eq!(s.stats().fading, 0);
}

/// Debug: the guidance subtypes the city's cells carry around each spawn (RE/24 LEDGE-17: are the swing bars Pole
/// edges, subtype 4?). `cargo test probe_damascus_guidance_subtypes -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_damascus_guidance_subtypes() {
    use crate::guidance::GuidanceSubType;
    for spawn in SPAWNS {
        let Some(s) = CitySim::new(spawn) else { return };
        let g = s.guidance();
        let mut counts = std::collections::BTreeMap::new();
        for e in g.edges.iter().filter(|e| e.subtype != GuidanceSubType::None) {
            *counts.entry(format!("{:?}", e.subtype)).or_insert(0usize) += 1;
        }
        let poles: Vec<_> = g.edges.iter().filter(|e| e.subtype == GuidanceSubType::Pole).take(8).map(|e| (e.p0, e.p1, e.n0, e.n1)).collect();
        eprintln!("{spawn}: {counts:?}");
        for p in poles {
            eprintln!("  pole {:.2?} -> {:.2?} n0 {:.2?} n1 {:.2?} len {:.2} dy {:.2}", p.0, p.1, p.2, p.3, p.0.distance(p.1), (p.1.y - p.0.y).abs());
        }
    }
}

/// Jump-target probe (game coordinates, Z up): the candidate query and the scorer at `LAB_POS` along `LAB_DIR` with
/// `LAB_KIND` (default 0) and beams on, as the free-step stay asks (RE/05 §2.15); then the guidance edges within 1.5 m
/// of `LAB_REF` (e.g. where the game landed).
#[test]
#[ignore = "live-analysis jump-target probe; requires the local game install"]
fn probe_jump_candidates_at() {
    use crate::player::jump_candidates::{query, select, Query};
    let v3 = |k: &str| std::env::var(k).ok().map(|s| { let v: Vec<f32> = s.split(',').map(|x| x.trim().parse().unwrap()).collect(); Vec3::new(v[0], v[2], -v[1]) });
    let (Some(pos), Some(dir)) = (v3("LAB_POS"), v3("LAB_DIR")) else { return };
    let kind: u32 = std::env::var("LAB_KIND").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let spawn = std::env::var("LAB_SPAWN").map(|s| format!("PlayerSpawner_DEBUG_{s}")).unwrap_or("PlayerSpawner_DEBUG_Bureau".into());
    let Some(mut s) = CitySim::new(&spawn) else { return };
    s.place(pos, dir);
    // the cells around the probe point streamed in first (a far probe point met no collision otherwise)
    s.settle_streaming(60.0, Some(pos));
    s.place(pos, dir);
    let game = |p: Vec3| Vec3::new(p.x, -p.z, p.y);
    // `LAB_EDGE=<n>`: the foot-level edge reports (0.75 m, drops ≥ 0.45 m) at `n` points from LAB_POS along LAB_DIR,
    // 2 cm apart, then across it (the ledge stop's report, A19)
    if let Some(n) = std::env::var("LAB_EDGE").ok().and_then(|v| v.parse::<usize>().ok()) {
        use crate::player::jump_candidates::edge_reports;
        let across = Vec3::new(-dir.z, 0.0, dir.x).normalize_or_zero();
        for (axis, step) in [("along", dir.normalize_or_zero()), ("across", across)] {
            for i in 0..n {
                let p = pos + step * (0.02 * i as f32);
                for r in edge_reports(p, 0.75, 0.45, None, s.guidance(), s.collision()) {
                    let room = crate::player::narrow::pull_down_clear(p, r.point, r.normal, s.collision());
                    eprintln!("{axis} {i}: feet {:.3} report {:.3} n {:.2} dist {:.3} drop {:.3} edge {} hang room {room}", game(p), game(r.point), game(r.normal), r.dist, r.drop, r.edge);
                    if !room && i == n / 2 {
                        // what fills the two boxes of `sub_B2E4F0` (as `narrow::pull_down_clear` places them)
                        let right = crate::player::right_of(r.normal);
                        let rot = Quat::from_axis_angle(right, 30f32.to_radians());
                        let boxes = [
                            ((r.point + r.normal * 0.25).with_y(p.y + 1.075), [right, Vec3::Y, r.normal], Vec3::new(0.3, 0.725, 0.75)),
                            (r.point + rot * r.normal * 0.4 - rot * Vec3::Y * 0.4, [right, rot * Vec3::Y, rot * r.normal], Vec3::new(0.3, 0.8, 0.3)),
                        ];
                        for (k, (c, axes, half)) in boxes.into_iter().enumerate() {
                            for t in s.collision().triangles.iter().filter(|t| t.intersects_box(c, axes, half)) {
                                eprintln!("  box {k}: triangle layer {} material {} normal {:.2} at {:.2?}", t.layer, t.material, game(t.normal), t.vertices.map(game));
                            }
                        }
                    }
                }
            }
        }
    }
    let q = Query { kind, mode: 0, long: false, beams: true, exclude: 0 };
    let cands = query(pos, dir, &q, s.guidance(), s.collision());
    for (i, c) in cands.iter().enumerate() {
        eprintln!("cand {i}: pos {:.3} ty {} sub {} flags {:#x} wall {:.2} dz {:.2} d {:.2}", game(c.pos), c.ty, c.sub, c.flags, game(c.wall), c.pos.y - pos.y, Vec2::new(c.pos.x - pos.x, c.pos.z - pos.z).length());
    }
    if let Some(r) = v3("LAB_DOWN") {
        let c = s.collision();
        for layer in [crate::layers::MAIN_CHARACTER, crate::layers::CHARACTER, crate::layers::ONLY_STATIC] {
            let d = c.ray_distance(r, Vec3::NEG_Y, 3.05, layer);
            eprintln!("down ray at {:.3} layer {layer}: {d:.3}", game(r));
        }
        if let Some((d, n, l)) = c.ray_hit_layer(r, Vec3::NEG_Y, 3.05, crate::layers::MAIN_CHARACTER) { eprintln!("hit at {d:.3} normal {:.2} layer {l}", game(n)); }
    }
    eprintln!("select mode 1: {:?}", select(&cands, pos, dir, dir, 1, false));
    eprintln!("select mode 2: {:?}", select(&cands, pos, dir, dir, 2, false));
    if let Some(r) = v3("LAB_REF") {
        for e in s.guidance().edges.iter() {
            let q = crate::guidance::closest_on_segment(e.p0, e.p1, r);
            if q.distance(r) < 1.5 {
                eprintln!("edge {:?} {:.3} -> {:.3} n0 {:.2} n1 {:.2} dist {:.2}", e.subtype, game(e.p0), game(e.p1), game(e.n0), game(e.n1), q.distance(r));
            }
        }
    }
}

/// Climb survey (map support): up to `LAB_N` (default 40) facades near the `LAB_SPAWN` district (default the Souk)
/// with a climbable column of ledge holds: a first hold 1.5–4 m above the street, the column's top at least 1.5 m
/// higher. Each is run up with high profile and Legs held, then climbed by holding up; prints how far it got, where it
/// stopped and in which context, and a summary. Game coordinates (Z up) in the output.
#[test]
#[ignore = "map support survey; requires the local game install"]
fn probe_city_climb_survey() {
    use crate::guidance::GuidanceSubType;
    let spawn_name = std::env::var("LAB_SPAWN").map(|s| format!("PlayerSpawner_DEBUG_{s}")).unwrap_or(SPAWNS[0].into());
    let n_max: usize = std::env::var("LAB_N").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let Some(mut s) = CitySim::new(&spawn_name) else { return };
    let game = |p: Vec3| Vec3::new(p.x, -p.z, p.y);
    let spawn = s.body().feet;
    let c = s.collision();
    let g = s.guidance();
    let ledges: Vec<_> = g.edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab && e.n1.y.abs() < 0.3 && e.p0.distance(e.p1) > 0.6).collect();
    let mut spots = Vec::new();
    for e in &ledges {
        let q = (e.p0 + e.p1) * 0.5;
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        if n == Vec3::ZERO { continue; }
        let stand = q + n * 1.5;
        let Some(floor) = c.floor_height_below(Vec3::new(stand.x, q.y - 1.0, stand.z), 5.0) else { continue };
        let h = q.y - floor;
        if !(1.5..=4.0).contains(&h) { continue; }
        // the lowest hold of its column: nothing below it on the same wall within 0.6 m sideways
        let column = |lo: f32| ledges.iter().filter(|o| {
            let m = (o.p0 + o.p1) * 0.5;
            let on = Vec3::new(o.n1.x, 0.0, o.n1.z).normalize_or_zero().dot(n) > 0.9 && ((m - q) - n * (m - q).dot(n)).with_y(0.0).length() < 0.6;
            on && m.y > lo
        }).map(|o| (o.p0.y + o.p1.y) * 0.5).collect::<Vec<_>>();
        if !column(floor + 0.3).iter().all(|&y| y >= q.y - 0.05) { continue; }
        let mut holds = column(q.y - 0.05);
        holds.sort_by(f32::total_cmp);
        holds.dedup_by(|a, b| (*a - *b).abs() < 0.1);
        let top = holds.last().copied().unwrap_or(q.y);
        let max_gap = holds.windows(2).map(|w| w[1] - w[0]).fold(0.0f32, f32::max);
        let gap_limit: f32 = std::env::var("LAB_GAP").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
        if top - q.y < 1.5 || max_gap > gap_limit { continue; }
        let feet = Vec3::new(stand.x, floor, stand.z);
        if !c.capsule_fits(feet) { continue; }
        // one spot per 3 m
        spots.push((feet.distance(spawn), feet, n, q, top, holds.iter().map(|h| h - floor).collect::<Vec<_>>()));
    }
    spots.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut picked: Vec<(f32, Vec3, Vec3, Vec3, f32, Vec<f32>)> = Vec::new();
    for sp in spots { if picked.iter().all(|p| p.1.distance(sp.1) > 3.0) { picked.push(sp); } if picked.len() >= n_max { break; } }
    if let Some(only) = std::env::var("LAB_ONLY").ok().and_then(|v| { let v: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect(); (v.len() == 2).then(|| Vec2::new(v[0], v[1])) }) {
        picked.retain(|p| Vec2::new(game(p.3).x, game(p.3).y).distance(only) < 0.5);
    }
    let trace = std::env::var_os("LAB_TRACE_CLIMB").is_some();
    eprintln!("{} facades", picked.len());
    // the spots as lab courses: feet and the yaw facing the wall (game coordinates, yaw 0 = +x)
    for (_, feet, n, q, _, holds) in &picked {
        let (f, w) = (game(*feet), game(-*n));
        eprintln!("COURSE {:.2} {:.2} {:.2} {:.1} wall {:.2} {:.2} holds {:.1?}", f.x, f.y, f.z, w.y.atan2(w.x).to_degrees(), game(*q).x, game(*q).y, holds);
    }
    if std::env::var_os("LAB_LIST_ONLY").is_some() { return; }
    let mut summary = std::collections::BTreeMap::<String, usize>::new();
    for (_, feet, n, q, top, holds) in picked {
        s.place(feet, -n);
        s.run(0.3);
        s.pad(-n, 1.0, true, true);
        s.press_legs();
        let mut best = feet.y;
        let mut seen = std::collections::BTreeSet::new();
        let mut on_wall = false;
        for k in 0..(14.0 * 60.0) as usize {
            // on the wall, climb with high profile and the stick (Legs would jump off); not on it after 3 s: walk to
            // it and climb from the ground with the empty-hand press (event 50)
            if !on_wall && matches!(s.loco().current, ActorContextId::Ledge | ActorContextId::Climb) { on_wall = true; s.pad(-n, 1.0, true, false); }
            if !on_wall && k == 180 { s.pad(-n, 1.0, true, false); }
            if !on_wall && k > 180 && k % 30 == 0 { s.app.world_mut().resource_mut::<PadInput>().hand_pressed_ago = 0.0; }
            s.step();
            if trace && k % 6 == 0 { let d = s.data(); eprintln!("  t {:.2} {:?} feet {:.2} ledge {:?} {:?} {:?} climb {:?} hands {:.2} {:.2}", k as f32 / 60.0, s.loco().current, game(s.body().feet), d.ledge.sub_state, d.ledge.hang_type, d.ledge.last_action, d.climb.last_action, game(d.ledge.hand_l), game(d.ledge.hand_r)); }
            best = best.max(s.body().feet.y);
            seen.insert(format!("{:?}", s.loco().current));
            if s.body().feet.y > top - 0.4 && matches!(s.loco().current, ActorContextId::Ground | ActorContextId::NarrowObject) { break; }
        }
        if trace {
            let d = s.data();
            let mid = (d.ledge.hand_l + d.ledge.hand_r) * 0.5;
            let c = s.collision();
            for dy in [0.1f32, 0.3, 0.5, 0.8, 1.1, 1.5, 2.0, 2.5] {
                let o = mid + n * 0.8 - Vec3::Y * dy;
                eprintln!("  wall ray {dy:.1} below the hands: {:.2} (from 0.8 out)", c.ray_distance(o, -n, 2.5, crate::layers::MAIN_CHARACTER));
            }
        }
        let f = s.body().feet;
        let ok = best > top - 0.4;
        let tag = if ok { "top".to_string() } else if seen.contains("Climb") || seen.contains("Ledge") { format!("stuck in {:?}", s.loco().current) } else { format!("never grabbed ({:?})", s.loco().current) };
        *summary.entry(tag.clone()).or_default() += 1;
        eprintln!("SURVEY {tag:28} wall {:.2} holds {:.1?} reached +{:.1} end {:.2} contexts {:?} last {:?}",
            game(q), holds, best - feet.y, game(f), seen, s.data().climb.last_action);
        s.pad(n, 0.0, false, false);
        s.run(0.5);
    }
    eprintln!("SUMMARY {summary:?}");
}


/// RE/27 S8: the city's long beams are chains of ≈ 3 m pieces in HumanGuidance's runtime report. Walking one goes on
/// from piece to piece (`DetectBeamSegments` 0xF753A0's next segment) and stops once at the chain's end; before, it
/// stepped off into the Ground at every piece's end, and at a short cap piece over an end it started and stopped
/// every few frames.
#[test]
fn damascus_beam_chains_walk_through_without_stopping() {
    use crate::guidance::GuidanceSubType;
    let mut walked = 0;
    for spawn in ["PlayerSpawner_DEBUG_Palace", "PlayerSpawner_DEBUG_Academy"] {
        let Some(mut s) = CitySim::new(spawn) else { return };
        let (c, g) = (s.collision(), s.guidance());
        let mut beams: Vec<(Vec3, Vec3)> = Vec::new();
        for e in g.edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab && e.p0.distance(e.p1) > 2.5 && e.n0.y > 0.9) {
            for b in crate::guidance::beam_contacts::detect(g, c, (e.p0 + e.p1) * 0.5, Vec3::new(1.5, 0.6, 1.5)) {
                if b.p0.distance(b.p1) > 2.5 && (b.p1.y - b.p0.y).abs() < 0.3 && !beams.iter().any(|x| x.0.distance(b.p0) < 0.5) { beams.push((b.p0, b.p1)); }
            }
        }
        let mut spots = Vec::new();
        for &(p0, p1) in &beams {
            for (a, b) in [(p0, p1), (p1, p0)] {
                let dir = (b - a).with_y(0.0).normalize();
                let from = a - dir * 1.2;
                let Some(floor) = c.floor_height_below(from + Vec3::Y * 0.4, 0.8) else { continue };
                let feet = Vec3::new(from.x, floor, from.z);
                if (floor - a.y).abs() > 0.3 || !c.capsule_fits(feet) || !c.capsule_fits(a.lerp(b, 0.5) + Vec3::Y * 0.05) { continue; }
                spots.push((feet, dir));
            }
        }
        for &(feet, dir) in spots.iter().take(6) {
            s.place(feet, dir);
            s.run(0.2);
            s.pad(dir, 1.0, false, false);
            if !s.run_until(3.0, |s| s.loco().current == ActorContextId::NarrowObject) { s.pad(dir, 0.0, false, false); s.run(1.0); continue; }
            walked += 1;
            let start = s.body().feet;
            let (mut starts, mut last) = (0, None);
            for _ in 0..300 {
                s.run(1.0 / 60.0);
                if s.loco().current != ActorContextId::NarrowObject { break; }
                let st = s.data().narrow.state;
                if st == crate::player::narrow::BeamState::Start && last != Some(st) { starts += 1; }
                // the animation follows: the beam walk plays (not the ground run) while the beam's Walk state runs
                if st == crate::player::narrow::BeamState::Walk && last == Some(st) {
                    if let Some(clip) = s.app.world().get::<crate::anim::AnimPlayer>(s.player).and_then(|p| p.clip.clone()) {
                        assert_eq!(clip, format!("act_{:08x}", crate::player::narrow::BEAM_WALK), "{spawn}: the walk plays {clip}");
                    }
                }
                last = Some(st);
            }
            let along = (s.body().feet - start).dot(dir);
            assert!(starts <= 1, "{spawn} from {start}: the walk started {starts} times");
            assert!(along > 2.5, "{spawn} from {start}: only {along:.2} m along before {:?} {:?}", s.loco().current, s.data().narrow.state);
            s.pad(dir, 0.0, false, false); s.run(1.0);
        }
    }
    assert!(walked >= 6, "only {walked} beams mounted");
}

/// RE/28 §1 (live 2026-10-13_210349/006): hanging beside the Bureau courtyard beam (0.25 m wide), the stick up climbs
/// back onto it: `HumanLedge__CanPullup` 0xDE2270 asks for room on top (`Human__KneeCatchRoomOnTop`), not a floor 0.5 m
/// in, and the stand-up's free-step stay mounts the beam (the game: hangwaist → hangknee → `freestep_entry` →
/// NarrowObject, on the beam's line).
#[test]
fn damascus_pulls_up_onto_a_beam_from_a_hang() {
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    // the roof south of the courtyard beam (game y 4.0 = port z −4.0), walking along it onto the beam (port +z)
    s.place(Vec3::new(51.97, 11.48, -4.0), Vec3::Z);
    s.run(0.5);
    s.pad(Vec3::Z, 0.6, false, false);
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::NarrowObject), "never mounted the beam");
    s.run(1.2);
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.run(1.0);
    // across the beam with the empty hand: the pull-down to a hang
    s.pad(Vec3::X, 1.0, false, false);
    s.run(0.3);
    s.app.world_mut().resource_mut::<crate::input::PadInput>().hand_held = true;
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ledge), "no pull-down: {:?}", s.loco().current);
    s.app.world_mut().resource_mut::<crate::input::PadInput>().hand_held = false;
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.run(2.0);
    assert_eq!(s.loco().current, ActorContextId::Ledge, "hanging");
    // the stick up (toward the facing: the sim's camera follows the heading)
    let f = Vec3::new(-s.body().heading.sin(), 0.0, -s.body().heading.cos());
    s.pad(f, 1.0, false, false);
    assert!(s.run_until(5.0, |s| s.loco().current == ActorContextId::NarrowObject && s.data().narrow.kind == crate::player::narrow::NarrowKind::Beam),
        "never back on the beam: {:?} {:?} {}", s.loco().current, s.data().ledge.last_action, s.body().feet);
    let feet = s.body().feet;
    assert!((feet.x - 51.97).abs() < 0.15 && (feet.y - 11.17).abs() < 0.05, "on the beam's line: {feet}");
}

/// RE/28 §7 (live 2026-10-13_210349/006): hanging free from the Bureau courtyard beam, the stick back (more than 135°
/// from the facing) turns round under the beam onto its other side (Ledge event 10, `xx_h_hangfree_turn_beam_180`), the
/// hands on the far edge, still in a free hang.
#[test]
fn damascus_hanging_from_a_beam_the_stick_back_turns_under_it() {
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    s.place(Vec3::new(51.97, 11.48, -4.0), Vec3::Z);
    s.run(0.5);
    s.pad(Vec3::Z, 0.6, false, false);
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::NarrowObject), "never mounted the beam");
    s.run(1.2);
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.run(1.0);
    s.pad(Vec3::X, 1.0, false, false);
    s.run(0.3);
    s.app.world_mut().resource_mut::<crate::input::PadInput>().hand_held = true;
    assert!(s.run_until(2.0, |s| s.loco().current == ActorContextId::Ledge), "no pull-down: {:?}", s.loco().current);
    s.app.world_mut().resource_mut::<crate::input::PadInput>().hand_held = false;
    s.pad(Vec3::ZERO, 0.0, false, false);
    s.run(2.0);
    assert_eq!(s.loco().current, ActorContextId::Ledge, "hanging");
    let f = Vec3::new(-s.body().heading.sin(), 0.0, -s.body().heading.cos());
    let side = (s.body().feet.x - 51.97).signum();
    // the stick back, away from the beam
    s.pad(-f, 1.0, false, false);
    assert!(s.run_until(1.0, |s| s.data().ledge.last_action == "turn under beam"), "no turn: {:?}", s.data().ledge.last_action);
    s.pad(Vec3::ZERO, 0.0, false, false);
    // the facing turns with the clip, no frame jumps through the half turn
    let mut h = s.body().heading;
    let mut worst = 0.0f32;
    for _ in 0..180 {
        s.step();
        let mut dh = s.body().heading - h;
        dh = (dh + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        worst = worst.max(dh.abs());
        h = s.body().heading;
    }
    assert!(worst < 0.25, "the heading jumps {worst} rad in one frame");
    assert_eq!(s.loco().current, ActorContextId::Ledge, "still hanging");
    let g = Vec3::new(-s.body().heading.sin(), 0.0, -s.body().heading.cos());
    assert!(g.dot(f) < -0.95, "turned round: {f} → {g}");
    assert_eq!(s.data().ledge.hang_type, crate::player::ledge::LedgeHangType::Free);
    let feet = s.body().feet;
    assert!((feet.x - 51.97).signum() == -side && (feet.x - 51.97).abs() > 0.1, "on the beam's other side: {feet}");
}

/// RE/28 §2 (live 2026-10-12_123944/014): standing on the Bureau courtyard beam in the Ground, the stick across it
/// mounts the beam in the frame it moves (the interpreter asks event 72's guard with the stick, before the edge halt
/// and the pivot); the port had stayed in the Ground, turning on the spot.
#[test]
fn damascus_standing_on_a_beam_the_stick_mounts_it() {
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    s.place(Vec3::new(51.97, 11.17, 1.0), Vec3::Z);
    s.run(0.5);
    assert_eq!(s.loco().current, ActorContextId::Ground);
    // the game's stick in 014: back and to the left of the facing (game md (−0.59, 0.81))
    s.pad(Vec3::new(-0.588, 0.0, -0.81), 1.0, false, false);
    assert!(s.run_until(0.2, |s| s.loco().current == ActorContextId::NarrowObject), "never mounted: {:?} {}", s.loco().current, s.body().feet);
}

/// RE/28 §5 (live 2026-10-13_210349/001): walking off the Bureau roof lip onto the courtyard beam 0.41 m lower, the root
/// stays on the lip until the capsule's rim leaves it (the game: 0.29 m onto the beam, game y 2.54 = port z −2.54) and
/// then drops in one frame, handing the drop to the stick-to-ground smoothing as the controller's residual.
#[test]
fn damascus_stepping_down_onto_a_beam_drops_where_the_game_does() {
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    s.place(Vec3::new(51.97, 11.48, -4.0), Vec3::Z);
    s.run(0.5);
    s.pad(Vec3::Z, 0.6, false, false);
    let mut drop = None;
    let mut last = s.body().feet;
    for _ in 0..300 {
        s.step();
        let f = s.body().feet;
        if s.loco().current == ActorContextId::NarrowObject && last.y - f.y > 0.25 {
            drop = Some((f.z, s.body().stick_residual, last.y - f.y));
            break;
        }
        last = f;
    }
    let (z, residual, snap) = drop.expect("never dropped onto the beam");
    assert!((z - -2.54).abs() < 0.12, "drops at z {z}, the game at −2.54");
    // the capsule's rounded bottom lowers the root a little over the rim first (the game: 0.08 m, then a 0.33 m snap)
    assert!((residual - snap).abs() < 1e-4 && (0.28..0.38).contains(&snap), "the snap ({snap}) is the smoothing's residual: {residual}");
}

/// Timing probe: the runtime beam report on the Bureau beam and at the slowest beam spot streamed in around it, then
/// whole frames standing on that beam.
#[test]
#[ignore]
fn probe_damascus_beam_frame_cost() {
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    s.place(Vec3::new(51.97, 11.48, -4.0), Vec3::Z);
    s.run(0.5);
    s.pad(Vec3::Z, 0.6, false, false);
    assert!(s.run_until(4.0, |s| s.loco().current == ActorContextId::NarrowObject), "never mounted the beam");
    s.run(1.0);
    let worst_beam;
    {
        let g = s.app.world().resource::<crate::guidance::GuidanceWorld>();
        let c = s.app.world().resource::<crate::collision::CollisionWorld>();
        let root = s.body().feet;
        let near = g.edges.iter().filter(|e| (e.p0 - root).abs().cmple(Vec3::new(8.0, 13.0, 8.0)).all()).count();
        let t = std::time::Instant::now();
        let mut n = 0;
        for _ in 0..10 { n = crate::guidance::beam_contacts::detect(g, c, root - Vec3::Y * 4.75, Vec3::new(7.0, 7.25, 7.0)).len(); }
        eprintln!("DCOST edges {} near {near} tris {} index {} detect {:.2} ms ({n} beams)", g.edges.len(), c.triangles.len(), c.index.is_some(), t.elapsed().as_secs_f64() * 100.0);
        // the worst spot among beam midpoints: the slowest detect
        let mut spots: Vec<Vec3> = Vec::new();
        for e in g.edges.iter().filter(|e| e.subtype == crate::guidance::GuidanceSubType::LedgeGrab).step_by(5) {
            for b in crate::guidance::beam_contacts::detect(g, c, (e.p0 + e.p1) * 0.5, Vec3::new(1.0, 0.5, 1.0)) {
                let m = (b.p0 + b.p1) * 0.5;
                if spots.iter().all(|q| q.distance(m) > 4.0) { spots.push(m); }
            }
        }
        let mut worst = (0.0f64, Vec3::ZERO, 0usize);
        for m in &spots {
            let t = std::time::Instant::now();
            let n = crate::guidance::beam_contacts::detect(g, c, *m - Vec3::Y * 4.75, Vec3::new(7.0, 7.25, 7.0)).len();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            if ms > worst.0 { worst = (ms, *m, n); }
        }
        {
            let m = worst.1;
            let (lo, hi) = (m - Vec3::new(9.0, 14.0, 9.0), m + Vec3::new(9.0, 4.5, 9.0));
            let t = std::time::Instant::now();
            let tris = c.triangles_overlapping(lo, hi).count();
            eprintln!("DCOST worst box: {tris} triangles ({:.2} ms), {} boxes in world", t.elapsed().as_secs_f64() * 1000.0, c.boxes.len());
            let edges: Vec<_> = g.edges.iter().filter(|e| e.subtype == crate::guidance::GuidanceSubType::LedgeGrab
                && e.p0.min(e.p1).cmple(hi).all() && e.p0.max(e.p1).cmpge(lo).all()).cloned().collect();
            let empty = crate::collision::CollisionWorld::default();
            let t = std::time::Instant::now();
            let n = crate::guidance::beam_contacts::pair_report(edges.clone(), &empty).len();
            eprintln!("DCOST worst: {} edges, pairing alone {:.2} ms ({n} untrimmed)", edges.len(), t.elapsed().as_secs_f64() * 1000.0);
        }
        worst_beam = crate::guidance::beam_contacts::detect(g, c, worst.1, Vec3::new(1.0, 0.5, 1.0)).into_iter().next();
        eprintln!("DCOST {} beam spots; worst detect {:.2} ms at {:?} ({} beams)", spots.len(), worst.0, worst.1, worst.2);
    }
    if let Some(b) = worst_beam {
        let point = b.p0.lerp(b.p1, 0.5);
        let facing = (b.p1 - b.p0).with_y(0.0).normalize();
        s.place(point, facing);
        let w = s.app.world_mut();
        let mut q = w.query::<(&mut crate::player::Locomotion, &mut crate::player::HumanDataBundle)>();
        let (mut loco, mut data) = q.get_mut(w, s.player).unwrap();
        crate::player::switch_context(&mut loco, &mut data, crate::player::TransitionSetup::ToBeam(crate::player::narrow::BeamEntry { p0: b.p0, p1: b.p1, point, from: point,
            toward_p1: true, mode: crate::player::narrow::BeamEntryMode::Straight, foot: 0, action: None, facing }));
        s.run(0.5);
        eprintln!("DCOST on the worst beam: {:?}", s.loco().current);
    }
    for (label, dir, stick, high) in [("idle", Vec3::Z, 0.0, false), ("idle again", Vec3::Z, 0.0, false), ("stick high", Vec3::Z, 1.0, true)] {
        s.pad(dir, stick, high, false);
        let t = std::time::Instant::now();
        for _ in 0..30 { s.app.update(); }
        eprintln!("DCOST frame {label}: {:.2} ms/frame, {:?} {:?} feet {:?}", t.elapsed().as_secs_f64() * 1000.0 / 30.0, s.loco().current, s.data().narrow.state, s.body().feet);
    }
}

/// Bug report 2026-10-16: running over the seams between Damascus buildings (two roof edges facing each other a few
/// centimetres apart) stopped at them and hung from them, the hang inside the next building, and free running jumped at
/// them. The edge reports measure the drop with the caller's feet (`Human__BuildEdgeReports` 0xB1BC50 →
/// `Human__MeasureDropBeyondEdge`(this, pos, …) at 0xB1BE87): the lip and line checks find the next roof, where the
/// sweep alone started inside it and measured a 7 m drop. Running and free running over level seams near the Bureau:
/// no hang from a seam and no takeoff at one.
#[test]
fn damascus_runs_over_the_seams_between_buildings() {
    use crate::guidance::GuidanceSubType;
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    s.settle_streaming(20.0, None);
    let roofs: Vec<_> = s.guidance().edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab && e.n0.y > 0.9 && e.n1.y.abs() < 0.3 && e.p0.distance(e.p1) > 1.0).cloned().collect();
    let mut seams = Vec::new();
    for a in &roofs {
        let na = Vec3::new(a.n1.x, 0.0, a.n1.z).normalize_or_zero();
        let qa = (a.p0 + a.p1) * 0.5;
        for b in &roofs {
            let nb = Vec3::new(b.n1.x, 0.0, b.n1.z).normalize_or_zero();
            let qb = b.closest_point(qa);
            let gap = (qb - qa).dot(na);
            if na.dot(nb) > -0.9 || !(-0.05..=0.1).contains(&gap) || (qb - qa - na * gap).with_y(0.0).length() > 0.3 || (qb.y - qa.y).abs() > 0.1 {
                continue;
            }
            let start = qa - na * 4.0;
            let c = s.collision();
            let Some(floor) = c.floor_height_below(start + Vec3::Y * 0.4, 0.8) else { continue };
            let feet = Vec3::new(start.x, floor, start.z);
            // level roof all the way to the seam and 2 m past it
            let level = (-7..=4).all(|k| c.floor_height_below(qa + na * (k as f32 * 0.5) + Vec3::Y * 0.4, 0.8).is_some_and(|y| (y - qa.y).abs() < 0.15));
            if (floor - qa.y).abs() > 0.3 || !level || !c.capsule_fits(feet) {
                continue;
            }
            seams.push((feet, na, qa, a.clone(), b.clone()));
        }
    }
    assert!(seams.len() >= 4, "only {} level seams near the Bureau", seams.len());
    let mut failures = Vec::new();
    for (feet, dir, qa, ea, eb) in seams.iter().take(10) {
        let on_seam = |p: Vec3| ea.closest_point(p).distance(p) < 0.1 || eb.closest_point(p).distance(p) < 0.1;
        for legs in [false, true] {
            s.place(*feet, *dir);
            s.run(0.2);
            s.pad(*dir, 1.0, true, legs);
            for _ in 0..120 {
                s.step();
                let cur = s.loco().current;
                if cur == ActorContextId::Ledge && (on_seam(s.data().ledge.hand_l) || on_seam(s.data().ledge.hand_r)) {
                    failures.push(format!("hung from the seam at {qa} (legs {legs})"));
                    break;
                }
                if cur == ActorContextId::InAir && (s.body().feet - *qa).with_y(0.0).length() < 1.0 {
                    failures.push(format!("took off at the seam at {qa} (legs {legs}): {}", s.body().feet));
                    break;
                }
                if (s.body().feet - *qa).dot(*dir) > 2.0 {
                    break;
                }
            }
            s.pad(*dir, 0.0, false, false);
            s.run(0.5);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Frame cost hanging among many holds (owner report 2026-10-16: the FPS dropped ~80 % holding a grab next to many
/// grabs): the street wall with the most guidance edges within 3.5 m of its hold, grabbed, then held still.
#[test]
#[ignore = "profile; requires the local game install"]
fn profile_damascus_hang_among_many_holds() {
    use crate::guidance::GuidanceSubType;
    let Some(mut s) = CitySim::new(SPAWNS[0]) else { return };
    let c = s.collision();
    let g = s.guidance();
    let mut spots = Vec::new();
    for e in g.edges.iter().filter(|e| e.subtype == GuidanceSubType::LedgeGrab && e.n0.y > 0.9 && e.n1.y.abs() < 0.3 && e.p0.distance(e.p1) > 1.0) {
        let q = (e.p0 + e.p1) * 0.5;
        let n = Vec3::new(e.n1.x, 0.0, e.n1.z).normalize_or_zero();
        let stand = q + n * 0.75;
        let Some(floor) = c.floor_height_below(Vec3::new(stand.x, q.y - 1.0, stand.z), 3.0) else { continue };
        if !(1.9..=2.5).contains(&(q.y - floor)) { continue; }
        let feet = Vec3::new(stand.x, floor, stand.z);
        if !c.capsule_fits(feet) { continue; }
        let near = g.edges.iter().filter(|o| (o.p0 + o.p1).distance(q * 2.0) < 7.0).count();
        spots.push((near, feet, n));
    }
    spots.sort_by(|a, b| b.0.cmp(&a.0));
    for &(near, feet, n) in spots.iter().take(8) {
        s.place(feet, -n);
        s.run(0.2);
        s.pad(-n, 1.0, true, true);
        s.press_legs();
        if !s.run_until(3.0, |s| s.loco().current == ActorContextId::Ledge) { s.pad(n, 0.0, false, false); s.run(1.0); continue; }
        s.pad(-n, 0.0, true, false);
        s.run(1.5);
        if s.loco().current != ActorContextId::Ledge { continue; }
        let mut times = Vec::new();
        for _ in 0..120 {
            let t = std::time::Instant::now();
            s.step();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        let hands = (s.data().ledge.hand_l + s.data().ledge.hand_r) * 0.5;
        let t = std::time::Instant::now();
        let r = s.guidance().ledge_report_depth(hands, 3.5, true, crate::guidance::LEDGE_DEPTH, s.collision());
        let report_ms = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!("PROFILE hang with {near} edges near: frame median {:.2} ms p95 {:.2} ms; one ledge report {report_ms:.2} ms ({} edges); {} boxes, {} triangles in the world", times[60], times[114], r.edges.len(), s.collision().boxes.len(), s.collision().triangles.len());
        s.pad(n, 0.0, false, false);
        s.run(1.5);
    }
}

/// Owner report (2026-10-16): a jump onto a crossbeam in the Souk with the stick let go slipped off. The free-step stay
/// looks for the beam with its small box at the arrival point + 0.5 m (NarrowObjectData +0xF0, 0xE09A13), and the
/// arrival point is the pulled-back target (InAirData+560); from the unpulled target the box missed the beam. Live
/// (lab 2026-10-16 souk chain, 7.23 s): `freestep_entry_tr_beam_air_90` onto the beam at (205.65, 63.78).
#[test]
fn damascus_a_jump_onto_a_crossbeam_with_the_stick_released_stays_on_it() {
    let Some(mut s) = CitySim::new(SPAWNS[0]) else { return };
    let bevy = |x: f32, y: f32, z: f32| Vec3::new(x, z, -y);
    let dir = Vec3::new(-0.7071, 0.0, 0.7071);
    let from = bevy(207.70, 65.83, 6.84);
    s.teleport(from + Vec3::Y * 0.05);
    s.settle_streaming(60.0, Some(from + Vec3::Y * 0.05));
    s.place(from, dir);
    s.run(0.3);
    s.pad(dir, 1.0, true, true);
    s.press_legs();
    assert!(s.run_until(1.0, |s| s.loco().current == ActorContextId::InAir), "no jump: {:?}", s.loco().current);
    s.pad(dir, 0.0, true, false);
    s.run(2.0);
    let f = s.body().feet;
    assert!(s.loco().current == ActorContextId::NarrowObject && s.data().narrow.kind == crate::player::narrow::NarrowKind::Beam,
        "not on the crossbeam: {:?} {:?} at {f}", s.loco().current, s.data().narrow.kind);
    assert!(Vec2::new(f.x - 205.65, f.z + 63.78).length() < 0.3 && (f.y - 6.84).abs() < 0.1, "where the game stops: {f}");
}

/// Import census (diagnostic): every cell decoded, with what the decoder leaves out and why.
#[test]
#[ignore = "diagnostic; decodes every Damascus cell"]
fn probe_damascus_import_census() {
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let mut skipped = std::collections::BTreeMap::<String, usize>::new();
    let (mut objects, mut tris, mut edges, mut cells, mut errors) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut lights, mut effects, mut emitters) = (0usize, 0usize, 0usize);
    for cell in 0..w.data.grid.cells.len() {
        if w.data.grid.cells[cell].datablock == 0 { continue; }
        match super::decode::decode_cell(&w.data, cell) {
            Ok(c) => {
                cells += 1;
                objects += c.objects.len();
                lights += c.lights.iter().filter(|l| l.world).count();
                effects += c.effects.len();
                emitters += c.effects.iter().map(|e| e.system.emitters.len()).sum::<usize>();
                tris += c.objects.iter().map(|o| o.collision.len()).sum::<usize>();
                edges += c.objects.iter().map(|o| o.edges.len()).sum::<usize>();
                for (k, v) in c.stats.skipped { *skipped.entry(k).or_default() += v; }
            }
            Err(e) => { errors += 1; if errors < 5 { eprintln!("CENSUS cell {cell}: {e}"); } }
        }
    }
    eprintln!("CENSUS {cells} cells, {errors} errors, {objects} objects, {tris} tris, {edges} edges, {lights} world lights, {effects} effects ({emitters} emitters)");
    for (k, v) in skipped { eprintln!("CENSUS skipped {v:6} {k}"); }
}

/// Mission DataBlocks (`AC_CITY_DATABLOCKS`): a named block becomes a cell that is always wanted, its placed entities
/// decoded like a grid cell's and its spawn templates at the origin left out. The Vizier's opened mission brings the
/// party's tables and their ledges near (−114, −107).
#[test]
fn damascus_mission_datablocks_load_as_always_wanted_cells() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    if !game.join(super::DAMASCUS.archive).is_file() { return; }
    let w = super::open_with(&game, &super::DAMASCUS, false, "M33_Vizier_MissionOpened").unwrap();
    let cell = w.data.grid.cells.len() - 1;
    assert_eq!(w.data.grid.cells[cell].level, super::grid::DATABLOCK_LEVEL);
    let far = Vec3::new(400.0, 0.0, 400.0);
    assert!(super::stream::wanted_cells(&w.data, &super::stream::content_mask(&w.data), far, None).contains(&cell), "wanted from anywhere");
    let d = super::decode::decode_cell(&w.data, cell).unwrap();
    let edges: usize = d.objects.iter().map(|o| o.edges.len()).sum();
    let tris: usize = d.objects.iter().map(|o| o.collision.len()).sum();
    assert!(edges > 500 && tris > 10_000, "{edges} edges, {tris} triangles");
    assert!(d.stats.skipped.get("spawn template").copied().unwrap_or(0) > 0);
    assert!(d.objects.iter().all(|o| o.transform.w_axis.truncate().length() > 1e-3), "no template at the origin");
    assert!(super::open_with(&game, &super::DAMASCUS, false, "").unwrap().data.grid.cells.len() == 1365);
}

/// Component census (diagnostic): which reflected classes (`AC_CLASSES` = a tsv of hash / name) occur in the city's
/// entities and the World file, counted per entity.
#[test]
#[ignore = "diagnostic; decodes every Damascus cell"]
fn probe_damascus_class_census() {
    let Ok(path) = std::env::var("AC_CLASSES") else { return };
    let names: std::collections::HashMap<u32, String> = std::fs::read_to_string(path).unwrap().lines().filter_map(|l| {
        let (h, n) = l.split_once('\t')?; Some((u32::from_str_radix(h, 16).ok()?, n.to_string()))
    }).collect();
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let count = |b: &[u8], acc: &mut std::collections::BTreeMap<String, usize>| {
        let mut seen = std::collections::HashSet::new();
        for i in 0..b.len().saturating_sub(3) {
            let v = u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
            if let Some(n) = names.get(&v) { if seen.insert(v) { *acc.entry(n.clone()).or_default() += 1; } }
        }
    };
    let mut cells = std::collections::BTreeMap::new();
    for cell in 0..w.data.grid.cells.len() {
        let c = &w.data.grid.cells[cell];
        if c.datablock == 0 { continue; }
        let Some((archive, file)) = w.data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else { continue };
        let a = w.data.archives.names.iter().position(|n| *n == archive).unwrap();
        let Some(entry) = w.data.archives.find(a, &file) else { continue };
        let Ok(res) = w.data.archives.file(a, entry) else { continue };
        for r in res.values() { for body in super::decode::bodies(r) { count(body, &mut cells); } }
    }
    let mut world = std::collections::BTreeMap::new();
    for r in &w.globals { count(&r.payload, &mut world); *world.entry(format!("[resource] {}", names.get(&r.class_hash).cloned().unwrap_or(format!("{:08x}", r.class_hash)))).or_default() += 1; }
    let mut v: Vec<_> = cells.into_iter().collect(); v.sort_by(|a, b| b.1.cmp(&a.1));
    for (n, c) in v { eprintln!("CLASS cells {c:7} {n}"); }
    let mut v: Vec<_> = world.into_iter().collect(); v.sort_by(|a, b| b.1.cmp(&a.1));
    for (n, c) in v { eprintln!("CLASS world {c:7} {n}"); }
}
/// Dumps cell entity bodies holding a class (`AC_DUMP_CLASS`) to `AC_DUMP_DIR`, at most `AC_DUMP_N` (diagnostic).
#[test]
#[ignore = "diagnostic"]
fn probe_dump_cell_bodies() {
    let (Ok(class), Ok(dir)) = (std::env::var("AC_DUMP_CLASS"), std::env::var("AC_DUMP_DIR")) else { return };
    let limit: usize = std::env::var("AC_DUMP_N").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let mut n = 0;
    for cell in 0..w.data.grid.cells.len() {
        let c = &w.data.grid.cells[cell];
        if c.datablock == 0 { continue; }
        let Some((archive, file)) = w.data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else { continue };
        let a = w.data.archives.names.iter().position(|n| *n == archive).unwrap();
        let Some(entry) = w.data.archives.find(a, &file) else { continue };
        let Ok(res) = w.data.archives.file(a, entry) else { continue };
        for r in res.values() {
            if let Ok(rc) = std::env::var("AC_DUMP_RES") {
                if r.class_hash == crate::assets::forge::crc32(&rc) && n < limit {
                    let p = format!("{dir}/{rc}_{:08x}.bin", r.id);
                    std::fs::write(&p, &r.payload).unwrap();
                    eprintln!("RES {} {:08x} {} bytes", r.name, r.id, r.payload.len());
                    n += 1;
                }
                continue;
            }
            for body in super::decode::bodies(r) {
                let at = super::decode::markers(body, &class);
                if at.is_empty() || n >= limit { continue; }
                let p = format!("{dir}/{class}_{n:03}.bin");
                std::fs::write(&p, body).unwrap();
                eprintln!("DUMP {} {:?} at {at:?} {} bytes -> {p}", r.name, super::decode::placement(body).map(|m| m.w_axis.truncate()), body.len());
                n += 1;
            }
        }
    }
}

/// Lists the placed effects (`AC_FX_NEAR="x y"`, game coordinates: the 15 nearest) with their emitters (diagnostic).
#[test]
#[ignore = "diagnostic"]
fn probe_damascus_effects() {
    let Ok(near) = std::env::var("AC_FX_NEAR") else { return };
    let v: Vec<f32> = near.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    let at = Vec3::new(v[0], v[1], 0.0);
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let mut all = Vec::new();
    for cell in 0..w.data.grid.cells.len() {
        let Ok(c) = super::decode::decode_cell(&w.data, cell) else { continue };
        for e in c.effects { all.push(e); }
    }
    all.sort_by(|a, b| a.frame.t.truncate().distance(at.truncate()).total_cmp(&b.frame.t.truncate().distance(at.truncate())));
    for e in all.iter().take(15) {
        let em: Vec<String> = e.system.emitters.iter().map(|m| format!("{:?}/{:?} rate {} max {} tex {:08x}", m.shape, m.blend, m.rate, m.max_count, m.texture)).collect();
        eprintln!("FX at {:?} scale {:.2}: {}", e.frame.t, e.scale, em.join(" | "));
    }
}
/// event's 3D fields and what it plays (diagnostic).
#[test]
#[ignore = "diagnostic"]
fn probe_damascus_emitters() {
    use crate::assets::dare::{bao_object, load_sound_bank, Picker};
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let mut counts: std::collections::HashMap<(u32, u32, usize), (usize, String)> = Default::default();
    for cell in 0..w.data.grid.cells.len() {
        let c = &w.data.grid.cells[cell];
        let Some((archive, file)) = w.data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else { continue };
        let a = w.data.archives.names.iter().position(|n| *n == archive).unwrap();
        let Some(entry) = w.data.archives.find(a, &file) else { continue };
        let Ok(res) = w.data.archives.file(a, entry) else { continue };
        for r in res.values() {
            for body in super::decode::bodies(r) {
                let descs = super::decode::markers(body, "SoundDescriptor");
                for p in super::decode::markers(body, "Ambient3dSoundComponent") {
                    let mine: Vec<usize> = descs.iter().copied().filter(|&q| q > p).collect();
                    let Some(&q) = mine.first() else { continue };
                    let rd = |o: usize| body.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).unwrap_or(0);
                    let e = counts.entry((rd(q + 4), rd(q + 8), mine.len())).or_insert((0, r.name.clone()));
                    e.0 += 1;
                }
            }
        }
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    let mut bank = load_sound_bank(&game).unwrap();
    let mut pick = Picker::new(1);
    eprintln!("EMIT {} distinct", v.len());
    for ((event, resource, n), (count, name)) in v.iter().take(25) {
        let mut todo = vec![*resource];
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = todo.pop() {
            if !seen.insert(id) { continue; }
            let Ok(r) = w.data.archives.get(id) else { continue };
            if r.class_hash != crate::assets::forge::crc32("SoundBao") { continue; }
            bank.insert(r.id, &r.name, &r.payload);
            let wrap = r.payload.len() - bao_object(&r.payload).map_or(0, |o| o.len());
            todo.extend((0..wrap.saturating_sub(3)).filter_map(|p| Some(u32::from_le_bytes(r.payload[p..p + 4].try_into().ok()?))).filter(|d| w.data.archives.contains(*d)));
        }
        let o = bank.object(*event).unwrap_or(&[]);
        let f = |k: usize| o.get(k..k + 4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).unwrap_or(f32::NAN);
        let sub = o.get(8..12).map(|b| u32::from_le_bytes(b.try_into().unwrap())).unwrap_or(99);
        let sound = o.get(12..16).map(|b| u32::from_le_bytes(b.try_into().unwrap())).unwrap_or(0);
        let st = bank.object(sound).and_then(|s| s.get(8..12)).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let voices = bank.resolve(*event, &|_| None, &mut pick);
        if st == Some(2) && std::env::var("AC_SND_DUMP").is_ok() {
            let so = bank.object(sound).unwrap();
            for (k, ch) in so.chunks(16).enumerate().take(24) { eprintln!("EMIT2 {sound:08x} +{:03x}: {:02x?}", k * 16, ch); }
        }
        eprintln!("EMIT {count:4}x {event:08x} ({n} desc) on {name}: event type {sub}, sound {sound:08x} type {st:?}, 3d [{:.2} {:.2} {:.2} {:.2} {:.2} {:.2}], voices {:?}",
            f(0x20), f(0x24), f(0x28), f(0x2C), f(0x30), f(0x34), voices.iter().map(|v| (v.res, v.channels, v.rate, v.frames, v.gain_db)).collect::<Vec<_>>());
    }
}

/// The presentation around the Bureau (RE/17 §11): the lantern lights, the placed particle systems, the ambient emitters
/// (the courtyard fountain: event 0x10990006 through its SoundBao 0x5D5784A5 to a looping IMA resource, full at 2 m,
/// silent at 15 m) and the Bureau's ambience sets.
#[test]
fn damascus_bureau_presentation() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    if !game.join(super::DAMASCUS.archive).is_file() { return; }
    let p = super::prepare(&game, &super::DAMASCUS, false, "PlayerSpawner_DEBUG_Bureau").unwrap();
    let lights: Vec<_> = p.cells.iter().flat_map(|c| c.lights.iter()).filter(|l| l.world).collect();
    assert!(!lights.is_empty() && lights.iter().all(|l| l.far > 0.0 && l.intensity > 0.0), "{} world lights", lights.len());
    let effects: usize = p.cells.iter().map(|c| c.effects.len()).sum();
    assert!(effects > 50, "{effects} effects");
    assert!(p.cells.iter().flat_map(|c| c.effects.iter()).all(|e| !e.system.emitters.is_empty()));
    let sounds: Vec<_> = p.cells.iter().flat_map(|c| c.sounds.iter()).collect();
    assert!(sounds.len() > 100, "{} emitters", sounds.len());
    let fountain = sounds.iter().find(|s| s.event == 0x1099_0006).expect("a fountain emitter");
    assert_eq!(fountain.resource, 0x5D57_84A5);
    let mut amb = super::presentation::CityAmbience::default();
    amb.load_for_test(&p.world.data.archives, fountain.resource);
    let (bank, range) = amb.bank_for_test(fountain.event);
    assert_eq!(range, Some((2.0, 15.0)));
    let v = bank.resolve(fountain.event, &|_| None, &mut crate::assets::dare::Picker::new(1));
    assert_eq!(v.len(), 1);
    assert_eq!((v[0].channels, v[0].rate, v[0].frames), (1, 32000, 0), "a loop: no stored frame count");
    let pcm = crate::assets::dare::decode_ima(bank.object(v[0].res).unwrap(), 1, 0).unwrap();
    assert!(pcm.len() > 32000 * 5, "{} samples", pcm.len());
    let sets: Vec<&str> = p.cells.iter().flat_map(|c| c.ambience.iter().map(|s| s.name.as_str())).collect();
    assert!(sets.contains(&"A_City_Bureau_Main_Room"), "{sets:?}");
    let fx: Vec<_> = p.cells.iter().flat_map(|c| c.ambience.iter()).flat_map(|s| s.fx.iter()).collect();
    assert!(fx.iter().all(|f| f.delay.0 >= 0.5 && f.delay.1 >= f.delay.0 && f.distance.1 >= f.distance.0 && f.distance.1 > 0.0), "{fx:?}");
    assert!(!fx.is_empty());
}

/// Entities whose name has `AC_PROBE_NAME`: their Visuals, drawables and what the decoder makes of them (diagnostic).
#[test]
#[ignore = "diagnostic"]
fn probe_damascus_named_entities() {
    let Ok(pat) = std::env::var("AC_PROBE_NAME") else { return };
    let limit: usize = std::env::var("AC_DUMP_N").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let cls = |id: u32| w.data.archives.get(id).map(|r| format!("{:08x} {}", r.class_hash, r.name)).unwrap_or_else(|e| format!("? {e}"));
    let mut n = 0;
    'outer: for cell in 0..w.data.grid.cells.len() {
        let c = &w.data.grid.cells[cell];
        let Some((archive, file)) = w.data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else { continue };
        let a = w.data.archives.names.iter().position(|n| *n == archive).unwrap();
        let Some(entry) = w.data.archives.find(a, &file) else { continue };
        let Ok(res) = w.data.archives.file(a, entry) else { continue };
        let mut list: Vec<_> = res.values().filter(|r| r.name.contains(&pat)).collect();
        list.sort_by_key(|r| r.id);
        for r in list {
            let bodies = super::decode::bodies(r);
            eprintln!("NAMED {} class {:08x} {} bytes, {} bodies, cell {cell} level {}", r.name, r.class_hash, r.payload.len(), bodies.len(), c.level);
            for (k, b) in bodies.iter().enumerate() {
                let vis = super::decode::markers(b, "Visual");
                let rd = |o: usize| b.get(o..o + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap())).unwrap_or(0);
                let v: Vec<String> = vis.iter().map(|&p| format!("@{p} u8 {} -> {}", b[p + 4], cls(rd(p + 5)))).collect();
                let comps: Vec<&str> = ["InertSwitch", "InertComponent", "RigidBodyComponent", "SkeletonComponent", "AnimComponent", "BhvKiosk", "Kiosk", "LODSelector", "BlendPosClip", "GuidanceSystem", "EntityGroup", "ReactiveProp"]
                    .into_iter().filter(|n| !super::decode::markers(b, n).is_empty()).collect();
                eprintln!("NAMED   body {k} ({} bytes) at {:?}: visuals {v:?} comps {comps:?}", b.len(), super::decode::placement(b).map(|m| m.w_axis.truncate()));
            }
            let d = super::decode::decode_file(&w.data, cell, a, &file);
            if let Ok(d) = d {
                for o in d.objects.iter().filter(|o| o.name.contains(&pat)) {
                    let m = o.meshes.first();
                    let local = m.map(|m| m.mesh.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(*p), h.max(*p))));
                    eprintln!("NAMED   -> object {} meshes {} lod {:?} verts {:?} local {:?} world {:?}..{:?} sections {:?}", o.name, o.meshes.len(), o.lod.mesh,
                        m.map(|m| m.mesh.positions.len()), local, o.visual_bounds.min, o.visual_bounds.max, m.map(|m| m.mesh.sections.iter().map(|s| (s.indices.len() / 3, s.material)).collect::<Vec<_>>()));
                }
                for (k, v) in &d.stats.skipped { eprintln!("NAMED   cell skip {v} {k}"); }
            }
            n += 1;
            if n >= limit { break 'outer; }
        }
    }
}

/// A kiosk's stall (`Outdoor_Market_Stall_Skinned`, Kiosk_Vases_034 by the Souk) is a skinned mesh stored per bone: posed
/// by its skeleton and its animations' first key it stands on the street (ground 8.0) up to its awning, instead of
/// lying folded under the street.
#[test]
fn damascus_kiosk_stall_is_posed() {
    let Some(game) = crate::assets::find_game_dir() else { return };
    if !game.join(super::DAMASCUS.archive).is_file() { return; }
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let at = super::decode::to_bevy(Vec3::new(-183.24, -104.34, 8.0));
    let cell = super::stream::wanted_cells(&w.data, &super::stream::content_mask(&w.data), at, None).into_iter()
        .find(|&c| super::decode::decode_cell(&w.data, c).is_ok_and(|d| d.objects.iter().any(|o| o.name.starts_with("Kiosk_Vases_034/")))).unwrap();
    let d = super::decode::decode_cell(&w.data, cell).unwrap();
    let stall = d.objects.iter().filter(|o| o.name.starts_with("Kiosk_Vases_034/"))
        .max_by_key(|o| o.meshes.first().map_or(0, |m| m.mesh.positions.len())).unwrap();
    let b = stall.visual_bounds;
    assert!(b.min.y > 7.5 && b.min.y < 8.1 && b.max.y > 10.8 && b.max.y < 11.6, "{:?}", b);
    assert!((b.max.x - b.min.x) > 3.5 || (b.max.z - b.min.z) > 3.5, "the stall's length {:?}", b);
}

/// Haystack census (diagnostic, plan W1): per haystack, the entity origin (what the game's searches read, 0xE15E6D)
/// and the collision box (the contacts, the dive ray and the hop-out footprint, 0xE05490 / 0xE434E0) beside the hay
/// mesh's bounds (the old stand-in).
#[test]
#[ignore = "diagnostic; decodes every Damascus cell"]
fn probe_damascus_haystack_census() {
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let mut n = 0;
    for cell in 0..w.data.grid.cells.len() {
        if w.data.grid.cells[cell].datablock == 0 { continue; }
        let Ok(c) = super::decode::decode_cell(&w.data, cell) else { continue };
        for o in c.objects.iter() {
            let Some(h) = o.haystack else { continue };
            n += 1;
            let mesh = o.meshes.first().map(|m| super::decode::mesh_bounds(&m.mesh, o.transform));
            eprintln!("HAY {cell:5} origin {:.2} box centre {:.2} half {:.2} | mesh {:.2?}", h.origin, h.centre, h.half, mesh.map(|m| (m.min, m.max)));
        }
    }
    eprintln!("HAY {n} haystacks");
}

/// Surface-material census (diagnostic, plan W9): collision triangles with a resolved sound material.
#[test]
#[ignore = "diagnostic; decodes every Damascus cell"]
fn probe_damascus_material_census() {
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let (mut all, mut with) = (0usize, 0usize);
    for cell in 0..w.data.grid.cells.len() {
        if w.data.grid.cells[cell].datablock == 0 { continue; }
        let Ok(c) = super::decode::decode_cell(&w.data, cell) else { continue };
        for t in c.objects.iter().flat_map(|o| o.collision.iter()) {
            all += 1;
            with += (t.material != crate::triangles::NO_MATERIAL) as usize;
        }
    }
    eprintln!("MATERIALS {with} of {all} collision triangles have a sound material");
}

/// Live check A3 (2026-10-18): the port's `Guidance__FindWallSupport` at a game call's origin and direction
/// (`LAB_ORIGIN`, `LAB_INTO`, game axes), with the box 0.175 / 0.3 / 0.3, step 0.07: the ray's hit and the surface
/// pieces, to compare with the game's result.
#[test]
#[ignore = "live-analysis probe; requires the local game install"]
fn probe_wall_support_at() {
    let v3 = |k: &str| std::env::var(k).ok().map(|s| { let v: Vec<f32> = s.split(',').map(|x| x.trim().parse().unwrap()).collect(); Vec3::new(v[0], v[2], -v[1]) });
    let (Some(origin), Some(into)) = (v3("LAB_ORIGIN"), v3("LAB_INTO")) else { return };
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Souk") else { return };
    s.place(origin + Vec3::new(0.0, 0.0, 0.0), into);
    s.settle_streaming(60.0, Some(origin));
    let g3 = |p: Vec3| Vec3::new(p.x, -p.z, p.y);
    let b = crate::player::ledge::SupportBox { across: 0.175, depth: 0.3, height: 0.3, step: 0.07 };
    let n = -Vec3::new(into.x, 0.0, into.z).normalize_or_zero();
    let c = s.collision();
    let start = origin + into.normalize() * -0.3;
    let d = c.sphere_free_distance(start, into.normalize(), 0.01, 0.6);
    eprintln!("ray from {:.3} along {:.3}: {d:.3} of 0.6", g3(start), g3(into));
    if let Some((dist, hn, layer)) = c.ray_hit_layer(start, into.normalize(), 0.6, crate::layers::CHARACTER) {
        eprintln!("  ray hit at {dist:.3} normal {:.2} layer {layer}", g3(hn));
    }
    eprintln!("support {:?}", crate::player::ledge::wall_support_point(origin, n, 0.0, 0.0, b, c).map(g3));
}

/// Live check (2026-10-18): the port's floor heights on a grid around `LAB_AT` (game axes), `LAB_STEP` m apart, from
/// 4 m above, to compare with where the game stands.
#[test]
#[ignore = "live-analysis probe; requires the local game install"]
fn probe_floor_grid() {
    let v: Vec<f32> = std::env::var("LAB_AT").ok().map(|s| s.split(',').map(|x| x.trim().parse().unwrap()).collect()).unwrap_or_default();
    if v.len() != 3 { return; }
    let step: f32 = std::env::var("LAB_STEP").ok().and_then(|s| s.parse().ok()).unwrap_or(0.5);
    let at = Vec3::new(v[0], v[2], -v[1]);
    let Some(mut s) = CitySim::new("PlayerSpawner_DEBUG_Bureau") else { return };
    s.teleport(at);
    s.settle_streaming(60.0, Some(at));
    let c = s.collision();
    for j in (-3..=3).rev() {
        let mut line = String::new();
        for i in -3..=3 {
            let p = Vec3::new(at.x + i as f32 * step, at.y + 4.0, at.z - j as f32 * step);
            let d = c.ray_distance(p, Vec3::NEG_Y, 12.0, crate::layers::MAIN_CHARACTER);
            line += &if d < 12.0 { format!("{:7.2}", p.y - d) } else { "   none".into() };
        }
        eprintln!("y {:+.1}: {line}", j as f32 * step);
    }
}

/// Live check B1 (2026-10-18): the breakable stalls (`BhvKiosk` entities) within `LAB_RADIUS` (default 60) m of
/// `LAB_AT` (game axes, default the Bureau spawner): name and position, to drive into in the running game.
#[test]
#[ignore = "live-analysis probe; requires the local game install"]
fn probe_breakable_stalls() {
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let v: Vec<f32> = std::env::var("LAB_AT").ok().map(|s| s.split(',').map(|x| x.trim().parse().unwrap()).collect()).unwrap_or(vec![46.39, 6.13, 4.14]);
    let radius: f32 = std::env::var("LAB_RADIUS").ok().and_then(|s| s.parse().ok()).unwrap_or(60.0);
    let at = Vec2::new(v[0], v[1]);
    let mut found = Vec::new();
    for cell in 0..w.data.grid.cells.len() {
        if w.data.grid.cells[cell].datablock == 0 { continue; }
        let c = &w.data.grid.cells[cell];
        let Some((archive, file)) = w.data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else { continue };
        let Some(a) = w.data.archives.names.iter().position(|n| *n == archive) else { continue };
        let Some(entry) = w.data.archives.find(a, &file) else { continue };
        let Ok(resources) = w.data.archives.file(a, entry) else { continue };
        for r in resources.values() {
            for body in super::decode::bodies(r) {
                if super::decode::markers(body, "BhvKiosk").is_empty() { continue; }
                let Ok(m) = super::decode::placement(body) else { continue };
                let p = m.w_axis.truncate();
                if Vec2::new(p.x, p.y).distance(at) < radius {
                    found.push((Vec2::new(p.x, p.y).distance(at), r.name.clone(), p));
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found.dedup_by(|a, b| a.2.distance(b.2) < 0.01);
    for (d, n, p) in found.iter().take(20) {
        eprintln!("STALL {d:6.1} m  {n}  at {:.2} {:.2} {:.2}", p.x, p.y, p.z);
    }
}

/// Draw sharing census (2026-10-18): over the cells wanted at `LAB_AT` (game axes, default the demo course start), the
/// drawn sections at the camera's distance from each object (no size cull), and how many distinct meshes they would
/// need when placements of the same mesh share it — with and without the placements' baked vertex colours.
#[test]
#[ignore = "live-analysis probe; requires the local game install"]
fn probe_draw_sharing() {
    use std::collections::{HashMap, HashSet};
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let v: Vec<f32> = std::env::var("LAB_AT").ok().map(|s| s.split(',').map(|x| x.trim().parse().unwrap()).collect()).unwrap_or(vec![51.2, 6.0, 11.8]);
    let at = Vec3::new(v[0], v[2], -v[1]);
    let mask = super::stream::content_mask(&w.data);
    let cells = super::stream::wanted_cells(&w.data, &mask, at, None);
    let (mut sections, mut objects, mut coloured, mut vertices) = (0usize, 0usize, 0usize, 0usize);
    let mut by_mesh: HashMap<(u32, usize), usize> = HashMap::new();
    let mut by_colour: HashSet<(u32, usize, u64)> = HashSet::new();
    let mut per_cell_material: HashSet<(usize, u32)> = HashSet::new();
    for &c in &cells {
        let Ok(d) = super::decode::decode_cell(&w.data, c) else { continue };
        for o in &d.objects {
            let b = &o.visual_bounds;
            let dist = at.distance(at.clamp(b.min, b.max));
            let Some(m) = o.lod.pick(dist).map(|m| m as usize).filter(|&m| m < o.meshes.len()) else { continue };
            objects += 1;
            let lod = &o.meshes[m];
            coloured += lod.colors.is_some() as usize;
            let hash = lod.colors.as_ref().map_or(0, |c| { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); c.hash(&mut h); h.finish() });
            for (k, s) in lod.mesh.sections.iter().enumerate() {
                sections += 1;
                vertices += s.indices.len();
                *by_mesh.entry((lod.id, k)).or_default() += 1;
                by_colour.insert((lod.id, k, hash));
                per_cell_material.insert((c, s.material));
            }
        }
    }
    // objects whose drawn mesh never changes while their cell can be loaded (same LOD from 0 to 200 m, no size cull)
    let (mut fixed_objects, mut fixed_sections, mut all_sections, mut fixed_cm) = (0usize, 0usize, 0usize, HashSet::new());
    for &c in &cells {
        let Ok(d) = super::decode::decode_cell(&w.data, c) else { continue };
        for o in &d.objects {
            if o.meshes.is_empty() { continue; }
            let first = o.lod.pick(0.0);
            let n = first.and_then(|m| o.meshes.get(m as usize)).map_or(0, |l| l.mesh.sections.len());
            all_sections += n;
            if o.size & 3 == 0 && first.is_some() && (0..=400).all(|k| o.lod.pick(k as f32 * 0.5) == first) {
                fixed_objects += 1;
                fixed_sections += n;
                for s in &o.meshes[first.unwrap() as usize].mesh.sections { fixed_cm.insert((c, s.material)); }
            }
        }
    }
    eprintln!("fixed-LOD objects {fixed_objects}: {fixed_sections} of {all_sections} LOD0 sections, merged into {} (cell, material) meshes", fixed_cm.len());
    let shared = by_mesh.values().filter(|&&n| n > 1).count();
    let in_shared: usize = by_mesh.values().filter(|&&n| n > 1).sum();
    eprintln!("cells {} objects drawn {objects} (with baked colours {coloured}) sections {sections} indices {vertices}", cells.len());
    eprintln!("distinct (mesh, section) {} — {shared} used more than once, covering {in_shared} sections", by_mesh.len());
    eprintln!("distinct (mesh, section, colours) {}", by_colour.len());
    eprintln!("distinct (cell, material) {}", per_cell_material.len());
}

/// Kiosk axis check (2026-10-18): near `LAB_AT` (game axes, default Kiosk_Grain_020), each object with Kiosk guidance:
/// its entity matrix's rows (game axes) against the directions of its Kiosk pieces.
#[test]
#[ignore = "live-analysis probe; requires the local game install"]
fn probe_kiosk_axes() {
    use crate::guidance::GuidanceSubType;
    let game = crate::assets::find_game_dir().unwrap();
    let w = super::open(&game, &super::DAMASCUS, false).unwrap();
    let v: Vec<f32> = std::env::var("LAB_AT").ok().map(|s| s.split(',').map(|x| x.trim().parse().unwrap()).collect()).unwrap_or(vec![98.35, 11.82, 4.0]);
    let at = Vec3::new(v[0], v[2], -v[1]);
    let mask = super::stream::content_mask(&w.data);
    let g = |v: Vec3| format!("({:.3} {:.3} {:.3})", v.x, -v.z, v.y);
    for c in super::stream::wanted_cells(&w.data, &mask, at, None) {
        let Ok(d) = super::decode::decode_cell(&w.data, c) else { continue };
        for o in &d.objects {
            let kiosk: Vec<_> = o.edges.iter().filter(|e| e.subtype == GuidanceSubType::Kiosk).collect();
            if kiosk.is_empty() || o.bounds.min.distance(at).min(o.bounds.max.distance(at)) > 30.0 { continue; }
            let m = o.transform;
            eprintln!("{} at {}: x {} y {} z {}", o.name, g(m.w_axis.truncate()), g(m.x_axis.truncate().normalize()), g(m.y_axis.truncate().normalize()), g(m.z_axis.truncate().normalize()));
            for e in kiosk { eprintln!("   kiosk piece {} -> {} dir {}", g(e.p0), g(e.p1), g((e.p1 - e.p0).normalize())); }
        }
    }
}
