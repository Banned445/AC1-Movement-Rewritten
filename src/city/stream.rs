//! Runtime streaming of a city world (RE/17 §5): grid cells load and unload around the player on loader threads,
//! objects switch LOD meshes by camera distance, and the distant city is drawn by the World's fake meshes.
//!
//! Game-side rules used: the 6-level grid and its cells (§1), the LODSelector distance bands (§3.2) and the fake
//! index spans per finest cell (§6). The exe's loading distances, LOD scale and fake switching were not traced, so
//! the radii below are stand-ins:
//! PORT: `LOAD_RADIUS` 160 m (the data's RegionCellDataLoadingDistance resources are named "LR - Portal (160m)"),
//! unload 32 m further; a cell of any level loads when the player is within the radius of its rectangle.
//! PORT: movement only sees objects within `PHYSICS_RADIUS` (40 m) of the player (their triangles, guidance edges, haystacks
//! and posts enter the shared CollisionWorld / GuidanceWorld in stable slots). Every movement query reaches far less
//! than that radius, so their answers are those of the whole city; this replaces Havok's broadphase and the guidance
//! partitioner, not a game rule.

use std::{collections::{HashMap, VecDeque}, sync::{mpsc, Arc, Condvar, Mutex}};

use bevy::{asset::RenderAssetUsages, image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor}, light::NotShadowCaster,
    mesh::{Indices, PrimitiveTopology}, prelude::*, render::render_resource::{Extent3d, TextureDimension}};

use super::{decode::{decode_cell, CityData, CityObject, DecodedCell}, fakes::FakeBlock, material::{MaterialData, TextureData}};
use crate::{collision::CollisionWorld, guidance::{GuidanceEdge, GuidanceSubType, GuidanceWorld}};

pub const LOAD_RADIUS: f32 = 160.0;
pub const UNLOAD_MARGIN: f32 = 32.0;
pub const PHYSICS_RADIUS: f32 = 40.0;
pub const PHYSICS_MARGIN: f32 = 12.0;
/// PORT: per-frame budget of mesh vertices built for LOD switches (spreads the cost of entering a district).
const LOD_VERTEX_BUDGET: usize = 400_000;
/// PORT: per-frame budget of collision triangles moved into the physics window.
const PHYSICS_TRIANGLE_BUDGET: usize = 40_000;
/// PORT: objects past this camera distance draw without casting shadows.
const SHADOW_DISTANCE: f32 = 90.0;
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

struct Physics {
    triangles: Vec<u32>,
    edges: Vec<usize>,
}

struct Obj {
    o: CityObject,
    shown: Option<usize>,
    entities: Vec<Entity>,
    physics: Option<Physics>,
}

enum Cell {
    Unloaded,
    Loading,
    Loaded { objects: Vec<Obj>, materials: Vec<u32> },
}

struct GpuMaterial {
    handle: Handle<StandardMaterial>,
    textures: Vec<u32>,
    refs: u32,
}

/// Counters for the debug overlay and tests.
#[derive(Default, Clone, Debug)]
pub struct CityStats {
    pub loaded: usize,
    pub loading: usize,
    pub objects: usize,
    pub drawn: usize,
    pub physics_objects: usize,
    pub physics_triangles: usize,
    pub physics_edges: usize,
    pub materials: usize,
    pub textures: usize,
    pub errors: usize,
    /// Last frame's streaming time per phase (ms): integrate, plan, LOD, physics, refresh.
    pub phase_ms: [f32; 5],
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
    pilotis_dirty: bool,
    haystacks_dirty: bool,
    plan_timer: f32,
    pub stats: CityStats,
    /// (seconds, frames, worst frame) for AC_CITY_LOG
    pub log: (f32, u32, f32),
    /// AC_CITY_TOUR start point
    pub tour: Option<Vec3>,
}

#[derive(Resource, Default)]
pub struct City(pub Option<Box<CityState>>);

/// The game coordinates (x, y) of a Bevy position.
fn game_xy(p: Vec3) -> Vec2 { Vec2::new(p.x, -p.z) }

fn rect_distance(p: Vec2, (min, max): (Vec2, Vec2)) -> f32 {
    (p - p.clamp(min, max)).length()
}

fn xz_distance(p: Vec3, b: &crate::collision::Aabb3) -> f32 {
    let q = Vec2::new(p.x, p.z);
    (q - q.clamp(Vec2::new(b.min.x, b.min.z), Vec2::new(b.max.x, b.max.z))).length()
}

