//! Static world Mesh / MeshShape streams. RE/14, readers 0x9F6EE0 / 0xA96610 / 0x647480.
use bevy::prelude::*;
use super::forge::crc32;

pub struct StaticSection {
    pub indices: Vec<u32>,
    pub material: u32,
}

pub struct StaticMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub tangents: Vec<Vec4>,
    pub uvs: Vec<Vec2>,
    pub colors: Vec<[f32; 4]>,
    pub sections: Vec<StaticSection>,
}

pub(crate) fn word(d: &[u8], p: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(d.get(p..p + 4).ok_or("truncated static geometry")?.try_into().unwrap()))
}

fn float(d: &[u8], p: usize) -> Result<f32, String> {
    let v = f32::from_bits(word(d, p)?);
    if v.is_finite() { Ok(v) } else { Err("non-finite static geometry".into()) }
}

/// Static, unskinned Mesh category 0, no inline SubMesh objects / bones.
pub fn parse_static_mesh(d: &[u8]) -> Result<StaticMesh, String> {
    if word(d, 4)? != crc32("Mesh") || word(d, 8)? != 0 || word(d, 12)? != 0 || word(d, 16)? != 0 {
        return Err("unsupported static mesh category".into());
    }
    if d.get(20) != Some(&0) || word(d, 26)? != crc32("CompiledMesh") {
        return Err("missing static CompiledMesh".into());
    }
    let size = word(d, 30)? as usize;
    let blob = d.get(34..34usize.checked_add(size).ok_or("compiled size overflow")?).ok_or("truncated CompiledMesh")?;
    if word(blob, 0)? != 22 || word(blob, 4)? != 24 { return Err("unsupported static vertex format".into()); }
    let vb = word(blob, 8)? as usize;
    let ib = word(blob, 12)? as usize;
    let count = word(blob, 24)? as usize;
    let groups = word(blob, 28)? as usize;
    if vb % 24 != 0 || ib % 6 != 0 || count > 256 || groups > count { return Err("invalid static mesh counts".into()); }
    let vertices = blob.get(36..36 + vb).ok_or("truncated static vertices")?;
    let index_bytes = blob.get(36 + vb..36 + vb + ib).ok_or("truncated static indices")?;
    let indices: Vec<u32> = index_bytes.chunks_exact(2).map(|p| u16::from_le_bytes(p.try_into().unwrap()) as u32).collect();
    if indices.iter().any(|&i| i as usize >= vertices.len() / 24) { return Err("static index outside vertex buffer".into()); }
    let mut mesh = StaticMesh { positions: Vec::new(), normals: Vec::new(), tangents: Vec::new(), uvs: Vec::new(), colors: Vec::new(), sections: Vec::new() };
    for v in vertices.chunks_exact(24) {
        let short = |p| i16::from_le_bytes(v[p..p + 2].try_into().unwrap()) as f32;
        // GEN_Standard packed-position vertex shader (MaterialTemplate in Game Bootstrap Settings):
        // mul scale, v0.w, 3.81481368e-6; mul xyz, abs(scale), v0.xyz. RE/14 §5.
        let scale = short(6).abs() * 3.814_813_7e-6;
        if scale == 0.0 { return Err("zero static position scale".into()); }
        mesh.positions.push(Vec3::new(short(0), short(2), short(4)) * scale);
        let direction = |p| Vec3::new(v[p] as f32, v[p + 1] as f32, v[p + 2] as f32) * (2.0 / 255.0) - Vec3::ONE;
        mesh.normals.push(direction(8).normalize_or_zero());
        mesh.tangents.push(direction(12).normalize_or_zero().extend(short(6).signum()));
        // PORT: existing atlas UV convention until the vertex-declaration normalization is audited.
        mesh.uvs.push(Vec2::new(short(20), short(22)) * (1.0 / 2048.0));
        mesh.colors.push([v[16] as f32 / 255.0, v[17] as f32 / 255.0, v[18] as f32 / 255.0, v[19] as f32 / 255.0]);
    }
    let tail = 34 + size;
    // CompiledMesh__Read 0xA96610 consumes blob then two u32 fields; Mesh__Read 0x9F6EE0
    // then consumes the material reference array. Two draw tables have DIFFERENT counts.
    if word(d, tail + 8)? as usize != count { return Err("static material count differs from draw count".into()); }
    let tables = 36 + vb + ib;
    for k in 0..count {
        let p = tables + k * 20;
        if word(blob, p)? != 4 { return Err("unsupported static primitive".into()); }
        let first = word(blob, p + 12)? as usize;
        let tris = word(blob, p + 16)? as usize;
        let section = indices.get(first..first + tris * 3).ok_or("invalid static section range")?.to_vec();
        mesh.sections.push(StaticSection { indices: section, material: word(d, tail + 12 + 4 * k)? });
    }
    Ok(mesh)
}

pub struct CollisionMesh {
    pub vertices: Vec<Vec3>,
    pub indices: Vec<u16>,
}

