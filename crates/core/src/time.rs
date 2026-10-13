//! Clocks and the time model (design §12.3).
//!
//! - The **sim clock** is real seconds for organisms: physiology, movement and
//!   brains. It is stored as an integer count of [`QUANTUM_SECONDS`], so it never
//!   drifts and replays exactly.
//! - The **planet clock** drives astronomy, climate and soil:
//!   `τ = τ₀ + k·(t − t₀)`, where `k` is the season speed. It is anchored so the
//!   player can change `k` mid-run without the sky jumping.
//! - The **geologic clock** is planet years × a further acceleration.

use crate::consts::{EARTH_SIDEREAL_YEAR, EARTH_SOLAR_DAY};

/// Smallest organism step, in seconds. Sim time is a whole number of these.
pub const QUANTUM_SECONDS: f64 = 0.25;

/// A point on the sim clock, in quanta since world creation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimTime(pub u64);

impl SimTime {
    pub const ZERO: SimTime = SimTime(0);

    pub fn from_quanta(q: u64) -> Self {
        SimTime(q)
    }

    /// The first quantum boundary at or after `seconds`.
    pub fn from_seconds_ceil(seconds: f64) -> Self {
        assert!(
            seconds >= 0.0 && seconds.is_finite(),
            "bad sim time {seconds}"
        );
        SimTime((seconds / QUANTUM_SECONDS).ceil() as u64)
    }

    pub fn quanta(self) -> u64 {
        self.0
    }

    pub fn seconds(self) -> f64 {
        self.0 as f64 * QUANTUM_SECONDS
    }
}

/// An organism step size: `QUANTUM_SECONDS · 2^level`. Organisms on the same
/// level step together; every level's boundaries are also boundaries of all
/// finer levels, which is what lets fine and coarse steppers synchronise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StepLevel(u8);

impl StepLevel {
    pub const MAX: u8 = 16;

    pub fn new(level: u8) -> Self {
        assert!(level <= Self::MAX, "step level {level} > {}", Self::MAX);
        StepLevel(level)
    }

    pub fn level(self) -> u8 {
        self.0
    }

    pub fn quanta(self) -> u64 {
        1 << self.0
    }

    pub fn seconds(self) -> f64 {
        self.quanta() as f64 * QUANTUM_SECONDS
    }

    /// The coarsest level in `[finest, coarsest]` whose step is ≤ `dt` seconds.
    pub fn for_max_seconds(dt: f64, finest: StepLevel, coarsest: StepLevel) -> Self {
        let mut level = finest;
        while level < coarsest && StepLevel(level.0 + 1).seconds() <= dt {
            level = StepLevel(level.0 + 1);
        }
        level
    }

    /// Whether an organism on this level takes a step at time `t`.
    pub fn is_boundary(self, t: SimTime) -> bool {
        t.0 % self.quanta() == 0
    }
}

/// Player-facing time settings.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeModel {
    /// Season speed k: planet seconds that pass per sim second. 1 = realistic.
    pub season_speed: f64,
    /// Geological years that pass per planet year.
    pub geologic_acceleration: f64,
    /// Finest organism step (default 0.25 s).
    pub finest_step: StepLevel,
    /// Coarsest organism step (default 16 s).
    pub coarsest_step: StepLevel,
}

impl Default for TimeModel {
    fn default() -> Self {
        Self {
            season_speed: 72.0,
            geologic_acceleration: 1_000.0,
            finest_step: StepLevel::new(0),
            coarsest_step: StepLevel::new(6),
        }
    }
}

impl TimeModel {
    pub fn validate(&self) -> Result<(), String> {
        if !(self.season_speed.is_finite() && self.season_speed > 0.0) {
            return Err(format!(
                "season_speed must be > 0, got {}",
                self.season_speed
            ));
        }
        if !(self.geologic_acceleration.is_finite() && self.geologic_acceleration > 0.0) {
            return Err(format!(
                "geologic_acceleration must be > 0, got {}",
                self.geologic_acceleration
            ));
        }
        if self.finest_step > self.coarsest_step {
            return Err("finest_step is coarser than coarsest_step".into());
        }
        Ok(())
    }
}

/// Length of the planet's day and year, in planet seconds. Phase 1 derives
/// these from the orbit and rotation; until then they default to Earth.
#[derive(Clone, Debug, PartialEq)]
pub struct Calendar {
    pub day_length: f64,
    pub year_length: f64,
}

impl Default for Calendar {
    fn default() -> Self {
        Self {
            day_length: EARTH_SOLAR_DAY,
            year_length: EARTH_SIDEREAL_YEAR,
        }
    }
}

