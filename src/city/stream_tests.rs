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
            .init_resource::<Assets<Mesh>>().init_resource::<Assets<StandardMaterial>>().init_resource::<Assets<Image>>()
            .init_resource::<CollisionWorld>().init_resource::<crate::guidance::GuidanceWorld>()
            .init_resource::<super::stream::City>()
            .add_systems(Update, (super::update_city, crate::player::interpreter::update_interpreter, crate::player::social::update_social, crate::player::crowd::update_crowd, crate::player::ground::update_ground,
                crate::player::air::update_air, crate::player::ledge::update_ledge, crate::player::climb::update_climb, crate::player::hay::update_hay, crate::player::kiosk::update_kiosk,
                crate::player::walling::update_walling, crate::player::narrow::update_narrow, crate::player::ladder::update_ladder, crate::player::release_limbs, super::hold_player).chain());
        app.add_systems(Update, crate::anim::headless_animation_systems().after(super::hold_player));
        let animated = crate::bone_pose::add_headless_animation(&mut app);
        let player = app.world_mut().spawn((crate::player::player_components(prepared.spawn, prepared.heading), Transform::default())).id();
        if animated {
            app.world_mut().entity_mut(player).insert(crate::anim::AnimPlayer::default());
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
    trace.apply_start(s.app.world_mut(), player);
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
        let clip = s.app.world().get::<crate::anim::AnimPlayer>(s.player).and_then(|p| p.clip.clone()).unwrap_or_default();
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
            let top = Vec3::new((st.min.x + st.max.x) * 0.5, st.max.y, (st.min.z + st.max.z) * 0.5);
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
    s.run(1.0);
    let game = |p: Vec3| Vec3::new(p.x, -p.z, p.y);
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
