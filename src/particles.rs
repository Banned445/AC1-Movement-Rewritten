//! Contact FX: the dust (sand, grass, straw, water …) the feet kick up. A ContactEvent with its FX bit spawns the
//! surface material's FX at the player (ContactManager 0x55B1E0); each FX spawns particle entities whose emitters
//! run the game's CPU particle code (RE/22):
//! - emission 0xA84EF0 / spawn 0xA84770: emitter shape 0xA82740, initial velocity 0xA82F90, the global LCG;
//! - update 0xA84FF0: three phases between four keys (size / scale, colour, rotation speed), velocity integration;
//! - quads 0xA228D0 / 0xA84130: one camera-facing quad per particle, rotated by its angle.
//!
//! The simulation runs in game space (Z up, metres) and is converted to Bevy space when drawn.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::ImageSampler;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::anim::AnimPlayer;
use crate::assets::ac_anim::EventKind;
use crate::assets::particles::{
    load_contact_fx, BlendMode, ContactFxData, DirectionMode, EmitterDef, EmitterShape, FlowMode, FxSpawn,
    ParticleSystemDef,
};

/// Game space ↔ Bevy space (the game is Z up with Y forward; Bevy is Y up).
fn to_bevy(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, -v.y)
}
fn to_game(v: Vec3) -> Vec3 {
    Vec3::new(v.x, -v.z, v.y)
}

/// The particle code's random source: the global LCG (`dword_1A1FC3C + 4`, 0x430830): x = 1664525 x + 1013904223,
/// value = (x & 0x0FFFFFFF) / 2^28.
#[derive(Clone, Debug)]
pub struct Lcg(u32);

impl Lcg {
    pub fn new(seed: u32) -> Self {
        Lcg(seed)
    }
    pub fn next01(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((self.0 & 0x0FFF_FFFF) as f64 * 3.725_290_298_461_914e-9) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        (b - a) * self.next01() + a
    }
}

/// The per-particle random channels (0x946740): the table entry at (915488749 channel + 702395077 seed) >> 24.
#[derive(Clone)]
pub struct SeedHash(pub [f32; 256]);

impl SeedHash {
    /// PORT: without the exe's table, a table drawn from the LCG stands in.
    pub fn fallback() -> Self {
        let mut r = Lcg::new(0x2008);
        SeedHash(std::array::from_fn(|_| r.next01()))
    }
    pub fn get(&self, seed: u32, channel: u32) -> f32 {
        self.0[(915_488_749u32.wrapping_mul(channel).wrapping_add(702_395_077u32.wrapping_mul(seed)) >> 24) as usize]
    }
    /// 0x946770.
    pub fn lerp(&self, seed: u32, channel: u32, a: f32, b: f32) -> f32 {
        a + (b - a) * self.get(seed, channel)
    }
}

/// An emitter's frame in game space (the instance matrix rows: X, Y, Z axes and the origin).
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
    pub t: Vec3,
}

impl Frame {
    #[cfg_attr(not(test), allow(dead_code))]
    pub const IDENTITY: Frame = Frame { x: Vec3::X, y: Vec3::Y, z: Vec3::Z, t: Vec3::ZERO };

    /// Row-major game matrix (as stored) → frame.
    pub fn from_rows(m: &[f32; 16]) -> Frame {
        let row = |i: usize| Vec3::new(m[4 * i], m[4 * i + 1], m[4 * i + 2]);
        Frame { x: row(0), y: row(1), z: row(2), t: row(3) }
    }
    /// `self` relative to `parent` (child rows times the parent matrix).
    pub fn then(&self, parent: &Frame) -> Frame {
        let dir = |v: Vec3| parent.x * v.x + parent.y * v.y + parent.z * v.z;
        Frame { x: dir(self.x), y: dir(self.y), z: dir(self.z), t: dir(self.t) + parent.t }
    }
    pub fn point(&self, p: Vec3) -> Vec3 {
        self.x * p.x + self.y * p.y + self.z * p.z + self.t
    }
    /// A world direction in this frame's axes (the inverse rotation for orthonormal axes).
    pub fn to_local_dir(&self, v: Vec3) -> Vec3 {
        Vec3::new(v.dot(self.x) / self.x.length_squared().max(1e-12), v.dot(self.y) / self.y.length_squared().max(1e-12), v.dot(self.z) / self.z.length_squared().max(1e-12))
    }
}

