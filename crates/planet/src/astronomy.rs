//! Where the star is in the sky: orbital position, seasons and insolation.
//!
//! Planet time t = 0 is the northern spring equinox. Seasons come from
//! geometry alone: the solar longitude λ advances along a Keplerian orbit, the
//! declination is sin δ = sin ε · sin λ, and the planet turns underneath. The
//! sub-solar point (where the star is overhead) follows from both.

use std::f64::consts::{PI, TAU};

use dirtbox_core::Vec3;
use dirtbox_core::consts::AU;

use crate::params::PlanetParams;

#[derive(Clone, Copy, Debug)]
pub struct SunState {
    /// Distance to the star, m.
    pub distance: f64,
    /// Top-of-atmosphere flux at this distance, W m⁻².
    pub flux: f64,
    /// Solar longitude λ (radians, 0 = northern spring equinox).
    pub solar_longitude: f64,
    /// Declination δ (radians): latitude where the star is overhead at noon.
    pub declination: f64,
    /// Unit vector toward the star in the planet's rotating frame.
    pub direction: Vec3,
}

/// Solves Kepler's equation E − e·sin E = M for the eccentric anomaly.
pub fn eccentric_anomaly(mean_anomaly: f64, e: f64) -> f64 {
    let m = mean_anomaly.rem_euclid(TAU);
    let mut ea = if e < 0.8 { m } else { PI };
    for _ in 0..50 {
        let f = ea - e * ea.sin() - m;
        let step = f / (1.0 - e * ea.cos());
        ea -= step;
        if step.abs() < 1e-13 {
            break;
        }
    }
    ea
}

fn true_from_eccentric(ea: f64, e: f64) -> f64 {
    2.0 * ((1.0 + e).sqrt() * (ea / 2.0).sin()).atan2((1.0 - e).sqrt() * (ea / 2.0).cos())
}

fn eccentric_from_true(nu: f64, e: f64) -> f64 {
    2.0 * ((1.0 - e).sqrt() * (nu / 2.0).sin()).atan2((1.0 + e).sqrt() * (nu / 2.0).cos())
}

impl PlanetParams {
    /// Mean anomaly at t = 0, chosen so t = 0 is the northern spring equinox.
    fn mean_anomaly_at_equinox(&self) -> f64 {
        let e = self.orbit.eccentricity;
        // λ = ν + ϖ, so the equinox (λ = 0) is at ν = −ϖ.
        let ea = eccentric_from_true(-self.orbit.perihelion_longitude, e);
        ea - e * ea.sin()
    }

    /// Rotation angle of the planet at t = 0. For a spinning planet, 0°
    /// longitude starts at local midnight. For a tidally locked planet, the
    /// star sits over 0° longitude.
    fn rotation_angle_at_start(&self) -> f64 {
        if self.rotation.tidally_locked {
            self.mean_anomaly_at_equinox() + self.orbit.perihelion_longitude
        } else {
            PI
        }
    }

    pub fn sun_state(&self, planet_seconds: f64) -> SunState {
        let e = self.orbit.eccentricity;
        let n = TAU / self.year_seconds();
        let m = self.mean_anomaly_at_equinox() + n * planet_seconds;
        let ea = eccentric_anomaly(m, e);
        let nu = true_from_eccentric(ea, e);
        let distance = self.orbit.semi_major_axis * AU * (1.0 - e * ea.cos());
        let lambda = (nu + self.orbit.perihelion_longitude).rem_euclid(TAU);
        let eps = self.orbit.obliquity;

        // Star direction in the planet's equatorial (non-rotating) frame.
        let (sl, cl) = lambda.sin_cos();
        let inertial = Vec3::new(cl, eps.cos() * sl, eps.sin() * sl);
        // Rotate into the planet's rotating frame.
        let theta = self.rotation_angle_at_start() + self.rotation_rate() * planet_seconds;
        let (st, ct) = theta.sin_cos();
        let direction = Vec3::new(
            inertial.x * ct + inertial.y * st,
            -inertial.x * st + inertial.y * ct,
            inertial.z,
        );

        SunState {
            distance,
            flux: self.star.luminosity() / (4.0 * PI * distance * distance),
            solar_longitude: lambda,
            declination: inertial.z.clamp(-1.0, 1.0).asin(),
            direction,
        }
    }
}

/// Instantaneous top-of-atmosphere insolation (W m⁻²) at a surface point.
pub fn instantaneous_insolation(sun: &SunState, point: Vec3) -> f64 {
    sun.flux * point.dot(sun.direction).max(0.0)
}

