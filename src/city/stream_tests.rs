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
            .add_systems(Update, (super::update_city, crate::player::social::update_social, crate::player::crowd::update_crowd, crate::player::ground::update_ground,
                crate::player::air::update_air, crate::player::ledge::update_ledge, crate::player::climb::update_climb, crate::player::hay::update_hay,
                crate::player::walling::update_walling, crate::player::narrow::update_narrow, crate::player::ladder::update_ladder, crate::player::release_limbs).chain());
        let player = app.world_mut().spawn((crate::player::player_components(prepared.spawn, prepared.heading), Transform::default())).id();
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
        self.app.update();
    }
    pub fn run(&mut self, seconds: f32) { for _ in 0..(seconds * 60.0) as usize { self.step(); } }
    pub fn body(&self) -> &Body { self.app.world().get::<Body>(self.player).unwrap() }
    pub fn loco(&self) -> &Locomotion { self.app.world().get::<Locomotion>(self.player).unwrap() }
    pub fn stats(&self) -> super::stream::CityStats { self.app.world().resource::<super::stream::City>().0.as_ref().unwrap().stats.clone() }
    pub fn teleport(&mut self, feet: Vec3) {
        let mut b = self.app.world_mut().get_mut::<Body>(self.player).unwrap();
        b.feet = feet; b.velocity = Vec3::ZERO; b.proxy.manifold.clear();
    }
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
        let up = s.run_until(8.0, |s| s.loco().current == ActorContextId::Ground);
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
