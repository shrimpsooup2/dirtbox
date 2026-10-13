//! Terrain: elevation, sea level, which cells are ocean, and hydrothermal
//! vents. Generated from seeded noise, then editable by the player (design
//! §11.2: terraforming).

use std::collections::VecDeque;

use dirtbox_core::Vec3;
use dirtbox_core::rng::derive_key;
use dirtbox_grid::{CellId, GeodesicGrid};

use crate::noise::Noise3;
use crate::params::PlanetParams;

const TERRAIN_DOMAIN: u64 = 0x7E44;

#[derive(Clone, Debug)]
pub struct Terrain {
    /// Height of each cell's surface above the planet's datum, m.
    pub elevation: Vec<f64>,
    sea_level: f64,
    ocean: Vec<bool>,
    /// Hydrothermal vent strength per cell, 0–1 (non-zero only in the ocean).
    /// Vents supply chemical energy for early life (Phase 2).
    pub vents: Vec<f64>,
}

/// What a terraforming brush does inside its radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BrushMode {
    /// Raise the ground by up to `amount` metres at the centre.
    Raise,
    /// Lower the ground by up to `amount` metres at the centre.
    Lower,
    /// Move each cell toward the mean of its neighbours (`amount` is 0–1).
    Smooth,
    /// Move each cell toward a target height (`amount` is 0–1).
    Flatten { target: f64 },
}

#[derive(Clone, Copy, Debug)]
pub struct Brush {
    pub mode: BrushMode,
    /// Brush centre on the unit sphere.
    pub centre: Vec3,
    /// Radius as an angle on the sphere (radians).
    pub radius: f64,
    pub amount: f64,
}

/// Highest and lowest elevation the player can terraform to, m.
pub const ELEVATION_LIMIT: f64 = 20_000.0;

impl Terrain {
    pub fn generate(grid: &GeodesicGrid, params: &PlanetParams, seed: u64) -> Self {
        let t = &params.terrain;
        let noise = Noise3::new(derive_key(seed, &[TERRAIN_DOMAIN]));
        let f = 1.2 * t.continent_scale.max(0.1);
        let raw: Vec<f64> = grid
            .cells()
            .map(|c| {
                let p = grid.position(c);
                // Domain-warped continents, mountain chains along them, fine detail.
                let warp = Vec3::new(
                    noise.fbm(p.x * 1.5 + 10.0, p.y * 1.5, p.z * 1.5, 3, 0.5),
                    noise.fbm(p.x * 1.5, p.y * 1.5 + 20.0, p.z * 1.5, 3, 0.5),
                    noise.fbm(p.x * 1.5, p.y * 1.5, p.z * 1.5 + 30.0, 3, 0.5),
                ) * 0.35;
                let q = p * f + warp;
                let continent = noise.fbm(q.x, q.y, q.z, 6, 0.5);
                let ridge = noise.ridged(p.x * f * 2.5 + 40.0, p.y * f * 2.5, p.z * f * 2.5, 5);
                let detail = noise.fbm(p.x * f * 8.0 + 80.0, p.y * f * 8.0, p.z * f * 8.0, 4, 0.5);
                let inland = smoothstep(-0.05, 0.25, continent);
                continent + t.roughness * (0.25 * ridge * inland + 0.08 * detail)
            })
            .collect();

        let areas: Vec<f64> = grid.cells().map(|c| grid.area(c)).collect();
        let raw_sea = weighted_quantile(&raw, &areas, t.ocean_fraction);
        let (lo, hi) = raw
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        let g = params.relative_gravity().max(0.05);
        let land_scale = t.max_elevation / g;
        let ocean_scale = t.ocean_depth / g;
        let elevation = raw
            .iter()
            .map(|&r| {
                if r >= raw_sea {
                    // Hypsometry: most land is low, a little is high.
                    let h = (r - raw_sea) / (hi - raw_sea).max(1e-9);
                    land_scale * h.powf(1.6) + 1.0
                } else {
                    // Continental shelf near the coast, then the abyssal plain.
                    let d = (raw_sea - r) / (raw_sea - lo).max(1e-9);
                    -ocean_scale * (0.12 * d + 0.88 * smoothstep(0.0, 0.18, d))
                }
            })
            .collect();

        let ridges = Noise3::new(derive_key(seed, &[TERRAIN_DOMAIN, 1]));
        let mut terrain = Terrain {
            elevation,
            sea_level: 0.0,
            ocean: Vec::new(),
            vents: vec![0.0; grid.cell_count()],
        };
        terrain.recompute_ocean(grid);
        for c in grid.cells() {
            if terrain.ocean[c as usize] {
                let p = grid.position(c) * (f * 1.8);
                let ridge = ridges.ridged(p.x + 123.0, p.y, p.z, 3);
                terrain.vents[c as usize] =
                    smoothstep(0.78, 0.95, ridge) * t.tectonic_activity.min(1.5);
            }
        }
        terrain
    }

