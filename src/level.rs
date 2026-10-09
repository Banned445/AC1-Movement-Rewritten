//! Greybox test level: rooftops with gaps sized around the game's jump bands (RE/01 §7b),
//! a tall tower for fall-damage tests and a few low obstacles for step-up.
//! Placeholder until real level geometry is loaded from .forge.

use bevy::prelude::*;

use crate::collision::{Aabb3, CollisionWorld};
use crate::guidance::{GuidanceEdge, GuidanceSubType, GuidanceWorld};

pub struct LevelPlugin;

impl Plugin for LevelPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.62, 0.72, 0.82)))
            // sky fill light: with Bevy's default (80) every face turned from the sun renders near-black, and the
            // baked folds of Altaïr's robe texture read as dark blotches
            .insert_resource(GlobalAmbientLight { color: Color::srgb(0.80, 0.86, 1.0), brightness: 1500.0, ..default() });
    }
}

/// (centre x, centre z, size x, size z, height)
const BUILDINGS: &[(f32, f32, f32, f32, f32)] = &[
    // a row of rooftops with gaps of 2, 3.5, 5 and 6.5 m
    (0.0, 12.0, 6.0, 6.0, 3.0),
    (8.0, 12.0, 6.0, 6.0, 3.5),
    (17.5, 12.0, 6.0, 6.0, 3.0),
    (28.5, 12.0, 6.0, 6.0, 4.0),
    (41.0, 12.0, 6.0, 6.0, 3.0),
    // stepped heights going up by ~1.2 m (just under the 1.3 m max-up of ground jumps)
    (0.0, 24.0, 5.0, 5.0, 2.0),
    (7.0, 24.0, 5.0, 5.0, 3.2),
    (14.0, 24.0, 5.0, 5.0, 4.4),
    (21.0, 24.0, 5.0, 5.0, 5.6),
    // a high block (too high to jump up to) and drops for landing tests
    (30.0, 26.0, 6.0, 6.0, 9.5),
    (-12.0, 4.0, 5.0, 5.0, 6.0),   // 6 m drop: heavy-damage threshold is 6.3 m
    (-12.0, 14.0, 5.0, 5.0, 8.0),  // 8 m drop: fatal (> 7.0 m)
    (-12.0, 24.0, 5.0, 5.0, 3.5),  // 3.5 m drop: roll (> 3 m)
    // low obstacles (step-up) and a wall
    (6.0, 0.0, 1.5, 1.5, 0.3),
    (9.0, 0.0, 1.5, 1.5, 0.6),
    (0.0, -8.0, 14.0, 0.6, 2.4),
    (40.0, 4.0, 4.0, 0.3, 1.0),  // a 1 m railing, 0.3 m thick: run and jump at it to vault it (pass-over, RE/04 §4.1.13)
    (30.0, -4.0, 4.0, 0.4, 1.1), // a 1.1 m wall: run into it to lean on it (ObstacleCollision, RE/02 §4.2)
    // --- stage 2: climbing ---
    (-20.0, 30.0, 6.0, 6.0, 9.6),  // climb tower: hold bands on its -Z face (CLIMB_FACE)
    (12.0, 42.0, 8.0, 0.6, 2.6),   // jump-up wall: top edge 2.6 m (ledge band max up 3.0 m)
    (16.0, 41.1, 1.0, 1.2, 3.5),   // pillar at the wall's +X end: blocks shimmy (inner corner)
    (22.0, -20.0, 4.0, 0.6, 2.6),  // wall G (x 20..24): hang at 2.6 m with no holds below …
    (22.0, -19.6, 4.0, 0.8, 7.0),  // … under tower G2, set back 0.3 m, with climb holds (JUMP_CLIMB_HOLDS)
    (28.0, -18.75, 4.0, 0.7, 2.4), // wall H (x 26..30, face z -19.1): the wall-hang ledge under overhang H (SLABS)
    // --- ledge moves (RE/03 §7.6) ---
    (33.0, 50.0, 6.0, 0.6, 2.6),   // L-wall, part A (x 30..36): hang on its -Z face …
    (36.3, 47.15, 0.6, 6.3, 4.0),  // … part B rises above it at x 36, with a stone ledge at 2.6 m (WALL_LEDGES)
    (20.0, 50.0, 6.0, 0.6, 2.6),   // side-jump wall C (x 17..23) …
    (25.5, 50.0, 3.0, 0.6, 2.6),   // … and D (x 24..27): a 1 m gap in the same edge line
    (42.0, 50.0, 4.0, 0.6, 2.6),   // hop-up wall E (x 40..44): hang at 2.6 m …
    (42.0, 50.15, 4.0, 0.3, 4.2),  // … with a ledge 1.6 m higher, set back 0.3 m (E2)
    // --- standing straight jump bands (0xB21DA0) ---
    (50.0, 50.3, 3.0, 1.0, 1.6),   // knee height: jump, hangknee, stand on top
    (56.0, 50.3, 3.0, 1.0, 2.2),   // 2.0–2.5 m with a wall below: jump into a wall hang
    // --- hang-type switch (0xDE1060): wall F (x 60..63) continues as an overhang slab (SLABS) with no wall below
    (61.5, 50.0, 3.0, 0.6, 2.6),
    // --- wall run (Walling, RE/05 §1): faces at z 59.25, run at them along +Z ---
    (70.0, 60.0, 3.0, 1.5, 1.8), // probe A: pull-up onto the top from the entry
    (76.0, 60.0, 3.0, 1.5, 3.8), // the vertical step, then probe C: hang from the top edge
    (82.0, 60.0, 3.0, 1.5, 6.0), // no ledge in reach: vertical end, drop back
    // --- beam (NarrowObject, RE/05 §2): two 4 m platforms joined by a 6 m beam at 4 m (BEAMS) ---
    (70.0, 70.0, 4.0, 4.0, 4.0),
    (80.0, 70.0, 4.0, 4.0, 4.0),
    // platform C (x 98..102) and a beam from it into wall Y (x 109..112, 8 m): the wall run from a beam (RE/05 §2.10)
    (100.0, 70.0, 4.0, 4.0, 4.0),
    (110.5, 70.0, 3.0, 4.0, 8.0),
    // --- hang → ladder side jump (0xDD55F0, RE/18 §7.1): a 2.6 m wall (x 140..143) whose ledge ends 1 m before a
    //     ladder wall (x 144..147, ladder at x 144.4 on its -Z face z 9.7)
    (141.5, 10.0, 3.0, 0.6, 2.6),
    (145.5, 10.0, 3.0, 0.6, 6.0),
    // --- ledge → climb side jump (0xDDD490 type 0, RE/18 §7.2): a 2.6 m wall (x -140..-137) whose ledge ends 1.6 m
    //     before a climb wall (x -135.4..-131.4, face z -135.3, bands to 5.4 m: CLIMB_WALLS), beyond the side move's reach
    (-138.5, -135.0, 3.0, 0.6, 2.6),
    (-133.4, -135.0, 4.0, 0.6, 6.0),
    // --- the climb's ledge grab (0xDF0980, RE/18 §7.3): a 7 m wall (x -130..-126, face z -135.3) whose bands start at
    //     3.0 m (CLIMB_WALLS): climbing down to the lowest band hangs from the one above it
    (-128.0, -135.0, 4.0, 0.6, 7.0),
    // --- the climb's side ledge grab (0xDF22C0, RE/18 §7.4): wall Y (x -120..-116, face z -135.5, bands to 6.6 m) and on its
    //     left (+X) block Z, square to the face and 2 m deep toward the climber, top 3.6 m: its top edge is at the
    //     hands' height with nothing for the feet
    (-118.0, -135.0, 4.0, 1.0, 7.0),
    (-115.0, -136.5, 2.0, 2.0, 3.6),
    // --- water tank (x 130..136, z 0..6, walls 2 m): deep water inside (WATER), the drowning (RE/18 §5)
    (133.0, 0.15, 6.0, 0.3, 2.0),
    (133.0, 5.85, 6.0, 0.3, 2.0),
    (130.15, 3.0, 0.3, 5.4, 2.0),
    (135.85, 3.0, 0.3, 5.4, 2.0),
    // --- kiosk (HumanKiosk, RE/18 §4): a 4.5 m roof (x 118..122, z 18..22) with a kiosk frame 3 m out (KIOSKS)
    (120.0, 20.0, 4.0, 4.0, 4.5),
    // --- pilotis (RE/05 §2.8): 0.5 m posts 2.5 m apart between two 3 m platforms, along +X at z 80 ---
    (70.0, 80.0, 4.0, 4.0, 3.0),
    (74.5, 80.0, 0.5, 0.5, 3.0),
    (77.0, 80.0, 0.5, 0.5, 3.0),
    (79.5, 80.0, 0.5, 0.5, 3.0),
    (84.0, 80.0, 4.0, 4.0, 3.0),
    // --- swing bars (RE/03 §7.10): platforms at z 86 / 101 (1.2 m), bars between them (SLABS)
    (60.0, 86.0, 3.0, 3.0, 1.2),
    (60.0, 101.5, 3.0, 3.0, 1.2),
    // --- ladder (RE/05 §4): a 5 m wall at z 64 (x 48..52), the ladder on its -Z face at x 50 (LADDERS)
    (50.0, 64.0, 4.0, 1.0, 5.0),
    // --- climb onto holds that stick out (TryTransitionToLedgeHang 0xDF4BA0): wall K (x -32..-28, face z -10.5)
    //     with a bay above 3.3 m jutting 0.9 m out of it (SLABS); climb bands on both faces (CLIMB_WALLS)
    (-30.0, -10.0, 4.0, 1.0, 7.0),
    // --- climb corner moves onto the ground (flag 4612): wall M (x -42..-38, face z -10.5) with bands to 6.6 m (CLIMB_WALLS);
    //     on its right (the climber faces +Z, so right is -X) block N, flush with the face, top 2.4 m: the grid route
    //     (0xDF4670); on its left block O, square to the face and 2 m deep toward the climber, top 2.4 m: the side
    //     candidates (0xDF28A0), a 90° turn
    (-40.0, -10.0, 4.0, 1.0, 7.0),
    (-43.5, -10.0, 3.0, 1.0, 2.4),
    (-37.0, -11.5, 2.0, 2.0, 2.4),
    // --- the climb's reach to a hang at the side (TryReachOtherSurface 0xDF9B30): wall P (x -60..-56, face z -10.5) with
    //     bands to 6.6 m (CLIMB_WALLS); on its left (+X) slab Q, a free-hang edge at 3.6 m past a gap (SLABS)
    (-58.0, -10.0, 4.0, 1.0, 7.0),
    // --- climb → climb (all faces at z -10.5, the climber faces +Z, left = +X):
    //     reach across a 2 m gap: wall R1 (x -74..-70) and wall R2 (x -68..-65.6), bands on both (CLIMB_WALLS)
    (-72.0, -10.0, 4.0, 1.0, 7.0),
    (-66.8, -10.0, 2.4, 1.0, 7.0),
    //     into an inside corner: wall S (x -84..-80) and wall T square to it on its left, 2.5 m out toward the climber
    //     (x -80..-79, z -13..-9.5), bands on its -X face (CLIMB_WALLS_X)
    (-82.0, -10.0, 4.0, 1.0, 7.0),
    (-79.5, -11.25, 1.0, 3.5, 7.0),
    //     round an outside corner: wall U (x -94..-90), bands round its +X end face (CLIMB_WALLS_X)
    (-92.0, -10.0, 4.0, 1.0, 7.0),
    //     onto a ladder at the side: wall W (x -106..-102) and the ladder wall (x -100.8..-99.2), ladder at x -100 (LADDERS)
    (-104.0, -10.0, 4.0, 1.0, 7.0),
    (-100.0, -10.0, 1.6, 1.0, 7.0),
    //     up across missing holds: wall V (x -118..-114), no band at 3.0 m (CLIMB_WALLS)
    (-116.0, -10.0, 4.0, 1.0, 7.0),
    //     onto a ladder at the side from the climb (TryLadder 0xDF10C0): wall X (x -134..-130) and a ladder wall
    //     (x -130..-128.4), the ladder at x -129.4 (LADDERS), its top blocked by a slab (SLABS)
    (-132.0, -10.0, 4.0, 1.0, 7.0),
    (-129.2, -10.0, 1.6, 1.0, 7.0),
];

