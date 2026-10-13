//! One grid cell's microbial community over one step: who can grow on what,
//! how they share food, light and nutrients, who eats whom, who dies, and
//! which new strains arise (design §6.1).
//!
//! Every module is a real redox reaction, and every mole is booked: carbon
//! moves between the atmosphere (CO₂, CH₄), dissolved organics and biomass;
//! nitrogen and phosphorus move between dissolved pools and biomass in the
//! Redfield ratio (C:N:P = 106:16:1).

use dirtbox_core::Rng;

use crate::genome::{Genome, MODULE_COUNT, Module};
use crate::registry::{EMPTY, Registry, StrainId};

/// Strain slots per cell (design §6.1: K).
pub const SLOTS: usize = 8;

pub const DAY: f64 = 86_400.0;
pub const YEAR: f64 = 365.25 * DAY;

/// Nitrogen and phosphorus per carbon in biomass (Redfield).
pub const N_PER_C: f64 = 16.0 / 106.0;
pub const P_PER_C: f64 = 1.0 / 106.0;

/// Dissolved O₂ in the mixed layer at today's air (0.21 bar), mol m⁻³.
pub const O2_SATURATION: f64 = 0.25;
/// Today's oxygen partial pressure, bar.
pub const O2_TODAY: f64 = 0.21;

/// Indices into a cell's pool array.
pub const OM: usize = 0;
pub const H2: usize = 1;
pub const RED: usize = 2;
pub const O2: usize = 3;
pub const NIT: usize = 4;
pub const PHOS: usize = 5;
/// Stored per cell.
pub const POOLS: usize = 6;
/// Working copy only: this cell's share of atmospheric CO₂ for the step.
pub const CO2: usize = 6;
const WORK: usize = 7;

/// Below this, a population is gone, mol C m⁻².
pub const MIN_BIOMASS: f64 = 1.0e-9;

// --- Physiology ---------------------------------------------------------
// Maximum specific growth rates at full efficiency, per day.
const MU_FERMENT: f64 = 1.0;
const MU_AEROBIC: f64 = 2.0;
const MU_METHANOGEN: f64 = 0.6;
const MU_CHEMO: f64 = 0.8;
const MU_ANOXYGENIC: f64 = 0.8;
const MU_OXYGENIC: f64 = 1.0;
const MU_PHAGO: f64 = 1.0;

// Growth yields, mol biomass C per mol substrate. They follow the energy
// each reaction releases: respiring organics with O₂ yields ~14× more
// energy than fermenting them.
const Y_FERMENT: f64 = 0.1;
const Y_AEROBIC: f64 = 0.5;
/// Per mol H₂ (4 H₂ + CO₂ → CH₄ releases little energy).
const Y_METHANOGEN: f64 = 0.015;
/// Per mol O₂-equivalent of reductant, with and without oxygen to burn it.
const Y_CHEMO_OXIC: f64 = 0.25;
const Y_CHEMO_ANOXIC: f64 = 0.05;
const Y_PHAGO: f64 = 0.35;
/// H₂ released per mol organic C fermented.
const H2_PER_FERMENT: f64 = 0.67;

// Half-saturation constants, mol m⁻² of the mixed layer.
const K_ORGANICS: f64 = 0.002;
const K_H2: f64 = 0.002;
const K_REDUCTANT: f64 = 0.01;
const K_O2: f64 = 0.05;
const K_PREY: f64 = 0.005;
/// Phototroph biomass that absorbs 63% of the light, mol C m⁻².
const SHADING_BIOMASS: f64 = 0.05;
/// Best-case photosynthesis: ~10 photons per CO₂ at ~0.22 MJ per mol of
/// photons, mol C per J of usable light.
const QUANTUM_YIELD: f64 = 4.5e-7;

// Death, per day.
const BASE_DEATH: f64 = 0.01;
const MAINTENANCE: f64 = 0.01;
const COLD_HEAT_DEATH: f64 = 0.15;
const DROUGHT_DEATH: f64 = 0.1;
const OXYGEN_DEATH: f64 = 0.5;
const TOXIN_DEATH: f64 = 0.4;

