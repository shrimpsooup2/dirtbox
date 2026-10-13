//! Vegetation: producers as cohorts in every cell (design §7.1, §7.6), in the
//! spirit of forest gap models.
//!
//! A cohort is one lineage at one size in one cell: individuals per m², their
//! height, and carbon in leaves, wood, roots and storage. Each year, light
//! filters down through the canopy (I = I₀·e^(−k·LAI above)), and every
//! cohort photosynthesises with the light it gets, limited by temperature,
//! soil water, CO₂ and nutrients. It pays for its tissues' upkeep, grows,
//! gets taller only by building wood (which costs more the stronger the
//! gravity), thins as crowns crowd, sheds litter, dies, and makes seeds
//! that may land nearby or in the next cell. Nothing decides that trees
//! should exist: height pays only where neighbours shade you.
//!
//! **Time-lapse (decision 13).** Plants live for decades, so their demography
//! runs on a faster clock: each planet year stands for `time_lapse` plant
//! years, each one driven by the cell's monthly climate over the last planet
//! year. Litter rots on the same clock, so the carbon cycle stays balanced.

use rayon::prelude::*;

use dirtbox_chemistry::metabolism::{NIT, PHOS, POOLS};
use dirtbox_chemistry::{AtmosphereChange, Biosphere, Environment, Milestone, Module, moles_per_bar};
use dirtbox_climate::transport::Geometry;
use dirtbox_core::Rng;
use dirtbox_core::lineage::{Lineage, LineageId, Lineages};
use dirtbox_grid::CellId;

use crate::plant::{GrowthForm, PlantGenome};

/// Cohorts per cell.
pub const COHORTS: usize = 10;

const VEGETATION_DOMAIN: u64 = 0x7EE5;
const GRADUATION_DOMAIN: u64 = 0x6AD0;
const SECONDS_PER_YEAR: f64 = 365.25 * 86_400.0;

/// Leaf area per carbon in leaves, m² per mol C (≈ 30 g C per m² of leaf).
pub const SLA: f64 = 0.4;
/// Light extinction coefficient of foliage.
const EXTINCTION: f64 = 0.5;
/// Densest foliage a crown can hold, m² leaf per m² crown.
const MAX_CROWN_LAI: f64 = 8.0;
/// Carbon fixed per joule of usable light absorbed, at best (C3-like).
const LIGHT_USE: f64 = 2.7e-7;
/// Light at which leaves are half saturated, W m⁻² of usable light.
const LIGHT_SATURATION: f64 = 150.0;
/// CO₂ partial pressure at which photosynthesis runs at half speed, bar.
const CO2_HALF: f64 = 3.0e-4;
/// Wood needed per individual: COEF·h^3.5 mol C at Earth gravity.
const WOOD_COEF: f64 = 4.0;
const WOOD_EXPONENT: f64 = 3.5;
/// Crown area per individual: CROWN·h^1.5 m².
const CROWN: f64 = 1.5;
/// Upkeep at 20 °C, per year per mol of tissue.
const LEAF_UPKEEP: f64 = 4.0;
const ROOT_UPKEEP: f64 = 1.5;
const WOOD_UPKEEP: f64 = 0.03;
/// Extra carbon burned to build new tissue.
const GROWTH_COST: f64 = 0.25;
/// Carbon per nitrogen in each tissue.
const LEAF_CN: f64 = 25.0;
const ROOT_CN: f64 = 40.0;
const WOOD_CN: f64 = 250.0;
const SEED_CN: f64 = 20.0;
/// Phosphorus per nitrogen.
const P_PER_N: f64 = 1.0 / 15.0;
/// Sugar burned per mol of nitrogen fixed by root partners.
const FIXATION_COST: f64 = 6.0;
/// Nitrogen pulled back from leaves before they fall.
const RESORPTION: f64 = 0.5;
/// Litter decay per year at 20 °C in moist soil.
const LITTER_DECAY: f64 = 0.5;
const LITTER_DECAY_WATER: f64 = 3.0;
/// How far a well-dispersed seed can carry in a plant year, m.
const SEED_RANGE: f64 = 30_000.0;
/// Phosphorus that roots can weather out of rock, mol m⁻² per plant year.
const ROCK_PHOSPHORUS: f64 = 0.01;
/// Microbial biomass N and P per C (Redfield), for graduating colonies.
const MICROBE_N: f64 = 16.0 / 106.0;
const MICROBE_P: f64 = 1.0 / 106.0;

#[derive(Clone, Debug, PartialEq)]
pub struct VegetationParams {
    /// Plant years per planet year (decision 13).
    pub time_lapse: f64,
    /// Chance per reproducing cohort per plant year that some of its seeds
    /// carry a mutation, before the player's multiplier.
    pub mutation_rate: f64,
    /// Chance per colony-forming strain population per geologic year of
    /// becoming a multicellular producer, before the structural multiplier.
    pub graduation_rate: f64,
}

impl Default for VegetationParams {
    fn default() -> Self {
        Self {
            time_lapse: 20.0,
            mutation_rate: 0.05,
            graduation_rate: 0.02,
        }
    }
}

/// One lineage at one size in one cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Cohort {
    pub lineage: LineageId,
    /// Individuals per m² of habitat.
    pub density: f64,
    /// Height, m.
    pub height: f64,
    /// Carbon in each tissue, mol C per m² of cell.
    pub leaf: f64,
    pub wood: f64,
    pub root: f64,
    pub store: f64,
    /// Plant years since the cohort formed.
    pub age: f64,
}

impl Cohort {
    pub fn biomass(&self) -> f64 {
        self.leaf + self.wood + self.root + self.store
    }

    fn nitrogen(&self) -> f64 {
        self.leaf / LEAF_CN + self.root / ROOT_CN + self.wood / WOOD_CN
    }

    fn scale(&mut self, f: f64) {
        self.density *= f;
        self.leaf *= f;
        self.wood *= f;
        self.root *= f;
        self.store *= f;
    }
}

/// A cell's climate in one month of the planet's year.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Month {
    temp: f64,
    wet: f64,
    light: f64,
    seen: bool,
}

#[derive(Clone, Debug, Default)]
pub struct VegetationStats {
    /// Living plant carbon, mol C.
    pub biomass: f64,
    pub land_biomass: f64,
    pub water_biomass: f64,
    /// Biomass by growth form (see [`GrowthForm::ALL`]), mol C.
    pub form_biomass: [f64; 5],
    /// Gross and net production in the last plant year, mol C per plant year.
    pub gpp: f64,
    pub npp: f64,
    /// Dead plant matter, mol C.
    pub litter: f64,
    pub cohorts: usize,
    pub lineages: usize,
    /// Share of land cells with a leaf area index above 0.5.
    pub green_land: f64,
    /// Share of land cells under a canopy ≥ 10 m with LAI ≥ 2.
    pub forest_land: f64,
    /// Tallest cohort anywhere, m.
    pub tallest: f64,
    /// Plant years simulated.
    pub plant_years: u64,
}

