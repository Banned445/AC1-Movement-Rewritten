//! City collision shapes beyond MeshShape / BoxShape (RE/17 §4). Archive observations, 2026-10-09:
//! - `BarrelShape` (0x97CD8890, reflection size 112): `{id, class, matrix (64), u32 n, n × plane vec4 (normal, d),
//!   u32 m, m × vertex vec4, material id}`: a convex hull. Every hull vertex lies on its faces' planes
//!   (|n·v + d| < 1e-3), so each face is rebuilt from the vertices on its plane.
//! - `CapsuleShape` (0xB8599052): `{id, class, vec4 a, vec4 b, f32 radius, material id}` (local to the rigid body).
//! - `ListShape` (0x86EBFD8D): `{id, class, u32 n, n × (u8 0, inline shape)}`.
//! The exe readers were not traced (field order from the reflection descriptors and the data).

use bevy::prelude::*;

use crate::assets::{forge::crc32, static_mesh::{parse_collision_mesh, parse_static_shape, word, CollisionMesh}};

fn float(d: &[u8], p: usize) -> Result<f32, String> {
    let v = f32::from_bits(word(d, p)?);
    if v.is_finite() { Ok(v) } else { Err("non-finite shape data".into()) }
}

fn vec3(d: &[u8], p: usize) -> Result<Vec3, String> {
    Ok(Vec3::new(float(d, p)?, float(d, p + 4)?, float(d, p + 8)?))
}

fn matrix(d: &[u8], p: usize) -> Result<Mat4, String> {
    let mut m = [0.0; 16];
    for (i, v) in m.iter_mut().enumerate() { *v = float(d, p + 4 * i)?; }
    Ok(Mat4::from_cols_array(&m))
}

/// Parse any supported static shape; returns the triangles (shape-local) and the bytes consumed.
pub fn parse_shape(d: &[u8]) -> Result<(CollisionMesh, usize), String> {
    let class = word(d, 4)?;
    if class == crc32("MeshShape") { return Ok((parse_collision_mesh(d)?, d.len())); }
    if class == crc32("BoxShape") { return Ok((parse_static_shape(d)?, 92)); }
    if class == crc32("BarrelShape") { return barrel(d); }
    if class == crc32("CapsuleShape") { return capsule(d); }
    if class == crc32("ListShape") { return list(d); }
    Err(format!("unsupported shape class {class:#x}"))
}

fn barrel(d: &[u8]) -> Result<(CollisionMesh, usize), String> {
    let m = matrix(d, 8)?;
    let n = word(d, 72)? as usize;
    if n > 4096 { return Err("invalid hull plane count".into()); }
    let planes = (0..n).map(|i| Ok((vec3(d, 76 + 16 * i)?, float(d, 88 + 16 * i)?))).collect::<Result<Vec<_>, String>>()?;
    let q = 76 + 16 * n;
    let count = word(d, q)? as usize;
    if count > 4096 { return Err("invalid hull vertex count".into()); }
    let vertices = (0..count).map(|i| vec3(d, q + 4 + 16 * i)).collect::<Result<Vec<_>, _>>()?;
    let end = q + 4 + 16 * count + 4;
    if end > d.len() { return Err("truncated BarrelShape".into()); }
    let mut indices = Vec::new();
    for (normal, dist) in planes {
        let on: Vec<u16> = (0..vertices.len()).filter(|&i| (normal.dot(vertices[i]) + dist).abs() < 2e-3).map(|i| i as u16).collect();
        if on.len() < 3 { continue; }
        let centre = on.iter().map(|&i| vertices[i as usize]).sum::<Vec3>() / on.len() as f32;
        let u = (vertices[on[0] as usize] - centre).normalize_or_zero();
        let v = normal.cross(u);
        let mut ring = on.clone();
        ring.sort_by(|&a, &b| {
            let pa = vertices[a as usize] - centre; let pb = vertices[b as usize] - centre;
            pa.dot(v).atan2(pa.dot(u)).total_cmp(&pb.dot(v).atan2(pb.dot(u)))
        });
        for k in 1..ring.len() - 1 { indices.extend([ring[0], ring[k], ring[k + 1]]); }
    }
    if indices.is_empty() { return Err("BarrelShape has no faces".into()); }
    Ok((CollisionMesh { vertices: vertices.iter().map(|&p| m.transform_point3(p)).collect(), indices }, end))
}