impl Calendar {
    /// Fraction of the current day elapsed, in [0, 1).
    pub fn day_fraction(&self, planet_seconds: f64) -> f64 {
        (planet_seconds / self.day_length).rem_euclid(1.0)
    }

    /// Fraction of the current year elapsed, in [0, 1).
    pub fn year_fraction(&self, planet_seconds: f64) -> f64 {
        (planet_seconds / self.year_length).rem_euclid(1.0)
    }

    pub fn years(&self, planet_seconds: f64) -> f64 {
        planet_seconds / self.year_length
    }
}

/// Maps sim time to planet time with a changeable season speed.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanetClock {
    anchor_sim: SimTime,
    anchor_planet: f64,
    season_speed: f64,
}

impl PlanetClock {
    pub fn new(season_speed: f64) -> Self {
        assert!(season_speed > 0.0, "season speed must be > 0");
        Self {
            anchor_sim: SimTime::ZERO,
            anchor_planet: 0.0,
            season_speed,
        }
    }

    pub fn season_speed(&self) -> f64 {
        self.season_speed
    }

    /// Planet seconds elapsed at sim time `t` (which must not precede the
    /// last speed change).
    pub fn planet_seconds_at(&self, t: SimTime) -> f64 {
        debug_assert!(t >= self.anchor_sim);
        let dt = (t.0 - self.anchor_sim.0) as f64 * QUANTUM_SECONDS;
        self.anchor_planet + self.season_speed * dt
    }

    /// The first sim time at which the planet clock has reached `planet_seconds`.
    pub fn sim_time_reaching(&self, planet_seconds: f64) -> SimTime {
        let ahead = (planet_seconds - self.anchor_planet).max(0.0);
        let mut q = self.anchor_sim.0 + (ahead / self.season_speed / QUANTUM_SECONDS).ceil() as u64;
        // Guard against floating-point rounding landing one quantum short.
        while self.planet_seconds_at(SimTime(q)) < planet_seconds {
            q += 1;
        }
        SimTime(q)
    }

    /// Changes the season speed from `now` on, keeping planet time continuous.
    pub fn set_season_speed(&mut self, now: SimTime, season_speed: f64) {
        assert!(season_speed > 0.0, "season speed must be > 0");
        self.anchor_planet = self.planet_seconds_at(now);
        self.anchor_sim = now;
        self.season_speed = season_speed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_gives_twenty_minute_days() {
        let m = TimeModel::default();
        m.validate().unwrap();
        let cal = Calendar::default();
        let clock = PlanetClock::new(m.season_speed);
        let day_end = clock.sim_time_reaching(cal.day_length);
        assert_eq!(day_end.seconds(), 1200.0);
        // An Earth year lasts about 5.07 sim-days.
        let year_sim_days = clock.sim_time_reaching(cal.year_length).seconds() / 86_400.0;
        assert!((year_sim_days - 5.073).abs() < 0.001, "{year_sim_days}");
    }

    #[test]
    fn changing_season_speed_keeps_planet_time_continuous() {
        let mut clock = PlanetClock::new(72.0);
        let now = SimTime::from_seconds_ceil(100.0);
        let before = clock.planet_seconds_at(now);
        clock.set_season_speed(now, 1.0);
        assert_eq!(clock.planet_seconds_at(now), before);
        let later = SimTime::from_seconds_ceil(110.0);
        assert!((clock.planet_seconds_at(later) - (before + 10.0)).abs() < 1e-9);
    }

    #[test]
    fn sim_time_reaching_is_the_first_quantum_at_or_after() {
        let clock = PlanetClock::new(7.3);
        for target in [0.0, 1.0, 3600.0, 12_345.678] {
            let t = clock.sim_time_reaching(target);
            assert!(clock.planet_seconds_at(t) >= target);
            if t.0 > 0 {
                assert!(clock.planet_seconds_at(SimTime(t.0 - 1)) < target);
            }
        }
    }

    #[test]
    fn step_levels_nest() {
        let fine = StepLevel::new(0);
        let coarse = StepLevel::new(6);
        assert_eq!(coarse.seconds(), 16.0);
        assert_eq!(StepLevel::for_max_seconds(3.0, fine, coarse).seconds(), 2.0);
        assert_eq!(StepLevel::for_max_seconds(0.1, fine, coarse), fine);
        assert_eq!(StepLevel::for_max_seconds(1e9, fine, coarse), coarse);
        // Every coarse boundary is a fine boundary.
        for q in 0..256 {
            let t = SimTime(q);
            if StepLevel::new(4).is_boundary(t) {
                assert!(StepLevel::new(2).is_boundary(t));
            }
        }
    }
}
