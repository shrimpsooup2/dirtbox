//! The world: shared state plus the systems that advance it.

use std::time::{Duration, Instant};

use dirtbox_climate::{Climate, ClimateParams, StepInputs, StepMode};
use dirtbox_core::{Calendar, PlanetClock, Rng, SimTime, TimeModel};
use dirtbox_grid::CellId;
use dirtbox_grid::GeodesicGrid;
use dirtbox_planet::{Brush, PlanetParams, Preset, Terrain};

use crate::hash::StateHasher;
use crate::schedule::{Cadence, Clock, Slot, StepContext, System};
use crate::settings::{EvolutionSettings, MutationBoost};

#[derive(Clone, Debug)]
pub struct WorldConfig {
    pub seed: u64,
    pub time: TimeModel,
    pub planet: PlanetParams,
    /// Start with frozen oceans and snow-covered land.
    pub start_frozen: bool,
    pub climate: ClimateParams,
    /// Whether to register the climate system (tests of the scheduler alone
    /// switch it off).
    pub run_climate: bool,
    /// Geodesic subdivision for environment fields (10n² + 2 cells).
    pub grid_subdivision: u32,
    pub evolution: EvolutionSettings,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self::from_preset(Preset::EarthModern)
    }
}

impl WorldConfig {
    pub fn from_preset(preset: Preset) -> Self {
        Self {
            seed: 0,
            time: TimeModel::default(),
            planet: preset.params(),
            start_frozen: preset.starts_frozen(),
            climate: ClimateParams::default(),
            run_climate: true,
            grid_subdivision: 32, // 10,242 cells, ~240 km apart on Earth
            evolution: EvolutionSettings::default(),
        }
    }
}

/// Everything systems read and write. Organisms, fields and pools are added
/// here by later phases.
pub struct WorldState {
    seed: u64,
    now: SimTime,
    time_model: TimeModel,
    calendar: Calendar,
    planet_clock: PlanetClock,
    evolution: EvolutionSettings,
    planet: PlanetParams,
    climate_mode: StepMode,
    pub grid: GeodesicGrid,
    pub terrain: Terrain,
    pub climate: Climate,
}

impl WorldState {
    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn now(&self) -> SimTime {
        self.now
    }

    pub fn time_model(&self) -> &TimeModel {
        &self.time_model
    }

    /// Day and year lengths, derived from the planet's orbit and rotation.
    pub fn calendar(&self) -> &Calendar {
        &self.calendar
    }

    pub fn evolution(&self) -> &EvolutionSettings {
        &self.evolution
    }

    pub fn planet(&self) -> &PlanetParams {
        &self.planet
    }

    pub fn climate_mode(&self) -> StepMode {
        self.climate_mode
    }

    pub fn planet_seconds(&self) -> f64 {
        self.planet_clock.planet_seconds_at(self.now)
    }

    pub fn geologic_years(&self) -> f64 {
        self.calendar.years(self.planet_seconds()) * self.time_model.geologic_acceleration
    }

    /// A random stream for one entity in one domain at the current tick.
    pub fn rng(&self, domain: u64, entity: u64) -> Rng {
        Rng::stream(self.seed, domain, entity, self.now.quanta())
    }

    pub fn hash_into(&self, h: &mut StateHasher) {
        h.write_u64(self.seed);
        h.write_u64(self.now.quanta());
        h.write_f64(self.planet_seconds());
        h.write_f64(self.planet_clock.season_speed());
        h.write_u64(self.grid.cell_count() as u64);
        h.write_f64(self.evolution.mutation_multiplier);
        h.write_f64(self.evolution.structural_multiplier);
        h.write_u64(match self.evolution.boost {
            MutationBoost::Environmental => 0,
            MutationBoost::Fixed => 1,
        });
        h.write_f64(self.planet.atmosphere.co2);
        for i in 0..self.grid.cell_count() {
            h.write_f64(self.terrain.elevation[i]);
            h.write_f64(self.climate.temperature[i]);
            h.write_f64(self.climate.vapour[i]);
            h.write_f64(self.climate.sea_ice[i]);
            h.write_f64(self.climate.snow[i]);
        }
    }

