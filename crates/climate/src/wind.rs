//! Surface winds and vertical motion from a circulation-cell model.
//!
//! Instead of solving fluid dynamics, the large-scale overturning cells are
//! derived from the planet's physics:
//!
//! - The width of the tropical (Hadley) cell scales as in Held & Hou (1980):
//!   φ_H ∝ √ΔT / (Ω a). It is 30° for Earth, wider for slow or small
//!   planets, and narrower for fast spinners, which then get more bands.
//! - The rest of each hemisphere is split into alternating indirect and
//!   direct cells of similar width (Earth: Ferrel + polar).
//! - The pattern is centred on the thermal equator, so the rain belt (ITCZ)
//!   follows the sun's seasons.
//! - Surface flow toward the equator is turned westward by Coriolis (trade
//!   winds); flow toward the pole is turned eastward (westerlies).
//! - Air rises where surface flows converge (rainy) and sinks where they
//!   diverge (dry: the subtropical deserts).
//!
//! On top of that, temperature contrasts drive local breezes (monsoons), and
//! wind blowing uphill is lifted (orographic rain and rain shadows).
//! Tidally locked worlds instead get flow from the night side toward the
//! point under the star, where air rises.

use dirtbox_core::Vec3;
use dirtbox_core::consts::{EARTH_RADIUS, EARTH_ROTATION_RATE};
use dirtbox_grid::GeodesicGrid;

/// Per-cell wind output.
pub struct WindField {
    /// Surface wind, m s⁻¹, as a 3D tangent vector.
    pub wind: Vec<Vec3>,
    /// Large-scale vertical motion index: +1 strong ascent, −1 strong descent.
    pub ascent: Vec<f64>,
    /// Lift from blowing uphill (positive) or downhill (negative).
    pub orographic: Vec<f64>,
}

pub struct CirculationInputs<'a> {
    pub grid: &'a GeodesicGrid,
    pub geometry: &'a crate::transport::Geometry,
    pub radius: f64,
    pub rotation_rate: f64,
    pub tidally_locked: bool,
    /// Unit vector toward the star (used for tidally locked worlds).
    pub sun_direction: Vec3,
    pub obliquity: f64,
    /// Sea-level-equivalent temperature per cell, K.
    pub theta: &'a [f64],
    /// Height above sea level per cell (0 over ocean), m.
    pub land_height: &'a [f64],
    pub speed_scale: f64,
}

/// Summary of the circulation, for display and tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct CirculationSummary {
    pub thermal_equator: f64,
    pub hadley_extent: f64,
    pub cells_per_hemisphere: usize,
    pub equator_pole_contrast: f64,
}

const LAT_BINS: usize = 90;
/// Length scale of the temperature contrasts that drive local breezes, m.
const THERMAL_LENGTH: f64 = 400e3;
/// Safety cap on surface wind, m s⁻¹.
const MAX_WIND: f64 = 30.0;

