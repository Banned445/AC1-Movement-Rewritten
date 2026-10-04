//! Loads Altaïr's model from the user's own game install at runtime (nothing is copied into the
//! project). Source: DataPC.forge → file "Rank 9" (his fully upgraded outfit).

use std::collections::HashMap;
use std::path::Path;

use super::ac_formats::{parse_mesh, parse_skeleton, parse_texture, resolve_materials, AcMesh, AcTexture, SkelBone};
use super::forge::{crc32, Forge, Resource};

/// Body parts that make up the Rank 9 outfit (resource names in the archive).
pub const PARTS: &[&str] = &[
    "UCMA_Altair_Body_C",
    "UCMA_Altair_Head",
    "UCMA_Altair_Boots_B",
    "UCMA_Altair_Cloth_UP",
    "UCMA_Altair_Flaps",
    "UCMA_Altair_Shoulderpad",
    "UCMA_Altair_BackSheath_B",
    "UCMA_Altair_Sword_Sheath_D",
    "UCMA_Altair_Knife_Foot",
    "UCMA_Altair_Knife_Shoulder",
    "UCMA_Altair_Knife_Belt",
];

pub struct PartMesh {
    pub name: String,
    /// Bevy space (Y-up, feet at y = 0, facing -Z), metres.
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Skin: up to 4 skeleton joint indices per vertex and normalised weights.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// One index list per submesh, with its diffuse texture id.
    pub sections: Vec<(Vec<u32>, Option<u32>)>,
}

pub struct AltairModel {
    pub parts: Vec<PartMesh>,
    pub textures: HashMap<u32, AcTexture>,
    /// Altaïr's skeleton (UCMA_Altair), game skeleton space.
    pub skeleton: Vec<SkelBone>,
    /// Lowest vertex z in model space (feet), used to put the feet at y = 0.
    pub min_z: f32,
    pub source: String,
}

/// Skeleton space → mesh model space: a constant 90° rotation about Z (verified on all 74 shared
/// bones, RE/09 §3.3): model = X0 · skeleton with X0 = [[0,1,0],[-1,0,0],[0,0,1]] (column vectors).
pub fn skeleton_to_model(v: [f32; 3]) -> [f32; 3] {
    [v[1], -v[0], v[2]]
}

/// Game space (Z-up, face toward -Y) → Bevy (Y-up, facing -Z): (x, y, z) → (-x, z, y).
fn to_bevy(v: [f32; 3]) -> [f32; 3] {
    [-v[0], v[2], v[1]]
}

/// Row-vector 4x4 helpers (game matrices: translation in row 3).
fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut r = [0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            r[i * 4 + j] = (0..4).map(|k| a[i * 4 + k] * b[k * 4 + j]).sum();
        }
    }
    r
}

/// Inverse of a rigid transform (rotation + translation), row-vector convention.
fn rigid_inverse(m: &[f32; 16]) -> [f32; 16] {
    let mut r = [0f32; 16];
    for i in 0..3 {
        for j in 0..3 {
            r[i * 4 + j] = m[j * 4 + i];
        }
    }
    for j in 0..3 {
        r[12 + j] = -(0..3).map(|k| m[12 + k] * r[k * 4 + j]).sum::<f32>();
    }
    r[15] = 1.0;
    r
}

fn xform_point(v: [f32; 3], m: &[f32; 16]) -> [f32; 3] {
    let mut r = [0f32; 3];
    for (j, rj) in r.iter_mut().enumerate() {
        *rj = v[0] * m[j] + v[1] * m[4 + j] + v[2] * m[8 + j] + m[12 + j];
    }
    r
}

fn xform_dir(v: [f32; 3], m: &[f32; 16]) -> [f32; 3] {
    let mut r = [0f32; 3];
    for (j, rj) in r.iter_mut().enumerate() {
        *rj = v[0] * m[j] + v[1] * m[4 + j] + v[2] * m[8 + j];
    }
    r
}

