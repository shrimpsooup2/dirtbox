//! Climate on the geodesic grid (design §4): an energy-balance model with a
//! gray greenhouse, circulation-cell winds, a water cycle with clouds, snow
//! and sea ice, rivers and lakes, and an optional carbon–silicate cycle.
//!
//! One step, of any length (an hour or a day of planet time), does:
//! 1. **Column physics** per cell: sunlight, albedo, clouds, greenhouse,
//!    evaporation and an implicit energy balance (stable for long steps).
//! 2. **Transport** by the wind: water vapour (conservative upwind, with rain
//!    forming as it goes) and heat (advective upwind).
//! 3. **Diffusion** of heat by weather eddies (implicit, conjugate
//!    gradients) and by ocean currents (between ocean cells).
//! 4. **Phase changes**: sea ice forming and melting, snowmelt.
//! 5. **Hydrology**: soil water, runoff, rivers and lakes (once per planet day).
//!
//! Everything is in SI units. Water is conserved exactly, including an ocean
//! reservoir; transport conserves energy.

// The kernels update several per-cell arrays together; an index loop is the
// clearest way to write that.
#![allow(clippy::needless_range_loop)]

pub mod hydrology;
pub mod physics;
pub mod stats;
pub mod transport;
pub mod wind;

use dirtbox_core::Vec3;
use dirtbox_core::consts::{
    CP_AIR, CP_SEA_WATER, EARTH_GRAVITY, EARTH_ROTATION_RATE, FREEZING_FRESH, FREEZING_SEA,
    ICE_DENSITY, LATENT_FUSION, LATENT_VAPORISATION, R_AIR, SEA_WATER_DENSITY,
};
use dirtbox_grid::{CellId, GeodesicGrid};
use dirtbox_planet::{PlanetParams, Terrain, daily_mean_insolation, instantaneous_insolation};

pub use hydrology::Drainage;
pub use stats::{ClimateZone, MonthlyStats, YearSummary};
pub use wind::CirculationSummary;

use physics::*;
pub use transport::Geometry;

/// How often the circulation is recomputed, planet s.
const WIND_INTERVAL: f64 = 6.0 * 3600.0;

/// Distance over which condensation heating spreads, m.
const LATENT_LENGTH: f64 = 300e3;

/// How sunlight is applied each step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepMode {
    /// Instantaneous sunlight: day and night (use steps of about an hour).
    Diurnal,
    /// Sunlight averaged over each rotation (use steps of about a day). Much
    /// faster for spinning up a climate or running decades.
    DailyMean,
}

/// Tunable parameters. Defaults are calibrated against modern Earth.
#[derive(Clone, Debug, PartialEq)]
pub struct ClimateParams {
    /// Depth of the ocean surface layer that exchanges heat with the air, m.
    pub ocean_mixed_layer: f64,
    /// Heat capacity of the land surface skin, J m⁻² K⁻¹.
    pub land_heat_capacity: f64,
    /// Heat capacity of a snow-covered sea-ice surface, J m⁻² K⁻¹.
    pub ice_heat_capacity: f64,
    /// Share of the air column's heat capacity that moves with the surface.
    pub boundary_layer_fraction: f64,
    /// Eddy diffusivity of the atmosphere at Earth's rotation rate, m² s⁻¹.
    pub atmosphere_diffusivity: f64,
    /// Effective diffusivity of ocean heat transport, m² s⁻¹.
    pub ocean_diffusivity: f64,
    pub albedo_ocean: f64,
    pub albedo_land_dry: f64,
    pub albedo_land_wet: f64,
    pub albedo_snow: f64,
    pub albedo_sea_ice: f64,
    pub albedo_cloud: f64,
    /// Thermal conductivity of sea ice, W m⁻¹ K⁻¹.
    pub ice_conductivity: f64,
    /// Sea ice thicker than this covers the water (switches the surface), m.
    pub ice_cover_thickness: f64,
    /// Water the soil can hold before it runs off, kg m⁻².
    pub soil_capacity: f64,
    /// Snow beyond this flows away as glacier ice, kg m⁻².
    pub max_snow: f64,
    /// Bulk transfer coefficient for evaporation.
    pub evaporation_coefficient: f64,
    /// Relative humidity at which neutral air starts to rain.
    pub rain_humidity: f64,
    /// Time for excess vapour to rain out, planet s.
    pub rain_timescale: f64,
    /// Time scale of light background rain at saturation, planet s.
    pub background_rain_timescale: f64,
    /// Temperature drop with height at Earth gravity, K m⁻¹.
    pub lapse_rate: f64,
    pub wind_speed_scale: f64,
    /// Run the carbon–silicate cycle (volcanic CO₂ vs. weathering).
    pub carbon_cycle: bool,
    /// Volcanic outgassing at Earth's tectonic activity, bar of CO₂ per
    /// geologic year.
    pub outgassing: f64,
}

impl Default for ClimateParams {
    fn default() -> Self {
        Self {
            ocean_mixed_layer: 50.0,
            land_heat_capacity: 6.0e5,
            ice_heat_capacity: 6.0e5,
            boundary_layer_fraction: 0.1,
            atmosphere_diffusivity: 1.5e6,
            ocean_diffusivity: 5.0e4,
            albedo_ocean: 0.06,
            albedo_land_dry: 0.28,
            albedo_land_wet: 0.18,
            albedo_snow: 0.65,
            albedo_sea_ice: 0.55,
            albedo_cloud: 0.33,
            ice_conductivity: 2.0,
            ice_cover_thickness: 0.3,
            soil_capacity: 150.0,
            max_snow: 20_000.0,
            evaporation_coefficient: 1.3e-3,
            rain_humidity: 0.85,
            rain_timescale: 3.0 * 3600.0,
            background_rain_timescale: 8.0 * 86_400.0,
            lapse_rate: 6.5e-3,
            wind_speed_scale: 1.0,
            carbon_cycle: false,
            outgassing: 6.0e-8,
        }
    }
}