/// Insolation averaged over one full rotation (W m⁻²) at a latitude, for a
/// spinning planet:
/// Q = (S/π)(h₀ sin φ sin δ + cos φ cos δ sin h₀), with cos h₀ = −tan φ tan δ.
pub fn daily_mean_insolation(flux: f64, latitude: f64, declination: f64) -> f64 {
    let x = -latitude.tan() * declination.tan();
    let h0 = if x >= 1.0 {
        0.0 // polar night
    } else if x <= -1.0 {
        PI // polar day
    } else {
        x.acos()
    };
    flux / PI
        * (h0 * latitude.sin() * declination.sin() + latitude.cos() * declination.cos() * h0.sin())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Preset;

    fn deg(x: f64) -> f64 {
        x.to_degrees()
    }

    #[test]
    fn kepler_solver_satisfies_the_equation() {
        for &e in &[0.0, 0.0167, 0.3, 0.9] {
            for k in 0..20 {
                let m = k as f64 * 0.37;
                let ea = eccentric_anomaly(m, e);
                assert!((ea - e * ea.sin() - m.rem_euclid(TAU)).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn earth_seasons() {
        let p = Preset::EarthModern.params();
        let year = p.year_seconds();
        let at = |frac: f64| p.sun_state(frac * year);
        // Equinox at t = 0, northern summer solstice about a quarter year later.
        assert!(deg(at(0.0).declination).abs() < 1e-6);
        let (best, decl) = (0..365)
            .map(|d| (d, at(d as f64 / 365.25).declination))
            .fold((0, f64::MIN), |a, b| if b.1 > a.1 { b } else { a });
        assert!((deg(decl) - 23.44).abs() < 0.01, "{}", deg(decl));
        assert!((85..100).contains(&best), "solstice on day {best}");
        // Perihelion in northern winter: closest in early January (~day 288 after the March equinox).
        let closest = (0..365)
            .min_by(|&a, &b| {
                at(a as f64 / 365.25)
                    .distance
                    .total_cmp(&at(b as f64 / 365.25).distance)
            })
            .unwrap();
        assert!((280..295).contains(&closest), "perihelion on day {closest}");
    }

    #[test]
    fn sun_rises_in_the_east() {
        // Over an hour, the sub-solar point moves west (longitude decreases).
        let p = Preset::EarthModern.params();
        let lon = |t: f64| p.sun_state(t).direction.longitude();
        let d = (lon(3600.0) - lon(0.0) + PI).rem_euclid(TAU) - PI;
        assert!((d.to_degrees() + 15.0).abs() < 0.2, "{}", d.to_degrees());
    }

    #[test]
    fn global_mean_insolation_is_a_quarter_of_the_flux() {
        let p = Preset::EarthModern.params();
        let grid = dirtbox_grid::GeodesicGrid::new(24);
        for t in [0.0, 1e6, 1.3e7] {
            let sun = p.sun_state(t);
            let total: f64 = grid
                .cells()
                .map(|c| instantaneous_insolation(&sun, grid.position(c)) * grid.area(c))
                .sum();
            let mean = total / (4.0 * PI);
            assert!(
                (mean / sun.flux - 0.25).abs() < 0.002,
                "{}",
                mean / sun.flux
            );
        }
    }

    #[test]
    fn daily_mean_matches_known_values() {
        let s = 1361.0;
        // Equator at equinox: S/π.
        assert!((daily_mean_insolation(s, 0.0, 0.0) - s / PI).abs() < 1e-9);
        // Polar night and polar day.
        let d = 23.44f64.to_radians();
        assert_eq!(daily_mean_insolation(s, -80f64.to_radians(), d), 0.0);
        let pole_summer = daily_mean_insolation(s, 90f64.to_radians() - 1e-9, d);
        assert!((pole_summer - s * d.sin()).abs() < 1e-3);
        // Daily mean equals the average of instantaneous values around a latitude circle.
        let lat = 40f64.to_radians();
        let sun = SunState {
            distance: AU,
            flux: s,
            solar_longitude: 0.0,
            declination: d,
            direction: Vec3::from_lat_lon(d, 0.0),
        };
        let n = 3600;
        let avg: f64 = (0..n)
            .map(|k| {
                instantaneous_insolation(&sun, Vec3::from_lat_lon(lat, TAU * k as f64 / n as f64))
            })
            .sum::<f64>()
            / n as f64;
        assert!(
            (avg - daily_mean_insolation(s, lat, d)).abs() < 0.5,
            "{avg}"
        );
    }

    #[test]
    fn tidally_locked_star_stays_put() {
        let p = Preset::TidallyLocked.params();
        let a = p.sun_state(0.0).direction;
        let b = p.sun_state(p.year_seconds() * 0.37).direction;
        assert!(a.dot(b) > 0.999, "star wandered: {}", a.dot(b));
        assert!(a.longitude().abs() < 0.05);
    }
}
