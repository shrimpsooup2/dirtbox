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

/// Solar luminosity, W.
pub const SOLAR_LUMINOSITY: f64 = 3.828e26;

/// Solar mass, kg.
pub const SOLAR_MASS: f64 = 1.988_47e30;

/// Solar effective temperature, K.
pub const SOLAR_TEMPERATURE: f64 = 5_772.0;

/// Astronomical unit, m.
pub const AU: f64 = 1.495_978_707e11;

/// Earth mean density, kg m⁻³.
pub const EARTH_DENSITY: f64 = 5_514.0;

/// Earth sidereal rotation rate, rad s⁻¹.
pub const EARTH_ROTATION_RATE: f64 = 7.292_115e-5;

/// Earth surface gravity, m s⁻².
pub const EARTH_GRAVITY: f64 = 9.806_65;

/// One standard bar, Pa.
pub const BAR: f64 = 1.0e5;

/// Specific heat of dry air at constant pressure, J kg⁻¹ K⁻¹.
pub const CP_AIR: f64 = 1_004.0;

/// Specific gas constant of dry air, J kg⁻¹ K⁻¹.
pub const R_AIR: f64 = 287.0;

/// Latent heat of vaporisation of water, J kg⁻¹.
pub const LATENT_VAPORISATION: f64 = 2.5e6;

/// Latent heat of fusion of water, J kg⁻¹.
pub const LATENT_FUSION: f64 = 3.34e5;

/// Freezing point of fresh water, K.
pub const FREEZING_FRESH: f64 = 273.15;

/// Freezing point of sea water, K.
pub const FREEZING_SEA: f64 = 271.35;

/// Density of ice, kg m⁻³.
pub const ICE_DENSITY: f64 = 917.0;

/// Density of sea water, kg m⁻³.
pub const SEA_WATER_DENSITY: f64 = 1_025.0;

/// Specific heat of sea water, J kg⁻¹ K⁻¹.
pub const CP_SEA_WATER: f64 = 3_990.0;