/// Inputs for one climate step.
pub struct StepInputs<'a> {
    pub grid: &'a GeodesicGrid,
    pub planet: &'a PlanetParams,
    pub terrain: &'a Terrain,
    /// Planet time at the start of the step, s.
    pub start: f64,
    /// Step length, planet s.
    pub dt: f64,
    pub mode: StepMode,
    /// Geologic years per planet second (for the carbon cycle).
    pub geologic_rate: f64,
}

/// What a step changed outside the climate itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepReport {
    /// Change in atmospheric CO₂ partial pressure from the carbon cycle, bar.
    pub co2_change: f64,
    pub diffusion_iterations: usize,
    pub advection_substeps: usize,
}

/// Global numbers for display and tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlobalSummary {
    pub mean_temperature: f64,
    pub min_temperature: f64,
    pub max_temperature: f64,
    /// Fraction of the surface under sea ice or thick snow.
    pub ice_fraction: f64,
    pub cloud_fraction: f64,
    pub planetary_albedo: f64,
    /// Mean precipitation, mm per Earth day.
    pub precipitation: f64,
    pub column_water: f64,
    /// Mean absorbed sunlight and outgoing infrared, W m⁻².
    pub absorbed: f64,
    pub outgoing: f64,
}

pub struct Climate {
    pub params: ClimateParams,
    geo: Geometry,
    radius: f64,

    /// Near-surface air / surface temperature, K.
    pub temperature: Vec<f64>,
    /// Water vapour column, kg m⁻².
    pub vapour: Vec<f64>,
    /// Sea ice thickness, m.
    pub sea_ice: Vec<f64>,
    /// Snow (water equivalent), kg m⁻².
    pub snow: Vec<f64>,
    /// Soil water, kg m⁻².
    pub soil: Vec<f64>,
    /// Lake on this cell (from the last routing).
    pub lake: Vec<bool>,

    // Diagnostics from the last step.
    pub wind: Vec<Vec3>,
    pub ascent: Vec<f64>,
    pub insolation: Vec<f64>,
    pub albedo: Vec<f64>,
    pub cloud: Vec<f64>,
    /// Outgoing infrared at the top of the atmosphere, W m⁻².
    pub olr: Vec<f64>,
    /// Precipitation rate, kg m⁻² s⁻¹.
    pub precipitation: Vec<f64>,
    /// Evaporation rate, kg m⁻² s⁻¹.
    pub evaporation: Vec<f64>,
    /// River discharge leaving each cell, m³ s⁻¹ (updated once per planet day).
    pub river_flow: Vec<f64>,
    pub circulation: CirculationSummary,
    orographic: Vec<f64>,
    time_since_wind: f64,

    pub stats: MonthlyStats,
    drainage: Drainage,
    /// Runoff accumulated since the last routing, kg m⁻².
    runoff_since_routing: Vec<f64>,
    time_since_routing: f64,
    /// Water in the ocean relative to the start, kg. Lets the water budget
    /// close exactly.
    ocean_water: f64,
}

impl Climate {
    /// A climate near equilibrium for the planet, or frozen over.
    pub fn new(
        grid: &GeodesicGrid,
        planet: &PlanetParams,
        terrain: &Terrain,
        params: ClimateParams,
        frozen: bool,
    ) -> Self {
        let n = grid.cell_count();
        let radius = planet.body.radius();
        let g = planet.body.gravity();
        let lapse = params.lapse_rate * g / EARTH_GRAVITY;
        let guess = initial_temperature(grid, planet);
        let temperature: Vec<f64> = grid
            .cells()
            .map(|c| {
                let t = guess[c as usize] - lapse * terrain.height_above_sea(c).max(0.0);
                if frozen { t.min(230.0) } else { t }
            })
            .collect();
        let sea_ice = grid
            .cells()
            .map(|c| {
                let t = temperature[c as usize];
                if !terrain.is_ocean(c) {
                    0.0
                } else if frozen {
                    3.0
                } else if t < FREEZING_SEA - 5.0 {
                    1.5
                } else {
                    0.0
                }
            })
            .collect();
        let snow = grid
            .cells()
            .map(|c| {
                if terrain.is_ocean(c) {
                    0.0
                } else if frozen {
                    500.0
                } else {
                    // Snow builds up from the water cycle; dry worlds stay bare.
                    0.0
                }
            })
            .collect();
        let vapour = temperature
            .iter()
            .map(|&t| 0.6 * saturated_column_water(t, g))
            .collect();
        let soil = grid
            .cells()
            .map(|c| {
                if terrain.is_ocean(c) {
                    0.0
                } else {
                    0.5 * params.soil_capacity
                }
            })
            .collect();
        Self {
            geo: Geometry::new(grid, radius),
            radius,
            temperature,
            vapour,
            sea_ice,
            snow,
            soil,
            lake: vec![false; n],
            wind: vec![Vec3::ZERO; n],
            ascent: vec![0.0; n],
            insolation: vec![0.0; n],
            albedo: vec![0.3; n],
            cloud: vec![0.0; n],
            olr: vec![0.0; n],
            precipitation: vec![0.0; n],
            evaporation: vec![0.0; n],
            river_flow: vec![0.0; n],
            circulation: CirculationSummary::default(),
            orographic: Vec::new(),
            time_since_wind: 0.0,
            stats: MonthlyStats::new(n),
            drainage: Drainage::build(grid, terrain),
            runoff_since_routing: vec![0.0; n],
            time_since_routing: 0.0,
            ocean_water: 0.0,
            params,
        }
    }

    /// Where water flows on land (rebuilt when the terrain changes).
    pub fn drainage(&self) -> &Drainage {
        &self.drainage
    }

    /// Grid geometry in physical units (areas, edge lengths, neighbours).
    pub fn geometry(&self) -> &Geometry {
        &self.geo
    }

    pub fn cell_area(&self, cell: CellId) -> f64 {
        self.geo.area[cell as usize]
    }

