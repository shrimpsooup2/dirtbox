//! Foundation types shared by every Dirtbox crate: physical constants, the
//! sim/planet/geologic clocks, deterministic random streams and sphere math.
//!
//! All quantities are SI unless a name says otherwise.

pub mod consts;
pub mod math;
pub mod rng;
pub mod time;

pub use math::Vec3;
pub use rng::Rng;
pub use time::{Calendar, PlanetClock, SimTime, StepLevel, TimeModel};