/// Ladders (bottom, top on the wall face; outward normal): guidance edges of sub-type Ladder.
pub const LADDERS: &[(Vec3, Vec3, Vec3)] = &[
    (Vec3::new(50.0, 0.0, 63.5), Vec3::new(50.0, 5.0, 63.5), Vec3::NEG_Z),
    (Vec3::new(-100.0, 0.0, -10.5), Vec3::new(-100.0, 7.0, -10.5), Vec3::NEG_Z),
    (Vec3::new(-129.4, 0.0, -10.5), Vec3::new(-129.4, 7.0, -10.5), Vec3::NEG_Z),
    (Vec3::new(144.4, 0.0, 9.7), Vec3::new(144.4, 6.0, 9.7), Vec3::NEG_Z),
];

/// Beams (p0, p1 on the top centre line; 0.2 m wide, 0.2 m thick): solid, and guidance edges of sub-type Beam.
pub const BEAMS: &[(Vec3, Vec3)] = &[
    (Vec3::new(72.0, 4.0, 70.0), Vec3::new(78.0, 4.0, 70.0)),
    // a free beam 2.5 m past platform B (x 82): reached by a running jump, a ledge 2.3 m above its far part
    (Vec3::new(84.5, 4.0, 70.0), Vec3::new(90.5, 4.0, 70.0)),
    // platform C → wall Y
    (Vec3::new(102.0, 4.0, 70.0), Vec3::new(108.8, 4.0, 70.0)),
    // a branch off it toward +Z at x 105 (0.3 m clear of it): the corner hop (RE/05 §2.11)
    (Vec3::new(105.0, 4.0, 70.3), Vec3::new(105.0, 4.0, 76.0)),
    // the branch bends 30° at z 76: walking on round the bend onto the next segment (0xF753A0)
    (Vec3::new(105.0, 4.0, 76.0), Vec3::new(106.5, 4.0, 78.6)),
];