pub struct Ctx<'a, 'w, 's> {
    pub commands: &'a mut Commands<'w, 's>,
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<StandardMaterial>,
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
            pilotis_dirty: true, haystacks_dirty: true, plan_timer: 0.0, stats: CityStats::default(), log: (0.0, 0, 0.0), tour: None }
    }

    /// Cells with content within `radius` of `focus` (Bevy space), nearest first.
    pub fn cells_within(&self, focus: Vec3, radius: f32) -> Vec<usize> {
        cells_within(&self.data, &self.has_content, focus, radius)
    }

    /// Per frame: take finished cells, plan loads/unloads, switch LODs, move objects in and out of the physics window.
    pub fn update(&mut self, player: Vec3, camera: Vec3, dt: f32, ctx: &mut Ctx) {
        let mut phase = [0.0f32; 5];
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
            if started.elapsed().as_secs_f32() > 0.004 { break; }
        }
        lap(0, &mut phase);
        self.plan_timer -= dt;
        if self.plan_timer <= 0.0 {
            self.plan_timer = 0.25;
            self.plan(player, ctx);
        }
        lap(1, &mut phase);
        self.update_lods(camera, ctx, LOD_VERTEX_BUDGET);
        lap(2, &mut phase);
        self.update_physics(player, ctx, PHYSICS_TRIANGLE_BUDGET);
        lap(3, &mut phase);
        self.refresh(ctx);
        lap(4, &mut phase);
        self.count();
        self.stats.phase_ms = phase;
    }

    /// The first frame: every LOD and the whole physics window at once (no budgets).
    pub fn prime(&mut self, player: Vec3, camera: Vec3, ctx: &mut Ctx) {
        self.update_physics(player, ctx, usize::MAX);
        self.update_lods(camera, ctx, usize::MAX);
        self.refresh(ctx);
        self.count();
    }

    fn refresh(&mut self, ctx: &mut Ctx) {
        if self.pilotis_dirty {
            self.pilotis_dirty = false;
            let mut all: Vec<Vec3> = Vec::new();
            for cell in &self.cells {
                let Cell::Loaded { objects, .. } = cell else { continue };
                for o in objects.iter().filter(|o| o.physics.is_some()) {
                    for &p in &o.o.pilotis { if !all.iter().any(|q| (*q - p).length() < 0.1) { all.push(p); } }
                }
            }
            ctx.guidance.jump_pilotis = Some(all);
        }
        if self.haystacks_dirty {
            self.haystacks_dirty = false;
            ctx.guidance.haystacks = self.cells.iter().filter_map(|c| match c { Cell::Loaded { objects, .. } => Some(objects), _ => None })
                .flatten().filter(|o| o.physics.is_some()).filter_map(|o| o.o.haystack).collect();
        }
    }

    fn plan(&mut self, player: Vec3, ctx: &mut Ctx) {
        let p = game_xy(player);
        let mut wanted = Vec::new();
        for c in 0..self.cells.len() {
            if !self.has_content[c] { continue; }
            let d = rect_distance(p, self.data.grid.cell_rect(&self.data.grid.cells[c]));
            match self.cells[c] {
                Cell::Unloaded if d < LOAD_RADIUS => wanted.push((d, c)),
                Cell::Loading if d < LOAD_RADIUS + UNLOAD_MARGIN => wanted.push((d, c)),
                Cell::Loading => self.cells[c] = Cell::Unloaded,
                Cell::Loaded { .. } if d > LOAD_RADIUS + UNLOAD_MARGIN => self.unload(c, ctx),
                _ => {}
            }
        }
        wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
        for &(_, c) in &wanted { self.cells[c] = Cell::Loading; }
        self.loader.request(wanted.into_iter().map(|w| w.1).collect());
    }

    pub fn integrate(&mut self, d: DecodedCell, ctx: &mut Ctx) {
        for (id, linear, t) in &d.textures {
            if self.textures.contains_key(id) { continue; }
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
        let objects = d.objects.into_iter().map(|o| Obj { o, shown: None, entities: Vec::new(), physics: None }).collect();
        self.cells[d.cell] = Cell::Loaded { objects, materials: used };
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
        let diffuse = match m.diffuse { Some(id) => Some(texture(id, false)?), None => None };
        let normal = m.normal.and_then(|id| texture(id, true));
        let textures = m.diffuse.into_iter().chain(m.normal.filter(|_| normal.is_some())).collect();
        // PORT: Bevy PBR replaces the native shaders (MultiBlender layers, specular, the water shader).
        let material = if m.water {
            StandardMaterial { base_color: Color::srgba(0.16, 0.24, 0.24, 0.55), normal_map_texture: normal, flip_normal_map_y: true,
                perceptual_roughness: 0.06, reflectance: 0.6, alpha_mode: AlphaMode::Blend, double_sided: true, cull_mode: None, ..default() }
        } else {
            StandardMaterial { base_color_texture: diffuse, normal_map_texture: normal, flip_normal_map_y: true,
                perceptual_roughness: 0.9, double_sided: true, cull_mode: None, alpha_mode: AlphaMode::Mask(0.5), ..default() }
        };
        Some(GpuMaterial { handle: ctx.materials.add(material), textures, refs: 0 })
    }

    fn release_material(&mut self, id: u32, ctx: &mut Ctx) {
        let Some(m) = self.materials.get_mut(&id) else { return };
        m.refs -= 1;
        if m.refs > 0 { return; }
        let m = self.materials.remove(&id).unwrap();
        ctx.materials.remove(&m.handle);
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
        let Cell::Loaded { objects, materials } = std::mem::replace(&mut self.cells[c], Cell::Unloaded) else { return };
        for mut o in objects {
            for e in o.entities.drain(..) { ctx.commands.entity(e).despawn(); }
            if let Some(p) = o.physics.take() { self.remove_physics(p, ctx); self.haystacks_dirty |= o.o.haystack.is_some(); self.pilotis_dirty |= !o.o.pilotis.is_empty(); }
        }
        for m in materials { self.release_material(m, ctx); }
        for &e in self.fake_spans.get(&c).into_iter().flatten() { ctx.commands.entity(e).insert(Visibility::Inherited); }
    }

    fn update_lods(&mut self, camera: Vec3, ctx: &mut Ctx, budget: usize) {
        // (distance, cell, object, wanted mesh) for every object whose drawn mesh must change
        let mut changes = Vec::new();
        for (c, cell) in self.cells.iter().enumerate() {
            let Cell::Loaded { objects, .. } = cell else { continue };
            for (k, o) in objects.iter().enumerate() {
                if o.o.meshes.is_empty() { continue; }
                let d = (o.o.center.distance(camera) - 0.0).max(0.0);
                let want = o.o.lods.iter().find(|b| d < b.0).and_then(|b| b.1);
                if want != o.shown { changes.push((d, c, k, want)); }
            }
        }
        changes.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut budget = budget;
        for (d, c, k, want) in changes {
            let Cell::Loaded { objects, .. } = &mut self.cells[c] else { continue };
            let o = &mut objects[k];
            let cost = want.map_or(0, |m| o.o.meshes[m].mesh.positions.len());
            if cost > budget && want.is_some() { continue; }
            budget = budget.saturating_sub(cost);
            for e in o.entities.drain(..) { ctx.commands.entity(e).despawn(); }
            o.shown = want;
            let Some(m) = want else { continue };
            let lod = &o.o.meshes[m];
            let transform = Transform::from_matrix(o.o.transform);
            for section in &lod.mesh.sections {
                let Some(material) = self.materials.get(&section.material) else { continue };
                let mesh = section_mesh(&lod.mesh, lod.colors.as_deref().map(|c| c.as_slice()), &section.indices);
                let mut e = ctx.commands.spawn((crate::map_menu::MapEntity, Mesh3d(ctx.meshes.add(mesh)), MeshMaterial3d(material.handle.clone()), transform));
                if d > SHADOW_DISTANCE { e.insert(NotShadowCaster); }
                o.entities.push(e.id());
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
                    self.haystacks_dirty |= o.o.haystack.is_some();
                    self.pilotis_dirty |= !o.o.pilotis.is_empty();
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
            let triangles = ctx.collision.add_triangles(o.o.collision.clone());
            let mut edges = Vec::with_capacity(o.o.edges.len());
            for e in &o.o.edges {
                match self.edge_free.pop() {
                    Some(slot) => { ctx.guidance.edges[slot] = e.clone(); edges.push(slot); }
                    None => { ctx.guidance.edges.push(e.clone()); edges.push(ctx.guidance.edges.len() - 1); }
                }
            }
            o.physics = Some(Physics { triangles, edges });
            self.haystacks_dirty |= o.o.haystack.is_some();
            self.pilotis_dirty |= !o.o.pilotis.is_empty();
        }
    }

    fn remove_physics(&mut self, p: Physics, ctx: &mut Ctx) { remove_physics(&mut self.edge_free, p, ctx); }

    fn count(&mut self) {
        let mut s = CityStats { materials: self.materials.len(), textures: self.textures.len(), errors: self.stats.errors, phase_ms: self.stats.phase_ms, ..default() };
        for cell in &self.cells {
            match cell {
                Cell::Loading => s.loading += 1,
                Cell::Loaded { objects, .. } => {
                    s.loaded += 1;
                    s.objects += objects.len();
                    for o in objects {
                        s.drawn += o.shown.is_some() as usize;
                        if let Some(p) = &o.physics { s.physics_objects += 1; s.physics_triangles += p.triangles.len(); s.physics_edges += p.edges.len(); }
                    }
                }
                Cell::Unloaded => {}
            }
        }
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
                    let mesh = section_mesh(&b.mesh, b.colors.as_deref(), &section.indices[range]);
                    let loaded = s.cell.is_some_and(|c| matches!(self.cells[c], Cell::Loaded { .. }));
                    let e = ctx.commands.spawn((crate::map_menu::MapEntity, Mesh3d(ctx.meshes.add(mesh)), MeshMaterial3d(material.clone()), transform,
                        NotShadowCaster, if loaded { Visibility::Hidden } else { Visibility::Inherited })).id();
                    if let Some(c) = s.cell { self.fake_spans.entry(c).or_default().push(e); }
                }
                let rest: Vec<u32> = section.indices.chunks_exact(3).enumerate().filter(|(t, _)| !covered[3 * t]).flat_map(|(_, t)| t.iter().copied()).collect();
                if !rest.is_empty() {
                    ctx.commands.spawn((crate::map_menu::MapEntity, Mesh3d(ctx.meshes.add(section_mesh(&b.mesh, b.colors.as_deref(), &rest))),
                        MeshMaterial3d(material.clone()), transform, NotShadowCaster));
                }
            }
        }
    }

    /// Is every cell within `radius` of `p` loaded?
    pub fn settled(&self, p: Vec3, radius: f32) -> bool {
        self.cells_within(p, radius).iter().all(|&c| matches!(self.cells[c], Cell::Loaded { .. }))
    }
}

