//! Horizontal transport on the geodesic grid: implicit diffusion (solved by
//! preconditioned conjugate gradients, stable for any step) and upwind
//! advection by the wind.
//!
//! The kernels run in parallel but stay bit-for-bit deterministic: each cell
//! computes its own update from read-only inputs, edge quantities are
//! evaluated once per edge (so the two sides see exactly opposite fluxes), and
//! sums are reduced in fixed-size chunks in a fixed order.

use rayon::prelude::*;

use dirtbox_core::Vec3;
use dirtbox_grid::GeodesicGrid;

/// Chunk size for deterministic parallel reductions.
const CHUNK: usize = 4096;

/// Grid geometry in physical units for one planet radius.
pub struct Geometry {
    /// Cell areas, m².
    pub area: Vec<f64>,
    /// CSR offsets: cell c owns entries `offsets[c]..offsets[c + 1]`.
    pub offsets: Vec<usize>,
    /// Per neighbour entry: the neighbour's index.
    pub neighbour: Vec<usize>,
    /// Per neighbour entry: index of the same edge seen from the neighbour.
    pub reverse: Vec<usize>,
    /// Per neighbour entry: shared edge length, m.
    pub edge_length: Vec<f64>,
    /// Per neighbour entry: edge length ÷ centre distance (dimensionless).
    pub conductance: Vec<f64>,
    pub min_area: f64,
}

impl Geometry {
    pub fn new(grid: &GeodesicGrid, radius: f64) -> Self {
        let n = grid.cell_count();
        let area: Vec<f64> = grid
            .cells()
            .map(|c| grid.area(c) * radius * radius)
            .collect();
        let mut offsets = Vec::with_capacity(n + 1);
        offsets.push(0);
        for c in grid.cells() {
            offsets.push(grid.neighbour_range(c).end);
        }
        let neighbour: Vec<usize> = grid.neighbour_ids().iter().map(|&j| j as usize).collect();
        let reverse = (0..n)
            .flat_map(|i| (offsets[i]..offsets[i + 1]).map(move |e| (i, e)))
            .map(|(i, e)| {
                let j = neighbour[e];
                (offsets[j]..offsets[j + 1])
                    .find(|&r| neighbour[r] == i)
                    .expect("asymmetric neighbours")
            })
            .collect();
        let edge_length = grid.edge_lengths().iter().map(|l| l * radius).collect();
        let conductance = grid
            .edge_lengths()
            .iter()
            .zip(grid.centre_distances())
            .map(|(l, d)| l / d)
            .collect();
        let min_area = area.iter().copied().fold(f64::MAX, f64::min);
        Self {
            area,
            offsets,
            neighbour,
            reverse,
            edge_length,
            conductance,
            min_area,
        }
    }

    pub fn cell_count(&self) -> usize {
        self.area.len()
    }

    pub fn edges(&self, cell: usize) -> std::ops::Range<usize> {
        self.offsets[cell]..self.offsets[cell + 1]
    }
}

/// Grids smaller than this run single-threaded: below it, thread hand-offs
/// cost more than they save.
const PARALLEL_MIN: usize = 32_768;

/// Sets `out[i] = f(i)`, in parallel for large arrays.
fn fill(out: &mut [f64], f: impl Fn(usize) -> f64 + Sync) {
    if out.len() >= PARALLEL_MIN {
        out.par_chunks_mut(CHUNK)
            .enumerate()
            .for_each(|(c, chunk)| {
                for (k, v) in chunk.iter_mut().enumerate() {
                    *v = f(c * CHUNK + k);
                }
            });
    } else {
        for (i, v) in out.iter_mut().enumerate() {
            *v = f(i);
        }
    }
}

/// Sets `x[i] = f(i, x[i])`, in parallel for large arrays.
fn update(x: &mut [f64], f: impl Fn(usize, f64) -> f64 + Sync) {
    if x.len() >= PARALLEL_MIN {
        x.par_chunks_mut(CHUNK).enumerate().for_each(|(c, chunk)| {
            for (k, v) in chunk.iter_mut().enumerate() {
                *v = f(c * CHUNK + k, *v);
            }
        });
    } else {
        for (i, v) in x.iter_mut().enumerate() {
            *v = f(i, *v);
        }
    }
}

