//! Grid cell → static city objects (RE/17 §3). Runs on loader threads; everything it returns is in Bevy space
//! (game (x, y, z) → (x, z, −y), as in RE/14).
//!
//! An entity body is `{u32 id, u32 class Entity, 64-byte placement, components…}` (Entity__Read 0x450E90). An
//! EntityGroup keeps its members inline, each starting `{u32 id, u32 Entity}` (RE/15 §3); every member is decoded as
//! its own object with its own (world) placement. Components are found by their class markers, as in RE/14 (PORT:
//! the complete reflection-driven component graph is not decoded):
//! - `Visual` → `{u8 active, u32 drawable}`: a `Mesh` or a `LODSelector` (5 × LODDescriptor `{graphic, f32 distance,
//!   f32, u8}`); one `MeshInstanceData` per drawn mesh, each followed by its `CompiledMeshInstance` colours.
//! - `InertComponent` / `RigidBodyComponent` → `RigidBody` `{+8 CollisionFilterInfo (+12 layer & 0x3F),
//!   +20 shape id, +40 matrix}`; shapes MeshShape / BoxShape / BarrelShape / CapsuleShape / ListShape.
//! - `GuidanceSystem` (RE/06, RE/14 §2) in the entity's frame.
//! - `BhvHayStack` marks a haystack.

use std::{collections::HashMap, sync::{Arc, Mutex, Weak}};

use bevy::prelude::*;

use super::{archive::Archives, grid::Grid, material::{MaterialCache, MaterialData, TextureData}, shapes::parse_shape};
use crate::{assets::{forge::{crc32, Resource}, static_mesh::{parse_static_mesh, word, StaticMesh}, world::{parse_guidance, NativePlacement}},
    collision::Aabb3, guidance::GuidanceEdge, triangles::Triangle};

/// Game (Z up) → Bevy (Y up).
pub const TO_BEVY: Mat4 = Mat4::from_cols(Vec4::X, Vec4::new(0.0, 0.0, -1.0, 0.0), Vec4::Y, Vec4::W);
pub fn to_bevy(p: Vec3) -> Vec3 { Vec3::new(p.x, p.z, -p.y) }

/// PORT: entity names left out, after AC1-rs's list (RE/15 §3): mission/debug helpers and markers.
const EXCLUDED: [&str; 4] = ["GP_MARK", "PositionHelper", "PillarDust", "OutOfBound"];

pub struct LodMesh {
    pub id: u32,
    pub mesh: Arc<StaticMesh>,
    /// The placement's baked per-vertex colours (BGRA → RGBA), when they match the mesh.
    pub colors: Option<Arc<Vec<[u8; 4]>>>,
}

pub struct CityObject {
    pub name: String,
    /// Bevy-space placement (TO_BEVY · entity matrix).
    pub transform: Mat4,
    /// Bevy-space bounding sphere of the most detailed mesh (LOD distances are measured to its centre).
    pub center: Vec3,
    pub radius: f32,
    /// Bevy-space bounds of everything the object brings (visual, collision, guidance).
    pub bounds: Aabb3,
    pub meshes: Vec<LodMesh>,
    /// Which mesh draws at a LOD distance (`LodTable::pick`); None = draws nothing.
    pub lod: LodTable,
    /// Bevy-space bounds of the drawn meshes: the LOD distance is measured from the camera to this box (0xAC5CE0).
    pub visual_bounds: Aabb3,
    pub collision: Vec<Triangle>,
    pub edges: Vec<GuidanceEdge>,
    pub haystack: Option<Aabb3>,
    /// Posts among the object's ledge edges (`targets::pilotis_for_edges`), found against its own cell's geometry
    /// on the loader thread.
    pub pilotis: Vec<Vec3>,
}

#[derive(Default, Debug, Clone)]
pub struct CellStats {
    pub bodies: usize,
    pub objects: usize,
    pub triangles: usize,
    pub edges: usize,
    pub skipped: HashMap<String, usize>,
}

pub struct DecodedCell {
    pub cell: usize,
    pub objects: Vec<CityObject>,
    pub materials: Vec<(u32, Arc<MaterialData>)>,
    /// Textures the cell needs that were not on the GPU when it was decoded.
    pub textures: Vec<(u32, bool, Arc<TextureData>)>,
    pub stats: CellStats,
}

