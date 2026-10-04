//! Decoders for AC1 `Mesh` and `TextureMap` resource payloads (RE/09).

use super::forge::crc32;

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Skinned-mesh position quantisation: metres = s16 / 2048 (fitted, RE/09 §3.3).
pub const POS_SCALE: f32 = 1.0 / 2048.0;
/// UVs are s16 / 4096 (RE/09 §3.3, hypothesis checked visually).
pub const UV_SCALE: f32 = 1.0 / 4096.0;

#[derive(Debug, Clone)]
pub struct MeshBone {
    pub bone_id: u32,
    /// Inverse bind matrix, row-major, row-vector convention (translation in row 3).
    pub inv_bind: [f32; 16],
}

#[derive(Debug, Clone)]
pub struct SubMesh {
    pub vstart: u32,
    pub vcount: u32,
    pub istart: u32,
    pub tris: u32,
    pub palette: Vec<u8>,
    /// Material resource id used by this submesh (Mesh material list, by submesh order).
    pub material: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct AcMesh {
    pub bones: Vec<MeshBone>,
    /// Game space (Z-up, metres, model origin at the hips).
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub bone_idx: Vec<[u8; 4]>,
    pub bone_w: Vec<[u8; 4]>,
    pub indices: Vec<u16>,
    pub submeshes: Vec<SubMesh>,
    /// Offset just after the palettes (start of the material list search).
    pub tail: usize,
}

pub const CLASS_MESH: u32 = 0x415D_9568;
pub const CLASS_COMPILED_MESH: u32 = 0xFC9E_1595;

/// Parse a skinned Mesh payload (vertex format 0x16, stride 32). Returns None for other layouts
/// (e.g. the 24-byte unskinned weapon format, not decoded yet).
pub fn parse_mesh(d: &[u8]) -> Option<AcMesh> {
    if u32_at(d, 4)? != CLASS_MESH {
        return None;
    }
    let nb = u32_at(d, 16)? as usize;
    let mut bones = Vec::with_capacity(nb);
    let mut p = 20;
    for _ in 0..nb {
        let bone_id = u32_at(d, p + 8)?;
        let mut m = [0f32; 16];
        for (k, v) in m.iter_mut().enumerate() {
            *v = f32_at(d, p + 12 + 4 * k);
        }
        bones.push(MeshBone { bone_id, inv_bind: m });
        p += 76;
    }
    // CompiledMesh object: find its class hash after the bone list
    let cm = (p..d.len().saturating_sub(4)).find(|&o| u32_at(d, o) == Some(CLASS_COMPILED_MESH))?;
    let data = cm + 8;
    let stride = u32_at(d, data + 4)? as usize;
    let vb = u32_at(d, data + 8)? as usize;
    let ib = u32_at(d, data + 12)? as usize;
    let nsub = u32_at(d, data + 24)? as usize;
    if stride != 32 {
        return None;
    }
    let v0 = data + 36;
    let nv = vb / stride;
    let mut m = AcMesh {
        bones,
        positions: Vec::with_capacity(nv),
        normals: Vec::with_capacity(nv),
        uvs: Vec::with_capacity(nv),
        bone_idx: Vec::with_capacity(nv),
        bone_w: Vec::with_capacity(nv),
        indices: Vec::with_capacity(ib / 2),
        submeshes: Vec::new(),
        tail: 0,
    };
    for i in 0..nv {
        let o = v0 + i * stride;
        let q = [i16_at(d, o), i16_at(d, o + 2), i16_at(d, o + 4)];
        m.positions.push([q[0] as f32 * POS_SCALE, q[1] as f32 * POS_SCALE, q[2] as f32 * POS_SCALE]);
        let n = &d[o + 8..o + 11];
        let nv3 = [n[0] as f32 / 127.5 - 1.0, n[1] as f32 / 127.5 - 1.0, n[2] as f32 / 127.5 - 1.0];
        m.normals.push(nv3);
        m.uvs.push([i16_at(d, o + 20) as f32 * UV_SCALE, i16_at(d, o + 22) as f32 * UV_SCALE]);
        m.bone_idx.push(d[o + 24..o + 28].try_into().unwrap());
        m.bone_w.push(d[o + 28..o + 32].try_into().unwrap());
    }
    let i0 = v0 + vb;
    for k in 0..ib / 2 {
        m.indices.push(u16::from_le_bytes(d[i0 + 2 * k..i0 + 2 * k + 2].try_into().unwrap()));
    }
    // submesh table {4, vstart, vcount, istart, tris} (written twice), then palettes
    let mut t = i0 + ib;
    for k in 0..nsub {
        let o = t + 20 * k;
        m.submeshes.push(SubMesh {
            vstart: u32_at(d, o + 4)?,
            vcount: u32_at(d, o + 8)?,
            istart: u32_at(d, o + 12)?,
            tris: u32_at(d, o + 16)?,
            palette: Vec::new(),
            material: None,
        });
    }
    t += 40 * nsub;
    for s in m.submeshes.iter_mut() {
        if d.get(t) != Some(&3) || d.get(t + 1) != Some(&1) {
            return None;
        }
        let n = d[t + 2] as usize;
        s.palette = d[t + 7..t + 7 + n].to_vec();
        t += 7 + n;
    }
    m.tail = t;
    Some(m)
}

/// Material list after the palettes: `u32 count == nsub` followed by that many resource ids; the
/// caller validates candidates with `is_material` (all ids must be Material resources).
pub fn resolve_materials(d: &[u8], m: &mut AcMesh, is_material: impl Fn(u32) -> bool) {
    let n = m.submeshes.len();
    for o in m.tail..d.len().saturating_sub(4 * (n + 1) - 1) {
        if u32_at(d, o) != Some(n as u32) {
            continue;
        }
        let ids: Vec<u32> = (0..n).filter_map(|k| u32_at(d, o + 4 + 4 * k)).collect();
        if ids.len() == n && ids.iter().all(|&id| is_material(id)) {
            for (s, id) in m.submeshes.iter_mut().zip(ids) {
                s.material = Some(id);
            }
            return;
        }
    }
}

/// One skeleton bone (RE/09 §3). Transforms are in skeleton space (Z-up, metres, hips origin).
#[derive(Debug, Clone)]
pub struct SkelBone {
    pub object_id: u32,
    /// CRC32 of the bone name.
    pub bone_id: u32,
    pub parent: Option<usize>,
    pub local_pos: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub local_rot: [f32; 4],
    pub global_pos: [f32; 3],
    pub global_rot: [f32; 4],
}

pub const CLASS_BONE: u32 = 0x9574_1049;

/// Parse a Skeleton payload: inline Bone objects `{u8 0, u32 objId, u32 Bone, u32 BoneID,
/// ObjectPtr parent (u8 2 + id | u8 3), transform object {u8 0, u32 id, u32 class, vec4 posA, quat rotA,
/// vec4 posB, quat rotB}}` — A = global, B = local (verified: parent.A ∘ child.B = child.A exactly).
pub fn parse_skeleton(d: &[u8]) -> Vec<SkelBone> {
    let mut bones: Vec<SkelBone> = Vec::new();
    let mut parents: Vec<Option<u32>> = Vec::new();
    let mut i = 8;
    while i + 9 < d.len() {
        if d[i] == 0 && u32_at(d, i + 5) == Some(CLASS_BONE) {
            let mut p = i + 9;
            let object_id = u32_at(d, i + 1).unwrap();
            let bone_id = u32_at(d, p).unwrap();
            p += 4;
            let parent = if d[p] == 2 {
                p += 5;
                u32_at(d, p - 4)
            } else {
                p += 1;
                None
            };
            p += 9; // embedded transform object header
            let f = |o: usize| f32_at(d, o);
            let ga = [f(p), f(p + 4), f(p + 8)];
            let gr = [f(p + 16), f(p + 20), f(p + 24), f(p + 28)];
            let la = [f(p + 32), f(p + 36), f(p + 40)];
            let lr = [f(p + 48), f(p + 52), f(p + 56), f(p + 60)];
            bones.push(SkelBone { object_id, bone_id, parent: None, local_pos: la, local_rot: lr, global_pos: ga, global_rot: gr });
            parents.push(parent);
            i = p + 64;
        } else {
            i += 1;
        }
    }
    for k in 0..bones.len() {
        if let Some(pid) = parents[k] {
            bones[k].parent = bones.iter().position(|b| b.object_id == pid);
        }
    }
    bones
}

/// Decoded texture: RGBA8 mip chain (largest first).
pub struct AcTexture {
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
}

fn chain_size(w: u32, h: u32, block_bytes: u32, mips: u32) -> u32 {
    let mut total = 0;
    let (mut w, mut h) = (w, h);
    for _ in 0..mips {
        total += w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * block_bytes;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    total
}

/// Parse a TextureMap payload: width @+8, height @+12, mip count @+0x20, then a u32 data size
/// followed by a BC1 (8-byte blocks) or BC3 (16-byte blocks) mip chain (RE/09 §4).
pub fn parse_texture(d: &[u8]) -> Option<AcTexture> {
    let w = u32_at(d, 8)?;
    let h = u32_at(d, 12)?;
    let mips = u32_at(d, 0x20)?;
    if !(1..=4096).contains(&w) || !(1..=4096).contains(&h) || !(1..=13).contains(&mips) {
        return None;
    }
    for (block, bc3) in [(8u32, false), (16u32, true)] {
        let size = chain_size(w, h, block, mips);
        if let Some(off) = (0x20..0x100.min(d.len().saturating_sub(4))).find(|&o| u32_at(d, o) == Some(size)) {
            let data = d.get(off + 4..off + 4 + size as usize)?;
            let mut out = Vec::new();
            let (mut mw, mut mh, mut p) = (w, h, 0usize);
            for _ in 0..mips {
                let bytes = (mw.div_ceil(4).max(1) * mh.div_ceil(4).max(1) * block) as usize;
                out.push(decode_bc(&data[p..p + bytes], mw, mh, bc3));
                p += bytes;
                mw = (mw / 2).max(1);
                mh = (mh / 2).max(1);
            }
            return Some(AcTexture { width: w, height: h, mips: out });
        }
    }
    None
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]
}

fn decode_bc(src: &[u8], w: u32, h: u32, bc3: bool) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let block = if bc3 { 16 } else { 8 };
    for by in 0..bh {
        for bx in 0..bw {
            let b = &src[(by * bw + bx) * block..][..block];
            let (alpha, col) = if bc3 { (Some(&b[0..8]), &b[8..16]) } else { (None, &b[0..8]) };
            let c0 = u16::from_le_bytes([col[0], col[1]]);
            let c1 = u16::from_le_bytes([col[2], col[3]]);
            let (p0, p1) = (rgb565(c0), rgb565(c1));
            let mut pal = [[0u8; 4]; 4];
            pal[0] = [p0[0], p0[1], p0[2], 255];
            pal[1] = [p1[0], p1[1], p1[2], 255];
            if c0 > c1 || bc3 {
                for k in 0..3 {
                    pal[2][k] = ((2 * p0[k] as u32 + p1[k] as u32) / 3) as u8;
                    pal[3][k] = ((p0[k] as u32 + 2 * p1[k] as u32) / 3) as u8;
                }
                pal[2][3] = 255;
                pal[3][3] = 255;
            } else {
                for k in 0..3 {
                    pal[2][k] = ((p0[k] as u32 + p1[k] as u32) / 2) as u8;
                }
                pal[2][3] = 255;
                pal[3] = [0, 0, 0, 0];
            }
            let bits = u32::from_le_bytes([col[4], col[5], col[6], col[7]]);
            let mut alphas = [255u8; 16];
            if let Some(a) = alpha {
                let (a0, a1) = (a[0] as u32, a[1] as u32);
                let mut ap = [0u32; 8];
                ap[0] = a0;
                ap[1] = a1;
                if a0 > a1 {
                    for k in 1..7 {
                        ap[k + 1] = ((7 - k as u32) * a0 + k as u32 * a1) / 7;
                    }
                } else {
                    for k in 1..5 {
                        ap[k + 1] = ((5 - k as u32) * a0 + k as u32 * a1) / 5;
                    }
                    ap[6] = 0;
                    ap[7] = 255;
                }
                let abits = a[2..8].iter().rev().fold(0u64, |acc, &x| (acc << 8) | x as u64);
                for (k, al) in alphas.iter_mut().enumerate() {
                    *al = ap[((abits >> (3 * k)) & 7) as usize] as u8;
                }
            }
            for py in 0..4 {
                for px in 0..4 {
                    let (x, y) = (bx * 4 + px, by * 4 + py);
                    if x >= w || y >= h {
                        continue;
                    }
                    let k = py * 4 + px;
                    let mut c = pal[((bits >> (2 * k)) & 3) as usize];
                    if bc3 {
                        c[3] = alphas[k];
                    }
                    out[(y * w + x) * 4..][..4].copy_from_slice(&c);
                }
            }
        }
    }
    out
}

/// Class hashes used when resolving materials (CRC32 of the class names).
pub fn class_hash(name: &str) -> u32 {
    crc32(name)
}