/// Haystacks (centre x, centre z, size x, size z, height): not solid, jump targets of type 0x800. The first
/// one sits 4.5 m off the high block's +X face (roof 9.5 m): the Leap of Faith test.
pub const HAYSTACKS: &[(f32, f32, f32, f32, f32)] = &[(37.5, 26.0, 2.2, 2.2, 1.5)];

/// Deep water (min, max; the top is the surface): the port's drown trigger (RE/18 §5), not solid.
pub const WATER: &[(Vec3, Vec3)] = &[(Vec3::new(130.3, 0.0, 0.3), Vec3::new(135.7, 1.8, 5.7))];

/// Kiosk frames (p0, p1 of the top bar, the side facing the roofs): guidance subtype Kiosk (8), not solid.
pub const KIOSKS: &[(Vec3, Vec3, Vec3)] = &[(Vec3::new(118.5, 3.0, 25.0), Vec3::new(121.5, 3.0, 25.0), Vec3::NEG_Z)];

/// Floating slabs (centre x, top y, centre z, size x, size z, thickness): free-hang ledges.
const SLABS: &[(f32, f32, f32, f32, f32, f32)] = &[
    // the free-hang balcony: 2.9 m, inside the tap jump's 3.0 m reach zone (JumpZones slot 3, RE/18 §1.2)
    (2.0, 2.9, 36.0, 6.0, 1.2, 0.3),
    (64.5, 2.6, 50.0, 3.0, 0.6, 0.3), // overhang continuing wall F's ledge (hang-type switch test)
    (60.0, 3.4, 90.0, 3.0, 0.2, 0.2), // swing bars 3.5 m apart
    (60.0, 3.4, 93.5, 3.0, 0.2, 0.2),
    (60.0, 3.4, 97.0, 3.0, 0.2, 0.2),
    (89.0, 6.3, 70.0, 2.0, 1.0, 0.3), // above the free beam: the beam's straight jump at a hand target (2.3 m)
    // an overhang 1 m out from the climb tower over its left gap: the climb's jump up to it (TryBackEject 0xDF2F50)
    (-22.0, 3.5, 26.5, 1.2, 1.0, 0.2),
    // overhang H: free hang at its -Z edge (3.6 m), 0.9 m in front of wall H's 2.4 m ledge (TryFreeHangDropToClimb 0xDDF390)
    (28.0, 3.6, -19.2, 4.0, 1.6, 0.2),
    // slab I on wall D's edge line (x 28.6..29.6, top 2.6, no wall below): a long side jump from D lands on one hand
    // and the second hand follows (SecondHandGrab, Ledge state 17)
    (29.1, 2.6, 50.0, 1.0, 0.6, 0.3),
    // the bay above wall K: 3.3..7.0 m, its face 0.9 m out at z -11.4
    (-30.0, 7.0, -10.95, 4.0, 0.9, 3.7),
    // slab Q left of wall P (x -54.2..-53.0, top 3.6, edge on the face line z -10.5, nothing below): a long reach from the
    // climb into a free hang, caught on one hand (`climb1m_tr_hangfree_left_3`, then SecondHandGrab)
    (-53.6, 3.6, -10.2, 1.2, 0.6, 0.3),
    // over the top of the ladder at x -129.4: the top is blocked (sub_E240B0), the climb stops below it
    (-129.2, 8.4, -10.25, 1.6, 1.5, 0.9),
];

