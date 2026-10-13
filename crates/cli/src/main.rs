//! `dirtbox`: headless command-line entry point.

mod climate_cmd;
mod life_cmd;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use dirtbox_core::{Calendar, TimeModel, consts::EARTH_RADIUS};
use dirtbox_grid::GeodesicGrid;
use dirtbox_hw::{
    BenchConfig, BudgetInputs, HardwareInfo, detect, recommend, run_benchmark, speed_for_population,
};

const USAGE: &str = "\
dirtbox — planet-scale life simulator (headless tools)

USAGE:
    dirtbox <command> [options]

COMMANDS:
    hwinfo       Show detected hardware
    calibrate    Benchmark this machine and recommend a population budget
                   --seconds <s>        time per measurement (default 2)
                   --threads <n>        worker threads (default: all cores)
                   --population <n>     also estimate speed for this population
                   --season-speed <k>   planet seconds per sim second (default 72)
    grid         Build a geodesic grid and print its statistics
                   --subdivision <n>    10n²+2 cells (default 64)
                   --cells <n>          pick the smallest subdivision with ≥ n cells
                   --radius-km <r>      planet radius (default 6371)
    climate      Spin up a planet's climate headless and print zonal means
                   --preset <name>      earth, early, cold, tidally, super, ocean, snowball (default earth)
                   --years <y>          planet years to run (default 6)
                   --subdivision <n>    grid subdivision (default 16)
                   --seed <s>           terrain seed (default 1)
                   --diurnal true       hourly steps with day and night (slower)
                   --co2-ppm <ppm>      override CO₂
                   --luminosity <f>     override the star's luminosity factor
                   --report-days <d>    print progress every d planet days
    life         Run the microbial era headless and report what evolves
                   --preset <name>      planet preset (default early)
                   --years <y>          planet years to run (default 50)
                   --subdivision <n>    grid subdivision (default 16)
                   --seed <s>           world seed (default 1)
                   --stride <d>         climate steps every d days (default 5)
                   --mutation <x>       point-mutation multiplier (default 1)
                   --structural <x>     structural-mutation multiplier (default 1)
                   --until <milestone>  stop at a milestone, e.g. oxygenic, 'oxygen in', phago
                   --report-years <y>   print a line every y planet years (default 5)
                   --plants <kind>      plant moss, grass, tree or mix on all land at the start
                   --time-lapse <x>     plant years per planet year (default 20)
    help         Show this message
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    };
    let result = match command.as_str() {
        "hwinfo" => Options::parse(rest, &[]).map(|_| hwinfo()),
        "calibrate" => Options::parse(rest, &["seconds", "threads", "population", "season-speed"])
            .and_then(calibrate),
        "grid" => Options::parse(rest, &["subdivision", "cells", "radius-km"]).and_then(grid),
        "climate" => Options::parse(
            rest,
            &[
                "preset",
                "years",
                "subdivision",
                "seed",
                "diurnal",
                "co2-ppm",
                "luminosity",
                "report-days",
            ],
        )
        .and_then(climate),
        "life" => Options::parse(
            rest,
            &[
                "preset",
                "years",
                "subdivision",
                "seed",
                "stride",
                "mutation",
                "structural",
                "until",
                "report-years",
                "plants",
                "time-lapse",
            ],
        )
        .and_then(life),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command `{other}`; try `dirtbox help`")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--name value` pairs, restricted to an allowed set.
struct Options(Vec<(String, String)>);

impl Options {
    fn parse(args: &[String], allowed: &[&str]) -> Result<Self, String> {
        let mut pairs = Vec::new();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let name = arg
                .strip_prefix("--")
                .filter(|n| allowed.contains(n))
                .ok_or_else(|| format!("unexpected argument `{arg}`"))?;
            let value = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
            pairs.push((name.to_string(), value.clone()));
        }
        Ok(Options(pairs))
    }

    fn get<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, String> {
        self.0
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| {
                v.parse()
                    .map_err(|_| format!("invalid value `{v}` for --{name}"))
            })
            .transpose()
    }
}

fn hwinfo() {
    print_hardware(&detect());
}