    /// Total water in the system, kg (vapour, snow, soil, ice and the ocean
    /// reservoir's change). Constant except for player edits.
    pub fn total_water(&self) -> f64 {
        let cells: f64 = (0..self.temperature.len())
            .map(|i| {
                (self.vapour[i]
                    + self.snow[i]
                    + self.soil[i]
                    + self.sea_ice[i] * ICE_DENSITY
                    + self.runoff_since_routing[i])
                    * self.geo.area[i]
            })
            .sum();
        cells + self.ocean_water
    }

    pub fn is_ice_covered(&self, cell: CellId) -> bool {
        self.sea_ice[cell as usize] >= self.params.ice_cover_thickness
    }

    /// Heat capacity of each cell's surface layer, J m⁻² K⁻¹.
    fn capacities(&self, terrain: &Terrain, air: f64) -> Vec<f64> {
        let p = &self.params;
        let ocean = SEA_WATER_DENSITY * CP_SEA_WATER * p.ocean_mixed_layer;
        let bl = p.boundary_layer_fraction * air;
        (0..self.temperature.len())
            .map(|i| {
                if terrain.ocean_mask()[i] {
                    if self.sea_ice[i] >= p.ice_cover_thickness {
                        p.ice_heat_capacity + bl
                    } else {
                        ocean
                    }
                } else {
                    p.land_heat_capacity + bl
                }
            })
            .collect()
    }

