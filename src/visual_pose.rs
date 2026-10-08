//! Authored bone modifiers on Altaïr's skeletons (RE/09 §8.6–8.17): the skirt, hood and sword tag, and the
//! main skeleton's hinges, compressions, roll bones and spring box. SkeletonComponent__RunModifiers 0x4E6820.
use bevy::prelude::*;
use crate::assets::ac_formats::parse_skeleton;

// The player's skeleton runs its authored modifiers in gameplay (LOD 5, +332 bit 0x10; RE/09 §8.16).
// AC_NO_SKIRT_ROTATION_COPIES=1 compares the old rest-pose skirt.
pub const SKIRT_ROTATION_COPIES: bool = true;
// Authored hood/sword-tag modifiers, independently fenced from the skirt (RE/09 §8.13).
const CHARACTER_EQUIPMENT_DYNAMICS: bool = true;

pub fn skirt_modifiers_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| (SKIRT_ROTATION_COPIES || std::env::var_os("AC_SKIRT_ROTATION_COPIES").is_some()) && std::env::var_os("AC_NO_SKIRT_ROTATION_COPIES").is_none())
}

fn equipment_modifiers_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("AC_CHARACTER_EQUIPMENT_DYNAMICS").map_or(CHARACTER_EQUIPMENT_DYNAMICS, |v| v != "0"))
}

// The main skeleton's own modifiers (UCMA_Altair: hinges, compressions, roll bones, spring box; RE/09 §8.17).
const CHARACTER_BODY_MODIFIERS: bool = true;

fn body_modifiers_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("AC_CHARACTER_BODY_MODIFIERS").map_or(CHARACTER_BODY_MODIFIERS, |v| v != "0"))
}

/// Which switch fences a modifier: the resource it was authored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group { Skirt, Equipment, Body }

impl Group {
    fn enabled(self) -> bool {
        match self { Group::Skirt => skirt_modifiers_enabled(), Group::Equipment => equipment_modifiers_enabled(), Group::Body => body_modifiers_enabled() }
    }
}

#[derive(Clone, Debug)]
pub struct SkirtCompression {
    pub group: Group,
    pub target: usize,
    pub sources: [usize; 2],
    pub position_weight: f32,
    pub rotation_weight: f32,
    pub position: bool,
    pub rotation: bool,
    /// Per-source rotation and position offsets (+32/+48 and +64/+80), composed as q_src·q_off and p + q_src·p_off.
    pub rotation_offsets: [Quat; 2],
    pub position_offsets: [Vec3; 2],
}

#[derive(Clone, Debug)]
pub struct SkirtLookAt {
    pub group: Group,
    pub target: usize,
    pub aim: usize,
    pub aim_axis: usize,
}

/// Rank 9's null-reference, zero-offset skirt Z/Y and hood X/Z look-at (0x648B90).
pub fn decode_look_at(data: &[u8]) -> Result<Vec<SkirtLookAt>, String> {
    let bones = parse_skeleton(data);
    let word = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let reference = |p| {
        if data.get(p) != Some(&2) { return None; }
        let id = word(p + 1)?;
        bones.iter().find(|b| b.object_id == id).map(|b| b.bone_id as usize)
    };
    let class = crate::assets::forge::crc32("LookAtBoneModifier");
    let mut result = Vec::new();
    for p in 5..data.len().saturating_sub(4) {
        if data[p - 5] != 0 || word(p) != Some(class) { continue; }
        let decode = || {
            let b = p + 4;
            let target = reference(b)?;
            // PORT: reject other layouts rather than inventing fallback/axis conventions.
            if !matches!(data.get(b + 5), Some(0 | 1)) || data.get(b + 6) != Some(&3) { return None; }
            let aim = reference(b + 7)?;
            let aim_axis = word(b + 28)? as usize;
            if target == aim || (0..4).any(|k| word(b + 12 + k * 4) != Some(0))
                || !matches!((aim_axis, word(b + 32)?), (2, 1) | (0, 2)) { return None; }
            Some(SkirtLookAt { group: Group::Skirt, target, aim, aim_axis })
        };
        result.push(decode().ok_or("unsupported skirt look-at layout")?);
    }
    Ok(result)
}