    /// Terrain from explicit elevations (m), e.g. a hand-built test world or
    /// a blank canvas to terraform. No vents.
    pub fn from_elevation(grid: &GeodesicGrid, elevation: Vec<f64>, sea_level: f64) -> Self {
        assert_eq!(elevation.len(), grid.cell_count());
        let mut t = Terrain {
            elevation,
            sea_level,
            ocean: Vec::new(),
            vents: vec![0.0; grid.cell_count()],
        };
        t.recompute_ocean(grid);
        t
    }

    pub fn sea_level(&self) -> f64 {
        self.sea_level
    }

    pub fn is_ocean(&self, cell: CellId) -> bool {
        self.ocean[cell as usize]
    }

    pub fn ocean_mask(&self) -> &[bool] {
        &self.ocean
    }

    /// Height above sea level (negative = depth below it), m.
    pub fn height_above_sea(&self, cell: CellId) -> f64 {
        self.elevation[cell as usize] - self.sea_level
    }

    /// Fraction of the surface area that is ocean.
    pub fn ocean_fraction(&self, grid: &GeodesicGrid) -> f64 {
        let total: f64 = grid
            .cells()
            .filter(|&c| self.is_ocean(c))
            .map(|c| grid.area(c))
            .sum();
        total / (4.0 * std::f64::consts::PI)
    }

    /// Moves the sea level (floods or drains the planet).
    pub fn set_sea_level(&mut self, grid: &GeodesicGrid, level: f64) {
        self.sea_level = level.clamp(-ELEVATION_LIMIT, ELEVATION_LIMIT);
        self.recompute_ocean(grid);
    }

    /// Applies a brush stroke. Returns the cells whose elevation changed.
    pub fn apply_brush(&mut self, grid: &GeodesicGrid, brush: &Brush) -> Vec<CellId> {
        let centre = brush.centre.normalized();
        let cells = cells_within(grid, centre, brush.radius);
        let before: Vec<f64> = cells.iter().map(|&c| self.elevation[c as usize]).collect();
        let mut changed = Vec::with_capacity(cells.len());
        for (&c, &old) in cells.iter().zip(&before) {
            let d = grid.position(c).dot(centre).clamp(-1.0, 1.0).acos();
            let w = falloff(d / brush.radius.max(1e-9));
            let new = match brush.mode {
                BrushMode::Raise => old + brush.amount * w,
                BrushMode::Lower => old - brush.amount * w,
                BrushMode::Smooth => {
                    // Use pre-stroke heights so the result doesn't depend on order.
                    let nbs = grid.neighbours(c);
                    let mean = nbs
                        .iter()
                        .map(|&n| match cells.iter().position(|&x| x == n) {
                            Some(i) => before[i],
                            None => self.elevation[n as usize],
                        })
                        .sum::<f64>()
                        / nbs.len() as f64;
                    old + brush.amount.clamp(0.0, 1.0) * w * (mean - old)
                }
                BrushMode::Flatten { target } => {
                    old + brush.amount.clamp(0.0, 1.0) * w * (target - old)
                }
            }
            .clamp(-ELEVATION_LIMIT, ELEVATION_LIMIT);
            if new != old {
                self.elevation[c as usize] = new;
                changed.push(c);
            }
        }
        if !changed.is_empty() {
            self.recompute_ocean(grid);
        }
        changed
    }

