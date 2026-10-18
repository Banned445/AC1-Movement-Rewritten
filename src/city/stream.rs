//! Runtime streaming of a city world (RE/17 §5): grid cells load and unload around the player on loader threads,
//! objects switch LOD meshes by camera distance, and the distant city is drawn by the World's fake meshes.
//!
//! The exe's rules (`GridStreamer__Update` 0x55D810, verified 2026-10-09, RE/17 §1.1): every frame the streamer wants
//! the cells under a square box of half-size `GridPartition__LoadingRadiusAt` (the byte map: 64 m inside Damascus) around
//! the player — the finest cells under it and all their parents — requests them nearest-and-ahead first
//! (`GridStreamer__RequestCellsInBox` 0x55D210: 0.6 · (1 − cos view angle) + 0.4 · d² / cell²), and unloads every cell it
//! does not want. While a cell under the half-cell box around the player is still loading, the player is held. A loaded
//! finest cell hides its fake spans; unloading shows them (`GridPartition__OnCellLoaded` 0x68B620 /
//! `FakeEntities__SetCellLoaded` 0x5E4AB0).
//! PORT: decoding runs on loader threads and the plan is refreshed every 0.25 s instead of every frame.
//! PORT (draw batching, not a game rule): the shown sections of a cell's objects are merged per material into batch
//! meshes, baked in world space on builder threads and rebuilt when one of their objects switches LOD; the old batch
//! draws until the new one is ready. Nearly every placement carries its own baked vertex colours, so placements of the
//! same mesh cannot share one GPU mesh (instancing); merging them is the static-batching form of the same saving.
//! Off by default (`AC_CITY_BATCH=1` turns it on): measured on the demo course (2026-10-18) it cut the drawn entities
//! from about 6 000 to 2 300 but saved only 0.15 ms per frame standing still, and the rebuilds' uploads made the
//! moving route slower (8.4 ms per frame against 7.6). Bevy's GPU-driven batching already draws the per-section
//! entities in few calls, so their number is not what the frame waits on. The LOD cross-fade never batches.
//! PORT: movement only sees objects within `PHYSICS_RADIUS` (40 m) of the player (their triangles, guidance edges, haystacks
//! and posts enter the shared CollisionWorld / GuidanceWorld in stable slots). Every movement query reaches far less
//! than that radius, so their answers are those of the whole city; this replaces Havok's broadphase and the guidance
//! partitioner, not a game rule.

use std::{collections::{HashMap, VecDeque}, sync::{mpsc, Arc, Condvar, Mutex}};

use bevy::{asset::RenderAssetUsages, image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor}, light::NotShadowCaster,
    mesh::{Indices, PrimitiveTopology}, prelude::*, render::render_resource::{Extent3d, TextureDimension}};

use super::{decode::{decode_cell, CityData, CityObject, DecodedCell, LodMesh}, fakes::FakeBlock, ground::GroundSurface, material::{MaterialData, TextureData}};
use crate::{collision::CollisionWorld, guidance::{GuidanceEdge, GuidanceSubType, GuidanceWorld}};

/// `GridStreamer__RequestCellsInBox` priority weight (dword_18D6780).
const VIEW_WEIGHT: f32 = 0.6;
pub const PHYSICS_RADIUS: f32 = 40.0;
pub const PHYSICS_MARGIN: f32 = 12.0;
/// PORT: per-frame budget of mesh vertices built for LOD switches, nearest first (spreads the cost of entering a
/// district: each built mesh is uploaded by the render world the next frame). 400 000 let one frame upload 80 000
/// vertices and take 9–12 ms in the render world's asset preparation (2026-10-18, `AC_RENDER_PROFILE`); 15 000 keeps
/// that near 1 ms. `AC_LOD_BUDGET=<vertices>` overrides it.
const LOD_VERTEX_BUDGET: usize = 15_000;
/// PORT: per-frame budget of batch vertices sent to the builder threads (each rebuilt batch is uploaded whole).
const BATCH_VERTEX_BUDGET: usize = 120_000;
/// PORT: the objects of a cell are batched in runs of this many (in the cell's order), which bounds a batch's rebuild.
const BATCH_OBJECTS: usize = 48;
/// PORT: at most this many texture bytes are uploaded per frame when integrating cells (one cell always goes through).
const TEXTURE_BYTE_BUDGET: usize = 6 << 20;
/// PORT: per-frame budget of collision triangles moved into the physics window.
const PHYSICS_TRIANGLE_BUDGET: usize = 40_000;
/// PORT: objects past this camera distance draw without casting shadows.
const SHADOW_DISTANCE: f32 = 90.0;
/// The small / medium objects' cull distances (`Renderer__VisibilityTest` 0xAC5CE0, renderer +728 / +732) and their
/// shadows' (+736 / +740), measured from the camera to the object's box without the LOD's view scale. Read live on the
/// owner's install (RE/30): the settings' 50 / 15 / 70 / 30 m divided by the graphics options' small / medium
/// modifiers 4.0 / 2.0 (`Renderer__SetupCullDistances` 0xAC6E90). PORT: those two options are taken as read.
const SMALL_CULL: (f32, f32) = (12.5, 3.75);
const MEDIUM_CULL: (f32, f32) = (35.0, 15.0);

/// Whether an object with these size flags (`decode::entity_size_flags`) draws, and casts a shadow, at camera distance
/// `raw` (no view scale).
fn size_cull(size: u8, raw: f32) -> (bool, bool) {
    let lim = if size & 1 != 0 { Some(SMALL_CULL) } else if size & 2 != 0 { Some(MEDIUM_CULL) } else { None };
    lim.map_or((true, true), |(draw, shadow)| (raw <= draw, raw <= shadow))
}

/// PORT: the cross-fade's blend factor is kept in this many steps (each step is a shared material) instead of the
/// exe's 255.
const FADE_LEVELS: usize = 16;
/// PORT: depth bias of the fading-in LOD toward the camera (the fade pass 0xABE770 uses D3D depth bias −1e-5 and slope
/// bias 1.00005, dword_1950BA4 / dword_1950BA8); here in depth-buffer steps.
const FADE_DEPTH_BIAS: f32 = 4096.0;
/// Freed guidance slots are parked here (far outside every world) until reused.
const PARKED: f32 = 1.0e7;

/// Loader threads decode cells off the main thread.
struct Loader {
    queue: Arc<(Mutex<(VecDeque<usize>, bool)>, Condvar)>,
    results: Mutex<mpsc::Receiver<(usize, Result<DecodedCell, String>)>>,
}