/// CompressBoneModifier__Read 0x6C4B50: owner, weights, two sources, two quaternion and two position offsets, flags.
pub fn decode_compressions(data: &[u8]) -> Result<Vec<SkirtCompression>, String> {
    let bones = parse_skeleton(data);
    let word = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let scalar = |p| word(p).map(f32::from_bits).filter(|v| v.is_finite());
    let reference = |p| {
        if data.get(p) != Some(&2) { return None; }
        let id = word(p + 1)?;
        bones.iter().find(|b| b.object_id == id).map(|b| b.bone_id as usize)
    };
    let class = crate::assets::forge::crc32("CompressBoneModifier");
    let mut result = Vec::new();
    for p in 5..data.len().saturating_sub(4) {
        if data[p - 5] != 0 || word(p) != Some(class) { continue; }
        let decode = || {
            let b = p + 4;
            let target = reference(b)?;
            let sources = [reference(b + 14)?, reference(b + 19)?];
            let position_weight = scalar(b + 6)?;
            let rotation_weight = scalar(b + 10)?;
            let vec4 = |o: usize| Some(Vec4::new(scalar(b + o)?, scalar(b + o + 4)?, scalar(b + o + 8)?, scalar(b + o + 12)?));
            let quat = |o: usize| vec4(o).map(Quat::from_vec4).filter(|q| (q.length() - 1.0).abs() < 1e-3).map(Quat::normalize);
            let rotation_offsets = [quat(24)?, quat(40)?];
            let position_offsets = [vec4(56)?.truncate(), vec4(72)?.truncate()];
            let rotation = *data.get(b + 88)?;
            let position = *data.get(b + 89)?;
            if rotation > 1 || position > 1 || sources.contains(&target) { return None; }
            Some(SkirtCompression { group: Group::Skirt, target, sources, position_weight, rotation_weight, position: position != 0, rotation: rotation != 0,
                rotation_offsets, position_offsets })
        };
        result.push(decode().ok_or("unsupported skirt compression layout")?);
    }
    Ok(result)
}

/// RotationPasteModifier deserialization: owner/base flag 0x697BD0, source 0x5FE7E0.
pub fn decode_rotation_copies(data: &[u8]) -> Result<Vec<(u32, u32)>, String> {
    let bones = parse_skeleton(data);
    let resolve = |id| bones.iter().find(|b| b.object_id == id).map(|b| b.bone_id);
    let word = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let class = crate::assets::forge::crc32("RotationPasteModifier");
    let mut copies = Vec::new();
    for p in 5..data.len().saturating_sub(4) {
        if data[p - 5] != 0 || word(p) != Some(class) { continue; }
        // PORT: this outfit uses external bone references; reject unknown inline/null layouts.
        if data.get(p + 4) != Some(&2) || data.get(p + 10) != Some(&2) {
            return Err("unsupported skirt rotation-copy references".into());
        }
        let owner = word(p + 5).and_then(resolve).ok_or("skirt rotation-copy owner missing")?;
        let source = word(p + 11).and_then(resolve).ok_or("skirt rotation-copy source missing")?;
        if owner == source || copies.iter().any(|(target, _)| *target == owner) {
            return Err("duplicate or cyclic skirt rotation copy".into());
        }
        copies.push((owner, source));
    }
    Ok(copies)
}

/// RollBoneModifier (read 0x6097B0): owner, mode (+20), weight (+24), source (+28).
#[derive(Clone, Debug)]
pub struct RollModifier { pub target: usize, pub source: usize, pub mode: u32, pub weight: f32 }

pub fn decode_rolls(data: &[u8]) -> Result<Vec<RollModifier>, String> {
    let bones = parse_skeleton(data);
    let word = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let reference = |p| {
        if data.get(p) != Some(&2) { return None; }
        let id = word(p + 1)?;
        bones.iter().find(|b| b.object_id == id).map(|b| b.bone_id as usize)
    };
    let class = crate::assets::forge::crc32("RollBoneModifier");
    let mut result = Vec::new();
    for p in 5..data.len().saturating_sub(4) {
        if data[p - 5] != 0 || word(p) != Some(class) { continue; }
        let decode = || {
            let b = p + 4;
            let target = reference(b)?;
            let mode = word(b + 6)?;
            let weight = word(b + 10).map(f32::from_bits).filter(|v| v.is_finite())?;
            let source = reference(b + 14)?;
            (mode <= 3 && source != target).then_some(RollModifier { target, source, mode, weight })
        };
        result.push(decode().ok_or("unsupported roll-bone modifier layout")?);
    }
    Ok(result)
}

/// SpringBoxModifier (read 0x5E84F0): stiffness +80, damping +84, environment share +88, force +32, box +48/+64.
#[derive(Clone, Debug)]
pub struct SpringBox {
    pub target: usize,
    pub stiffness: f32,
    pub damping: f32,
    pub environment: f32,
    pub force: Vec3,
    pub min: Vec3,
    pub max: Vec3,
}

#[derive(Clone, Default)]
pub struct SpringState { pub position: Vec3, pub previous: Vec3, pub anchor: Vec3, pub live: bool }

pub fn decode_spring_boxes(data: &[u8]) -> Result<Vec<SpringBox>, String> {
    let bones = parse_skeleton(data);
    let word = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let scalar = |p| word(p).map(f32::from_bits).filter(|v| v.is_finite());
    let class = crate::assets::forge::crc32("SpringBoxModifier");
    let mut result = Vec::new();
    for p in 5..data.len().saturating_sub(4) {
        if data[p - 5] != 0 || word(p) != Some(class) { continue; }
        let decode = || {
            let b = p + 4;
            if data.get(b) != Some(&2) { return None; }
            let target = bones.iter().find(|bone| Some(bone.object_id) == word(b + 1))?.bone_id as usize;
            let vec3 = |o: usize| Some(Vec3::new(scalar(b + o)?, scalar(b + o + 4)?, scalar(b + o + 8)?));
            let (min, max) = (vec3(34)?, vec3(50)?);
            if min.cmpgt(max).any() { return None; }
            Some(SpringBox { target, stiffness: scalar(b + 6)?, damping: scalar(b + 10)?, environment: scalar(b + 14)?, force: vec3(18)?, min, max })
        };
        result.push(decode().ok_or("unsupported spring-box modifier layout")?);
    }
    Ok(result)
}