/// Further climb walls (x range, face z with normal -Z, band index range: heights 0.6·k): wall K's bands up to 3.0 m
/// and the bay's from 3.6 m, 0.9 m further out, so a hand step up reaches holds that stick out of the wall.
const CLIMB_WALLS: &[((f32, f32), f32, (i32, i32))] = &[
    ((-31.6, -28.4), -10.5, (1, 5)),
    ((-31.6, -28.4), -11.4, (6, 11)),
    // wall M, between blocks N and O
    ((-41.95, -38.05), -10.5, (1, 11)),
    // wall P, the climb's reach to slab Q
    ((-59.95, -56.05), -10.5, (1, 11)),
    // walls R1 / R2, the reach across the gap
    ((-73.95, -70.05), -10.5, (1, 11)),
    ((-67.95, -65.65), -10.5, (1, 11)),
    // wall S, stopping short of the corner with wall T
    ((-83.95, -80.4), -10.5, (1, 11)),
    // wall U
    ((-93.95, -90.05), -10.5, (1, 11)),
    // wall W, beside the ladder
    ((-105.95, -102.05), -10.5, (1, 11)),
    // wall V: bands to 2.4 m, none at 3.0 m, then 3.6 m up
    ((-117.95, -114.05), -10.5, (1, 4)),
    ((-117.95, -114.05), -10.5, (6, 11)),
    // wall X, its holds ending 0.65 m short of the ladder at x -129.4
    ((-133.95, -130.05), -10.5, (1, 11)),
    // the climb wall beside the 2.6 m ledge (the ledge side jump onto climb holds)
    ((-135.35, -131.45), -135.3, (1, 9)),
    // the climb's ledge grab: bands from 3.0 m only
    ((-129.95, -126.05), -135.3, (5, 11)),
    // wall Y, the side ledge grab onto block Z
    ((-119.95, -116.05), -135.5, (1, 11)),
];

