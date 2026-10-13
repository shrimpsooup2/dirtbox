//! `dirtbox climate`: spin up a planet's climate headless and report it.

use std::time::Instant;

use dirtbox_climate::{Climate, ClimateParams, ClimateZone, StepInputs, StepMode};
use dirtbox_grid::GeodesicGrid;
use dirtbox_planet::{Preset, Terrain};

pub struct ClimateRun {
    pub preset: Preset,
    pub years: f64,
    pub subdivision: u32,
    pub seed: u64,
    pub diurnal: bool,
    pub co2_ppm: Option<f64>,
    pub luminosity: Option<f64>,
    /// Print progress every this many planet days (default: every 5 s of wall time).
    pub report_days: Option<u64>,
}

pub fn preset_by_name(name: &str) -> Result<Preset, String> {
    let key = name.to_lowercase().replace(['-', '_', ' '], "");
    Preset::ALL
        .into_iter()
        .find(|p| {
            let n = p
                .name()
                .to_lowercase()
                .replace(['-', '_', ' ', '(', ')', ','], "");
            n.starts_with(&key)
        })
        .ok_or_else(|| {
            let names: Vec<&str> = Preset::ALL.iter().map(|p| p.name()).collect();
            format!(
                "unknown preset `{name}`; choose one of: {}",
                names.join(", ")
            )
        })
}

