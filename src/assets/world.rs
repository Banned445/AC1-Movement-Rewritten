//! Native world-data decoding (RE/06 §2, RE/08). Reads the user's install only.
//! This is the guidance/placement portion of the importer, not a playable map loader.

use bevy::prelude::*;
use std::path::Path;

use super::forge::{crc32, Forge};
use crate::guidance::{GuidanceEdge, GuidanceSubType};

#[derive(Debug, Clone)]
pub struct NativeEdge {
    pub enabled: bool,
    pub subtype: GuidanceSubType,
    pub vertices: [usize; 2],
    pub normals: [Vec3; 2],
}

#[derive(Debug, Default, Clone)]
pub struct NativeGuidance {
    pub id: u32,
    pub active: bool,
    pub edges: Vec<NativeEdge>,
    pub vertices: Vec<Vec3>,
    pub filter_flags: u8,
    pub filter_cos_angle: f32,
    pub filter_corner_angle: f32,
    pub check_world_orientation: bool,
}

#[derive(Debug, Clone)]
pub struct NativePlacement {
    pub id: u32,
    pub name: String,
    /// Game coordinates: Z up, row-vector serialized matrix transposed for glam.
    pub transform: Mat4,
    pub guidance: NativeGuidance,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let end = self.pos.checked_add(N).ok_or("world-data offset overflow")?;
        let bytes = self.data.get(self.pos..end).ok_or_else(|| format!("truncated world data at {}", self.pos))?;
        self.pos = end;
        Ok(bytes.try_into().unwrap())
    }

    fn u32(&mut self) -> Result<u32, String> { Ok(u32::from_le_bytes(self.take()?)) }
    fn bool(&mut self) -> Result<bool, String> {
        match self.take::<1>()?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("invalid world-data boolean".into()),
        }
    }
    fn f32(&mut self) -> Result<f32, String> {
        let value = f32::from_le_bytes(self.take()?);
        if value.is_finite() { Ok(value) } else { Err("non-finite world-data float".into()) }
    }
    fn object(&mut self, class: &str) -> Result<u32, String> {
        let id = self.u32()?;
        if self.u32()? != crc32(class) { return Err(format!("expected {class} object")); }
        Ok(id)
    }
    fn count(&mut self, stride: usize) -> Result<usize, String> {
        let count = self.u32()? as usize;
        // Native SmallArray count occupies 14 bits (0x545AF0). Check input before allocating.
        if count > 0x3fff || count > self.data.len().saturating_sub(self.pos) / stride {
            return Err("invalid world-data array count".into());
        }
        Ok(count)
    }
    fn normal(&mut self) -> Result<Vec3, String> {
        // GuidanceObject__Read 0x5ACC10 packs stream floats via 0x5ABF60, then queries
        // unpack signed 10-bit components /511 (0x5ABC40). Preserve that quantization.
        let mut values = [0.0; 3];
        for v in &mut values { *v = (self.f32()?.clamp(-1.0, 1.0) * 511.0).trunc() / 511.0; }
        self.f32()?; // fourth component is packed separately; spatial queries use xyz
        Ok(Vec3::from_array(values))
    }
}

fn subtype(value: u32) -> Result<GuidanceSubType, String> {
    match value {
        0 => Ok(GuidanceSubType::None),
        1 => Ok(GuidanceSubType::LedgeGrab),
        2 => Ok(GuidanceSubType::Beam),
        3 => Ok(GuidanceSubType::Ladder),
        4 => Ok(GuidanceSubType::Pole),
        5 => Ok(GuidanceSubType::Rope),
        6 => Ok(GuidanceSubType::Surface),
        7 => Ok(GuidanceSubType::Quadruped),
        8 => Ok(GuidanceSubType::Kiosk),
        _ => Err(format!("unsupported guidance subtype {value}")),
    }
}

