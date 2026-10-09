//! The world's streaming grid (RE/17 §1): `World.GridLayout` and the `GridPartition` cell list.
//!
//! Archive observations (Damascus and Masyaf, 2026-10-09):
//! - `GridLayout` (class 0x7DB083ED, embedded in the World payload) serializes a byte, then five i32:
//!   cell size (32), origin x (−480), origin y (−512), 0, and the finest level's width in cells (32).
//! - `GridPartition` (class 0x0E2F4444) = `{u32 levels, u32 cells, cells × GridCell, u32 n, n bytes}` with
//!   `GridCell = {u32 0, u32 class 0x3435AD40, u32 GridCellDataBlock id, u32 n, n × u32 resource ids}`.
//!   The cell count is the sum of `(width >> level)²` over the levels: each level halves the width and doubles the cell
//!   size, so large objects sit in coarser cells. Cell k of level L covers
//!   `origin + size·2^L·(col, row)` .. `+ size·2^L` with `col, row = k % (width >> L), k / (width >> L)`.
//! GridLayout in memory = serialized order (verified 2026-10-09): +0 cell size, +4 / +8 origin x / y, +12 (0), +16 finest
//! width (`GridLayout__CellIndexAt` 0x54FA00, `GridLayout__LevelInfo` 0x54F780: level L has cell size `size << L`, width
//! `width >> L`, base Σ earlier widths²), +20 byte = the loading radius outside the grid (`GridPartition__LoadingRadiusAt`
//! 0x68AE10). The partition's trailing byte map is the **loading radius in metres per finest cell** (0x68AE10).

use bevy::prelude::*;

use crate::assets::{forge::crc32, static_mesh::word};

#[derive(Clone, Debug)]
pub struct GridCell {
    pub level: u32,
    pub col: u32,
    pub row: u32,
    /// GridCellDataBlock resource id (the cell's stored file holds it).
    pub datablock: u32,
    /// Ids the cell lists besides its own file: shared dependencies (meshes, materials, …) in other files (hypothesis).
    pub dependencies: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct Grid {
    pub cell_size: f32,
    /// Game coordinates (x, y) of the grid's corner; z is up.
    pub origin: Vec2,
    pub width: u32,
    pub levels: u32,
    pub cells: Vec<GridCell>,
    /// The partition's trailing per-finest-cell byte map: loading radius in metres (0x68AE10).
    pub cell_bytes: Vec<u8>,
    /// GridLayout +20: the loading radius where the position is outside the grid.
    pub default_radius: u8,
}

impl Grid {
    /// Game-space rectangle (min, max) of a cell.
    pub fn cell_rect(&self, c: &GridCell) -> (Vec2, Vec2) {
        let size = self.cell_size * (1u32 << c.level) as f32;
        let min = self.origin + Vec2::new(c.col as f32, c.row as f32) * size;
        (min, min + Vec2::splat(size))
    }

    pub fn level_size(&self, level: u32) -> f32 {
        self.cell_size * (1u32 << level) as f32
    }

    /// Game-space rectangle of the whole grid.
    pub fn bounds(&self) -> (Vec2, Vec2) {
        (self.origin, self.origin + Vec2::splat(self.cell_size * self.width as f32))
    }

    /// `GridPartition__LoadingRadiusAt` 0x68AE10: the finest cell under `p` (`GridLayout__CellIndexAt` 0x54FA00, integer
    /// truncation) picks its byte; outside the grid the GridLayout's byte.
    pub fn loading_radius(&self, p: Vec2) -> f32 {
        let size = self.cell_size as i32;
        let (dx, dy) = (p.x as i32 - self.origin.x as i32, p.y as i32 - self.origin.y as i32);
        let (x, y) = (dx / size, dy / size);
        let w = self.width as i32;
        if dx < 0 || dy < 0 || x >= w || y >= w { return self.default_radius as f32; }
        self.cell_bytes.get((x + w * y) as usize).copied().unwrap_or(self.default_radius) as f32
    }

