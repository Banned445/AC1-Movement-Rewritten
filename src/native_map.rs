//! Bounded Masyaf rooftop import from the user's install. RE/14; no bundled game data.
use std::{collections::HashMap, path::Path};
use bevy::{asset::RenderAssetUsages, image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor}, mesh::{Indices, PrimitiveTopology}, prelude::*, render::render_resource::{Extent3d, TextureDimension, TextureFormat}};
use crate::{assets::{ac_formats::{parse_texture, AcTexture}, forge::{crc32, Forge, Resource}, static_mesh::{parse_static_mesh, word, StaticMesh}, world::{inspect_cell, NativePlacement}}, collision::CollisionWorld, guidance::GuidanceWorld, triangles::Triangle};

/// One opt-in switch fences scene selection and the source-triangle collision path.
pub const NATIVE_MAP_IMPORT: &str = "AC_NATIVE_MAP";
pub fn enabled() -> bool { std::env::var(NATIVE_MAP_IMPORT).is_ok_and(|v| v == "masyaf-roofs") }

struct Resources {
    archives: Vec<Forge>,
    index: HashMap<u32, (usize, usize)>,
    cache: HashMap<u32, Resource>,
}

impl Resources {
    fn open(game: &Path) -> Result<Self, String> {
        let mut archives = Vec::new(); let mut index = HashMap::new();
        for archive in ["DataPC_Masyaf.forge", "DataPC_Common.forge", "DataPC.forge"] {
            let mut f = Forge::open(&game.join(archive)).map_err(|e| e.to_string())?;
            for entry in f.entries.clone() {
                for id in f.resource_ids(&entry).map_err(|e| e.to_string())? { index.entry(id).or_insert((archives.len(), entry.index)); }
            }
            archives.push(f);
        }
        Ok(Self { archives, index, cache: HashMap::new() })
    }
    fn get(&mut self, id: u32) -> Result<Resource, String> {
        if let Some(r) = self.cache.get(&id) { return Ok(r.clone()); }
        let &(a, e) = self.index.get(&id).ok_or_else(|| format!("missing map resource {id:#x}"))?;
        let entry = self.archives[a].entries[e].clone();
        for resource in self.archives[a].resources(&entry).map_err(|e| e.to_string())? {
            if self.index.get(&resource.id) == Some(&(a, e)) { self.cache.entry(resource.id).or_insert(resource); }
        }
        self.cache.get(&id).cloned().ok_or("indexed map resource absent from file".into())
    }
    fn texture(&mut self, set: u32, channel: &str) -> Result<Option<(u32, AcTexture)>, String> {
        let set = self.get(set)?;
        if set.class_hash != crc32("TextureSet") { return Err("map material has no TextureSet".into()); }
        for p in (8..set.payload.len().saturating_sub(3)).step_by(4) {
            let id = word(&set.payload, p)?;
            if id == 0 || !self.index.contains_key(&id) { continue; }
            let spec = self.get(id)?;
            if spec.class_hash != crc32("TextureMapSpec") || !spec.name.contains(channel) { continue; }
            // TextureMapSpec__Read 0xA15F70 ends with a handle and the typed TextureMap reference.
            let texture_id = word(&spec.payload, spec.payload.len().checked_sub(4).ok_or("truncated texture spec")?)?;
            let texture = self.get(texture_id).map_err(|e| format!("texture spec {}: {e}", spec.name))?;
            let parsed = parse_texture(&texture.payload).ok_or_else(|| format!("unsupported map texture {}", texture.name))?;
            return Ok(Some((texture_id, parsed)));
        }
        Ok(None)
    }
}

fn marker(data: &[u8], class: &str) -> Result<usize, String> {
    let hash = crc32(class).to_le_bytes();
    let candidates: Vec<_> = data.windows(4).enumerate().filter_map(|(p, v)| (v == hash).then_some(p)).collect();
    if candidates.len() != 1 { return Err(format!("ambiguous or absent {class}")); }
    Ok(candidates[0])
}

struct Part {
    placement: NativePlacement,
    mesh: StaticMesh,
}
struct MaterialData {
    name: String,
    template: String,
    diffuse: (u32, AcTexture),
    normal: Option<(u32, AcTexture)>,
}
pub(crate) struct ImportedMap {
    parts: Vec<Part>,
    materials: HashMap<u32, MaterialData>,
    collision: CollisionWorld,
    guidance: GuidanceWorld,
    spawn: Vec3,
}