/// Decoded static meshes, shared while any loaded cell uses them.
#[derive(Default)]
pub struct MeshCache(Mutex<HashMap<u32, Weak<StaticMesh>>>);

impl MeshCache {
    pub fn get(&self, archives: &Archives, id: u32) -> Result<Arc<StaticMesh>, String> {
        if let Some(m) = self.0.lock().unwrap().get(&id).and_then(Weak::upgrade) { return Ok(m); }
        let r = archives.get(id)?;
        if r.class_hash != crc32("Mesh") { return Err(format!("{} is not a Mesh", r.name)); }
        let m = match parse_static_mesh(&r.payload) {
            Ok(m) => m,
            Err(_) => bind_pose_mesh(&r.payload).ok_or_else(|| format!("{}: unsupported mesh", r.name))?,
        };
        let m = Arc::new(m);
        self.0.lock().unwrap().insert(id, Arc::downgrade(&m));
        Ok(m)
    }
}

/// Meshes the plain static reader does not take, drawn in their bind / rest pose: kind 0 or 1 with bones
/// (haystacks, scaffolds, stalls, trees …) and kind 4 with inline SubMesh objects (cloth tarps, flags). Their
/// CompiledMesh blob has the static layout (RE/14 §2: `{u32 format 22, u32 stride, vb, ib, 0, 0, draw count, group
/// count, …}`, vertices, indices, 20-byte draw records, then the material list after the blob). Stride 24 is the
/// GEN_Standard vertex (position s16 · |w| · 3.81e-6, RE/14 §2); stride 32 the skinned vertex of RE/09 §3.1 (model space
/// at bind, s16 / 2048).
/// PORT: the props' skeletons, animations (swaying, breaking) and cloth simulation are not played.
fn bind_pose_mesh(d: &[u8]) -> Option<StaticMesh> {
    let cm = (20..d.len().saturating_sub(4)).find(|&o| word(d, o).ok() == Some(crate::assets::ac_formats::CLASS_COMPILED_MESH))?;
    let size = word(d, cm + 4).ok()? as usize;
    let blob = d.get(cm + 8..cm + 8 + size)?;
    let w = |p: usize| word(blob, p).ok();
    let stride = w(4)? as usize;
    if w(0)? != 22 || !(stride == 24 || stride == 32) { return None; }
    let (vb, ib, count) = (w(8)? as usize, w(12)? as usize, w(24)? as usize);
    if vb % stride != 0 || ib % 2 != 0 || count == 0 || count > 256 { return None; }
    let vertices = blob.get(36..36 + vb)?;
    let n = vb / stride;
    let tris: usize = (0..count).map(|k| w(36 + vb + ib + 20 * k + 16).map(|t| t as usize)).sum::<Option<usize>>()?;
    let bytes = blob.get(36 + vb..36 + vb + ib)?;
    let indices: Vec<u32> = if n > 0xFFFF && ib == tris * 12 { bytes.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]])).collect() }
        else { bytes.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]]) as u32).collect() };
    if indices.iter().any(|&i| i as usize >= n) { return None; }
    let short = |v: &[u8], p: usize| i16::from_le_bytes([v[p], v[p + 1]]) as f32;
    let dir = |v: &[u8], p: usize| (Vec3::new(v[p] as f32, v[p + 1] as f32, v[p + 2] as f32) / 127.5 - Vec3::ONE).normalize_or_zero();
    let mut out = StaticMesh { positions: Vec::with_capacity(n), normals: Vec::with_capacity(n), tangents: Vec::with_capacity(n), uvs: Vec::with_capacity(n),
        colors: Vec::with_capacity(n), sections: Vec::new() };
    for v in vertices.chunks_exact(stride) {
        let (nm, t) = (dir(v, 8), dir(v, 12));
        out.normals.push(nm);
        out.uvs.push(Vec2::new(short(v, 20), short(v, 22)) * crate::assets::ac_formats::UV_SCALE);
        if stride == 32 {
            out.positions.push(Vec3::new(short(v, 0), short(v, 2), short(v, 4)) * crate::assets::ac_formats::POS_SCALE);
            out.tangents.push(t.extend(if nm.cross(t).dot(dir(v, 16)) < 0.0 { -1.0 } else { 1.0 }));
            out.colors.push([1.0; 4]);
        } else {
            let scale = short(v, 6).abs() * 3.814_813_7e-6;
            out.positions.push(Vec3::new(short(v, 0), short(v, 2), short(v, 4)) * scale);
            out.tangents.push(t.extend(short(v, 6).signum()));
            out.colors.push([v[16] as f32 / 255.0, v[17] as f32 / 255.0, v[18] as f32 / 255.0, v[19] as f32 / 255.0]);
        }
    }
    let tail = cm + 8 + size;
    if word(d, tail + 8).ok()? as usize != count { return None; }
    for k in 0..count {
        let p = 36 + vb + ib + 20 * k;
        let (first, tris) = (w(p + 12)? as usize, w(p + 16)? as usize);
        out.sections.push(crate::assets::static_mesh::StaticSection { indices: indices.get(first..first + 3 * tris)?.to_vec(), material: word(d, tail + 12 + 4 * k).ok()? });
    }
    Some(out)
}