pub fn compute(inputs: &CirculationInputs) -> (WindField, CirculationSummary) {
    let grid = inputs.grid;
    let n = grid.cell_count();
    let (profile, contrast) = zonal_profile(grid, inputs.theta);

    let mut summary = CirculationSummary {
        equator_pole_contrast: contrast,
        ..Default::default()
    };
    let mut wind = vec![Vec3::ZERO; n];
    let mut ascent = vec![0.0; n];

    // Overall wind strength grows with the temperature contrast that drives it.
    let strength = (contrast.max(1.0) / 40.0).sqrt().clamp(0.3, 2.0) * inputs.speed_scale;

    if inputs.tidally_locked {
        let s = inputs.sun_direction;
        for c in grid.cells() {
            let p = grid.position(c);
            let cos_angle = p.dot(s).clamp(-1.0, 1.0);
            let toward = s - p * cos_angle;
            let sin_angle = toward.length();
            if sin_angle > 1e-9 {
                wind[c as usize] = toward * (8.0 * strength / sin_angle * sin_angle.min(1.0));
            }
            ascent[c as usize] = cos_angle;
        }
    } else {
        let max_shift = (inputs.obliquity.to_degrees() + 8.0).min(45.0);
        let thermal_equator = thermal_equator(&profile, max_shift);
        let speed_ratio = (EARTH_ROTATION_RATE * EARTH_RADIUS)
            / (inputs.rotation_rate.abs() * inputs.radius).max(1.0);
        let hadley = (30.0 * speed_ratio * (contrast.max(1.0) / 40.0).sqrt()).clamp(8.0, 90.0);
        let rest = 90.0 - hadley;
        let others = if rest < 5.0 {
            0
        } else {
            ((rest / hadley).round() as usize).max(1)
        };
        summary.thermal_equator = thermal_equator;
        summary.hadley_extent = hadley;
        summary.cells_per_hemisphere = 1 + others;

        for c in grid.cells() {
            let p = grid.position(c);
            let lat = p.latitude().to_degrees();
            let (east, north) = grid.east_north(c);
            // Distance from the thermal equator, stretched so each side spans 0–90.
            let (s, poleward_sign) = if lat >= thermal_equator {
                (
                    (lat - thermal_equator) / (90.0 - thermal_equator) * 90.0,
                    1.0,
                )
            } else {
                (
                    (thermal_equator - lat) / (90.0 + thermal_equator) * 90.0,
                    -1.0,
                )
            };
            let (band, x) = if s <= hadley || others == 0 {
                (0usize, s / hadley.max(1e-9))
            } else {
                let w = rest / others as f64;
                let k = (((s - hadley) / w) as usize).min(others - 1);
                (k + 1, (s - hadley - k as f64 * w) / w)
            };
            let x = x.clamp(0.0, 1.0);
            // Even bands (Hadley, polar…) are thermally direct: surface flow
            // toward the equator side. Odd bands (Ferrel…) are indirect.
            let direct = band % 2 == 0;
            let shape = (std::f64::consts::PI * x).sin();
            let equatorward = if direct { 1.0 } else { -1.0 };
            let v_north = -poleward_sign * equatorward * 1.5 * strength * shape;
            let u_east = -equatorward * 6.0 * strength * shape;
            wind[c as usize] = east * u_east + north * v_north;
            ascent[c as usize] = (std::f64::consts::PI * x).cos() * if direct { 1.0 } else { -1.0 };
        }
    }

    // Local thermal circulation: air flows toward warmer-than-usual regions
    // (land in summer, sea in winter) and rises over them.
    // Only large-scale contrasts (hundreds of km) drive these breezes, so the
    // anomaly is smoothed over a fixed physical length, independent of the grid.
    let mut anomaly: Vec<f64> = grid
        .cells()
        .map(|c| inputs.theta[c as usize] - profile[lat_bin(grid.latitude(c))])
        .collect();
    crate::transport::smooth(inputs.geometry, &mut anomaly, THERMAL_LENGTH);
    let grad_t = gradient(grid, inputs.radius, &anomaly);
    let grad_h = gradient(grid, inputs.radius, inputs.land_height);
    let mut orographic = vec![0.0; n];
    for i in 0..n {
        wind[i] += grad_t[i] * (8.0e4 * inputs.speed_scale);
        ascent[i] = (ascent[i] + 0.03 * anomaly[i]).clamp(-1.5, 1.5);
        // Vertical velocity from flow up a slope: w = u·∇h (m/s), normalised.
        orographic[i] = (wind[i].dot(grad_h[i]) / 0.06).clamp(-1.5, 1.5);
        let speed = wind[i].length();
        if speed > MAX_WIND {
            wind[i] = wind[i] * (MAX_WIND / speed);
        }
    }

    (
        WindField {
            wind,
            ascent,
            orographic,
        },
        summary,
    )
}

fn lat_bin(lat: f64) -> usize {
    let x = (lat.to_degrees() + 90.0) / 180.0 * LAT_BINS as f64;
    (x as usize).min(LAT_BINS - 1)
}