pub fn run(cfg: ClimateRun) -> Result<(), String> {
    let mut planet = cfg.preset.params();
    if let Some(ppm) = cfg.co2_ppm {
        planet.atmosphere.co2 = ppm * 1e-6 * planet.atmosphere.pressure_bar();
    }
    if let Some(l) = cfg.luminosity {
        planet.star.luminosity_factor = l;
    }
    planet.validate()?;
    let grid = GeodesicGrid::new(cfg.subdivision);
    let terrain = Terrain::generate(&grid, &planet, cfg.seed);
    let mut climate = Climate::new(
        &grid,
        &planet,
        &terrain,
        ClimateParams::default(),
        cfg.preset.starts_frozen(),
    );
    let calendar = planet.calendar();
    let (dt, mode) = if cfg.diurnal {
        (3600.0, StepMode::Diurnal)
    } else {
        (calendar.day_length.min(86_400.0), StepMode::DailyMean)
    };
    let steps = (cfg.years * calendar.year_length / dt).ceil() as u64;
    println!(
        "{} · {} cells · {} · {:.1} planet years ({} steps of {:.0} s)",
        cfg.preset.name(),
        grid.cell_count(),
        if cfg.diurnal { "diurnal" } else { "daily-mean" },
        cfg.years,
        steps,
        dt
    );
    let water0 = climate.total_water();
    let started = Instant::now();
    let mut t = 0.0;
    let mut last_report = Instant::now();
    for step in 0..steps {
        let report = climate.step(&StepInputs {
            grid: &grid,
            planet: &planet,
            terrain: &terrain,
            start: t,
            dt,
            mode,
            geologic_rate: 0.0,
        });
        planet.atmosphere.co2 += report.co2_change;
        t += dt;
        let report_due = match cfg.report_days {
            Some(days) => (step + 1) % days.max(1) == 0,
            None => last_report.elapsed().as_secs_f64() > 5.0,
        };
        if report_due || step + 1 == steps {
            let s = climate.summary(&grid, &terrain);
            eprintln!(
                "  year {:6.2}: mean {:6.1} K, ice {:4.1}%, cloud {:4.1}%, albedo {:.3}, precip {:.2} mm/d, vapour {:.1} kg/m², SW in {:.0} / IR out {:.0} W/m² ({} CG iters, {} substeps)",
                t / calendar.year_length,
                s.mean_temperature,
                s.ice_fraction * 100.0,
                s.cloud_fraction * 100.0,
                s.planetary_albedo,
                s.precipitation,
                s.column_water,
                s.absorbed,
                s.outgoing,
                report.diffusion_iterations,
                report.advection_substeps
            );
            last_report = Instant::now();
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "  {:.1} s wall ({:.2} s per planet year)",
        elapsed,
        elapsed / cfg.years
    );
    let water_drift = (climate.total_water() - water0) / water0.abs().max(1.0);
    println!("  water budget drift: {water_drift:.2e} (relative)");

    println!();
    println!("  Current state by latitude:");
    println!(
        "  {:>9}  {:>7} {:>7} {:>6} {:>6} {:>8} {:>8} {:>7}",
        "latitude", "T °C", "albedo", "cloud", "RH", "snow", "seaice", "SW in"
    );
    for band in (0..9).rev() {
        let lo = -90.0 + band as f64 * 20.0;
        let hi = lo + 20.0;
        let cells: Vec<usize> = grid
            .cells()
            .filter(|&c| (lo..hi).contains(&grid.latitude(c).to_degrees()))
            .map(|c| c as usize)
            .collect();
        let mean = |f: &dyn Fn(usize) -> f64| {
            cells
                .iter()
                .map(|&i| f(i) * grid.area(i as u32))
                .sum::<f64>()
                / cells.iter().map(|&i| grid.area(i as u32)).sum::<f64>()
        };
        let g = planet.body.gravity();
        println!(
            "  {:>4}–{:<4}  {:>7.1} {:>7.3} {:>6.2} {:>6.2} {:>8.0} {:>8.2} {:>7.0}",
            lo,
            hi,
            mean(&|i| climate.temperature[i] - 273.15),
            mean(&|i| climate.albedo[i]),
            mean(&|i| climate.cloud[i]),
            mean(&|i| climate.vapour[i]
                / dirtbox_climate::physics::saturated_column_water(climate.temperature[i], g)),
            mean(&|i| climate.snow[i]),
            mean(&|i| climate.sea_ice[i]),
            mean(&|i| climate.insolation[i]),
        );
    }
    let Some(year) = climate.stats.last_year.as_ref() else {
        println!("  (run at least two planet years for annual statistics)");
        return Ok(());
    };
    println!();
    println!("  Zonal means over the last full year (land only for precipitation):");
    println!(
        "  {:>9}  {:>8}  {:>8}  {:>10}  {:>10}",
        "latitude", "T all", "T land", "P all", "P land"
    );
    println!(
        "  {:>9}  {:>8}  {:>8}  {:>10}  {:>10}",
        "", "°C", "°C", "mm/yr", "mm/yr"
    );
    for band in (0..18).rev() {
        let lo = -90.0 + band as f64 * 10.0;
        let hi = lo + 10.0;
        let (mut t_all, mut t_land, mut p_all, mut p_land, mut w_all, mut w_land) =
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for c in grid.cells() {
            let lat = grid.latitude(c).to_degrees();
            if lat < lo || lat >= hi {
                continue;
            }
            let i = c as usize;
            let a = grid.area(c);
            let tm = year.annual_mean_temperature(i) - 273.15;
            let pm = year.annual_precipitation(i);
            t_all += tm * a;
            p_all += pm * a;
            w_all += a;
            if !terrain.is_ocean(c) {
                t_land += tm * a;
                p_land += pm * a;
                w_land += a;
            }
        }
        let f = |s: f64, w: f64| {
            if w > 0.0 {
                format!("{:.0}", s / w)
            } else {
                "-".into()
            }
        };
        let f1 = |s: f64, w: f64| {
            if w > 0.0 {
                format!("{:.1}", s / w)
            } else {
                "-".into()
            }
        };
        println!(
            "  {:>4}–{:<4}  {:>8}  {:>8}  {:>10}  {:>10}",
            lo,
            hi,
            f1(t_all, w_all),
            f1(t_land, w_land),
            f(p_all, w_all),
            f(p_land, w_land)
        );
    }
    println!();
    println!("  Land climate zones (share of land area):");
    let land: f64 = grid
        .cells()
        .filter(|&c| !terrain.is_ocean(c))
        .map(|c| grid.area(c))
        .sum();
    for zone in ClimateZone::ALL {
        let a: f64 = grid
            .cells()
            .filter(|&c| !terrain.is_ocean(c) && year.zones[c as usize] == zone)
            .map(|c| grid.area(c))
            .sum();
        if a > 0.0 {
            println!("    {:<20} {:5.1}%", zone.label(), a / land * 100.0);
        }
    }
    Ok(())
}