/// One particle (the 60-byte record; positions in the emitter's simulation space).
#[derive(Clone, Debug)]
pub struct Particle {
    /// +0 seed of the random channels.
    pub seed: u32,
    /// +4 position.
    pub pos: Vec3,
    /// +16 rotation (rad).
    pub rot: f32,
    /// +20 size (w, h) or scale (x, y, z).
    pub size: Vec3,
    /// +32 colour (ARGB).
    pub color: u32,
    /// +36 velocity.
    pub vel: Vec3,
    /// +48 time in the current phase.
    pub age: f32,
    /// +56 bits 1-2: the phase (0 birth, 1 life, 2 death, 3 dead).
    pub phase: u8,
}

/// A running emitter (ParticleEmitter instance data).
#[derive(Clone, Debug)]
pub struct EmitterSim {
    /// The emitter's world frame (FX target × FX local matrix × emitter local matrix).
    pub frame: Frame,
    /// +136: the entity's scale (sizes, speeds and shape sizes).
    pub scale: f32,
    /// +100: time towards the emitter's Delay.
    pub time: f32,
    /// Emission accumulator and total emitted (a4[5].w, a4[6].x of 0xA84EF0).
    pub accumulator: f32,
    pub emitted: u32,
    pub particles: Vec<Particle>,
}

impl EmitterSim {
    pub fn new(frame: Frame, scale: f32) -> Self {
        EmitterSim { frame, scale, time: 0.0, accumulator: 0.0, emitted: 0, particles: Vec::new() }
    }

    /// The frame the shape samples in: the emitter's own local matrix when its particles are local, else the world
    /// frame (0xA82740).
    fn sample_frame(&self, e: &EmitterDef) -> Frame {
        if e.local { Frame::from_rows(&e.local_matrix) } else { self.frame }
    }