impl Loader {
    fn new(data: Arc<CityData>, threads: usize) -> Self {
        let queue: Arc<(Mutex<(VecDeque<usize>, bool)>, Condvar)> = Arc::default();
        let (tx, rx) = mpsc::channel();
        for _ in 0..threads {
            let (queue, tx, data) = (queue.clone(), tx.clone(), data.clone());
            std::thread::spawn(move || loop {
                let cell = {
                    let (lock, cv) = &*queue;
                    let mut q = lock.lock().unwrap();
                    loop {
                        if q.1 { return; }
                        if let Some(c) = q.0.pop_front() { break c; }
                        q = cv.wait(q).unwrap();
                    }
                };
                if tx.send((cell, decode_cell(&data, cell))).is_err() { return; }
            });
        }
        Self { queue, results: Mutex::new(rx) }
    }
    /// Replace the pending jobs (nearest first).
    fn request(&self, cells: Vec<usize>) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().unwrap();
        q.0 = cells.into();
        cv.notify_all();
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        let (lock, cv) = &*self.queue;
        lock.lock().unwrap().1 = true;
        cv.notify_all();
    }
}

/// One batch to (re)build: every member section baked into world space.
struct BatchJob {
    key: BatchKey,
    id: u64,
    layered: bool,
    /// (mesh, placement colours, section, the drawn transform's matrix)
    parts: Vec<(Arc<crate::assets::static_mesh::StaticMesh>, Option<Arc<Vec<[u8; 4]>>>, usize, Mat4)>,
}

/// Builder threads bake batch meshes off the main thread.
struct Builder {
    jobs: mpsc::Sender<BatchJob>,
    results: Mutex<mpsc::Receiver<(BatchKey, u64, Mesh)>>,
}

impl Builder {
    fn new(threads: usize) -> Self {
        let (jobs, rx) = mpsc::channel::<BatchJob>();
        let rx = Arc::new(Mutex::new(rx));
        let (tx, results) = mpsc::channel();
        for _ in 0..threads {
            let (rx, tx) = (rx.clone(), tx.clone());
            std::thread::spawn(move || loop {
                let job = { let Ok(job) = rx.lock().unwrap().recv() else { return }; job };
                if tx.send((job.key, job.id, batch_mesh(&job.parts, job.layered))).is_err() { return; }
            });
        }
        Self { jobs, results: Mutex::new(results) }
    }
}

/// A batch: the shown sections of one material among a run of a cell's objects, with one shadow setting.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct BatchKey {
    cell: usize,
    run: u32,
    material: u32,
    shadow: bool,
}

struct Batch {
    /// (object index in the cell, mesh, section)
    members: Vec<(u32, u8, u16)>,
    entity: Option<Entity>,
    /// Members changed since the drawn mesh was built.
    dirty: bool,
    /// A rebuild is on a builder thread.
    building: bool,
    /// Tells this batch from an earlier one of the same key (a cell can unload and load again while a job runs).
    id: u64,
    /// Camera distance of the nearest member when it last changed (rebuild order).
    near: f32,
}

struct Physics {
    triangles: Vec<u32>,
    edges: Vec<usize>,
}

struct Obj {
    o: CityObject,
    shown: Option<usize>,
    entities: Vec<Entity>,
    /// The batches the shown sections are in (instead of `entities`).
    batches: Vec<BatchKey>,
    /// The next LOD drawn on top of `shown` while the distance is inside the fade width.
    fade: Option<FadeIn>,
    physics: Option<Physics>,
}

struct FadeIn {
    mesh: usize,
    level: u8,
    /// (entity, material id) per drawn section
    sections: Vec<(Entity, u32)>,
    /// Every section of the mesh is drawn (none had a material that cannot fade), so it can become `shown` as is.
    complete: bool,
}

enum Cell {
    Unloaded,
    Loading,
    /// `extras`: the cell's presentation entities (lights), despawned with the cell.
    Loaded { objects: Vec<Obj>, materials: Vec<u32>, extras: Vec<Entity> },
}

/// A city material on the GPU: plain (Bevy PBR) or layered (`ground`).
#[derive(Clone)]
enum CityMaterial {
    Plain(Handle<StandardMaterial>),
    Ground(Handle<GroundSurface>),
}

impl CityMaterial {
    fn layered(&self) -> bool { matches!(self, CityMaterial::Ground(_)) }

    fn insert(&self, e: &mut EntityCommands) {
        match self { CityMaterial::Plain(h) => e.insert(MeshMaterial3d(h.clone())), CityMaterial::Ground(h) => e.insert(MeshMaterial3d(h.clone())) };
    }

    fn remove(&self, ctx: &mut Ctx) {
        match self { CityMaterial::Plain(h) => { ctx.materials.remove(h); } CityMaterial::Ground(h) => { ctx.grounds.remove(h); } }
    }
}

struct GpuMaterial {
    handle: CityMaterial,
    textures: Vec<u32>,
    refs: u32,
    /// Opaque materials can fade; the water material is already translucent.
    fadeable: bool,
    /// Translucent copies per fade level (created on first use).
    fades: Vec<Option<CityMaterial>>,
}

/// Counters for the debug overlay and tests.
#[derive(Default, Clone, Debug)]
pub struct CityStats {
    pub loaded: usize,
    pub loading: usize,
    pub objects: usize,
    pub drawn: usize,
    /// Objects cross-fading into their next LOD.
    pub fading: usize,
    pub physics_objects: usize,
    pub physics_triangles: usize,
    pub physics_edges: usize,
    pub materials: usize,
    pub textures: usize,
    /// Local lights of the loaded cells.
    pub lights: usize,
    pub errors: usize,
    /// Last frame's streaming time per phase (ms): integrate, plan, LOD, physics, refresh.
    pub phase_ms: [f32; 5],
    /// Last frame's streaming work: cells integrated, texture bytes uploaded, LOD vertices built, collision triangles
    /// moved into the physics window.
    pub work: [usize; 4],
    /// Drawn mesh entities (per-section entities and batches).
    pub entities: usize,
    /// Batches, and those waiting for or on a builder thread.
    pub batches: usize,
    pub batches_pending: usize,
}

