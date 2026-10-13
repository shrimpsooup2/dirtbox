//! Multicellular life (design §6.2, §7). Phase 3 brings producers: plants,
//! mosses and seaweeds as cohorts in every cell, descended from colonies of
//! photosynthetic microbes. Motile bodies and brains follow in later phases.

pub mod plant;
pub mod vegetation;

pub use plant::{GrowthForm, PlantGenome};
pub use vegetation::{COHORTS, Cohort, SLA, Vegetation, VegetationParams, VegetationStats};
