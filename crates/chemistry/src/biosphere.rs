//! The planet's microbial biosphere: per-cell pools and strain fields, the
//! abiotic chemistry around them, dispersal, and the atmosphere they share.

use rayon::prelude::*;

use dirtbox_climate::Climate;
use dirtbox_climate::transport::Geometry;
use dirtbox_core::Rng;
use dirtbox_grid::{CellId, GeodesicGrid};
use dirtbox_planet::astronomy::{daily_mean_insolation, instantaneous_insolation};
use dirtbox_planet::{PlanetParams, Terrain};

use crate::events::{LifeEvent, Milestone};
use crate::genome::{Genome, MODULE_COUNT, Module};
use crate::metabolism::{
    self, CO2, CellEnv, DAY, H2, MIN_BIOMASS, N_PER_C, NIT, O2, O2_SATURATION, O2_TODAY, OM,
    P_PER_C, PHOS, POOLS, RED, SLOTS, YEAR,
};
use crate::params::{LifeParams, MutationRates};
use crate::registry::{EMPTY, Registry, StrainId, StrainRecord};

const LIFE_DOMAIN: u64 = 0x11FE;
/// A module runs well enough to count toward a milestone, 0–1.
const ESTABLISHED_EFFICIENCY: f64 = 0.2;
/// Living carbon on land that counts as colonised, mol C (≈1 g C m⁻² over
/// Earth's land).
const LAND_COLONISED: f64 = 1.0e13;
/// Mean molar mass of air used to convert moles to partial pressure, kg mol⁻¹.
/// Using one constant keeps the conversion linear, so moles are conserved
/// exactly; for air that isn't mostly N₂/O₂ it is an approximation.
pub const AIR_MOLAR_MASS: f64 = 0.028_97;
/// Grids smaller than this run the per-cell work on one thread.
const PARALLEL_MIN: usize = 2_048;
/// Immigrants smaller than this die on arrival, mol C m⁻².
const MIN_IMMIGRANT: f64 = 1.0e-8;
/// Most a strain's mobility multiplies dispersal by.
const MAX_MOBILITY: f64 = 3.0;

// Abiotic sources, mol m⁻² per year.
/// Organics made by UV light and lightning in an oxygen-free sky, at Earth's light.
const UV_ORGANICS: f64 = 3.0e-4;
const LIGHTNING_NITROGEN: f64 = 3.0e-4;
/// At full vent strength.
const VENT_H2: f64 = 0.3;
const VENT_REDUCTANT: f64 = 1.0;
const VENT_ORGANICS: f64 = 0.1;
const VENT_NITROGEN: f64 = 0.02;
const VENT_PHOSPHORUS: f64 = 0.002;
/// Dissolved iron welling up from an anoxic deep ocean, at Earth's tectonics.
const UPWELLING_REDUCTANT: f64 = 0.02;
/// Rock weathering on wet land.
const WEATHERING_PHOSPHORUS: f64 = 5.0e-4;
/// Reducing volcanic gases that consume atmospheric O₂, mol per year at
/// Earth's tectonics.
const VOLCANIC_O2_SINK: f64 = 2.0e12;

// Abiotic sinks.
const REDUCTANT_OXIDATION_DAYS: f64 = 10.0;
const REDUCTANT_MINERAL_YEARS: f64 = 50.0;
const ORGANIC_OXIDATION_PER_YEAR: f64 = 0.2;
const ORGANIC_DECAY_PER_YEAR: f64 = 0.05;
const ORGANIC_BURIAL_PER_YEAR: f64 = 0.002;
const H2_ESCAPE_DAYS: f64 = 30.0;
/// Gas exchange across the sea surface, m per day.
const PISTON_VELOCITY: f64 = 4.0;
const LEACH_PER_YEAR: f64 = 0.2;
const ORGANIC_LEACH_PER_YEAR: f64 = 0.02;
/// Methane lifetime in today's air, years (much longer without oxygen).
const CH4_LIFETIME: f64 = 12.0;

/// Phosphate in the deep ocean at the start, mol m⁻³ (as on Earth).
const DEEP_P: f64 = 2.2e-3;
/// Mean depth of the ocean below the mixed layer, m.
const DEEP_DEPTH: f64 = 3_700.0;

/// Everything the biosphere reads from the rest of the world.
pub struct Environment<'a> {
    pub grid: &'a GeodesicGrid,
    pub terrain: &'a Terrain,
    pub climate: &'a Climate,
    pub planet: &'a PlanetParams,
    /// Planet time at the start of the step, s.
    pub start: f64,
}

/// Change in the atmosphere's gases, mol.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AtmosphereChange {
    pub co2: f64,
    pub ch4: f64,
    pub o2: f64,
    pub n2: f64,
}

impl AtmosphereChange {
    pub fn add(&mut self, o: &AtmosphereChange) {
        self.co2 += o.co2;
        self.ch4 += o.ch4;
        self.o2 += o.o2;
        self.n2 += o.n2;
    }

    /// Applies the change to the planet's partial pressures.
    pub fn apply(&self, planet: &mut PlanetParams) {
        let k = 1.0 / moles_per_bar(planet);
        let a = &mut planet.atmosphere;
        a.co2 = (a.co2 + self.co2 * k).max(0.0);
        a.ch4 = (a.ch4 + self.ch4 * k).max(0.0);
        a.o2 = (a.o2 + self.o2 * k).max(0.0);
        a.n2 = (a.n2 + self.n2 * k).max(0.0);
    }
}