pub struct CityState {
    pub data: Arc<CityData>,
    loader: Loader,
    cells: Vec<Cell>,
    has_content: Vec<bool>,
    materials: HashMap<u32, GpuMaterial>,
    textures: HashMap<u32, (Handle<Image>, u32)>,
    /// finest cell → fake span entities hidden while the cell is loaded
    fake_spans: HashMap<usize, Vec<Entity>>,
    edge_free: Vec<usize>,
    haystacks_dirty: bool,
    plan_timer: f32,
    /// This frame's `CityStats::work`.
    work: [usize; 4],
    /// Draw batching (`AC_CITY_BATCH=1`, off by default).
    pub batch: bool,
    /// `LOD_VERTEX_BUDGET` or `AC_LOD_BUDGET`.
    lod_budget: usize,
    batches: HashMap<BatchKey, Batch>,
    builder: Builder,
    next_batch: u64,
    pub stats: CityStats,
    /// (seconds, frames, worst frame) for AC_CITY_LOG
    pub log: (f32, u32, f32),
    /// AC_CITY_TOUR start point
    pub tour: Option<Vec3>,
    /// A cell under the near box is still loading: the player is held at this position (0x55D810).
    pub hold: Option<Vec3>,
    /// Draw the LOD cross-fade (`AC_LOD_FADE=1`). Off like the retail game: its fade pass (0xABE770) only runs while
    /// renderer +94 is set, which needs `g_LODBlendPassEnabled` (0x1A25846) — 0 in the running game (read live in
    /// Damascus, 2026-10-09; the `LODBlend` INI value, GraphicsSettings +0x52, is loaded but never read).
    pub lod_fade: bool,
    /// The particle effects of newly loaded cells and the cells unloaded since, for `presentation::update_effects`.
    pub effects_out: Vec<(usize, super::presentation::CityEffect)>,
    pub effects_unloaded: Vec<usize>,
    /// The same for the ambient sound emitters (`presentation::update_ambience`).
    pub sounds_out: Vec<(usize, super::presentation::CitySound)>,
    pub sounds_unloaded: Vec<usize>,
    pub ambience_out: Vec<(usize, Vec<super::presentation::AmbienceSet>)>,
}

#[derive(Resource, Default)]
pub struct City(pub Option<Box<CityState>>);

/// The game coordinates (x, y) of a Bevy position.
fn game_xy(p: Vec3) -> Vec2 { Vec2::new(p.x, -p.z) }

fn xz_distance(p: Vec3, b: &crate::collision::Aabb3) -> f32 {
    let q = Vec2::new(p.x, p.z);
    (q - q.clamp(Vec2::new(b.min.x, b.min.z), Vec2::new(b.max.x, b.max.z))).length()
}

pub struct Ctx<'a, 'w, 's> {
    pub commands: &'a mut Commands<'w, 's>,
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<StandardMaterial>,
    pub grounds: &'a mut Assets<GroundSurface>,
    pub images: &'a mut Assets<Image>,
    pub collision: &'a mut CollisionWorld,
    pub guidance: &'a mut GuidanceWorld,
}