    pub fn step(&mut self, input: &StepInputs) -> StepReport {
        let grid = input.grid;
        let planet = input.planet;
        let terrain = input.terrain;
        let dt = input.dt;
        let n = grid.cell_count();
        assert_eq!(
            n,
            self.temperature.len(),
            "grid changed without rebuilding the climate"
        );
        let p = self.params.clone();
        let atm = &planet.atmosphere;
        let g = planet.body.gravity();
        let pressure = atm.pressure();
        let air = atm.column_mass(g) * CP_AIR;
        let lapse = p.lapse_rate * g / EARTH_GRAVITY;
        let ocean = terrain.ocean_mask();
        let land_height: Vec<f64> = grid
            .cells()
            .map(|c| {
                if ocean[c as usize] {
                    0.0
                } else {
                    terrain.height_above_sea(c).max(0.0)
                }
            })
            .collect();
        let mut report = StepReport::default();

        // --- Sunlight -------------------------------------------------------
        let sun = planet.sun_state(input.start + dt / 2.0);
        let daily = input.mode == StepMode::DailyMean && !planet.rotation.tidally_locked;
        for c in grid.cells() {
            let pos = grid.position(c);
            self.insolation[c as usize] = if daily {
                daily_mean_insolation(sun.flux, pos.latitude(), sun.declination)
            } else {
                instantaneous_insolation(&sun, pos)
            };
        }

        // --- Wind -----------------------------------------------------------
        let mut theta: Vec<f64> = (0..n)
            .map(|i| self.temperature[i] + lapse * land_height[i])
            .collect();
        // The large-scale circulation changes slowly: refresh it every few
        // planet hours rather than every step.
        self.time_since_wind += dt;
        if self.time_since_wind >= WIND_INTERVAL || self.orographic.len() != n {
            self.time_since_wind = 0.0;
            let (field, circulation) = wind::compute(&wind::CirculationInputs {
                grid,
                geometry: &self.geo,
                radius: self.radius,
                rotation_rate: planet.rotation_rate(),
                tidally_locked: planet.rotation.tidally_locked,
                sun_direction: sun.direction,
                obliquity: planet.orbit.obliquity,
                theta: &theta,
                land_height: &land_height,
                speed_scale: p.wind_speed_scale,
            });
            self.wind = field.wind;
            self.ascent = field.ascent;
            self.orographic = field.orographic;
            self.circulation = circulation;
        }
        let orographic = self.orographic.clone();

        // --- Column physics -------------------------------------------------
        let capacity = self.capacities(terrain, air);
        let rayleigh = rayleigh_albedo(atm);
        let ice_capacity = p.ice_cover_thickness;
        for i in 0..n {
            let t = self.temperature[i];
            let wsat = saturated_column_water(t, g).max(1e-6);
            let rh = (self.vapour[i] / wsat).min(1.5);
            let lift = self.ascent[i] + 0.5 * orographic[i].max(0.0);
            let stratiform = 0.95 * smoothstep(0.05, 0.8, rh) * (0.8 + 0.2 * lift.clamp(-1.0, 1.0));
            // Deep convection (heavy rain) builds thick, bright anvil clouds.
            let rain_mm_day = self.precipitation[i] * 86_400.0;
            let convective = 0.9 * (1.0 - (-rain_mm_day / 20.0).exp());
            let cloud = stratiform.max(convective).min(0.95);
            let cloud_albedo = p.albedo_cloud + 0.2 * convective;
            self.cloud[i] = cloud;

            let is_lake = !ocean[i] && self.lake[i];
            let ice_cover = ocean[i] && self.sea_ice[i] >= ice_capacity;
            let surface_albedo = if ocean[i] {
                let f = (self.sea_ice[i] / ice_capacity).clamp(0.0, 1.0);
                p.albedo_ocean + f * (p.albedo_sea_ice - p.albedo_ocean)
            } else if is_lake {
                p.albedo_ocean
            } else {
                let wet = (self.soil[i] / p.soil_capacity).clamp(0.0, 1.0);
                let bare = p.albedo_land_dry + wet * (p.albedo_land_wet - p.albedo_land_dry);
                let snow_cover = (self.snow[i] / 50.0).clamp(0.0, 1.0);
                bare + snow_cover * (p.albedo_snow - bare)
            };
            let albedo = rayleigh
                + (1.0 - rayleigh)
                    * (cloud * cloud_albedo.max(surface_albedo) + (1.0 - cloud) * surface_albedo);
            self.albedo[i] = albedo;
            let absorbed = self.insolation[i] * (1.0 - albedo);
            let tau = optical_depth(atm, self.vapour[i], cloud);

            // Evaporation and sublimation (kg m⁻² s⁻¹), limited by supply.
            let speed = self.wind[i].length().max(5.0);
            let rho_air = pressure / (R_AIR * t);
            let potential = rho_air
                * p.evaporation_coefficient
                * speed
                * saturation_specific_humidity(t, pressure)
                * (1.0 - rh).max(0.0);
            let (rate, latent) = if ocean[i] {
                if ice_cover {
                    let e = (0.05 * potential).min(self.sea_ice[i] * ICE_DENSITY / dt);
                    self.sea_ice[i] -= e * dt / ICE_DENSITY;
                    (e, LATENT_VAPORISATION + LATENT_FUSION)
                } else {
                    self.ocean_water -= potential * dt * self.geo.area[i];
                    (potential, LATENT_VAPORISATION)
                }
            } else if is_lake {
                self.ocean_water -= potential * dt * self.geo.area[i];
                (potential, LATENT_VAPORISATION)
            } else if self.snow[i] > 1.0 {
                let e = (0.3 * potential).min(self.snow[i] / dt);
                self.snow[i] -= e * dt;
                (e, LATENT_VAPORISATION + LATENT_FUSION)
            } else {
                let e = (potential * (self.soil[i] / p.soil_capacity).clamp(0.0, 1.0))
                    .min(self.soil[i] / dt);
                self.soil[i] -= e * dt;
                (e, LATENT_VAPORISATION)
            };
            self.vapour[i] += rate * dt;
            self.evaporation[i] = rate;

            // Implicit energy balance with linearised outgoing radiation (and
            // conduction through sea ice from the water below).
            let mut forcing = absorbed - outgoing_longwave(t, tau) - latent * rate;
            let mut stiffness = outgoing_longwave_derivative(t, tau);
            let ice_k = if ice_cover {
                let k = p.ice_conductivity / self.sea_ice[i].max(0.1);
                forcing += k * (FREEZING_SEA - t);
                stiffness += k;
                k
            } else {
                0.0
            };
            let new_t = t + dt * forcing / (capacity[i] + dt * stiffness);
            if ice_cover {
                // Heat drawn up through the ice freezes more water at its base.
                let conducted = ice_k * (FREEZING_SEA - new_t);
                if conducted > 0.0 {
                    let grow = conducted * dt / (ICE_DENSITY * LATENT_FUSION);
                    self.sea_ice[i] += grow;
                    self.ocean_water -= grow * ICE_DENSITY * self.geo.area[i];
                }
            }
            self.temperature[i] = new_t;
            self.olr[i] =
                outgoing_longwave(t, tau) + outgoing_longwave_derivative(t, tau) * (new_t - t);
        }

        // --- Transport by the wind ------------------------------------------
        for i in 0..n {
            theta[i] = self.temperature[i] + lapse * land_height[i];
        }
        let un = transport::edge_velocities(grid, &self.geo, &self.wind);
        let substeps = transport::advection_substeps(&self.geo, &un, dt);
        report.advection_substeps = substeps;
        let sub_dt = dt / substeps as f64;
        let coupling: Vec<f64> = capacity.iter().map(|&c| (air / c).min(1.0)).collect();
        let heat_before: f64 = (0..n)
            .map(|i| capacity[i] * self.geo.area[i] * theta[i])
            .sum();
        let mut rain_mass = vec![0.0; n];
        let mut latent_added = 0.0;
        // Latent heat released by condensation, J, spread out after transport.
        let mut latent_heat = vec![0.0; n];
        let rain_keep = 1.0 - (-sub_dt / p.rain_timescale).exp();
        let drizzle_keep = 1.0 - (-sub_dt / p.background_rain_timescale).exp();
        for _ in 0..substeps {
            transport::advect_flux_form(&self.geo, &un, &mut self.vapour, sub_dt);
            transport::advect_advective_form(&self.geo, &un, &mut theta, &coupling, sub_dt);
            for i in 0..n {
                let t = theta[i] - lapse * land_height[i];
                let wsat = saturated_column_water(t, g);
                let threshold =
                    (p.rain_humidity - 0.15 * self.ascent[i] - 0.3 * orographic[i].max(0.0)
                        + 0.1 * (-orographic[i]).max(0.0))
                    .clamp(0.55, 0.97);
                // Large-scale rain above the threshold, plus light background
                // rain (showers and fronts the circulation model doesn't resolve).
                let rh = (self.vapour[i] / wsat.max(1e-9)).min(1.0);
                let excess = (self.vapour[i] - threshold * wsat).max(0.0);
                let condensed = excess * rain_keep + self.vapour[i] * rh.powi(4) * drizzle_keep;
                if condensed <= 0.0 {
                    continue;
                }
                self.vapour[i] -= condensed;
                rain_mass[i] += condensed;
                let snowing = t < FREEZING_FRESH;
                let mut heat =
                    condensed * (LATENT_VAPORISATION + if snowing { LATENT_FUSION } else { 0.0 });
                if ocean[i] {
                    if self.sea_ice[i] >= ice_capacity && snowing {
                        self.sea_ice[i] += condensed / ICE_DENSITY;
                    } else {
                        self.ocean_water += condensed * self.geo.area[i];
                        if snowing {
                            // Snow melting into open water takes its fusion heat back.
                            heat -= condensed * LATENT_FUSION;
                        }
                    }
                } else if self.lake[i] && !snowing {
                    self.ocean_water += condensed * self.geo.area[i];
                } else if snowing {
                    self.snow[i] += condensed;
                } else {
                    self.soil[i] += condensed;
                }
                latent_heat[i] += heat * self.geo.area[i];
            }
        }
        // Condensation heats the air aloft, which spreads it over hundreds of
        // kilometres before it reaches the surface. Smooth the released heat
        // over a fixed length (conservatively), then warm each cell by its share.
        let mut latent_per_area: Vec<f64> =
            (0..n).map(|i| latent_heat[i] / self.geo.area[i]).collect();
        transport::smooth(&self.geo, &mut latent_per_area, LATENT_LENGTH);
        for i in 0..n {
            theta[i] += latent_per_area[i] / capacity[i];
        }
        latent_added += latent_heat.iter().sum::<f64>();

        // Advective-form transport is not exactly conservative where the wind
        // converges; return any drift evenly so energy is conserved.
        let heat_after: f64 = (0..n)
            .map(|i| capacity[i] * self.geo.area[i] * theta[i])
            .sum();
        let drift = heat_after - heat_before - latent_added;
        let total_capacity: f64 = (0..n).map(|i| capacity[i] * self.geo.area[i]).sum();
        for t in theta.iter_mut() {
            *t -= drift / total_capacity;
        }
        for i in 0..n {
            self.precipitation[i] = rain_mass[i] / dt;
        }

        // --- Eddy diffusion of heat (implicit) -------------------------------
        let rotation_factor = (EARTH_ROTATION_RATE / planet.rotation_rate().abs().max(1e-12))
            .powi(2)
            .clamp(0.1, 10.0);
        let k_atm = air * p.atmosphere_diffusivity * rotation_factor;
        report.diffusion_iterations =
            transport::diffuse_implicit(&self.geo, &mut theta, &capacity, k_atm, dt);
        // Weather eddies mix moisture as well as heat (conservatively).
        let unit = vec![1.0; n];
        transport::diffuse_implicit(
            &self.geo,
            &mut self.vapour,
            &unit,
            p.atmosphere_diffusivity * rotation_factor,
            dt,
        );
        for i in 0..n {
            self.temperature[i] = theta[i] - lapse * land_height[i];
        }

        // --- Ocean heat transport --------------------------------------------
        self.ocean_transport(grid, terrain, dt);

        // --- Phase changes ---------------------------------------------------
        self.phase_changes(terrain, &capacity);

        // --- Soil, runoff, rivers -------------------------------------------
        for i in 0..n {
            if ocean[i] {
                continue;
            }
            if self.soil[i] > p.soil_capacity {
                self.runoff_since_routing[i] += self.soil[i] - p.soil_capacity;
                self.soil[i] = p.soil_capacity;
            }
            if self.snow[i] > p.max_snow {
                // Glaciers flow downhill and calve into the sea.
                self.runoff_since_routing[i] += self.snow[i] - p.max_snow;
                self.snow[i] = p.max_snow;
            }
        }
        self.time_since_routing += dt;
        let routing_interval = planet.calendar().day_length.min(86_400.0);
        if self.time_since_routing >= routing_interval {
            self.route_rivers(grid, terrain, &open_water_evaporation(self, pressure));
        }

        // --- Statistics -------------------------------------------------------
        let calendar = planet.calendar();
        let mid = input.start + dt / 2.0;
        let year = (mid / calendar.year_length).floor() as i64;
        self.stats.record(
            year,
            calendar.year_fraction(mid),
            dt,
            &self.temperature,
            &self.precipitation,
            ocean,
        );

        // --- Carbon–silicate cycle -------------------------------------------
        if p.carbon_cycle {
            report.co2_change = self.carbon_change(grid, terrain, planet, dt * input.geologic_rate);
        }
        report
    }

