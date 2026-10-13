//! Where water flows once it runs off the land: drainage directions, river
//! discharge and lakes.
//!
//! Drainage uses a priority flood (Barnes et al. 2014) outward from the
//! ocean: every land cell gets a downstream receiver along a surface with its
//! depressions filled. Cells inside a filled depression are basins, and they
//! hold a lake when enough river water reaches them to balance evaporation.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use dirtbox_grid::{CellId, GeodesicGrid};
use dirtbox_planet::Terrain;

pub struct Drainage {
    /// Downstream neighbour of each land cell (`None` for ocean cells and the
    /// outlet of an ocean-free world).
    pub receiver: Vec<Option<CellId>>,
    /// Land cells ordered from upstream to downstream.
    pub order: Vec<CellId>,
    /// Depth of the filled depression at each cell, m (0 outside basins).
    pub basin_depth: Vec<f64>,
}

#[derive(PartialEq)]
struct Item(f64, CellId);
impl Eq for Item {}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap on height, ties broken by cell id for determinism.
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Drainage {
    pub fn build(grid: &GeodesicGrid, terrain: &Terrain) -> Self {
        let n = grid.cell_count();
        let elev = &terrain.elevation;
        let mut filled = elev.clone();
        let mut done = vec![false; n];
        let mut receiver = vec![None; n];
        let mut heap = BinaryHeap::new();
        for c in grid.cells() {
            if terrain.is_ocean(c) {
                done[c as usize] = true;
                heap.push(Item(elev[c as usize], c));
            }
        }
        if heap.is_empty() {
            // No ocean: drain everything toward the lowest point.
            let lowest = (0..n as CellId)
                .min_by(|&a, &b| elev[a as usize].total_cmp(&elev[b as usize]))
                .unwrap();
            done[lowest as usize] = true;
            heap.push(Item(elev[lowest as usize], lowest));
        }
        let mut downstream_first = Vec::with_capacity(n);
        while let Some(Item(h, c)) = heap.pop() {
            if !terrain.is_ocean(c) {
                downstream_first.push(c);
            }
            for &nb in grid.neighbours(c) {
                let j = nb as usize;
                if done[j] {
                    continue;
                }
                done[j] = true;
                filled[j] = elev[j].max(h + 1e-3);
                receiver[j] = Some(c);
                heap.push(Item(filled[j], nb));
            }
        }
        let order = downstream_first.into_iter().rev().collect();
        let basin_depth = filled
            .iter()
            .zip(elev)
            .map(|(f, e)| (f - e).max(0.0))
            .collect();
        Self {
            receiver,
            order,
            basin_depth,
        }
    }

    /// The ocean cell where each land cell's water reaches the sea (`None`
    /// for ocean cells, and for land on a world without an ocean).
    pub fn outlets(&self, terrain: &Terrain) -> Vec<Option<CellId>> {
        let mut outlet = vec![None; self.receiver.len()];
        // Downstream first, so each receiver's outlet is already known.
        for &c in self.order.iter().rev() {
            outlet[c as usize] = match self.receiver[c as usize] {
                Some(r) if terrain.is_ocean(r) => Some(r),
                Some(r) => outlet[r as usize],
                None => None,
            };
        }
        outlet
    }

    /// River discharge (kg s⁻¹ leaving each cell) from per-cell runoff rates
    /// (kg m⁻² s⁻¹) and areas (m²).
    pub fn accumulate(&self, runoff: &[f64], area: &[f64]) -> Vec<f64> {
        let mut flow: Vec<f64> = runoff.iter().zip(area).map(|(r, a)| r * a).collect();
        for &c in &self.order {
            if let Some(r) = self.receiver[c as usize] {
                flow[r as usize] += flow[c as usize];
            }
        }
        flow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirtbox_planet::Preset;

    #[test]
    fn all_land_drains_and_flow_is_conserved() {
        let grid = GeodesicGrid::new(20);
        let terrain = Terrain::generate(&grid, &Preset::EarthModern.params(), 4);
        let d = Drainage::build(&grid, &terrain);
        let area: Vec<f64> = grid.cells().map(|c| grid.area(c)).collect();
        let runoff: Vec<f64> = grid
            .cells()
            .map(|c| if terrain.is_ocean(c) { 0.0 } else { 1.0 })
            .collect();
        let flow = d.accumulate(&runoff, &area);
        // Everything that runs off the land arrives at the ocean.
        let land_total: f64 = grid
            .cells()
            .filter(|&c| !terrain.is_ocean(c))
            .map(|c| area[c as usize])
            .sum();
        let into_ocean: f64 = d
            .order
            .iter()
            .filter(|&&c| d.receiver[c as usize].is_some_and(|r| terrain.is_ocean(r)))
            .map(|&c| flow[c as usize])
            .sum();
        assert!(((into_ocean - land_total) / land_total).abs() < 1e-9);
        // Receivers are neighbours, and every land cell has one.
        for c in grid.cells().filter(|&c| !terrain.is_ocean(c)) {
            let r = d.receiver[c as usize].expect("land cell without receiver");
            assert!(grid.neighbours(c).contains(&r));
        }
    }

    #[test]
    fn every_land_cell_has_an_ocean_outlet() {
        let grid = GeodesicGrid::new(16);
        let terrain = Terrain::generate(&grid, &Preset::EarthModern.params(), 2);
        let d = Drainage::build(&grid, &terrain);
        let outlets = d.outlets(&terrain);
        for c in grid.cells() {
            match outlets[c as usize] {
                Some(o) => assert!(terrain.is_ocean(o) && !terrain.is_ocean(c)),
                None => assert!(terrain.is_ocean(c)),
            }
        }
    }

    #[test]
    fn world_without_ocean_still_drains() {
        let grid = GeodesicGrid::new(10);
        let mut params = Preset::EarthModern.params();
        params.terrain.ocean_fraction = 0.0;
        let terrain = Terrain::generate(&grid, &params, 1);
        let d = Drainage::build(&grid, &terrain);
        let outlets = grid
            .cells()
            .filter(|&c| d.receiver[c as usize].is_none())
            .count();
        assert_eq!(outlets, 1);
    }
}