fn refs_in(payload: &[u8], pred: impl Fn(u32) -> bool) -> Vec<u32> {
    let mut out = Vec::new();
    for o in 0..payload.len().saturating_sub(3) {
        let v = u32::from_le_bytes(payload[o..o + 4].try_into().unwrap());
        if pred(v) && !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

pub fn load_altair(game_dir: &Path) -> Result<AltairModel, String> {
    let path = game_dir.join("DataPC.forge");
    let mut forge = Forge::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let entry = forge.find("Rank 9").cloned().ok_or("file 'Rank 9' not found in DataPC.forge")?;
    let res = forge.resources(&entry).map_err(|e| e.to_string())?;
    let by_id: HashMap<u32, &Resource> = res.iter().map(|r| (r.id, r)).collect();
    let class = |name: &str| crc32(name);
    let (c_mesh, c_mat, c_set, c_tex) = (class("Mesh"), class("Material"), class("TextureSet"), class("TextureMap"));
    let find = |name: &str, cls: u32| res.iter().find(|r| r.name == name && r.class_hash == cls);

    // The character Entity carries a material override table: adjacent (placeholder, real) Material
    // id pairs, e.g. Hands_Empty → Leather at Rank 9 (RE/09 §4.3).
    let is_mat = |v: u32| by_id.get(&v).is_some_and(|r| r.class_hash == c_mat);
    let mut overrides: HashMap<u32, u32> = HashMap::new();
    if let Some(ent) = find("UCMA_Altair_Rank_9", crc32("Entity")) {
        let p = &ent.payload;
        for o in 0..p.len().saturating_sub(7) {
            let a = u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
            let b = u32::from_le_bytes(p[o + 4..o + 8].try_into().unwrap());
            if a != b && is_mat(a) && is_mat(b) {
                overrides.entry(a).or_insert(b);
            }
        }
    }

    // material → diffuse texture id
    let diffuse_of = |mat_id: u32| -> Option<u32> {
        let mut mat = *by_id.get(overrides.get(&mat_id).unwrap_or(&mat_id))?;
        if let Some(base) = mat.name.strip_suffix("_Empty") {
            // no override: the real material has the same name without "_Empty"
            mat = find(base, c_mat)?;
        }
        // Material → TextureSet → TextureMapSpec ("…DiffuseMapSpec") → TextureMap (RE/09 §4.2)
        let sets = refs_in(&mat.payload, |v| by_id.get(&v).is_some_and(|r| r.class_hash == c_set));
        let set = by_id.get(sets.first()?)?;
        let mut frontier = vec![set.id];
        for _ in 0..3 {
            let mut next = Vec::new();
            for id in frontier {
                let r = by_id.get(&id)?;
                for v in refs_in(&r.payload, |v| v != id && by_id.contains_key(&v)) {
                    let t = by_id[&v];
                    if !t.name.contains("Diffuse") {
                        continue;
                    }
                    if t.class_hash == c_tex {
                        return Some(v);
                    }
                    next.push(v);
                }
            }
            frontier = next;
        }
        None
    };

    // parse parts (game space)
    let mut parsed: Vec<(String, AcMesh)> = Vec::new();
    for &name in PARTS {
        let Some(r) = find(name, c_mesh) else { continue };
        let Some(mut m) = parse_mesh(&r.payload) else { continue };
        resolve_materials(&r.payload, &mut m, |id| by_id.get(&id).is_some_and(|r| r.class_hash == c_mat));
        parsed.push((name.to_string(), m));
    }
    if parsed.is_empty() {
        return Err("no Altaïr meshes could be parsed".into());
    }

    // Parts whose shared bones have a different bind matrix than the body (head, knives, sword
    // sheath) are modelled in that bone's space: map them into body space through the shared bone
    // (v_body = v_part · invBind_part(b) · bind_body(b)), RE/09 §3.5. Parts with identical binds are
    // already in body space.
    let body_bones: HashMap<u32, [f32; 16]> =
        parsed.iter().find(|(n, _)| n == "UCMA_Altair_Body_C").map(|(_, m)| m.bones.iter().map(|b| (b.bone_id, b.inv_bind)).collect()).unwrap_or_default();
    // bone a bone-space part is attached through (skin fallback for bones the body skeleton lacks)
    let mut attach_bone: HashMap<String, u32> = HashMap::new();
    for (name, m) in parsed.iter_mut() {
        if name == "UCMA_Altair_Body_C" {
            continue;
        }
        let same_bind = |b: &super::ac_formats::MeshBone| {
            body_bones.get(&b.bone_id).is_some_and(|bm| bm.iter().zip(b.inv_bind.iter()).all(|(x, y)| (x - y).abs() < 1e-3))
        };
        if m.bones.iter().any(same_bind) {
            continue;
        }
        if let Some(shared) = m.bones.iter().find(|b| body_bones.contains_key(&b.bone_id)) {
            attach_bone.insert(name.clone(), shared.bone_id);
            let t = mat_mul(&shared.inv_bind, &rigid_inverse(&body_bones[&shared.bone_id]));
            for p in m.positions.iter_mut() {
                *p = xform_point(*p, &t);
            }
            for n in m.normals.iter_mut() {
                *n = xform_dir(*n, &t);
            }
        }
    }

    // feet on the ground: lowest vertex → y = 0
    let min_z = parsed.iter().flat_map(|(_, m)| m.positions.iter().map(|p| p[2])).fold(f32::MAX, f32::min);

    // skeleton (for skinning / animation)
    let skeleton = find("UCMA_Altair", crc32("Skeleton")).map(|r| parse_skeleton(&r.payload)).unwrap_or_default();
    let joint_of: HashMap<u32, u16> = skeleton.iter().enumerate().map(|(i, b)| (b.bone_id, i as u16)).collect();

    let mut textures = HashMap::new();
    let mut parts = Vec::new();
    for (name, m) in parsed {
        let positions: Vec<[f32; 3]> = m.positions.iter().map(|&p| { let b = to_bevy(p); [b[0], b[1] - min_z, b[2]] }).collect();
        let normals: Vec<[f32; 3]> = m.normals.iter().map(|&n| to_bevy(n)).collect();
        // winding: compare geometric face normals with the stored vertex normals and flip if needed
        let mut agree = 0i64;
        for tri in m.indices.chunks_exact(3) {
            let (a, b, c) = (positions[tri[0] as usize], positions[tri[1] as usize], positions[tri[2] as usize]);
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let fnrm = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let vn = normals[tri[0] as usize];
            agree += if fnrm[0] * vn[0] + fnrm[1] * vn[1] + fnrm[2] * vn[2] >= 0.0 { 1 } else { -1 };
        }
        let flip = agree < 0;
        let mut sections = Vec::new();
        for s in &m.submeshes {
            let range = s.istart as usize..(s.istart + 3 * s.tris) as usize;
            let mut idx: Vec<u32> = m.indices[range].iter().map(|&i| i as u32).collect();
            if flip {
                for tri in idx.chunks_exact_mut(3) {
                    tri.swap(1, 2);
                }
            }
            let tex = s.material.and_then(diffuse_of);
            if let Some(t) = tex {
                if let std::collections::hash_map::Entry::Vacant(e) = textures.entry(t) {
                    if let Some(decoded) = by_id.get(&t).and_then(|r| parse_texture(&r.payload)) {
                        e.insert(decoded);
                    }
                }
            }
            sections.push((idx, tex.filter(|t| textures.contains_key(t))));
        }
        // skin: palette-local bone → mesh bone → BoneID → skeleton joint; bones the body skeleton
        // lacks (face, tags) fall back to the part's attach bone (or the root)
        let fallback = attach_bone.get(&name).and_then(|b| joint_of.get(b)).copied().unwrap_or(0);
        // Bones the 90-bone skeleton lacks (the hood / robe cloth bones and the sword bone, driven by the game's
        // cloth and attachment systems) take the skeleton joint nearest to their bind position, never the root
        // (`Reference`): with the root, the hood and robe tore off whenever an animation moved the root away
        // from the body (jump takeoffs). PORT: no cloth simulation, so they follow that joint rigidly.
        let missing_joint = |bone: &super::ac_formats::MeshBone| -> u16 {
            if let Some(j) = attach_bone.get(&name).and_then(|b| joint_of.get(b)) {
                return *j;
            }
            // bind position in body space (bone-space parts were mapped through the body's bind of that bone)
            let bind = rigid_inverse(body_bones.get(&bone.bone_id).unwrap_or(&bone.inv_bind));
            let p = [bind[12], bind[13], bind[14]];
            let mut best = (f32::MAX, fallback);
            for (j, sb) in skeleton.iter().enumerate() {
                if sb.parent.is_none() {
                    continue;
                }
                let q = skeleton_to_model(sb.global_pos);
                let d = (0..3).map(|k| (q[k] - p[k]).powi(2)).sum::<f32>();
                if d < best.0 {
                    best = (d, j as u16);
                }
            }
            best.1
        };
        let mut joints = vec![[0u16; 4]; m.positions.len()];
        let mut weights = vec![[1.0f32, 0.0, 0.0, 0.0]; m.positions.len()];
        for s in &m.submeshes {
            for v in s.vstart as usize..(s.vstart + s.vcount) as usize {
                let mut j = [fallback; 4];
                let mut w = [0f32; 4];
                for k in 0..4 {
                    let local = m.bone_idx[v][k] as usize;
                    w[k] = m.bone_w[v][k] as f32 / 255.0;
                    let mesh_bone = s.palette.get(local).and_then(|&mb| m.bones.get(mb as usize));
                    j[k] = match mesh_bone {
                        Some(b) => joint_of.get(&b.bone_id).copied().unwrap_or_else(|| missing_joint(b)),
                        None => fallback,
                    };
                }
                let sum: f32 = w.iter().sum();
                if sum > 1e-4 {
                    for x in w.iter_mut() {
                        *x /= sum;
                    }
                } else {
                    w = [1.0, 0.0, 0.0, 0.0];
                }
                joints[v] = j;
                weights[v] = w;
            }
        }
        parts.push(PartMesh { name, positions, normals, uvs: m.uvs.clone(), joints, weights, sections });
    }
    Ok(AltairModel { parts, textures, skeleton, min_z, source: format!("{} / Rank 9", path.display()) })
}