/// Everything the loader threads share.
pub struct CityData {
    pub archives: Arc<Archives>,
    pub grid: Grid,
    pub meshes: MeshCache,
    pub materials: MaterialCache,
    /// Upload BC blocks as they are (the adapter supports BC).
    pub bc: bool,
    /// Textures currently on the GPU (the main thread keeps this up to date).
    pub resident: Mutex<std::collections::HashSet<u32>>,
}

fn markers(b: &[u8], class: &str) -> Vec<usize> {
    let h = crc32(class).to_le_bytes();
    b.windows(4).enumerate().filter(|(_, w)| *w == h).map(|(p, _)| p).collect()
}

/// Split a resource payload into entity bodies (one for an Entity, the group's own plus each member for a group).
pub fn bodies(r: &Resource) -> Vec<&[u8]> {
    let mut starts = vec![0usize];
    if r.class_hash == crc32("EntityGroup") {
        let h = crc32("Entity").to_le_bytes();
        starts.extend((12..r.payload.len().saturating_sub(4)).filter(|&p| r.payload[p..p + 4] == h).map(|p| p - 4));
    }
    starts.iter().enumerate().map(|(k, &s)| &r.payload[s..starts.get(k + 1).copied().unwrap_or(r.payload.len())]).collect()
}

pub fn decode_cell(data: &CityData, cell: usize) -> Result<DecodedCell, String> {
    let c = &data.grid.cells[cell];
    let mut out = DecodedCell { cell, objects: Vec::new(), materials: Vec::new(), textures: Vec::new(), stats: CellStats::default() };
    let Some((archive, file)) = data.archives.location(c.datablock).map(|(a, f)| (a.to_string(), f.to_string())) else {
        return Err(format!("cell {cell}: datablock {:#x} not in the archives", c.datablock));
    };
    let a = data.archives.names.iter().position(|n| *n == archive).unwrap();
    let entry = data.archives.find(a, &file).ok_or("cell file vanished")?;
    let resources = data.archives.file(a, entry)?;
    // Keep the archive order (TOC order) so object order, and with it slot order, is deterministic.
    let mut list: Vec<&Arc<Resource>> = resources.values().filter(|r| r.class_hash == crc32("Entity") || r.class_hash == crc32("EntityGroup")).collect();
    list.sort_by_key(|r| r.id);
    let mut materials = HashMap::new();
    for r in list {
        for body in bodies(r) {
            out.stats.bodies += 1;
            let name = if body.as_ptr() == r.payload.as_ptr() { r.name.clone() } else { format!("{}/{:08x}", r.name, word(body, 0).unwrap_or(0)) };
            if EXCLUDED.iter().any(|x| name.contains(x)) { *out.stats.skipped.entry("excluded name".into()).or_default() += 1; continue; }
            match decode_body(data, body, &name, &mut materials, &mut out.stats.skipped) {
                Ok(Some(o)) => {
                    out.stats.triangles += o.collision.len();
                    out.stats.edges += o.edges.len();
                    out.objects.push(o);
                }
                Ok(None) => {}
                Err(e) => { *out.stats.skipped.entry(e).or_default() += 1; }
            }
        }
    }
    out.stats.objects = out.objects.len();
    // PORT: posts are looked for among the cell's own geometry (the game's candidates come from guidance queries at
    // jump time, RE/04 §4.1); a post whose neighbourhood spans two cells is judged on its own cell.
    if out.objects.iter().any(|o| o.edges.iter().any(|e| e.subtype == crate::guidance::GuidanceSubType::LedgeGrab)) {
        let mut collision = crate::collision::CollisionWorld::default();
        collision.enable_index(8.0);
        collision.add_triangles(out.objects.iter().flat_map(|o| o.collision.iter().cloned()).collect());
        let guidance = crate::guidance::GuidanceWorld { edges: out.objects.iter().flat_map(|o| o.edges.iter().cloned()).collect(), ..Default::default() };
        for o in &mut out.objects {
            if o.edges.iter().any(|e| e.subtype == crate::guidance::GuidanceSubType::LedgeGrab) {
                o.pilotis = crate::player::targets::pilotis_for_edges(o.edges.iter(), &guidance, &collision);
            }
        }
    }
    let resident = data.resident.lock().unwrap().clone();
    for (id, m) in materials {
        for (tex, linear) in m.diffuse.map(|d| (d, false)).into_iter().chain(m.normal.map(|n| (n, true))) {
            if resident.contains(&tex) || out.textures.iter().any(|t| t.0 == tex) { continue; }
            match data.materials.texture(&data.archives, tex, linear, data.bc) {
                Ok(t) => out.textures.push((tex, linear, t)),
                Err(e) => { *out.stats.skipped.entry(format!("texture: {e}")).or_default() += 1; }
            }
        }
        out.materials.push((id, m));
    }
    Ok(out)
}