fn print_hardware(hw: &HardwareInfo) {
    println!("Hardware");
    println!(
        "  CPU   {} ({} logical cores)",
        hw.cpu_model.as_deref().unwrap_or("unknown model"),
        hw.logical_cores
    );
    let ram = |b: Option<u64>| {
        b.map_or("unknown".to_string(), |b| {
            format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64)
        })
    };
    println!(
        "  RAM   {} total, {} available",
        ram(hw.total_ram_bytes),
        ram(hw.available_ram_bytes)
    );
    println!("  GPU   reported by the 3D viewer (not built yet)");
    println!("  OS    {}", hw.os);
}

fn calibrate(opts: Options) -> Result<(), String> {
    let seconds: f64 = opts.get("seconds")?.unwrap_or(2.0);
    if !(seconds > 0.0 && seconds.is_finite()) {
        return Err("--seconds must be > 0".into());
    }
    let hw = detect();
    let threads: usize = opts.get("threads")?.unwrap_or(hw.logical_cores);
    if threads == 0 {
        return Err("--threads must be ≥ 1".into());
    }
    let time = TimeModel {
        season_speed: opts
            .get("season-speed")?
            .unwrap_or(TimeModel::default().season_speed),
        ..TimeModel::default()
    };
    time.validate()?;
    let population: Option<u64> = opts.get("population")?;

    print_hardware(&hw);
    println!();
    println!("Benchmark (proxy organism step: sensing + 50-neuron CTRNN + movement)");
    let measure = Duration::from_secs_f64(seconds);
    let single = run_benchmark(&BenchConfig {
        threads: 1,
        measure,
        ..Default::default()
    });
    println!(
        "  1 thread    {:>8} organism-steps/s",
        si(single.steps_per_second())
    );
    let all = if threads > 1 {
        let r = run_benchmark(&BenchConfig {
            threads,
            measure,
            ..Default::default()
        });
        println!(
            "  {threads} threads   {:>8} organism-steps/s ({:.1}× scaling)",
            si(r.steps_per_second()),
            r.steps_per_second() / single.steps_per_second()
        );
        r
    } else {
        single
    };

    let calendar = Calendar::default();
    let year_sim_seconds = calendar.year_length / time.season_speed;
    let inputs = BudgetInputs {
        throughput: all.steps_per_second(),
        mean_step_seconds: BudgetInputs::DEFAULT_MEAN_STEP_SECONDS,
        headroom: BudgetInputs::DEFAULT_HEADROOM,
        bytes_per_organism: BudgetInputs::DEFAULT_BYTES_PER_ORGANISM,
        available_ram_bytes: hw.available_ram_bytes,
        planet_year_sim_seconds: year_sim_seconds,
    };

    println!();
    println!(
        "Recommended population budget (season speed {}: an Earth year lasts {:.2} sim-days)",
        time.season_speed,
        year_sim_seconds / 86_400.0
    );
    println!(
        "  {:<10} {:>11} {:>7}   {:<18} Use it to",
        "Tier", "Organisms", "Speed", "Planet year takes"
    );
    for r in recommend(&inputs) {
        println!(
            "  {:<10} {:>11} {:>6}×   {:<18} {}{}",
            r.tier.name,
            group(r.population),
            r.tier.speed,
            duration(r.wall_seconds_per_planet_year),
            r.tier.purpose,
            if r.ram_limited {
                " (limited by RAM)"
            } else {
                ""
            }
        );
    }
    if let Some(n) = population {
        let (speed, over_ram) = speed_for_population(&inputs, n);
        println!();
        println!(
            "  With {} organisms: about {:.0}× speed; a planet year takes {}{}",
            group(n),
            speed,
            duration(year_sim_seconds / speed),
            if over_ram {
                ". Warning: this exceeds the RAM budget"
            } else {
                ""
            }
        );
    }
    println!();
    println!("Notes:");
    println!(
        "  - Speed is sim seconds per wall second. The budget sizes the world; ecology decides the real head count."
    );
    println!(
        "  - These figures come from a stand-in workload. They will be re-measured once real organisms exist (Phases 4–5)."
    );
    Ok(())
}

