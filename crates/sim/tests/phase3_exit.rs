//! Phase 3 exit checks (design §13): colonies of specialised cells become
//! multicellular producers, vegetation follows climate, taller producers
//! evolve where neighbours shade them, and carbon stays conserved.

use dirtbox_chemistry::{Genome, MODULE_COUNT, Milestone, Module, moles_per_bar};
use dirtbox_climate::{ClimateZone, StepMode};
use dirtbox_core::SimTime;
use dirtbox_organism::PlantGenome;
use dirtbox_planet::Preset;
use dirtbox_sim::{EvolutionSettings, World, WorldConfig};

fn world(preset: Preset, subdivision: u32, tweak: impl FnOnce(&mut WorldConfig)) -> World {
    let mut config = WorldConfig {
        seed: 5,
        grid_subdivision: subdivision,
        seed_life: false,
        ..WorldConfig::from_preset(preset)
    };
    tweak(&mut config);
    let mut w = World::with_spin_up(config, 2.0, &mut |_| {}).unwrap();
    w.set_climate_mode(StepMode::DailyMean);
    w.set_climate_stride(5);
    w
}

fn run_years(w: &mut World, years: f64) {
    let s = w.state();
    let year = s.calendar().year_length / s.time_model().season_speed;
    let end = SimTime::from_seconds_ceil(s.now().seconds() + years * year);
    w.run_until(end);
}

fn land(w: &World) -> Vec<u32> {
    let s = w.state();
    s.grid.cells().filter(|&c| !s.terrain.is_ocean(c)).collect()
}

fn plant(w: &mut World, genomes: &[PlantGenome]) {
    let cells = land(w);
    for g in genomes {
        w.state_mut().introduce_plants(&cells, g.clone(), 0.05);
    }
}

/// Mean of `f` over land cells in the given climate zones.
fn zone_mean(w: &World, zones: &[ClimateZone], f: impl Fn(u32) -> f64) -> f64 {
    let s = w.state();
    let year = s.climate.stats.last_year.as_ref().expect("a full year of climate");
    let cells: Vec<u32> = land(w)
        .into_iter()
        .filter(|&c| zones.contains(&year.zones[c as usize]))
        .collect();
    assert!(!cells.is_empty(), "no cells in {zones:?}");
    cells.iter().map(|&c| f(c)).sum::<f64>() / cells.len() as f64
}

#[test]
fn colonies_of_specialised_cells_become_multicellular() {
    let mut w = world(Preset::EarlyEarth, 10, |c| {
        c.evolution.structural_multiplier = 10.0;
    });
    let coast: Vec<u32> = {
        let s = w.state();
        s.grid
            .cells()
            .filter(|&c| {
                s.terrain.is_ocean(c) && s.grid.neighbours(c).iter().any(|&j| !s.terrain.is_ocean(j))
            })
            .collect()
    };
    assert!(!coast.is_empty());
    let mut colony = Genome::base_bacterium();
    colony.modules = [0.0; MODULE_COUNT];
    colony.modules[Module::OxygenicPhotosynthesis.index()] = 0.7;
    colony.temp_optimum = 288.0;
    colony.temp_tolerance = 15.0;
    colony.oxygen_tolerance = 0.9;
    colony.adhesion = 0.7;
    colony.differentiation = 0.7;
    for &c in &coast {
        w.state_mut().introduce_strain(c, colony.clone(), 0.02);
    }
    run_years(&mut w, 2.0);
    let s = w.state();
    assert!(s.life.has_reached(Milestone::Multicellular), "no graduation");
    assert!(s.vegetation.registry().total_ever() > 0);
    assert!(s.vegetation.stats().water_biomass > 0.0, "seaweeds should grow");
}

#[test]
fn vegetation_follows_climate() {
    let mut w = world(Preset::EarthModern, 10, |_| {});
    plant(&mut w, &[PlantGenome::moss(), PlantGenome::grass(), PlantGenome::tree()]);
    run_years(&mut w, 15.0);
    let s = w.state();
    let lai = |c: u32| s.vegetation.leaf_area()[c as usize];
    let wet = zone_mean(&w, &[ClimateZone::Rainforest, ClimateZone::Temperate, ClimateZone::Monsoon], lai);
    let desert = zone_mean(&w, &[ClimateZone::Desert], lai);
    let ice = zone_mean(&w, &[ClimateZone::IceCap], lai);
    let height = |c: u32| s.vegetation.canopy_height(c);
    let wet_height = zone_mean(&w, &[ClimateZone::Rainforest, ClimateZone::Temperate], height);
    let cold_height = zone_mean(&w, &[ClimateZone::Tundra], height);
    eprintln!("LAI wet {wet:.2} desert {desert:.2} ice {ice:.2}; canopy wet {wet_height:.1} m tundra {cold_height:.1} m");
    assert!(wet > 1.0, "humid zones should be green: LAI {wet}");
    assert!(desert < 0.2 * wet, "deserts should be sparse: {desert} vs {wet}");
    assert!(ice < 0.05, "ice caps stay bare: {ice}");
    assert!(wet_height > 3.0 * cold_height.max(0.5), "forests where it's warm and wet");
    assert!(s.life.has_reached(Milestone::PlantsOnLand));
}