impl CityState {
    pub fn new(data: Arc<CityData>) -> Self {
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(2).clamp(1, 6));
        let has_content = content_mask(&data);
        let n = data.grid.cells.len();
        Self { loader: Loader::new(data.clone(), threads), data, cells: (0..n).map(|_| Cell::Unloaded).collect(), has_content,
            materials: HashMap::new(), textures: HashMap::new(), fake_spans: HashMap::new(), edge_free: Vec::new(),
            haystacks_dirty: true, plan_timer: 0.0, work: [0; 4],
            batch: std::env::var("AC_CITY_BATCH").is_ok_and(|v| v == "1"),
            lod_budget: std::env::var("AC_LOD_BUDGET").ok().and_then(|v| v.parse().ok()).unwrap_or(LOD_VERTEX_BUDGET), batches: HashMap::new(), builder: Builder::new(threads.min(3)), next_batch: 0, stats: CityStats::default(), log: (0.0, 0, 0.0), tour: None, hold: None,
            lod_fade: std::env::var("AC_LOD_FADE").is_ok_and(|v| v == "1"), effects_out: Vec::new(), effects_unloaded: Vec::new(),
            sounds_out: Vec::new(), sounds_unloaded: Vec::new(), ambience_out: Vec::new() }
    }

    /// The cells the streamer wants with the player at `focus` (Bevy space), in request order.
    pub fn wanted(&self, focus: Vec3, view: Option<Vec3>) -> Vec<usize> {
        wanted_cells(&self.data, &self.has_content, focus, view)
    }

    /// Is every wanted cell around `p` loaded?
    pub fn settled(&self, p: Vec3) -> bool {
        self.wanted(p, None).iter().all(|&c| matches!(self.cells[c], Cell::Loaded { .. }))
    }

    pub fn is_loaded(&self, c: usize) -> bool { matches!(self.cells[c], Cell::Loaded { .. }) }

    /// Per frame: take finished cells, plan loads/unloads, switch LODs, move objects in and out of the physics window.
    pub fn update(&mut self, player: Vec3, camera: Vec3, view: Vec3, lod_scale: f32, dt: f32, ctx: &mut Ctx) {
        let mut phase = [0.0f32; 5];
        self.work = [0; 4];
        let mut clock = std::time::Instant::now();
        let mut lap = |k: usize, phase: &mut [f32; 5]| { phase[k] = clock.elapsed().as_secs_f32() * 1000.0; clock = std::time::Instant::now(); };
        // finished decodes (bounded so a burst does not stall a frame)
        let started = std::time::Instant::now();
        loop {
            let next = self.loader.results.lock().unwrap().try_recv();
            let Ok((cell, result)) = next else { break };
            match result {
                Ok(d) if matches!(self.cells[cell], Cell::Loading) => self.integrate(d, ctx),
                Ok(_) => {}
                Err(e) => { warn!("city cell {cell}: {e}"); self.stats.errors += 1; self.cells[cell] = Cell::Unloaded; }
            }
            if started.elapsed().as_secs_f32() > 0.004 || self.work[1] > TEXTURE_BYTE_BUDGET { break; }
        }
        lap(0, &mut phase);
        self.plan_timer -= dt;
        if self.plan_timer <= 0.0 {
            self.plan_timer = 0.25;
            self.plan(player, view, ctx);
        }
        // the half-cell box around the player (0x55D810's first request): hold while any of it is still loading
        let p = game_xy(player);
        let near = (self.data.grid.cell_size * 0.5) as i32;
        let waiting = (0..self.cells.len()).any(|c| self.has_content[c] && !self.is_loaded(c) && self.data.grid.cell_in_box(&self.data.grid.cells[c], p, near));
        self.hold = if waiting { Some(self.hold.unwrap_or(player)) } else { None };
        lap(1, &mut phase);
        self.update_lods(camera, lod_scale, ctx, self.lod_budget);
        lap(2, &mut phase);
        self.update_physics(player, ctx, PHYSICS_TRIANGLE_BUDGET);
        lap(3, &mut phase);
        self.refresh(ctx);
        lap(4, &mut phase);
        self.count();
        self.stats.phase_ms = phase;
        self.stats.work = self.work;
    }

    /// The first frame: every LOD and the whole physics window at once (no budgets).
    pub fn prime(&mut self, player: Vec3, camera: Vec3, ctx: &mut Ctx) {
        self.update_physics(player, ctx, usize::MAX);
        self.update_lods(camera, 1.0, ctx, usize::MAX);
        // the first frame waits for its batches
        let pending = self.batches.values().filter(|b| b.building).count();
        let results: Vec<_> = { let rx = self.builder.results.lock().unwrap(); (0..pending).filter_map(|_| rx.recv().ok()).collect() };
        for (key, id, mesh) in results { self.apply_batch(key, id, mesh, ctx); }
        self.refresh(ctx);
        self.count();
    }

    fn refresh(&mut self, ctx: &mut Ctx) {
        if self.haystacks_dirty {
            self.haystacks_dirty = false;
            let live = || self.cells.iter().filter_map(|c| match c { Cell::Loaded { objects, .. } => Some(objects), _ => None })
                .flatten().filter(|o| o.physics.is_some());
            ctx.guidance.haystacks = live().filter_map(|o| o.o.haystack).collect();
            // the kiosks' owners: the entity's forward row (Bevy space: the matrix's Y axis), the Kiosk context's axis
            ctx.guidance.kiosks = live().filter(|o| kiosk_owner(&o.o))
                .map(|o| (o.o.bounds, Vec3::new(o.o.transform.y_axis.x, 0.0, o.o.transform.y_axis.z).normalize_or_zero())).collect();
        }
    }

    fn plan(&mut self, player: Vec3, view: Vec3, ctx: &mut Ctx) {
        let order = self.wanted(player, Some(view));
        let mut want = vec![false; self.cells.len()];
        for &c in &order { want[c] = true; }
        // 0x55D810: every cell not wanted this frame unloads (no hysteresis)
        for c in 0..self.cells.len() {
            match self.cells[c] {
                Cell::Loading if !want[c] => self.cells[c] = Cell::Unloaded,
                Cell::Loaded { .. } if !want[c] => self.unload(c, ctx),
                _ => {}
            }
        }
        let requests: Vec<usize> = order.into_iter().filter(|&c| !self.is_loaded(c)).collect();
        for &c in &requests { self.cells[c] = Cell::Loading; }
        self.loader.request(requests);
    }

    pub fn integrate(&mut self, d: DecodedCell, ctx: &mut Ctx) {
        self.work[0] += 1;
        for (id, linear, t) in &d.textures {
            if self.textures.contains_key(id) { continue; }
            self.work[1] += t.data.len();
            let h = ctx.images.add(image(t, *linear));
            self.textures.insert(*id, (h, 0));
            self.data.resident.lock().unwrap().insert(*id);
        }
        let mut used = Vec::new();
        for (id, m) in &d.materials {
            if !self.materials.contains_key(id) {
                let Some(gpu) = self.material(m, ctx) else { continue };
                self.materials.insert(*id, gpu);
            }
            self.materials.get_mut(id).unwrap().refs += 1;
            used.push(*id);
        }
        let objects = d.objects.into_iter().map(|o| Obj { o, shown: None, entities: Vec::new(), batches: Vec::new(), fade: None, physics: None }).collect();
        let extras = super::presentation::spawn_lights(&d.lights, ctx.commands);
        self.effects_out.extend(d.effects.into_iter().map(|e| (d.cell, e)));
        self.sounds_out.extend(d.sounds.into_iter().map(|e| (d.cell, e)));
        if !d.ambience.is_empty() { self.ambience_out.push((d.cell, d.ambience)); }
        self.cells[d.cell] = Cell::Loaded { objects, materials: used, extras };
        for &e in self.fake_spans.get(&d.cell).into_iter().flatten() { ctx.commands.entity(e).insert(Visibility::Hidden); }
    }

    fn material(&mut self, m: &MaterialData, ctx: &mut Ctx) -> Option<GpuMaterial> {
        let mut texture = |id: u32, linear: bool| -> Option<Handle<Image>> {
            if let Some(t) = self.textures.get_mut(&id) { t.1 += 1; return Some(t.0.clone()); }
            // freed after this cell was decoded: decode again now (rare)
            let t = self.data.materials.texture(&self.data.archives, id, linear, self.data.bc).ok()?;
            let h = ctx.images.add(image(&t, linear));
            self.textures.insert(id, (h.clone(), 1));
            self.data.resident.lock().unwrap().insert(id);
            Some(h)
        };
        if !m.layers.is_empty() {
            // the layered ground (`ground`): every map once
            let mut handles = HashMap::new();
            for (id, linear) in m.maps() {
                if let Some(h) = texture(id, linear) { handles.insert(id, h); }
            }
            let map = |k: usize, normal: bool| m.layers.get(k).and_then(|l| if normal { l.normal } else { l.diffuse }).and_then(|id| handles.get(&id).cloned());
            let mut controls = super::ground::controls(&m.layers);
            for k in 0..3 { if map(k, true).is_none() { controls.tiles[k].w = 0.0; } }
            let blend = super::ground::GroundBlend { controls, base: map(0, false), layer1: map(1, false), layer2: map(2, false),
                base_normal: map(0, true), layer1_normal: map(1, true), layer2_normal: map(2, true) };
            // PORT: opaque; the native shader writes the vertex colour's alpha (its alpha test is not checked)
            let surface = GroundSurface { base: StandardMaterial { flip_normal_map_y: true, perceptual_roughness: 0.9, double_sided: true, cull_mode: None, ..default() },
                extension: blend };
            let textures = handles.into_keys().collect();
            return Some(GpuMaterial { handle: CityMaterial::Ground(ctx.grounds.add(surface)), textures, refs: 0, fadeable: true, fades: Vec::new() });
        }
        let diffuse = match m.diffuse { Some(id) => Some(texture(id, false)?), None => None };
        let normal = m.normal.and_then(|id| texture(id, true));
        let textures = m.diffuse.into_iter().chain(m.normal.filter(|_| normal.is_some())).collect();
        // PORT: Bevy PBR replaces the native shaders (specular, the water shader); the MultiBlender layers are `ground`.
        let material = if m.water {
            StandardMaterial { base_color: Color::srgba(0.16, 0.24, 0.24, 0.55), normal_map_texture: normal, flip_normal_map_y: true,
                perceptual_roughness: 0.06, reflectance: 0.6, alpha_mode: AlphaMode::Blend, double_sided: true, cull_mode: None, ..default() }
        } else {
            StandardMaterial { base_color_texture: diffuse, normal_map_texture: normal, flip_normal_map_y: true,
                perceptual_roughness: 0.9, double_sided: true, cull_mode: None, alpha_mode: AlphaMode::Mask(0.5), ..default() }
        };
        Some(GpuMaterial { handle: CityMaterial::Plain(ctx.materials.add(material)), textures, refs: 0, fadeable: !m.water, fades: Vec::new() })
    }

    fn release_material(&mut self, id: u32, ctx: &mut Ctx) {
        let Some(m) = self.materials.get_mut(&id) else { return };
        m.refs -= 1;
        if m.refs > 0 { return; }
        let m = self.materials.remove(&id).unwrap();
        m.handle.remove(ctx);
        for h in m.fades.iter().flatten() { h.remove(ctx); }
        for t in m.textures {
            let Some(e) = self.textures.get_mut(&t) else { continue };
            e.1 -= 1;
            if e.1 == 0 {
                let (h, _) = self.textures.remove(&t).unwrap();
                ctx.images.remove(&h);
                self.data.resident.lock().unwrap().remove(&t);
            }
        }
    }

    fn unload(&mut self, c: usize, ctx: &mut Ctx) {
        let Cell::Loaded { objects, materials, extras } = std::mem::replace(&mut self.cells[c], Cell::Unloaded) else { return };
        for e in extras { ctx.commands.entity(e).despawn(); }
        self.effects_unloaded.push(c);
        self.sounds_unloaded.push(c);
        let gone: Vec<BatchKey> = self.batches.keys().filter(|k| k.cell == c).copied().collect();
        for k in gone {
            if let Some(e) = self.batches.remove(&k).and_then(|b| b.entity) { ctx.commands.entity(e).despawn(); }
        }
        for mut o in objects {
            for e in o.entities.drain(..) { ctx.commands.entity(e).despawn(); }
            for (e, _) in o.fade.take().into_iter().flat_map(|f| f.sections) { ctx.commands.entity(e).despawn(); }
            if let Some(p) = o.physics.take() { self.remove_physics(p, ctx); self.haystacks_dirty |= o.o.haystack.is_some() || kiosk_owner(&o.o); }
        }
        for m in materials { self.release_material(m, ctx); }
        for &e in self.fake_spans.get(&c).into_iter().flatten() { ctx.commands.entity(e).insert(Visibility::Inherited); }
    }

    /// `LODSelectorInstance__Submit` 0xA90160: the LOD distance is the camera's distance to the object's box (the
    /// visibility test 0xAC5CE0) times `lod_scale` = tan(fov_y / 2) · 2.414213 (0xA2F890: 1 at a 45° view). The picked
    /// LOD draws as usual; inside the fade width before its distance the next LOD is queued on top of it with blend
    /// factor alpha (`LodTable::fade`). The exe's fade pass (0xABE770, after the opaque pass) would draw it depth-biased
    /// toward the camera with src · alpha + dst · (1 − alpha) (D3D blend factor, 0xA4A580 mode 4), only on the pixels
    /// the picked LOD covered (each fading pair gets a stencil reference, 0xABB0E0); in the retail exe that pass is
    /// switched off (`lod_fade`), so the LOD switches at the distance.
    /// PORT (with `lod_fade`): Bevy has no per-draw stencil reference, so the fading LOD also blends where the picked one
    /// did not draw; the blend factor is kept in `FADE_LEVELS` steps.
    fn update_lods(&mut self, camera: Vec3, lod_scale: f32, ctx: &mut Ctx, budget: usize) {
        let batched = self.batch && !self.lod_fade;
        self.take_batches(ctx);
        // (distance, cell, object, wanted mesh, wanted fade) for every object whose drawn meshes must change
        let mut changes = Vec::new();
        for (c, cell) in self.cells.iter().enumerate() {
            let Cell::Loaded { objects, .. } = cell else { continue };
            for (k, o) in objects.iter().enumerate() {
                if o.o.meshes.is_empty() { continue; }
                let b = &o.o.visual_bounds;
                let raw = camera.distance(camera.clamp(b.min, b.max));
                let d = raw * lod_scale;
                let want = o.o.lod.pick(d).map(|m| m as usize).filter(|&m| m < o.o.meshes.len()).filter(|_| size_cull(o.o.size, raw).0);
                let fade = want.filter(|_| self.lod_fade).and(o.o.lod.fade(d)).map(|(m, a)| (m as usize, fade_level(a)))
                    .filter(|&(m, _)| m < o.o.meshes.len() && Some(m) != want);
                // drawn the other way (batched or per section) than the mode now asks
                let mode = if batched { !o.entities.is_empty() } else { !o.batches.is_empty() };
                if want != o.shown || mode || fade != o.fade.as_ref().map(|f| (f.mesh, f.level)) { changes.push((d, c, k, want, fade)); }
            }
        }
        changes.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut budget = budget;
        for (d, c, k, want, fade) in changes {
            let Cell::Loaded { objects, .. } = &mut self.cells[c] else { continue };
            let o = &mut objects[k];
            let b = &o.o.visual_bounds;
            let shadow = d <= SHADOW_DISTANCE && size_cull(o.o.size, camera.distance(camera.clamp(b.min, b.max))).1;
            let transform = Transform::from_matrix(o.o.transform);
            if batched {
                // into the batches, which are rebuilt below within the batch budget
                for e in o.entities.drain(..) { ctx.commands.entity(e).despawn(); }
                for (e, _) in o.fade.take().into_iter().flat_map(|f| f.sections) { ctx.commands.entity(e).despawn(); }
                for key in o.batches.drain(..) {
                    let Some(b) = self.batches.get_mut(&key) else { continue };
                    b.members.retain(|&(obj, _, _)| obj != k as u32);
                    b.dirty = true;
                    b.near = b.near.min(d);
                }
                o.shown = want;
                let Some(m) = want else { continue };
                let run = (k / BATCH_OBJECTS) as u32;
                for (si, section) in o.o.meshes[m].mesh.sections.iter().enumerate() {
                    if !self.materials.contains_key(&section.material) { continue; }
                    let key = BatchKey { cell: c, run, material: section.material, shadow };
                    let next = &mut self.next_batch;
                    let b = self.batches.entry(key).or_insert_with(|| {
                        *next += 1;
                        Batch { members: Vec::new(), entity: None, dirty: true, building: false, id: *next, near: d }
                    });
                    b.members.push((k as u32, m as u8, si as u16));
                    b.dirty = true;
                    b.near = b.near.min(d);
                    if !o.batches.contains(&key) { o.batches.push(key); }
                }
                continue;
            }
            let rebatched = !o.batches.is_empty();
            for key in o.batches.drain(..) {
                if let Some(b) = self.batches.get_mut(&key) { b.members.retain(|&(obj, _, _)| obj != k as u32); b.dirty = true; }
            }
            if rebatched { o.shown = None; }
            if want != o.shown {
                // the LOD that was fading in becomes the drawn one: keep its entities, give them the opaque materials
                let promote = want.is_some() && o.fade.as_ref().is_some_and(|f| Some(f.mesh) == want && f.complete);
                let cost = if promote { 0 } else { want.map_or(0, |m| o.o.meshes[m].mesh.positions.len()) };
                if cost > budget && want.is_some() { continue; }
                budget = budget.saturating_sub(cost);
                self.work[2] += cost;
                for e in o.entities.drain(..) { ctx.commands.entity(e).despawn(); }
                o.shown = want;
                if promote {
                    for (e, id) in o.fade.take().unwrap().sections {
                        let Some(m) = self.materials.get(&id) else { ctx.commands.entity(e).despawn(); continue };
                        let mut e = ctx.commands.entity(e);
                        m.handle.insert(&mut e);
                        if shadow { e.remove::<NotShadowCaster>(); }
                        o.entities.push(e.id());
                    }
                } else if let Some(m) = want {
                    let lod = &o.o.meshes[m];
                    let handles: Vec<_> = lod.mesh.sections.iter().map(|s| self.materials.get(&s.material).map(|m| m.handle.clone())).collect();
                    o.entities = spawn_lod(lod, &handles, transform, shadow, ctx).into_iter().map(|(e, _)| e).collect();
                }
            }
            if o.shown != want { continue; }
            match (fade, &mut o.fade) {
                (None, None) => {}
                (Some((m, level)), Some(f)) if f.mesh == m => {
                    if f.level == level { continue; }
                    f.level = level;
                    for &(e, id) in &f.sections {
                        if let Some(h) = fade_material(&mut self.materials, ctx, id, level) { h.insert(&mut ctx.commands.entity(e)); }
                    }
                }
                (fade, current) => {
                    for (e, _) in current.take().into_iter().flat_map(|f| f.sections) { ctx.commands.entity(e).despawn(); }
                    let Some((m, level)) = fade else { continue };
                    let lod = &o.o.meshes[m];
                    let cost = lod.mesh.positions.len();
                    if cost > budget { continue; }
                    budget -= cost;
                    let handles: Vec<_> = lod.mesh.sections.iter().map(|s| fade_material(&mut self.materials, ctx, s.material, level)).collect();
                    let complete = handles.iter().all(Option::is_some);
                    o.fade = Some(FadeIn { mesh: m, level, sections: spawn_lod(lod, &handles, transform, false, ctx), complete });
                }
            }
        }
        if batched { self.rebuild_batches(ctx, if budget == usize::MAX { usize::MAX } else { BATCH_VERTEX_BUDGET }); }
    }

    /// Send the changed batches to the builder threads, nearest first, within `budget` vertices (one always goes); an
    /// emptied batch is removed at once.
    fn rebuild_batches(&mut self, ctx: &mut Ctx, budget: usize) {
        let mut dirty: Vec<(f32, BatchKey)> = self.batches.iter().filter(|(_, b)| b.dirty && !b.building).map(|(k, b)| (b.near, *k)).collect();
        dirty.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut left = budget;
        for (_, key) in dirty {
            let b = self.batches.get_mut(&key).unwrap();
            if b.members.is_empty() {
                if let Some(e) = b.entity { ctx.commands.entity(e).despawn(); }
                self.batches.remove(&key);
                continue;
            }
            let Cell::Loaded { objects, .. } = &self.cells[key.cell] else { continue };
            let mut parts = Vec::with_capacity(b.members.len());
            let mut vertices = 0;
            b.members.sort_unstable();
            for &(obj, m, si) in &b.members {
                let o = &objects[obj as usize];
                let lod = &o.o.meshes[m as usize];
                vertices += lod.mesh.sections[si as usize].indices.len();
                parts.push((lod.mesh.clone(), lod.colors.clone(), si as usize, Transform::from_matrix(o.o.transform).to_matrix()));
            }
            if vertices > left && left < budget { break; }
            left = left.saturating_sub(vertices);
            self.work[2] += vertices;
            b.dirty = false;
            b.building = true;
            b.near = f32::INFINITY;
            let layered = self.materials.get(&key.material).is_some_and(|m| m.handle.layered());
            let _ = self.builder.jobs.send(BatchJob { key, id: b.id, layered, parts });
        }
    }

    /// Take the finished batch meshes (about `BATCH_VERTEX_BUDGET` vertices per frame) and draw them.
    fn take_batches(&mut self, ctx: &mut Ctx) {
        let mut taken = 0;
        while taken < BATCH_VERTEX_BUDGET {
            let next = self.builder.results.lock().unwrap().try_recv();
            let Ok((key, id, mesh)) = next else { break };
            taken += mesh.count_vertices();
            self.apply_batch(key, id, mesh, ctx);
        }
    }

    fn apply_batch(&mut self, key: BatchKey, id: u64, mesh: Mesh, ctx: &mut Ctx) {
        let Some(b) = self.batches.get_mut(&key).filter(|b| b.id == id) else { return };
        b.building = false;
        let Some(material) = self.materials.get(&key.material) else { return };
        let handle = ctx.meshes.add(mesh);
        match b.entity {
            Some(e) => { ctx.commands.entity(e).insert(Mesh3d(handle)); }
            None => {
                let mut e = ctx.commands.spawn((crate::map_menu::MapEntity, Mesh3d(handle), Transform::IDENTITY));
                material.handle.insert(&mut e);
                if !key.shadow { e.insert(NotShadowCaster); }
                b.entity = Some(e.id());
            }
        }
    }

    fn update_physics(&mut self, player: Vec3, ctx: &mut Ctx, budget: usize) {
        let (limit, mut budget) = (budget, budget);
        let mut add = Vec::new();
        for (c, cell) in self.cells.iter_mut().enumerate() {
            let Cell::Loaded { objects, .. } = cell else { continue };
            for (k, o) in objects.iter_mut().enumerate() {
                let d = xz_distance(player, &o.o.bounds);
                if o.physics.is_none() && d < PHYSICS_RADIUS && (!o.o.collision.is_empty() || !o.o.edges.is_empty()) { add.push((d, c, k)); }
                else if o.physics.is_some() && d > PHYSICS_RADIUS + PHYSICS_MARGIN {
                    let p = o.physics.take().unwrap();
                    self.haystacks_dirty |= o.o.haystack.is_some() || kiosk_owner(&o.o);
                    remove_physics(&mut self.edge_free, p, ctx);
                }
            }
        }
        add.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, c, k) in add {
            let Cell::Loaded { objects, .. } = &mut self.cells[c] else { continue };
            let o = &mut objects[k];
            if o.o.collision.len() > budget && budget < limit { break; }
            budget = budget.saturating_sub(o.o.collision.len());
            self.work[3] += o.o.collision.len();
            let triangles = ctx.collision.add_triangles(o.o.collision.clone());
            let mut edges = Vec::with_capacity(o.o.edges.len());
            for e in &o.o.edges {
                match self.edge_free.pop() {
                    Some(slot) => { ctx.guidance.edges[slot] = e.clone(); edges.push(slot); }
                    None => { ctx.guidance.edges.push(e.clone()); edges.push(ctx.guidance.edges.len() - 1); }
                }
            }
            o.physics = Some(Physics { triangles, edges });
            self.haystacks_dirty |= o.o.haystack.is_some() || kiosk_owner(&o.o);
        }
    }

    fn remove_physics(&mut self, p: Physics, ctx: &mut Ctx) { remove_physics(&mut self.edge_free, p, ctx); }

    fn count(&mut self) {
        let mut s = CityStats { materials: self.materials.len(), textures: self.textures.len(), errors: self.stats.errors, phase_ms: self.stats.phase_ms,
            work: self.stats.work, ..default() };
        for cell in &self.cells {
            match cell {
                Cell::Loading => s.loading += 1,
                Cell::Loaded { objects, extras, .. } => {
                    s.lights += extras.len();
                    s.loaded += 1;
                    s.objects += objects.len();
                    for o in objects {
                        s.drawn += o.shown.is_some() as usize;
                        s.entities += o.entities.len() + o.fade.as_ref().map_or(0, |f| f.sections.len());
                        s.fading += o.fade.is_some() as usize;
                        if let Some(p) = &o.physics { s.physics_objects += 1; s.physics_triangles += p.triangles.len(); s.physics_edges += p.edges.len(); }
                    }
                }
                Cell::Unloaded => {}
            }
        }
        s.batches = self.batches.len();
        s.batches_pending = self.batches.values().filter(|b| b.dirty || b.building).count();
        s.entities += self.batches.values().filter(|b| b.entity.is_some()).count();
        self.stats = s;
    }

    /// The distant city: one entity per fake span (hidden while its cell is loaded) plus each section's uncovered rest.
    pub fn spawn_fakes(&mut self, blocks: Vec<FakeBlock>, ctx: &mut Ctx) {
        for b in blocks {
            let transform = Transform::from_matrix(b.transform);
            for (si, section) in b.mesh.sections.iter().enumerate() {
                let Some(material) = self.materials.get(&section.material).map(|m| m.handle.clone()).or_else(|| {
                    let m = self.data.materials.material(&self.data.archives, section.material).ok()?;
                    let gpu = self.material(&m, ctx)?;
                    let h = gpu.handle.clone();
                    self.materials.insert(section.material, GpuMaterial { refs: 1, ..gpu });
                    Some(h)
                }) else { continue };
                let mut covered = vec![false; section.indices.len()];
                let spans: Vec<_> = b.spans.iter().filter(|s| s.section == si).collect();
                for s in &spans {
                    let range = s.first as usize..(s.first + s.count).min(section.indices.len() as u32) as usize;
                    if range.is_empty() { continue; }
                    for f in &mut covered[range.clone()] { *f = true; }
                    let mesh = section_mesh(&b.mesh, b.colors.as_deref(), &section.indices[range], material.layered());
                    let loaded = s.cell.is_some_and(|c| matches!(self.cells[c], Cell::Loaded { .. }));
                    let mut e = ctx.commands.spawn((crate::map_menu::MapEntity, Mesh3d(ctx.meshes.add(mesh)), transform,
                        NotShadowCaster, if loaded { Visibility::Hidden } else { Visibility::Inherited }));
                    material.insert(&mut e);
                    let e = e.id();
                    if let Some(c) = s.cell { self.fake_spans.entry(c).or_default().push(e); }
                }
                let rest: Vec<u32> = section.indices.chunks_exact(3).enumerate().filter(|(t, _)| !covered[3 * t]).flat_map(|(_, t)| t.iter().copied()).collect();
                if !rest.is_empty() {
                    let mut e = ctx.commands.spawn((crate::map_menu::MapEntity, Mesh3d(ctx.meshes.add(section_mesh(&b.mesh, b.colors.as_deref(), &rest, material.layered()))),
                        transform, NotShadowCaster));
                    material.insert(&mut e);
                }
            }
        }
    }

}