fn collect(n: usize, f: impl Fn(usize) -> f64 + Sync) -> Vec<f64> {
    let mut out = vec![0.0; n];
    fill(&mut out, f);
    out
}

/// Σ f(i), summed per fixed chunk and then in chunk order, so the result is
/// identical whether or not it ran in parallel.
fn sum(n: usize, f: impl Fn(usize) -> f64 + Sync) -> f64 {
    let chunk_sum = |c: usize| (c * CHUNK..((c + 1) * CHUNK).min(n)).map(&f).sum::<f64>();
    let chunks = n.div_ceil(CHUNK);
    let partial: Vec<f64> = if n >= PARALLEL_MIN {
        (0..chunks).into_par_iter().map(chunk_sum).collect()
    } else {
        (0..chunks).map(chunk_sum).collect()
    };
    partial.iter().sum()
}

/// Solves one implicit diffusion step for a field `x` with per-cell capacity
/// per area `capacity` and conductance `k` (per unit edge-length/distance
/// ratio). The solve stops at a modest tolerance; whatever imbalance remains
/// is returned evenly, so Σ capacity·area·x is conserved to rounding.
///
/// (C_i A_i/dt) x'_i + Σ_j K_ij (x'_i − x'_j) = (C_i A_i/dt) x_i
pub fn diffuse_implicit(geo: &Geometry, x: &mut [f64], capacity: &[f64], k: f64, dt: f64) -> usize {
    diffuse_implicit_limited(geo, x, capacity, k, dt, 1000)
}

/// [`diffuse_implicit`] with a cap on solver iterations (conservation stays exact).
pub fn diffuse_implicit_limited(
    geo: &Geometry,
    x: &mut [f64],
    capacity: &[f64],
    k: f64,
    dt: f64,
    max_iterations: usize,
) -> usize {
    let n = x.len();
    let mass = collect(n, |i| capacity[i] * geo.area[i] / dt);
    let diag = collect(n, |i| {
        mass[i] + geo.edges(i).map(|e| k * geo.conductance[e]).sum::<f64>()
    });
    let apply = |v: &[f64], i: usize| {
        let mut s = diag[i] * v[i];
        for e in geo.edges(i) {
            s -= k * geo.conductance[e] * v[geo.neighbour[e]];
        }
        s
    };

    let b = collect(n, |i| mass[i] * x[i]);
    let mut r = collect(n, |i| b[i] - apply(x, i));
    let mut p = collect(n, |i| r[i] / diag[i]);
    let mut rz = sum(n, |i| r[i] * r[i] / diag[i]);
    let tolerance = 1e-6 * sum(n, |i| b[i] * b[i]).sqrt().max(1e-300);
    let mut ap = vec![0.0; n];
    let mut iterations = 0;
    while iterations < max_iterations && sum(n, |i| r[i] * r[i]).sqrt() > tolerance {
        iterations += 1;
        fill(&mut ap, |i| apply(&p, i));
        let pap = sum(n, |i| p[i] * ap[i]);
        if pap <= 0.0 {
            break;
        }
        let alpha = rz / pap;
        update(x, |i, v| v + alpha * p[i]);
        update(&mut r, |i, v| v - alpha * ap[i]);
        let rz_new = sum(n, |i| r[i] * r[i] / diag[i]);
        let beta = rz_new / rz;
        rz = rz_new;
        update(&mut p, |i, v| r[i] / diag[i] + beta * v);
    }
    // Σ(Ax − b) equals the change in Σ mass·x; remove it with a uniform shift.
    let imbalance = sum(n, |i| apply(x, i) - b[i]);
    let shift = imbalance / sum(n, |i| mass[i]);
    update(x, |_, v| v - shift);
    iterations
}

/// Smooths a per-area field over a physical length scale `length` (m) by
/// solving (1 − ℓ²∇²) x' = x. The area-weighted total Σ x·A is conserved, and
/// the result doesn't depend on grid resolution.
pub fn smooth(geo: &Geometry, x: &mut [f64], length: f64) {
    let unit = vec![1.0; x.len()];
    // An approximate smoothing is all that's needed; a few iterations get the
    // shape right, and the final correction keeps the total exact.
    diffuse_implicit_limited(geo, x, &unit, 1.0, length * length, 20);
}

