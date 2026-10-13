//! Producer genomes: how a multicellular photosynthesiser builds itself
//! (design §7.2, §7.6). Plants, mosses and seaweeds are all this one model
//! with different settings.

use dirtbox_chemistry::{Genome, Module};
use dirtbox_core::Rng;

#[derive(Clone, Debug, PartialEq)]
pub struct PlantGenome {
    /// Height the plant grows to, m. Crusts are millimetres; trees tens of metres.
    pub max_height: f32,
    /// How new growth is split between leaves, roots and wood (relative weights).
    pub leaf_share: f32,
    pub root_share: f32,
    pub wood_share: f32,
    /// Share of a mature plant's surplus spent on seeds (or spores), 0–1.
    pub seed_share: f32,
    /// Carbon per seed, mol C. Small seeds are many and travel far; big seeds
    /// survive in the shade.
    pub seed_mass: f32,
    /// Share of seeds that leave the cell (wind, water), 0–1.
    pub dispersal: f32,
    /// 0 = evergreen, 1 = drops its leaves in bad seasons.
    pub deciduous: f32,
    /// Optimal temperature, K, and the width of the comfortable range, K.
    pub temp_optimum: f32,
    pub temp_tolerance: f32,
    /// How far below freezing its tissues survive, K.
    pub frost_hardiness: f32,
    /// 0–1: keeps working in dry soil, but grows slower at best.
    pub drought_tolerance: f32,
    /// 0–1: hosts nitrogen-fixing partners in its roots (costs sugar).
    pub nitrogen_fixing: f32,
    /// 0 = aquatic, 1 = fully adapted to land (cuticle, roots, vessels).
    pub land: f32,
}

impl PlantGenome {
    /// The first multicellular producer, built from a colony-forming
    /// photosynthetic strain: a thin mat or filament in the water (or a crust
    /// on land), spreading by spores.
    pub fn from_strain(g: &Genome, on_land: bool) -> Self {
        PlantGenome {
            max_height: 0.02,
            leaf_share: 0.7,
            root_share: 0.2,
            wood_share: 0.1,
            seed_share: 0.3,
            seed_mass: 1e-7,
            dispersal: 0.5,
            deciduous: 0.0,
            temp_optimum: g.temp_optimum,
            temp_tolerance: g.temp_tolerance.max(8.0),
            frost_hardiness: 2.0,
            drought_tolerance: (0.3 * g.dormancy).min(1.0),
            nitrogen_fixing: (g.efficiency(Module::NitrogenFixation) as f32 * g.differentiation).min(1.0),
            land: if on_land { 0.3 } else { 0.0 },
        }
    }

    /// A moss-like ground cover (for seeding and tests).
    pub fn moss() -> Self {
        PlantGenome {
            max_height: 0.03,
            leaf_share: 0.7,
            root_share: 0.25,
            wood_share: 0.05,
            seed_share: 0.3,
            seed_mass: 1e-7,
            dispersal: 0.6,
            deciduous: 0.0,
            temp_optimum: 285.0,
            temp_tolerance: 12.0,
            frost_hardiness: 25.0,
            drought_tolerance: 0.6,
            // Cyanobacterial partners, as in lichens and moss crusts.
            nitrogen_fixing: 0.4,
            land: 0.9,
        }
    }

    /// A grass-like herb.
    pub fn grass() -> Self {
        PlantGenome {
            max_height: 0.5,
            leaf_share: 0.55,
            root_share: 0.4,
            wood_share: 0.05,
            seed_share: 0.25,
            seed_mass: 2e-4,
            dispersal: 0.3,
            deciduous: 0.8,
            temp_optimum: 295.0,
            temp_tolerance: 10.0,
            frost_hardiness: 15.0,
            drought_tolerance: 0.5,
            nitrogen_fixing: 0.0,
            land: 1.0,
        }
    }

    /// A broadleaf tree.
    pub fn tree() -> Self {
        PlantGenome {
            max_height: 25.0,
            leaf_share: 0.3,
            root_share: 0.25,
            wood_share: 0.45,
            seed_share: 0.1,
            seed_mass: 5e-3,
            dispersal: 0.15,
            deciduous: 0.2,
            temp_optimum: 297.0,
            temp_tolerance: 9.0,
            frost_hardiness: 10.0,
            drought_tolerance: 0.2,
            nitrogen_fixing: 0.0,
            land: 1.0,
        }
    }

    /// Allocation weights normalised to sum to 1: (leaf, root, wood).
    pub fn allocation(&self) -> (f64, f64, f64) {
        let (l, r, w) = (self.leaf_share as f64, self.root_share as f64, self.wood_share as f64);
        let t = (l + r + w).max(1e-9);
        (l / t, r / t, w / t)
    }