/// Which grid cells have a non-empty stored file (empty cells are 104-byte files).
pub fn content_mask(data: &CityData) -> Vec<bool> {
    data.grid.cells.iter().map(|c| {
        data.archives.location(c.datablock).and_then(|(_, f)| data.archives.forges[0].find(f)).is_some_and(|e| e.size > 104)
    }).collect()
}

/// The cells `GridStreamer__Update` 0x55D810 wants with the player at `focus` (Bevy space): every cell (with content)
/// under the square box of the loading radius at the player's cell, ordered as 0x55D210 requests them — by
/// 0.6 · (1 − cos(angle between the view and the cell centre)) + 0.4 · d² / cell² (d to the cell centre, the view
/// flattened; without a view only the distance term counts).
pub fn wanted_cells(data: &CityData, has_content: &[bool], focus: Vec3, view: Option<Vec3>) -> Vec<usize> {
    let g = &data.grid;
    let p = game_xy(focus);
    let r = g.loading_radius(p) as i32;
    let dir = view.map(|v| game_xy(v).normalize_or_zero());
    let mut v: Vec<(f32, usize)> = (0..g.cells.len()).filter(|&c| has_content[c] && g.cell_in_box(&g.cells[c], p, r)).map(|c| {
        let (min, max) = g.cell_rect(&g.cells[c]);
        let to = (min + max) * 0.5 - p;
        let d2 = to.length_squared();
        let cos = match dir { Some(d) if d2 > 0.0 => d.dot(to / d2.sqrt()), _ => 1.0 };
        ((1.0 - cos) * VIEW_WEIGHT + (1.0 - VIEW_WEIGHT) * d2 / (g.cell_size * g.cell_size), c)
    }).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|d| d.1).collect()
}