/// Moles of gas per bar of partial pressure on this planet.
pub fn moles_per_bar(planet: &PlanetParams) -> f64 {
    1.0e5 * planet.body.surface_area() / (planet.body.gravity() * AIR_MOLAR_MASS)
}

/// Global numbers after each step.
#[derive(Clone, Debug, Default)]
pub struct LifeStats {
    /// Living biomass, mol C.
    pub biomass: f64,
    pub ocean_biomass: f64,
    pub land_biomass: f64,
    /// Biomass of strains carrying each module, mol C.
    pub module_biomass: [f64; MODULE_COUNT],
    /// Biomass of strains that run each module well (efficiency ≥ 0.2),
    /// mol C: a module counts as established once these are common.
    pub module_established: [f64; MODULE_COUNT],
    /// Biomass of complex (endosymbiotic) cells, mol C.
    pub complex_biomass: f64,
    /// Strains alive now.
    pub strains: usize,
    /// Cells with any life.
    pub occupied_cells: usize,
    /// Carbon fixed by autotrophs, mol C per planet year (last step's rate).
    pub productivity: f64,
    /// Organic carbon dissolved in water and soil, mol C.
    pub organics: f64,
    /// Carbon buried in sediments so far, mol C.
    pub buried: f64,
}

pub struct Biosphere {
    pub params: LifeParams,
    seed: u64,
    steps: u64,
    /// Strain slot ids and biomass (mol C m⁻²), `SLOTS` per cell.
    ids: Vec<StrainId>,
    bio: Vec<f64>,
    /// Dissolved pools per cell, mol m⁻² (see `metabolism::OM` etc.).
    pools: Vec<[f64; POOLS]>,
    /// Nutrients in the deep ocean, mol.
    deep_n: f64,
    deep_p: f64,
    buried_c: f64,
    registry: Registry,
    /// Atmospheric change not yet handed back (seeding).
    pending: AtmosphereChange,
    stats: LifeStats,
    events: Vec<LifeEvent>,
    reached: Vec<Milestone>,
    extinct: bool,
    /// Planet time at the end of the last step, s.
    time: f64,
    /// Cell latitudes, cached.
    latitudes: Vec<f64>,
    /// Usable light at the surface in the last step, W m⁻².
    light: Vec<f64>,
}

/// What one cell's step hands back to the planet.
#[derive(Default)]
struct CellResult {
    exchange: metabolism::CellExchange,
    /// Gas fluxes into the atmosphere, mol m⁻².
    co2: f64,
    o2: f64,
    n2_fixed_abiotic: f64,
    buried: f64,
}