    fn ocean_transport(&mut self, grid: &GeodesicGrid, terrain: &Terrain, dt: f64) {
        let p = &self.params;
        let ocean = terrain.ocean_mask();
        let c_ml = SEA_WATER_DENSITY * CP_SEA_WATER * p.ocean_mixed_layer;
        let k = c_ml * p.ocean_diffusivity;
        let ids = grid.neighbour_ids();
        // Explicit substeps sized for stability on the smallest cell.
        let worst = grid
            .cells()
            .map(|c| {
                grid.neighbour_range(c)
                    .map(|e| self.geo.conductance[e])
                    .sum::<f64>()
                    / self.geo.area[c as usize]
            })
            .fold(0.0f64, f64::max);
        let substeps = ((dt * k * worst / c_ml / 0.4).ceil() as usize).max(1);
        let h = dt / substeps as f64;
        let ice_cover = p.ice_cover_thickness;
        for _ in 0..substeps {
            let water_t: Vec<f64> = (0..ocean.len())
                .map(|i| {
                    if self.sea_ice[i] >= ice_cover {
                        FREEZING_SEA
                    } else {
                        self.temperature[i]
                    }
                })
                .collect();
            let mut heat = vec![0.0; ocean.len()];
            for c in grid.cells() {
                let i = c as usize;
                if !ocean[i] {
                    continue;
                }
                for e in grid.neighbour_range(c) {
                    let j = ids[e] as usize;
                    if j > i && ocean[j] {
                        let flux = k * self.geo.conductance[e] * (water_t[j] - water_t[i]) * h;
                        heat[i] += flux;
                        heat[j] -= flux;
                    }
                }
            }
            for i in 0..ocean.len() {
                if heat[i] == 0.0 {
                    continue;
                }
                if self.sea_ice[i] >= ice_cover {
                    // Heat arriving under the ice melts it from below (or freezes more).
                    let melt = heat[i] / (ICE_DENSITY * LATENT_FUSION * self.geo.area[i]);
                    let melt = melt.min(self.sea_ice[i]);
                    self.sea_ice[i] -= melt;
                    self.ocean_water += melt * ICE_DENSITY * self.geo.area[i];
                } else {
                    self.temperature[i] += heat[i] / (c_ml * self.geo.area[i]);
                }
            }
        }
    }