pub(crate) fn load(game: &Path) -> Result<ImportedMap, String> {
    let placements = inspect_cell(game, "DataPC_Masyaf.forge", "Cell01952_DataBlock")?;
    let mut resources = Resources::open(game)?;
    let mut map = ImportedMap { parts: Vec::new(), materials: HashMap::new(), collision: CollisionWorld::default(), guidance: GuidanceWorld::default(), spawn: Vec3::ZERO };
    // Source placements kept intact; three adjoining houses and the ladder between their roofs.
    let wanted = ["House_6x6x8_01a_001", "House_6x6x4_01a_008", "House_3x3x8_01a_017", "Ladder_4m_030"];
    for name in wanted {
        let p = placements.iter().find(|p| p.name == name).cloned().ok_or_else(|| format!("missing map placement {name}"))?;
        append_part(&mut map, &mut resources, p)?;
    }
    // PORT: a rooftop spawn inside the imported crop; the game's mission spawn is outside this slice.
    let roof = map.parts.iter().find(|p| p.placement.name == wanted[0]).unwrap().placement.transform.w_axis.truncate();
    let origin = Vec3::new(roof.x, roof.z + 10.0, -roof.y);
    let y = map.collision.floor_height_below(origin, 12.0).ok_or("native rooftop spawn has no collision support")?;
    map.spawn = Vec3::new(origin.x, y, origin.z);
    if !map.collision.capsule_fits(map.spawn) { return Err("native rooftop spawn is blocked".into()); }
    map.guidance.jump_pilotis = Some(crate::player::targets::find_jump_pilotis(&map.guidance,&map.collision));
    Ok(map)
}

pub(crate) fn load_village(game: &Path) -> Result<ImportedMap,String> {
    let mut resources = Resources::open(game)?;
    let mut map = ImportedMap { parts: Vec::new(), materials: HashMap::new(), collision: CollisionWorld::default(), guidance: GuidanceWorld::default(), spawn: Vec3::ZERO };
    // Archive observations in RE/14: four adjoining 32 m placement cells and shared village ground.
    let cells = ["Cell05460_DataBlock","Cell01951_DataBlock","Cell01952_DataBlock","Cell02015_DataBlock","Cell02016_DataBlock"];
    let mut skipped = Vec::new();
    for cell in cells {
        let entry = resources.archives[0].find(cell).cloned().ok_or_else(|| format!("missing village cell {cell}"))?;
        let entities = resources.archives[0].resources(&entry).map_err(|e|e.to_string())?;
        for entity in entities.into_iter().filter(|r|r.class_hash == crc32("Entity")) {
            if cell == cells[0] && !entity.name.starts_with("GroundVillage") { continue; }
            if !entity.payload.windows(4).any(|v|v == crc32("Visual").to_le_bytes()) { continue; }
            // PORT: animated vegetation/particles/actors require separate systems; this crop imports static space.
            if ["VEG_","Veg_","Countryside_","Plants_","BirdShit_"].iter().any(|prefix|entity.name.starts_with(prefix)) { continue; }
            let guidance = if entity.payload.windows(4).any(|v|v==crc32("GuidanceSystem").to_le_bytes()) {
                let g = marker(&entity.payload,"GuidanceSystem")?;
                crate::assets::world::parse_guidance(&entity.payload[g-4..])?
            } else { default() };
            let p = NativePlacement { id:entity.id, name:entity.name.clone(), transform:crate::assets::world::entity_transform(&entity.payload)?, guidance };
            let collision_count = map.collision.triangles.len();
            let guidance_count = map.guidance.edges.len();
            if let Err(error) = append_part(&mut map,&mut resources,p) {
                map.collision.triangles.truncate(collision_count);
                map.guidance.edges.truncate(guidance_count);
                if entity.name.starts_with("GroundVillage") || entity.name.starts_with("House_") || entity.name.starts_with("Ladder_") {
                    return Err(format!("required village geometry {}: {error}",entity.name));
                }
                skipped.push(format!("{}: {error}",entity.name));
            }
        }
    }
    if !skipped.is_empty() { warn!("village: {} unsupported decorative placements: {:?}",skipped.len(),skipped); }
    // PORT: a supported street spawn near the sample houses, instead of a mission-specific spawn.
    let origin = Vec3::new(19.0,36.0,36.0);
    let y = map.collision.floor_height_below(origin,4.0).ok_or("village street has no native ground support")?;
    map.spawn = Vec3::new(origin.x,y,origin.z);
    if !map.collision.capsule_fits(map.spawn) { return Err("village street spawn is blocked".into()); }
    map.guidance.jump_pilotis = Some(crate::player::targets::find_jump_pilotis(&map.guidance,&map.collision));
    info!("Masyaf village: {} source placements, {} collision triangles, {} authored edges",map.parts.len(),map.collision.triangles.len(),map.guidance.edges.len());
    Ok(map)
}

