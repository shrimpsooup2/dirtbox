//! Player-facing settings that are not part of the physical world model.

/// How a mutation boost interacts with lineages' own DNA repair genes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MutationBoost {
    /// The boost acts like a mutagenic environment (radiation, chemicals): it
    /// scales the rate *before* DNA repair. Lineages can evolve more repair to
    /// push back, paying its energy cost, just as they would in nature.
    #[default]
    Environmental,
    /// The boost applies *after* repair, so it cannot be evolved away. Use this
    /// when you want evolution sped up no matter what.
    Fixed,
}

/// Player controls over the pace of evolution (design §11.2).
///
/// These multiply the mutation rates that lineages evolve for themselves.
/// 1.0 is natural, values above 1 speed evolution up, and 0 freezes genomes.
#[derive(Clone, Debug, PartialEq)]
pub struct EvolutionSettings {
    /// Multiplies point mutations: small changes to existing genes (part
    /// sizes, weights, enzyme efficiencies, trait optima).
    pub mutation_multiplier: f64,
    /// Multiplies structural mutations: adding, removing or duplicating body
    /// parts, neurons, synapses and metabolic modules. These drive new kinds of
    /// complexity, so they get their own knob.
    pub structural_multiplier: f64,
    pub boost: MutationBoost,
}

impl EvolutionSettings {
    pub const MAX_MULTIPLIER: f64 = 100.0;

    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [
            ("mutation_multiplier", self.mutation_multiplier),
            ("structural_multiplier", self.structural_multiplier),
        ] {
            if !(0.0..=Self::MAX_MULTIPLIER).contains(&v) {
                return Err(format!(
                    "{name} must be between 0 and {}, got {v}",
                    Self::MAX_MULTIPLIER
                ));
            }
        }
        Ok(())
    }
}

impl Default for EvolutionSettings {
    fn default() -> Self {
        Self {
            mutation_multiplier: 1.0,
            structural_multiplier: 1.0,
            boost: MutationBoost::Environmental,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_bounds() {
        assert!(EvolutionSettings::default().validate().is_ok());
        let frozen = EvolutionSettings {
            mutation_multiplier: 0.0,
            structural_multiplier: 0.0,
            ..Default::default()
        };
        assert!(frozen.validate().is_ok());
        let too_high = EvolutionSettings {
            structural_multiplier: 1e3,
            ..Default::default()
        };
        assert!(too_high.validate().is_err());
        let nan = EvolutionSettings {
            mutation_multiplier: f64::NAN,
            ..Default::default()
        };
        assert!(nan.validate().is_err());
    }
}