    fn phase_changes(&mut self, terrain: &Terrain, capacity: &[f64]) {
        let p = &self.params;
        let ocean = terrain.ocean_mask();
        let c_ml = SEA_WATER_DENSITY * CP_SEA_WATER * p.ocean_mixed_layer;
        for i in 0..self.temperature.len() {
            let area = self.geo.area[i];
            if ocean[i] {
                if self.sea_ice[i] >= p.ice_cover_thickness {
                    // Ice surface: melt from the top above 0 °C.
                    if self.temperature[i] > FREEZING_FRESH {
                        let energy = capacity[i] * (self.temperature[i] - FREEZING_FRESH);
                        let melt = (energy / (ICE_DENSITY * LATENT_FUSION)).min(self.sea_ice[i]);
                        self.sea_ice[i] -= melt;
                        self.ocean_water += melt * ICE_DENSITY * area;
                        self.temperature[i] = FREEZING_FRESH
                            + (energy - melt * ICE_DENSITY * LATENT_FUSION) / capacity[i];
                    }
                    if self.sea_ice[i] < p.ice_cover_thickness {
                        // Broken up: back to open water at the freezing point.
                        self.temperature[i] = FREEZING_SEA;
                    }
                } else {
                    if self.temperature[i] < FREEZING_SEA {
                        let deficit = c_ml * (FREEZING_SEA - self.temperature[i]);
                        let grow = deficit / (ICE_DENSITY * LATENT_FUSION);
                        self.sea_ice[i] += grow;
                        self.ocean_water -= grow * ICE_DENSITY * area;
                        self.temperature[i] = FREEZING_SEA;
                    } else if self.sea_ice[i] > 0.0 {
                        let energy = c_ml * (self.temperature[i] - FREEZING_SEA);
                        let melt = (energy / (ICE_DENSITY * LATENT_FUSION)).min(self.sea_ice[i]);
                        self.sea_ice[i] -= melt;
                        self.ocean_water += melt * ICE_DENSITY * area;
                        self.temperature[i] -= melt * ICE_DENSITY * LATENT_FUSION / c_ml;
                    }
                    if self.sea_ice[i] >= p.ice_cover_thickness {
                        // Frozen over: the surface is now ice, starting at freezing.
                        self.temperature[i] = self.temperature[i].min(FREEZING_SEA);
                    }
                }
            } else if self.snow[i] > 0.0 && self.temperature[i] > FREEZING_FRESH {
                let energy = capacity[i] * (self.temperature[i] - FREEZING_FRESH);
                let melt = (energy / LATENT_FUSION).min(self.snow[i]);
                self.snow[i] -= melt;
                self.soil[i] += melt;
                self.temperature[i] -= melt * LATENT_FUSION / capacity[i];
            }
        }
    }

    fn route_rivers(
        &mut self,
        grid: &GeodesicGrid,
        terrain: &Terrain,
        open_water_evaporation: &[f64],
    ) {
        let interval = self.time_since_routing;
        let rates: Vec<f64> = self
            .runoff_since_routing
            .iter()
            .map(|r| r / interval)
            .collect();
        let flow = self.drainage.accumulate(&rates, &self.geo.area);
        for c in grid.cells() {
            let i = c as usize;
            // Runoff reaches the sea within the routing interval.
            self.ocean_water += self.runoff_since_routing[i] * self.geo.area[i];
            self.runoff_since_routing[i] = 0.0;
            self.river_flow[i] = if terrain.is_ocean(c) {
                0.0
            } else {
                flow[i] / 1000.0
            };
            // A basin holds a lake if its inflow can keep up with evaporation.
            let inflow = flow[i] - rates[i] * self.geo.area[i];
            self.lake[i] = !terrain.is_ocean(c)
                && self.drainage.basin_depth[i] > 1.0
                && inflow > open_water_evaporation[i] * self.geo.area[i];
        }
        self.time_since_routing = 0.0;
    }

    /// Change in CO₂ (bar) over `geologic_years`: volcanic outgassing minus
    /// silicate weathering, which grows with runoff and warmth.
    fn carbon_change(
        &self,
        grid: &GeodesicGrid,
        terrain: &Terrain,
        planet: &PlanetParams,
        geologic_years: f64,
    ) -> f64 {
        let outgassing = self.params.outgassing * planet.terrain.tectonic_activity;
        // Weathering normalised to Earth: ~1.2e9 kg/s of runoff at 288 K
        // balances Earth's outgassing.
        let earth_area_ratio = planet.body.surface_area() / 5.1e14;
        let weathering_index: f64 = grid
            .cells()
            .filter(|&c| !terrain.is_ocean(c))
            .map(|c| {
                let i = c as usize;
                let runoff_rate = (self.precipitation[i] - self.evaporation[i]).max(0.0);
                runoff_rate * self.geo.area[i] * (0.09 * (self.temperature[i] - 288.0)).exp()
            })
            .sum::<f64>()
            / (1.2e9 * earth_area_ratio);
        let weathering = self.params.outgassing * weathering_index;
        let change = (outgassing - weathering) * geologic_years;
        change.max(-planet.atmosphere.co2)
    }

    pub fn summary(&self, grid: &GeodesicGrid, terrain: &Terrain) -> GlobalSummary {
        let mut s = GlobalSummary {
            min_temperature: f64::MAX,
            max_temperature: f64::MIN,
            ..Default::default()
        };
        let total: f64 = self.geo.area.iter().sum();
        for c in grid.cells() {
            let i = c as usize;
            let w = self.geo.area[i] / total;
            let t = self.temperature[i];
            s.mean_temperature += t * w;
            s.min_temperature = s.min_temperature.min(t);
            s.max_temperature = s.max_temperature.max(t);
            let icy = if terrain.is_ocean(c) {
                self.sea_ice[i] >= self.params.ice_cover_thickness
            } else {
                self.snow[i] > 100.0
            };
            if icy {
                s.ice_fraction += w;
            }
            s.cloud_fraction += self.cloud[i] * w;
            s.planetary_albedo += self.albedo[i] * self.insolation[i] * w;
            s.precipitation += self.precipitation[i] * 86_400.0 * w;
            s.column_water += self.vapour[i] * w;
            s.absorbed += self.insolation[i] * (1.0 - self.albedo[i]) * w;
            s.outgoing += self.olr[i] * w;
        }
        let mean_insolation: f64 = grid
            .cells()
            .map(|c| self.insolation[c as usize] * self.geo.area[c as usize] / total)
            .sum();
        if mean_insolation > 0.0 {
            s.planetary_albedo /= mean_insolation;
        }
        s
    }