fn rigid_transform(data: &[u8], p: usize) -> Result<Mat4, String> {
    let mut values = [0.0;16];
    for (i,v) in values.iter_mut().enumerate() { *v = f32::from_bits(word(data,p+i*4)?); }
    let matrix = Mat4::from_cols_array(&values);
    if !matrix.is_finite() || values[3] != 0.0 || values[7] != 0.0 || values[11] != 0.0 || values[15] != 1.0 { return Err("invalid rigid-body transform".into()); }
    Ok(matrix)
}

fn append_part(map: &mut ImportedMap, resources: &mut Resources, p: NativePlacement) -> Result<(),String> {
    let name = p.name.clone();
    let entity = resources.get(p.id)?;
    let visual = marker(&entity.payload, "Visual")?;
    if entity.payload.get(visual + 4) != Some(&1) { return Err("inactive native visual".into()); }
    let mut drawable = resources.get(word(&entity.payload, visual + 5)?)?;
    if drawable.class_hash == crc32("LODSelector") { drawable = resources.get(word(&drawable.payload, 20)?)?; }
    if drawable.class_hash != crc32("Mesh") { return Err(format!("unsupported drawable in {name}")); }
    let mut mesh = parse_static_mesh(&drawable.payload).map_err(|e| format!("{name}: {e}"))?;
    // The first MeshInstanceData is the source full-detail LOD (0xA8F4C0 / 0xA881B0).
    let instance = entity.payload.windows(4).position(|v| v == crc32("MeshInstanceData").to_le_bytes()).ok_or("missing mesh instance")?;
    if word(&entity.payload, instance + 5)? != drawable.id { return Err("mesh instance does not match source LOD".into()); }
    let compiled = entity.payload[instance..].windows(4).position(|v| v == crc32("CompiledMeshInstance").to_le_bytes()).ok_or("missing compiled mesh instance")? + instance;
    let bytes = word(&entity.payload, compiled + 4)? as usize;
    let data = entity.payload.get(compiled + 8..compiled + 8 + bytes).ok_or("truncated instance colors")?;
    if data.len() != 16 + mesh.positions.len() * 4 || word(data, 8)? as usize != mesh.positions.len() * 4 { return Err("instance colors do not match source vertices".into()); }
    mesh.colors = data[16..].chunks_exact(4).map(|c| [c[2] as f32 / 255.0, c[1] as f32 / 255.0, c[0] as f32 / 255.0, c[3] as f32 / 255.0]).collect();

    if let Ok(inert) = marker(&entity.payload, "InertComponent") {
        // Scoped fixed RigidBody stream from 0x651640, followed by generation type, bool, guidance handle.
        let guidance_handle = word(&entity.payload, inert + 126)?;
        // PORT: some entities have a null inert guidance handle; their unique Entity-local GuidanceSystem
        // is retained pending decoding the complete component graph. Reject other mismatched handles.
        let unlinked_guidance = guidance_handle == 0;
        if word(&entity.payload, inert + 17)? != crc32("RigidBody") || word(&entity.payload, inert + 25)? != crc32("CollisionFilterInfo")
            || (guidance_handle != p.guidance.id && !unlinked_guidance) { return Err(format!("unsupported native component ownership in {name}")); }
        let shape = resources.get(word(&entity.payload, inert + 37)?).map_err(|e| format!("{name} shape: {e}"))?;
        let shape = crate::assets::static_mesh::parse_static_shape(&shape.payload).map_err(|e| format!("{name}: {e}"))?;
        let layer = (word(&entity.payload, inert + 29)? & 0x3f) as u8;
        let rigid = rigid_transform(&entity.payload, inert + 57)?;
        let convert = |v: Vec3| { let v = (p.transform * rigid).transform_point3(v); Vec3::new(v.x, v.z, -v.y) };
        for indices in shape.indices.chunks_exact(3) {
            if let Some(t) = Triangle::new([convert(shape.vertices[indices[0] as usize]), convert(shape.vertices[indices[1] as usize]), convert(shape.vertices[indices[2] as usize])], layer) { map.collision.triangles.push(t); }
        }
    }
    for (active, mut edge) in p.world_edges() {
        if !active { continue; }
        // Existing movement probes expect n1 to be the lower face normal (0x66BB40).
        if edge.n1.y > edge.n0.y { std::mem::swap(&mut edge.n0, &mut edge.n1); }
        // Static validity checks can be evaluated once in owner world orientation (0x6B0E10).
        if p.guidance.check_world_orientation && !filter_accepts(&p.guidance, edge.n0, edge.n1) { continue; }
        map.guidance.edges.push(edge);
    }
    for section in &mesh.sections {
        if map.materials.contains_key(&section.material) { continue; }
        let material = resources.get(section.material).map_err(|e| format!("{name} material: {e}"))?;
        if material.class_hash != crc32("Material") { return Err("invalid map material reference".into()); }
        let template = resources.get(word(&material.payload, 8)?).map_err(|e| format!("{} template: {e}", material.name))?.name;
        let set = word(&material.payload, 12)?;
        let diffuse = resources.texture(set, "Diffuse")?.ok_or_else(|| format!("{} has no decoded diffuse map", material.name))?;
        let normal = resources.texture(set, "Normal")?;
        map.materials.insert(section.material, MaterialData { name: material.name, template, diffuse, normal });
    }
    // Keep the source pose and records available alongside the rendered mesh.
    map.parts.push(Part { placement: p, mesh });

    Ok(())
}