/// Static shape variants used by village props. BoxShape reader 0x6D1F00 supplies half extents
/// and a local matrix. Preserve the authored box faces as triangles instead of world AABB substitutes.
pub fn parse_static_shape(d: &[u8]) -> Result<CollisionMesh,String> {
    if word(d,4)? == crc32("MeshShape") { return parse_collision_mesh(d); }
    if word(d,4)? != crc32("BoxShape") { return Err(format!("unsupported static shape class {:#x}",word(d,4)?)); }
    let half = Vec3::new(float(d,8)?,float(d,12)?,float(d,16)?).max(Vec3::splat(0.025));
    let mut matrix = [0.0;16];
    for (i,v) in matrix.iter_mut().enumerate() { *v = float(d,24+i*4)?; }
    let matrix = Mat4::from_cols_array(&matrix);
    let vertices = (0..8).map(|i| matrix.transform_point3(Vec3::new(if i&1==0 {-half.x} else {half.x},if i&2==0 {-half.y} else {half.y},if i&4==0 {-half.z} else {half.z}))).collect();
    Ok(CollisionMesh { vertices, indices: vec![0,2,3,0,3,1,4,5,7,4,7,6,0,1,5,0,5,4,2,6,7,2,7,3,0,4,6,0,6,2,1,3,7,1,7,5] })
}

/// MeshShape__Read 0x647480; 0x6468B0 binds its u16 array as triangle lists, stride 6.
pub fn parse_collision_mesh(d: &[u8]) -> Result<CollisionMesh, String> {
    if word(d, 4)? != crc32("MeshShape") { return Err("expected MeshShape".into()); }
    let n = word(d, 8)? as usize;
    if n > d.len().saturating_sub(12) / 16 { return Err("invalid collision vertex count".into()); }
    let mut vertices = Vec::with_capacity(n);
    for i in 0..n {
        let p = 12 + i * 16;
        vertices.push(Vec3::new(float(d, p)?, float(d, p + 4)?, float(d, p + 8)?));
    }
    let p = 12 + n * 16;
    let mopp = word(d, p)? as usize;
    let p = p.checked_add(4 + mopp).ok_or("collision size overflow")?;
    let p = p.checked_add(16).ok_or("collision offset overflow")?;
    let count = word(d, p)? as usize;
    if count % 3 != 0 || count > d.len().saturating_sub(p + 4) / 2 { return Err("invalid collision triangle count".into()); }
    let indices: Vec<u16> = d[p + 4..p + 4 + count * 2].chunks_exact(2).map(|p| u16::from_le_bytes(p.try_into().unwrap())).collect();
    if indices.iter().any(|&i| i as usize >= vertices.len()) { return Err("collision index outside vertex buffer".into()); }
    Ok(CollisionMesh { vertices, indices })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_box_shape_keeps_its_rotation_and_rejects_nonfinite_data() {
        let mut data = vec![0u8;92];
        data[4..8].copy_from_slice(&crc32("BoxShape").to_le_bytes());
        for (p,v) in [(8,1.0f32),(12,0.5),(16,2.0)] { data[p..p+4].copy_from_slice(&v.to_le_bytes()); }
        let matrix = Mat4::from_rotation_translation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),Vec3::new(3.0,4.0,5.0));
        for (i,v) in matrix.to_cols_array().iter().enumerate() { data[24+i*4..28+i*4].copy_from_slice(&v.to_le_bytes()); }
        let shape = parse_static_shape(&data).unwrap();
        let min = shape.vertices.iter().fold(Vec3::splat(f32::INFINITY),|a,&p|a.min(p));
        let max = shape.vertices.iter().fold(Vec3::splat(f32::NEG_INFINITY),|a,&p|a.max(p));
        assert!(min.abs_diff_eq(Vec3::new(2.5,3.0,3.0),1e-5));
        assert!(max.abs_diff_eq(Vec3::new(3.5,5.0,7.0),1e-5));
        data[24..28].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse_static_shape(&data).is_err());
    }
    #[test]
    fn native_house_static_mesh_and_collision_from_install() {
        let Some(game) = super::super::find_game_dir() else { return };
        let path = game.join("DataPC_Masyaf.forge");
        if !path.is_file() { return; }
        let mut f = super::super::forge::Forge::open(&path).unwrap();
        let e = f.find("Cell05240_DataBlock").cloned().unwrap();
        let resources = f.resources(&e).unwrap();
        let r = resources.iter().find(|r| r.name == "House_3x3x8_01a" && r.class_hash == crc32("Mesh")).unwrap();
        let m = parse_static_mesh(&r.payload).unwrap();
        assert_eq!(m.positions.len(), 4412);
        assert_eq!(m.sections.len(), 9);
        assert_eq!(m.sections.iter().map(|s| s.indices.len()).sum::<usize>(), 12246);
        let min = m.positions.iter().fold(Vec3::splat(f32::INFINITY), |a, &v| a.min(v));
        let max = m.positions.iter().fold(Vec3::splat(f32::NEG_INFINITY), |a, &v| a.max(v));
        assert!((min.z + 3.027).abs() < 0.005 && (max.z - 8.188).abs() < 0.005);
        let r = resources.iter().find(|r| r.name == "House_3x3x8_01a" && r.class_hash == crc32("MeshShape")).unwrap();
        let shape = parse_collision_mesh(&r.payload).unwrap();
        assert_eq!(shape.vertices.len(), 623);
        assert_eq!(shape.indices.len(), 3768);
        let mut corrupt = r.payload.clone();
        corrupt[12..16].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse_collision_mesh(&corrupt).is_err());
    }
}