/// Area-weighted zonal mean of `theta` per latitude bin (gaps filled from
/// neighbours, then smoothed), and the equator-to-pole contrast.
fn zonal_profile(grid: &GeodesicGrid, theta: &[f64]) -> (Vec<f64>, f64) {
    let mut sum = vec![0.0; LAT_BINS];
    let mut weight = vec![0.0; LAT_BINS];
    for c in grid.cells() {
        let b = lat_bin(grid.latitude(c));
        sum[b] += theta[c as usize] * grid.area(c);
        weight[b] += grid.area(c);
    }
    let mut profile: Vec<Option<f64>> = (0..LAT_BINS)
        .map(|b| (weight[b] > 0.0).then(|| sum[b] / weight[b]))
        .collect();
    // Fill any empty bins (coarse grids) from the nearest filled one.
    for b in 0..LAT_BINS {
        if profile[b].is_none() {
            let nearest = (1..LAT_BINS)
                .flat_map(|d| [b.checked_sub(d), Some(b + d).filter(|&x| x < LAT_BINS)])
                .flatten()
                .find_map(|x| profile[x]);
            profile[b] = nearest;
        }
    }
    let raw: Vec<f64> = profile.into_iter().map(|v| v.unwrap_or(250.0)).collect();
    let smooth: Vec<f64> = (0..LAT_BINS)
        .map(|b| {
            let lo = b.saturating_sub(2);
            let hi = (b + 2).min(LAT_BINS - 1);
            raw[lo..=hi].iter().sum::<f64>() / (hi - lo + 1) as f64
        })
        .collect();
    let band_mean = |from: f64, to: f64| {
        let (mut s, mut w) = (0.0, 0.0);
        for c in grid.cells() {
            let lat = grid.latitude(c).to_degrees().abs();
            if lat >= from && lat < to {
                s += theta[c as usize] * grid.area(c);
                w += grid.area(c);
            }
        }
        if w > 0.0 { s / w } else { 0.0 }
    };
    let contrast = band_mean(0.0, 20.0) - band_mean(60.0, 90.1);
    (smooth, contrast)
}

/// Latitude (degrees) of the warmest point of the zonal profile, limited to
/// ±`max_shift` and eased so the rain belt moves smoothly.
fn thermal_equator(profile: &[f64], max_shift: f64) -> f64 {
    let lat_of = |b: usize| -90.0 + (b as f64 + 0.5) * 180.0 / LAT_BINS as f64;
    // Weighted centroid of the warmest part of the profile.
    let max = profile.iter().copied().fold(f64::MIN, f64::max);
    let (mut s, mut w) = (0.0, 0.0);
    for (b, &t) in profile.iter().enumerate() {
        let weight = (t - (max - 3.0)).max(0.0);
        s += lat_of(b) * weight;
        w += weight;
    }
    let centre = if w > 0.0 { s / w } else { 0.0 };
    centre.clamp(-max_shift, max_shift)
}