/// Climb walls facing ±X (face x, z range, outward normal x, band index range): the corner climbs.
const CLIMB_WALLS_X: &[(f32, (f32, f32), f32, (i32, i32))] = &[
    // wall T's -X face, from the inside corner with wall S out toward the climber
    (-80.0, (-12.95, -10.6), -1.0, (1, 11)),
    // wall U's +X end face
    (-90.0, (-10.45, -9.55), 1.0, (1, 11)),
];

/// Extra ledges on wall faces (p0, p1, outward normal): stone ledges that are not roof edges.
const WALL_LEDGES: &[(Vec3, Vec3, Vec3)] = &[
    // along part B's -X face, meeting part A's ledge at the inner corner (x 36, z 49.7)
    (Vec3::new(35.92, 2.6, 44.0), Vec3::new(35.92, 2.6, 49.7), Vec3::NEG_X),
];

/// Climb tower face: x range, face z (normal -Z), band heights 0.6 m apart, plus a missing patch.
const CLIMB_FACE: (f32, f32, f32) = (-22.6, -17.4, 27.0);
const CLIMB_BANDS: std::ops::RangeInclusive<i32> = 1..=15; // 0.6 .. 9.0 m
/// How far the stone bands stick out of the tower face.
const CLIMB_BAND_DEPTH: f32 = 0.08;
/// Missing holds (x range, y range): the right one tests blocked grid moves (look-around); the left one sits under an
/// overhang slab (SLABS) for the climb's jump up to it (TryBackEject 0xDF2F50). The right one is 1.8 m wide: the climb's
/// reach up (0xDE8400) tries feet up to 0.6 m to either side and hands 0.375 m beyond, so a narrower gap is reached past.
const CLIMB_GAPS: &[((f32, f32), (f32, f32))] = &[((-19.2, -17.4), (2.9, 4.3)), ((-22.6, -21.4), (2.9, 4.3))];

/// Tower G2's climb holds (x range, face z, band heights): the jump up from a hang on wall G (2.6 m) to climb holds
/// (TryJumpUpToClimb 0xDD5E10). Hands land on the 4.1 m band (1.5 m up: beyond a hand step), feet on the 2.9 m one.
/// There is no 3.5 m band, which a hand step from the hang would reach first in the port (it does not tell climb
/// holds from ledges; the game's searches use different guidance filters).
const JUMP_CLIMB_HOLDS: ((f32, f32), f32, &[f32]) = ((20.0, 24.0), -20.0, &[2.9, 4.1, 4.7, 5.3, 5.9, 6.5]);