/// What happened in one cell during a plant year.
#[derive(Default)]
struct CellOut {
    /// Net carbon to the air, mol m⁻².
    co2: f64,
    /// Nitrogen fixed from N₂, mol m⁻².
    n_fixed: f64,
    gpp: f64,
    npp: f64,
    /// Seeds leaving the cell: (lineage, count m⁻², carbon m⁻²).
    seeds_out: Vec<(LineageId, f64, f64)>,
    /// Seeds staying: same units.
    seeds_local: Vec<(LineageId, f64, f64)>,
    /// Seeds carrying a new mutation: (parent, genome, count, carbon).
    mutants: Vec<(LineageId, PlantGenome, f64, f64)>,
}

/// Read-only conditions for a plant year.
struct YearContext<'a> {
    registry: &'a Lineages<PlantGenome>,
    clim: &'a [[Month; 12]],
    ocean: &'a [bool],
    habitat: &'a [f64],
    /// Share of a cell's dispersing seeds that cross into a neighbour: seeds
    /// travel kilometres, cells are hundreds of kilometres wide.
    crossing: &'a [f64],
    co2_factor: f64,
    co2_cap: f64,
    uv: f64,
    gravity: f64,
    mutation: f64,
    seed: u64,
    tick: u64,
}

pub struct Vegetation {
    pub params: VegetationParams,
    seed: u64,
    cells: Vec<Vec<Cohort>>,
    /// Dead plant matter, mol C m⁻², and the nitrogen in it, mol N m⁻².
    litter: Vec<f64>,
    litter_n: Vec<f64>,
    registry: Lineages<PlantGenome>,
    clim: Vec<[Month; 12]>,
    /// Leaf area index per cell (m² leaf per m² of cell), for the climate.
    lai: Vec<f64>,
    /// Plant years owed to the clock.
    owed: f64,
    /// Geologic years since the last graduation check.
    since_check: f64,
    years: u64,
    pending: AtmosphereChange,
    stats: VegetationStats,
}

fn temperature_fitness(g: &PlantGenome, t: f64) -> f64 {
    let z = (t - g.temp_optimum as f64) / g.temp_tolerance as f64;
    (-0.5 * z * z).exp()
}

/// Soil water a plant can use, 0–1. Roots reach more of it; drought-tolerant
/// plants keep working closer to wilting.
fn water_fitness(g: &PlantGenome, wet: f64, root_ratio: f64) -> f64 {
    let reach = 1.0 - (1.0 - wet.clamp(0.0, 1.0)).powf(1.0 + root_ratio.min(3.0));
    let wilt = 0.02 + 0.25 * (1.0 - g.drought_tolerance as f64);
    ((reach - wilt) / 0.3).clamp(0.0, 1.0)
}

fn height_from_wood(wood_per_individual: f64, coef: f64) -> f64 {
    (wood_per_individual.max(0.0) / coef).powf(1.0 / WOOD_EXPONENT)
}

fn crown_area(height: f64) -> f64 {
    CROWN * height.max(0.002).powf(1.5)
}

/// Wood needed per individual at a height (gravity-scaled; water buoys).
fn wood_coefficient(gravity: f64, ocean: bool) -> f64 {
    WOOD_COEF * gravity * if ocean { 0.1 } else { 1.0 }
}

impl Vegetation {
    pub fn new(cell_count: usize, params: VegetationParams, seed: u64) -> Self {
        Self {
            params,
            seed,
            cells: vec![Vec::new(); cell_count],
            litter: vec![0.0; cell_count],
            litter_n: vec![0.0; cell_count],
            registry: Lineages::default(),
            clim: vec![[Month::default(); 12]; cell_count],
            lai: vec![0.0; cell_count],
            owed: 0.0,
            since_check: 0.0,
            years: 0,
            pending: AtmosphereChange::default(),
            stats: VegetationStats::default(),
        }
    }

    pub fn registry(&self) -> &Lineages<PlantGenome> {
        &self.registry
    }

    pub fn lineage(&self, id: LineageId) -> Option<&Lineage<PlantGenome>> {
        self.registry.get(id)
    }

    pub fn stats(&self) -> &VegetationStats {
        &self.stats
    }

    pub fn cohorts(&self, cell: CellId) -> &[Cohort] {
        &self.cells[cell as usize]
    }

    /// Leaf area index of each cell (m² of leaves per m² of ground).
    pub fn leaf_area(&self) -> &[f64] {
        &self.lai
    }

    /// Dead plant matter in a cell: (mol C m⁻², mol N m⁻²).
    pub fn litter(&self, cell: CellId) -> (f64, f64) {
        (self.litter[cell as usize], self.litter_n[cell as usize])
    }

    /// Living plant carbon in a cell, mol C m⁻².
    pub fn biomass(&self, cell: CellId) -> f64 {
        self.cells[cell as usize].iter().map(Cohort::biomass).sum()
    }

    /// Height of the tallest cohort in a cell, m (0 if bare).
    pub fn canopy_height(&self, cell: CellId) -> f64 {
        self.cells[cell as usize]
            .iter()
            .filter(|c| c.leaf > 1e-6)
            .map(|c| c.height)
            .fold(0.0, f64::max)
    }

    /// The cohort with the most biomass in a cell.
    pub fn dominant(&self, cell: CellId) -> Option<&Cohort> {
        self.cells[cell as usize]
            .iter()
            .max_by(|a, b| a.biomass().total_cmp(&b.biomass()))
    }

    pub fn is_alive(&self) -> bool {
        self.cells.iter().any(|c| !c.is_empty())
    }

    /// Plant and litter carbon, mol C (conserved with the air's CO₂).
    pub fn carbon_inventory(&self, geo: &Geometry) -> f64 {
        (0..self.cells.len())
            .map(|i| (self.cells[i].iter().map(Cohort::biomass).sum::<f64>() + self.litter[i]) * geo.area[i])
            .sum()
    }

    /// Atmospheric change not yet handed back (from seeding).
    pub fn take_pending(&mut self) -> AtmosphereChange {
        std::mem::take(&mut self.pending)
    }