/// Hydrothermal fluid temperature at the heart of a vent field, K.
const VENT_FLUID: f64 = 360.0;
/// Temperature tolerance with no specialist/generalist penalty, K.
const TOLERANCE_REF: f64 = 10.0;
/// Share of nutrients from dead cells that sinks below the mixed layer.
pub const EXPORT_FRACTION: f64 = 0.15;

/// A new strain takes this share of its parent's local population.
const SPAWN_SHARE: f64 = 0.02;
/// Populations this size or larger mutate at the full rate, mol C m⁻².
const MUTATION_REF: f64 = 1.0e-3;

/// What a cell offers its microbes this step.
#[derive(Clone, Copy, Debug)]
pub struct CellEnv {
    pub ocean: bool,
    /// Water temperature in the ocean (liquid under ice), ground temperature on land, K.
    pub temperature: f64,
    /// 1 in water; soil moisture on land (0 when frozen solid).
    pub wetness: f64,
    /// Usable light at the surface, W m⁻² (daily mean).
    pub light: f64,
    /// Hydrothermal vent strength, 0–1.
    pub vent: f64,
    /// Oxygen relative to water saturated with today's air (land: air).
    pub oxygen: f64,
    /// CO₂ limitation of carbon fixation, 0–1.
    pub co2_factor: f64,
    /// Step length, s.
    pub dt: f64,
    /// Geologic years this step stands for (mutation).
    pub geologic_years: f64,
}

/// A new strain that arose in this cell.
pub struct Spawn {
    pub parent: StrainId,
    pub genome: Genome,
    pub biomass: f64,
}

/// What the cell exchanged with the world outside its own pools.
#[derive(Default)]
pub struct CellExchange {
    /// Methane released, mol m⁻².
    pub ch4: f64,
    /// Nitrogen fixed from N₂, mol N m⁻².
    pub n_fixed: f64,
    /// Nutrients sunk to the deep ocean, mol m⁻².
    pub export_n: f64,
    pub export_p: f64,
    /// Carbon fixed by autotrophs, mol C m⁻² (for productivity stats).
    pub fixed: f64,
    pub spawns: Vec<Spawn>,
}

fn gaussian(t: f64, optimum: f64, sigma: f64) -> f64 {
    let z = (t - optimum) / sigma;
    (-0.5 * z * z).exp()
}

/// How well a genome copes with a cell's temperatures, 0–1. Vents add a hot
/// gradient (from the water's temperature up to the vent fluid's) covering
/// half the cell at full strength; a strain whose optimum lies anywhere along
/// it finds a comfortable spot there.
pub fn thermal_fitness(g: &Genome, env: &CellEnv) -> f64 {
    let opt = g.temp_optimum as f64;
    let tol = g.temp_tolerance as f64;
    // Generalists pay for their breadth.
    let peak = (TOLERANCE_REF / tol).sqrt().min(1.2);
    let hot = 0.5 * env.vent;
    let t = env.temperature;
    let ambient = gaussian(t, opt, tol);
    let gradient = if hot > 0.0 {
        gaussian(opt.clamp(t.min(VENT_FLUID), VENT_FLUID), opt, tol)
    } else {
        0.0
    };
    (peak * ((1.0 - hot) * ambient + hot * gradient)).min(1.0)
}

/// Oxygen stress on a strain, 0–1.
pub fn oxygen_stress(g: &Genome, oxygen: f64) -> f64 {
    let x = oxygen.max(0.0);
    x / (x + 0.02) * (1.0 - g.oxygen_tolerance as f64)
}

/// Fraction of growth a strain loses to its traits' upkeep (≥ 1).
fn upkeep(g: &Genome) -> f64 {
    1.0 + 0.3 * g.motility as f64
        + 0.15 * g.dormancy as f64
        + 0.2 * g.adhesion as f64
        + 0.4 * g.toxin as f64
        + 0.1 * (g.module_count().saturating_sub(1)) as f64
        + if g.complex { 0.1 } else { 0.0 }
}

/// Share of a cell's enzyme budget that goes to module `m`: carrying many
/// pathways means running each one slower. Square-rooted, so a weak new
/// module costs its owner little at first.
fn allocation(g: &Genome, m: Module) -> f64 {
    let total: f64 = Module::ALL
        .iter()
        .filter(|&&k| k != Module::NitrogenFixation)
        .map(|&k| g.efficiency(k))
        .sum();
    if total > 0.0 {
        (g.efficiency(m) / total).min(1.0).sqrt()
    } else {
        0.0
    }
}