/// File offsets of a modifier class's records, in the same order its decoder returns them.
pub fn record_offsets(data: &[u8], class: &str) -> Vec<usize> {
    let class = crate::assets::forge::crc32(class).to_le_bytes();
    (5..data.len().saturating_sub(4)).filter(|&p| data[p - 5] == 0 && data[p..p + 4] == class).collect()
}

/// One step of the authored list, in evaluation order (index into the kind's vector).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier { Compress(usize), LookAt(usize), Copy(usize), Hinge(usize), Roll(usize), Spring(usize) }

/// The SkeletonComponent's embedded force config (SkeletonComponent__Read 0x4E4E30 → ForceConfig__Read 0x5C6B20):
/// Some(scale) when enabled. PORT: located by its class marker inside the component.
pub fn decode_skeleton_force(entity: &[u8]) -> Option<f32> {
    let word = |p: usize| entity.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let component = (4..entity.len().saturating_sub(4)).find(|&p| word(p) == Some(crate::assets::forge::crc32("SkeletonComponent")))?;
    let mut configs = (component..(component + 400).min(entity.len().saturating_sub(4))).filter(|&p| word(p) == Some((-403070396i32) as u32));
    let p = configs.next()?;
    if configs.next().is_some() || entity.get(p + 9..p + 11)? != [0, 0] { return None; }
    let scale = word(p + 5).map(f32::from_bits).filter(|v| v.is_finite())?;
    (*entity.get(p + 4)? == 1).then_some(scale)
}

#[derive(Component)]
pub struct VisualRotationCopies {
    pub joints: Vec<Entity>,
    /// Authored local rest of every joint; secondary joints restart from it each frame (0x4E0650).
    pub rest: Vec<Transform>,
    /// Movement joints come first; the animator owns them.
    pub primary: usize,
    pub force_scale: Option<f32>,
    /// The previous frame's environment sample at bone 0 (0x4E7550 stores it after the modifiers).
    pub environment: Vec3,
    pub parents: Vec<Option<usize>>,
    pub copies: Vec<(usize, usize)>,
    pub compressions: Vec<SkirtCompression>,
    pub look_at: Vec<SkirtLookAt>,
    pub hinges: Vec<crate::skirt_hinge::SkirtHinge>,
    pub hinge_states: Vec<crate::skirt_hinge::HingeState>,
    pub rolls: Vec<RollModifier>,
    pub springs: Vec<SpringBox>,
    pub spring_states: Vec<SpringState>,
    /// Evaluation order (`evaluation_order`).
    pub order: Vec<Modifier>,
    /// The last modifier output, held on paused frames: the animator rewrites the movement joints every frame.
    pub held: Vec<(usize, Transform)>,
    pub root: Entity,
    pub previous_anchor: Option<Vec3>,
}

fn world_pose(local: &[Transform], parents: &[Option<usize>]) -> Vec<Mat4> {
    let mut world = vec![Mat4::IDENTITY; local.len()];
    for i in 0..local.len() {
        world[i] = parents[i].map_or(Mat4::IDENTITY, |p| world[p]) * local[i].to_matrix();
    }
    world
}

fn compression_rotation(second: Quat, first: Quat, weight: f32) -> Quat {
    // Native shortest-arc interpolation and small-angle fallback (0x48F120).
    let dot = second.dot(first);
    if dot.abs() >= 1.0 { return first; }
    let angle = dot.abs().acos();
    if angle <= 0.0005 { return first; }
    let a = ((1.0 - weight) * angle).sin() / angle.sin();
    let b = (weight * angle).sin() / angle.sin() * if dot < 0.0 { -1.0 } else { 1.0 };
    (second * a + first * b).normalize()
}

fn compress_pose(local: &mut [Transform], parents: &[Option<usize>], compressions: &[SkirtCompression]) {
    for c in compressions {
        let world = world_pose(local, parents);
        // 0x6C4FF0: offsets compose as q_src·q_off (0x4BF460, Hamilton) and p_src + q_src·p_off (0x42EE70).
        let source = |k: usize| {
            let (_, rotation, position) = world[c.sources[k]].to_scale_rotation_translation();
            (rotation * c.rotation_offsets[k], position + rotation * c.position_offsets[k])
        };
        let ((first_rotation, first_position), (second_rotation, second_position)) = (source(0), source(1));
        let parent = parents[c.target].map_or(Mat4::IDENTITY, |p| world[p]);
        // 0x6C4FF0 blends source B toward source A and updates the owner hierarchy.
        if c.position {
            local[c.target].translation = parent.inverse().transform_point3(second_position.lerp(first_position, c.position_weight));
        }
        if c.rotation {
            local[c.target].rotation = (parent.to_scale_rotation_translation().1.inverse()
                * compression_rotation(second_rotation, first_rotation, c.rotation_weight)).normalize();
        }
    }
}