    pub fn hash_words(&self, write: &mut dyn FnMut(u64)) {
        write(self.years);
        write(self.owed.to_bits());
        for (i, cell) in self.cells.iter().enumerate() {
            write(self.litter[i].to_bits());
            write(self.litter_n[i].to_bits());
            for c in cell {
                write(c.lineage as u64);
                for v in [c.density, c.height, c.leaf, c.wood, c.root, c.store, c.age] {
                    write(v.to_bits());
                }
            }
        }
        write(self.registry.total_ever());
    }

    /// Plants one lineage of a genome in each of `cells` (the player's
    /// seeding tool, and tests), drawing its carbon from the air.
    pub fn introduce(&mut self, geo: &Geometry, cells: &[CellId], genome: PlantGenome, biomass: f64, now: f64) -> LineageId {
        let id = self.registry.insert(genome, None, now);
        for &cell in cells {
            let c = cell as usize;
            let cohort = self.seedling_cohort(id, biomass, 1.0, false);
            self.pending.co2 -= biomass * geo.area[c];
            self.pending.n2 -= 0.5 * cohort.nitrogen() * geo.area[c];
            self.cells[c].push(cohort);
            self.merge_cohorts(c);
        }
        id
    }

    /// A young cohort holding `carbon` mol C m⁻² in tissues.
    fn seedling_cohort(&self, lineage: LineageId, carbon: f64, gravity: f64, ocean: bool) -> Cohort {
        let g = self.registry.genome(lineage);
        let (leaf, root, wood) = (0.5 * carbon, 0.35 * carbon, 0.15 * carbon);
        // Seedlings start at a size set by the seed.
        let coef = wood_coefficient(gravity, ocean);
        let per = (g.seed_mass as f64 * 0.15).max(1e-12);
        let height = height_from_wood(per, coef).clamp(0.002, g.max_height as f64);
        Cohort {
            lineage,
            density: wood / per,
            height,
            leaf,
            wood,
            root,
            store: 0.0,
            age: 0.0,
        }
    }

    /// Records today's climate into the monthly climatology, then runs any
    /// plant years owed. Returns the change to the atmosphere.
    pub fn step(
        &mut self,
        env: &Environment,
        life: &mut Biosphere,
        dt: f64,
        geologic_years: f64,
        mutation_multiplier: f64,
        structural_multiplier: f64,
    ) -> AtmosphereChange {
        let n = self.cells.len();
        assert_eq!(n, env.grid.cell_count(), "vegetation and grid disagree");
        self.observe(env, life, dt);
        self.since_check += geologic_years;
        self.owed += dt / env.planet.year_seconds() * self.params.time_lapse;
        if self.owed >= 1.0 {
            let years = std::mem::take(&mut self.since_check);
            self.graduate(env, life, years, structural_multiplier);
        }
        while self.owed >= 1.0 {
            self.owed -= 1.0;
            self.plant_year(env, life, mutation_multiplier);
        }
        std::mem::take(&mut self.pending)
    }

    fn observe(&mut self, env: &Environment, life: &Biosphere, dt: f64) {
        let light = life.light();
        if light.len() != self.clim.len() {
            return;
        }
        let year = env.planet.year_seconds();
        let month = (((env.start + dt / 2.0) / year).rem_euclid(1.0) * 12.0) as usize % 12;
        let blend = (dt / (year / 12.0)).min(1.0);
        let c = env.climate;
        for (i, months) in self.clim.iter_mut().enumerate() {
            let cell = i as CellId;
            let ocean = env.terrain.is_ocean(cell);
            let temp = if ocean { c.temperature[i].max(271.4) } else { c.temperature[i] };
            let wet = if ocean || c.lake[i] {
                1.0
            } else {
                let w = (c.soil[i] / c.params.soil_capacity).clamp(0.0, 1.0);
                if temp < 271.15 { 0.1 * w } else { w }
            };
            let now = Month {
                temp,
                wet,
                light: light[i],
                seen: true,
            };
            if !months.iter().any(|m| m.seen) {
                // First look: assume every month is like today until seen.
                *months = [Month { seen: false, ..now }; 12];
                months[month] = now;
            } else if !months[month].seen {
                months[month] = now;
            } else {
                let m = &mut months[month];
                m.temp += (temp - m.temp) * blend;
                m.wet += (wet - m.wet) * blend;
                m.light += (light[i] - m.light) * blend;
            }
        }
    }

    /// Colonies of sticky, specialised photosynthetic cells may become
    /// multicellular producers (design §6.2).
    fn graduate(&mut self, env: &Environment, life: &mut Biosphere, geologic_years: f64, multiplier: f64) {
        let p = self.params.graduation_rate * geologic_years * multiplier;
        if p <= 0.0 {
            return;
        }
        let now = env.start;
        let geo = env.climate.geometry();
        let gravity = env.planet.relative_gravity();
        let mut first = None;
        for cell in env.grid.cells() {
            let candidates: Vec<(u32, f64)> = life
                .strains_in(cell)
                .into_iter()
                .filter(|&(id, b)| {
                    let g = &life.strain(id).unwrap().genome;
                    b >= 0.005
                        && g.adhesion >= 0.4
                        && g.differentiation >= 0.4
                        && g.efficiency(Module::OxygenicPhotosynthesis) >= 0.2
                })
                .collect();
            if candidates.is_empty() {
                continue;
            }
            let mut rng = Rng::stream(self.seed, GRADUATION_DOMAIN, cell as u64, self.years);
            for (id, b) in candidates {
                if !rng.chance(1.0 - (-p).exp()) || self.cells[cell as usize].len() >= COHORTS {
                    continue;
                }
                let ocean = env.terrain.is_ocean(cell);
                let genome = PlantGenome::from_strain(&life.strain(id).unwrap().genome, !ocean);
                let taken = life.take_biomass(cell, id, 0.5 * b);
                let lineage = self.registry.insert(genome, None, now);
                let cohort = self.seedling_cohort(lineage, taken, gravity, ocean);
                // The colony's surplus nutrients go back to the water.
                let pools = &mut life.pools_mut()[cell as usize];
                let n_excess = taken * MICROBE_N - cohort.nitrogen();
                pools[NIT] += n_excess;
                pools[PHOS] += taken * MICROBE_P - cohort.nitrogen() * P_PER_N;
                self.cells[cell as usize].push(cohort);
                first.get_or_insert(cell);
            }
        }
        if first.is_some() {
            life.record_milestone(Milestone::Multicellular, now);
        }
        let _ = geo;
    }

