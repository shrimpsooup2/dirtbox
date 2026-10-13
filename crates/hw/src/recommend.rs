//! Turns a measured throughput into a recommended population budget.
//!
//! ```text
//! N = throughput × mean_step × headroom / target_speed
//! ```
//! - `throughput`: organism steps per wall second.
//! - `mean_step`: average adaptive step length (sim seconds).
//! - `headroom`: share of CPU time left for organisms after climate, fields and
//!   the viewer.
//! - `target_speed`: sim seconds per wall second.
//!
//! The result is capped so the organisms fit in half of the available RAM.

/// A target simulation speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tier {
    pub name: &'static str,
    pub purpose: &'static str,
    /// Sim seconds per wall second.
    pub speed: f64,
}

pub const TIERS: [Tier; 3] = [
    Tier {
        name: "Watch",
        purpose: "follow individual creatures in close to real time",
        speed: 10.0,
    },
    Tier {
        name: "Balanced",
        purpose: "watch ecosystems change season to season",
        speed: 100.0,
    },
    Tier {
        name: "Evolve",
        purpose: "run many generations quickly",
        speed: 1_000.0,
    },
];

#[derive(Clone, Debug)]
pub struct BudgetInputs {
    /// Measured organism steps per wall second, all cores.
    pub throughput: f64,
    /// Average organism step length, sim seconds. With adaptive steps of
    /// 0.25–16 s most organisms are idle or travelling most of the time; 2 s is
    /// the design estimate until real populations can be measured.
    pub mean_step_seconds: f64,
    pub headroom: f64,
    /// Estimated memory per organism (genome, body, brain, pools).
    pub bytes_per_organism: u64,
    pub available_ram_bytes: Option<u64>,
    /// Planet year length in sim seconds (year length ÷ season speed).
    pub planet_year_sim_seconds: f64,
}

impl BudgetInputs {
    pub const DEFAULT_MEAN_STEP_SECONDS: f64 = 2.0;
    pub const DEFAULT_HEADROOM: f64 = 0.6;
    /// Design estimate; replaced by a measured value once organisms exist.
    pub const DEFAULT_BYTES_PER_ORGANISM: u64 = 16 * 1024;

    fn ram_cap(&self) -> Option<u64> {
        self.available_ram_bytes
            .map(|ram| ram / 2 / self.bytes_per_organism)
    }
}

#[derive(Clone, Debug)]
pub struct Recommendation {
    pub tier: Tier,
    pub population: u64,
    /// True when memory, not CPU, set the number.
    pub ram_limited: bool,
    /// Wall-clock seconds per planet year at this tier's speed.
    pub wall_seconds_per_planet_year: f64,
}

pub fn recommend(inputs: &BudgetInputs) -> Vec<Recommendation> {
    TIERS
        .iter()
        .map(|&tier| {
            let cpu = inputs.throughput * inputs.mean_step_seconds * inputs.headroom / tier.speed;
            let cpu = round_down_2sf(cpu as u64);
            let (population, ram_limited) = match inputs.ram_cap() {
                Some(cap) if cap < cpu => (round_down_2sf(cap), true),
                _ => (cpu, false),
            };
            Recommendation {
                tier,
                population,
                ram_limited,
                wall_seconds_per_planet_year: inputs.planet_year_sim_seconds / tier.speed,
            }
        })
        .collect()
}

/// Expected speed (sim seconds per wall second) for a chosen population, and
/// whether it would exceed the RAM budget.
pub fn speed_for_population(inputs: &BudgetInputs, population: u64) -> (f64, bool) {
    let speed =
        inputs.throughput * inputs.mean_step_seconds * inputs.headroom / population.max(1) as f64;
    let over_ram = inputs.ram_cap().is_some_and(|cap| population > cap);
    (speed, over_ram)
}

/// Rounds down to two significant figures (123_456 → 120_000).
fn round_down_2sf(v: u64) -> u64 {
    if v < 100 {
        return v;
    }
    let mut scale = 1;
    while v / scale >= 100 {
        scale *= 10;
    }
    v / scale * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(ram: Option<u64>) -> BudgetInputs {
        BudgetInputs {
            throughput: 10_000_000.0,
            mean_step_seconds: 2.0,
            headroom: 0.6,
            bytes_per_organism: 16 * 1024,
            available_ram_bytes: ram,
            planet_year_sim_seconds: 438_000.0,
        }
    }

    #[test]
    fn cpu_bound_recommendations_scale_with_speed() {
        let recs = recommend(&inputs(None));
        let pops: Vec<u64> = recs.iter().map(|r| r.population).collect();
        assert_eq!(pops, [1_200_000, 120_000, 12_000]);
        assert!(recs.iter().all(|r| !r.ram_limited));
        assert!((recs[1].wall_seconds_per_planet_year - 4_380.0).abs() < 1e-9);
    }

    #[test]
    fn ram_caps_the_population() {
        // 8 GiB available → 4 GiB budget → 262,144 organisms at 16 KiB.
        let recs = recommend(&inputs(Some(8 << 30)));
        assert_eq!(recs[0].population, 260_000);
        assert!(recs[0].ram_limited);
        assert_eq!(recs[1].population, 120_000);
        assert!(!recs[1].ram_limited);
    }

    #[test]
    fn custom_population_speed() {
        let (speed, over) = speed_for_population(&inputs(Some(8 << 30)), 60_000);
        assert!((speed - 200.0).abs() < 1e-9);
        assert!(!over);
        assert!(speed_for_population(&inputs(Some(8 << 30)), 1_000_000).1);
    }

    #[test]
    fn rounding() {
        assert_eq!(round_down_2sf(99), 99);
        assert_eq!(round_down_2sf(123_456), 120_000);
        assert_eq!(round_down_2sf(1_000), 1_000);
    }
}