/// Velocity normal to each edge (m s⁻¹, positive from the cell toward its
/// neighbour), averaging the two cells' winds. Exactly antisymmetric: the
/// entry seen from the neighbour is the negative.
pub fn edge_velocities(grid: &GeodesicGrid, geo: &Geometry, wind: &[Vec3]) -> Vec<f64> {
    let normals = grid.edge_normals();
    (0..geo.cell_count())
        .into_par_iter()
        .with_min_len(PARALLEL_MIN)
        .flat_map_iter(|i| {
            let p = grid.position(i as u32);
            geo.edges(i).map(move |e| {
                let j = geo.neighbour[e];
                // Compute each edge once, from its lower-indexed cell.
                let (a, b, edge) = if i < j {
                    (i, j, e)
                } else {
                    (j, i, geo.reverse[e])
                };
                let pa = if a == i { p } else { grid.position(a as u32) };
                let wb = wind[b] - pa * pa.dot(wind[b]);
                let v = (wind[a] + wb).dot(normals[edge]) * 0.5;
                if a == i { v } else { -v }
            })
        })
        .collect()
}

/// Number of substeps that keeps upwind advection stable (Courant number ≤ 0.5).
pub fn advection_substeps(geo: &Geometry, un: &[f64], dt: f64) -> usize {
    let worst = (0..geo.cell_count())
        .map(|i| {
            geo.edges(i)
                .map(|e| un[e].max(0.0) * geo.edge_length[e])
                .sum::<f64>()
                * dt
                / geo.area[i]
        })
        .fold(0.0, f64::max);
    ((worst / 0.5).ceil() as usize).max(1)
}

/// One conservative (flux-form) upwind step for a column quantity `q` (per
/// m²): what leaves one cell enters its neighbour, so Σ q·A is exact.
pub fn advect_flux_form(geo: &Geometry, un: &[f64], q: &mut [f64], dt: f64) {
    let old = q.to_vec();
    fill(q, |i| {
        let mut net = 0.0;
        for e in geo.edges(i) {
            let j = geo.neighbour[e];
            let u = un[e];
            // Outflow carries this cell's value, inflow the neighbour's.
            net -= if u > 0.0 { u * old[i] } else { u * old[j] } * geo.edge_length[e];
        }
        old[i] + net * dt / geo.area[i]
    });
}

