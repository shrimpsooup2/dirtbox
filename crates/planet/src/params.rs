//! The player-editable description of a planetary system (design §3).

use dirtbox_core::consts::{
    AU, BAR, EARTH_GRAVITY, EARTH_RADIUS, G, SOLAR_LUMINOSITY, SOLAR_MASS, SOLAR_TEMPERATURE,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Star {
    /// Mass in solar masses.
    pub mass: f64,
    /// Multiplier on main-sequence luminosity: models a young faint star (< 1),
    /// a brightening one (> 1), or a player-forced ice age or heat wave.
    pub luminosity_factor: f64,
}

impl Star {
    /// Main-sequence luminosity (W) from piecewise mass–luminosity relations.
    pub fn luminosity(&self) -> f64 {
        let m = self.mass;
        let relative = if m < 0.43 {
            0.23 * m.powf(2.3)
        } else if m < 2.0 {
            m.powi(4)
        } else {
            1.4 * m.powf(3.5)
        };
        relative * SOLAR_LUMINOSITY * self.luminosity_factor
    }

    /// Effective temperature (K), using R ∝ M^0.8 for the stellar radius.
    pub fn temperature(&self) -> f64 {
        let relative_luminosity = self.luminosity() / SOLAR_LUMINOSITY;
        let relative_radius = self.mass.powf(0.8);
        SOLAR_TEMPERATURE * (relative_luminosity / relative_radius.powi(2)).powf(0.25)
    }

    /// Fraction of the star's light in the photosynthetically usable band
    /// (400–700 nm), from the Planck spectrum. Phase 2 uses this for
    /// photosynthesis; red dwarfs give much less usable light per watt.
    pub fn par_fraction(&self) -> f64 {
        planck_band_fraction(self.temperature(), 400e-9, 700e-9)
    }

    pub fn gm(&self) -> f64 {
        G * self.mass * SOLAR_MASS
    }
}

/// Fraction of blackbody power between two wavelengths (m).
fn planck_band_fraction(temperature: f64, lo: f64, hi: f64) -> f64 {
    // Integrate B_λ numerically and normalise by σT⁴/π.
    const H: f64 = 6.626_070_15e-34;
    const C: f64 = 2.997_924_58e8;
    const K: f64 = 1.380_649e-23;
    let steps = 200;
    let dl = (hi - lo) / steps as f64;
    let mut sum = 0.0;
    for i in 0..steps {
        let l = lo + (i as f64 + 0.5) * dl;
        sum += 2.0 * H * C * C / l.powi(5) / ((H * C / (l * K * temperature)).exp() - 1.0) * dl;
    }
    sum * std::f64::consts::PI / (dirtbox_core::consts::STEFAN_BOLTZMANN * temperature.powi(4))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Orbit {
    /// Semi-major axis, AU.
    pub semi_major_axis: f64,
    pub eccentricity: f64,
    /// Solar longitude at perihelion (radians): where in the seasonal cycle the
    /// planet is closest to its star. 0 means at the northern spring equinox.
    pub perihelion_longitude: f64,
    /// Axial tilt, radians.
    pub obliquity: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rotation {
    /// Sidereal rotation period, hours. Ignored when tidally locked.
    pub sidereal_day_hours: f64,
    /// Rotation period equals the orbital period: one side always faces the star.
    pub tidally_locked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    /// Mean radius, km.
    pub radius_km: f64,
    /// Mean density, kg m⁻³.
    pub density: f64,
}

impl Body {
    pub fn radius(&self) -> f64 {
        self.radius_km * 1000.0
    }

    pub fn mass(&self) -> f64 {
        4.0 / 3.0 * std::f64::consts::PI * self.radius().powi(3) * self.density
    }

    pub fn gravity(&self) -> f64 {
        G * self.mass() / self.radius().powi(2)
    }

    pub fn escape_velocity(&self) -> f64 {
        (2.0 * G * self.mass() / self.radius()).sqrt()
    }

    pub fn surface_area(&self) -> f64 {
        4.0 * std::f64::consts::PI * self.radius().powi(2)
    }
}

/// Atmospheric composition as partial pressures in bar. The total is the
/// surface pressure.
#[derive(Clone, Debug, PartialEq)]
pub struct Atmosphere {
    pub n2: f64,
    pub o2: f64,
    pub co2: f64,
    pub ch4: f64,
    pub argon: f64,
}

impl Atmosphere {
    pub fn pressure_bar(&self) -> f64 {
        self.n2 + self.o2 + self.co2 + self.ch4 + self.argon
    }

    pub fn pressure(&self) -> f64 {
        self.pressure_bar() * BAR
    }

    /// Column mass of air, kg m⁻².
    pub fn column_mass(&self, gravity: f64) -> f64 {
        self.pressure() / gravity
    }

    /// Mixing ratio by partial pressure, in parts per million.
    pub fn ppm(&self, partial_bar: f64) -> f64 {
        let p = self.pressure_bar();
        if p > 0.0 { partial_bar / p * 1e6 } else { 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainParams {
    /// Fraction of the surface below sea level, 0–1.
    pub ocean_fraction: f64,
    /// Roughly how many continents (sets the scale of the base noise).
    pub continent_scale: f64,
    /// Highest mountains on an Earth-gravity world, m. Scaled by 1/g.
    pub max_elevation: f64,
    /// Typical ocean depth on an Earth-gravity world, m. Scaled by 1/g.
    pub ocean_depth: f64,
    /// 0–1: how rugged the land is.
    pub roughness: f64,
    /// Strength of volcanism and plate activity, 1 = Earth. Sets vent output
    /// and CO₂ outgassing.
    pub tectonic_activity: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanetParams {
    pub star: Star,
    pub orbit: Orbit,
    pub rotation: Rotation,
    pub body: Body,
    pub atmosphere: Atmosphere,
    pub terrain: TerrainParams,
}

impl PlanetParams {
    pub fn validate(&self) -> Result<(), String> {
        let check = |ok: bool, msg: &str| if ok { Ok(()) } else { Err(msg.to_string()) };
        check(
            self.star.mass >= 0.08 && self.star.mass <= 20.0,
            "star mass must be 0.08–20 solar masses",
        )?;
        check(
            self.star.luminosity_factor > 0.0,
            "luminosity factor must be > 0",
        )?;
        check(
            self.orbit.semi_major_axis > 0.005,
            "orbit must be wider than 0.005 AU",
        )?;
        check(
            (0.0..0.95).contains(&self.orbit.eccentricity),
            "eccentricity must be 0–0.95",
        )?;
        check(
            (0.0..=std::f64::consts::PI).contains(&self.orbit.obliquity),
            "obliquity must be 0–180°",
        )?;
        check(
            self.rotation.tidally_locked || self.rotation.sidereal_day_hours > 0.5,
            "day must be longer than 0.5 h",
        )?;
        check(
            self.body.radius_km >= 500.0 && self.body.radius_km <= 30_000.0,
            "radius must be 500–30,000 km",
        )?;
        check(
            self.body.density >= 500.0 && self.body.density <= 20_000.0,
            "density must be 500–20,000 kg/m³",
        )?;
        let a = &self.atmosphere;
        check(
            [a.n2, a.o2, a.co2, a.ch4, a.argon]
                .iter()
                .all(|&p| p >= 0.0 && p.is_finite()),
            "partial pressures must be ≥ 0",
        )?;
        check(
            a.pressure_bar() <= 200.0,
            "surface pressure must be ≤ 200 bar",
        )?;
        check(
            (0.0..=1.0).contains(&self.terrain.ocean_fraction),
            "ocean fraction must be 0–1",
        )?;
        Ok(())
    }

    /// Orbital period (sidereal year), s.
    pub fn year_seconds(&self) -> f64 {
        let a = self.orbit.semi_major_axis * AU;
        std::f64::consts::TAU * (a.powi(3) / self.star.gm()).sqrt()
    }

    /// Sidereal rotation period, s.
    pub fn sidereal_day_seconds(&self) -> f64 {
        if self.rotation.tidally_locked {
            self.year_seconds()
        } else {
            self.rotation.sidereal_day_hours * 3600.0
        }
    }

    pub fn rotation_rate(&self) -> f64 {
        std::f64::consts::TAU / self.sidereal_day_seconds()
    }

    /// Length of a solar day (noon to noon), s; `None` if the sun never moves
    /// across the sky (tidally locked).
    pub fn solar_day_seconds(&self) -> Option<f64> {
        let synodic_rate = self.rotation_rate() - std::f64::consts::TAU / self.year_seconds();
        (synodic_rate.abs() > 1e-12).then(|| std::f64::consts::TAU / synodic_rate.abs())
    }

    /// Mean stellar flux at the planet's orbit, W m⁻².
    pub fn mean_flux(&self) -> f64 {
        let a = self.orbit.semi_major_axis * AU;
        self.star.luminosity() / (4.0 * std::f64::consts::PI * a * a)
    }

    pub fn calendar(&self) -> dirtbox_core::Calendar {
        let year = self.year_seconds();
        dirtbox_core::Calendar {
            day_length: self.solar_day_seconds().unwrap_or(year),
            year_length: year,
        }
    }

    /// Gravity relative to Earth.
    pub fn relative_gravity(&self) -> f64 {
        self.body.gravity() / EARTH_GRAVITY
    }

    /// Radius relative to Earth.
    pub fn relative_radius(&self) -> f64 {
        self.body.radius() / EARTH_RADIUS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Preset;

    #[test]
    fn earth_numbers() {
        let p = Preset::EarthModern.params();
        p.validate().unwrap();
        assert!(
            (p.year_seconds() / 86_400.0 - 365.25).abs() < 0.2,
            "{}",
            p.year_seconds() / 86_400.0
        );
        let solar_day = p.solar_day_seconds().unwrap();
        assert!((solar_day - 86_400.0).abs() < 30.0, "{solar_day}");
        assert!((p.mean_flux() - 1361.0).abs() < 5.0, "{}", p.mean_flux());
        assert!((p.body.gravity() - 9.81).abs() < 0.05);
        assert!((p.body.escape_velocity() - 11_186.0).abs() < 100.0);
        assert!((p.star.temperature() - 5772.0).abs() < 1.0);
        let par = p.star.par_fraction();
        assert!((0.35..0.45).contains(&par), "{par}");
    }

    #[test]
    fn red_dwarf_gives_less_usable_light() {
        let sun = Star {
            mass: 1.0,
            luminosity_factor: 1.0,
        };
        let red = Star {
            mass: 0.3,
            luminosity_factor: 1.0,
        };
        assert!(red.temperature() < 4000.0);
        assert!(red.par_fraction() < sun.par_fraction() * 0.7);
    }

    #[test]
    fn tidal_lock_has_no_solar_day() {
        let p = Preset::TidallyLocked.params();
        assert!(p.solar_day_seconds().is_none());
        assert_eq!(p.calendar().day_length, p.year_seconds());
    }
}