    /// 0xA82740: a spawn position on the emitter shape.
    fn shape_point(&self, e: &EmitterDef, rng: &mut Lcg) -> Vec3 {
        let f = self.sample_frame(e);
        let s = self.scale;
        let [a, b, c] = e.shape_size;
        match e.shape {
            EmitterShape::Point => f.t,
            EmitterShape::Rectangle => {
                let p = f.x * (rng.range(-0.5, 0.5) * s * a) + f.t;
                f.y * (rng.range(-0.5, 0.5) * s * b) + p
            }
            EmitterShape::Circle => {
                let angle = rng.range(0.0, 12.566_371);
                let r = rng.range(0.0, s * a);
                f.x * (r * angle.cos()) + f.y * (r * angle.sin()) + f.t
            }
            EmitterShape::Box => {
                let p = f.x * (rng.range(-0.5, 0.5) * s * a) + f.t;
                let p = f.y * (rng.range(-0.5, 0.5) * s * b) + p;
                f.z * (rng.range(-1.0, 0.0) * s * c) + p
            }
            EmitterShape::Sphere => {
                let th = rng.range(0.0, std::f32::consts::TAU);
                let ph = rng.range(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
                let r = rng.range(0.0, s * a);
                f.x * (th.cos() * r * ph.cos()) + f.y * (th.sin() * r * ph.cos()) + f.z * (ph.sin() * r) + f.t
            }
            EmitterShape::Cylinder => {
                let angle = rng.range(0.0, 12.566_371);
                let r = rng.range(0.0, s * a);
                let h = rng.range(-1.0, 0.0);
                f.x * (angle.cos() * r) + f.y * (angle.sin() * r) + f.z * (h * b * s) + f.t
            }
        }
    }

    /// 0xA82F90: the initial velocity (direction by DirectionMode, speed Min..MaxVelocity, times the scale).
    fn initial_velocity(&self, e: &EmitterDef, pos: Vec3, rng: &mut Lcg) -> Vec3 {
        let f = self.frame;
        let mut dir = match e.direction {
            DirectionMode::EmitterZ => {
                let (vx, vy) = (e.direction_variation[0].abs(), e.direction_variation[1].abs());
                let a = rng.range(-vx, vx);
                let b = rng.range(-vy, vy);
                (f.y * a.sin() + f.z * a.cos() + f.z * b.cos() + f.x * b.sin()) / 2.0
            }
            DirectionMode::FromCenter => {
                let d = pos - f.t;
                if d.length() > 0.0005 { d / d.length() } else { f.z }
            }
            DirectionMode::ToCenter => {
                let d = f.t - pos;
                if d.length() > 0.0005 { d / d.length() } else { -f.z }
            }
        };
        if e.local {
            dir = f.to_local_dir(dir);
        }
        let speed = rng.range(e.velocity[0], e.velocity[1]);
        dir * speed * self.scale
    }

    /// 0xA84770: spawn up to `n` particles.
    fn spawn(&mut self, sys: &ParticleSystemDef, e: &EmitterDef, n: u32, rng: &mut Lcg, hash: &SeedHash) {
        if e.delay >= self.time {
            return;
        }
        let alive = self.particles.len() as u32;
        let n = if alive + n > e.max_count { e.max_count.saturating_sub(alive) } else { n };
        for _ in 0..n {
            let seed = (rng.next01() as f64 * 4_294_967_296.0) as u32;
            if e.flags & 0x2_0000 != 0 {
                rng.next01(); // a random start frame (texture animation; PORT: frames are not drawn)
            }
            let pos = self.shape_point(e, rng);
            let rot = if e.stretch { 0.0 } else { rng.range(e.rotation[0], e.rotation[1]) };
            let r = hash.get(seed, 4);
            let size = if e.three_d {
                Vec3::from(e.scale_min[0]) + (Vec3::from(e.scale_max[0]) - Vec3::from(e.scale_min[0])) * r
            } else {
                let w = e.size_min[0][0] + (e.size_max[0][0] - e.size_min[0][0]) * r;
                let h = e.size_min[0][1] + (e.size_max[0][1] - e.size_min[0][1]) * r;
                Vec3::new(w, h, 0.0)
            };
            let vel = self.initial_velocity(e, pos, rng) * sys.velocity_factor;
            self.particles.push(Particle { seed, pos, rot, size, color: e.color[0], vel, age: 0.0, phase: 0 });
        }
    }

    /// 0xA84EF0: emission for this frame.
    fn emit(&mut self, sys: &ParticleSystemDef, e: &EmitterDef, dt: f32, rng: &mut Lcg, hash: &SeedHash) {
        if e.flow == FlowMode::SpawnOnRequest {
            return;
        }
        // PORT: CameraRelativeSpawn (+80 bit 14, a camera-distance gate) is not modelled; no contact FX sets it
        let rate = sys.rate_factor * e.rate;
        let mut n = 0u32;
        if rate.abs() > 0.0005 {
            self.accumulator += dt;
            n = (self.accumulator * rate) as u32;
            self.accumulator -= n as f32 / rate;
        }
        self.emitted += n;
        if e.flow == FlowMode::Once && self.emitted > e.max_count {
            n -= self.emitted - e.max_count;
            self.emitted = e.max_count;
        }
        self.spawn(sys, e, n, rng, hash);
    }

    /// 0xA869F0 / 0xA84FF0: one frame: the delay, every particle's phases and keys, then emission.
    pub fn update(&mut self, sys: &ParticleSystemDef, e: &EmitterDef, dt: f32, wind: Vec3, rng: &mut Lcg, hash: &SeedHash) {
        let mut dt = dt;
        if e.delay >= self.time {
            self.time += dt;
            if e.delay >= self.time {
                return;
            }
            dt = self.time - e.delay;
        }
        // the acceleration (+148) times the scale, into the emitter's space for local particles
        let mut accel = Vec3::from(e.acceleration) * self.scale;
        if e.local {
            accel = self.frame.to_local_dir(accel);
        }
        let wind = if e.wind.0 { wind * e.wind.1 } else { Vec3::ZERO };
        let mut i = 0;
        while i < self.particles.len() {
            let dead = update_particle(&mut self.particles[i], sys, e, dt, accel, wind, self.scale, hash);
            if dead {
                self.particles.swap_remove(i);
            } else {
                i += 1;
            }
        }
        self.emit(sys, e, dt, rng, hash);
    }

    /// The particles in world (game) space.
    pub fn world_pos(&self, e: &EmitterDef, p: &Particle) -> Vec3 {
        if e.local { self.frame.point(p.pos) } else { p.pos }
    }
}

/// Lerp an ARGB colour channel by channel with the game's truncation (0xA86042).
fn lerp_argb(a: u32, b: u32, t: f32) -> u32 {
    if a == b {
        return a;
    }
    let ch = |c: u32, s: u32| ((c >> s) & 0xFF) as f32;
    let mix = |s: u32| (((ch(a, s) + (ch(b, s) - ch(a, s)) * t) as i32) as u32 & 0xFF) << s;
    mix(24) | mix(16) | mix(8) | mix(0)
}

/// 0xA84FF0 for one particle. Returns true when it died.
#[allow(clippy::too_many_arguments)]
fn update_particle(p: &mut Particle, sys: &ParticleSystemDef, e: &EmitterDef, dt: f32, accel: Vec3, wind: Vec3, scale: f32, hash: &SeedHash) -> bool {
    let mut remaining = dt;
    while remaining.abs() > 0.0005 {
        if p.phase >= 3 {
            return true;
        }
        let ph = p.phase as usize;
        let duration = hash.lerp(p.seed, 0, e.phase_min[ph], e.phase_max[ph]) * sys.duration_factor;
        if duration.abs() <= 0.0005 {
            p.phase += 1;
            continue;
        }
        let step = if p.age + remaining > duration { duration - p.age } else { remaining };
        p.age += step;
        let t = p.age / duration;
        // velocity: acceleration and the wind force (0x5C8A60), then the position
        p.vel += accel * step;
        p.vel += wind * step;
        p.pos += p.vel * step;
        // PORT: ParticleEffect Cyclone (+80 bit 28, the +168 sine offsets) is not modelled; no contact FX uses it
        // rotation speed: keys p and p + 1, each a random lerp of Min / MaxRotationSpeed
        let (rs0, rs1) = (e.rotation_speed_min[ph], e.rotation_speed_max[ph + 1]);
        if !e.stretch && (rs0 != 0.0 || rs1 != 0.0) {
            let (c0, c1) = if e.per_key_random { (ph as u32 + 8, ph as u32 + 9) } else { (8, 8) };
            let k0 = hash.lerp(p.seed, c0, e.rotation_speed_min[ph], e.rotation_speed_max[ph]);
            let k1 = hash.lerp(p.seed, c1, e.rotation_speed_min[ph + 1], e.rotation_speed_max[ph + 1]);
            p.rot += ((k1 - k0) * t + k0) * step;
        }
        // size (2D) or scale (3D): keys p and p + 1
        let (c0, c1) = if e.per_key_random { (ph as u32 + 4, ph as u32 + 5) } else { (4, 4) };
        let (r0, r1) = (hash.get(p.seed, c0), hash.get(p.seed, c1));
        p.size = if e.three_d {
            let k = |i: usize, r: f32| (Vec3::from(e.scale_min[i]) + (Vec3::from(e.scale_max[i]) - Vec3::from(e.scale_min[i])) * r) * scale;
            k(ph, r0).lerp(k(ph + 1, r1), t)
        } else {
            let k = |i: usize, r: f32| {
                Vec2::new(e.size_min[i][0] + (e.size_max[i][0] - e.size_min[i][0]) * r, e.size_min[i][1] + (e.size_max[i][1] - e.size_min[i][1]) * r) * scale
            };
            k(ph, r0).lerp(k(ph + 1, r1), t).extend(0.0)
        };
        p.color = lerp_argb(e.color[ph], e.color[ph + 1], t);
        // +80 bit 7: killed below ZMin
        if e.kill_below_zmin && e.zmin >= p.pos.z {
            return true;
        }
        remaining -= step;
        if remaining.abs() > 0.0005 {
            p.phase += 1;
            // the next phase starts with the remaining time as its age, then adds it again (sic, 0xA86494)
            p.age = remaining;
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------------------------
// Bevy glue
// ---------------------------------------------------------------------------------------------------------------

/// One spawned FX: its pending and running particle entities.
struct FxInstance {
    /// Where the FX was spawned: the target's frame (game space).
    target: Frame,
    age: f32,
    spawns: Vec<FxSpawn>,
    started: Vec<bool>,
    /// Running emitters: (spawn index, particle system id, emitter index, simulation, the mesh entity).
    emitters: Vec<(usize, u32, usize, EmitterSim, Entity)>,
}

#[derive(Resource)]
pub struct ContactFx {
    data: Option<Arc<ContactFxData>>,
    loading: Option<Arc<Mutex<Option<Result<ContactFxData, String>>>>>,
    pub(crate) hash: SeedHash,
    rng: Lcg,
    active: Vec<FxInstance>,
    images: HashMap<u32, Handle<Image>>,
    materials: HashMap<(u32, u8), Handle<StandardMaterial>>,
    pub enabled: bool,
}

impl Default for ContactFx {
    fn default() -> Self {
        ContactFx {
            data: None,
            loading: None,
            hash: SeedHash::fallback(),
            rng: Lcg::new(0x1A1F_C3C0),
            active: Vec::new(),
            images: HashMap::new(),
            materials: HashMap::new(),
            enabled: true,
        }
    }
}

pub struct ParticlesPlugin;

impl Plugin for ParticlesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ContactFx>()
            .add_systems(Startup, start_loading)
            .add_systems(Update, (spawn_contact_fx, update_fx).chain().after(crate::anim::apply_clip).after(crate::ik::IkSet));
    }
}

fn start_loading(mut fx: ResMut<ContactFx>) {
    if std::env::var_os("AC_NO_MODEL").is_some() || std::env::var_os("AC_NO_FX").is_some() {
        return;
    }
    let slot = Arc::new(Mutex::new(None));
    let out = slot.clone();
    std::thread::spawn(move || {
        let r = load_contact_fx(&crate::assets::game_dir());
        *out.lock().unwrap() = Some(r);
    });
    fx.loading = Some(slot);
}

/// The FX group for a surface material (0x5AFB90): the material's group, else Default (4001).
fn group_for(data: &ContactFxData, material: Option<u16>) -> Option<&[Vec<u32>; 15]> {
    material.and_then(|m| data.groups.get(&(m as u32))).or_else(|| data.groups.get(&4001))
}

fn spawn_contact_fx(
    mut fx: ResMut<ContactFx>,
    players: Query<(&AnimPlayer, &GlobalTransform, Option<&crate::player::Body>)>,
    collision: Res<crate::collision::CollisionWorld>,
) {
    if let Some(slot) = fx.loading.clone() {
        match slot.lock().unwrap().take() {
            Some(Ok(data)) => {
                info!("contact FX: {} materials, {} particle systems from your install", data.groups.len(), data.systems.len());
                if let Some(t) = data.random {
                    fx.hash = SeedHash(t);
                }
                fx.data = Some(Arc::new(data));
                fx.loading = None;
            }
            Some(Err(e)) => {
                warn!("contact FX not loaded: {e}");
                fx.loading = None;
            }
            None => {}
        }
    }
    let Some(data) = fx.data.clone() else { return };
    if !fx.enabled {
        return;
    }
    for (anim, at, body) in &players {
        // the same FX is spawned once per entity and frame (0x55AFB0)
        let mut this_frame: Vec<u32> = Vec::new();
        for ev in &anim.events {
            // ContactEvent +12 bit 1: spawn the contact FX
            let EventKind::Contact { ty, bits } = ev.kind else { continue };
            if bits & 2 == 0 {
                continue;
            }
            // the entity's ground contact material (0x55A580)
            // PORT (hypothesis): the triangle under the feet stands in for the controller's ground contact
            let feet = body.map_or(at.translation(), |b| b.feet);
            let material = collision.material_below(feet + Vec3::Y * 0.3, 1.5);
            let Some(group) = group_for(&data, material) else { continue };
            let Some(list) = group.get(ty as usize) else { continue };
            for &id in list {
                if this_frame.contains(&id) {
                    continue;
                }
                this_frame.push(id);
                let Some(spawns) = data.fx.get(&id) else { continue };
                debug!("contact FX: {} t={:.3} type {ty} material {material:?} -> FX {id:08x} ({} spawned entities)", ev.clip, ev.time, spawns.len());
                // the FX target: the player entity (game frame: Y forward, Z up, X = Y × Z)
                let forward = to_game(body.map_or(at.forward().as_vec3(), |b| b.forward()));
                let target = Frame { x: forward.cross(Vec3::Z), y: forward, z: Vec3::Z, t: to_game(feet) };
                fx.active.push(FxInstance { target, age: 0.0, started: vec![false; spawns.len()], spawns: spawns.clone(), emitters: Vec::new() });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn update_fx(
    mut commands: Commands,
    mut fx: ResMut<ContactFx>,
    time: Res<Time>,
    wind: Res<crate::wind::WindField>,
    camera: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut handles: Query<&mut Mesh3d>,
) {
    let Some(data) = fx.data.clone() else { return };
    let dt = time.delta_secs();
    let Ok(cam) = camera.single() else { return };
    let (right, up) = (cam.right().as_vec3(), cam.up().as_vec3());
    let fx = &mut *fx;
    let mut active = std::mem::take(&mut fx.active);
    for inst in &mut active {
        inst.age += dt;
        // start the spawned entities whose time has come
        for (k, s) in inst.spawns.iter().enumerate() {
            if inst.started[k] || inst.age < s.start {
                continue;
            }
            inst.started[k] = true;
            let Some(&sys_id) = data.entities.get(&s.entity) else { continue };
            let Some(sys) = data.systems.get(&sys_id) else { continue };
            let base = Frame::from_rows(&s.local).then(&inst.target);
            for (ei, e) in sys.emitters.iter().enumerate() {
                let frame = Frame::from_rows(&e.local_matrix).then(&base);
                let material = material_for(fx, &data, e, &mut images, &mut materials);
                let sim = EmitterSim::new(frame, 1.0);
                // the mesh has its full vertex layout from the start (the pipeline is keyed on it)
                let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
                build_quads(&mut mesh, &sim, sys, e, right, up);
                let entity = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), Transform::IDENTITY, NoFrustumCulling, NotShadowCaster, NotShadowReceiver)).id();
                inst.emitters.push((k, sys_id, ei, sim, entity));
            }
        }
        // run the emitters; an entity is removed at the end of its FX element
        // LIVE: whether the spawned entity's particles finish their lives after the element ends (here they are cut)
        let spawns = &inst.spawns;
        let age = inst.age;
        inst.emitters.retain_mut(|(k, sys_id, ei, sim, entity)| {
            let s = &spawns[*k];
            if age > s.start + s.duration {
                commands.entity(*entity).despawn();
                return false;
            }
            let sys = &data.systems[sys_id];
            let e = &sys.emitters[*ei];
            let w = to_game(wind.sample(to_bevy(sim.frame.t), 1.0));
            sim.update(sys, e, dt, w, &mut fx.rng, &fx.hash);
            if let Ok(mut m) = handles.get_mut(*entity) {
                if let Some(mut mesh) = meshes.get_mut(&mut m.0) {
                    build_quads(&mut mesh, sim, sys, e, right, up);
                }
            }
            true
        });
    }
    active.retain(|i| !i.emitters.is_empty() || i.started.iter().any(|s| !s));
    fx.active = active;
}

/// One unlit material per (texture, blend mode), from the emitter's TextureMap.
fn material_for(fx: &mut ContactFx, data: &ContactFxData, e: &EmitterDef, images: &mut Assets<Image>, materials: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
    let key = (e.texture, e.blend as u8);
    if let Some(h) = fx.materials.get(&key) {
        return h.clone();
    }
    let image = data.textures.get(&e.texture).map(|t| {
        fx.images
            .entry(e.texture)
            .or_insert_with(|| {
                let mut img = Image::new_uninit(
                    Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
                    TextureDimension::D2,
                    if t.srgb { TextureFormat::Rgba8UnormSrgb } else { TextureFormat::Rgba8Unorm },
                    RenderAssetUsages::RENDER_WORLD,
                );
                img.texture_descriptor.mip_level_count = t.mips.len() as u32;
                img.data = Some(t.mips.concat());
                img.sampler = ImageSampler::Descriptor(crate::assets::ac_formats::texture_sampler(t));
                images.add(img)
            })
            .clone()
    });
    // BlendMode (+80 bits 21-23)
    let alpha_mode = match e.blend {
        BlendMode::Additive | BlendMode::Dodge => AlphaMode::Add,
        BlendMode::Modulate => AlphaMode::Multiply,
        BlendMode::Copy => AlphaMode::Opaque,
        // PORT: Blend_Substractive has no Bevy blend state; alpha blending stands in
        BlendMode::Subtractive | BlendMode::Alpha => AlphaMode::Blend,
    };
    // PORT: LightingMode PerVertex (the scene light on the particles, +452 / +456) is not applied; the particles are
    // unlit
    let h = materials.add(StandardMaterial {
        base_color_texture: image,
        unlit: true,
        alpha_mode,
        cull_mode: None,
        double_sided: true,
        ..default()
    });
    fx.materials.insert(key, h.clone());
    h
}

/// 0xA228D0 / 0xA84130: one quad per particle: the centre, ±half the size along the camera's right and up turned by
/// the particle's rotation, the particle colour, UVs (0, 1) (0, 0) (1, 0) (1, 1) with the flip bits 19 / 20.
/// PORT (hypothesis): the billboard and the rotation are done here; the game's vertex shader (not decoded) turns the
/// corner offsets it is given. Motion stretch (+80 bit 27) and texture frames are not drawn (no contact FX uses them).
pub(crate) fn build_quads(mesh: &mut Mesh, sim: &EmitterSim, sys: &ParticleSystemDef, e: &EmitterDef, right: Vec3, up: Vec3) {
    let n = sim.particles.len();
    let mut pos = Vec::with_capacity(4 * n);
    let mut col = Vec::with_capacity(4 * n);
    let mut uv = Vec::with_capacity(4 * n);
    let mut idx = Vec::with_capacity(6 * n);
    let alpha_factor = sys.alpha_factor;
    let (fu, fv) = (e.flags & 0x8_0000 != 0, e.flags & 0x10_0000 != 0);
    let u = |x: f32| if fu { 1.0 - x } else { x };
    let v = |y: f32| if fv { 1.0 - y } else { y };
    for p in &sim.particles {
        let centre = to_bevy(sim.world_pos(e, p));
        let (hx, hy) = (p.size.x * 0.5, p.size.y * 0.5);
        let (s, c) = p.rot.sin_cos();
        let corner = |x: f32, y: f32| centre + right * (x * c - y * s) + up * (x * s + y * c);
        let base = pos.len() as u32;
        pos.extend([corner(-hx, hy), corner(-hx, -hy), corner(hx, -hy), corner(hx, hy)]);
        uv.extend([[u(0.0), v(1.0)], [u(0.0), v(0.0)], [u(1.0), v(0.0)], [u(1.0), v(1.0)]]);
        let ch = |sh: u32| ((p.color >> sh) & 0xFF) as f32 / 255.0;
        let rgba = Color::srgba(ch(16), ch(8), ch(0), ch(24) * alpha_factor).to_linear().to_f32_array();
        col.extend([rgba; 4]);
        idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    if n == 0 {
        // never an empty vertex buffer (Bevy's mesh slabs do not take one): one transparent, degenerate quad
        pos.extend([Vec3::ZERO; 4]);
        uv.extend([[0.0, 0.0]; 4]);
        col.extend([[0.0; 4]; 4]);
        idx.extend([0, 1, 2, 0, 2, 3]);
    }
    let count = pos.len();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; count]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    mesh.insert_indices(Indices::U32(idx));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dust() -> (ParticleSystemDef, EmitterDef) {
        // the default dust's authored values (ParticleSystem_GFX_FX_CollisionMaterial_Default)
        let e = EmitterDef {
            flags: (2 << 1) | (4 << 21) | (1 << 24) | (1 << 10) | (1 << 11),
            shape: EmitterShape::Circle,
            flow: FlowMode::Once,
            direction: DirectionMode::EmitterZ,
            blend: BlendMode::Alpha,
            three_d: false,
            local: false,
            kill_below_zmin: false,
            stretch: false,
            local_matrix: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            shape_size: [0.1, 1.0, 1.0],
            zmin: 0.0,
            delay: 0.0,
            max_count: 30,
            rate: 300.0,
            acceleration: [0.0, 0.0, -2.0],
            per_key_random: false,
            velocity: [0.1, 1.0],
            direction_variation: [2.0, 0.0],
            rotation: [0.0, 8.0],
            phase_min: [0.338_33, 0.880_17, 0.621_15],
            phase_max: [0.338_33, 0.880_17, 0.621_15],
            size_min: [[0.094_72; 2], [0.160_69; 2], [0.964_76, 0.968_75], [1.449_45; 2]],
            size_max: [[0.094_72; 2], [0.160_69; 2], [0.964_76, 0.968_75], [1.449_45; 2]],
            scale_min: [[1.0; 3]; 4],
            scale_max: [[1.0; 3]; 4],
            color: [0x24FF_FFFF, 0x1FFF_FFFF, 0x0FFF_FFFF, 0x00FF_FFFF],
            rotation_speed_min: [0.0; 4],
            rotation_speed_max: [0.0; 4],
            texture: 0,
            stretch_length: 0.0,
            stretch_fade: 0.0,
            stretch_fade2: 0.0,
            wind: (true, 1.0),
        };
        let sys = ParticleSystemDef { emitters: vec![e.clone()], rate_factor: 1.0, duration_factor: 1.0, velocity_factor: 1.0, alpha_factor: 1.0, color_factor: 0xFFFF_FFFF };
        (sys, e)
    }

    #[test]
    fn lcg_and_seed_hash_follow_the_game() {
        let mut r = Lcg::new(0);
        // x1 = 1013904223 = 0x3C6EF35F; (x1 & 0x0FFFFFFF) / 2^28
        assert!((r.next01() - (0x0C6E_F35F as f32 / 268_435_456.0)).abs() < 1e-6);
        let h = SeedHash(std::array::from_fn(|i| i as f32 / 256.0));
        // (915488749 * 4 + 702395077 * 1) >> 24
        let i = (915_488_749u32.wrapping_mul(4).wrapping_add(702_395_077)) >> 24;
        assert_eq!(h.get(1, 4), i as f32 / 256.0);
    }

    #[test]
    fn default_dust_bursts_thirty_particles_that_grow_fade_and_die() {
        let (sys, e) = dust();
        let hash = SeedHash::fallback();
        let mut rng = Lcg::new(7);
        let mut sim = EmitterSim::new(Frame::IDENTITY, 1.0);
        // 0.1 s at 300/s: the whole burst of 30 (MaxParticleCount, FlowMode Once)
        for _ in 0..12 {
            sim.update(&sys, &e, 1.0 / 60.0, Vec3::ZERO, &mut rng, &hash);
        }
        assert_eq!(sim.particles.len(), 30);
        let mut r = Lcg::new(3);
        assert!((0..200).map(|_| sim.shape_point(&e, &mut r)).all(|p| p.truncate().length() <= 0.1 + 1e-5 && p.z == 0.0), "spawned in the 0.1 m disc");
        // EmitterZ with DirectionVariation (2, 0): z = (cos a + 1) / 2 ≥ 0.29 of the speed
        assert!((0..200).map(|_| sim.initial_velocity(&e, Vec3::ZERO, &mut r)).all(|v| v.z >= 0.29 * v.length() - 1e-5 && v.length() <= 1.0), "EmitterZ sends them up");
        // a second later: grown, fainter, falling back under the 2 m/s² gravity
        for _ in 0..60 {
            sim.update(&sys, &e, 1.0 / 60.0, Vec3::ZERO, &mut rng, &hash);
        }
        let p = &sim.particles[0];
        assert!(p.size.x > 0.3 && p.phase == 1, "{p:?}");
        assert!((p.color >> 24) < 0x24 && (p.color & 0x00FF_FFFF) == 0x00FF_FFFF);
        // no new particles after the burst, and all gone after their 1.84 s
        for _ in 0..90 {
            sim.update(&sys, &e, 1.0 / 60.0, Vec3::ZERO, &mut rng, &hash);
        }
        assert!(sim.particles.is_empty(), "{} left", sim.particles.len());
        assert_eq!(sim.emitted, 30);
    }

    #[test]
    fn keys_interpolate_within_each_phase() {
        let (sys, e) = dust();
        let hash = SeedHash::fallback();
        let mut p = Particle { seed: 1, pos: Vec3::ZERO, rot: 0.0, size: Vec3::ZERO, color: e.color[0], vel: Vec3::ZERO, age: 0.0, phase: 0 };
        // half way through the birth phase: half way between the birth and begin-life keys
        assert!(!update_particle(&mut p, &sys, &e, 0.338_33 / 2.0, Vec3::ZERO, Vec3::ZERO, 1.0, &hash));
        assert!((p.size.x - (0.094_72 + 0.160_69) / 2.0).abs() < 1e-4, "{}", p.size.x);
        assert_eq!(p.color >> 24, 33, "alpha 36 → 31 half way: 33.5, truncated");
    }

    #[test]
    fn frames_compose_child_then_parent() {
        let parent = Frame { x: Vec3::Y, y: -Vec3::X, z: Vec3::Z, t: Vec3::new(1.0, 2.0, 3.0) };
        let child = Frame { t: Vec3::new(1.0, 0.0, 0.0), ..Frame::IDENTITY };
        let f = child.then(&parent);
        assert!((f.t - Vec3::new(1.0, 3.0, 3.0)).length() < 1e-6);
        assert!((f.x - Vec3::Y).length() < 1e-6);
    }

    #[test]
    #[ignore = "reads the game install (DataPC.forge)"]
    fn contact_groups_fall_back_to_default() {
        let data = load_contact_fx(&crate::assets::game_dir()).unwrap();
        let default = &data.groups[&4001];
        // 0x5AFB90: the material's own group, else Default (4001): no material, or an id without a group
        assert!(std::ptr::eq(group_for(&data, None).unwrap(), default));
        assert!(std::ptr::eq(group_for(&data, Some(999)).unwrap(), default));
        for (&m, g) in data.groups.iter().filter(|(m, _)| **m < 4000) {
            assert!(std::ptr::eq(group_for(&data, Some(m as u16)).unwrap(), g));
        }
        // the Default group's run step (type 7) spawns an FX whose entities resolve to particle systems
        let fx = &data.fx[&default[7][0]];
        assert!(fx.iter().all(|s| data.entities.get(&s.entity).is_some_and(|ps| data.systems.contains_key(ps))));
    }
}