    /// One plant year everywhere.
    fn plant_year(&mut self, env: &Environment, life: &mut Biosphere, mutation_multiplier: f64) {
        let geo = env.climate.geometry();
        let planet = env.planet;
        let atm = &planet.atmosphere;
        let mpb = moles_per_bar(planet);
        let planet_area: f64 = geo.area.iter().sum();
        let ocean = env.terrain.ocean_mask();
        // Producers in the sea need shallow water: shelves and coasts.
        let habitat: Vec<f64> = env
            .grid
            .cells()
            .map(|c| {
                if !ocean[c as usize] {
                    return 1.0;
                }
                let depth = -env.terrain.height_above_sea(c);
                let shelf = (1.0 - depth / 200.0).clamp(0.0, 1.0);
                let coast = env.grid.neighbours(c).iter().any(|&j| !ocean[j as usize]);
                if coast { shelf.max(0.1) } else { shelf }
            })
            .collect();
        let air_oxygen = atm.o2 / 0.21;
        let crossing: Vec<f64> = geo.area.iter().map(|a| (SEED_RANGE / a.sqrt()).min(1.0)).collect();
        let ctx = YearContext {
            crossing: &crossing,
            registry: &self.registry,
            clim: &self.clim,
            ocean,
            habitat: &habitat,
            co2_factor: atm.co2 / (atm.co2 + CO2_HALF),
            co2_cap: 0.5 * atm.co2 * mpb / planet_area,
            // Without an ozone layer, UV scorches anything on land.
            uv: 1.0 / (1.0 + air_oxygen / 0.01),
            gravity: planet.relative_gravity(),
            mutation: self.params.mutation_rate * mutation_multiplier,
            seed: self.seed,
            tick: self.years,
        };
        let pools = life.pools_mut();
        let outs: Vec<CellOut> = self
            .cells
            .par_iter_mut()
            .zip(self.litter.par_iter_mut())
            .zip(self.litter_n.par_iter_mut())
            .zip(pools.par_iter_mut())
            .enumerate()
            .map(|(i, (((cohorts, litter), litter_n), pool))| grow_cell(&ctx, i, cohorts, litter, litter_n, pool))
            .collect();
        let gravity = ctx.gravity;

        // Seeds: new mutations get their lineages; the rest are sown.
        let now = env.start;
        let mut incoming: Vec<Vec<(LineageId, f64, f64)>> = vec![Vec::new(); self.cells.len()];
        let mut change = AtmosphereChange::default();
        let (mut gpp, mut npp) = (0.0, 0.0);
        for (i, out) in outs.into_iter().enumerate() {
            let a = geo.area[i];
            change.co2 += out.co2 * a;
            change.n2 -= 0.5 * out.n_fixed * a;
            gpp += out.gpp * a;
            npp += out.npp * a;
            incoming[i].extend(out.seeds_local);
            for (parent, genome, count, carbon) in out.mutants {
                let serial = self.registry.get(parent).map(|r| r.serial);
                let id = self.registry.insert(genome, serial, now);
                incoming[i].push((id, count, carbon));
            }
            let neighbours = env.grid.neighbours(i as CellId);
            let share = 1.0 / neighbours.len() as f64;
            for (id, count, carbon) in out.seeds_out {
                for &j in neighbours {
                    let r = share * a / geo.area[j as usize];
                    incoming[j as usize].push((id, count * r, carbon * r));
                }
            }
        }

        // Sowing: seeds that find light and room become seedlings.
        let registry = &self.registry;
        let clim = &self.clim;
        self.cells
            .par_iter_mut()
            .zip(self.litter.par_iter_mut())
            .zip(self.litter_n.par_iter_mut())
            .zip(incoming.into_par_iter())
            .enumerate()
            .for_each(|(i, (((cohorts, litter), litter_n), seeds))| {
                sow(registry, &clim[i], ocean[i], habitat[i], gravity, cohorts, litter, litter_n, seeds);
            });
        for c in 0..self.cells.len() {
            self.merge_cohorts(c);
        }

        self.years += 1;
        self.census(env, gpp, npp);
        let t = env.start;
        if self.stats.land_biomass >= 1e12 {
            life.record_milestone(Milestone::PlantsOnLand, t);
        }
        if self.stats.forest_land >= 0.01 {
            life.record_milestone(Milestone::Forests, t);
        }
        self.pending.add(&change);
    }

    /// Keeps at most [`COHORTS`] per cell: same-lineage cohorts of similar
    /// height merge, then the smallest joins its most similar neighbour.
    fn merge_cohorts(&mut self, c: usize) {
        let registry = &self.registry;
        let cohorts = &mut self.cells[c];
        cohorts.retain(|x| x.density > 1e-12 && x.biomass() > 1e-12);
        let similar = |a: &Cohort, b: &Cohort| {
            a.lineage == b.lineage && (a.height / b.height).ln().abs() < 0.2
        };
        let mut i = 0;
        while i < cohorts.len() {
            if let Some(j) = (i + 1..cohorts.len()).find(|&j| similar(&cohorts[i], &cohorts[j])) {
                let b = cohorts.remove(j);
                absorb(&mut cohorts[i], b);
            } else {
                i += 1;
            }
        }
        // Too many: first merge size classes of the same lineage (closest
        // heights first), so rare lineages — new mutants — keep their place.
        while cohorts.len() > COHORTS {
            let mut best: Option<(usize, usize, f64)> = None;
            for a in 0..cohorts.len() {
                for b in a + 1..cohorts.len() {
                    if cohorts[a].lineage == cohorts[b].lineage {
                        let d = (cohorts[a].height / cohorts[b].height).ln().abs();
                        if best.is_none_or(|x| d < x.2) {
                            best = Some((a, b, d));
                        }
                    }
                }
            }
            let Some((a, b, _)) = best else { break };
            let small = cohorts.remove(b);
            absorb(&mut cohorts[a], small);
        }
        while cohorts.len() > COHORTS {
            let k = (0..cohorts.len())
                .min_by(|&a, &b| cohorts[a].biomass().total_cmp(&cohorts[b].biomass()).then(a.cmp(&b)))
                .unwrap();
            let small = cohorts.remove(k);
            let g = registry.genome(small.lineage);
            let j = (0..cohorts.len())
                .min_by(|&a, &b| {
                    let d = |x: &Cohort| {
                        g.distance(registry.genome(x.lineage)) + (x.height / small.height).ln().abs()
                    };
                    d(&cohorts[a]).total_cmp(&d(&cohorts[b])).then(a.cmp(&b))
                })
                .unwrap();
            absorb(&mut cohorts[j], small);
        }
        cohorts.sort_by(|a, b| b.height.total_cmp(&a.height).then(a.lineage.cmp(&b.lineage)));
    }

