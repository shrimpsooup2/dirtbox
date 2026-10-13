//! World state and the multi-rate scheduler (design §12.3).
//!
//! The world advances as a discrete-event loop: each system declares how often
//! it runs (in sim, planet or geologic time), and the clock jumps straight to
//! the next moment something is due. Eras with nothing to do at fine
//! resolution therefore cost nothing at fine resolution. Systems due at the same
//! moment run in registration order, which keeps runs deterministic.

pub mod hash;
pub mod schedule;
pub mod world;

pub use hash::StateHasher;
pub use schedule::{Cadence, Clock, StepContext, System};
pub use world::{World, WorldConfig, WorldState};