    /// Advances the climate over `[start, start + dt)` of planet time.
    pub fn step_climate(&mut self, start: f64, dt: f64) {
        let geologic_rate = self.time_model.geologic_acceleration / self.calendar.year_length;
        let report = self.climate.step(&StepInputs {
            grid: &self.grid,
            planet: &self.planet,
            terrain: &self.terrain,
            start,
            dt,
            mode: self.climate_mode,
            geologic_rate,
        });
        self.planet.atmosphere.co2 = (self.planet.atmosphere.co2 + report.co2_change).max(0.0);
    }

    /// First sim time strictly after `now` that lands on a multiple of the
    /// cadence's period.
    fn next_due(&self, cadence: Cadence, now: SimTime) -> SimTime {
        let period = cadence.period;
        let due = match cadence.clock {
            Clock::Sim => {
                let k = (now.seconds() / period).floor() + 1.0;
                SimTime::from_seconds_ceil(k * period)
            }
            Clock::Planet => {
                let k = (self.planet_clock.planet_seconds_at(now) / period).floor() + 1.0;
                self.planet_clock.sim_time_reaching(k * period)
            }
            Clock::Geologic => {
                let accel = self.time_model.geologic_acceleration;
                let years = self
                    .calendar
                    .years(self.planet_clock.planet_seconds_at(now))
                    * accel;
                let k = (years / period).floor() + 1.0;
                let planet_seconds = k * period / accel * self.calendar.year_length;
                self.planet_clock.sim_time_reaching(planet_seconds)
            }
        };
        due.max(SimTime(now.quanta() + 1))
    }
}

/// Runs the climate on the planet clock: hourly steps with day and night, or
/// day-long steps with daily-mean sunlight for fast running.
pub struct ClimateSystem;

impl System for ClimateSystem {
    fn name(&self) -> &'static str {
        "climate"
    }

    fn cadence(&self, state: &WorldState) -> Cadence {
        Cadence::planet_seconds(match state.climate_mode {
            StepMode::Diurnal => 3_600.0,
            StepMode::DailyMean => state.calendar.day_length.min(86_400.0),
        })
    }

    fn run(&mut self, state: &mut WorldState, ctx: &StepContext) {
        let dt = ctx.elapsed_planet_seconds;
        if dt > 0.0 {
            state.step_climate(ctx.planet_seconds - dt, dt);
        }
    }
}

pub struct World {
    state: WorldState,
    slots: Vec<Slot>,
}

impl World {
    pub fn new(config: WorldConfig) -> Result<Self, String> {
        Self::build(config, 0.0, &mut |_| {})
    }

    /// Builds a world and settles its climate for `spin_up_years` (on a
    /// coarse grid, then mapped to the full one). `progress` reports 0–1.
    pub fn with_spin_up(
        config: WorldConfig,
        spin_up_years: f64,
        progress: &mut dyn FnMut(f64),
    ) -> Result<Self, String> {
        Self::build(config, spin_up_years, progress)
    }

    fn build(
        config: WorldConfig,
        spin_up_years: f64,
        progress: &mut dyn FnMut(f64),
    ) -> Result<Self, String> {
        config.time.validate()?;
        config.evolution.validate()?;
        config.planet.validate()?;
        if !(1..=512).contains(&config.grid_subdivision) {
            return Err("grid subdivision must be 1–512".into());
        }
        let grid = GeodesicGrid::new(config.grid_subdivision);
        let terrain = Terrain::generate(&grid, &config.planet, config.seed);
        let years = spin_up_years.round();
        let climate = dirtbox_climate::spin_up(
            &grid,
            &config.planet,
            &terrain,
            config.climate.clone(),
            config.start_frozen,
            years,
            progress,
        );
        let calendar = config.planet.calendar();
        let state = WorldState {
            seed: config.seed,
            now: SimTime::ZERO,
            planet_clock: PlanetClock::new(config.time.season_speed),
            time_model: config.time,
            calendar,
            evolution: config.evolution,
            planet: config.planet,
            climate_mode: StepMode::Diurnal,
            grid,
            terrain,
            climate,
        };
        let mut world = Self {
            state,
            slots: Vec::new(),
        };
        if config.run_climate {
            world.add_system(Box::new(ClimateSystem));
        }
        Ok(world)
    }

    pub fn state(&self) -> &WorldState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut WorldState {
        &mut self.state
    }