/// Parse an embedded GuidanceSystem beginning at its object id/class header.
/// Component read: 0x5271C0 / 0x545AF0; object read: 0x5ACC10; filter: 0x6B08B0.
/// The Partitioner is retained in the source data, but is not decoded here.
pub fn parse_guidance(data: &[u8]) -> Result<NativeGuidance, String> {
    let mut r = Reader { data, pos: 0 };
    let id = r.object("GuidanceSystem")?;
    let active = r.bool()?;
    let count = r.count(53)?;
    let mut edges = Vec::with_capacity(count);
    for _ in 0..count {
        r.object("GuidanceObject")?;
        edges.push(NativeEdge {
            enabled: r.bool()?,
            subtype: subtype(r.u32()?)?,
            vertices: [r.u32()? as usize, r.u32()? as usize],
            normals: [r.normal()?, r.normal()?],
        });
    }
    let coords = r.count(2)?;
    if coords % 3 != 0 { return Err("guidance coordinates are not xyz triples".into()); }
    let mut vertices = Vec::with_capacity(coords / 3);
    for _ in 0..coords / 3 {
        let mut xyz = [0.0; 3];
        for v in &mut xyz { *v = u16::from_le_bytes(r.take()?) as f32 * 0.005 - 163.84; }
        vertices.push(Vec3::from_array(xyz));
    }
    if edges.iter().any(|e| e.vertices.iter().any(|&v| v >= vertices.len() || v > 0x1fff)) {
        return Err("guidance edge references a missing vertex".into());
    }
    r.object("EdgeFilter")?;
    let mut filter_flags = 0;
    for bit in 0..7 { if r.bool()? { filter_flags |= 1 << bit; } }
    r.f32()?; // first float is overwritten with cos(last float) by 0x6B08B0
    let filter_corner_angle = r.f32()?;
    let filter_cos_angle = r.f32()?.cos();
    let check_world_orientation = r.bool()?;
    // 0x66A000 then reads a Partitioner object pointer (0x931780).
    match r.take::<1>()?[0] {
        0 => { r.object("Partitioner")?; }
        2 => { r.u32()?; }
        3 => {}
        _ => return Err("unsupported guidance partitioner pointer".into()),
    }
    Ok(NativeGuidance { id, active, edges, vertices, filter_flags, filter_cos_angle, filter_corner_angle, check_world_orientation })
}

/// Entity's serialized 4x4 placement (Entity__Read 0x450E90 → 0x52BB00 → 0x4D9C10).
pub fn entity_transform(data: &[u8]) -> Result<Mat4, String> {
    let mut r = Reader { data, pos: 0 };
    r.object("Entity")?;
    let mut matrix = [0.0; 16];
    for value in &mut matrix { *value = r.f32()?; }
    if matrix[3] != 0.0 || matrix[7] != 0.0 || matrix[11] != 0.0 || matrix[15] != 1.0 {
        return Err("entity placement is not affine".into());
    }
    let matrix = Mat4::from_cols_array(&matrix);
    if matrix.determinant().abs() < 1e-8 { return Err("singular entity placement".into()); }
    Ok(matrix)
}

/// Inspect placements carrying a static GuidanceSystem in one archive cell.
/// PORT: strict hash-marker discovery until the complete entity/component graph is decoded.
/// Reject malformed/ambiguous candidates; this is not used to populate movement's GuidanceWorld.
pub fn inspect_cell(game: &Path, archive: &str, cell: &str) -> Result<Vec<NativePlacement>, String> {
    let mut forge = Forge::open(&game.join(archive)).map_err(|e| e.to_string())?;
    let entry = forge.find(cell).cloned().ok_or_else(|| format!("missing map cell {cell}"))?;
    let resources = forge.resources(&entry).map_err(|e| e.to_string())?;
    let marker = crc32("GuidanceSystem").to_le_bytes();
    let mut placements = Vec::new();
    for resource in resources.iter().filter(|r| r.class_hash == crc32("Entity")) {
        let offsets: Vec<usize> = resource.payload.windows(4).enumerate().filter_map(|(o, bytes)| (bytes == marker && o >= 4).then_some(o - 4)).collect();
        if offsets.is_empty() { continue; }
        if offsets.len() != 1 { return Err(format!("{}: ambiguous guidance ownership", resource.name)); }
        let guidance = parse_guidance(&resource.payload[offsets[0]..]).map_err(|e| format!("{}: {e}", resource.name))?;
        placements.push(NativePlacement {
            id: resource.id,
            name: resource.name.clone(),
            transform: entity_transform(&resource.payload)?,
            guidance,
        });
    }
    Ok(placements)
}