/// One entity body → object (None when it has nothing static to show or collide with).
fn decode_body(data: &CityData, b: &[u8], name: &str, materials: &mut HashMap<u32, Arc<MaterialData>>, skipped: &mut HashMap<String, usize>) -> Result<Option<CityObject>, String> {
    let visuals = markers(b, "Visual");
    let rigid = markers(b, "RigidBody");
    let guidance = markers(b, "GuidanceSystem");
    if visuals.is_empty() && rigid.is_empty() && guidance.is_empty() { return Ok(None); }
    let placement = placement(b)?;
    let transform = TO_BEVY * placement;
    let mut o = CityObject { name: name.to_string(), transform, center: transform.w_axis.truncate(), radius: 0.0,
        bounds: Aabb3 { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) }, meshes: Vec::new(), lod: LodTable::default(),
        visual_bounds: Aabb3 { min: Vec3::ZERO, max: Vec3::ZERO },
        collision: Vec::new(), edges: Vec::new(), haystack: None, pilotis: Vec::new() };
    // --- visual
    if let Some(&v) = visuals.first() {
        if b.get(v + 4) == Some(&1) {
            let drawable = word(b, v + 5)?;
            if drawable != 0 {
                if let Err(e) = visual(data, b, v, drawable, &mut o, materials) {
                    // the object keeps its collision and guidance without the visual
                    o.meshes.clear(); o.lod = LodTable::default();
                    *skipped.entry(format!("visual: {e}")).or_default() += 1;
                }
            }
        }
    }
    // --- collision
    for &p in &rigid {
        if word(b, p + 8)? != crc32("CollisionFilterInfo") { continue; }
        let layer = (word(b, p + 12)? & 0x3f) as u8;
        let shape_id = word(b, p + 20)?;
        if shape_id == 0 { continue; }
        let shape = data.archives.get(shape_id).map_err(|e| format!("shape: {e}"))?;
        let (mesh, _) = parse_shape(&shape.payload).map_err(|e| format!("shape: {e}"))?;
        let mut m = [0.0; 16];
        for (i, x) in m.iter_mut().enumerate() { *x = f32::from_bits(word(b, p + 40 + 4 * i)?); }
        let rb = Mat4::from_cols_array(&m);
        if !rb.is_finite() || m[3] != 0.0 || m[7] != 0.0 || m[11] != 0.0 || m[15] != 1.0 { return Err("collision: invalid rigid-body matrix".into()); }
        let world = transform * rb;
        for t in mesh.indices.chunks_exact(3) {
            let v = [0, 1, 2].map(|k| world.transform_point3(mesh.vertices[t[k] as usize]));
            if let Some(t) = Triangle::new(v, layer) { o.collision.push(t); }
        }
    }
    // --- guidance
    for &g in &guidance {
        let Some(start) = g.checked_sub(4) else { continue };
        let Ok(parsed) = parse_guidance(&b[start..]) else { continue };
        let p = NativePlacement { id: parsed.id, name: name.to_string(), transform: placement, guidance: parsed };
        for (active, mut edge) in p.world_edges() {
            if !active { continue; }
            // movement probes expect n1 to be the lower face normal (0x66BB40)
            if edge.n1.y > edge.n0.y { std::mem::swap(&mut edge.n0, &mut edge.n1); }
            if p.guidance.check_world_orientation && !crate::native_map::filter_accepts(&p.guidance, edge.n0, edge.n1) { continue; }
            o.edges.push(edge);
        }
    }
    let grow = |a: &mut Aabb3, p: Vec3| { a.min = a.min.min(p); a.max = a.max.max(p); };
    for t in &o.collision { grow(&mut o.bounds, t.bounds.min); grow(&mut o.bounds, t.bounds.max); }
    for e in &o.edges { grow(&mut o.bounds, e.p0); grow(&mut o.bounds, e.p1); }
    if !markers(b, "BhvHayStack").is_empty() {
        // PORT: the haystack volume is the hay mesh's bounds; BhvHayStack's own fields are not decoded.
        let visual = o.meshes.first().map(|m| mesh_bounds(&m.mesh, o.transform));
        o.haystack = visual.or(Some(o.bounds)).filter(|b| b.min.is_finite());
    }
    if o.meshes.is_empty() && o.collision.is_empty() && o.edges.is_empty() { return Ok(None); }
    if !o.bounds.min.is_finite() { o.bounds = Aabb3 { min: o.center, max: o.center }; }
    Ok(Some(o))
}