    /// Keeps the climate consistent after the terrain changed (terraforming,
    /// sea level). Cells that became ocean take their soil water and snow into
    /// the sea; cells that became land turn their sea ice into snow. Rivers
    /// are re-routed.
    pub fn terrain_changed(
        &mut self,
        grid: &GeodesicGrid,
        planet: &PlanetParams,
        before: &Terrain,
        terrain: &Terrain,
    ) {
        // Air temperature follows height: a cell lifted by Δh is Γ·Δh colder
        // (its sea-level-equivalent temperature is unchanged), so new
        // mountains are cold and new basins warm without inventing heat.
        let lapse = self.params.lapse_rate * planet.body.gravity() / EARTH_GRAVITY;
        let land_height = |t: &Terrain, c: CellId| {
            if t.is_ocean(c) {
                0.0
            } else {
                t.height_above_sea(c).max(0.0)
            }
        };
        let was_ocean = before.ocean_mask();
        for c in grid.cells() {
            let i = c as usize;
            let area = self.geo.area[i];
            self.temperature[i] -= lapse * (land_height(terrain, c) - land_height(before, c));
            let now_ocean = terrain.is_ocean(c);
            if now_ocean && !was_ocean[i] {
                self.ocean_water +=
                    (self.soil[i] + self.snow[i] + self.runoff_since_routing[i]) * area;
                self.soil[i] = 0.0;
                self.snow[i] = 0.0;
                self.runoff_since_routing[i] = 0.0;
                self.lake[i] = false;
                self.temperature[i] = self.temperature[i].max(FREEZING_SEA);
            } else if !now_ocean && was_ocean[i] {
                self.snow[i] += self.sea_ice[i] * ICE_DENSITY;
                self.sea_ice[i] = 0.0;
            }
        }
        // Re-diagnose last year's climate zones where the ground moved, using
        // the same lapse-rate shift, so the zone map responds at once.
        if let Some(year) = &mut self.stats.last_year {
            for c in grid.cells() {
                let i = c as usize;
                let dh = land_height(terrain, c) - land_height(before, c);
                if dh != 0.0 || was_ocean[i] != terrain.is_ocean(c) {
                    for t in year.temperature[i].iter_mut() {
                        *t -= (lapse * dh) as f32;
                    }
                    year.zones[i] = stats::classify(
                        &year.temperature[i],
                        &year.precipitation[i],
                        terrain.is_ocean(c),
                    );
                }
            }
        }
        self.drainage = Drainage::build(grid, terrain);
    }
}

/// Open-water evaporation rate per cell (kg m⁻² s⁻¹) at current conditions,
/// used to decide whether basins can hold lakes.
fn open_water_evaporation(c: &Climate, pressure: f64) -> Vec<f64> {
    (0..c.temperature.len())
        .map(|i| {
            let t = c.temperature[i];
            let rho = pressure / (R_AIR * t);
            rho * c.params.evaporation_coefficient
                * c.wind[i].length().max(3.0)
                * saturation_specific_humidity(t, pressure)
                * 0.3
        })
        .collect()
}

