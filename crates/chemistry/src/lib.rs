//! Chemistry and the microbial era (design §5–6): dissolved pools of organics,
//! hydrogen, reductants, oxygen and nutrients in every cell; strain fields of
//! microbes whose genomes carry real metabolic reactions; and the coupling of
//! both to the atmosphere.
//!
//! Life starts as base bacteria at hydrothermal vents. New metabolisms arise
//! by mutation and spread only where they pay their way, so the order of
//! innovations, and whether oxygen ever fills the air, emerges from the
//! planet the player built.

pub mod biosphere;
pub mod events;
pub mod genome;
pub mod metabolism;
pub mod params;
pub mod registry;

pub use biosphere::{AtmosphereChange, Biosphere, Environment, LifeStats, moles_per_bar};
pub use events::{LifeEvent, Milestone};
pub use genome::{Genome, MODULE_COUNT, Module};
pub use metabolism::SLOTS;
pub use params::{LifeParams, MutationRates};
pub use registry::{EMPTY, Registry, StrainId, StrainRecord};

/// Indices of the dissolved pools, for display.
pub mod pool {
    pub use crate::metabolism::{
        H2, NIT as NITROGEN, O2, OM as ORGANICS, PHOS as PHOSPHORUS, RED as REDUCTANTS,
    };
}