/// An Entity or EntityGroup body's placement (Entity__Read 0x450E90 → 0x4D9C10): 16 floats after the id/class.
/// Some authored matrices carry w = 0.99999994; accept affine within 1e-4.
pub fn placement(b: &[u8]) -> Result<Mat4, String> {
    let class = word(b, 4)?;
    if class != crc32("Entity") && class != crc32("EntityGroup") { return Err("placement: not an entity".into()); }
    let mut m = [0.0; 16];
    for (i, v) in m.iter_mut().enumerate() { *v = f32::from_bits(word(b, 8 + 4 * i)?); }
    if m.iter().any(|v| !v.is_finite()) || m[3].abs() > 1e-4 || m[7].abs() > 1e-4 || m[11].abs() > 1e-4 || (m[15] - 1.0).abs() > 1e-4 {
        return Err("placement: not affine".into());
    }
    m[3] = 0.0; m[7] = 0.0; m[11] = 0.0; m[15] = 1.0;
    let m = Mat4::from_cols_array(&m);
    if m.determinant().abs() < 1e-8 { return Err("placement: singular".into()); }
    Ok(m)
}

pub fn mesh_bounds(mesh: &StaticMesh, transform: Mat4) -> Aabb3 {
    let (lo, hi) = mesh.positions.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(l, h), &p| (l.min(p), h.max(p)));
    let mut b = Aabb3 { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) };
    for i in 0..8 {
        let c = Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        let p = transform.transform_point3(c);
        b.min = b.min.min(p); b.max = b.max.max(p);
    }
    b
}

/// LODSelector payload (`LODSelector__Read` 0xA90FE0): 5 × LODDescriptor `{id, class, graphic, f32 distance, f32 fade
/// width, u8}` (0xA8F720) from payload 12, then two bytes: selector flag bit 0 (keep empty slots) and bit 1 (no fade).
pub fn lod_descriptors(selector: &[u8]) -> Result<([(u32, f32); 5], u8), String> {
    let mut out = [(0, 0.0); 5];
    for (k, d) in out.iter_mut().enumerate() {
        let p = 12 + 21 * k;
        if word(selector, p + 4)? != crc32("LODDescriptor") { return Err("LODSelector without LODDescriptors".into()); }
        *d = (word(selector, p + 8)?, f32::from_bits(word(selector, p + 12)?));
    }
    let flags = selector.get(12 + 105).copied().unwrap_or(0) & 1 | (selector.get(12 + 106).copied().unwrap_or(0) & 1) << 1;
    Ok((out, flags))
}

/// The game's per-selector LOD table (verified in IDA 2026-10-09, RE/17 §3.2): `LODSelector__BuildTable` 0xA90C30
/// builds it after the read, `LODTable__Pick` 0xA8F510 looks it up when the instance is submitted
/// (`LODSelectorInstance__Submit` 0xA90160).
#[derive(Clone)]
pub struct LodTable {
    /// 255 / (last kept distance · 256 / 255).
    pub scale: f32,
    /// Mesh slot (descriptor index) per quantised distance; None = an empty kept slot (draws nothing).
    pub entries: Option<Box<[Option<u8>; 256]>>,
}

