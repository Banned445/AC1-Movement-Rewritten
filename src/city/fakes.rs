//! Distant city: `FakeEntities` (RE/17 §6, archive observations 2026-10-09).
//!
//! The World's FakeEntities resource lists one `FakeEntity` per 4·cell-size block of the grid (Damascus: 8 × 8 blocks
//! of 128 m, in Z-order). A non-empty block is an inline Entity (placed at the block centre) whose Visual draws one
//! merged low-detail `Damascus_FakeMesh_N`, followed by one `SubMeshFakeIndexSpans` per mesh section: 16 `FakeIndexSpan
//! {u32 first index, u32 index count}` records, one per finest grid cell of the block (Z-order inside the block too:
//! all 1,047 non-empty Damascus spans then fall on cells that have content; row-major leaves 39 on empty cells). The game hides a span while its cell's real objects are loaded
//! (**hypothesis**: the switch itself was not traced; the data only fixes which indices belong to which cell).

use std::sync::Arc;

use bevy::prelude::*;

use super::decode::{placement, CityData, TO_BEVY};
use crate::assets::{forge::{crc32, Resource}, static_mesh::{parse_static_mesh, word, StaticMesh}};

pub struct FakeSpan {
    /// Finest-level grid cell index (into `Grid::cells`).
    pub cell: Option<usize>,
    pub section: usize,
    pub first: u32,
    pub count: u32,
}

pub struct FakeBlock {
    pub name: String,
    pub transform: Mat4,
    pub mesh: Arc<StaticMesh>,
    pub colors: Option<Vec<[u8; 4]>>,
    pub spans: Vec<FakeSpan>,
}

fn markers(b: &[u8], class: &str) -> Vec<usize> {
    let h = crc32(class).to_le_bytes();
    b.windows(4).enumerate().filter(|(_, w)| *w == h).map(|(p, _)| p).collect()
}

/// Z-order block index → (column, row) on a `blocks`-wide grid.
fn morton(i: usize) -> (usize, usize) {
    let (mut x, mut y) = (0, 0);
    for b in 0..8 { x |= ((i >> (2 * b)) & 1) << b; y |= ((i >> (2 * b + 1)) & 1) << b; }
    (x, y)
}

pub fn parse(data: &CityData, globals: &[Arc<Resource>]) -> Result<Vec<FakeBlock>, String> {
    let Some(fakes) = globals.iter().find(|r| r.class_hash == crc32("FakeEntities")) else { return Ok(Vec::new()) };
    let d = &fakes.payload;
    let records = markers(d, "FakeEntity");
    let grid = &data.grid;
    let per = 4usize; // finest cells per block side: 1 << (block level 2)
    let blocks = grid.width as usize / per;
    let mut out = Vec::new();
    for (k, &p) in records.iter().enumerate() {
        let end = records.get(k + 1).map_or(d.len(), |&q| q - 4);
        let rec = &d[p + 4..end];
        let Some(&v) = markers(rec, "Visual").first() else { continue };
        if rec.get(v + 4) != Some(&1) { continue; }
        let mesh_id = word(rec, v + 5)?;
        let mesh_res = data.archives.get(mesh_id)?;
        let mesh = parse_static_mesh(&mesh_res.payload).map_err(|e| format!("{}: {e}", mesh_res.name))?;
        let transform = TO_BEVY * placement(rec)?;
        // the record's own CompiledMeshInstance colours
        let colors = markers(rec, "CompiledMeshInstance").first().and_then(|&c| {
            let bytes = word(rec, c + 4).ok()? as usize;
            let blob = rec.get(c + 8..c + 8 + bytes)?;
            let n = mesh.positions.len();
            (blob.len() == 16 + 4 * n).then(|| blob[16..].chunks_exact(4).map(|c| [c[2], c[1], c[0], c[3]]).collect())
        });
        let (bx, by) = morton(k);
        let mut spans = Vec::new();
        for (section, &s) in markers(rec, "SubMeshFakeIndexSpans").iter().enumerate() {
            let n = word(rec, s + 4)? as usize;
            for c in 0..n.min(per * per) {
                let q = s + 8 + 16 * c;
                if word(rec, q + 4)? != crc32("FakeIndexSpan") { return Err("malformed FakeIndexSpan".into()); }
                let (first, count) = (word(rec, q + 8)?, word(rec, q + 12)?);
                if count == 0 { continue; }
                let (ix, iy) = morton(c);
                let (cx, cy) = (bx * per + ix, by * per + iy);
                let cell = (cx < grid.width as usize && cy < grid.width as usize).then(|| cy * grid.width as usize + cx);
                spans.push(FakeSpan { cell, section, first, count });
            }
        }
        if bx >= blocks || by >= blocks { continue; }
        out.push(FakeBlock { name: mesh_res.name.clone(), transform, mesh: Arc::new(mesh), colors, spans });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn fake_spans_belong_to_cells_with_content() {
        let Some(game) = crate::assets::find_game_dir() else { return };
        if !game.join(super::super::DAMASCUS.archive).is_file() { return; }
        let w = super::super::open(&game, &super::super::DAMASCUS, true).unwrap();
        let blocks = super::parse(&w.data, &w.globals).unwrap();
        assert_eq!(blocks.len(), 24);
        // a span's cell must be a finest cell whose stored file has entities (non-empty files are > 104 bytes)
        let a = &w.data.archives;
        let mut empty = 0;
        let mut total = 0;
        for b in &blocks {
            let idx = b.mesh.sections.iter().map(|s| s.indices.len()).sum::<usize>();
            assert!(b.spans.iter().all(|s| (s.first + s.count) as usize <= idx * 4));
            for s in &b.spans {
                total += 1;
                let cell = &w.data.grid.cells[s.cell.unwrap()];
                let (_, file) = a.location(cell.datablock).unwrap();
                let entry = a.forges[0].find(file).unwrap();
                if entry.size <= 104 { empty += 1; }
            }
        }
        assert!(total > 50 && empty == 0, "{empty} of {total} spans point at empty cells");
    }
}