impl Biosphere {
    /// An empty biosphere with abiotic pools set for the planet's current
    /// atmosphere. Call [`Biosphere::seed`] to start life.
    pub fn new(
        grid: &GeodesicGrid,
        terrain: &Terrain,
        planet: &PlanetParams,
        params: LifeParams,
        seed: u64,
    ) -> Self {
        let n = grid.cell_count();
        let oxygen = planet.atmosphere.o2 / O2_TODAY;
        let anoxic = oxygen < 0.01;
        // Without oxygen, little nitrate builds up: N is scarce until
        // something learns to fix it.
        let n_ratio = 16.0 * if anoxic { 0.25 } else { 1.0 };
        let o2_column = O2_SATURATION * params.mixed_layer * oxygen;
        let surface_p = DEEP_P * params.mixed_layer;
        let pools = (0..n)
            .map(|i| {
                let mut p = [0.0; POOLS];
                if terrain.ocean_mask()[i] {
                    p[RED] = if anoxic { 1.0 } else { 0.0 };
                    p[O2] = o2_column;
                    p[PHOS] = surface_p;
                    p[NIT] = surface_p * n_ratio;
                } else if anoxic {
                    // Raw rock: almost nothing for roots yet.
                    p[PHOS] = 0.005;
                    p[NIT] = 0.01;
                } else {
                    // An oxygenated world is assumed to have soils built by a
                    // long history of life.
                    p[PHOS] = 0.2;
                    p[NIT] = 3.0;
                }
                p
            })
            .collect();
        let r = planet.body.radius();
        let ocean_area: f64 = grid
            .cells()
            .filter(|&c| terrain.is_ocean(c))
            .map(|c| grid.area(c) * r * r)
            .sum();
        Self {
            params,
            seed,
            steps: 0,
            ids: vec![EMPTY; n * SLOTS],
            bio: vec![0.0; n * SLOTS],
            pools,
            deep_n: DEEP_P * DEEP_DEPTH * n_ratio * ocean_area,
            deep_p: DEEP_P * DEEP_DEPTH * ocean_area,
            buried_c: 0.0,
            registry: Registry::default(),
            pending: AtmosphereChange::default(),
            stats: LifeStats::default(),
            events: Vec::new(),
            reached: Vec::new(),
            extinct: false,
            time: 0.0,
            latitudes: Vec::new(),
            light: Vec::new(),
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn stats(&self) -> &LifeStats {
        &self.stats
    }

    pub fn events(&self) -> &[LifeEvent] {
        &self.events
    }

    pub fn has_reached(&self, m: Milestone) -> bool {
        self.reached.contains(&m)
    }

    pub fn is_alive(&self) -> bool {
        self.bio.iter().any(|&b| b > 0.0)
    }

    pub fn cell_count(&self) -> usize {
        self.pools.len()
    }

    /// A dissolved pool in one cell, mol m⁻² (index from `metabolism`).
    pub fn pool(&self, cell: CellId, pool: usize) -> f64 {
        self.pools[cell as usize][pool]
    }

    /// Mutable access to every cell's pools, for producers that share them
    /// (roots taking up nutrients, litter returning them).
    pub fn pools_mut(&mut self) -> &mut [[f64; POOLS]] {
        &mut self.pools
    }

    /// Usable light at each cell's surface in the last step (daily mean),
    /// W m⁻². Empty before the first step.
    pub fn light(&self) -> &[f64] {
        &self.light
    }

    /// Removes up to `amount` mol C m⁻² of strain `id` from a cell (with the
    /// nutrients it holds) and returns how much was taken. For colonies that
    /// leave the strain field to become multicellular organisms.
    pub fn take_biomass(&mut self, cell: CellId, id: StrainId, amount: f64) -> f64 {
        let base = cell as usize * SLOTS;
        let Some(k) = (base..base + SLOTS).find(|&k| self.ids[k] == id) else {
            return 0.0;
        };
        let taken = amount.min(self.bio[k]).max(0.0);
        self.bio[k] -= taken;
        if self.bio[k] < MIN_BIOMASS {
            let rest = self.bio[k].max(0.0);
            let p = &mut self.pools[cell as usize];
            p[OM] += rest;
            p[NIT] += rest * N_PER_C;
            p[PHOS] += rest * P_PER_C;
            self.bio[k] = 0.0;
            self.ids[k] = EMPTY;
        }
        taken
    }

    /// Records a milestone reached outside the biosphere (e.g. by producers).
    pub fn record_milestone(&mut self, m: Milestone, time: f64) {
        self.record(m, time);
    }

    /// The strains living in a cell with their biomass (mol C m⁻²),
    /// largest first.
    pub fn strains_in(&self, cell: CellId) -> Vec<(StrainId, f64)> {
        let base = cell as usize * SLOTS;
        let mut v: Vec<(StrainId, f64)> = (base..base + SLOTS)
            .filter(|&k| self.ids[k] != EMPTY)
            .map(|k| (self.ids[k], self.bio[k]))
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    /// Living biomass in a cell, mol C m⁻².
    pub fn biomass(&self, cell: CellId) -> f64 {
        let base = cell as usize * SLOTS;
        self.bio[base..base + SLOTS].iter().sum()
    }

    /// The strain with the most biomass in a cell.
    pub fn dominant(&self, cell: CellId) -> Option<StrainId> {
        self.strains_in(cell).first().map(|s| s.0)
    }

    /// Shannon diversity (effective number of strains, e^H) in a cell.
    pub fn diversity(&self, cell: CellId) -> f64 {
        let s = self.strains_in(cell);
        let total: f64 = s.iter().map(|x| x.1).sum();
        if total <= 0.0 {
            return 0.0;
        }
        let h: f64 = s
            .iter()
            .filter(|x| x.1 > 0.0)
            .map(|x| {
                let p = x.1 / total;
                -p * p.ln()
            })
            .sum();
        h.exp()
    }

    /// Organic carbon, biomass and buried carbon on the planet, mol C. With
    /// atmospheric CO₂ and CH₄ this is conserved.
    pub fn carbon_inventory(&self, geo: &Geometry) -> f64 {
        let cells: f64 = (0..self.pools.len())
            .map(|i| {
                (self.pools[i][OM] + self.bio[i * SLOTS..(i + 1) * SLOTS].iter().sum::<f64>())
                    * geo.area[i]
            })
            .sum();
        cells + self.buried_c
    }

    /// Atmospheric change not yet handed back (from seeding); the caller
    /// applies it to the planet.
    pub fn take_pending(&mut self) -> AtmosphereChange {
        std::mem::take(&mut self.pending)
    }

    /// Feeds every value that defines the biosphere's state to `write`, for
    /// replay checks.
    pub fn hash_words(&self, write: &mut dyn FnMut(u64)) {
        write(self.steps);
        for (&id, &b) in self.ids.iter().zip(&self.bio) {
            write(id as u64);
            write(b.to_bits());
        }
        for p in &self.pools {
            for v in p {
                write(v.to_bits());
            }
        }
        write(self.deep_n.to_bits());
        write(self.deep_p.to_bits());
        write(self.buried_c.to_bits());
        write(self.registry.total_ever());
    }

    /// Seeds base bacteria at the strongest vents (or, on a world without
    /// vents, the warmest ocean). Returns the cells seeded.
    pub fn seed(
        &mut self,
        grid: &GeodesicGrid,
        terrain: &Terrain,
        climate: &Climate,
        now: f64,
    ) -> Vec<CellId> {
        let mut cells: Vec<CellId> = grid.cells().filter(|&c| terrain.is_ocean(c)).collect();
        if cells.is_empty() {
            cells = grid.cells().collect();
        }
        let score =
            |c: CellId| terrain.vents[c as usize] * 1e3 + climate.temperature[c as usize] * 1e-3;
        cells.sort_by(|&a, &b| score(b).total_cmp(&score(a)).then(a.cmp(&b)));
        cells.truncate(self.params.seed_sites);
        let id = self.registry.insert(Genome::base_bacterium(), None, now);
        for &c in &cells {
            self.seed_at(climate.geometry(), c, id);
        }
        self.census(grid, climate.geometry(), terrain);
        self.record(Milestone::LifeBegins, now);
        self.extinct = false;
        cells
    }

    /// Introduces a population of any genome into a cell (for experiments and
    /// the player's seeding tools). Returns its id.
    pub fn introduce(
        &mut self,
        geo: &Geometry,
        cell: CellId,
        genome: Genome,
        biomass: f64,
        now: f64,
    ) -> StrainId {
        let id = self.registry.insert(genome, None, now);
        let b = biomass;
        let c = cell as usize;
        self.place(geo, c, id, b);
        self.pending.co2 -= b * geo.area[c];
        self.pools[c][NIT] -= b * N_PER_C;
        self.pools[c][PHOS] -= b * P_PER_C;
        id
    }

    /// Places a seed population of `id` in one cell, drawing its carbon and
    /// nutrients from the air and the cell's water.
    fn seed_at(&mut self, geo: &Geometry, cell: CellId, id: StrainId) {
        let b = self.params.seed_biomass;
        let c = cell as usize;
        self.place(geo, c, id, b);
        self.pending.co2 -= b * geo.area[c];
        let p = &mut self.pools[c];
        p[NIT] -= b * N_PER_C;
        p[PHOS] -= b * P_PER_C;
    }

    /// Adds `amount` of strain `id` to cell `c`, making room if needed: the
    /// smallest resident (or the newcomer, if it is smaller) is merged into
    /// its nearest relative in the cell.
    fn place(&mut self, _geo: &Geometry, c: usize, id: StrainId, amount: f64) {
        let base = c * SLOTS;
        let slots = base..base + SLOTS;
        if let Some(k) = slots.clone().find(|&k| self.ids[k] == id) {
            self.bio[k] += amount;
            return;
        }
        if let Some(k) = slots.clone().find(|&k| self.ids[k] == EMPTY) {
            self.ids[k] = id;
            self.bio[k] = amount;
            return;
        }
        let smallest = slots
            .clone()
            .min_by(|&a, &b| self.bio[a].total_cmp(&self.bio[b]).then(a.cmp(&b)))
            .unwrap();
        let reg = &self.registry;
        let nearest = |g: &Genome, skip: usize| {
            slots
                .clone()
                .filter(|&k| k != skip)
                .min_by(|&a, &b| {
                    g.distance(reg.genome(self.ids[a]))
                        .total_cmp(&g.distance(reg.genome(self.ids[b])))
                        .then(a.cmp(&b))
                })
                .unwrap()
        };
        if amount > self.bio[smallest] {
            let k = nearest(reg.genome(self.ids[smallest]), smallest);
            self.bio[k] += self.bio[smallest];
            self.ids[smallest] = id;
            self.bio[smallest] = amount;
        } else {
            let k = nearest(reg.genome(id), usize::MAX);
            self.bio[k] += amount;
        }
    }

    /// Usable light at the surface of each cell, W m⁻² (daily mean on a
    /// spinning planet).
    fn surface_light(&mut self, env: &Environment, dt: f64) -> Vec<f64> {
        let planet = env.planet;
        let sun = planet.sun_state(env.start + dt / 2.0);
        if self.latitudes.len() != env.grid.cell_count() {
            self.latitudes = env
                .grid
                .cells()
                .map(|c| env.grid.position(c).latitude())
                .collect();
        }
        let latitudes = &self.latitudes;
        let slow = planet.rotation.tidally_locked
            || planet.solar_day_seconds().is_none_or(|d| d > 4.0 * DAY);
        let par = planet.star.par_fraction();
        let c = env.climate;
        env.grid
            .cells()
            .map(|cell| {
                let i = cell as usize;
                let pos = env.grid.position(cell);
                let top = if slow {
                    instantaneous_insolation(&sun, pos)
                } else {
                    daily_mean_insolation(sun.flux, latitudes[i], sun.declination)
                };
                let mut light = top * 0.75 * (1.0 - 0.5 * c.cloud[i]) * par;
                if env.terrain.is_ocean(cell) {
                    if c.is_ice_covered(cell) {
                        light *= 0.05;
                    }
                } else if c.snow[i] > 20.0 {
                    light *= 0.1;
                }
                light
            })
            .collect()
    }

    /// Wetness each cell offers microbes, 0–1.
    fn wetness(env: &Environment) -> Vec<f64> {
        let c = env.climate;
        env.grid
            .cells()
            .map(|cell| {
                let i = cell as usize;
                if env.terrain.is_ocean(cell) || c.lake[i] {
                    1.0
                } else {
                    let w = (c.soil[i] / c.params.soil_capacity).clamp(0.0, 1.0);
                    if c.temperature[i] < 271.15 {
                        w * 0.05
                    } else {
                        w
                    }
                }
            })
            .collect()
    }

    /// Advances the biosphere by `dt` planet seconds, which stand for
    /// `geologic_years` of evolution. Returns the change to the atmosphere.
    pub fn step(
        &mut self,
        env: &Environment,
        dt: f64,
        geologic_years: f64,
        rates: &MutationRates,
    ) -> AtmosphereChange {
        let n = self.pools.len();
        assert_eq!(n, env.grid.cell_count(), "biosphere and grid disagree");
        let geo = env.climate.geometry();
        let planet = env.planet;
        let atm = &planet.atmosphere;
        let mpb = moles_per_bar(planet);
        let planet_area: f64 = geo.area.iter().sum();
        let co2_share = 0.5 * atm.co2 * mpb / planet_area;
        let o2_share = 0.5 * atm.o2 * mpb / planet_area;
        let air_oxygen = atm.o2 / O2_TODAY;
        let co2_factor = atm.co2 / (atm.co2 + 5.0e-5);
        let years = dt / YEAR;
        let days = dt / DAY;
        let ocean = env.terrain.ocean_mask();
        let ocean_area: f64 = (0..n).filter(|&i| ocean[i]).map(|i| geo.area[i]).sum();
        let light = self.surface_light(env, dt);
        self.light.clone_from(&light);
        let wet = Self::wetness(env);
        let mixed = self.params.mixed_layer;
        let o2_saturated = O2_SATURATION * mixed * air_oxygen;
        let exchange = 1.0 - (-PISTON_VELOCITY * days / mixed).exp();
        let uv_shield = 1.0 / (1.0 + air_oxygen / 0.01);
        let deep_oxic = 1.0 / (1.0 + air_oxygen / 0.05);
        let tectonics = planet.terrain.tectonic_activity;
        let n2_factor = (atm.n2 / 0.78).min(2.0);

        // The overturning deep ocean trades nutrients with the surface,
        // returning what sank.
        if ocean_area > 0.0 {
            let volume = ocean_area * DEEP_DEPTH;
            let (cn, cp) = (self.deep_n / volume, self.deep_p / volume);
            let r = 1.0 - (-years * DEEP_DEPTH / self.params.overturn_years / mixed).exp();
            for (i, p) in self.pools.iter_mut().enumerate() {
                if ocean[i] {
                    let dn = (cn * mixed - p[NIT]) * r;
                    let dp = (cp * mixed - p[PHOS]) * r;
                    p[NIT] += dn;
                    p[PHOS] += dp;
                    self.deep_n -= dn * geo.area[i];
                    self.deep_p -= dp * geo.area[i];
                }
            }
        }

        // --- Every cell: life, then abiotic chemistry ---------------------
        let registry = &self.registry;
        let (seed, tick) = (self.seed, self.steps);
        let climate = env.climate;
        let vents = &env.terrain.vents;
        let run_cell = |i: usize,
                        ids: &mut [StrainId],
                        bio: &mut [f64],
                        stored: &mut [f64; POOLS]|
         -> CellResult {
            let is_ocean = ocean[i];
            let oxygen = if is_ocean {
                stored[O2] / (O2_SATURATION * mixed)
            } else {
                air_oxygen
            };
            let cell_env = CellEnv {
                ocean: is_ocean,
                temperature: if is_ocean {
                    climate.temperature[i].max(271.4)
                } else {
                    climate.temperature[i]
                },
                wetness: wet[i],
                light: light[i],
                vent: vents[i],
                oxygen,
                co2_factor,
                dt,
                geologic_years,
            };
            let mut w = metabolism::working_pools(stored, co2_share);
            if !is_ocean {
                w[O2] = o2_share;
            }
            let o2_before = w[O2];
            let mut rng = Rng::stream(seed, LIFE_DOMAIN, i as u64, tick);
            let mut out = CellResult {
                exchange: metabolism::step_cell(
                    registry, &cell_env, ids, bio, &mut w, rates, &mut rng,
                ),
                ..Default::default()
            };

            // Sources.
            let uv = UV_ORGANICS * years * uv_shield * light[i] / 60.0;
            let vent = vents[i];
            let organics = (uv + VENT_ORGANICS * vent * years).min(0.5 * w[CO2].max(0.0));
            w[OM] += organics;
            w[CO2] -= organics;
            let lightning = LIGHTNING_NITROGEN * n2_factor * years;
            w[NIT] += lightning;
            out.n2_fixed_abiotic = lightning;
            if is_ocean {
                w[H2] += VENT_H2 * vent * years;
                w[RED] +=
                    (VENT_REDUCTANT * vent + UPWELLING_REDUCTANT * tectonics * deep_oxic) * years;
                w[NIT] += VENT_NITROGEN * vent * years;
                w[PHOS] += VENT_PHOSPHORUS * vent * years;
            } else {
                w[PHOS] += WEATHERING_PHOSPHORUS * wet[i] * years;
            }

            // Reductants burn oxygen quickly, or settle out as minerals.
            let r = w[RED].min(w[O2]).max(0.0) * (1.0 - (-days / REDUCTANT_OXIDATION_DAYS).exp());
            w[RED] -= r;
            w[O2] -= r;
            w[RED] *= (-years / REDUCTANT_MINERAL_YEARS).exp();
            // Organics oxidise slowly in oxygen, decay slowly without it,
            // and some are buried.
            let ox = (w[OM]
                * (1.0 - (-ORGANIC_OXIDATION_PER_YEAR * oxygen.min(1.0) * years).exp()))
            .min(w[O2].max(0.0));
            w[OM] -= ox;
            w[O2] -= ox;
            w[CO2] += ox;
            let decay = w[OM] * (1.0 - (-ORGANIC_DECAY_PER_YEAR * years).exp());
            w[OM] -= decay;
            w[CO2] += decay;
            if is_ocean {
                let buried = w[OM] * (1.0 - (-ORGANIC_BURIAL_PER_YEAR * years).exp());
                w[OM] -= buried;
                out.buried = buried;
            }
            // Hydrogen escapes to the air and on to space.
            let escape_days = if is_ocean { H2_ESCAPE_DAYS } else { 3.0 };
            w[H2] *= (-days / escape_days).exp();

            // Oxygen: the sea surface trades with the air; land breathes it directly.
            if is_ocean {
                let ice = if climate.is_ice_covered(i as CellId) {
                    0.1
                } else {
                    1.0
                };
                let flux = ((w[O2] - o2_saturated) * exchange * ice).max(-o2_share);
                w[O2] -= flux;
                out.o2 = flux;
            } else {
                out.o2 = w[O2] - o2_before;
                w[O2] = 0.0;
                w[RED] = 0.0;
            }
            out.co2 = metabolism::split_pools(&w, stored) - co2_share;
            out
        };

        let results: Vec<CellResult> = if n >= PARALLEL_MIN {
            self.ids
                .par_chunks_mut(SLOTS)
                .zip(self.bio.par_chunks_mut(SLOTS))
                .zip(self.pools.par_iter_mut())
                .enumerate()
                .map(|(i, ((ids, bio), p))| run_cell(i, ids, bio, p))
                .collect()
        } else {
            self.ids
                .chunks_mut(SLOTS)
                .zip(self.bio.chunks_mut(SLOTS))
                .zip(self.pools.iter_mut())
                .enumerate()
                .map(|(i, ((ids, bio), p))| run_cell(i, ids, bio, p))
                .collect()
        };

        // --- Sum the fluxes in cell order (deterministic) ----------------
        let mut change = std::mem::take(&mut self.pending);
        let mut fixed = 0.0;
        let mut spawns = Vec::new();
        for (i, r) in results.into_iter().enumerate() {
            let a = geo.area[i];
            change.co2 += r.co2 * a;
            change.o2 += r.o2 * a;
            change.ch4 += r.exchange.ch4 * a;
            change.n2 -= 0.5 * (r.exchange.n_fixed + r.n2_fixed_abiotic) * a;
            self.deep_n += r.exchange.export_n * a;
            self.deep_p += r.exchange.export_p * a;
            self.buried_c += r.buried * a;
            fixed += r.exchange.fixed * a;
            for s in r.exchange.spawns {
                spawns.push((i, s));
            }
        }

        // --- New strains --------------------------------------------------
        let now = env.start + dt;
        for (i, s) in spawns {
            let parent = self.registry.get(s.parent).map(|r| r.serial);
            let id = self.registry.insert(s.genome, parent, now);
            self.place(geo, i, id, s.biomass);
        }

        self.leach(env, &wet, years);
        self.disperse(geo, ocean, &wet, dt);

        // --- Air chemistry ------------------------------------------------
        let ch4_now = (atm.ch4 * mpb + change.ch4).max(0.0);
        let o2_now = (atm.o2 * mpb + change.o2).max(0.0);
        let lifetime = (CH4_LIFETIME / (air_oxygen + 1e-3).powf(0.7)).min(5_000.0);
        let burned = ch4_now * (1.0 - (-years / lifetime).exp());
        change.ch4 -= burned;
        change.co2 += burned;
        let o2_used = (2.0 * burned).min(0.5 * o2_now);
        change.o2 -= o2_used;
        let volcanic =
            (VOLCANIC_O2_SINK * tectonics * years).min(0.5 * (o2_now - o2_used).max(0.0));
        change.o2 -= volcanic;

        self.steps += 1;
        self.time = now;
        self.census(env.grid, geo, env.terrain);
        self.stats.productivity = fixed / years.max(1e-12);
        self.check_milestones(planet, &change, mpb);
        change
    }

    /// Rivers carry dissolved nutrients and organics from land to the sea.
    fn leach(&mut self, env: &Environment, wet: &[f64], years: f64) {
        let outlets = env.climate.drainage().outlets(env.terrain);
        let geo = env.climate.geometry();
        for (i, outlet) in outlets.iter().enumerate() {
            let Some(o) = *outlet else { continue };
            let o = o as usize;
            let f = 1.0 - (-LEACH_PER_YEAR * wet[i] * years).exp();
            let fo = 1.0 - (-ORGANIC_LEACH_PER_YEAR * wet[i] * years).exp();
            let ratio = geo.area[i] / geo.area[o];
            for (k, frac) in [(NIT, f), (PHOS, f), (OM, fo)] {
                let moved = self.pools[i][k] * frac;
                self.pools[i][k] -= moved;
                self.pools[o][k] += moved * ratio;
            }
        }
    }

    /// Spreads microbes and dissolved pools between neighbouring cells
    /// (explicit diffusion in substeps, gathered per cell so it runs in
    /// parallel and conserves mass exactly).
    fn disperse(&mut self, geo: &Geometry, ocean: &[bool], wet: &[f64], dt: f64) {
        let n = self.pools.len();
        // Per-edge rate (s⁻¹) for the cell that owns the edge.
        let mut strain_rate = vec![0.0; geo.neighbour.len()];
        let mut pool_rate = vec![0.0; geo.neighbour.len()];
        let mut fastest: f64 = 0.0;
        for c in 0..n {
            let mut total = 0.0;
            for e in geo.edges(c) {
                let j = geo.neighbour[e];
                let k = geo.conductance[e] / geo.area[c];
                let d = if ocean[c] && ocean[j] {
                    pool_rate[e] = self.params.ocean_dispersal * k;
                    self.params.ocean_dispersal
                } else {
                    self.params.land_dispersal * wet[c].min(wet[j])
                };
                strain_rate[e] = d * k;
                total += strain_rate[e];
            }
            fastest = fastest.max(total);
        }
        let mobility: Vec<f64> = (0..self.registry.capacity())
            .map(|id| {
                self.registry.get(id as StrainId).map_or(0.0, |r| {
                    let g = &r.genome;
                    ((1.0 + 2.0 * g.motility as f64) * (1.0 - 0.6 * g.adhesion as f64))
                        .min(MAX_MOBILITY)
                })
            })
            .collect();
        let most_mobile = mobility.iter().copied().fold(0.0, f64::max);
        let fastest_pool = (0..n)
            .map(|c| geo.edges(c).map(|e| pool_rate[e]).sum::<f64>())
            .fold(0.0, f64::max);
        // Explicit diffusion keeps everything positive while each cell sends
        // out less than half of itself per substep.
        let substeps = |rate: f64| ((rate * dt) / 0.5).ceil().max(1.0) as usize;

        let pool_steps = substeps(fastest_pool);
        let h = dt / pool_steps as f64;
        for _ in 0..pool_steps {
            let old = self.pools.clone();
            let mix = |c: usize, p: &mut [f64; POOLS]| {
                if !ocean[c] {
                    return;
                }
                for e in geo.edges(c) {
                    let r = pool_rate[e] * h;
                    if r > 0.0 {
                        let j = geo.neighbour[e];
                        for k in 0..POOLS {
                            p[k] += r * (old[j][k] - old[c][k]);
                        }
                    }
                }
            };
            if n >= PARALLEL_MIN {
                self.pools
                    .par_iter_mut()
                    .enumerate()
                    .for_each(|(c, p)| mix(c, p));
            } else {
                self.pools
                    .iter_mut()
                    .enumerate()
                    .for_each(|(c, p)| mix(c, p));
            }
        }

        let registry = &self.registry;
        let strain_steps = substeps(fastest * most_mobile);
        let h = dt / strain_steps as f64;
        for _ in 0..strain_steps {
            let old_ids = self.ids.clone();
            let old_bio = self.bio.clone();
            let occupied: Vec<bool> = old_ids
                .chunks(SLOTS)
                .map(|s| s.iter().any(|&id| id != EMPTY))
                .collect();
            let gather =
                |c: usize, ids: &mut [StrainId], bio: &mut [f64], pool: &mut [f64; POOLS]| {
                    if !occupied[c]
                        && !geo
                            .edges(c)
                            .any(|e| strain_rate[e] > 0.0 && occupied[geo.neighbour[e]])
                    {
                        return;
                    }
                    let mut cand = [(EMPTY, 0.0f64); SLOTS * 7];
                    let mut count = 0;
                    let mut add = |id: StrainId, amount: f64| {
                        if count < cand.len() {
                            cand[count] = (id, amount);
                            count += 1;
                        }
                    };
                    let out_rate: f64 = geo.edges(c).map(|e| strain_rate[e]).sum::<f64>() * h;
                    for k in c * SLOTS..(c + 1) * SLOTS {
                        let id = old_ids[k];
                        if id != EMPTY {
                            add(id, old_bio[k] * (1.0 - out_rate * mobility[id as usize]));
                        }
                    }
                    for e in geo.edges(c) {
                        let r = strain_rate[e] * h;
                        if r <= 0.0 {
                            continue;
                        }
                        let j = geo.neighbour[e];
                        for k in j * SLOTS..(j + 1) * SLOTS {
                            let id = old_ids[k];
                            if id != EMPTY {
                                add(id, old_bio[k] * r * mobility[id as usize]);
                            }
                        }
                    }
                    // Merge entries of the same strain (in a fixed order, so the
                    // sums are deterministic).
                    let cand = &mut cand[..count];
                    cand.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
                    let mut merged = 0;
                    for k in 0..cand.len() {
                        if merged > 0 && cand[merged - 1].0 == cand[k].0 {
                            cand[merged - 1].1 += cand[k].1;
                        } else {
                            cand[merged] = cand[k];
                            merged += 1;
                        }
                    }
                    let cand = &mut cand[..merged];
                    // Keep the K largest. Residents squeezed out merge into their
                    // nearest relative; newcomers that find no room, or are too few
                    // to establish, die on arrival.
                    if cand.len() > SLOTS {
                        cand.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                    }
                    let resident = |id: StrainId| old_ids[c * SLOTS..(c + 1) * SLOTS].contains(&id);
                    let mut kept = 0;
                    ids.fill(EMPTY);
                    bio.fill(0.0);
                    for &(id, amount) in cand.iter() {
                        let viable =
                            amount >= MIN_IMMIGRANT || (amount >= MIN_BIOMASS && resident(id));
                        if kept < SLOTS && viable {
                            ids[kept] = id;
                            bio[kept] = amount;
                            kept += 1;
                        } else if kept > 0 && amount >= MIN_BIOMASS && resident(id) {
                            let g = registry.genome(id);
                            let k = (0..kept)
                                .min_by(|&a, &b| {
                                    g.distance(registry.genome(ids[a]))
                                        .total_cmp(&g.distance(registry.genome(ids[b])))
                                        .then(a.cmp(&b))
                                })
                                .unwrap();
                            bio[k] += amount;
                        } else {
                            let a = amount.max(0.0);
                            pool[OM] += a;
                            pool[NIT] += a * N_PER_C;
                            pool[PHOS] += a * P_PER_C;
                        }
                    }
                };
            if n >= PARALLEL_MIN {
                self.ids
                    .par_chunks_mut(SLOTS)
                    .zip(self.bio.par_chunks_mut(SLOTS))
                    .zip(self.pools.par_iter_mut())
                    .enumerate()
                    .for_each(|(c, ((ids, bio), p))| gather(c, ids, bio, p));
            } else {
                self.ids
                    .chunks_mut(SLOTS)
                    .zip(self.bio.chunks_mut(SLOTS))
                    .zip(self.pools.iter_mut())
                    .enumerate()
                    .for_each(|(c, ((ids, bio), p))| gather(c, ids, bio, p));
            }
        }
    }

    /// Recounts every strain's biomass and range, frees extinct strains and
    /// refreshes the global stats.
    fn census(&mut self, grid: &GeodesicGrid, geo: &Geometry, terrain: &Terrain) {
        let mut totals = vec![(0.0, 0u32); self.registry.capacity()];
        let mut s = LifeStats {
            productivity: self.stats.productivity,
            buried: self.buried_c,
            ..Default::default()
        };
        for c in grid.cells() {
            let i = c as usize;
            let a = geo.area[i];
            let mut cell_total = 0.0;
            for k in i * SLOTS..(i + 1) * SLOTS {
                let id = self.ids[k];
                if id == EMPTY {
                    continue;
                }
                let m = self.bio[k] * a;
                totals[id as usize].0 += m;
                totals[id as usize].1 += 1;
                cell_total += m;
            }
            if cell_total > 0.0 {
                s.occupied_cells += 1;
            }
            s.biomass += cell_total;
            if terrain.is_ocean(c) {
                s.ocean_biomass += cell_total;
            } else {
                s.land_biomass += cell_total;
            }
            s.organics += self.pools[i][OM] * a;
        }
        self.registry.census(&totals);
        for (_, r) in self.registry.iter() {
            for m in Module::ALL {
                if r.genome.has(m) {
                    s.module_biomass[m.index()] += r.biomass;
                }
                if r.genome.efficiency(m) >= ESTABLISHED_EFFICIENCY {
                    s.module_established[m.index()] += r.biomass;
                }
            }
            if r.genome.complex {
                s.complex_biomass += r.biomass;
            }
        }
        s.strains = self.registry.len();
        self.stats = s;
    }

    fn record(&mut self, m: Milestone, time: f64) {
        if !self.reached.contains(&m) {
            self.reached.push(m);
            self.events.push(LifeEvent { time, milestone: m });
        }
    }

    fn check_milestones(&mut self, planet: &PlanetParams, change: &AtmosphereChange, mpb: f64) {
        let t = self.time;
        // Common: at least 10⁹ mol C and 0.5% of all life.
        let established = (1.0e9f64).max(0.005 * self.stats.biomass);
        for m in Module::ALL {
            // Fermentation is where life starts, not a milestone.
            if m != Module::Fermentation && self.stats.module_established[m.index()] >= established
            {
                self.record(Milestone::Metabolism(m), t);
            }
        }
        if self.stats.complex_biomass >= established {
            self.record(Milestone::ComplexCells, t);
        }
        if self.stats.land_biomass >= LAND_COLONISED {
            self.record(Milestone::LandColonised, t);
        }
        let o2 = planet.atmosphere.o2 + change.o2 / mpb;
        if self.has_reached(Milestone::LifeBegins) {
            if o2 >= 1e-3 * O2_TODAY {
                self.record(Milestone::OxygenAppears, t);
            }
            if o2 >= 0.1 * O2_TODAY {
                self.record(Milestone::OxygenRich, t);
            }
        }
        if self.has_reached(Milestone::LifeBegins) && self.stats.biomass <= 0.0 && !self.extinct {
            self.extinct = true;
            self.events.push(LifeEvent {
                time: t,
                milestone: Milestone::Extinction,
            });
        }
    }

    /// Details of one strain.
    pub fn strain(&self, id: StrainId) -> Option<&StrainRecord> {
        self.registry.get(id)
    }

    /// Keeps pools consistent after terraforming: water that became land
    /// loses its dissolved gases; new sea starts without them.
    pub fn terrain_changed(&mut self, terrain: &Terrain) {
        for (i, p) in self.pools.iter_mut().enumerate() {
            if !terrain.ocean_mask()[i] {
                p[O2] = 0.0;
                p[RED] = 0.0;
                p[H2] = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moles_per_bar_matches_earth() {
        let earth = dirtbox_planet::Preset::EarthModern.params();
        let mol = moles_per_bar(&earth);
        // Earth's atmosphere holds ~1.8e20 mol in ~1 bar.
        assert!((mol / 1.78e20 - 1.0).abs() < 0.05, "{mol:e}");
    }
}