    /// How different two genomes are (0 = identical).
    pub fn distance(&self, o: &PlantGenome) -> f64 {
        let ln = |a: f32, b: f32| ((a.max(1e-9) / b.max(1e-9)) as f64).ln().abs();
        ln(self.max_height, o.max_height)
            + 0.3 * ln(self.seed_mass, o.seed_mass)
            + ((self.leaf_share - o.leaf_share).abs()
                + (self.root_share - o.root_share).abs()
                + (self.wood_share - o.wood_share).abs()
                + (self.seed_share - o.seed_share).abs()
                + (self.dispersal - o.dispersal).abs()
                + (self.deciduous - o.deciduous).abs()
                + (self.drought_tolerance - o.drought_tolerance).abs()
                + (self.nitrogen_fixing - o.nitrogen_fixing).abs()
                + 2.0 * (self.land - o.land).abs()) as f64
            + ((self.temp_optimum - o.temp_optimum).abs() / 10.0) as f64
            + ((self.temp_tolerance - o.temp_tolerance).abs() / 10.0) as f64
            + ((self.frost_hardiness - o.frost_hardiness).abs() / 10.0) as f64
    }

    /// A copy with one or two genes nudged.
    pub fn mutant(&self, rng: &mut Rng) -> PlantGenome {
        let mut g = self.clone();
        let changes = 1 + rng.below(2);
        for _ in 0..changes {
            let n = |rng: &mut Rng, sd: f64| (rng.normal() * sd) as f32;
            let unit = |v: f32, rng: &mut Rng| (v + n(rng, 0.06)).clamp(0.0, 1.0);
            let logn = |v: f32, rng: &mut Rng, sd: f64| (v as f64 * (rng.normal() * sd).exp()) as f32;
            match rng.below(13) {
                0 | 1 => g.max_height = logn(g.max_height, rng, 0.25).clamp(0.003, 120.0),
                2 => g.leaf_share = unit(g.leaf_share, rng).max(0.02),
                3 => g.root_share = unit(g.root_share, rng).max(0.02),
                4 => g.wood_share = unit(g.wood_share, rng),
                5 => g.seed_share = unit(g.seed_share, rng).clamp(0.01, 0.9),
                6 => g.seed_mass = logn(g.seed_mass, rng, 0.5).clamp(1e-8, 0.5),
                7 => g.dispersal = unit(g.dispersal, rng),
                8 => g.deciduous = unit(g.deciduous, rng),
                9 => {
                    if rng.chance(0.5) {
                        g.temp_optimum = (g.temp_optimum + n(rng, 2.0)).clamp(250.0, 330.0);
                    } else {
                        g.temp_tolerance = (g.temp_tolerance + n(rng, 1.0)).clamp(3.0, 25.0);
                    }
                }
                10 => g.frost_hardiness = (g.frost_hardiness + n(rng, 3.0)).clamp(0.0, 70.0),
                11 => {
                    if rng.chance(0.5) {
                        g.drought_tolerance = unit(g.drought_tolerance, rng);
                    } else {
                        g.nitrogen_fixing = unit(g.nitrogen_fixing, rng);
                    }
                }
                _ => g.land = unit(g.land, rng),
            }
        }
        g
    }

    /// Everyday name for the growth form.
    pub fn form(&self) -> GrowthForm {
        GrowthForm::of(self.max_height as f64, self.land as f64)
    }
}

/// Broad growth forms, by height and habitat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrowthForm {
    Seaweed,
    Crust,
    Herb,
    Shrub,
    Tree,
}

impl GrowthForm {
    pub const ALL: [GrowthForm; 5] = [
        GrowthForm::Seaweed,
        GrowthForm::Crust,
        GrowthForm::Herb,
        GrowthForm::Shrub,
        GrowthForm::Tree,
    ];

    pub fn of(height: f64, land: f64) -> Self {
        if land < 0.5 {
            GrowthForm::Seaweed
        } else if height < 0.05 {
            GrowthForm::Crust
        } else if height < 1.5 {
            GrowthForm::Herb
        } else if height < 6.0 {
            GrowthForm::Shrub
        } else {
            GrowthForm::Tree
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GrowthForm::Seaweed => "Seaweeds",
            GrowthForm::Crust => "Mosses and crusts",
            GrowthForm::Herb => "Herbs and grasses",
            GrowthForm::Shrub => "Shrubs",
            GrowthForm::Tree => "Trees",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutants_stay_in_bounds() {
        let mut rng = Rng::from_seed(9);
        let mut g = PlantGenome::grass();
        for _ in 0..5000 {
            g = g.mutant(&mut rng);
            assert!((0.003..=120.0).contains(&g.max_height));
            assert!((1e-8..=0.5).contains(&g.seed_mass));
            let (l, r, w) = g.allocation();
            assert!((l + r + w - 1.0).abs() < 1e-9 && l > 0.0);
            assert!((0.0..=1.0).contains(&g.land));
        }
        assert!(g.distance(&PlantGenome::grass()) > 0.0);
        assert_eq!(g.distance(&g), 0.0);
    }

    #[test]
    fn graduated_producers_start_small_and_aquatic() {
        let mut s = Genome::base_bacterium();
        s.modules[Module::OxygenicPhotosynthesis.index()] = 0.6;
        let p = PlantGenome::from_strain(&s, false);
        assert_eq!(p.form(), GrowthForm::Seaweed);
        assert!(p.max_height < 0.1);
        assert_eq!(PlantGenome::tree().form(), GrowthForm::Tree);
        assert_eq!(PlantGenome::moss().form(), GrowthForm::Crust);
    }
}