fn look_at_pose(local: &mut [Transform], parents: &[Option<usize>], modifiers: &[SkirtLookAt]) {
    for m in modifiers {
        let world = world_pose(local, parents);
        let owner = world[m.target];
        let Some(direction) = (world[m.aim].w_axis - owner.w_axis).truncate().try_normalize() else { continue; };
        // 0x648E94 uses the owner's current Z row when it has a parent.
        let reference = owner.z_axis.truncate();
        let Some(side) = reference.cross(direction).try_normalize() else { continue; };
        let up = direction.cross(side);
        // Native 0x648D80: skirt axes 2/1 = side/up/aim; hood axes 0/2 = aim/side/up.
        let basis = if m.aim_axis == 0 { Mat3::from_cols(direction, side, up) } else { Mat3::from_cols(side, up, direction) };
        let rotation = Quat::from_mat3(&basis);
        let parent = parents[m.target].map_or(Quat::IDENTITY, |p| world[p].to_scale_rotation_translation().1);
        // PORT: keep the preceding pose for coincident/parallel directions instead of a singular frame.
        local[m.target].rotation = (parent.inverse() * rotation).normalize();
    }
}

fn copy_rotations(local: &mut [Transform], parents: &[Option<usize>], copies: &[(usize, usize)]) {
    // Native matrix copy retains owner position (0x5FEA10); convert to parent-local rotation.
    let mut world = vec![Mat4::IDENTITY; local.len()];
    for &(target, source) in copies {
        for i in 0..local.len() {
            world[i] = parents[i].map_or(Mat4::IDENTITY, |p| world[p]) * local[i].to_matrix();
        }
        let source_rotation = world[source].to_scale_rotation_translation().1;
        let parent_rotation = parents[target].map_or(Quat::IDENTITY, |p| world[p].to_scale_rotation_translation().1);
        local[target].rotation = (parent_rotation.inverse() * source_rotation).normalize();
    }
}

/// Quaternion → three angles (0x4BCB10): a about Y, b about X (the roll channel), c about Z.
fn native_euler(q: Quat) -> (f32, f32, f32) {
    let (x, y, z, w) = (q.x, q.y, q.z, q.w);
    let a_num = 2.0 * (w * y + z * x);
    let a_den = 1.0 - 2.0 * (y * y + x * x);
    let b_num = 2.0 * (w * x - z * y);
    let d = (a_num * a_num + a_den * a_den).sqrt();
    if d == 0.0 {
        let e = 2.0 * (w * z - y * x);
        let f = 1.0 - 2.0 * (z * z + y * y);
        return (0.0, b_num.atan2(d), e.atan2(f));
    }
    (a_num.atan2(a_den), b_num.atan2(d), (2.0 * (w * z + y * x)).atan2(1.0 - 2.0 * (z * z + x * x)))
}

/// Three angles → quaternion (0x4BD120), the inverse of `native_euler`.
fn native_from_euler(a: f32, b: f32, c: f32) -> Quat {
    let (s0, c0) = (a * 0.5).sin_cos();
    let (s1, c1) = (b * 0.5).sin_cos();
    let (s2, c2) = (c * 0.5).sin_cos();
    Quat::from_xyzw(s2 * c1 * s0 + c2 * c0 * s1, c2 * c1 * s0 - s2 * c0 * s1, s2 * c1 * c0 - c2 * s1 * s0, s2 * s1 * s0 + c2 * c1 * c0)
}

/// RollBoneModifier__Update 0x609BF0 on local rotations: keep the owner's outer angles, drive its middle angle from the
/// source's (modes 0/2: -b·(1 - w), 0x609940; modes 1/3: (b ± 90°)·w, mirrored past 90°, 0x609A20).
fn roll_pose(local: &mut [Transform], m: &RollModifier) {
    let (sa, sb, sc) = native_euler(local[m.source].rotation);
    let (a, _, c) = native_euler(local[m.target].rotation);
    let b = match m.mode {
        0 | 2 => -sb * (1.0 - m.weight),
        _ => {
            let shifted = if m.mode == 3 { sb - std::f32::consts::FRAC_PI_2 } else { sb + std::f32::consts::FRAC_PI_2 };
            let mirrored = if sc.abs() > std::f32::consts::FRAC_PI_2 || sa.abs() > std::f32::consts::FRAC_PI_2 { -shifted } else { shifted };
            mirrored * m.weight
        }
    };
    local[m.target].rotation = native_from_euler(a, b, c);
}