/// An object whose guidance holds Kiosk pieces (its forward is the Kiosk context's axis).
fn kiosk_owner(o: &CityObject) -> bool {
    o.edges.iter().any(|e| e.subtype == GuidanceSubType::Kiosk)
}

fn remove_physics(edge_free: &mut Vec<usize>, p: Physics, ctx: &mut Ctx) {
    ctx.collision.remove_triangles(&p.triangles);
    let parked = Vec3::splat(PARKED);
    for slot in p.edges {
        ctx.guidance.edges[slot] = GuidanceEdge { p0: parked, p1: parked, n0: Vec3::Y, n1: Vec3::Z, subtype: GuidanceSubType::None };
        edge_free.push(slot);
    }
}

/// One mesh section with only the vertices it uses.
/// One entity per mesh section that has a material; returns (entity, material id).
fn spawn_lod(lod: &LodMesh, materials: &[Option<CityMaterial>], transform: Transform, shadow: bool, ctx: &mut Ctx) -> Vec<(Entity, u32)> {
    let mut out = Vec::with_capacity(lod.mesh.sections.len());
    for (section, material) in lod.mesh.sections.iter().zip(materials) {
        let Some(material) = material else { continue };
        let mesh = section_mesh(&lod.mesh, lod.colors.as_deref().map(|c| c.as_slice()), &section.indices, material.layered());
        let mut e = ctx.commands.spawn((crate::map_menu::MapEntity, Mesh3d(ctx.meshes.add(mesh)), transform));
        material.insert(&mut e);
        if !shadow { e.insert(NotShadowCaster); }
        out.push((e.id(), section.material));
    }
    out
}

