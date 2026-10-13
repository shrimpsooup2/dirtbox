//! `dirtbox life`: run the microbial era headless and report what evolves.

use std::time::Instant;

use dirtbox_chemistry::{Milestone, Module, pool};
use dirtbox_climate::StepMode;
use dirtbox_core::SimTime;
use dirtbox_planet::Preset;
use dirtbox_sim::{EvolutionSettings, World, WorldConfig};

pub struct LifeRun {
    pub preset: Preset,
    pub years: f64,
    pub subdivision: u32,
    pub seed: u64,
    pub stride: u32,
    pub mutation: f64,
    pub structural: f64,
    pub until: Option<Milestone>,
    pub report_years: f64,
}

pub fn milestone_by_name(name: &str) -> Result<Milestone, String> {
    let key = name.to_lowercase().replace(['-', '_', ' '], "");
    Milestone::targets()
        .into_iter()
        .find(|m| {
            m.label()
                .to_lowercase()
                .replace([' ', '(', ')', '-'], "")
                .starts_with(&key)
        })
        .ok_or_else(|| {
            let names: Vec<String> = Milestone::targets().iter().map(|m| m.label()).collect();
            format!(
                "unknown milestone `{name}`; choose one of: {}",
                names.join(", ")
            )
        })
}

pub fn run(cfg: LifeRun) -> Result<(), String> {
    let config = WorldConfig {
        seed: cfg.seed,
        grid_subdivision: cfg.subdivision,
        evolution: EvolutionSettings {
            mutation_multiplier: cfg.mutation,
            structural_multiplier: cfg.structural,
            ..Default::default()
        },
        ..WorldConfig::from_preset(cfg.preset)
    };
    let started = Instant::now();
    let mut world = World::with_spin_up(config, 3.0, &mut |_| {})?;
    world.set_climate_mode(StepMode::DailyMean);
    world.set_climate_stride(cfg.stride);
    let s = world.state();
    println!(
        "{} · {} cells · climate every {} day(s) · spin-up {:.1} s",
        cfg.preset.name(),
        s.grid.cell_count(),
        cfg.stride,
        started.elapsed().as_secs_f64()
    );
    let year_sim = s.calendar().year_length / s.time_model().season_speed;
    let accel = s.time_model().geologic_acceleration;
    let mut printed_events = 0;
    let started = Instant::now();
    let mut year = 0.0;
    println!(
        "{:>7} {:>10} {:>7} {:>6} {:>9} {:>9} {:>9} {:>8}  dominant metabolisms",
        "year", "biomass", "strains", "cells", "O₂ ppm", "CH₄ ppm", "CO₂ ppm", "T °C"
    );
    while year < cfg.years {
        year = (year + cfg.report_years).min(cfg.years);
        let end = SimTime::from_seconds_ceil(year * year_sim);
        let start_now = world.state().now();
        if let Some(m) = cfg.until {
            let left = (end.seconds() - start_now.seconds()) / year_sim;
            world.run_until_milestone(m, left, None);
        } else {
            world.run_until(end);
        }
        let s = world.state();
        let life = &s.life;
        let st = life.stats();
        let a = &s.planet().atmosphere;
        let mut mods: Vec<(Module, f64)> = Module::ALL
            .iter()
            .map(|&m| (m, st.module_biomass[m.index()]))
            .filter(|x| x.1 > 0.0)
            .collect();
        mods.sort_by(|x, y| y.1.total_cmp(&x.1));
        let mods: Vec<String> = mods
            .iter()
            .take(4)
            .map(|(m, b)| format!("{} {:.0}%", short(*m), 100.0 * b / st.biomass.max(1e-30)))
            .collect();
        let summary = s.climate.summary(&s.grid, &s.terrain);
        let now_years = s.now().seconds() / year_sim;
        println!(
            "{:>7.1} {:>10.3e} {:>7} {:>6} {:>9.3} {:>9.3} {:>9.0} {:>8.1}  {}",
            now_years,
            st.biomass,
            st.strains,
            st.occupied_cells,
            a.ppm(a.o2),
            a.ppm(a.ch4),
            a.ppm(a.co2),
            summary.mean_temperature - 273.15,
            mods.join(", ")
        );
        for e in &life.events()[printed_events..] {
            println!(
                "        ★ year {:.1} (≈{:.0} kyr of evolution): {}",
                e.time / s.calendar().year_length,
                e.time / s.calendar().year_length * accel / 1e3,
                e.milestone.describe()
            );
        }
        printed_events = life.events().len();
        if let Some(m) = cfg.until {
            if life.has_reached(m) {
                println!("Reached “{}”.", m.label());
                break;
            }
        }
        if !life.is_alive() && life.has_reached(Milestone::LifeBegins) {
            println!("Life died out.");
            break;
        }
    }
    let s = world.state();
    let ocean_cells: Vec<u32> = s.grid.cells().filter(|&c| s.terrain.is_ocean(c)).collect();
    let mean = |k: usize| {
        ocean_cells.iter().map(|&c| s.life.pool(c, k)).sum::<f64>()
            / ocean_cells.len().max(1) as f64
    };
    println!(
        "Ocean means (mol/m²): organics {:.3e} · H₂ {:.3e} · reductants {:.3e} · O₂ {:.3e} · N {:.3e} · P {:.3e}",
        mean(pool::ORGANICS),
        mean(pool::H2),
        mean(pool::REDUCTANTS),
        mean(pool::O2),
        mean(pool::NITROGEN),
        mean(pool::PHOSPHORUS)
    );
    let st = s.life.stats();
    println!(
        "Productivity {:.2e} mol C/yr · organics {:.2e} mol · buried {:.2e} mol · strains ever {} · wall {:.1} s",
        st.productivity,
        st.organics,
        st.buried,
        s.life.registry().total_ever(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn short(m: Module) -> &'static str {
    match m {
        Module::Fermentation => "ferm",
        Module::Methanogenesis => "meth",
        Module::Chemosynthesis => "chemo",
        Module::AnoxygenicPhotosynthesis => "anoxPS",
        Module::OxygenicPhotosynthesis => "oxyPS",
        Module::AerobicRespiration => "aerobic",
        Module::NitrogenFixation => "Nfix",
        Module::Phagocytosis => "phago",
    }
}