/// Per-cell gradient of a scalar field (units per metre) as a tangent vector,
/// using the Green–Gauss estimate over each cell's edges.
pub fn gradient(grid: &GeodesicGrid, radius: f64, field: &[f64]) -> Vec<Vec3> {
    let ids = grid.neighbour_ids();
    let lens = grid.edge_lengths();
    let normals = grid.edge_normals();
    grid.cells()
        .map(|c| {
            let i = c as usize;
            let mut g = Vec3::ZERO;
            for k in grid.neighbour_range(c) {
                let j = ids[k] as usize;
                // Face value = mean of the two cells; subtract the cell's own
                // value so a constant field gives exactly zero.
                let face = 0.5 * (field[i] + field[j]) - field[i];
                g += normals[k] * (face * lens[k]);
            }
            g * (1.0 / (grid.area(c) * radius))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirtbox_core::consts::EARTH_ROTATION_RATE;

    fn earthlike_theta(grid: &GeodesicGrid) -> Vec<f64> {
        grid.cells()
            .map(|c| 300.0 - 50.0 * grid.latitude(c).sin().powi(2))
            .collect()
    }

    fn run(grid: &GeodesicGrid, theta: &[f64], rotation: f64) -> (WindField, CirculationSummary) {
        let zeros = vec![0.0; grid.cell_count()];
        let geometry = crate::transport::Geometry::new(grid, EARTH_RADIUS);
        compute(&CirculationInputs {
            grid,
            geometry: &geometry,
            radius: EARTH_RADIUS,
            rotation_rate: rotation,
            tidally_locked: false,
            sun_direction: Vec3::new(1.0, 0.0, 0.0),
            obliquity: 0.4,
            theta,
            land_height: &zeros,
            speed_scale: 1.0,
        })
    }

    #[test]
    fn earth_gets_trades_westerlies_and_three_cells() {
        let grid = GeodesicGrid::new(16);
        let theta = earthlike_theta(&grid);
        let (field, summary) = run(&grid, &theta, EARTH_ROTATION_RATE);
        assert_eq!(summary.cells_per_hemisphere, 3);
        assert!(
            (summary.hadley_extent - 33.0).abs() < 5.0,
            "{}",
            summary.hadley_extent
        );
        let mean_east = |lo: f64, hi: f64| {
            let (mut s, mut n) = (0.0, 0.0);
            for c in grid.cells() {
                let lat = grid.latitude(c).to_degrees().abs();
                if lat > lo && lat < hi {
                    s += field.wind[c as usize].dot(grid.east_north(c).0);
                    n += 1.0;
                }
            }
            s / n
        };
        assert!(
            mean_east(5.0, 25.0) < -2.0,
            "trade winds should blow from the east"
        );
        assert!(
            mean_east(40.0, 55.0) > 2.0,
            "mid-latitudes should have westerlies"
        );
        // Rising air at the equator, sinking in the subtropics.
        let mean_ascent = |lo: f64, hi: f64| {
            let v: Vec<f64> = grid
                .cells()
                .filter(|&c| (lo..hi).contains(&grid.latitude(c).to_degrees().abs()))
                .map(|c| field.ascent[c as usize])
                .collect();
            v.iter().sum::<f64>() / v.len() as f64
        };
        assert!(mean_ascent(0.0, 8.0) > 0.5);
        assert!(mean_ascent(26.0, 36.0) < -0.5);
    }

    #[test]
    fn fast_spinners_get_more_cells() {
        let grid = GeodesicGrid::new(16);
        let theta = earthlike_theta(&grid);
        let (_, slow) = run(&grid, &theta, EARTH_ROTATION_RATE / 10.0);
        let (_, fast) = run(&grid, &theta, EARTH_ROTATION_RATE * 4.0);
        assert_eq!(slow.cells_per_hemisphere, 1);
        assert!(
            fast.cells_per_hemisphere >= 5,
            "{}",
            fast.cells_per_hemisphere
        );
    }

    #[test]
    fn rain_belt_follows_the_warm_hemisphere() {
        let grid = GeodesicGrid::new(16);
        let theta: Vec<f64> = grid
            .cells()
            .map(|c| 300.0 - 50.0 * (grid.latitude(c) - 0.25).sin().powi(2))
            .collect();
        let (_, s) = run(&grid, &theta, EARTH_ROTATION_RATE);
        assert!(s.thermal_equator > 8.0, "{}", s.thermal_equator);
    }

    #[test]
    fn gradient_of_linear_field() {
        let grid = GeodesicGrid::new(20);
        // f = z on the unit sphere; its surface gradient is the tangent part of ẑ.
        let f: Vec<f64> = grid.cells().map(|c| grid.position(c).z).collect();
        let g = gradient(&grid, 1.0, &f);
        for c in grid.cells().step_by(37) {
            let p = grid.position(c);
            let expected = Vec3::new(0.0, 0.0, 1.0) - p * p.z;
            assert!(
                (g[c as usize] - expected).length() < 0.05,
                "{:?} vs {:?}",
                g[c as usize],
                expected
            );
        }
    }
}