    /// `GridPartition__IterCellsInBox` 0x68C3E0 + `GridStreamer__RequestCellAndParents` 0x55CC70: the finest cells under the
    /// square box `(int)p ± r` (clamped to the grid) and all their parents, i.e. every cell of any level overlapping it.
    pub fn cell_in_box(&self, c: &GridCell, p: Vec2, r: i32) -> bool {
        let size = self.cell_size as i32;
        let w = self.width as i32;
        let lo = |v: i32, o: i32| ((v - r - o).max(0) / size).min(w - 1);
        let hi = |v: i32, o: i32| ((v + r - o).max(0) / size).min(w - 1);
        let (ox, oy) = (self.origin.x as i32, self.origin.y as i32);
        let (px, py) = (p.x as i32, p.y as i32);
        if px + r < ox || py + r < oy || px - r >= ox + size * w || py - r >= oy + size * w { return false; }
        let shift = c.level as i32;
        let (x0, x1, y0, y1) = (lo(px, ox) >> shift, hi(px, ox) >> shift, lo(py, oy) >> shift, hi(py, oy) >> shift);
        (x0..=x1).contains(&(c.col as i32)) && (y0..=y1).contains(&(c.row as i32))
    }
}

/// Read `GridLayout` out of a World payload: (cell size, origin, finest width, default loading radius).
pub fn parse_layout(world: &[u8]) -> Result<(f32, Vec2, u32, u8), String> {
    let hash = crc32("GridLayout").to_le_bytes();
    let at: Vec<usize> = world.windows(4).enumerate().filter(|(_, w)| *w == hash).map(|(p, _)| p).collect();
    let &[p] = at.as_slice() else { return Err("World has no unique GridLayout".into()) };
    let i = |k: usize| word(world, p + 5 + 4 * k).map(|v| v as i32);
    let (size, ox, oy, width) = (i(0)?, i(1)?, i(2)?, i(4)?);
    if !(1..=4096).contains(&size) || !(1..=1024).contains(&width) {
        return Err(format!("implausible GridLayout: size {size}, width {width}"));
    }
    Ok((size as f32, Vec2::new(ox as f32, oy as f32), width as u32, world[p + 4]))
}

pub fn parse_partition(data: &[u8], size: f32, origin: Vec2, width: u32, default_radius: u8) -> Result<Grid, String> {
    if word(data, 4)? != crc32("GridPartition") { return Err("expected GridPartition".into()); }
    let levels = word(data, 8)?;
    let count = word(data, 12)? as usize;
    let expected: usize = (0..levels).map(|l| ((width >> l) as usize).pow(2)).sum();
    if levels == 0 || levels > 12 || width >> (levels - 1) == 0 || count != expected {
        return Err(format!("GridPartition has {count} cells for {levels} levels of width {width} (expected {expected})"));
    }
    let mut cells = Vec::with_capacity(count);
    let mut p = 16;
    let (mut level, mut base) = (0u32, 0usize);
    for k in 0..count {
        while k >= base + ((width >> level) as usize).pow(2) { base += ((width >> level) as usize).pow(2); level += 1; }
        if word(data, p + 4)? != crc32("GridCell") { return Err(format!("GridCell {k} has the wrong class")); }
        let datablock = word(data, p + 8)?;
        let n = word(data, p + 12)? as usize;
        if n > data.len() / 4 { return Err("invalid GridCell dependency count".into()); }
        let dependencies = (0..n).map(|i| word(data, p + 16 + 4 * i)).collect::<Result<Vec<_>, _>>()?;
        let w = width >> level;
        let local = (k - base) as u32;
        cells.push(GridCell { level, col: local % w, row: local / w, datablock, dependencies });
        p += 16 + 4 * n;
    }
    let n = word(data, p)? as usize;
    let cell_bytes = data.get(p + 4..p + 4 + n).ok_or("truncated GridPartition byte map")?.to_vec();
    Ok(Grid { cell_size: size, origin, width, levels, cells, cell_bytes, default_radius })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_levels_halve_the_width_and_double_the_cell() {
        let mut data = Vec::new();
        let mut w = |v: u32| data.extend(v.to_le_bytes());
        w(1); w(crc32("GridPartition")); w(2); w(5);
        for k in 0..5u32 { w(0); w(crc32("GridCell")); w(100 + k); w(1); w(7); }
        w(0);
        let g = parse_partition(&data, 32.0, Vec2::new(-64.0, -64.0), 4, 95).err();
        assert!(g.is_some(), "4x4 + 2x2 = 20 cells, not 5");
        let mut data = Vec::new();
        let mut w = |v: u32| data.extend(v.to_le_bytes());
        w(1); w(crc32("GridPartition")); w(2); w(5);
        for k in 0..5u32 { w(0); w(crc32("GridCell")); w(100 + k); w(0); }
        w(2); data.extend([9u8, 8]);
        let g = parse_partition(&data, 32.0, Vec2::new(-64.0, -64.0), 2, 95).unwrap();
        assert_eq!(g.cells.len(), 5);
        assert_eq!((g.cells[3].level, g.cells[3].col, g.cells[3].row), (0, 1, 1));
        assert_eq!(g.cell_rect(&g.cells[3]), (Vec2::new(-32.0, -32.0), Vec2::new(0.0, 0.0)));
        assert_eq!(g.cell_rect(&g.cells[4]), (Vec2::new(-64.0, -64.0), Vec2::new(0.0, 0.0)));
        assert_eq!(g.cell_bytes, vec![9, 8]);
        // radius byte of the finest cell under the point, the layout byte outside
        assert_eq!(g.loading_radius(Vec2::new(-10.0, -60.0)), 8.0);
        assert_eq!(g.loading_radius(Vec2::new(-70.0, 0.0)), 95.0);
        // a 10 m box at (-48, -48) covers finest cell (0,0) and the top cell only; at (-40, -40) it reaches cell (1,1)
        assert!(g.cell_in_box(&g.cells[0], Vec2::new(-48.0, -48.0), 10) && g.cell_in_box(&g.cells[4], Vec2::new(-48.0, -48.0), 10));
        assert!(!g.cell_in_box(&g.cells[3], Vec2::new(-48.0, -48.0), 10) && g.cell_in_box(&g.cells[3], Vec2::new(-40.0, -40.0), 10));
    }
}