    pub fn add_system(&mut self, system: Box<dyn System>) {
        let cadence = system.cadence(&self.state);
        assert!(
            cadence.period.is_finite() && cadence.period > 0.0,
            "{}: period must be > 0",
            system.name()
        );
        let now = self.state.now;
        self.slots.push(Slot {
            next_due: self.state.next_due(cadence, now),
            last_run: now,
            last_run_planet: self.state.planet_seconds(),
            system,
        });
    }

    pub fn system_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.slots.iter().map(|s| s.system.name())
    }

    /// Recomputes when each planet- and geologic-clock system is next due
    /// (after the clock or calendar changed).
    fn reschedule(&mut self) {
        let now = self.state.now;
        for slot in &mut self.slots {
            let cadence = slot.system.cadence(&self.state);
            if cadence.clock != Clock::Sim {
                slot.next_due = self.state.next_due(cadence, now);
            }
        }
    }

    /// Changes the season speed from now on. Planet time stays continuous.
    pub fn set_season_speed(&mut self, season_speed: f64) -> Result<(), String> {
        if !(season_speed.is_finite() && season_speed > 0.0) {
            return Err(format!("season speed must be > 0, got {season_speed}"));
        }
        let now = self.state.now;
        self.state.planet_clock.set_season_speed(now, season_speed);
        self.state.time_model.season_speed = season_speed;
        self.reschedule();
        Ok(())
    }

    /// Switches between hourly (day and night) and daily-mean climate steps.
    pub fn set_climate_mode(&mut self, mode: StepMode) {
        if self.state.climate_mode != mode {
            self.state.climate_mode = mode;
            self.reschedule();
        }
    }

    /// Replaces the planet's parameters while the world runs (orbit, star,
    /// atmosphere, rotation…). Planet time carries on; the calendar follows
    /// the new orbit. Changing the radius rescales the climate's geometry.
    pub fn set_planet(&mut self, planet: PlanetParams) -> Result<(), String> {
        planet.validate()?;
        if planet.body.radius() != self.state.planet.body.radius() {
            self.state
                .climate
                .set_radius(&self.state.grid, planet.body.radius());
        }
        self.state.calendar = planet.calendar();
        self.state.planet = planet;
        self.reschedule();
        Ok(())
    }

    /// Terraforms with a brush. Returns the cells that changed.
    pub fn apply_brush(&mut self, brush: &Brush) -> Vec<CellId> {
        let before = self.state.terrain.clone();
        let changed = self.state.terrain.apply_brush(&self.state.grid, brush);
        if !changed.is_empty() {
            self.state.climate.terrain_changed(
                &self.state.grid,
                &self.state.planet,
                &before,
                &self.state.terrain,
            );
        }
        changed
    }

    /// Moves the sea level, flooding or draining the planet.
    pub fn set_sea_level(&mut self, level: f64) {
        let before = self.state.terrain.clone();
        self.state.terrain.set_sea_level(&self.state.grid, level);
        self.state.climate.terrain_changed(
            &self.state.grid,
            &self.state.planet,
            &before,
            &self.state.terrain,
        );
    }

    /// Changes the player's evolution-speed settings; takes effect immediately.
    pub fn set_evolution(&mut self, evolution: EvolutionSettings) -> Result<(), String> {
        evolution.validate()?;
        self.state.evolution = evolution;
        Ok(())
    }

    /// When the next system is due, if any.
    pub fn next_event(&self) -> Option<SimTime> {
        self.slots.iter().map(|s| s.next_due).min()
    }

    /// Runs every system due up to and including `end`, then leaves the clock
    /// at `end`. Returns the number of system runs.
    pub fn run_until(&mut self, end: SimTime) -> u64 {
        self.run_until_within(end, None).0
    }

    /// Like [`run_until`](Self::run_until), but stops early once `budget` of
    /// wall time has been spent. Returns the number of system runs and the
    /// sim time actually reached (≤ `end`).
    pub fn run_until_within(&mut self, end: SimTime, budget: Option<Duration>) -> (u64, SimTime) {
        let started = Instant::now();
        let mut runs = 0;
        while let Some(t) = self.next_event().filter(|&t| t <= end) {
            if budget.is_some_and(|b| started.elapsed() >= b) {
                return (runs, self.state.now);
            }
            self.state.now = t;
            let planet_seconds = self.state.planet_seconds();
            let geologic_years = self.state.geologic_years();
            for slot in &mut self.slots {
                if slot.next_due != t {
                    continue;
                }
                let ctx = StepContext {
                    now: t,
                    sim_seconds: t.seconds(),
                    planet_seconds,
                    geologic_years,
                    elapsed_sim_seconds: t.seconds() - slot.last_run.seconds(),
                    elapsed_planet_seconds: planet_seconds - slot.last_run_planet,
                };
                slot.system.run(&mut self.state, &ctx);
                slot.last_run = t;
                slot.last_run_planet = planet_seconds;
                slot.next_due = self.state.next_due(slot.system.cadence(&self.state), t);
                runs += 1;
            }
        }
        self.state.now = self.state.now.max(end);
        (runs, self.state.now)
    }

    pub fn state_hash(&self) -> u64 {
        let mut h = StateHasher::default();
        self.state.hash_into(&mut h);
        for slot in &self.slots {
            h.write_bytes(slot.system.name().as_bytes());
            h.write_u64(slot.next_due.quanta());
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<(&'static str, u64, f64)>>>;

    struct Probe {
        name: &'static str,
        cadence: Cadence,
        log: Log,
    }

    impl System for Probe {
        fn name(&self) -> &'static str {
            self.name
        }
        fn cadence(&self, _: &WorldState) -> Cadence {
            self.cadence
        }
        fn run(&mut self, state: &mut WorldState, ctx: &StepContext) {
            let draw = state.rng(1, 0).f64();
            self.log
                .lock()
                .unwrap()
                .push((self.name, ctx.now.quanta(), draw));
        }
    }

    fn world_with(seed: u64, probes: &[(&'static str, Cadence)]) -> (World, Log) {
        let log: Log = Arc::default();
        let mut w = World::new(WorldConfig {
            seed,
            grid_subdivision: 2,
            run_climate: false,
            ..Default::default()
        })
        .unwrap();
        for &(name, cadence) in probes {
            w.add_system(Box::new(Probe {
                name,
                cadence,
                log: log.clone(),
            }));
        }
        (w, log)
    }

    fn one_sim_day() -> SimTime {
        SimTime::from_seconds_ceil(86_400.0)
    }

    #[test]
    fn planet_hour_system_runs_every_fifty_sim_seconds_at_default_speed() {
        let (mut w, log) = world_with(1, &[("hourly", Cadence::planet_seconds(3600.0))]);
        let runs = w.run_until(one_sim_day());
        assert_eq!(runs, 86_400 / 50);
        let log = log.lock().unwrap();
        assert_eq!(log[0].1, SimTime::from_seconds_ceil(50.0).quanta());
        assert_eq!(w.state().now(), one_sim_day());
    }

    #[test]
    fn simultaneous_systems_run_in_registration_order() {
        let (mut w, log) = world_with(
            1,
            &[
                ("b", Cadence::sim_seconds(10.0)),
                ("a", Cadence::sim_seconds(5.0)),
            ],
        );
        w.run_until(SimTime::from_seconds_ceil(10.0));
        let names: Vec<_> = log.lock().unwrap().iter().map(|e| e.0).collect();
        assert_eq!(names, ["a", "b", "a"]);
    }

    #[test]
    fn same_seed_same_run_different_seed_different_run() {
        let probes = [
            ("fast", Cadence::sim_seconds(0.25)),
            ("hourly", Cadence::planet_seconds(3600.0)),
            ("geo", Cadence::geologic_years(1.0)),
        ];
        let run = |seed| {
            let (mut w, log) = world_with(seed, &probes);
            w.run_until(SimTime::from_seconds_ceil(600.0));
            let log = log.lock().unwrap().clone();
            (w.state_hash(), log)
        };
        let (h1, l1) = run(7);
        let (h2, l2) = run(7);
        let (_, l3) = run(8);
        assert_eq!(h1, h2);
        assert_eq!(l1, l2);
        assert_ne!(l1, l3);
    }

    #[test]
    fn geologic_cadence_follows_acceleration() {
        let (mut w, _) = world_with(1, &[("geo", Cadence::geologic_years(1.0))]);
        let year_end = {
            let s = w.state();
            let year_sim_seconds = s.calendar().year_length / s.time_model().season_speed;
            SimTime::from_seconds_ceil(year_sim_seconds)
        };
        let runs = w.run_until(year_end);
        assert_eq!(runs, 1_000);
    }

    #[test]
    fn changing_season_speed_reschedules_planet_systems() {
        let (mut w, log) = world_with(1, &[("hourly", Cadence::planet_seconds(3600.0))]);
        w.run_until(SimTime::from_seconds_ceil(100.0)); // runs at 50 s and 100 s
        w.set_season_speed(1.0).unwrap(); // a planet hour is now an hour
        w.run_until(SimTime::from_seconds_ceil(100.0 + 3600.0));
        let times: Vec<f64> = log
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.1 as f64 * 0.25)
            .collect();
        assert_eq!(times, [50.0, 100.0, 3700.0]);
    }

    #[test]
    fn elapsed_time_is_reported_per_system() {
        struct Check;
        impl System for Check {
            fn name(&self) -> &'static str {
                "check"
            }
            fn cadence(&self, _: &WorldState) -> Cadence {
                Cadence::planet_seconds(3600.0)
            }
            fn run(&mut self, _: &mut WorldState, ctx: &StepContext) {
                assert!((ctx.elapsed_planet_seconds - 3600.0).abs() < 1e-6);
                assert!((ctx.elapsed_sim_seconds - 50.0).abs() < 1e-9);
            }
        }
        let mut w = World::new(WorldConfig {
            grid_subdivision: 1,
            run_climate: false,
            ..Default::default()
        })
        .unwrap();
        w.add_system(Box::new(Check));
        assert_eq!(w.run_until(SimTime::from_seconds_ceil(500.0)), 10);
    }

    #[test]
    fn climate_runs_on_the_planet_clock_and_is_deterministic() {
        let config = WorldConfig {
            grid_subdivision: 6,
            ..Default::default()
        };
        let run = || {
            let mut w = World::new(config.clone()).unwrap();
            // Two planet days at the default season speed (40 sim-minutes).
            let runs = w.run_until(SimTime::from_seconds_ceil(2_400.0));
            (runs, w.state_hash())
        };
        let (runs, h1) = run();
        assert_eq!(runs, 48, "hourly climate steps");
        assert_eq!(run().1, h1);
        let mut w = World::new(config).unwrap();
        w.set_climate_mode(StepMode::DailyMean);
        assert_eq!(w.run_until(SimTime::from_seconds_ceil(2_400.0)), 2);
    }

    #[test]
    fn budgeted_runs_stop_early_and_resume() {
        let mut w = World::new(WorldConfig {
            grid_subdivision: 8,
            ..Default::default()
        })
        .unwrap();
        let end = SimTime::from_seconds_ceil(86_400.0 * 30.0);
        let (_, reached) = w.run_until_within(end, Some(Duration::from_millis(1)));
        assert!(reached < end);
        assert!(w.state().now() == reached);
    }

    #[test]
    fn planet_edits_and_terraforming_apply_live() {
        let mut w = World::new(WorldConfig {
            grid_subdivision: 8,
            ..Default::default()
        })
        .unwrap();
        let mut planet = w.state().planet().clone();
        planet.orbit.semi_major_axis = 1.5;
        w.set_planet(planet).unwrap();
        assert!(w.state().calendar().year_length > 1.8 * 365.0 * 86_400.0);
        let mut bad = w.state().planet().clone();
        bad.orbit.eccentricity = 2.0;
        assert!(w.set_planet(bad).is_err());

        let before = w.state().terrain.ocean_fraction(&w.state().grid);
        w.set_sea_level(300.0);
        assert!(w.state().terrain.ocean_fraction(&w.state().grid) > before);
        let centre = w.state().grid.position(0);
        let changed = w.apply_brush(&Brush {
            mode: dirtbox_planet::BrushMode::Raise,
            centre,
            radius: 0.3,
            amount: 2_000.0,
        });
        assert!(!changed.is_empty());
        w.run_until(SimTime::from_seconds_ceil(1_200.0));
        assert!(w.state().climate.temperature.iter().all(|t| t.is_finite()));
    }
}
