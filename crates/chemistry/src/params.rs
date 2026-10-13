//! Tunable numbers for the microbial era. Physiology constants that follow
//! from chemistry (yields, stoichiometry) live next to the code that uses
//! them in `metabolism.rs`; the knobs here shape pace and spread.

/// Rates at which new strains arise (design §6.1, §11.2).
///
/// Microbial evolution runs on the geologic clock: these are chances per
/// occupied strain-cell per geologic year, before the player's multipliers.
/// That is how the microbial era is compressed (decision log #11).
#[derive(Clone, Debug, PartialEq)]
pub struct MutationRates {
    /// Small changes to existing genes.
    pub point: f64,
    /// Gaining or losing a metabolic module.
    pub structural: f64,
    /// A phagocyte keeping a module from the prey it ate.
    pub endosymbiosis: f64,
}

impl Default for MutationRates {
    fn default() -> Self {
        Self {
            point: 2.0e-3,
            structural: 5.0e-5,
            endosymbiosis: 2.0e-3,
        }
    }
}

impl MutationRates {
    /// The default rates scaled by the player's multipliers.
    pub fn scaled(&self, point: f64, structural: f64) -> Self {
        Self {
            point: self.point * point,
            structural: self.structural * structural,
            endosymbiosis: self.endosymbiosis * structural,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LifeParams {
    /// Depth of the sunlit, well-mixed surface ocean that the pools describe, m.
    pub mixed_layer: f64,
    /// Horizontal mixing of plankton and dissolved pools in the ocean, m² s⁻¹.
    pub ocean_dispersal: f64,
    /// Spread of microbes across wet land (wind, water, animals later), m² s⁻¹.
    pub land_dispersal: f64,
    /// How many of the strongest vents get base bacteria when life is seeded.
    pub seed_sites: usize,
    /// Biomass of each seeded population, mol C m⁻².
    pub seed_biomass: f64,
    /// Overturning time of the deep ocean, which trades nutrients with the
    /// surface, years.
    pub overturn_years: f64,
}

impl Default for LifeParams {
    fn default() -> Self {
        Self {
            mixed_layer: 50.0,
            ocean_dispersal: 5.0e4,
            land_dispersal: 5.0e3,
            seed_sites: 12,
            seed_biomass: 1.0e-3,
            overturn_years: 300.0,
        }
    }
}
