//! Demo parkour course (`AC_DEMO_COURSE=1`): a strip of stations at z ≈ 130 on the greybox's empty north side, each one
//! a copy of a tested greybox setup, for showcase recordings. Not part of the reverse-engineered game: the layout is
//! the port's own. Off by default, so the greybox, the tests and the replay fixtures are unchanged.
//!
//! The run: walk / run / vault a railing → wall run onto a block → free run up two roofs → the beam → a rooftop gap →
//! pull down into a wall hang, shimmy, let go → jump up to a ledge, shimmy, side jump, pull up → ladder → pilotis →
//! a free-run drop and roll → swing bars → climb the tower → Leap of Faith into a haystack, hop out.

use bevy::prelude::*;

use crate::collision::{Aabb3, CollisionWorld};
use crate::guidance::{GuidanceEdge, GuidanceSubType, GuidanceWorld};

pub fn enabled() -> bool {
    std::env::var_os("AC_DEMO_COURSE").is_some()
}

/// The run's start: on the ground at the west end, facing +X.
pub const START: Vec3 = Vec3::new(-140.0, 0.0, 130.0);
pub const START_HEADING: f32 = -std::f32::consts::FRAC_PI_2;

/// Blocks (min x, max x, min z, max z, height), roof edges on every block of 0.5 m or more.
pub const BLOCKS: &[(f32, f32, f32, f32, f32)] = &[
    // a 1 m railing, 0.3 m thick: the pass-over vault
    (-112.15, -111.85, 128.0, 132.0, 1.0),
    // the wall-run block (face x -95): the entry's pull-up onto its 1.8 m top
    (-95.0, -91.0, 128.0, 132.0, 1.8),
    // two roofs 2 m apart, each about 1.2 m higher: the free run's jumps up
    (-89.0, -84.0, 127.5, 132.5, 3.0),
    (-82.0, -77.0, 127.5, 132.5, 4.0),
    // the beam's far platform
    (-71.0, -67.0, 128.0, 132.0, 4.0),
    // a 3.5 m roof past a 3.5 m gap: the free-run jump down, then the pull-down from its +X edge
    (-63.5, -57.5, 127.0, 133.0, 3.5),
    // the ledge walls (face x -45, 2.6 m): C and D with a 1 m gap between them
    (-45.0, -42.0, 125.0, 131.0, 2.6),
    (-45.0, -42.0, 132.0, 135.0, 2.6),
    // the ladder's platform (face x -25, 5 m)
    (-25.0, -21.0, 128.0, 132.0, 5.0),
    // the pilotis: 0.5 m posts 2.5 m apart at 5 m, then the far platform
    (-18.75, -18.25, 129.75, 130.25, 5.0),
    (-16.25, -15.75, 129.75, 130.25, 5.0),
    (-13.75, -13.25, 129.75, 130.25, 5.0),
    (-11.0, -7.0, 128.0, 132.0, 5.0),
    // the swing bars' platforms (1.2 m)
    (6.5, 9.5, 128.5, 131.5, 1.2),
    (22.0, 25.0, 128.5, 131.5, 1.2),
    // the climb tower (face x 35, roof 9.6 m), climb bands on its -X face
    (35.0, 41.0, 127.0, 133.0, 9.6),
];

/// The beam (p0, p1 on its top centre line): between the second roof (x -77) and its platform (x -71).
pub const BEAMS: &[(Vec3, Vec3)] = &[(Vec3::new(-77.0, 4.0, 130.0), Vec3::new(-71.0, 4.0, 130.0))];

/// Swing bars across the run (x, top y, z range): Pole edges, 3.5 m apart.
pub const BARS: &[(f32, f32, (f32, f32))] = &[(12.0, 3.4, (128.5, 131.5)), (15.5, 3.4, (128.5, 131.5)), (19.0, 3.4, (128.5, 131.5))];

/// The ladder on the platform's -X face.
pub const LADDERS: &[(Vec3, Vec3, Vec3)] = &[(Vec3::new(-25.0, 0.0, 130.0), Vec3::new(-25.0, 5.0, 130.0), Vec3::NEG_X)];

/// Climb bands on the tower's -X face (face x, z range, band index range: heights 0.6·k).
pub const CLIMB_X: (f32, (f32, f32), (i32, i32)) = (35.0, (127.4, 132.6), (1, 15));
const BAND_DEPTH: f32 = 0.08;

/// The haystack 4.5 m off the tower's +X face (centre x, centre z, size, height).
pub const HAYSTACKS: &[(f32, f32, f32, f32)] = &[(45.5, 130.0, 2.2, 1.5)];

