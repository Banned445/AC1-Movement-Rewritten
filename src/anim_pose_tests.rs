//! The animation step as the simulation sees it (bone positions in the simulation): a player with the movement
//! contexts, the animation library and a rig, run headless. Tests that need the install skip without it.
use super::*;
use crate::bone_pose::{self, PoseSkeleton};
use crate::player::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

/// The contexts, the clip choice, the animation step and the rig writer, with Altaïr's rig spawned as the renderer
/// spawns it (`model::attach_altair`). None without the install.
fn build(feet: Vec3, heading: f32) -> Option<(App, Entity)> {
    let bones = crate::assets::altair::load_skeleton(&game_dir()).ok()?;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)))
        .insert_resource(crate::input::PadInput { legs_pressed_ago: f32::INFINITY, hand_pressed_ago: f32::INFINITY, ..default() })
        .insert_resource(SpawnPoint(feet))
        .insert_resource(PoseSkeleton::from_bones(&bones))
        .init_resource::<crate::camera::CameraRig>()
        .init_resource::<AnimLibrary>()
        .add_systems(Startup, load_library)
        .add_systems(Update, ((interpreter::update_interpreter, social::update_social, crowd::update_crowd, ground::update_ground, air::update_air, ledge::update_ledge, climb::update_climb, hay::update_hay, kiosk::update_kiosk, walling::update_walling, narrow::update_narrow, ladder::update_ladder, release_limbs).chain(), (choose_clip, animate_pose, apply_clip).chain()).chain());
    let (c, g) = crate::level::geometry();
    app.insert_resource(c).insert_resource(g);
    let player = app.world_mut().spawn((player_components(feet, heading), Transform::default(), AnimPlayer::default())).id();
    let root = app.world_mut().spawn(Transform::default()).id();
    app.world_mut().entity_mut(player).add_child(root);
    let skel = PoseSkeleton::from_bones(&bones);
    let mut joints = Vec::new();
    for (i, t) in skel.rest.iter().enumerate() {
        let e = app.world_mut().spawn(*t).id();
        let parent = skel.parents[i].map(|p| joints[p]).unwrap_or(root);
        app.world_mut().entity_mut(parent).add_child(e);
        joints.push(e);
    }
    app.world_mut().entity_mut(player).insert(crate::model::Rig { root, joints, rest: skel.rest.clone(), bone_ids: skel.bone_ids.clone(), parents: skel.parents.clone() });
    app.update();
    Some((app, player))
}

fn pad(app: &mut App, dir: Vec3, stick: f32, high: bool, legs: bool) {
    let mut p = app.world_mut().resource_mut::<crate::input::PadInput>();
    p.dir = dir.normalize_or_zero();
    p.magnitude = stick;
    p.speed01 = if stick <= 0.35 { 0.0 } else { ((stick - 0.35) / 0.65).min(1.0) };
    p.high_profile = high;
    p.legs_held = legs;
}

/// Runs `seconds`, calling `each` after every frame.
fn run(app: &mut App, player: Entity, seconds: f32, mut each: impl FnMut(&App)) {
    for _ in 0..(seconds * 60.0) as usize {
        {
            let mut p = app.world_mut().resource_mut::<crate::input::PadInput>();
            p.legs_pressed_ago += 1.0 / 60.0;
            p.hand_pressed_ago += 1.0 / 60.0;
        }
        let h = app.world().get::<Body>(player).unwrap().heading;
        app.world_mut().resource_mut::<crate::camera::CameraRig>().yaw = h;
        app.update();
        each(app);
    }
}

/// Three scripted runs: a ladder (walk at it, climb, wait, climb down), a free-run jump across a roof gap, and a walk,
/// a run and the stop into the wait.
fn scripted(mut each: impl FnMut(&App, Entity)) -> bool {
    let Some((mut app, p)) = build(Vec3::new(50.0, 0.0, 60.5), std::f32::consts::PI) else { return false };
    for (dir, stick, high, legs, secs) in [(Vec3::Z, 1.0, false, false, 5.0), (Vec3::ZERO, 0.0, false, false, 1.0), (Vec3::NEG_Z, 1.0, false, false, 5.0)] {
        pad(&mut app, dir, stick, high, legs);
        run(&mut app, p, secs, |a| each(a, p));
    }
    let (mut app, p) = build(Vec3::new(7.0, 3.5, 12.0), -std::f32::consts::FRAC_PI_2).unwrap();
    for (dir, stick, high, legs, secs) in [(Vec3::X, 1.0, true, true, 4.0), (Vec3::X, 0.0, false, false, 1.0)] {
        pad(&mut app, dir, stick, high, legs);
        run(&mut app, p, secs, |a| each(a, p));
    }
    let (mut app, p) = build(Vec3::new(0.0, 0.0, 8.6), 0.0).unwrap();
    for (dir, stick, high, legs, secs) in [(Vec3::NEG_Z, 0.6, false, false, 2.0), (Vec3::NEG_Z, 1.0, true, false, 2.0), (Vec3::ZERO, 0.0, false, false, 2.0)] {
        pad(&mut app, dir, stick, high, legs);
        run(&mut app, p, secs, |a| each(a, p));
    }
    true
}