impl Default for LodTable {
    /// A drawable without a selector: slot 0 at every distance.
    fn default() -> Self { Self { scale: 0.0, entries: None } }
}

impl LodTable {
    /// `LODSelector__CollectLODs` 0xA90A10 + `LODSelector__BuildTable` 0xA90C30. Only the first four descriptors count (four LOD instances, 0xA90090). A slot with a
    /// mesh is kept; a slot without one is kept too when flag bit 0 is set (it then draws nothing), otherwise, when a
    /// mesh follows it, it pushes the previous kept slot's distance out to its own. The table then walks the distance in
    /// 255 steps up to the last kept distance · 256/255, advancing at most one kept slot per step; past the end the last
    /// kept slot stays (no LOD cut-off).
    pub fn build(desc: &[(u32, f32); 5], flags: u8) -> Self {
        let keep_empty = flags & 1 != 0;
        let last_mesh = (0..4).rev().find(|&i| desc[i].0 != 0).unwrap_or(0);
        let mut kept: Vec<(u8, f32)> = Vec::new();
        for i in 0..4 {
            if desc[i].0 != 0 || keep_empty { kept.push((i as u8, desc[i].1)); }
            else if i < last_mesh { if let Some(k) = kept.last_mut() { k.1 = desc[i].1; } }
        }
        let Some(&(_, last)) = kept.last() else { return Self { scale: 0.0, entries: Some(Box::new([None; 256])) } };
        let max = last * 256.0 / 255.0;
        let step = max / 255.0;
        let mut entries = Box::new([None; 256]);
        let (mut d, mut k) = (0.0f32, 0usize);
        for e in entries.iter_mut() {
            if d > kept[k].1 { k = (k + 1).min(kept.len() - 1); }
            d += step;
            let slot = kept[k].0;
            *e = (desc[slot as usize].0 != 0).then_some(slot);
        }
        Self { scale: if max > 0.0 { 255.0 / max } else { f32::INFINITY }, entries: Some(entries) }
    }

    /// The descriptor slot drawn at LOD distance `d` (`LODTable__Pick` 0xA8F510: index = trunc(scale · d) clamped to 255).
    pub fn pick(&self, d: f32) -> Option<u8> {
        let Some(e) = &self.entries else { return Some(0) };
        let i = (self.scale * d.max(0.0)).min(255.0) as usize;
        e[i.min(255)]
    }
}