    fn census(&mut self, env: &Environment, gpp: f64, npp: f64) {
        let geo = env.climate.geometry();
        let mut totals = vec![(0.0, 0u32); self.registry.capacity()];
        let mut s = VegetationStats {
            gpp,
            npp,
            plant_years: self.years,
            ..Default::default()
        };
        let mut land_cells = 0usize;
        let (mut green, mut forest) = (0usize, 0usize);
        for (i, cohorts) in self.cells.iter().enumerate() {
            let a = geo.area[i];
            let ocean = env.terrain.is_ocean(i as CellId);
            let mut lai = 0.0;
            let mut top: f64 = 0.0;
            for c in cohorts {
                let m = c.biomass() * a;
                totals[c.lineage as usize].0 += m;
                totals[c.lineage as usize].1 += 1;
                s.biomass += m;
                if ocean {
                    s.water_biomass += m;
                } else {
                    s.land_biomass += m;
                }
                let form = self.registry.genome(c.lineage).form();
                let k = GrowthForm::ALL.iter().position(|&f| f == form).unwrap();
                s.form_biomass[k] += m;
                lai += c.leaf * SLA;
                if c.leaf > 1e-6 {
                    top = top.max(c.height);
                }
            }
            s.cohorts += cohorts.len();
            s.litter += self.litter[i] * a;
            s.tallest = s.tallest.max(top);
            self.lai[i] = if ocean { 0.0 } else { lai };
            if !ocean {
                land_cells += 1;
                if lai > 0.5 {
                    green += 1;
                }
                if lai >= 2.0 && top >= 10.0 {
                    forest += 1;
                }
            }
        }
        self.registry.census(&totals);
        s.lineages = self.registry.len();
        s.green_land = green as f64 / land_cells.max(1) as f64;
        s.forest_land = forest as f64 / land_cells.max(1) as f64;
        self.stats = s;
    }

    /// After terraforming: producers in cells that switched between land and
    /// sea keep growing, but find the habitat they're adapted to (or not).
    pub fn terrain_changed(&mut self) {}
}

/// Adds cohort `b` into `a` (keeping `a`'s lineage); height follows the
/// combined wood per individual.
fn absorb(a: &mut Cohort, b: Cohort) {
    let wood_per = (a.wood + b.wood) / (a.density + b.density).max(1e-30);
    let ratio = if a.wood > 0.0 && a.density > 0.0 { wood_per / (a.wood / a.density) } else { 1.0 };
    a.height *= ratio.max(1e-9).powf(1.0 / WOOD_EXPONENT);
    a.density += b.density;
    a.leaf += b.leaf;
    a.wood += b.wood;
    a.root += b.root;
    a.store += b.store;
    a.age = a.age.max(b.age);
}