/// How easily `pred` can engulf `prey`, 0–1.
fn edibility(pred: &Genome, prey: &Genome) -> f64 {
    let ratio = pred.size as f64 / prey.size as f64;
    ((ratio - 1.5) / 1.5).clamp(0.0, 1.0) * (1.0 - 0.8 * prey.adhesion as f64)
}

/// Per-strain working values for one step.
#[derive(Clone, Copy, Default)]
struct Work {
    // Reaction extents for the step, in each reaction's substrate units.
    ferment: f64,
    aerobic: f64,
    methanogen: f64,
    chemo: f64,
    /// Share of chemosynthesis that burns O₂.
    chemo_oxic: f64,
    anoxygenic: f64,
    oxygenic: f64,
    /// Gross new biomass before upkeep, mol C m⁻².
    growth: f64,
    /// Share of its nitrogen the strain can fix itself.
    fixing: f64,
    activity: f64,
    comfort: f64,
    stress_o2: f64,
}

/// Advances one cell. `pools` holds the cell's stored pools plus the
/// working CO₂ share; the caller turns the change in that share into an
/// atmospheric flux.
pub fn step_cell(
    registry: &Registry,
    env: &CellEnv,
    ids: &mut [StrainId],
    bio: &mut [f64],
    pools: &mut [f64; 7],
    rates: &crate::params::MutationRates,
    rng: &mut Rng,
) -> CellExchange {
    let mut out = CellExchange::default();
    let days = env.dt / DAY;
    let live: Vec<usize> = (0..ids.len())
        .filter(|&s| ids[s] != EMPTY && bio[s] > 0.0)
        .collect();
    if live.is_empty() {
        return out;
    }
    let genome = |s: usize| registry.genome(ids[s]);
    let mut w = [Work::default(); SLOTS];

    // --- Conditions each strain feels -----------------------------------
    for &s in &live {
        let g = genome(s);
        let fit = thermal_fitness(g, env);
        let stress = oxygen_stress(g, env.oxygen);
        w[s].comfort = (fit / 0.3).min(1.0);
        w[s].stress_o2 = stress;
        w[s].activity = fit * env.wetness * (1.0 - stress);
    }

    // --- Light: phototrophs shade each other ----------------------------
    let photo_weight = |s: usize| {
        let g = genome(s);
        bio[s]
            * (g.efficiency(Module::AnoxygenicPhotosynthesis)
                + g.efficiency(Module::OxygenicPhotosynthesis))
    };
    let total_photo: f64 = live.iter().map(|&s| photo_weight(s)).sum();
    let absorbed_light = if total_photo > 0.0 {
        env.light * env.dt * (1.0 - (-total_photo / SHADING_BIOMASS).exp())
    } else {
        0.0
    };

    // --- Potential reaction extents -------------------------------------
    let sat = |x: f64, k: f64| x.max(0.0) / (x.max(0.0) + k);
    let o2_avail = pools[O2].max(0.0);
    let oxic_share = sat(o2_avail, K_O2);
    for &s in &live {
        let g = genome(s);
        let b = bio[s];
        let a = w[s].activity;
        if a <= 0.0 {
            continue;
        }
        let affinity = 1.0 + 2.0 * g.motility as f64;
        let speed = (g.size as f64).powf(-0.25) * a * b * days;
        let e = |m: Module| g.efficiency(m) * allocation(g, m);
        if g.has(Module::Fermentation) {
            w[s].ferment = MU_FERMENT
                * e(Module::Fermentation)
                * speed
                * sat(pools[OM], K_ORGANICS / affinity)
                / Y_FERMENT;
        }
        if g.has(Module::AerobicRespiration) {
            w[s].aerobic = MU_AEROBIC
                * e(Module::AerobicRespiration)
                * speed
                * sat(pools[OM], K_ORGANICS / affinity)
                * oxic_share
                / Y_AEROBIC;
        }
        if g.has(Module::Methanogenesis) {
            w[s].methanogen = MU_METHANOGEN
                * e(Module::Methanogenesis)
                * speed
                * sat(pools[H2], K_H2 / affinity)
                * env.co2_factor
                / Y_METHANOGEN;
        }
        if g.has(Module::Chemosynthesis) {
            let y = Y_CHEMO_ANOXIC + (Y_CHEMO_OXIC - Y_CHEMO_ANOXIC) * oxic_share;
            w[s].chemo_oxic = oxic_share;
            w[s].chemo = MU_CHEMO
                * e(Module::Chemosynthesis)
                * speed
                * (0.25 + 0.75 * oxic_share)
                * sat(pools[RED], K_REDUCTANT / affinity)
                * env.co2_factor
                / y;
        }
        if total_photo > 0.0 {
            let share = absorbed_light / total_photo * b * a * env.co2_factor;
            for (m, mu, slot) in [
                (
                    Module::AnoxygenicPhotosynthesis,
                    MU_ANOXYGENIC,
                    &mut w[s].anoxygenic,
                ),
                (
                    Module::OxygenicPhotosynthesis,
                    MU_OXYGENIC,
                    &mut w[s].oxygenic,
                ),
            ] {
                if g.has(m) {
                    let light_limited = share * g.efficiency(m) * QUANTUM_YIELD * g.efficiency(m);
                    let capacity = mu * e(m) * speed * env.co2_factor;
                    *slot = light_limited.min(capacity);
                }
            }
            if w[s].anoxygenic > 0.0 {
                w[s].anoxygenic *= sat(pools[RED], K_REDUCTANT / affinity);
            }
        }
        if g.has(Module::NitrogenFixation) {
            let inhibition =
                env.oxygen / (env.oxygen + 0.05) * (1.0 - 0.7 * g.oxygen_tolerance as f64);
            w[s].fixing = e(Module::NitrogenFixation) * (1.0 - inhibition).max(0.0);
        }
    }

    // --- Competition for shared substrates -------------------------------
    let mut demand = [0.0; WORK];
    for &s in &live {
        let x = &w[s];
        demand[OM] += x.ferment + x.aerobic;
        demand[H2] += x.methanogen;
        demand[RED] += x.chemo + x.anoxygenic;
        demand[O2] += x.aerobic * (1.0 - Y_AEROBIC) + x.chemo * x.chemo_oxic;
        demand[CO2] += x.methanogen * (0.25 + Y_METHANOGEN)
            + x.chemo * (Y_CHEMO_ANOXIC + (Y_CHEMO_OXIC - Y_CHEMO_ANOXIC) * x.chemo_oxic)
            + x.anoxygenic
            + x.oxygenic;
    }
    let mut scale = [1.0; WORK];
    for k in 0..WORK {
        if demand[k] > 0.0 {
            scale[k] = (0.95 * pools[k].max(0.0) / demand[k]).min(1.0);
        }
    }
    for &s in &live {
        let x = &mut w[s];
        x.ferment *= scale[OM];
        x.aerobic *= scale[OM].min(scale[O2]);
        x.methanogen *= scale[H2].min(scale[CO2]);
        x.chemo *= scale[RED]
            .min(scale[CO2])
            .min(if x.chemo_oxic > 0.0 { scale[O2] } else { 1.0 });
        x.anoxygenic *= scale[RED].min(scale[CO2]);
        x.oxygenic *= scale[CO2];
        let y_chemo = Y_CHEMO_ANOXIC + (Y_CHEMO_OXIC - Y_CHEMO_ANOXIC) * x.chemo_oxic;
        x.growth = x.ferment * Y_FERMENT
            + x.aerobic * Y_AEROBIC
            + x.methanogen * Y_METHANOGEN
            + x.chemo * y_chemo
            + x.anoxygenic
            + x.oxygenic;
    }

    // --- Nutrients: N then P limit everyone's growth ---------------------
    let net =
        |s: usize, w: &[Work; SLOTS]| w[s].growth / (upkeep(genome(s)) * (1.0 + 0.3 * w[s].fixing));
    let n_demand: f64 = live
        .iter()
        .map(|&s| net(s, &w) * N_PER_C * (1.0 - w[s].fixing))
        .sum();
    let n_scale = if n_demand > 0.0 {
        (pools[NIT].max(0.0) / n_demand).min(1.0)
    } else {
        1.0
    };
    let mut limit = [1.0; SLOTS];
    for &s in &live {
        limit[s] = w[s].fixing + (1.0 - w[s].fixing) * n_scale;
    }
    let p_demand: f64 = live.iter().map(|&s| net(s, &w) * limit[s] * P_PER_C).sum();
    let p_scale = if p_demand > 0.0 {
        (pools[PHOS].max(0.0) / p_demand).min(1.0)
    } else {
        1.0
    };

    // --- Apply metabolism -------------------------------------------------
    for &s in &live {
        let r = limit[s] * p_scale;
        let x = &mut w[s];
        x.ferment *= r;
        x.aerobic *= r;
        x.methanogen *= r;
        x.chemo *= r;
        x.anoxygenic *= r;
        x.oxygenic *= r;
        x.growth *= r;
        let x = *x;
        let gained = net(s, &w);
        let y_chemo = Y_CHEMO_ANOXIC + (Y_CHEMO_OXIC - Y_CHEMO_ANOXIC) * x.chemo_oxic;

        pools[OM] -= x.ferment + x.aerobic;
        pools[H2] += x.ferment * (1.0 - Y_FERMENT) * H2_PER_FERMENT - x.methanogen;
        pools[RED] -= x.chemo + x.anoxygenic;
        pools[O2] += x.oxygenic - x.aerobic * (1.0 - Y_AEROBIC) - x.chemo * x.chemo_oxic;
        let fixed_c = x.methanogen * Y_METHANOGEN + x.chemo * y_chemo + x.anoxygenic + x.oxygenic;
        pools[CO2] += (x.ferment + x.aerobic) * 1.0 - x.methanogen * 0.25 - fixed_c
            // what upkeep burns goes back as CO₂
            + (x.growth - gained)
            // heterotrophs: the carbon they assimilated is not respired
            - (x.ferment * Y_FERMENT + x.aerobic * Y_AEROBIC);
        out.ch4 += x.methanogen * 0.25;
        out.fixed += fixed_c;

        let n_needed = gained * N_PER_C;
        let n_self = n_needed * (x.fixing / limit[s].max(1e-12)).min(1.0);
        out.n_fixed += n_self;
        pools[NIT] -= n_needed - n_self;
        pools[PHOS] -= gained * P_PER_C;
        bio[s] += gained;
    }

    // --- Predation --------------------------------------------------------
    let mut eaten = [[0.0; SLOTS]; SLOTS];
    let mut loss = [0.0; SLOTS];
    for &i in &live {
        let g = genome(i);
        if !g.has(Module::Phagocytosis) || w[i].activity <= 0.0 {
            continue;
        }
        let prey_total: f64 = live
            .iter()
            .filter(|&&j| j != i)
            .map(|&j| edibility(g, genome(j)) * bio[j])
            .sum();
        if prey_total <= 0.0 {
            continue;
        }
        let affinity = 1.0 + 2.0 * g.motility as f64;
        let intake = MU_PHAGO
            * g.efficiency(Module::Phagocytosis)
            * allocation(g, Module::Phagocytosis)
            * (g.size as f64).powf(-0.25)
            * w[i].activity
            * bio[i]
            * days
            * sat(prey_total, K_PREY / affinity)
            / Y_PHAGO;
        for &j in &live {
            if j != i {
                let share = edibility(g, genome(j)) * bio[j] / prey_total;
                eaten[i][j] = intake * share;
                loss[j] += intake * share;
            }
        }
    }
    for &j in &live {
        if loss[j] > 0.9 * bio[j] {
            let f = 0.9 * bio[j] / loss[j];
            for &i in &live {
                eaten[i][j] *= f;
            }
        }
    }
    for &i in &live {
        let total: f64 = live.iter().map(|&j| eaten[i][j]).sum();
        if total <= 0.0 {
            continue;
        }
        for &j in &live {
            bio[j] -= eaten[i][j];
        }
        let gained = total * Y_PHAGO / upkeep(genome(i));
        bio[i] += gained;
        pools[CO2] += total - gained;
        release(pools, &mut out, env.ocean, total - gained);
        // Endosymbiosis: now and then, a meal stays on as a partner.
        let p = rates.endosymbiosis * env.geologic_years * (total / bio[i]).min(1.0);
        if p > 0.0 && rng.chance(1.0 - (-p).exp()) {
            let mut pick = rng.f64() * total;
            let partner = live.iter().copied().find(|&j| {
                pick -= eaten[i][j];
                pick < 0.0
            });
            if let Some(g) = partner.and_then(|j| genome(i).with_symbiont(genome(j), rng)) {
                let b = bio[i] * SPAWN_SHARE * 5.0;
                bio[i] -= b;
                out.spawns.push(Spawn {
                    parent: ids[i],
                    genome: g,
                    biomass: b,
                });
            }
        }
    }

    // --- Toxins, stress and death ------------------------------------------
    let toxin_mass: f64 = live.iter().map(|&s| genome(s).toxin as f64 * bio[s]).sum();
    let total_mass: f64 = live.iter().map(|&s| bio[s]).sum();
    for &s in &live {
        let g = genome(s);
        let dormancy = g.dormancy as f64;
        let x = &w[s];
        let own_toxin = g.toxin as f64 * bio[s];
        let poison = (toxin_mass - own_toxin) / (total_mass + 0.01);
        let death = BASE_DEATH * (g.size as f64).powf(-0.2) * (1.0 - 0.3 * g.adhesion as f64)
            + COLD_HEAT_DEATH * (1.0 - x.comfort).powi(2) * (1.0 - 0.9 * dormancy)
            + DROUGHT_DEATH * (1.0 - env.wetness) * (1.0 - 0.9 * dormancy)
            + OXYGEN_DEATH * x.stress_o2 * (1.0 - 0.5 * dormancy)
            + TOXIN_DEATH * poison;
        let upkeep_rate =
            MAINTENANCE * (1.0 + g.motility as f64) * (1.0 - 0.8 * dormancy * (1.0 - x.comfort));
        let dead = bio[s] * (1.0 - (-death * days).exp());
        let burned = (bio[s] - dead) * (1.0 - (-upkeep_rate * days).exp());
        bio[s] -= dead + burned;
        pools[OM] += dead;
        pools[CO2] += burned;
        release(pools, &mut out, env.ocean, dead + burned);
    }

    // --- Mutation ---------------------------------------------------------
    for &s in &live {
        let b = bio[s];
        if b < 1e3 * MIN_BIOMASS {
            continue;
        }
        let size = (b / (b + MUTATION_REF)).sqrt();
        for structural in [false, true] {
            let rate = if structural {
                rates.structural
            } else {
                rates.point
            };
            let p = rate * env.geologic_years * size;
            if p > 0.0 && rng.chance(1.0 - (-p).exp()) {
                let parent = genome(s);
                let g = if structural {
                    parent.structural_mutant(rng)
                } else {
                    parent.point_mutant(rng)
                };
                if &g != parent {
                    let take = bio[s] * SPAWN_SHARE;
                    bio[s] -= take;
                    out.spawns.push(Spawn {
                        parent: ids[s],
                        genome: g,
                        biomass: take,
                    });
                }
            }
        }
    }

    // --- Populations that died out -----------------------------------------
    for &s in &live {
        if bio[s] < MIN_BIOMASS {
            let b = bio[s].max(0.0);
            pools[OM] += b;
            release(pools, &mut out, env.ocean, b);
            bio[s] = 0.0;
            ids[s] = EMPTY;
        }
    }
    out
}