/// The x ranges of the band at height y, with the gaps cut out.
fn band_segments(y: f32) -> Vec<(f32, f32)> {
    let (x0, x1, _) = CLIMB_FACE;
    let mut segs = vec![(x0, x1)];
    for &((gx0, gx1), (gy0, gy1)) in CLIMB_GAPS {
        if (gy0..=gy1).contains(&y) {
            segs = segs.into_iter().flat_map(|(a, b)| [(a, b.min(gx0)), (a.max(gx1), b)]).filter(|(a, b)| b - a > 1e-3).collect();
        }
    }
    segs
}

/// Pure level data (collision + guidance), shared by the renderer and the simulation tests.
pub fn geometry() -> (CollisionWorld, GuidanceWorld) {
    let mut collision = CollisionWorld::default();
    let mut guidance = GuidanceWorld::default();
    collision.boxes.push(Aabb3 { min: Vec3::new(-150.0, -1.0, -150.0), max: Vec3::new(150.0, 0.0, 150.0) });
    for &(x, z, sx, sz, h) in BUILDINGS {
        let min = Vec3::new(x - sx * 0.5, 0.0, z - sz * 0.5);
        let max = Vec3::new(x + sx * 0.5, h, z + sz * 0.5);
        collision.boxes.push(Aabb3 { min, max });
        // grab edges from 0.5 m up: the 0.6 m box is a free-step step-up (0xB21DA0 < 0.7 m band); the 0.3 m step
        // is walked onto (step offset 0.37 m)
        if h >= 0.5 {
            add_roof_edges(&mut guidance, min, max);
        }
    }
    for &(x, top, z, sx, sz, t) in SLABS {
        let min = Vec3::new(x - sx * 0.5, top - t, z - sz * 0.5);
        let max = Vec3::new(x + sx * 0.5, top, z + sz * 0.5);
        collision.boxes.push(Aabb3 { min, max });
        add_roof_edges(&mut guidance, min, max);
    }
    for &(p0, p1, n) in LADDERS {
        guidance.edges.push(GuidanceEdge { p0, p1, n0: Vec3::Y, n1: n, subtype: GuidanceSubType::Ladder });
    }
    for &(p0, p1) in BEAMS {
        let (lo, hi) = (p0.min(p1), p0.max(p1));
        collision.boxes.push(Aabb3 { min: Vec3::new(lo.x - 0.1, lo.y - 0.2, lo.z - 0.1), max: Vec3::new(hi.x + 0.1, hi.y, hi.z + 0.1) });
        guidance.edges.push(GuidanceEdge { p0, p1, n0: Vec3::Y, n1: Vec3::Y, subtype: GuidanceSubType::Beam });
        // PORT: the beam's two top side edges are LedgeGrab edges too, like a pilotis top (0xB2B600); a beam's own
        // guidance data in the game files is not checked. The pull-down from a beam (0xE504D0) hangs from them.
        let d = Vec3::new(p1.x - p0.x, 0.0, p1.z - p0.z).normalize_or_zero();
        let side = Vec3::new(-d.z, 0.0, d.x);
        for n in [side, -side] {
            guidance.edges.push(GuidanceEdge { p0: p0 + n * 0.1, p1: p1 + n * 0.1, n0: Vec3::Y, n1: n, subtype: GuidanceSubType::LedgeGrab });
        }
    }
    for &(min, max) in WATER {
        guidance.water.push(Aabb3 { min, max });
    }
    for &(p0, p1, n) in KIOSKS {
        guidance.edges.push(GuidanceEdge { p0, p1, n0: Vec3::Y, n1: n, subtype: GuidanceSubType::Kiosk });
    }
    for &(x, z, sx, sz, h) in HAYSTACKS {
        guidance.haystacks.push(Aabb3 { min: Vec3::new(x - sx * 0.5, 0.0, z - sz * 0.5), max: Vec3::new(x + sx * 0.5, h, z + sz * 0.5) });
    }
    for &(p0, p1, n1) in WALL_LEDGES {
        guidance.edges.push(GuidanceEdge { p0, p1, n0: Vec3::Y, n1, subtype: GuidanceSubType::LedgeGrab });
    }
    // climbing holds: horizontal stone bands on the tower face, split at the gap
    let (_, _, fz) = CLIMB_FACE;
    for k in CLIMB_BANDS {
        let y = 0.6 * k as f32;
        for (a, b) in band_segments(y) {
            guidance.edges.push(GuidanceEdge {
                // holds are the bands' outer top edges (the band sticks out CLIMB_BAND_DEPTH from the face)
                p0: Vec3::new(a, y, fz - CLIMB_BAND_DEPTH),
                p1: Vec3::new(b, y, fz - CLIMB_BAND_DEPTH),
                n0: Vec3::Y,
                n1: Vec3::NEG_Z,
                subtype: GuidanceSubType::LedgeGrab,
            });
        }
    }
    for &((a, b), fz, (k0, k1)) in CLIMB_WALLS {
        for k in k0..=k1 {
            let y = 0.6 * k as f32;
            guidance.edges.push(GuidanceEdge {
                p0: Vec3::new(a, y, fz - CLIMB_BAND_DEPTH),
                p1: Vec3::new(b, y, fz - CLIMB_BAND_DEPTH),
                n0: Vec3::Y,
                n1: Vec3::NEG_Z,
                subtype: GuidanceSubType::LedgeGrab,
            });
        }
    }
    for &(fx, (a, b), nx, (k0, k1)) in CLIMB_WALLS_X {
        for k in k0..=k1 {
            let y = 0.6 * k as f32;
            let x = fx + nx * CLIMB_BAND_DEPTH;
            guidance.edges.push(GuidanceEdge {
                p0: Vec3::new(x, y, a),
                p1: Vec3::new(x, y, b),
                n0: Vec3::Y,
                n1: Vec3::new(nx, 0.0, 0.0),
                subtype: GuidanceSubType::LedgeGrab,
            });
        }
    }
    let ((a, b), fz, rows) = JUMP_CLIMB_HOLDS;
    for &y in rows {
        guidance.edges.push(GuidanceEdge {
            p0: Vec3::new(a, y, fz - CLIMB_BAND_DEPTH),
            p1: Vec3::new(b, y, fz - CLIMB_BAND_DEPTH),
            n0: Vec3::Y,
            n1: Vec3::NEG_Z,
            subtype: GuidanceSubType::LedgeGrab,
        });
    }
    (collision, guidance)
}