/// SpringBoxModifier__Update 0x5E8A20: a point mass pulled to its rest anchor in the parent, damped relative to the moving
/// anchor, pushed by its force and the environment, frame-dt Verlet, clamped to the box in the parent's space.
fn spring_pose(local: &mut [Transform], parents: &[Option<usize>], frame: Mat4, s: &SpringBox, rest: Vec3, state: &mut SpringState,
    force: Vec3, environment: Vec3, dt: f32, reset: bool) {
    let parent = frame * parents[s.target].map_or(Mat4::IDENTITY, |p| world_pose(local, parents)[p]);
    let anchor = parent.transform_point3(rest);
    if reset || !state.live {
        // SpringBoxModifier init 0x5E8600: rest clamped into the box; position, previous and anchor coincide.
        local[s.target].translation = rest.max(s.min).min(s.max);
        let at = parent.transform_point3(local[s.target].translation);
        *state = SpringState { position: at, previous: at, anchor, live: true };
        return;
    }
    let velocity = ((state.position - state.previous) - (anchor - state.anchor)) / dt;
    let offset = state.position - anchor;
    let mut total = force + environment * s.environment;
    let distance = offset.length();
    if distance > 0.0 {
        let n = offset / distance;
        let radial = n.dot(velocity);
        total += -(velocity - n * radial) * s.damping - n * (s.stiffness * distance + radial * s.damping);
    }
    let next = total * (dt * dt) + state.position * 2.0 - state.previous;
    let clamped = parent.inverse().transform_point3(next).min(s.max).max(s.min);
    local[s.target].translation = clamped;
    state.previous = state.position;
    state.position = parent.transform_point3(clamped);
    state.anchor = anchor;
}

/// Native lists run parents before children (0x4E90C0 builds them per bone; the exact native bone order is not
/// recovered). Hypothesis: order by hierarchy depth, keeping each resource's authored order within a depth.
pub fn evaluation_order(parents: &[Option<usize>], authored: &[(Modifier, usize)]) -> Vec<Modifier> {
    let depth = |mut i: usize| { let mut d = 0; while let Some(p) = parents[i] { d += 1; i = p; } d };
    let mut keyed: Vec<_> = authored.iter().enumerate().map(|(k, &(m, owner))| (depth(owner), k, m)).collect();
    keyed.sort_by_key(|&(d, k, _)| (d, k));
    keyed.into_iter().map(|(_, _, m)| m).collect()
}

