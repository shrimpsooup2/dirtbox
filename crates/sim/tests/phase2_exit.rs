//! Phase 2 exit checks (design §13): base bacteria spread, new metabolisms
//! evolve, oxygen rises when oxygenic photosynthesis appears, methanogens
//! make methane, carbon is conserved and runs replay exactly.

use dirtbox_chemistry::{Genome, Milestone, Module, moles_per_bar};
use dirtbox_climate::StepMode;
use dirtbox_core::SimTime;
use dirtbox_planet::Preset;
use dirtbox_sim::{EvolutionSettings, World, WorldConfig};

fn early_world(subdivision: u32, seed_life: bool, tweak: impl FnOnce(&mut WorldConfig)) -> World {
    let mut config = WorldConfig {
        seed: 3,
        grid_subdivision: subdivision,
        seed_life,
        ..WorldConfig::from_preset(Preset::EarlyEarth)
    };
    tweak(&mut config);
    let mut w = World::with_spin_up(config, 1.0, &mut |_| {}).unwrap();
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

/// Atmospheric carbon (CO₂ + CH₄) plus all carbon in water, life and
/// sediments, mol.
fn total_carbon(w: &World) -> (f64, f64) {
    let s = w.state();
    let a = &s.planet().atmosphere;
    let air = (a.co2 + a.ch4) * moles_per_bar(s.planet());
    (air, s.life.carbon_inventory(s.climate.geometry()))
}

/// A strain adapted to temperate water, running one module well.
fn specialist(m: Module) -> Genome {
    let mut g = Genome::base_bacterium();
    g.modules = [0.0; dirtbox_chemistry::MODULE_COUNT];
    g.modules[m.index()] = 0.7;
    g.temp_optimum = 290.0;
    g.temp_tolerance = 15.0;
    g.oxygen_tolerance = 0.7;
    g
}

#[test]
fn base_bacteria_spread_from_the_vents() {
    let mut w = early_world(10, true, |_| {});
    let seeded = w.state().life.stats().occupied_cells;
    assert!(seeded > 0 && seeded <= 12, "{seeded}");
    run_years(&mut w, 4.0);
    let st = w.state().life.stats();
    assert!(
        st.occupied_cells > 5 * seeded,
        "{} cells after 4 years",
        st.occupied_cells
    );
    assert!(st.biomass > 0.0);
}

#[test]
fn a_new_metabolism_evolves() {
    let mut w = early_world(10, true, |c| {
        c.evolution = EvolutionSettings {
            mutation_multiplier: 3.0,
            structural_multiplier: 10.0,
            ..Default::default()
        };
    });
    let mut found = None;
    for _ in 0..40 {
        run_years(&mut w, 1.0);
        found = w
            .state()
            .life
            .events()
            .iter()
            .find(|e| matches!(e.milestone, Milestone::Metabolism(_)))
            .cloned();
        if found.is_some() {
            break;
        }
    }
    let e = found.expect("no new metabolism in 40 years");
    eprintln!(
        "first innovation: {:?} at year {:.1}",
        e.milestone,
        e.time / w.state().calendar().year_length
    );
    assert!(w.state().life.registry().total_ever() > 10);
}

#[test]
fn oxygen_rises_when_oxygenic_photosynthesis_appears() {
    let mut control = early_world(10, false, |_| {});
    let mut w = early_world(10, false, |_| {});
    assert_eq!(w.state().planet().atmosphere.o2, 0.0);
    let ocean: Vec<u32> = {
        let s = w.state();
        s.grid
            .cells()
            .filter(|&c| s.terrain.is_ocean(c))
            .step_by(7)
            .collect()
    };
    for &c in &ocean {
        w.state_mut()
            .introduce_strain(c, specialist(Module::OxygenicPhotosynthesis), 1e-3);
    }
    run_years(&mut w, 8.0);
    run_years(&mut control, 8.0);
    let o2 = w.state().planet().atmosphere.o2;
    let st = w.state().life.stats();
    eprintln!(
        "O₂ {:.2} ppm, productivity {:.2e} mol/yr",
        w.state().planet().atmosphere.ppm(o2),
        st.productivity
    );
    assert!(st.productivity > 1e13);
    assert!(
        w.state().planet().atmosphere.ppm(o2) > 1.0,
        "O₂ only {o2:e} bar"
    );
    assert_eq!(
        control.state().planet().atmosphere.o2,
        0.0,
        "no life, no oxygen"
    );
}

#[test]
fn methanogens_make_methane() {
    let no_methane = |c: &mut WorldConfig| c.planet.atmosphere.ch4 = 0.0;
    let mut control = early_world(10, false, no_methane);
    let mut w = early_world(10, false, no_methane);
    let vents: Vec<u32> = {
        let s = w.state();
        s.grid
            .cells()
            .filter(|&c| s.terrain.vents[c as usize] > 0.0)
            .collect()
    };
    assert!(!vents.is_empty());
    for &c in &vents {
        let mut g = specialist(Module::Methanogenesis);
        g.temp_optimum = 320.0;
        w.state_mut().introduce_strain(c, g, 1e-3);
    }
    run_years(&mut w, 3.0);
    run_years(&mut control, 3.0);
    let ch4 = w.state().planet().atmosphere.ch4;
    assert_eq!(control.state().planet().atmosphere.ch4, 0.0);
    assert!(ch4 > 0.0, "methanogens released no methane");
    assert!(w.state().life.stats().module_biomass[Module::Methanogenesis.index()] > 0.0);
}

#[test]
fn carbon_is_conserved() {
    let mut w = early_world(10, true, |c| {
        c.evolution = EvolutionSettings {
            mutation_multiplier: 5.0,
            structural_multiplier: 20.0,
            ..Default::default()
        };
    });
    let (air0, life0) = total_carbon(&w);
    run_years(&mut w, 6.0);
    let (air1, life1) = total_carbon(&w);
    let drift = (air1 + life1) - (air0 + life0);
    let moved = (life1 - life0).abs();
    eprintln!("carbon moved into water/life/sediment {moved:.3e} mol, drift {drift:.3e} mol");
    assert!(moved > 1e10, "the biosphere should have taken up carbon");
    // Rounding in the atmosphere's (large) partial pressures is all that's allowed.
    assert!(
        drift.abs() < 1e-12 * air0 * 2000.0 + 1e-9 * moved,
        "drift {drift:e}"
    );
}

#[test]
fn life_runs_replay_exactly() {
    // Large enough for the parallel paths.
    let run = || {
        let mut w = early_world(16, true, |c| {
            c.evolution.structural_multiplier = 20.0;
        });
        run_years(&mut w, 0.3);
        (w.state_hash(), w.state().life.registry().total_ever())
    };
    let (h1, n1) = run();
    let (h2, n2) = run();
    assert!(n1 > 1, "some mutants should have appeared");
    assert_eq!((h1, n1), (h2, n2));
}