/// Returns the nutrients in `carbon` mol of broken-down biomass: most to the
/// cell's dissolved pools, some (in the ocean) sinking to the deep.
fn release(pools: &mut [f64; 7], out: &mut CellExchange, ocean: bool, carbon: f64) {
    let n = carbon * N_PER_C;
    let p = carbon * P_PER_C;
    let sink = if ocean { EXPORT_FRACTION } else { 0.0 };
    pools[NIT] += n * (1.0 - sink);
    pools[PHOS] += p * (1.0 - sink);
    out.export_n += n * sink;
    out.export_p += p * sink;
}

/// Working pool array for a cell: its stored pools plus a CO₂ share.
pub fn working_pools(stored: &[f64; POOLS], co2_share: f64) -> [f64; 7] {
    let mut w = [0.0; 7];
    w[..POOLS].copy_from_slice(stored);
    w[CO2] = co2_share;
    w
}

/// Splits a working array back into stored pools and the CO₂ left.
pub fn split_pools(work: &[f64; 7], stored: &mut [f64; POOLS]) -> f64 {
    stored.copy_from_slice(&work[..POOLS]);
    work[CO2]
}

#[allow(dead_code)]
const _: () = assert!(MODULE_COUNT == 8);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::MutationRates;

    fn env(temperature: f64, vent: f64) -> CellEnv {
        CellEnv {
            ocean: true,
            temperature,
            wetness: 1.0,
            light: 50.0,
            vent,
            oxygen: 0.0,
            co2_factor: 1.0,
            dt: DAY,
            geologic_years: 0.0,
        }
    }

    #[test]
    fn base_bacteria_need_vents_on_a_temperate_planet() {
        let g = Genome::base_bacterium();
        let cold = thermal_fitness(&g, &env(285.0, 0.0));
        let vent = thermal_fitness(&g, &env(285.0, 1.0));
        assert!(cold < 0.1, "{cold}");
        assert!(vent > 4.0 * cold, "{vent}");
        // A strain adapted to cool water does well away from vents.
        let mut cool = g.clone();
        cool.temp_optimum = 288.0;
        assert!(thermal_fitness(&cool, &env(285.0, 0.0)) > 0.8);
    }

    fn one_strain(g: Genome, b: f64) -> (Registry, Vec<StrainId>, Vec<f64>) {
        let mut reg = Registry::default();
        let id = reg.insert(g, None, 0.0);
        let mut ids = vec![EMPTY; SLOTS];
        let mut bio = vec![0.0; SLOTS];
        ids[0] = id;
        bio[0] = b;
        (reg, ids, bio)
    }

    fn total_carbon(pools: &[f64; 7], bio: &[f64], ch4: f64) -> f64 {
        pools[OM] + pools[CO2] + bio.iter().sum::<f64>() + ch4
    }

    #[test]
    fn fermenters_grow_on_organics_and_conserve_carbon_and_nutrients() {
        let mut g = Genome::base_bacterium();
        g.temp_optimum = 290.0;
        let (reg, mut ids, mut bio) = one_strain(g, 1e-3);
        let mut pools = [0.0; 7];
        pools[OM] = 0.5;
        pools[NIT] = 0.1;
        pools[PHOS] = 0.01;
        pools[CO2] = 10.0;
        let c0 = total_carbon(&pools, &bio, 0.0);
        let n0 = pools[NIT] + bio.iter().sum::<f64>() * N_PER_C;
        let p0 = pools[PHOS] + bio.iter().sum::<f64>() * P_PER_C;
        let mut rng = Rng::from_seed(1);
        let rates = MutationRates::default();
        let e = env(290.0, 0.0);
        let mut ch4 = 0.0;
        let mut export = (0.0, 0.0);
        for _ in 0..30 {
            let x = step_cell(&reg, &e, &mut ids, &mut bio, &mut pools, &rates, &mut rng);
            ch4 += x.ch4;
            export.0 += x.export_n;
            export.1 += x.export_p;
        }
        assert!(bio[0] > 1e-2, "fermenters should bloom: {}", bio[0]);
        assert!(pools[H2] > 0.0, "fermentation releases hydrogen");
        let c1 = total_carbon(&pools, &bio, ch4);
        assert!((c1 - c0).abs() < 1e-12 * c0.max(1.0), "carbon {c0} → {c1}");
        let n1 = pools[NIT] + bio.iter().sum::<f64>() * N_PER_C + export.0;
        let p1 = pools[PHOS] + bio.iter().sum::<f64>() * P_PER_C + export.1;
        assert!((n1 - n0).abs() < 1e-12, "nitrogen {n0} → {n1}");
        assert!((p1 - p0).abs() < 1e-12, "phosphorus {p0} → {p1}");
    }

    #[test]
    fn oxygenic_photosynthesis_makes_oxygen_and_is_light_limited() {
        let mut g = Genome::base_bacterium();
        g.modules = [0.0; MODULE_COUNT];
        g.modules[Module::OxygenicPhotosynthesis.index()] = 0.8;
        g.temp_optimum = 290.0;
        g.oxygen_tolerance = 1.0;
        let run = |light: f64| {
            let (reg, mut ids, mut bio) = one_strain(g.clone(), 1e-2);
            let mut pools = [0.0; 7];
            pools[NIT] = 1.0;
            pools[PHOS] = 0.1;
            pools[CO2] = 100.0;
            let mut e = env(290.0, 0.0);
            e.light = light;
            let mut rng = Rng::from_seed(2);
            let mut fixed = 0.0;
            for _ in 0..20 {
                fixed += step_cell(
                    &reg,
                    &e,
                    &mut ids,
                    &mut bio,
                    &mut pools,
                    &MutationRates::default(),
                    &mut rng,
                )
                .fixed;
            }
            (fixed, pools[O2])
        };
        let (dim, o2_dim) = run(5.0);
        let (bright, o2_bright) = run(80.0);
        assert!(bright > 2.0 * dim, "{dim} vs {bright}");
        assert!(o2_bright > o2_dim && o2_dim > 0.0);
        // Every carbon fixed released one O₂ (none consumed here).
        assert!((o2_bright - bright).abs() < 1e-9 * bright);
    }

    #[test]
    fn oxygen_poisons_anaerobes() {
        let mut g = Genome::base_bacterium();
        g.temp_optimum = 290.0;
        let run = |oxygen: f64| {
            let (reg, mut ids, mut bio) = one_strain(g.clone(), 1e-2);
            let mut pools = [0.0; 7];
            pools[OM] = 0.5;
            pools[NIT] = 0.1;
            pools[PHOS] = 0.01;
            pools[O2] = oxygen * O2_SATURATION * 50.0;
            pools[CO2] = 10.0;
            let mut e = env(290.0, 0.0);
            e.oxygen = oxygen;
            let mut rng = Rng::from_seed(3);
            for _ in 0..30 {
                step_cell(
                    &reg,
                    &e,
                    &mut ids,
                    &mut bio,
                    &mut pools,
                    &MutationRates::default(),
                    &mut rng,
                );
            }
            bio[0]
        };
        assert!(run(1.0) < 1e-3 * run(0.0));
    }

    #[test]
    fn predators_eat_smaller_cells() {
        let mut reg = Registry::default();
        let mut small = Genome::base_bacterium();
        small.temp_optimum = 290.0;
        let mut big = small.clone();
        big.size = 5.0;
        big.modules[Module::Phagocytosis.index()] = 0.8;
        let prey = reg.insert(small, None, 0.0);
        let pred = reg.insert(big, None, 0.0);
        let mut ids = vec![EMPTY; SLOTS];
        let mut bio = vec![0.0; SLOTS];
        ids[0] = prey;
        ids[1] = pred;
        bio[0] = 0.05;
        bio[1] = 1e-3;
        let mut pools = [0.0; 7];
        pools[NIT] = 0.1;
        pools[PHOS] = 0.01;
        pools[CO2] = 10.0;
        let mut rng = Rng::from_seed(4);
        let e = env(290.0, 0.0);
        for _ in 0..10 {
            step_cell(
                &reg,
                &e,
                &mut ids,
                &mut bio,
                &mut pools,
                &MutationRates::default(),
                &mut rng,
            );
        }
        assert!(bio[1] > 3e-3, "predator grew: {}", bio[1]);
        assert!(bio[0] < 0.05);
    }

    #[test]
    fn mutation_spawns_new_strains_at_the_geologic_rate() {
        let mut g = Genome::base_bacterium();
        g.temp_optimum = 290.0;
        let (reg, mut ids, mut bio) = one_strain(g, 0.1);
        let mut pools = [0.0; 7];
        pools[OM] = 0.5;
        pools[NIT] = 0.1;
        pools[PHOS] = 0.01;
        pools[CO2] = 10.0;
        let mut e = env(290.0, 0.0);
        let mut rng = Rng::from_seed(5);
        let none = step_cell(
            &reg,
            &e,
            &mut ids,
            &mut bio,
            &mut pools,
            &MutationRates::default(),
            &mut rng,
        );
        assert!(none.spawns.is_empty(), "no geologic time, no mutation");
        e.geologic_years = 1000.0;
        let some = step_cell(
            &reg,
            &e,
            &mut ids,
            &mut bio,
            &mut pools,
            &MutationRates::default(),
            &mut rng,
        );
        assert!(!some.spawns.is_empty());
    }
}