/// PORT: Havok collides with the exact capsule; the port's triangle world gets a 12-sided capsule hull.
fn capsule(d: &[u8]) -> Result<(CollisionMesh, usize), String> {
    let (a, b, r) = (vec3(d, 8)?, vec3(d, 24)?, float(d, 40)?);
    if !(r > 0.0) || d.len() < 48 { return Err("invalid CapsuleShape".into()); }
    let axis = (b - a).try_normalize().unwrap_or(Vec3::Z);
    let u = axis.any_orthonormal_vector();
    let v = axis.cross(u);
    const SIDES: usize = 12;
    // rings: a's pole, a's hemisphere, the two cylinder ends, b's hemisphere, b's pole
    let rings: [(Vec3, f32, f32); 4] = [(a, -0.7071, 0.7071), (a, 0.0, 1.0), (b, 0.0, 1.0), (b, 0.7071, 0.7071)];
    let mut vertices = vec![a - axis * r];
    for (c, along, radial) in rings {
        for k in 0..SIDES {
            let t = k as f32 / SIDES as f32 * std::f32::consts::TAU;
            vertices.push(c + axis * (along * r) + (u * t.cos() + v * t.sin()) * (radial * r));
        }
    }
    vertices.push(b + axis * r);
    let top = (vertices.len() - 1) as u16;
    let mut indices = Vec::new();
    for k in 0..SIDES as u16 {
        let n = (k + 1) % SIDES as u16;
        indices.extend([0, 1 + n, 1 + k]);
        for ring in 0..3u16 {
            let (p, q) = (1 + ring * SIDES as u16, 1 + (ring + 1) * SIDES as u16);
            indices.extend([p + k, p + n, q + n, p + k, q + n, q + k]);
        }
        indices.extend([1 + 3 * SIDES as u16 + k, 1 + 3 * SIDES as u16 + n, top]);
    }
    Ok((CollisionMesh { vertices, indices }, 48))
}

fn list(d: &[u8]) -> Result<(CollisionMesh, usize), String> {
    let n = word(d, 8)? as usize;
    if n > 256 { return Err("invalid ListShape count".into()); }
    let mut out = CollisionMesh { vertices: Vec::new(), indices: Vec::new() };
    let mut p = 12;
    for _ in 0..n {
        if d.get(p) != Some(&0) { return Err("ListShape child is not inline".into()); }
        let child = d.get(p + 1..).ok_or("truncated ListShape")?;
        if word(child, 4)? == crc32("MeshShape") { return Err("inline MeshShape in ListShape".into()); }
        let (mesh, used) = parse_shape(child)?;
        let base = out.vertices.len() as u16;
        out.vertices.extend(mesh.vertices);
        out.indices.extend(mesh.indices.iter().map(|i| i + base));
        p += 1 + used;
    }
    Ok((out, p + 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(d: &mut Vec<u8>, v: &[f32]) { for x in v { d.extend(x.to_le_bytes()); } }

    #[test]
    fn barrel_hull_faces_are_rebuilt_from_vertices_on_their_planes() {
        // a unit cube as a hull: 6 planes, 8 vertices, translated by (0, 0, 5)
        let mut d = Vec::new();
        d.extend(7u32.to_le_bytes()); d.extend(crc32("BarrelShape").to_le_bytes());
        put(&mut d, &Mat4::from_translation(Vec3::Z * 5.0).to_cols_array());
        d.extend(6u32.to_le_bytes());
        for n in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z] { put(&mut d, &[n.x, n.y, n.z, -0.5]); }
        d.extend(8u32.to_le_bytes());
        for i in 0..8 { put(&mut d, &[if i & 1 == 0 { -0.5 } else { 0.5 }, if i & 2 == 0 { -0.5 } else { 0.5 }, if i & 4 == 0 { -0.5 } else { 0.5 }, 0.0]); }
        d.extend(0u32.to_le_bytes());
        let (mesh, used) = parse_shape(&d).unwrap();
        assert_eq!(used, d.len());
        assert_eq!(mesh.indices.len(), 6 * 2 * 3);
        let area: f32 = mesh.indices.chunks(3).map(|t| {
            let [a, b, c] = [0, 1, 2].map(|k| mesh.vertices[t[k] as usize]);
            (b - a).cross(c - a).length() * 0.5
        }).sum();
        assert!((area - 6.0).abs() < 1e-4, "{area}");
        assert!(mesh.vertices.iter().all(|v| (v.z - 5.0).abs() <= 0.5 + 1e-5));
    }

    #[test]
    fn capsule_and_list_shapes_triangulate() {
        let mut cap = Vec::new();
        cap.extend(1u32.to_le_bytes()); cap.extend(crc32("CapsuleShape").to_le_bytes());
        put(&mut cap, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 1.0, 0.5]);
        cap.extend(0u32.to_le_bytes());
        let (mesh, used) = parse_shape(&cap).unwrap();
        assert_eq!(used, 48);
        let (lo, hi) = mesh.vertices.iter().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(v.z), h.max(v.z)));
        assert!((lo + 0.5).abs() < 1e-5 && (hi - 2.5).abs() < 1e-5);
        let mut list = Vec::new();
        list.extend(2u32.to_le_bytes()); list.extend(crc32("ListShape").to_le_bytes()); list.extend(2u32.to_le_bytes());
        for _ in 0..2 { list.push(0); list.extend(&cap); }
        list.extend(0u32.to_le_bytes());
        let (mesh2, used) = parse_shape(&list).unwrap();
        assert_eq!(used, list.len());
        assert_eq!(mesh2.indices.len(), 2 * mesh.indices.len());
    }
}
