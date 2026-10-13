//! Ready-made worlds (design §11.1). Every value can be edited afterwards.

use crate::params::{Atmosphere, Body, Orbit, PlanetParams, Rotation, Star, TerrainParams};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    EarthModern,
    EarlyEarth,
    ColdDesert,
    TidallyLocked,
    SuperEarth,
    OceanWorld,
    Snowball,
}

impl Preset {
    pub const ALL: [Preset; 7] = [
        Preset::EarthModern,
        Preset::EarlyEarth,
        Preset::ColdDesert,
        Preset::TidallyLocked,
        Preset::SuperEarth,
        Preset::OceanWorld,
        Preset::Snowball,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Preset::EarthModern => "Earth (modern)",
            Preset::EarlyEarth => "Early Earth",
            Preset::ColdDesert => "Cold desert (Mars-like)",
            Preset::TidallyLocked => "Tidally locked red-dwarf world",
            Preset::SuperEarth => "Super-Earth",
            Preset::OceanWorld => "Ocean world",
            Preset::Snowball => "Snowball",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Preset::EarthModern => {
                "Today's Sun, orbit and tilt, with pre-industrial air (280 ppm CO₂) and bare land."
            }
            Preset::EarlyEarth => {
                "About 3.5 billion years ago: a Sun 20% fainter, no oxygen, a CO₂- and methane-rich \
                 sky, faster spin and few continents. The natural start for life from scratch."
            }
            Preset::ColdDesert => "A small, distant world with thin CO₂ air and a little water.",
            Preset::TidallyLocked => {
                "Close orbit around a red dwarf: one hemisphere in permanent day, the other in permanent night."
            }
            Preset::SuperEarth => {
                "1.5× Earth's radius, ~1.8× gravity, thicker air, about Earth's distance from its star."
            }
            Preset::OceanWorld => "Earth-like, but 97% ocean with a scatter of islands.",
            Preset::Snowball => {
                "Earth with little CO₂ and a slightly fainter Sun, starting frozen over."
            }
        }
    }

    /// Whether the world should start frozen rather than near equilibrium.
    pub fn starts_frozen(self) -> bool {
        self == Preset::Snowball
    }

    pub fn params(self) -> PlanetParams {
        let earth = PlanetParams {
            star: Star {
                mass: 1.0,
                luminosity_factor: 1.0,
            },
            orbit: Orbit {
                semi_major_axis: 1.000_001,
                eccentricity: 0.0167,
                perihelion_longitude: 282.94f64.to_radians(),
                obliquity: 23.44f64.to_radians(),
            },
            rotation: Rotation {
                sidereal_day_hours: 23.934_5,
                tidally_locked: false,
            },
            body: Body {
                radius_km: 6371.0,
                density: 5514.0,
            },
            atmosphere: Atmosphere {
                n2: 0.781,
                o2: 0.209,
                co2: 0.000_28,
                ch4: 0.000_000_7,
                argon: 0.009_3,
            },
            terrain: TerrainParams {
                ocean_fraction: 0.71,
                continent_scale: 1.0,
                max_elevation: 6_500.0,
                ocean_depth: 4_000.0,
                roughness: 0.5,
                tectonic_activity: 1.0,
            },
        };
        match self {
            Preset::EarthModern => earth,
            Preset::EarlyEarth => PlanetParams {
                star: Star {
                    luminosity_factor: 0.8,
                    ..earth.star
                },
                rotation: Rotation {
                    sidereal_day_hours: 15.0,
                    ..earth.rotation
                },
                // Enough CO₂ and methane to offset the faint young Sun.
                atmosphere: Atmosphere {
                    n2: 0.8,
                    o2: 0.0,
                    co2: 0.1,
                    ch4: 0.003,
                    argon: 0.009,
                },
                terrain: TerrainParams {
                    ocean_fraction: 0.88,
                    tectonic_activity: 2.0,
                    ..earth.terrain
                },
                ..earth
            },
            Preset::ColdDesert => PlanetParams {
                orbit: Orbit {
                    semi_major_axis: 1.524,
                    eccentricity: 0.093,
                    perihelion_longitude: 251f64.to_radians(),
                    obliquity: 25.19f64.to_radians(),
                },
                rotation: Rotation {
                    sidereal_day_hours: 24.62,
                    tidally_locked: false,
                },
                body: Body {
                    radius_km: 3390.0,
                    density: 3934.0,
                },
                atmosphere: Atmosphere {
                    n2: 0.000_16,
                    o2: 0.000_01,
                    co2: 0.005_8,
                    ch4: 0.0,
                    argon: 0.000_12,
                },
                terrain: TerrainParams {
                    ocean_fraction: 0.05,
                    max_elevation: 8_000.0,
                    tectonic_activity: 0.1,
                    ..earth.terrain
                },
                ..earth
            },
            Preset::TidallyLocked => PlanetParams {
                star: Star {
                    mass: 0.3,
                    luminosity_factor: 1.0,
                },
                // Placed where it receives about Earth's flux.
                orbit: Orbit {
                    semi_major_axis: 0.115,
                    eccentricity: 0.01,
                    perihelion_longitude: 0.0,
                    obliquity: 0.0,
                },
                rotation: Rotation {
                    sidereal_day_hours: 24.0,
                    tidally_locked: true,
                },
                atmosphere: Atmosphere {
                    n2: 0.99,
                    o2: 0.0,
                    co2: 0.001,
                    ch4: 0.0,
                    argon: 0.01,
                },
                ..earth
            },
            Preset::SuperEarth => PlanetParams {
                orbit: Orbit {
                    semi_major_axis: 1.02,
                    ..earth.orbit
                },
                body: Body {
                    radius_km: 9_550.0,
                    density: 6_500.0,
                },
                atmosphere: Atmosphere {
                    n2: 1.6,
                    o2: 0.0,
                    co2: 0.002,
                    ch4: 0.0,
                    argon: 0.02,
                },
                terrain: TerrainParams {
                    continent_scale: 1.6,
                    tectonic_activity: 1.5,
                    ..earth.terrain
                },
                ..earth
            },
            Preset::OceanWorld => PlanetParams {
                terrain: TerrainParams {
                    ocean_fraction: 0.97,
                    continent_scale: 2.0,
                    ..earth.terrain
                },
                ..earth
            },
            Preset::Snowball => PlanetParams {
                star: Star {
                    luminosity_factor: 0.94,
                    ..earth.star
                },
                atmosphere: Atmosphere {
                    co2: 0.000_1,
                    ..earth.atmosphere
                },
                ..earth
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_presets_are_valid() {
        for p in Preset::ALL {
            p.params()
                .validate()
                .unwrap_or_else(|e| panic!("{}: {e}", p.name()));
        }
    }

    #[test]
    fn tidally_locked_world_gets_about_earths_flux() {
        let p = Preset::TidallyLocked.params();
        let ratio = p.mean_flux() / Preset::EarthModern.params().mean_flux();
        assert!((0.8..1.2).contains(&ratio), "{ratio}");
    }

    #[test]
    fn super_earth_has_higher_gravity() {
        let g = Preset::SuperEarth.params().relative_gravity();
        assert!((1.6..2.0).contains(&g), "{g}");
    }
}