/// One plant year in one cell: light, growth, upkeep, nutrients, height,
/// thinning, litter, death and seeds.
fn grow_cell(
    ctx: &YearContext,
    i: usize,
    cohorts: &mut Vec<Cohort>,
    litter: &mut f64,
    litter_n: &mut f64,
    pool: &mut [f64; POOLS],
) -> CellOut {
    let mut out = CellOut::default();
    let months = &ctx.clim[i];
    let ocean = ctx.ocean[i];
    let habitat = ctx.habitat[i];
    let month_seconds = SECONDS_PER_YEAR / 12.0;
    let mean_temp = months.iter().map(|m| m.temp).sum::<f64>() / 12.0;
    let mean_wet = months.iter().map(|m| m.wet).sum::<f64>() / 12.0;

    // Litter rots, returning CO₂ and nutrients.
    let decay_rate = if ocean {
        LITTER_DECAY_WATER
    } else {
        LITTER_DECAY * (0.2 + 0.8 * mean_wet)
    } * 2f64.powf((mean_temp - 293.15) / 10.0).min(4.0);
    let rot = 1.0 - (-decay_rate).exp();
    let (dc, dn) = (*litter * rot, *litter_n * rot);
    *litter -= dc;
    *litter_n -= dn;
    out.co2 += dc;
    pool[NIT] += dn;
    pool[PHOS] += dn * P_PER_N;

    if cohorts.is_empty() {
        return out;
    }
    if !ocean {
        // Roots and their fungi dissolve rock, freeing phosphorus.
        let roots: f64 = cohorts.iter().map(|c| c.root).sum();
        pool[PHOS] += ROCK_PHOSPHORUS * (roots / (roots + 5.0)) * mean_wet;
    }
    let mut to_litter = |c: f64, n: f64| {
        *litter += c;
        *litter_n += n;
    };
    if habitat <= 0.0 {
        for c in cohorts.drain(..) {
            to_litter(c.biomass(), c.nitrogen());
        }
        return out;
    }
    let coef = wood_coefficient(ctx.gravity, ocean);
    cohorts.sort_by(|a, b| b.height.total_cmp(&a.height).then(a.lineage.cmp(&b.lineage)));
    let k = cohorts.len();

    // --- Light and photosynthesis, month by month -----------------------
    let mut gpp = vec![0.0; k];
    let mut upkeep = vec![0.0; k];
    let mut leaf_on = vec![[1.0f64; 12]; k];
    let mut dry_months = vec![0.0; k];
    for (ci, c) in cohorts.iter().enumerate() {
        let g = ctx.registry.genome(c.lineage);
        let root_ratio = c.root / c.leaf.max(1e-12);
        for (m, mo) in months.iter().enumerate() {
            let f_env = temperature_fitness(g, mo.temp) * if ocean { 1.0 } else { water_fitness(g, mo.wet, root_ratio) };
            // Deciduous plants drop leaves when a month isn't worth it.
            let good = ((f_env - 0.1) / 0.2).clamp(0.0, 1.0);
            leaf_on[ci][m] = 1.0 - g.deciduous as f64 * (1.0 - good);
            if !ocean && water_fitness(g, mo.wet, root_ratio) < 0.05 {
                dry_months[ci] += 1.0;
            }
        }
    }
    for m in 0..12 {
        let mo = &months[m];
        let light = if ocean { 0.5 * mo.light } else { mo.light };
        let saturation = 1.0 / (1.0 + light / LIGHT_SATURATION);
        let mut above = 0.0;
        for (ci, c) in cohorts.iter().enumerate() {
            let g = ctx.registry.genome(c.lineage);
            let lai = c.leaf * SLA / habitat * leaf_on[ci][m];
            let absorbed = (-EXTINCTION * above).exp() * (1.0 - (-EXTINCTION * lai).exp());
            above += lai;
            let root_ratio = c.root / c.leaf.max(1e-12);
            let f_t = temperature_fitness(g, mo.temp);
            let f_w = if ocean { 1.0 } else { water_fitness(g, mo.wet, root_ratio) };
            let habitat_fit = if ocean {
                1.0 - 0.6 * g.land as f64
            } else {
                g.land as f64
            };
            gpp[ci] += light * month_seconds * absorbed * LIGHT_USE * saturation * f_t * f_w
                * ctx.co2_factor
                * habitat_fit
                * (1.0 - 0.3 * g.drought_tolerance as f64)
                * habitat;
            let q10 = 2f64.powf((mo.temp - 293.15) / 10.0).min(8.0);
            upkeep[ci] += (c.leaf * LEAF_UPKEEP * leaf_on[ci][m].max(0.1)
                + c.root * ROOT_UPKEEP
                + c.wood * WOOD_UPKEEP)
                * q10
                / 12.0;
        }
    }
    let total_gpp: f64 = gpp.iter().sum();
    if total_gpp > ctx.co2_cap {
        let f = ctx.co2_cap / total_gpp;
        gpp.iter_mut().for_each(|x| *x *= f);
    }

    // --- Carbon balance and potential growth -----------------------------
    let mut rng = Rng::stream(ctx.seed, VEGETATION_DOMAIN, i as u64, ctx.tick);
    let mut grow = vec![(0.0, 0.0, 0.0, 0.0); k]; // leaf, root, wood, seeds
    let mut spread = vec![0.0; k]; // new individuals (clonal spread), m⁻²
    let mut fixing = vec![0.0; k];
    for (ci, c) in cohorts.iter_mut().enumerate() {
        let g = ctx.registry.genome(c.lineage);
        out.gpp += gpp[ci];
        out.co2 -= gpp[ci];
        let net = gpp[ci] - upkeep[ci];
        out.co2 += upkeep[ci];
        out.npp += net;
        c.store += net;
        if c.store < 0.0 {
            // Starving: burn tissue for the deficit; some plants die.
            let deficit = -c.store;
            c.store = 0.0;
            let tissue = c.leaf + c.wood + c.root;
            let f = (1.5 * deficit / tissue.max(1e-30)).min(1.0);
            let burned = deficit.min(tissue);
            let n0 = c.nitrogen();
            let keep = 1.0 - burned / tissue.max(1e-30);
            c.leaf *= keep;
            c.wood *= keep;
            c.root *= keep;
            // Upkeep it couldn't pay was never respired.
            out.co2 -= deficit - burned;
            pool[NIT] += n0 - c.nitrogen();
            pool[PHOS] += (n0 - c.nitrogen()) * P_PER_N;
            let dead_n = c.nitrogen() * f;
            let dead_c = (c.leaf + c.wood + c.root) * f;
            c.scale(1.0 - f);
            to_litter(dead_c, dead_n);
        }
        let target = 0.4 * (c.leaf + c.root);
        let spare = (c.store - target).max(0.0);
        if spare <= 0.0 {
            continue;
        }
        let build = spare / (1.0 + GROWTH_COST);
        let mature = c.height >= 0.6 * g.max_height as f64;
        let seeds = if mature { g.seed_share as f64 * build } else { 0.0 };
        let (fl, fr, fw) = g.allocation();
        let mut tissue = build - seeds;
        // Crowns hold only so much foliage.
        let crown_cover = (c.density * crown_area(c.height) / 1.0).min(1.0) * habitat;
        let leaf_cap = MAX_CROWN_LAI / SLA * crown_cover.max(1e-6);
        let mut leaf = (tissue * fl).min((leaf_cap - c.leaf).max(0.0));
        let mut root = tissue * fr;
        let mut wood = tissue * fw;
        let spill = tissue * fl - leaf;
        // Beyond full height, wood isn't needed: put it into seeds or roots.
        let full = c.density * coef * (g.max_height as f64).powf(WOOD_EXPONENT);
        let wood_room = (full - c.wood).max(0.0);
        let mut extra = (wood + spill - wood_room).max(0.0);
        wood = (wood + spill).min(wood_room);
        // Full-grown plants fill gaps around them by branching or sending
        // out new shoots, until their crowns close.
        let per_individual = (c.leaf + c.wood + c.root) / c.density.max(1e-30);
        let gaps = (habitat / crown_area(c.height) - c.density).max(0.0);
        if extra > 0.0 && gaps > 0.0 && per_individual > 0.0 {
            let clonal = extra.min(gaps * per_individual);
            let f = clonal / (c.leaf + c.wood + c.root).max(1e-30);
            leaf += c.leaf * f;
            root += c.root * f;
            wood += c.wood * f;
            spread[ci] = c.density * f;
            extra -= clonal;
        }
        let mut seeds = seeds;
        if mature {
            seeds += extra;
        } else {
            root += extra;
        }
        tissue = leaf + root + wood;
        let _ = tissue;
        if leaf < 0.0 {
            leaf = 0.0;
        }
        grow[ci] = (leaf, root, wood, seeds);
        fixing[ci] = g.nitrogen_fixing as f64;
    }

    // --- Nutrients: roots share the soil's N and P ------------------------
    let need_n: Vec<f64> = grow
        .iter()
        .map(|&(l, r, w, s)| l / LEAF_CN + r / ROOT_CN + w / WOOD_CN + s / SEED_CN)
        .collect();
    let roots: f64 = cohorts.iter().map(|c| c.root).sum::<f64>().max(1e-30);
    let avail_n = pool[NIT].max(0.0) * 0.9;
    let avail_p = pool[PHOS].max(0.0) * 0.9;
    for (ci, c) in cohorts.iter_mut().enumerate() {
        let (l, r, w, s) = grow[ci];
        let built = l + r + w + s;
        if built <= 0.0 {
            continue;
        }
        let share = c.root / roots;
        let n_from_soil = (need_n[ci] * (1.0 - fixing[ci])).min(avail_n * share);
        let n_ratio = if need_n[ci] > 0.0 {
            ((n_from_soil + need_n[ci] * fixing[ci]) / need_n[ci]).min(1.0)
        } else {
            1.0
        };
        let p_need = need_n[ci] * P_PER_N;
        let p_ratio = if p_need > 0.0 { (avail_p * share / p_need).min(1.0) } else { 1.0 };
        let ratio = n_ratio.min(p_ratio);
        let fixed_share = 1.0 - (n_from_soil / (need_n[ci] * ratio).max(1e-30)).min(1.0);
        let full_cost = (l + r + w + s) * ratio * (1.0 + GROWTH_COST)
            + need_n[ci] * ratio * fixed_share * FIXATION_COST;
        // Fixing nitrogen can cost more sugar than is spare: grow less.
        let ratio = if full_cost > c.store { ratio * c.store / full_cost } else { ratio };
        let (l, r, w, s) = (l * ratio, r * ratio, w * ratio, s * ratio);
        let n_used = need_n[ci] * ratio;
        let fixed = n_used * fixed_share;
        let soil_n = n_used - fixed;
        let cost = (l + r + w + s) * (1.0 + GROWTH_COST) + fixed * FIXATION_COST;
        c.store -= cost;
        out.co2 += cost - (l + r + w + s);
        pool[NIT] -= soil_n;
        pool[PHOS] -= n_used * P_PER_N;
        out.n_fixed += fixed;
        c.leaf += l;
        c.root += r;
        c.wood += w;
        c.density += spread[ci] * ratio;
        // Unspendable sugar beyond twice the reserve target is respired.
        let cap = 0.8 * (c.leaf + c.root) + 0.01;
        if c.store > cap {
            out.co2 += c.store - cap;
            c.store = cap;
        }
        if s > 0.0 {
            let g = ctx.registry.genome(c.lineage);
            let count = s / g.seed_mass as f64;
            let away = g.dispersal as f64 * ctx.crossing[i];
            // Seed N travels with the seeds (SEED_CN), as carbon below.
            if rng.chance((1.0 - (-ctx.mutation).exp()).min(1.0)) {
                let mutant = g.mutant(&mut rng);
                out.mutants.push((c.lineage, mutant, 0.2 * count, 0.2 * s));
                out.seeds_local.push((c.lineage, 0.8 * count * (1.0 - away), 0.8 * s * (1.0 - away)));
                out.seeds_out.push((c.lineage, 0.8 * count * away, 0.8 * s * away));
            } else {
                out.seeds_local.push((c.lineage, count * (1.0 - away), s * (1.0 - away)));
                out.seeds_out.push((c.lineage, count * away, s * away));
            }
        }
    }

    // --- Height, thinning, turnover and death -----------------------------
    let coldest = months.iter().map(|m| m.temp).fold(f64::MAX, f64::min) - 5.0;
    for (ci, c) in cohorts.iter_mut().enumerate() {
        let g = ctx.registry.genome(c.lineage);
        c.age += 1.0;
        if c.density > 0.0 {
            c.height = height_from_wood(c.wood / c.density, coef).clamp(0.002, (g.max_height as f64).max(0.002));
        }
        // Crowns can't overlap more than the ground allows.
        let cover = c.density * crown_area(c.height) / habitat;
        if cover > 1.0 {
            let f = 1.0 - 1.0 / cover;
            to_litter(c.biomass() * f, c.nitrogen() * f);
            c.scale(1.0 - f);
        }
        // Leaves, roots and twigs turn over.
        let d = g.deciduous as f64;
        let mut leaf_loss = d + (1.0 - d) / 3.0;
        let frost = (273.15 - g.frost_hardiness as f64) - coldest;
        if frost > 0.0 {
            leaf_loss += (0.1 * frost).min(1.0) * (1.0 - d);
        }
        let leaf_drop = c.leaf * leaf_loss.min(1.0);
        let root_drop = c.root * (1.0 - (-0.7f64).exp());
        let wood_drop = c.wood * (1.0 - (-0.02f64).exp());
        c.leaf -= leaf_drop;
        c.root -= root_drop;
        c.wood -= wood_drop;
        let resorbed = leaf_drop / LEAF_CN * RESORPTION;
        pool[NIT] += resorbed;
        pool[PHOS] += resorbed * P_PER_N;
        // Fallen leaves are replaced from reserves (next spring's flush for
        // deciduous plants), as far as sugar and nitrogen allow.
        let regrow = leaf_drop
            .min(c.store / (1.0 + GROWTH_COST))
            .min(pool[NIT].max(0.0) * 0.9 * LEAF_CN)
            .min(pool[PHOS].max(0.0) * 0.9 * LEAF_CN / P_PER_N)
            .max(0.0);
        c.leaf += regrow;
        c.store -= regrow * (1.0 + GROWTH_COST);
        out.co2 += regrow * GROWTH_COST;
        pool[NIT] -= regrow / LEAF_CN;
        pool[PHOS] -= regrow / LEAF_CN * P_PER_N;
        to_litter(
            leaf_drop + root_drop + wood_drop,
            leaf_drop / LEAF_CN * (1.0 - RESORPTION) + root_drop / ROOT_CN + wood_drop / WOOD_CN,
        );
        // Deaths.
        let woody = c.wood / c.biomass().max(1e-30);
        let mut mortality = 0.01 + 1.0 / (4.0 + 15.0 * g.max_height as f64);
        if frost > 0.0 {
            mortality += 0.08 * frost * woody;
        }
        if ocean {
            mortality += 0.3 * g.land as f64;
        } else {
            mortality += 4.0 * ctx.uv + 0.8 * (1.0 - g.land as f64).powi(2);
            mortality += 0.05 * dry_months[ci] * (1.0 - g.drought_tolerance as f64);
        }
        let f = 1.0 - (-mortality).exp();
        to_litter(c.biomass() * f, c.nitrogen() * f);
        c.scale(1.0 - f);
    }
    cohorts.retain(|c| c.density > 1e-12 && c.biomass() > 1e-12);
    out
}