/// A rough annual-mean temperature per cell from local radiative balance
/// (including the water-vapour feedback), blended toward the global mean to
/// stand in for heat transport. Gives the climate a head start so it settles
/// within a few years.
fn initial_temperature(grid: &GeodesicGrid, planet: &PlanetParams) -> Vec<f64> {
    let g = planet.body.gravity();
    let year = planet.year_seconds();
    let samples = 24;
    let equilibrium = |q: f64| {
        // Fixed point of T = (Q(1 − α)(1 + ¾τ(T))/σ)^¼ with τ from saturated-ish air.
        let mut t: f64 = 280.0;
        for _ in 0..30 {
            let w = 0.6 * saturated_column_water(t, g);
            let tau = optical_depth(&planet.atmosphere, w, 0.64);
            let next =
                (q * 0.69 * (1.0 + 0.75 * tau) / dirtbox_core::consts::STEFAN_BOLTZMANN).powf(0.25);
            // Capped: a starting guess must not begin inside a runaway greenhouse.
            t = 0.5 * t + 0.5 * next.clamp(40.0, 320.0);
        }
        t
    };
    let local: Vec<f64> = grid
        .cells()
        .map(|c| {
            let pos = grid.position(c);
            let q = (0..samples)
                .map(|k| {
                    let sun = planet.sun_state(year * (k as f64 + 0.5) / samples as f64);
                    if planet.rotation.tidally_locked {
                        instantaneous_insolation(&sun, pos)
                    } else {
                        daily_mean_insolation(sun.flux, pos.latitude(), sun.declination)
                    }
                })
                .sum::<f64>()
                / samples as f64;
            equilibrium(q)
        })
        .collect();
    let total: f64 = grid.cells().map(|c| grid.area(c)).sum();
    let mean: f64 = grid
        .cells()
        .map(|c| local[c as usize] * grid.area(c))
        .sum::<f64>()
        / total;
    local
        .iter()
        .map(|&t| (0.5 * t + 0.5 * mean).max(40.0))
        .collect()
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Brings a new climate close to equilibrium quickly: runs `years` planet
/// years on a coarse grid (daily-mean steps), then maps the result onto the
/// full grid, correcting temperatures for the height differences. The
/// returned climate corresponds to planet time `years × year length`, so use a
/// whole number of years to land back on the starting season.
///
/// `progress` is called with the fraction done (0–1).
pub fn spin_up(
    grid: &GeodesicGrid,
    planet: &PlanetParams,
    terrain: &Terrain,
    params: ClimateParams,
    frozen: bool,
    years: f64,
    progress: &mut dyn FnMut(f64),
) -> Climate {
    const COARSE: u32 = 16;
    let mut fine = Climate::new(grid, planet, terrain, params.clone(), frozen);
    if years <= 0.0 {
        progress(1.0);
        return fine;
    }
    let coarse_grid = GeodesicGrid::new(COARSE.min(grid.subdivision()));
    // Coarse terrain: the mean height of the fine cells that fall in each coarse cell.
    let mut sum = vec![0.0; coarse_grid.cell_count()];
    let mut count = vec![0usize; coarse_grid.cell_count()];
    let mut owner = vec![0 as CellId; grid.cell_count()];
    let mut hint = 0;
    for c in grid.cells() {
        let k = coarse_grid.locate_from(hint, grid.position(c));
        hint = k;
        owner[c as usize] = k;
        sum[k as usize] += terrain.elevation[c as usize];
        count[k as usize] += 1;
    }
    let coarse_elevation: Vec<f64> = coarse_grid
        .cells()
        .map(|k| {
            let i = k as usize;
            if count[i] > 0 {
                sum[i] / count[i] as f64
            } else {
                terrain.elevation[grid.locate(coarse_grid.position(k)) as usize]
            }
        })
        .collect();
    let coarse_terrain =
        Terrain::from_elevation(&coarse_grid, coarse_elevation, terrain.sea_level());
    let mut coarse = Climate::new(&coarse_grid, planet, &coarse_terrain, params, frozen);

    let dt = planet.calendar().day_length.min(86_400.0);
    let steps = (years * planet.year_seconds() / dt).round().max(1.0) as usize;
    let mut t = 0.0;
    for s in 0..steps {
        coarse.step(&StepInputs {
            grid: &coarse_grid,
            planet,
            terrain: &coarse_terrain,
            start: t,
            dt,
            mode: StepMode::DailyMean,
            geologic_rate: 0.0,
        });
        t += dt;
        if s % 16 == 0 {
            progress(s as f64 / steps as f64);
        }
    }

    // Map onto the fine grid.
    let g = planet.body.gravity();
    let lapse = fine.params.lapse_rate * g / EARTH_GRAVITY;
    let land_height = |t: &Terrain, c: CellId| {
        if t.is_ocean(c) {
            0.0
        } else {
            t.height_above_sea(c).max(0.0)
        }
    };
    for c in grid.cells() {
        let i = c as usize;
        let k = owner[i];
        let ki = k as usize;
        let dh = land_height(terrain, c) - land_height(&coarse_terrain, k);
        let t_new = coarse.temperature[ki] - lapse * dh;
        fine.temperature[i] = t_new;
        fine.vapour[i] = coarse.vapour[ki].min(saturated_column_water(t_new, g));
        let fine_ocean = terrain.is_ocean(c);
        let coarse_ocean = coarse_terrain.is_ocean(k);
        if fine_ocean {
            fine.sea_ice[i] = if coarse_ocean {
                coarse.sea_ice[ki]
            } else if t_new < FREEZING_SEA {
                1.0
            } else {
                0.0
            };
            fine.temperature[i] = if fine.sea_ice[i] >= fine.params.ice_cover_thickness {
                t_new.min(FREEZING_FRESH)
            } else {
                t_new.max(FREEZING_SEA)
            };
        } else {
            fine.snow[i] = if coarse_ocean { 0.0 } else { coarse.snow[ki] };
            fine.soil[i] = if coarse_ocean {
                0.5 * fine.params.soil_capacity
            } else {
                coarse.soil[ki]
            };
        }
        fine.wind[i] = coarse.wind[ki];
        fine.cloud[i] = coarse.cloud[ki];
        fine.precipitation[i] = coarse.precipitation[ki];
    }
    // Carry the last simulated year over too, so yearly statistics and
    // climate zones are available from the start.
    if let Some(year) = &coarse.stats.last_year {
        let temperature: Vec<[f32; 12]> = grid
            .cells()
            .map(|c| {
                let k = owner[c as usize];
                let dh = land_height(terrain, c) - land_height(&coarse_terrain, k);
                year.temperature[k as usize].map(|t| t - (lapse * dh) as f32)
            })
            .collect();
        let precipitation: Vec<[f32; 12]> = grid
            .cells()
            .map(|c| year.precipitation[owner[c as usize] as usize])
            .collect();
        let zones = grid
            .cells()
            .map(|c| {
                let i = c as usize;
                stats::classify(&temperature[i], &precipitation[i], terrain.is_ocean(c))
            })
            .collect();
        fine.stats.last_year = Some(YearSummary {
            temperature,
            precipitation,
            zones,
        });
    }
    // The mapped state defines the new water baseline.
    fine.ocean_water = 0.0;
    progress(1.0);
    fine
}

impl Climate {
    /// Rebuilds the physical geometry for a new planet radius. Per-area
    /// quantities (temperature, water columns) are kept.
    pub fn set_radius(&mut self, grid: &GeodesicGrid, radius: f64) {
        self.radius = radius;
        self.geo = Geometry::new(grid, radius);
    }
}

#[cfg(test)]
mod spin_up_tests {
    use super::*;
    use dirtbox_planet::Preset;

    #[test]
    fn spin_up_lands_near_the_coarse_equilibrium() {
        let planet = Preset::EarthModern.params();
        let grid = GeodesicGrid::new(24);
        let terrain = Terrain::generate(&grid, &planet, 1);
        let mut calls = 0;
        let climate = spin_up(
            &grid,
            &planet,
            &terrain,
            ClimateParams::default(),
            false,
            4.0,
            &mut |_| calls += 1,
        );
        assert!(calls > 1);
        let mean = climate
            .temperature
            .iter()
            .zip(grid.cells())
            .map(|(t, c)| t * grid.area(c))
            .sum::<f64>()
            / (4.0 * std::f64::consts::PI);
        assert!((280.0..295.0).contains(&mean), "{mean}");
        assert!(climate.temperature.iter().all(|t| t.is_finite()));
        let year = climate
            .stats
            .last_year
            .as_ref()
            .expect("year statistics carried over");
        assert_eq!(year.zones.len(), grid.cell_count());
    }
}