#[test]
fn taller_producers_evolve_under_shading() {
    let mut w = world(Preset::EarthModern, 8, |c| {
        c.evolution = EvolutionSettings {
            mutation_multiplier: 6.0,
            ..Default::default()
        };
        c.vegetation.time_lapse = 50.0;
    });
    let mut short = PlantGenome::grass();
    short.max_height = 0.3;
    short.wood_share = 0.15;
    short.nitrogen_fixing = 0.3;
    plant(&mut w, &[short.clone()]);
    run_years(&mut w, 12.0);
    let s = w.state();
    let v = &s.vegetation;
    // Biomass-weighted genetic height ceiling where the canopy is dense,
    // and where it is sparse.
    let mean_ceiling = |dense: bool| {
        let (mut sum, mut weight) = (0.0, 0.0);
        for c in land(&w) {
            if (v.leaf_area()[c as usize] > 2.0) != dense {
                continue;
            }
            for k in v.cohorts(c) {
                let g = &v.lineage(k.lineage).unwrap().genome;
                sum += g.max_height as f64 * k.biomass();
                weight += k.biomass();
            }
        }
        sum / weight.max(1e-30)
    };
    let (dense, sparse) = (mean_ceiling(true), mean_ceiling(false));
    let dense_cells = land(&w).iter().filter(|&&c| v.leaf_area()[c as usize] > 2.0).count();
    let mut ceilings: Vec<f32> = v.registry().iter().map(|(_, r)| r.genome.max_height).collect();
    ceilings.sort_by(f32::total_cmp);
    eprintln!(
        "{dense_cells} dense cells; {} lineages; ceilings min {:.2} median {:.2} max {:.2}",
        ceilings.len(),
        ceilings.first().unwrap_or(&0.0),
        ceilings.get(ceilings.len() / 2).unwrap_or(&0.0),
        ceilings.last().unwrap_or(&0.0)
    );
    eprintln!(
        "evolved height ceiling: dense canopy {dense:.2} m, sparse {sparse:.2} m (started at {:.2} m); tallest {:.1} m",
        short.max_height,
        v.stats().tallest
    );
    assert!(dense > 2.0 * short.max_height as f64, "no taller plants evolved");
    assert!(dense > sparse, "height should pay most where plants shade each other");
}

#[test]
fn plants_wait_for_an_ozone_layer() {
    let mut early = world(Preset::EarlyEarth, 8, |_| {});
    let mut modern = world(Preset::EarthModern, 8, |_| {});
    for w in [&mut early, &mut modern] {
        plant(w, &[PlantGenome::moss(), PlantGenome::grass()]);
        run_years(w, 3.0);
    }
    let land_biomass = |w: &World| w.state().vegetation.stats().land_biomass;
    eprintln!("land plants without O₂ {:.2e}, with {:.2e}", land_biomass(&early), land_biomass(&modern));
    assert!(land_biomass(&early) < 0.01 * land_biomass(&modern));
}

#[test]
fn carbon_is_conserved_with_plants() {
    let mut w = world(Preset::EarthModern, 8, |c| c.seed_life = true);
    plant(&mut w, &[PlantGenome::moss(), PlantGenome::tree()]);
    let total = |w: &World| {
        let s = w.state();
        let a = &s.planet().atmosphere;
        let geo = s.climate.geometry();
        (
            (a.co2 + a.ch4) * moles_per_bar(s.planet()),
            s.life.carbon_inventory(geo) + s.vegetation.carbon_inventory(geo),
        )
    };
    let (air0, life0) = total(&w);
    run_years(&mut w, 5.0);
    let (air1, life1) = total(&w);
    let drift = (air1 + life1) - (air0 + life0);
    eprintln!("carbon into life {:.3e} mol, drift {drift:.3e} mol", life1 - life0);
    assert!(life1 - life0 > 1e13, "plants should store carbon");
    assert!(drift.abs() < 1e-11 * air0 * 2000.0 + 1e-9 * (life1 - life0).abs(), "drift {drift:e}");
}

#[test]
fn vegetation_replays_exactly() {
    let run = || {
        let mut w = world(Preset::EarthModern, 16, |c| {
            c.vegetation.time_lapse = 40.0;
            c.evolution.mutation_multiplier = 3.0;
        });
        plant(&mut w, &[PlantGenome::grass(), PlantGenome::tree()]);
        run_years(&mut w, 1.2);
        (w.state_hash(), w.state().vegetation.registry().total_ever())
    };
    let (h1, n1) = run();
    assert!(n1 > 2, "mutant lineages should have appeared");
    assert_eq!((h1, n1), run());
}
