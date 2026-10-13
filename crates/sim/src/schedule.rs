//! Systems and their cadences.

use dirtbox_core::SimTime;

use crate::world::WorldState;

/// Which clock a cadence is measured on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    /// Real seconds for organisms.
    Sim,
    /// Planet seconds (season speed × sim seconds).
    Planet,
    /// Geologic years.
    Geologic,
}

/// How often a system runs: every `period` units of `clock` (seconds for Sim
/// and Planet, years for Geologic), at whole multiples of the period.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cadence {
    pub clock: Clock,
    pub period: f64,
}

impl Cadence {
    pub fn sim_seconds(period: f64) -> Self {
        Self {
            clock: Clock::Sim,
            period,
        }
    }

    pub fn planet_seconds(period: f64) -> Self {
        Self {
            clock: Clock::Planet,
            period,
        }
    }

    pub fn geologic_years(period: f64) -> Self {
        Self {
            clock: Clock::Geologic,
            period,
        }
    }
}

/// What a system sees when it runs.
#[derive(Clone, Copy, Debug)]
pub struct StepContext {
    pub now: SimTime,
    pub sim_seconds: f64,
    pub planet_seconds: f64,
    pub geologic_years: f64,
    /// Time since this system last ran (or since world start), on each clock.
    pub elapsed_sim_seconds: f64,
    pub elapsed_planet_seconds: f64,
}

pub trait System: Send {
    fn name(&self) -> &'static str;
    fn cadence(&self) -> Cadence;
    fn run(&mut self, state: &mut WorldState, ctx: &StepContext);
}

/// A registered system plus its bookkeeping.
pub(crate) struct Slot {
    pub system: Box<dyn System>,
    pub next_due: SimTime,
    pub last_run: SimTime,
    pub last_run_planet: f64,
}