/// Adds the course to the level's collision and guidance (after the greybox, in the same order every time).
pub fn add(collision: &mut CollisionWorld, guidance: &mut GuidanceWorld) {
    for &(x0, x1, z0, z1, h) in BLOCKS {
        let (min, max) = (Vec3::new(x0, 0.0, z0), Vec3::new(x1, h, z1));
        collision.boxes.push(Aabb3 { min, max });
        if h >= 0.5 {
            crate::level::add_roof_edges(guidance, min, max);
        }
    }
    for &(p0, p1) in BEAMS {
        let (lo, hi) = (p0.min(p1), p0.max(p1));
        collision.boxes.push(Aabb3 { min: Vec3::new(lo.x - 0.1, lo.y - 0.2, lo.z - 0.1), max: Vec3::new(hi.x + 0.1, hi.y, hi.z + 0.1) });
        guidance.edges.push(GuidanceEdge { p0, p1, n0: Vec3::Y, n1: Vec3::Y, subtype: GuidanceSubType::Beam });
        let d = Vec3::new(p1.x - p0.x, 0.0, p1.z - p0.z).normalize_or_zero();
        let side = Vec3::new(-d.z, 0.0, d.x);
        for n in [side, -side] {
            guidance.edges.push(GuidanceEdge { p0: p0 + n * 0.1, p1: p1 + n * 0.1, n0: Vec3::Y, n1: n, subtype: GuidanceSubType::LedgeGrab });
        }
    }
    for &(x, top, (z0, z1)) in BARS {
        collision.boxes.push(Aabb3 { min: Vec3::new(x - 0.1, top - 0.2, z0), max: Vec3::new(x + 0.1, top, z1) });
        for (xx, n) in [(x - 0.05, Vec3::NEG_X), (x + 0.05, Vec3::X)] {
            guidance.edges.push(GuidanceEdge { p0: Vec3::new(xx, top, z0), p1: Vec3::new(xx, top, z1), n0: Vec3::Y, n1: n, subtype: GuidanceSubType::Pole });
        }
    }
    for &(p0, p1, n) in LADDERS {
        guidance.edges.push(GuidanceEdge { p0, p1, n0: Vec3::Y, n1: n, subtype: GuidanceSubType::Ladder });
    }
    let (fx, (z0, z1), (k0, k1)) = CLIMB_X;
    for k in k0..=k1 {
        let y = 0.6 * k as f32;
        let x = fx - BAND_DEPTH;
        guidance.edges.push(GuidanceEdge { p0: Vec3::new(x, y, z0), p1: Vec3::new(x, y, z1), n0: Vec3::Y, n1: Vec3::NEG_X, subtype: GuidanceSubType::LedgeGrab });
    }
    for &(x, z, s, h) in HAYSTACKS {
        guidance.haystacks.push(Aabb3 { min: Vec3::new(x - s * 0.5, 0.0, z - s * 0.5), max: Vec3::new(x + s * 0.5, h, z + s * 0.5) });
    }
}

/// The course's meshes (the greybox's materials and colours).
pub fn spawn(commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) {
    let wall = materials.add(StandardMaterial { base_color: Color::srgb(0.86, 0.80, 0.68), perceptual_roughness: 0.9, ..default() });
    let wood = materials.add(StandardMaterial { base_color: Color::srgb(0.45, 0.32, 0.2), perceptual_roughness: 0.9, ..default() });
    let band = materials.add(StandardMaterial { base_color: Color::srgb(0.62, 0.55, 0.45), ..default() });
    let hay = materials.add(StandardMaterial { base_color: Color::srgb(0.85, 0.72, 0.30), perceptual_roughness: 1.0, ..default() });
    let mut cube = |size: Vec3, at: Vec3, mat: &Handle<StandardMaterial>| {
        commands.spawn((crate::map_menu::MapEntity, Mesh3d(meshes.add(Cuboid::new(size.x, size.y, size.z))), MeshMaterial3d(mat.clone()), Transform::from_translation(at)));
    };
    for &(x0, x1, z0, z1, h) in BLOCKS {
        cube(Vec3::new(x1 - x0, h, z1 - z0), Vec3::new((x0 + x1) * 0.5, h * 0.5, (z0 + z1) * 0.5), &wall);
    }
    for &(p0, p1) in BEAMS {
        let (lo, hi) = (p0.min(p1), p0.max(p1));
        cube(Vec3::new(hi.x - lo.x + 0.2, 0.2, hi.z - lo.z + 0.2), (lo + hi) * 0.5 - Vec3::Y * 0.1, &wood);
    }
    for &(x, top, (z0, z1)) in BARS {
        cube(Vec3::new(0.2, 0.2, z1 - z0), Vec3::new(x, top - 0.1, (z0 + z1) * 0.5), &wood);
    }
    for &(p0, p1, n) in LADDERS {
        // rails and rungs, 0.1 m off the wall
        let len = p0.distance(p1);
        let side = n.cross(Vec3::Y).normalize_or_zero();
        let mid = (p0 + p1) * 0.5 + n * 0.1;
        for s in [-0.25, 0.25] {
            cube(Vec3::new(0.06, len, 0.06), mid + side * s, &wood);
        }
        let mut y = 0.5;
        while y < len {
            cube(Vec3::new(0.06, 0.05, 0.5).abs(), Vec3::new(p0.x + n.x * 0.1, p0.y + y, p0.z + n.z * 0.1), &wood);
            y += 0.5;
        }
    }
    let (fx, (z0, z1), (k0, k1)) = CLIMB_X;
    for k in k0..=k1 {
        let y = 0.6 * k as f32;
        cube(Vec3::new(BAND_DEPTH, 0.08, z1 - z0), Vec3::new(fx - BAND_DEPTH * 0.5, y - 0.04, (z0 + z1) * 0.5), &band);
    }
    for &(x, z, s, h) in HAYSTACKS {
        cube(Vec3::new(s, h, s), Vec3::new(x, h * 0.5, z), &hay);
    }
}
