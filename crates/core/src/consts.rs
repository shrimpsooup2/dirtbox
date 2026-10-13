//! Physical constants and reference values (SI units).

/// Newtonian gravitational constant, m³ kg⁻¹ s⁻².
pub const G: f64 = 6.674_30e-11;

/// Stefan–Boltzmann constant, W m⁻² K⁻⁴.
pub const STEFAN_BOLTZMANN: f64 = 5.670_374_419e-8;

/// Seconds in one Earth solar day.
pub const EARTH_SOLAR_DAY: f64 = 86_400.0;

/// Seconds in one Earth sidereal year.
pub const EARTH_SIDEREAL_YEAR: f64 = 365.256_363 * EARTH_SOLAR_DAY;

/// Earth mean radius, m.
pub const EARTH_RADIUS: f64 = 6.371_0e6;