fn climate(opts: Options) -> Result<(), String> {
    let preset = climate_cmd::preset_by_name(
        &opts
            .get::<String>("preset")?
            .unwrap_or_else(|| "earth".into()),
    )?;
    climate_cmd::run(climate_cmd::ClimateRun {
        preset,
        years: opts.get("years")?.unwrap_or(6.0),
        subdivision: opts.get("subdivision")?.unwrap_or(16),
        seed: opts.get("seed")?.unwrap_or(1),
        diurnal: opts.get("diurnal")?.unwrap_or(false),
        co2_ppm: opts.get("co2-ppm")?,
        luminosity: opts.get("luminosity")?,
        report_days: opts.get("report-days")?,
    })
}

fn grid(opts: Options) -> Result<(), String> {
    let subdivision = match (opts.get::<u32>("subdivision")?, opts.get::<usize>("cells")?) {
        (Some(_), Some(_)) => return Err("use either --subdivision or --cells, not both".into()),
        (Some(n), None) => n,
        (None, Some(c)) => GeodesicGrid::subdivision_for_cells(c),
        (None, None) => 64,
    };
    if subdivision == 0 || subdivision > 2048 {
        return Err("--subdivision must be in 1..=2048".into());
    }
    let radius_km: f64 = opts.get("radius-km")?.unwrap_or(EARTH_RADIUS / 1000.0);

    let t0 = Instant::now();
    let g = GeodesicGrid::new(subdivision);
    let build = t0.elapsed();

    let r2 = radius_km * radius_km;
    let areas: Vec<f64> = g.cells().map(|c| g.area(c) * r2).collect();
    let (min, max) = areas
        .iter()
        .fold((f64::MAX, 0.0f64), |(lo, hi), &a| (lo.min(a), hi.max(a)));
    let mean = areas.iter().sum::<f64>() / areas.len() as f64;
    let pentagons = g.cells().filter(|&c| g.neighbours(c).len() == 5).count();

    println!("Geodesic grid, subdivision {subdivision}");
    println!(
        "  cells          {} ({} hexagons, {pentagons} pentagons)",
        group(g.cell_count() as u64),
        group((g.cell_count() - pentagons) as u64)
    );
    println!("  planet radius  {radius_km} km");
    println!(
        "  cell area      mean {mean:.1} km², min {min:.1}, max {max:.1} (max/min {:.3})",
        max / min
    );
    println!(
        "  cell spacing   ~{:.1} km",
        (mean * 2.0 / 3f64.sqrt()).sqrt()
    );
    println!("  built in       {:.0} ms", build.as_secs_f64() * 1000.0);
    Ok(())
}

/// 12_345_678 → "12.3 M".
fn si(v: f64) -> String {
    match v {
        v if v >= 1e9 => format!("{:.2} G", v / 1e9),
        v if v >= 1e6 => format!("{:.2} M", v / 1e6),
        v if v >= 1e3 => format!("{:.1} k", v / 1e3),
        v => format!("{v:.0}"),
    }
}

/// 1234567 → "1,234,567".
fn group(v: u64) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn duration(seconds: f64) -> String {
    match seconds {
        s if s < 90.0 => format!("{s:.0} s"),
        s if s < 90.0 * 60.0 => format!("{:.1} min", s / 60.0),
        s if s < 48.0 * 3600.0 => format!("{:.1} h", s / 3600.0),
        s => format!("{:.1} days", s / 86_400.0),
    }
}

fn life(o: Options) -> Result<(), String> {
    let preset =
        climate_cmd::preset_by_name(&o.get::<String>("preset")?.unwrap_or("early".into()))?;
    let until = o
        .get::<String>("until")?
        .map(|m| life_cmd::milestone_by_name(&m))
        .transpose()?;
    life_cmd::run(life_cmd::LifeRun {
        preset,
        years: o.get("years")?.unwrap_or(50.0),
        subdivision: o.get("subdivision")?.unwrap_or(16),
        seed: o.get("seed")?.unwrap_or(1),
        stride: o.get("stride")?.unwrap_or(5),
        mutation: o.get("mutation")?.unwrap_or(1.0),
        structural: o.get("structural")?.unwrap_or(1.0),
        until,
        report_years: o.get("report-years")?.unwrap_or(5.0),
        plants: o.get("plants")?,
        time_lapse: o.get("time-lapse")?,
    })
}