pub fn update_rotation_copies(time: Res<Time>, wind: Res<crate::wind::WindField>, mut rigs: Query<(Entity, &mut VisualRotationCopies)>, mut transforms: Query<&mut Transform>) {
    let dt = time.delta_secs();
    for (player, mut rig) in &mut rigs {
        if dt <= 0.0 {
            for &(i, transform) in &rig.held {
                if let Ok(mut t) = transforms.get_mut(rig.joints[i]) { *t = transform; }
            }
            continue;
        }
        let (Ok(player_pose), Ok(root_pose)) = (transforms.get(player), transforms.get(rig.root)) else { continue; };
        let frame = player_pose.to_matrix() * root_pose.to_matrix();
        let gravity_frame = root_pose.rotation;
        let anchor = frame.w_axis.truncate();
        // PORT: bounded reset on teleports/long stalls in place of native frame-id continuity.
        let reset = dt > 0.1 || rig.previous_anchor.is_none_or(|old| old.distance_squared(anchor) > 4.0);
        rig.previous_anchor = Some(anchor);
        // SkeletonComponent__BeginFrame 0x4E0650 copies the authored base pose into the live pose every frame,
        // so modifiers never read their own previous output. The animator already rebuilds the movement joints.
        for i in rig.primary..rig.joints.len() {
            if let Ok(mut transform) = transforms.get_mut(rig.joints[i]) { *transform = rig.rest[i]; }
        }
        let Some(mut local) = rig.joints.iter().map(|&joint| transforms.get(joint).ok().copied()).collect::<Option<Vec<_>>>() else { continue; };
        let mut written = vec![false; local.len()];
        // 0x4E6820 passes each owner's invocation index; a later hinge on the same owner composes (0x697C80).
        let mut previous_owner = None;
        let mut invocation = 0;
        let environment = rig.environment;
        let order = rig.order.clone();
        for m in order {
            let (group, owner) = match m {
                Modifier::Compress(i) => (rig.compressions[i].group, rig.compressions[i].target),
                Modifier::LookAt(i) => (rig.look_at[i].group, rig.look_at[i].target),
                Modifier::Copy(i) => (Group::Skirt, rig.copies[i].0),
                Modifier::Hinge(i) => (rig.hinges[i].group, rig.hinges[i].target),
                Modifier::Roll(i) => (Group::Body, rig.rolls[i].target),
                Modifier::Spring(i) => (Group::Body, rig.springs[i].target),
            };
            if !group.enabled() { continue; }
            invocation = if previous_owner == Some(owner) { invocation + 1 } else { 0 };
            previous_owner = Some(owner);
            written[owner] = true;
            match m {
                Modifier::Compress(i) => compress_pose(&mut local, &rig.parents, std::slice::from_ref(&rig.compressions[i])),
                Modifier::LookAt(i) => look_at_pose(&mut local, &rig.parents, std::slice::from_ref(&rig.look_at[i])),
                Modifier::Copy(i) => copy_rotations(&mut local, &rig.parents, std::slice::from_ref(&rig.copies[i])),
                Modifier::Roll(i) => roll_pose(&mut local, &rig.rolls[i]),
                Modifier::Spring(i) => {
                    let s = rig.springs[i].clone();
                    let rest = rig.rest[s.target].translation;
                    let mut state = std::mem::take(&mut rig.spring_states[i]);
                    spring_pose(&mut local, &rig.parents, frame, &s, rest, &mut state, gravity_frame * s.force, environment, dt, reset);
                    rig.spring_states[i] = state;
                }
                Modifier::Hinge(i) => {
                    let h = rig.hinges[i].clone();
                    let world: Vec<_> = world_pose(&local, &rig.parents).into_iter().map(|m| frame * m).collect();
                    let parent = rig.parents[h.target].map_or(frame, |p| world[p]);
                    let base = parent * h.rest.to_matrix();
                    let owner = world[h.target];
                    let force = h.force_reference.map_or(gravity_frame * h.force, |r| world[r].transform_vector3(h.force));
                    let references: Vec<_> = h.constraints.iter().map(|c| c.reference.map(|r| world[r].transform_vector3(c.reference_direction))).collect();
                    let solved = rig.hinge_states[i].solve(&h, base, owner, force, &references, environment, dt, reset);
                    // 0x697C80 composes later modifiers on an owner as current * inverse(base) * solved.
                    let applied = if invocation > 0 { owner * base.inverse() * solved } else { solved };
                    let (_, rotation, translation) = (parent.inverse() * applied).to_scale_rotation_translation();
                    local[h.target].translation = translation;
                    local[h.target].rotation = rotation.normalize();
                }
            }
        }
        // SkeletonComponent__UpdateSecondary 0x4E7550: sample at global bone 0 (0x4E1CA0) after the modifiers.
        let root = frame * world_pose(&local, &rig.parents)[0];
        rig.environment = rig.force_scale.map_or(Vec3::ZERO, |scale| wind.sample(root.w_axis.truncate(), scale));
        rig.held.clear();
        for (i, &w) in written.iter().enumerate() {
            if !w { continue; }
            if let Ok(mut transform) = transforms.get_mut(rig.joints[i]) { *transform = local[i]; }
            rig.held.push((i, local[i]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skeleton_force_config_rejects_truncation_and_disabled_configs() {
        let mut data = vec![0u8; 8];
        data.extend(crate::assets::forge::crc32("SkeletonComponent").to_le_bytes());
        data.extend([0u8; 12]);
        data.extend(((-403070396i32) as u32).to_le_bytes());
        data.push(1);
        data.extend(1.0f32.to_le_bytes());
        data.extend([0, 0, 1]);
        assert_eq!(decode_skeleton_force(&data), Some(1.0));
        let p = data.len() - 8;
        data[p] = 0;
        assert_eq!(decode_skeleton_force(&data), None, "a disabled config samples nothing");
        assert_eq!(decode_skeleton_force(&data[..data.len() - 5]), None);
    }
    #[test]
    fn look_at_uses_owner_reference_preserves_position_and_primary_pose() {
        let mut local = vec![Transform::from_xyz(3.0, 1.0, 0.0).with_rotation(Quat::from_rotation_z(0.4)),
            Transform::from_xyz(0.0, 2.0, 0.0), Transform::from_xyz(0.0, 0.5, 0.0).with_rotation(Quat::from_rotation_x(0.7))];
        let parents = [None, Some(0), Some(0)];
        let before = local.clone();
        let old_world = world_pose(&local, &parents);
        look_at_pose(&mut local, &parents, &[SkirtLookAt { group: Group::Skirt, target: 2, aim: 1, aim_axis: 2 }]);
        assert_eq!(local[..2], before[..2]);
        assert_eq!(local[2].translation, before[2].translation);
        let world = world_pose(&local, &parents);
        let direction = (world[1].w_axis - world[2].w_axis).truncate().normalize();
        assert!(world[2].z_axis.truncate().distance(direction) < 1e-5);
        assert!(world[2].w_axis.distance(old_world[2].w_axis) < 1e-5);
        let side = old_world[2].z_axis.truncate().cross(direction).normalize();
        assert!(world[2].x_axis.truncate().distance(side) < 1e-5);
        assert!((world[2].determinant() - 1.0).abs() < 1e-5);
    }
    #[test]
    fn look_at_degenerate_directions_retain_preceding_pose() {
        let mut local = vec![Transform::IDENTITY, Transform::IDENTITY, Transform::from_xyz(0.0, 0.0, 1.0)];
        let parents = [None, Some(0), Some(0)];
        let before = local.clone();
        look_at_pose(&mut local, &parents, &[SkirtLookAt { group: Group::Skirt, target: 1, aim: 0, aim_axis: 2 }, SkirtLookAt { group: Group::Skirt, target: 1, aim: 2, aim_axis: 2 }]);
        assert_eq!(local, before);
    }
    #[test]
    fn hood_look_at_aims_x_and_keeps_a_right_handed_frame() {
        let mut local = vec![Transform::from_xyz(3.0, 1.0, 0.0).with_rotation(Quat::from_rotation_y(0.4)),
            Transform::from_xyz(0.0, 2.0, 0.0), Transform::from_xyz(0.0, 0.5, 0.0).with_rotation(Quat::from_rotation_x(0.7))];
        let parents = [None, Some(0), Some(0)];
        let before = local.clone();
        let old_world = world_pose(&local, &parents);
        look_at_pose(&mut local, &parents, &[SkirtLookAt { group: Group::Skirt, target: 2, aim: 1, aim_axis: 0 }]);
        let world = world_pose(&local, &parents);
        let aim = (world[1].w_axis - world[2].w_axis).truncate().normalize();
        let side = old_world[2].z_axis.truncate().cross(aim).normalize();
        assert!(world[2].x_axis.truncate().distance(aim) < 1e-5);
        assert!(world[2].y_axis.truncate().distance(side) < 1e-5);
        assert!(world[2].z_axis.truncate().distance(aim.cross(side)) < 1e-5);
        assert!((world[2].determinant()-1.0).abs() < 1e-5);
        assert!(world[2].w_axis.distance(old_world[2].w_axis) < 1e-5);
        assert_eq!(local[..2], before[..2]);
        assert_eq!(local[2].translation, before[2].translation);
    }
    #[test]
    fn compression_blends_world_frames_and_preserves_primary_joints() {
        let mut local = vec![Transform::from_xyz(4.0, 2.0, 0.0).with_rotation(Quat::from_rotation_z(0.4)),
            Transform::from_xyz(1.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(0.8)),
            Transform::from_xyz(-1.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(-0.8)),
            Transform::from_xyz(0.0, 1.0, 0.0)];
        let before = local.clone();
        let parents = [None, Some(0), Some(0), Some(0)];
        let world = world_pose(&local, &parents);
        let c = SkirtCompression { group: Group::Skirt, target: 3, sources: [1, 2], position_weight: 0.25, rotation_weight: 0.75, position: true, rotation: true, rotation_offsets: [Quat::IDENTITY; 2], position_offsets: [Vec3::ZERO; 2] };
        compress_pose(&mut local, &parents, &[c]);
        assert_eq!(local[..3], before[..3]);
        let actual = world_pose(&local, &parents)[3];
        assert!(actual.w_axis.truncate().distance(world[2].w_axis.truncate().lerp(world[1].w_axis.truncate(), 0.25)) < 1e-5);
        assert!(local[3].rotation.angle_between(Quat::from_rotation_y(0.4)) < 1e-5);
        assert_eq!(local[3].scale, Vec3::ONE);
    }
    #[test]
    fn compression_flags_preserve_disabled_channels() {
        let mut local = vec![Transform::from_rotation(Quat::from_rotation_z(0.5)), Transform::from_xyz(1.0, 0.0, 0.0), Transform::from_xyz(0.0, 1.0, 0.0)];
        let parents = [None, Some(0), Some(0)];
        let mut c = SkirtCompression { group: Group::Skirt, target: 2, sources: [0, 1], position_weight: 0.5, rotation_weight: 0.5, position: false, rotation: true, rotation_offsets: [Quat::IDENTITY; 2], position_offsets: [Vec3::ZERO; 2] };
        let before = local[2];
        compress_pose(&mut local, &parents, &[c.clone()]);
        assert_eq!(local[2].translation, before.translation);
        c.position = true;
        c.rotation = false;
        let before = local[2].rotation;
        compress_pose(&mut local, &parents, &[c]);
        assert_eq!(local[2].rotation, before);
        let q = Quat::from_rotation_x(0.6);
        assert!(compression_rotation(q, -q, 0.3).angle_between(q) < 1e-5);
    }
    #[test]
    fn rotation_copy_preserves_position_and_primary_pose() {
        let mut local = vec![Transform::from_rotation(Quat::from_rotation_z(0.6)),
            Transform::from_xyz(0.2, 0.0, 0.0).with_rotation(Quat::from_rotation_x(0.9)),
            Transform::from_xyz(0.0, 0.4, 0.0).with_rotation(Quat::from_rotation_y(0.8))];
        let before = local.clone();
        let old_world = before[0].to_matrix() * before[1].to_matrix() * before[2].to_matrix();
        copy_rotations(&mut local, &[None, Some(0), Some(1)], &[(2, 0)]);
        assert_eq!(local[..2], before[..2]);
        assert_eq!(local[2].translation, before[2].translation);
        let world = local[0].to_matrix() * local[1].to_matrix() * local[2].to_matrix();
        assert!(world.w_axis.distance(old_world.w_axis) < 1e-6);
        assert!(world.to_scale_rotation_translation().1.angle_between(local[0].rotation) < 1e-5);
    }
    #[test]
    fn native_euler_round_trips_and_isolates_the_roll_channel() {
        for q in [Quat::from_rotation_x(0.7), Quat::from_rotation_y(-1.1) * Quat::from_rotation_z(0.4), Quat::from_euler(EulerRot::XYZ, 0.3, -0.5, 1.2)] {
            let (a, b, c) = native_euler(q);
            assert!(native_from_euler(a, b, c).angle_between(q) < 1e-4);
        }
        let (a, b, c) = native_euler(Quat::from_rotation_x(0.7));
        assert!(a.abs() < 1e-6 && (b - 0.7).abs() < 1e-6 && c.abs() < 1e-6, "the middle angle is the X twist");
    }
    #[test]
    fn roll_bones_counter_or_follow_the_source_twist() {
        let mut local = vec![Transform::from_rotation(Quat::from_rotation_x(0.8)), Transform::from_rotation(Quat::from_rotation_z(0.3))];
        roll_pose(&mut local, &RollModifier { target: 1, source: 0, mode: 0, weight: 0.25 });
        let (_, b, c) = native_euler(local[1].rotation);
        assert!((b - (-0.8 * 0.75)).abs() < 1e-5 && (c - 0.3).abs() < 1e-5, "mode 0 counters 1 - w of the source twist");
        roll_pose(&mut local, &RollModifier { target: 1, source: 0, mode: 1, weight: 0.5 });
        assert!((native_euler(local[1].rotation).1 - (0.8 + std::f32::consts::FRAC_PI_2) * 0.5).abs() < 1e-5);
    }
    #[test]
    fn spring_box_settles_at_its_anchor_and_respects_the_box() {
        let parents = [None, Some(0)];
        let mut local = vec![Transform::IDENTITY, Transform::from_xyz(0.0, 0.0, 0.1)];
        let s = SpringBox { target: 1, stiffness: 50.0, damping: 15.0, environment: 0.0, force: Vec3::ZERO, min: Vec3::splat(-0.05), max: Vec3::splat(0.2) };
        let mut state = SpringState::default();
        spring_pose(&mut local, &parents, Mat4::IDENTITY, &s, Vec3::new(0.0, 0.0, 0.1), &mut state, Vec3::ZERO, Vec3::ZERO, 1.0 / 60.0, false);
        // The parent jumps sideways: the mass lags inside the box, then returns to its anchor.
        let mut moved = 0.0f32;
        for frame in 0..600 {
            let parent_x = if frame < 300 { 0.5 } else { 0.5 };
            local[0].translation.x = parent_x;
            spring_pose(&mut local, &parents, Mat4::IDENTITY, &s, Vec3::new(0.0, 0.0, 0.1), &mut state, Vec3::ZERO, Vec3::ZERO, 1.0 / 60.0, false);
            assert!(local[1].translation.cmpge(s.min).all() && local[1].translation.cmple(s.max).all());
            moved = moved.max(local[1].translation.x.abs());
        }
        assert!(moved > 0.01, "inertia shows inside the box");
        assert!(local[1].translation.distance(Vec3::new(0.0, 0.0, 0.1)) < 1e-3, "settles at the rest anchor");
    }
    #[test]
    fn compression_offsets_compose_after_the_source_frame() {
        let mut local = vec![Transform::IDENTITY, Transform::from_rotation(Quat::from_rotation_z(0.5)), Transform::IDENTITY, Transform::IDENTITY];
        let parents = [None, Some(0), Some(0), Some(0)];
        let off = Quat::from_rotation_x(0.3);
        let c = SkirtCompression { group: Group::Body, target: 3, sources: [1, 2], position_weight: 1.0, rotation_weight: 1.0, position: true, rotation: true,
            rotation_offsets: [off, Quat::IDENTITY], position_offsets: [Vec3::X, Vec3::ZERO] };
        compress_pose(&mut local, &parents, &[c]);
        assert!(local[3].rotation.angle_between(Quat::from_rotation_z(0.5) * off) < 1e-5);
        assert!(local[3].translation.distance(Quat::from_rotation_z(0.5) * Vec3::X) < 1e-5);
    }
    #[test]
    fn evaluation_runs_parents_before_children_and_keeps_authored_order() {
        let parents = [None, Some(0), Some(1), Some(0)];
        let order = evaluation_order(&parents, &[(Modifier::Hinge(0), 2), (Modifier::Compress(0), 1), (Modifier::LookAt(0), 1), (Modifier::Roll(0), 3)]);
        assert_eq!(order, vec![Modifier::Compress(0), Modifier::LookAt(0), Modifier::Roll(0), Modifier::Hinge(0)]);
    }
    #[test]
    fn chained_copies_use_the_updated_source_pose() {
        let mut local = vec![Transform::from_rotation(Quat::from_rotation_y(0.7)),
            Transform::from_rotation(Quat::from_rotation_x(0.4)), Transform::from_rotation(Quat::from_rotation_z(0.5))];
        copy_rotations(&mut local, &[None, Some(0), Some(1)], &[(1, 0), (2, 1)]);
        assert!(local[1].rotation.angle_between(Quat::IDENTITY) < 1e-5);
        assert!(local[2].rotation.angle_between(Quat::IDENTITY) < 1e-5);
    }
}