    /// Ocean = cells below sea level that belong to a large connected body of
    /// water. Small isolated basins below sea level stay dry land (they may
    /// hold lakes if rivers fill them).
    fn recompute_ocean(&mut self, grid: &GeodesicGrid) {
        let n = grid.cell_count();
        let below: Vec<bool> = self.elevation.iter().map(|&e| e < self.sea_level).collect();
        let mut component = vec![u32::MAX; n];
        let mut sizes = Vec::new();
        let mut queue = VecDeque::new();
        for start in 0..n {
            if !below[start] || component[start] != u32::MAX {
                continue;
            }
            let id = sizes.len() as u32;
            let mut size = 0usize;
            component[start] = id;
            queue.push_back(start as CellId);
            while let Some(c) = queue.pop_front() {
                size += 1;
                for &nb in grid.neighbours(c) {
                    if below[nb as usize] && component[nb as usize] == u32::MAX {
                        component[nb as usize] = id;
                        queue.push_back(nb);
                    }
                }
            }
            sizes.push(size);
        }
        let largest = sizes.iter().copied().max().unwrap_or(0);
        let threshold = (largest / 50).max(20).min(largest);
        self.ocean = component
            .iter()
            .map(|&id| id != u32::MAX && sizes[id as usize] >= threshold)
            .collect();
    }
}

/// Cells whose centres lie within `radius` (radians) of `centre`, found by a
/// breadth-first search outward from the nearest cell.
pub fn cells_within(grid: &GeodesicGrid, centre: Vec3, radius: f64) -> Vec<CellId> {
    let start = grid.locate(centre);
    let cos_r = radius.cos();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut queue = VecDeque::from([start]);
    seen.insert(start);
    while let Some(c) = queue.pop_front() {
        if grid.position(c).dot(centre) < cos_r && c != start {
            continue;
        }
        out.push(c);
        for &nb in grid.neighbours(c) {
            if seen.insert(nb) {
                queue.push_back(nb);
            }
        }
    }
    out.sort_unstable();
    out
}