pub(crate) fn build_level(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut collision: ResMut<CollisionWorld>,
    mut guidance: ResMut<GuidanceWorld>,
) {
    let (c, g) = geometry();
    *collision = c;
    *guidance = g;

    let ground_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.78, 0.70, 0.55),
        perceptual_roughness: 0.95,
        ..default()
    });
    commands.spawn((crate::map_menu::MapEntity,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(300.0, 300.0))),
        MeshMaterial3d(ground_mat),
    ));
    let wall_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.86, 0.80, 0.68),
        perceptual_roughness: 0.9,
        ..default()
    });
    for &(x, z, sx, sz, h) in BUILDINGS {
        commands.spawn((crate::map_menu::MapEntity,
            Mesh3d(meshes.add(Cuboid::new(sx, h, sz))),
            MeshMaterial3d(wall_mat.clone()),
            Transform::from_xyz(x, h * 0.5, z),
        ));
    }
    for &(x, top, z, sx, sz, t) in SLABS {
        commands.spawn((crate::map_menu::MapEntity,
            Mesh3d(meshes.add(Cuboid::new(sx, t, sz))),
            MeshMaterial3d(wall_mat.clone()),
            Transform::from_xyz(x, top - t * 0.5, z),
        ));
    }
    let hay_mat = materials.add(StandardMaterial { base_color: Color::srgb(0.85, 0.72, 0.30), perceptual_roughness: 1.0, ..default() });
    for &(x, z, sx, sz, h) in HAYSTACKS {
        commands.spawn((crate::map_menu::MapEntity,Mesh3d(meshes.add(Cuboid::new(sx, h, sz))), MeshMaterial3d(hay_mat.clone()), Transform::from_xyz(x, h * 0.5, z)));
    }
    let water_mat = materials.add(StandardMaterial { base_color: Color::srgba(0.15, 0.35, 0.55, 0.6), alpha_mode: AlphaMode::Blend, perceptual_roughness: 0.2, ..default() });
    for &(min, max) in WATER {
        let size = max - min;
        commands.spawn((crate::map_menu::MapEntity, Mesh3d(meshes.add(Cuboid::new(size.x, size.y, size.z))), MeshMaterial3d(water_mat.clone()), Transform::from_translation((min + max) * 0.5)));
    }
    // kiosk frames: the top bar only (PORT: the greybox draws no awning or posts; not solid)
    let kiosk_mat = materials.add(StandardMaterial { base_color: Color::srgb(0.55, 0.35, 0.2), perceptual_roughness: 0.9, ..default() });
    for &(p0, p1, _) in KIOSKS {
        let len = p0.distance(p1);
        let rot = Quat::from_rotation_arc(Vec3::X, (p1 - p0).normalize_or(Vec3::X));
        commands.spawn((crate::map_menu::MapEntity, Mesh3d(meshes.add(Cuboid::new(len, 0.08, 0.08))), MeshMaterial3d(kiosk_mat.clone()), Transform::from_translation((p0 + p1) * 0.5).with_rotation(rot)));
    }
    let beam_mat = materials.add(StandardMaterial { base_color: Color::srgb(0.45, 0.32, 0.2), perceptual_roughness: 0.9, ..default() });
    for &(p0, p1) in BEAMS {
        let (lo, hi) = (p0.min(p1), p0.max(p1));
        let size = Vec3::new(hi.x - lo.x + 0.2, 0.2, hi.z - lo.z + 0.2);
        commands.spawn((crate::map_menu::MapEntity,Mesh3d(meshes.add(Cuboid::new(size.x, size.y, size.z))), MeshMaterial3d(beam_mat.clone()), Transform::from_translation((lo + hi) * 0.5 - Vec3::Y * 0.1)));
    }
    // visual stone bands on the climb tower
    let band_mat = materials.add(StandardMaterial { base_color: Color::srgb(0.62, 0.55, 0.45), ..default() });
    let (_, _, fz) = CLIMB_FACE;
    for k in CLIMB_BANDS {
        let y = 0.6 * k as f32;
        for (a, b) in band_segments(y) {
            commands.spawn((crate::map_menu::MapEntity,
                Mesh3d(meshes.add(Cuboid::new(b - a, 0.08, CLIMB_BAND_DEPTH))),
                MeshMaterial3d(band_mat.clone()),
                Transform::from_xyz((a + b) * 0.5, y - 0.04, fz - CLIMB_BAND_DEPTH * 0.5),
            ));
        }
    }
    for &((a, b), fz, (k0, k1)) in CLIMB_WALLS {
        for k in k0..=k1 {
            let y = 0.6 * k as f32;
            commands.spawn((crate::map_menu::MapEntity,
                Mesh3d(meshes.add(Cuboid::new(b - a, 0.08, CLIMB_BAND_DEPTH))),
                MeshMaterial3d(band_mat.clone()),
                Transform::from_xyz((a + b) * 0.5, y - 0.04, fz - CLIMB_BAND_DEPTH * 0.5),
            ));
        }
    }
    for &(fx, (a, b), nx, (k0, k1)) in CLIMB_WALLS_X {
        for k in k0..=k1 {
            let y = 0.6 * k as f32;
            commands.spawn((crate::map_menu::MapEntity,
                Mesh3d(meshes.add(Cuboid::new(CLIMB_BAND_DEPTH, 0.08, b - a))),
                MeshMaterial3d(band_mat.clone()),
                Transform::from_xyz(fx + nx * CLIMB_BAND_DEPTH * 0.5, y - 0.04, (a + b) * 0.5),
            ));
        }
    }
    let ((a, b), fz, rows) = JUMP_CLIMB_HOLDS;
    for &y in rows {
        commands.spawn((crate::map_menu::MapEntity,
            Mesh3d(meshes.add(Cuboid::new(b - a, 0.08, CLIMB_BAND_DEPTH))),
            MeshMaterial3d(band_mat.clone()),
            Transform::from_xyz((a + b) * 0.5, y - 0.04, fz - CLIMB_BAND_DEPTH * 0.5),
        ));
    }

    // light
    commands.spawn((crate::map_menu::MapEntity,
        DirectionalLight { illuminance: 12_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(30.0, 60.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Four LedgeGrab edges around a roof: n0 = up (roof), n1 = outward wall normal.
fn add_roof_edges(g: &mut GuidanceWorld, min: Vec3, max: Vec3) {
    let y = max.y;
    let c = [
        Vec3::new(min.x, y, min.z),
        Vec3::new(max.x, y, min.z),
        Vec3::new(max.x, y, max.z),
        Vec3::new(min.x, y, max.z),
    ];
    let normals = [Vec3::NEG_Z, Vec3::X, Vec3::Z, Vec3::NEG_X];
    for i in 0..4 {
        g.edges.push(GuidanceEdge {
            p0: c[i],
            p1: c[(i + 1) % 4],
            n0: Vec3::Y,
            n1: normals[i],
            subtype: GuidanceSubType::LedgeGrab,
        });
    }
}