/// Which grid cells have a non-empty stored file (empty cells are 104-byte files).
pub fn content_mask(data: &CityData) -> Vec<bool> {
    data.grid.cells.iter().map(|c| {
        data.archives.location(c.datablock).and_then(|(_, f)| data.archives.forges[0].find(f)).is_some_and(|e| e.size > 104)
    }).collect()
}

/// Cells with content within `radius` of `focus` (Bevy space), nearest first.
pub fn cells_within(data: &CityData, has_content: &[bool], focus: Vec3, radius: f32) -> Vec<usize> {
    let p = game_xy(focus);
    let mut v: Vec<(f32, usize)> = (0..data.grid.cells.len()).filter(|&c| has_content[c])
        .map(|c| (rect_distance(p, data.grid.cell_rect(&data.grid.cells[c])), c)).filter(|d| d.0 < radius).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|d| d.1).collect()
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
fn section_mesh(m: &crate::assets::static_mesh::StaticMesh, colors: Option<&[[u8; 4]]>, indices: &[u32]) -> Mesh {
    let mut map = vec![u32::MAX; m.positions.len()];
    let (mut pos, mut nor, mut tan, mut uv, mut col, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::with_capacity(indices.len()));
    for &i in indices {
        let i = i as usize;
        if map[i] == u32::MAX {
            map[i] = pos.len() as u32;
            pos.push(m.positions[i].to_array());
            nor.push(m.normals[i].to_array());
            tan.push(m.tangents[i].to_array());
            uv.push(m.uvs[i].to_array());
            col.push(match colors { Some(c) => c[i].map(|b| b as f32 / 255.0), None => m.colors[i] });
        }
        idx.push(map[i]);
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tan);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    mesh.insert_indices(Indices::U32(idx));
    mesh
}

fn image(t: &TextureData, _linear: bool) -> Image {
    let mut img = Image::new_uninit(Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 }, TextureDimension::D2, t.format, RenderAssetUsages::RENDER_WORLD);
    img.texture_descriptor.mip_level_count = t.mips;
    img.data = Some(t.data.clone());
    // PORT: repeat sampling until TextureMapSpec's sampler fields are decoded (as RE/14).
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { address_mode_u: ImageAddressMode::Repeat, address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: 8, ..ImageSamplerDescriptor::linear() });
    img
}