fn visual(data: &CityData, b: &[u8], v: usize, drawable: u32, o: &mut CityObject, materials: &mut HashMap<u32, Arc<MaterialData>>) -> Result<(), String> {
    let r = data.archives.get(drawable)?;
    // slot → mesh id
    let (slots, table): (Vec<u32>, LodTable) = if r.class_hash == crc32("LODSelector") {
        let (desc, flags) = lod_descriptors(&r.payload)?;
        (desc.iter().take(4).map(|d| d.0).collect(), LodTable::build(&desc, flags))
    } else if r.class_hash == crc32("Mesh") {
        (vec![drawable], LodTable::default())
    } else if r.class_hash == crc32("DynamicMesh") {
        // A cloth (the rooftop gardens' tarps): the DynamicMesh resource is only a header; the entity's SoftBody
        // component references its source Mesh (reflection: SoftBody +372 Mesh). PORT: drawn in its rest shape, the
        // soft-body simulation is not ported.
        let soft = markers(b, "SoftBody").into_iter().find(|&p| p > v).ok_or("cloth without a SoftBody")?;
        let mesh = (soft + 4..b.len().saturating_sub(4)).filter_map(|p| word(b, p).ok())
            .find(|&id| id != 0 && data.archives.contains(id) && data.archives.get(id).is_ok_and(|m| m.class_hash == crc32("Mesh")))
            .ok_or("cloth SoftBody without a Mesh")?;
        (vec![mesh], LodTable::default())
    } else {
        return Err("unsupported drawable".into());
    };
    // MeshInstanceData (mesh id at +5) → the following CompiledMeshInstance's colour blob.
    let instances = markers(&b[v..], "MeshInstanceData").into_iter().map(|p| p + v).collect::<Vec<_>>();
    let compiled = markers(&b[v..], "CompiledMeshInstance").into_iter().map(|p| p + v).collect::<Vec<_>>();
    let mut colors: HashMap<u32, &[u8]> = HashMap::new();
    for (k, &p) in instances.iter().enumerate() {
        let next = instances.get(k + 1).copied().unwrap_or(b.len());
        let Some(&c) = compiled.iter().find(|&&c| c > p && c < next) else { continue };
        let bytes = word(b, c + 4)? as usize;
        if let Some(blob) = b.get(c + 8..c + 8 + bytes) { colors.insert(word(b, p + 5)?, blob); }
    }
    let mut index_of_slot = HashMap::new();
    for (slot, &id) in slots.iter().enumerate() {
        if id == 0 { continue; }
        let mesh = data.meshes.get(&data.archives, id)?;
        let col = colors.get(&id).and_then(|blob| {
            let n = mesh.positions.len();
            (blob.len() == 16 + n * 4 && word(blob, 8).ok()? as usize == n * 4)
                .then(|| Arc::new(blob[16..].chunks_exact(4).map(|c| [c[2], c[1], c[0], c[3]]).collect::<Vec<_>>()))
        });
        for s in &mesh.sections {
            if !materials.contains_key(&s.material) {
                let m = data.materials.material(&data.archives, s.material)?;
                materials.insert(s.material, m);
            }
        }
        index_of_slot.insert(slot, o.meshes.len());
        o.meshes.push(LodMesh { id, mesh, colors: col });
    }
    // descriptor slots → this object's mesh indices
    o.lod = LodTable { scale: table.scale, entries: table.entries.map(|e| Box::new(e.map(|s| s.and_then(|s| index_of_slot.get(&(s as usize)).map(|&m| m as u8))))) };
    if let Some(first) = o.meshes.first() {
        let bounds = mesh_bounds(&first.mesh, o.transform);
        o.center = (bounds.min + bounds.max) * 0.5;
        o.radius = (bounds.max - bounds.min).length() * 0.5;
        o.visual_bounds = bounds;
        for m in &o.meshes {
            let bb = mesh_bounds(&m.mesh, o.transform);
            o.bounds.min = o.bounds.min.min(bb.min); o.bounds.max = o.bounds.max.max(bb.max);
            o.visual_bounds.min = o.visual_bounds.min.min(bb.min); o.visual_bounds.max = o.visual_bounds.max.max(bb.max);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_table_follows_the_exe() {
        // MM-M- 20/40/80/160/320: the empty slot 2 pushes slot 1 out to 80 m; slot 3 from there on, with no cut-off
        let t = LodTable::build(&[(1, 20.0), (2, 40.0), (0, 80.0), (4, 160.0), (0, 320.0)], 0);
        assert_eq!([t.pick(0.0), t.pick(19.0), t.pick(30.0), t.pick(70.0), t.pick(100.0), t.pick(400.0), t.pick(1e5)],
            [Some(0), Some(0), Some(1), Some(1), Some(3), Some(3), Some(3)]);
        // M--M- 100/101/102/350/351: slot 0 to 102 m, then slot 3
        let t = LodTable::build(&[(1, 100.0), (0, 101.0), (0, 102.0), (4, 350.0), (0, 351.0)], 0);
        assert_eq!([t.pick(90.0), t.pick(101.5), t.pick(110.0), t.pick(500.0)], [Some(0), Some(0), Some(3), Some(3)]);
        // -M---: the leading empty slot is dropped, slot 1 at every distance
        let t = LodTable::build(&[(0, 9999.0), (1, 10000.0), (0, 10001.0), (0, 10002.0), (0, 10003.0)], 0);
        assert_eq!([t.pick(0.0), t.pick(5e4)], [Some(1), Some(1)]);
        // flag bit 0 keeps empty slots: M---- 50/51/80/160 draws nothing past 50 m
        let t = LodTable::build(&[(1, 50.0), (0, 51.0), (0, 80.0), (0, 160.0), (0, 320.0)], 1);
        assert_eq!([t.pick(40.0), t.pick(60.0), t.pick(1e4)], [Some(0), None, None]);
        // the fifth descriptor is never read
        let t = LodTable::build(&[(1, 10.0), (0, 20.0), (0, 40.0), (0, 80.0), (5, 160.0)], 0);
        assert_eq!(t.pick(1e4), Some(0));
    }
}