/// One advective-form upwind step: each cell moves toward the value carried in
/// from upwind, scaled by `coupling` (0–1) per cell. A uniform field stays
/// uniform even where the wind converges.
pub fn advect_advective_form(geo: &Geometry, un: &[f64], q: &mut [f64], coupling: &[f64], dt: f64) {
    let old = q.to_vec();
    fill(q, |i| {
        let mut change = 0.0;
        for e in geo.edges(i) {
            if un[e] < 0.0 {
                change += -un[e] * geo.edge_length[e] * (old[geo.neighbour[e]] - old[i]);
            }
        }
        old[i] + coupling[i] * change * dt / geo.area[i]
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirtbox_core::Rng;

    fn total(geo: &Geometry, cap: &[f64], x: &[f64]) -> f64 {
        x.iter()
            .zip(cap)
            .zip(&geo.area)
            .map(|((x, c), a)| x * c * a)
            .sum()
    }

    #[test]
    fn implicit_diffusion_conserves_and_smooths() {
        let grid = GeodesicGrid::new(12);
        let geo = Geometry::new(&grid, 6.371e6);
        let mut rng = Rng::from_seed(1);
        let mut x: Vec<f64> = grid.cells().map(|_| 280.0 + 20.0 * rng.normal()).collect();
        let cap: Vec<f64> = grid
            .cells()
            .map(|c| if c % 3 == 0 { 2e8 } else { 3e6 })
            .collect();
        let e0 = total(&geo, &cap, &x);
        let var0 = x.iter().map(|v| (v - 280.0).powi(2)).sum::<f64>();
        // A full day in one step: explicit would be wildly unstable here.
        let iters = diffuse_implicit(&geo, &mut x, &cap, 2e13, 86_400.0);
        let e1 = total(&geo, &cap, &x);
        assert!(
            ((e1 - e0) / e0).abs() < 1e-13,
            "energy drift {}",
            (e1 - e0) / e0
        );
        let var1 = x.iter().map(|v| (v - 280.0).powi(2)).sum::<f64>();
        assert!(var1 < var0 * 0.5);
        assert!(x.iter().all(|v| v.is_finite()));
        assert!(iters < 200, "{iters} iterations");
    }

    #[test]
    fn smoothing_is_conservative_and_resolution_independent() {
        let radius = 6.371e6;
        let spread = |n: u32| {
            let grid = GeodesicGrid::new(n);
            let geo = Geometry::new(&grid, radius);
            let spike = grid.locate(Vec3::new(1.0, 0.0, 0.0)) as usize;
            let mut x = vec![0.0; grid.cell_count()];
            x[spike] = 1.0 / geo.area[spike];
            smooth(&geo, &mut x, 400e3);
            let total: f64 = x.iter().zip(&geo.area).map(|(x, a)| x * a).sum();
            assert!((total - 1.0).abs() < 1e-9);
            // Fraction of the mass within 400 km of the spike.
            grid.cells()
                .filter(|&c| {
                    grid.position(c).dot(Vec3::new(1.0, 0.0, 0.0)) > (400e3 / radius).cos()
                })
                .map(|c| x[c as usize] * geo.area[c as usize])
                .sum::<f64>()
        };
        let (coarse, fine) = (spread(24), spread(64));
        assert!((coarse - fine).abs() < 0.12, "{coarse} vs {fine}");
    }

    #[test]
    fn flux_form_conserves_mass_and_moves_downwind() {
        let grid = GeodesicGrid::new(12);
        let geo = Geometry::new(&grid, 6.371e6);
        // Solid-body eastward rotation at ~10 m/s at the equator.
        let wind: Vec<Vec3> = grid
            .cells()
            .map(|c| grid.east_north(c).0 * (10.0 * grid.latitude(c).cos()))
            .collect();
        let un = edge_velocities(&grid, &geo, &wind);
        let mut q: Vec<f64> = grid
            .cells()
            .map(|c| (grid.longitude(c).cos().max(0.0)) * 30.0)
            .collect();
        let m0: f64 = q.iter().zip(&geo.area).map(|(q, a)| q * a).sum();
        let x_centroid = |q: &[f64]| {
            let s: Vec3 = grid.cells().fold(Vec3::ZERO, |acc, c| {
                acc + grid.position(c) * (q[c as usize] * grid.area(c))
            });
            s.longitude()
        };
        let before = x_centroid(&q);
        let dt = 3600.0;
        let sub = advection_substeps(&geo, &un, dt);
        for _ in 0..(24 * sub) {
            advect_flux_form(&geo, &un, &mut q, dt / sub as f64);
        }
        let m1: f64 = q.iter().zip(&geo.area).map(|(q, a)| q * a).sum();
        assert!(((m1 - m0) / m0).abs() < 1e-12);
        assert!(
            q.iter().all(|&v| v >= -1e-9),
            "upwind must not create negatives"
        );
        assert!(x_centroid(&q) > before, "blob should drift east");
    }

    #[test]
    fn edge_velocities_are_antisymmetric() {
        let grid = GeodesicGrid::new(8);
        let geo = Geometry::new(&grid, 6.371e6);
        let mut rng = Rng::from_seed(4);
        let wind: Vec<Vec3> = grid
            .cells()
            .map(|c| {
                let (e, n) = grid.east_north(c);
                e * rng.normal() + n * rng.normal()
            })
            .collect();
        let un = edge_velocities(&grid, &geo, &wind);
        for e in 0..un.len() {
            assert_eq!(un[e], -un[geo.reverse[e]]);
        }
    }

    #[test]
    fn advective_form_keeps_uniform_fields_uniform() {
        let grid = GeodesicGrid::new(10);
        let geo = Geometry::new(&grid, 6.371e6);
        // A strongly convergent wind toward the equator.
        let wind: Vec<Vec3> = grid
            .cells()
            .map(|c| grid.east_north(c).1 * (-5.0 * grid.latitude(c).sin()))
            .collect();
        let un = edge_velocities(&grid, &geo, &wind);
        let mut q = vec![290.0; grid.cell_count()];
        let coupling = vec![1.0; grid.cell_count()];
        advect_advective_form(&geo, &un, &mut q, &coupling, 600.0);
        assert!(q.iter().all(|&v| (v - 290.0).abs() < 1e-9));
    }
}