impl NativePlacement {
    /// Transform authored positions and both normals into Bevy's Y-up frame. Keep source
    /// edge order/identity, enabled bits and subtypes; no geometry-derived holds are added.
    pub fn world_edges(&self) -> Vec<(bool, GuidanceEdge)> {
        let convert = |p: Vec3| Vec3::new(p.x, p.z, -p.y);
        self.guidance.edges.iter().map(|e| {
            (self.guidance.active && e.enabled, GuidanceEdge {
                p0: convert(self.transform.transform_point3(self.guidance.vertices[e.vertices[0]])),
                p1: convert(self.transform.transform_point3(self.guidance.vertices[e.vertices[1]])),
                n0: convert(self.transform.transform_vector3(e.normals[0])),
                n1: convert(self.transform.transform_vector3(e.normals[1])),
                subtype: e.subtype,
            })
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut data = Vec::new();
        let mut word = |v: u32| data.extend(v.to_le_bytes());
        word(1); word(crc32("GuidanceSystem"));
        data.push(1);
        data.extend(1u32.to_le_bytes());
        data.extend(2u32.to_le_bytes()); data.extend(crc32("GuidanceObject").to_le_bytes());
        data.push(1);
        for v in [3u32, 0, 1] { data.extend(v.to_le_bytes()); }
        for v in [0.0f32, 1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0] { data.extend(v.to_le_bytes()); }
        data.extend(6u32.to_le_bytes());
        for v in [32768u16, 32768, 32768, 32768, 32768, 33568] { data.extend(v.to_le_bytes()); }
        data.extend(3u32.to_le_bytes()); data.extend(crc32("EdgeFilter").to_le_bytes());
        data.extend([0, 1, 0, 0, 1, 0, 0]);
        for v in [0.707f32, 0.3927, std::f32::consts::FRAC_PI_4] { data.extend(v.to_le_bytes()); }
        data.extend([0, 3]);
        data
    }

    #[test]
    fn native_guidance_rejects_truncation_and_bad_references() {
        let data = fixture();
        for end in 0..data.len() { assert!(parse_guidance(&data[..end]).is_err(), "accepted truncation at {end}"); }
        let mut bad = data.clone();
        bad[26..30].copy_from_slice(&2u32.to_le_bytes());
        assert!(parse_guidance(&bad).unwrap_err().contains("missing vertex"));
        let mut bad = data;
        bad[34..38].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse_guidance(&bad).unwrap_err().contains("non-finite"));
    }

    #[test]
    fn native_guidance_preserves_ladder_and_disabled_edge_identity() {
        let mut guidance = parse_guidance(&fixture()).unwrap();
        assert_eq!(guidance.edges[0].subtype, GuidanceSubType::Ladder);
        guidance.edges[0].enabled = false;
        let placement = NativePlacement { id: 1, name: "synthetic".into(), transform: Mat4::from_translation(Vec3::new(10.0, 20.0, 30.0)), guidance };
        let edges = placement.world_edges();
        assert_eq!(edges.len(), 1);
        assert!(!edges[0].0);
        assert!(edges[0].1.p0.distance(Vec3::new(10.0, 30.0, -20.0)) < 0.0001);
        assert!((edges[0].1.p1.y - edges[0].1.p0.y - 4.0).abs() < 0.0001);
    }

    #[test]
    fn masyaf_native_placements_and_holds_from_install() {
        let Some(game) = super::super::find_game_dir() else { eprintln!("skipped: game install not found"); return; };
        if !game.join("DataPC_Masyaf.forge").is_file() { eprintln!("skipped: Masyaf archive absent"); return; }
        let placements = inspect_cell(&game, "DataPC_Masyaf.forge", "Cell01952_DataBlock").unwrap();
        let house = placements.iter().find(|p| p.name == "House_3x3x8_01a_017").unwrap();
        assert_eq!(house.guidance.edges.len(), 93);
        assert_eq!(house.guidance.vertices.len(), 115);
        assert!(house.guidance.edges.iter().all(|e| e.subtype == GuidanceSubType::LedgeGrab));
        assert!(house.transform.w_axis.truncate().distance(Vec3::new(28.50049, -40.749153, 34.50558)) < 0.0001);
        for name in ["Ladder_4m_030", "Ladder_4m_031"] {
            let ladder = placements.iter().find(|p| p.name == name).unwrap();
            assert_eq!(ladder.guidance.edges.len(), 1);
            assert_eq!(ladder.guidance.edges[0].subtype, GuidanceSubType::Ladder);
        }
        for placement in placements {
            for (_, edge) in placement.world_edges() { assert!(edge.p0.is_finite() && edge.p1.is_finite() && edge.n0.is_finite() && edge.n1.is_finite()); }
        }
    }
}
