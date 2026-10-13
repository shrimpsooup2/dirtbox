//! Every strain alive anywhere on the planet, stored once and referred to by
//! a slot index from the per-cell strain fields.

use dirtbox_core::lineage::{Lineage, Lineages};

use crate::genome::Genome;

pub use dirtbox_core::lineage::{EMPTY, LineageId as StrainId};

pub type StrainRecord = Lineage<Genome>;
pub type Registry = Lineages<Genome>;