/// The simulation's bone positions are where the renderer draws the bones: the rig's joint hierarchy under the player
/// at the body's feet, heading and tilt.
#[test]
fn the_bone_pose_matches_the_drawn_skeleton() {
    let mut frames = 0;
    let mut worst = 0.0f32;
    let ran = scripted(|app, p| {
        let (body, bone, rig) = (app.world().get::<Body>(p).unwrap(), app.world().get::<BonePose>(p).unwrap(), app.world().get::<crate::model::Rig>(p).unwrap());
        let skel = app.world().resource::<PoseSkeleton>();
        assert!(bone.seq > 0, "the animation step posed the skeleton");
        let player = Transform { translation: body.feet, rotation: body.tilt * Quat::from_rotation_y(body.heading), scale: Vec3::ONE }.to_matrix();
        let mut world: Vec<Mat4> = Vec::new();
        let root = player * app.world().get::<Transform>(rig.root).unwrap().to_matrix();
        for (i, e) in rig.joints.iter().enumerate() {
            let parent = rig.parents[i].map(|q| world[q]).unwrap_or(root);
            world.push(parent * app.world().get::<Transform>(*e).unwrap().to_matrix());
        }
        for id in [bone_pose::HEAD, bone_pose::SPINE, bone_pose::HIPS, bone_pose::LEFT_HAND, bone_pose::RIGHT_HAND, bone_pose::LEFT_FOOT, bone_pose::RIGHT_FOOT] {
            let i = skel.index(id).expect("bone in the skeleton");
            let drawn = world[i].w_axis.truncate();
            let pos = bone.world_pos(skel, id, body).unwrap();
            worst = worst.max(drawn.distance(pos));
        }
        frames += 1;
    });
    if !ran {
        eprintln!("skipped: game install not found");
        return;
    }
    assert!(frames > 1000 && worst < 1e-4, "{frames} frames, worst {worst} m");
}

/// A tool for refactors of the animation step (not a regression test): `GOLDEN_OUT=<file>` writes every joint's
/// local transform over the scripted runs, `GOLDEN_IN=<file>` compares a later build with it (1e-6).
#[test]
#[ignore = "refactor tool; needs the install"]
fn golden_pose_dump() {
    let mut out = Vec::new();
    let ran = scripted(|app, p| {
        let rig = app.world().get::<crate::model::Rig>(p).unwrap();
        let r = app.world().get::<Transform>(rig.root).unwrap();
        let mut line = format!("R {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6}|", r.rotation.x, r.rotation.y, r.rotation.z, r.rotation.w, r.translation.x, r.translation.y, r.translation.z);
        for e in &rig.joints {
            let t = app.world().get::<Transform>(*e).unwrap();
            line += &format!("{:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6}|", t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w, t.translation.x, t.translation.y, t.translation.z);
        }
        out.push(line);
    });
    if !ran {
        return;
    }
    if let Some(f) = std::env::var_os("GOLDEN_OUT") {
        std::fs::write(f, out.join("\n")).unwrap();
    }
    if let Some(f) = std::env::var_os("GOLDEN_IN") {
        let want = std::fs::read_to_string(f).unwrap();
        let want: Vec<&str> = want.lines().collect();
        assert_eq!(want.len(), out.len(), "frame count");
        let mut worst = (0.0f64, 0usize);
        for (k, (a, b)) in want.iter().zip(&out).enumerate() {
            let pa: Vec<f64> = a.split(['|', ' ']).filter_map(|x| x.parse().ok()).collect();
            let pb: Vec<f64> = b.split(['|', ' ']).filter_map(|x| x.parse().ok()).collect();
            assert_eq!(pa.len(), pb.len(), "frame {k}");
            for (x, y) in pa.iter().zip(&pb) {
                if (x - y).abs() > worst.0 {
                    worst = ((x - y).abs(), k);
                }
            }
        }
        eprintln!("golden: {} frames, worst difference {:.2e} at frame {}", out.len(), worst.0, worst.1);
        assert!(worst.0 <= 1e-6, "the pose changed");
    }
}