/// Smooth brush falloff: 1 at the centre, 0 at the rim.
fn falloff(x: f64) -> f64 {
    if x >= 1.0 { 0.0 } else { (1.0 - x * x).powi(2) }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Value below which `fraction` of the total weight lies.
fn weighted_quantile(values: &[f64], weights: &[f64], fraction: f64) -> f64 {
    if fraction <= 0.0 {
        return values.iter().copied().fold(f64::MAX, f64::min) - 1.0;
    }
    if fraction >= 1.0 {
        return values.iter().copied().fold(f64::MIN, f64::max) + 1.0;
    }
    let mut idx: Vec<usize> = (0..values.len()).collect();
    idx.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    let total: f64 = weights.iter().sum();
    let mut acc = 0.0;
    for i in idx {
        acc += weights[i];
        if acc >= fraction * total {
            // Just above this value, so it lands in the lower set.
            return values[i] + 1e-12;
        }
    }
    values.iter().copied().fold(f64::MIN, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Preset;

    fn earth(grid: &GeodesicGrid, seed: u64) -> Terrain {
        Terrain::generate(grid, &Preset::EarthModern.params(), seed)
    }

    #[test]
    fn ocean_fraction_matches_the_request() {
        let grid = GeodesicGrid::new(24);
        for preset in [Preset::EarthModern, Preset::OceanWorld, Preset::ColdDesert] {
            let p = preset.params();
            let t = Terrain::generate(&grid, &p, 3);
            let f = t.ocean_fraction(&grid);
            // Small isolated basins become land, so allow a little shortfall.
            assert!(
                f <= p.terrain.ocean_fraction + 0.01 && f > p.terrain.ocean_fraction - 0.05,
                "{}: {f}",
                preset.name()
            );
        }
    }

    #[test]
    fn deterministic_per_seed() {
        let grid = GeodesicGrid::new(12);
        assert_eq!(earth(&grid, 5).elevation, earth(&grid, 5).elevation);
        assert_ne!(earth(&grid, 5).elevation, earth(&grid, 6).elevation);
    }

    #[test]
    fn relief_is_plausible_and_scales_with_gravity() {
        let grid = GeodesicGrid::new(24);
        let e = earth(&grid, 1);
        let max = e.elevation.iter().copied().fold(f64::MIN, f64::max);
        let min = e.elevation.iter().copied().fold(f64::MAX, f64::min);
        assert!((5_000.0..=6_600.0).contains(&max), "{max}");
        assert!((-4_100.0..=-3_000.0).contains(&min), "{min}");
        let heavy = Terrain::generate(&grid, &Preset::SuperEarth.params(), 1);
        let heavy_max = heavy.elevation.iter().copied().fold(f64::MIN, f64::max);
        assert!(heavy_max < max * 0.7, "{heavy_max} vs {max}");
    }

    #[test]
    fn vents_only_in_the_ocean() {
        let grid = GeodesicGrid::new(24);
        let t = earth(&grid, 2);
        assert!(t.vents.iter().any(|&v| v > 0.0), "no vents at all");
        for c in grid.cells() {
            if t.vents[c as usize] > 0.0 {
                assert!(t.is_ocean(c));
            }
        }
    }

    #[test]
    fn raising_the_sea_floods_land() {
        let grid = GeodesicGrid::new(16);
        let mut t = earth(&grid, 1);
        let before = t.ocean_fraction(&grid);
        t.set_sea_level(&grid, 500.0);
        assert!(t.ocean_fraction(&grid) > before);
    }

    #[test]
    fn brushes_shape_the_ground() {
        let grid = GeodesicGrid::new(24);
        let mut t = earth(&grid, 1);
        // Pick a deep ocean cell and raise an island there.
        let c = (0..grid.cell_count() as CellId)
            .min_by(|&a, &b| t.elevation[a as usize].total_cmp(&t.elevation[b as usize]))
            .unwrap();
        let centre = grid.position(c);
        let radius = 0.08;
        let changed = t.apply_brush(
            &grid,
            &Brush {
                mode: BrushMode::Raise,
                centre,
                radius,
                amount: 9_000.0,
            },
        );
        assert!(!changed.is_empty());
        assert!(
            t.elevation[c as usize] > 0.0 && !t.is_ocean(c),
            "island did not rise"
        );
        // Cells outside the radius are untouched.
        for o in grid.cells() {
            if grid.position(o).dot(centre) < radius.cos() - 1e-9 {
                assert!(!changed.contains(&o));
            }
        }
        // Smoothing reduces local roughness.
        let rough = |t: &Terrain| -> f64 {
            changed
                .iter()
                .map(|&c| {
                    let nbs = grid.neighbours(c);
                    let mean = nbs.iter().map(|&n| t.elevation[n as usize]).sum::<f64>()
                        / nbs.len() as f64;
                    (t.elevation[c as usize] - mean).abs()
                })
                .sum()
        };
        let r0 = rough(&t);
        t.apply_brush(
            &grid,
            &Brush {
                mode: BrushMode::Smooth,
                centre,
                radius,
                amount: 1.0,
            },
        );
        assert!(rough(&t) < r0);
        // Flatten pulls the centre to the target.
        t.apply_brush(
            &grid,
            &Brush {
                mode: BrushMode::Flatten { target: 100.0 },
                centre,
                radius,
                amount: 1.0,
            },
        );
        assert!((t.elevation[c as usize] - 100.0).abs() < 1.0);
    }
}