/// The cross-fade's blend factor as one of `FADE_LEVELS` steps (1..=FADE_LEVELS).
fn fade_level(alpha: f32) -> u8 { ((alpha * FADE_LEVELS as f32).round() as u8).clamp(1, FADE_LEVELS as u8) }

/// The translucent copy of a material for a fade level: the blend factor as alpha, drawn depth-biased toward the camera.
fn fade_material(materials: &mut HashMap<u32, GpuMaterial>, ctx: &mut Ctx, id: u32, level: u8) -> Option<CityMaterial> {
    let m = materials.get_mut(&id).filter(|m| m.fadeable)?;
    if m.fades.is_empty() { m.fades = vec![None; FADE_LEVELS]; }
    let slot = &mut m.fades[level as usize - 1];
    if slot.is_none() {
        let alpha = level as f32 / FADE_LEVELS as f32;
        let fade = |base: StandardMaterial| StandardMaterial { base_color: base.base_color.with_alpha(alpha), alpha_mode: AlphaMode::Blend, depth_bias: FADE_DEPTH_BIAS, ..base };
        *slot = Some(match &m.handle {
            CityMaterial::Plain(h) => { let base = ctx.materials.get(h)?.clone(); CityMaterial::Plain(ctx.materials.add(fade(base))) }
            CityMaterial::Ground(h) => {
                let g = ctx.grounds.get(h)?.clone();
                CityMaterial::Ground(ctx.grounds.add(GroundSurface { base: fade(g.base), extension: g.extension }))
            }
        });
    }
    slot.clone()
}