#[cfg(test)]
pub(crate) fn simulation_fixture() -> Option<(CollisionWorld, GuidanceWorld, Vec3)> {
    let game = crate::assets::find_game_dir()?;
    if !game.join("DataPC_Masyaf.forge").is_file() { return None; }
    let map = load(&game).expect("native map fixture must decode when game data is present");
    Some((map.collision, map.guidance, map.spawn))
}

#[cfg(test)]
pub(crate) fn village_simulation_fixture() -> Option<(CollisionWorld, GuidanceWorld, Vec3)> {
    let game = crate::assets::find_game_dir()?;
    if !game.join("DataPC_Masyaf.forge").is_file() { return None; }
    let map = load_village(&game).expect("native village must decode when game data is present");
    Some((map.collision,map.guidance,map.spawn))
}

fn filter_accepts(g: &crate::assets::world::NativeGuidance, a: Vec3, b: Vec3) -> bool {
    let cos = g.filter_cos_angle;
    let wall = |n: Vec3| n.y.abs() < cos;
    let floor_wall = (a.y > cos && wall(b)) || (b.y > cos && wall(a));
    let ceiling_wall = (a.y < -cos && wall(b)) || (b.y < -cos && wall(a));
    let slab = (a.y > cos && b.y < -cos) || (b.y > cos && a.y < -cos);
    // PORT: wall/wall dihedral uses the unsigned normal angle; native signed edge-axis test remains open.
    let corner = wall(a) && wall(b) && a.normalize_or_zero().dot(b.normalize_or_zero()).clamp(-1.0, 1.0).acos() > g.filter_corner_angle;
    (g.filter_flags & 1 != 0 && corner) || (g.filter_flags & 2 != 0 && floor_wall) || (g.filter_flags & 4 != 0 && ceiling_wall) || (g.filter_flags & 16 != 0 && slab)
}