/// Seeds arriving in a cell germinate where they find light and a climate
/// they can live in; the rest rot.
#[allow(clippy::too_many_arguments)]
fn sow(
    registry: &Lineages<PlantGenome>,
    months: &[Month; 12],
    ocean: bool,
    habitat: f64,
    gravity: f64,
    cohorts: &mut Vec<Cohort>,
    litter: &mut f64,
    litter_n: &mut f64,
    seeds: Vec<(LineageId, f64, f64)>,
) {
    if seeds.is_empty() {
        return;
    }
    let canopy: f64 = cohorts.iter().map(|c| c.leaf * SLA).sum::<f64>() / habitat.max(1e-9);
    let ground_light = (-EXTINCTION * canopy).exp();
    let coef = wood_coefficient(gravity, ocean);
    for (id, count, carbon) in seeds {
        if count <= 0.0 || carbon <= 0.0 {
            continue;
        }
        let g = registry.genome(id);
        let seed_n = carbon / SEED_CN;
        let fit: f64 = months
            .iter()
            .map(|m| {
                temperature_fitness(g, m.temp) * if ocean { 1.0 } else { water_fitness(g, m.wet, 1.0) }
            })
            .sum::<f64>()
            / 12.0;
        let habitat_fit = if ocean { 1.0 - g.land as f64 } else { (g.land as f64).powi(2) };
        let shade = g.seed_mass as f64 / (g.seed_mass as f64 + 1e-3);
        let p = if habitat > 0.0 {
            0.05 * fit * habitat_fit * (ground_light + (1.0 - ground_light) * shade)
        } else {
            0.0
        };
        // Room: seedlings can cover at most half the light gaps.
        let per = (g.seed_mass as f64 * 0.15).max(1e-12);
        let h0 = height_from_wood(per, coef).clamp(0.002, (g.max_height as f64).max(0.002));
        let room = 0.5 * ground_light.max(0.05) * habitat / crown_area(h0);
        let recruits = (count * p).min(room);
        let established = (recruits * g.seed_mass as f64 * 0.8).min(carbon);
        if established <= 0.0 {
            *litter += carbon;
            *litter_n += seed_n;
            continue;
        }
        let (leaf, root, wood) = (0.5 * established, 0.35 * established, 0.15 * established);
        let cohort = Cohort {
            lineage: id,
            density: recruits,
            height: h0,
            leaf,
            wood,
            root,
            store: 0.0,
            age: 0.0,
        };
        // Seeds are richer in N than seedlings: the rest goes to the litter.
        *litter += carbon - established;
        *litter_n += seed_n - cohort.nitrogen().min(seed_n);
        if let Some(existing) = cohorts.iter_mut().find(|c| c.lineage == id && c.height < 2.0 * h0) {
            absorb(existing, cohort);
        } else {
            cohorts.push(cohort);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn climate(temp: f64, wet: f64, light: f64) -> [Month; 12] {
        [Month {
            temp,
            wet,
            light,
            seen: true,
        }; 12]
    }

    struct Plot {
        registry: Lineages<PlantGenome>,
        clim: Vec<[Month; 12]>,
        cohorts: Vec<Cohort>,
        litter: f64,
        litter_n: f64,
        pool: [f64; POOLS],
        co2: f64,
    }

    impl Plot {
        fn new(genomes: &[PlantGenome], month: [Month; 12]) -> Self {
            let mut registry = Lineages::default();
            let mut cohorts = Vec::new();
            for g in genomes {
                let id = registry.insert(g.clone(), None, 0.0);
                let per = (g.seed_mass as f64 * 0.15).max(1e-12);
                cohorts.push(Cohort {
                    lineage: id,
                    density: 0.01 / per,
                    height: height_from_wood(per, WOOD_COEF).max(0.002),
                    leaf: 0.05,
                    wood: 0.01,
                    root: 0.03,
                    store: 0.0,
                    age: 0.0,
                });
            }
            let mut pool = [0.0; POOLS];
            pool[NIT] = 5.0;
            pool[PHOS] = 0.5;
            Plot {
                registry,
                clim: vec![month],
                cohorts,
                litter: 0.0,
                litter_n: 0.0,
                pool,
                co2: 0.0,
            }
        }

        fn carbon(&self) -> f64 {
            self.cohorts.iter().map(Cohort::biomass).sum::<f64>() + self.litter
        }

        fn run(&mut self, years: u64) {
            for y in 0..years {
                let ctx = YearContext {
                    registry: &self.registry,
                    clim: &self.clim,
                    ocean: &[false],
                    habitat: &[1.0],
                    crossing: &[1.0],
                    co2_factor: 4e-4 / (4e-4 + CO2_HALF),
                    co2_cap: 1e9,
                    uv: 0.0,
                    gravity: 1.0,
                    mutation: 0.0,
                    seed: 1,
                    tick: y,
                };
                let out = grow_cell(&ctx, 0, &mut self.cohorts, &mut self.litter, &mut self.litter_n, &mut self.pool);
                self.co2 += out.co2;
                let seeds: Vec<_> = out.seeds_local.into_iter().chain(out.seeds_out).collect();
                sow(&self.registry, &self.clim[0], false, 1.0, 1.0, &mut self.cohorts, &mut self.litter, &mut self.litter_n, seeds);
            }
        }

        fn nitrogen(&self) -> f64 {
            self.cohorts.iter().map(Cohort::nitrogen).sum::<f64>() + self.litter_n + self.pool[NIT]
        }

        fn lai(&self) -> f64 {
            self.cohorts.iter().map(|c| c.leaf * SLA).sum()
        }
    }

    #[test]
    fn plants_grow_in_a_kind_climate_and_carbon_balances() {
        let mut p = Plot::new(&[PlantGenome::grass()], climate(295.0, 0.8, 100.0));
        let c0 = p.carbon();
        let n0 = p.nitrogen();
        p.run(30);
        assert!(p.lai() > 1.0, "grassland should close: LAI {}", p.lai());
        let c1 = p.carbon();
        assert!((c1 + p.co2 - c0).abs() < 1e-9 * c1.max(1.0), "carbon {c0} → {c1}, air {}", p.co2);
        let n1 = p.nitrogen();
        assert!((n1 - n0).abs() < 1e-9 * n0, "nitrogen {n0} → {n1}");
    }

    #[test]
    fn deserts_and_frozen_ground_stay_bare() {
        let mut wet = Plot::new(&[PlantGenome::grass()], climate(295.0, 0.8, 100.0));
        let mut dry = Plot::new(&[PlantGenome::grass()], climate(300.0, 0.03, 120.0));
        let mut ice = Plot::new(&[PlantGenome::grass()], climate(245.0, 0.1, 40.0));
        for p in [&mut wet, &mut dry, &mut ice] {
            p.run(30);
        }
        assert!(dry.lai() < 0.1 * wet.lai(), "desert {} vs wet {}", dry.lai(), wet.lai());
        assert!(ice.lai() < 0.05 * wet.lai());
    }

    #[test]
    fn trees_overtop_grass_where_both_can_grow() {
        let mut p = Plot::new(&[PlantGenome::grass(), PlantGenome::tree()], climate(297.0, 0.9, 110.0));
        p.run(150);
        let by = |form: GrowthForm| -> f64 {
            p.cohorts
                .iter()
                .filter(|c| p.registry.genome(c.lineage).form() == form)
                .map(Cohort::biomass)
                .sum()
        };
        assert!(by(GrowthForm::Tree) > by(GrowthForm::Herb), "trees {} grass {}", by(GrowthForm::Tree), by(GrowthForm::Herb));
        let tallest = p.cohorts.iter().map(|c| c.height).fold(0.0, f64::max);
        assert!(tallest > 8.0, "tallest {tallest}");
    }

    #[test]
    fn wood_costs_more_on_heavy_worlds() {
        assert!(wood_coefficient(2.0, false) > wood_coefficient(1.0, false));
        assert!(wood_coefficient(1.0, true) < wood_coefficient(1.0, false));
        let h = height_from_wood(WOOD_COEF * 10f64.powf(WOOD_EXPONENT), WOOD_COEF);
        assert!((h - 10.0).abs() < 1e-9);
    }
}