/// `layered`: the section draws with a layered material (`ground`), whose weights ride in UV_1.
pub(crate) fn section_mesh(m: &crate::assets::static_mesh::StaticMesh, colors: Option<&[[u8; 4]]>, indices: &[u32], layered: bool) -> Mesh {
    let mut map = vec![u32::MAX; m.positions.len()];
    let (mut pos, mut nor, mut tan, mut uv, mut col, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::with_capacity(indices.len()));
    let blended = layered && m.blend.len() == m.positions.len();
    let mut weights = Vec::new();
    for &i in indices {
        let i = i as usize;
        if map[i] == u32::MAX {
            map[i] = pos.len() as u32;
            pos.push(m.positions[i].to_array());
            nor.push(m.normals[i].to_array());
            tan.push(m.tangents[i].to_array());
            uv.push(m.uvs[i].to_array());
            col.push(match colors { Some(c) => c[i].map(|b| b as f32 / 255.0), None => m.colors[i] });
            if blended { weights.push([m.blend[i][0] as f32 / 255.0, m.blend[i][1] as f32 / 255.0]); }
        }
        idx.push(map[i]);
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tan);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    if blended { mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, weights); }
    mesh.insert_indices(Indices::U32(idx));
    mesh
}

/// One batch mesh: each part's section in world space (the drawn transform applied as Bevy's mesh shaders apply it:
/// normals by the inverse transpose, tangents by the matrix with the determinant's sign on w), in the parts' order.
/// `layered`: the parts' blend weights ride in UV_1 (zero for a part without them, which the shader reads the same).
fn batch_mesh(parts: &[(Arc<crate::assets::static_mesh::StaticMesh>, Option<Arc<Vec<[u8; 4]>>>, usize, Mat4)], layered: bool) -> Mesh {
    let (mut pos, mut nor, mut tan, mut uv, mut col, mut idx, mut weights) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (m, colors, section, matrix) in parts {
        let linear = Mat3::from_mat4(*matrix);
        let normal_matrix = linear.inverse().transpose();
        let sign = if linear.determinant() < 0.0 { -1.0 } else { 1.0 };
        let blended = layered && m.blend.len() == m.positions.len();
        let mut map = vec![u32::MAX; m.positions.len()];
        for &i in &m.sections[*section].indices {
            let i = i as usize;
            if map[i] == u32::MAX {
                map[i] = pos.len() as u32;
                pos.push(matrix.transform_point3(m.positions[i]).to_array());
                let n = m.normals[i];
                nor.push(if n == Vec3::ZERO { n } else { (normal_matrix * n).normalize() }.to_array());
                let t = m.tangents[i];
                tan.push(if t == Vec4::ZERO { t } else { (linear * t.truncate()).normalize().extend(t.w * sign) }.to_array());
                uv.push(m.uvs[i].to_array());
                col.push(match colors { Some(c) => c[i].map(|b| b as f32 / 255.0), None => m.colors[i] });
                if layered { weights.push(if blended { [m.blend[i][0] as f32 / 255.0, m.blend[i][1] as f32 / 255.0] } else { [0.0; 2] }); }
            }
            idx.push(map[i]);
        }
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tan);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    if layered && weights.iter().any(|w| *w != [0.0; 2]) { mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, weights); }
    mesh.insert_indices(Indices::U32(idx));
    mesh
}

pub(crate) fn image(t: &TextureData, _linear: bool) -> Image {
    let mut img = Image::new_uninit(Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 }, TextureDimension::D2, t.format, RenderAssetUsages::RENDER_WORLD);
    img.texture_descriptor.mip_level_count = t.mips;
    img.data = Some(t.data.clone());
    // PORT: repeat sampling until TextureMapSpec's sampler fields are decoded (as RE/14).
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { address_mode_u: ImageAddressMode::Repeat, address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: 8, ..ImageSamplerDescriptor::linear() });
    img
}