fn image(t: &AcTexture, linear: bool) -> Image {
    let mut img = Image::new_uninit(Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 }, TextureDimension::D2,
        if linear { TextureFormat::Rgba8Unorm } else { TextureFormat::Rgba8UnormSrgb }, RenderAssetUsages::default());
    img.texture_descriptor.mip_level_count = t.mips.len() as u32;
    img.data = Some(t.mips.concat());
    // PORT: repeat sampling until TextureMapSpec's sampler fields are decoded for every material.
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { address_mode_u: ImageAddressMode::Repeat, address_mode_v: ImageAddressMode::Repeat, ..ImageSamplerDescriptor::linear() });
    img
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn(In(map): In<ImportedMap>, mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>, mut images: ResMut<Assets<Image>>,
    mut collision: ResMut<CollisionWorld>, mut guidance: ResMut<GuidanceWorld>, mut spawn_point: ResMut<crate::player::SpawnPoint>,
    mut camera: ResMut<crate::camera::CameraRig>, mut players: Query<(&mut crate::player::Body, &mut Transform), With<crate::player::Player>>) {
    let mut handles = HashMap::new();
    let village = map.parts.iter().any(|p|p.placement.name=="GroundVillage");
    for (id, material) in map.materials {
        // PORT: Bevy PBR replaces the native shader. MultiBlender material layers/specular settings
        // remain unported; report them explicitly. Geometry, source diffuse/normal maps and AO are read.
        if material.template != "GEN_Standard" { warn!("native map material {} uses {}; material layer blending is not yet ported", material.name, material.template); }
        let diffuse = images.add(image(&material.diffuse.1, false));
        let normal = material.normal.as_ref().map(|(_, t)| images.add(image(t, true)));
        handles.insert(id, materials.add(StandardMaterial { base_color_texture: Some(diffuse), normal_map_texture: normal, flip_normal_map_y: true,
            perceptual_roughness: 0.9, double_sided: true, cull_mode: None, alpha_mode: AlphaMode::Mask(0.5), ..default() }));
    }
    let conversion = Mat4::from_cols(Vec4::X, Vec4::new(0.0, 0.0, -1.0, 0.0), Vec4::Y, Vec4::W);
    for part in map.parts {
        let transform = Transform::from_matrix(conversion * part.placement.transform);
        for section in part.mesh.sections {
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, part.mesh.positions.iter().map(|v| v.to_array()).collect::<Vec<_>>());
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, part.mesh.normals.iter().map(|v| v.to_array()).collect::<Vec<_>>());
            mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, part.mesh.tangents.iter().map(|v| v.to_array()).collect::<Vec<_>>());
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, part.mesh.uvs.iter().map(|v| v.to_array()).collect::<Vec<_>>());
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, part.mesh.colors.clone());
            mesh.insert_indices(Indices::U32(section.indices));
            commands.spawn((crate::map_menu::MapEntity, Mesh3d(meshes.add(mesh)), MeshMaterial3d(handles[&section.material].clone()), transform, Name::new(part.placement.name.clone())));
        }
    }
    info!("native Masyaf scene: {} collision triangles, {} authored guidance edges", map.collision.triangles.len(), map.guidance.edges.len());
    *collision = map.collision; *guidance = map.guidance;
    spawn_point.0 = map.spawn;
    // PORT: frame the selected crop with the existing orbit camera, pending NavigationCamera RE.
    if village { *camera = default(); }
    else { camera.yaw = -0.4; camera.pitch = -0.55; camera.distance = 12.0; }
    if let Ok((mut body, mut transform)) = players.single_mut() { body.feet = map.spawn; body.grounded = true; body.velocity = Vec3::ZERO; body.proxy.manifold.clear(); transform.translation = map.spawn; }
    // PORT: the test-scene sun remains until WorldAmbiance / native light data is imported.
    commands.spawn((crate::map_menu::MapEntity, DirectionalLight { illuminance: 10000.0, shadow_maps_enabled: true, ..default() }, Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.4, 0.0))));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_village_loads_ground_and_connected_buildings() {
        let Some(game) = crate::assets::find_game_dir() else { return; };
        if !game.join("DataPC_Masyaf.forge").is_file() { return; }
        let map = load_village(&game).unwrap();
        assert!(map.parts.len()>30);
        assert!(map.parts.iter().any(|p|p.placement.name=="GroundVillage"));
        assert!(map.collision.support(map.spawn).is_some());
        assert!(map.guidance.edges.len()>500);
    }
    #[test]
    fn native_roof_slice_loads_source_geometry_holds_and_materials() {
        let Some(game) = crate::assets::find_game_dir() else { return; };
        if !game.join("DataPC_Masyaf.forge").is_file() { return; }
        let map = load(&game).unwrap();
        assert_eq!(map.parts.len(), 4);
        assert!(map.collision.boxes.is_empty());
        assert!(map.collision.triangles.len() > 2000);
        assert!(map.guidance.edges.iter().any(|e| e.subtype == crate::guidance::GuidanceSubType::Ladder));
        assert!(map.guidance.edges.len() >= 93);
        assert!(map.collision.support(map.spawn).is_some());
        assert!(map.materials.values().all(|m| !m.diffuse.1.mips.is_empty()));
    }
}
