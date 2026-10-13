//! Microbial genomes: which metabolisms a strain carries, how good it is at
//! each, and the traits that shape where it can live (design §6.1).

use dirtbox_core::Rng;

/// A metabolic module: one real redox reaction the cell can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Module {
    /// Organics → CO₂ + H₂. Low yield; the base bacteria start here.
    Fermentation = 0,
    /// 4 H₂ + CO₂ → CH₄. Fixes carbon; makes methane, a greenhouse gas.
    Methanogenesis = 1,
    /// Oxidises vent reductants (sulfide, ferrous iron); fixes carbon. Much
    /// better with oxygen.
    Chemosynthesis = 2,
    /// Light + reductants → fixed carbon. No oxygen made.
    AnoxygenicPhotosynthesis = 3,
    /// Light + water → fixed carbon + O₂. Can change the whole planet.
    OxygenicPhotosynthesis = 4,
    /// Organics + O₂ → CO₂. About 14× the energy of fermentation.
    AerobicRespiration = 5,
    /// N₂ → fixed nitrogen. Energetically expensive; poisoned by O₂.
    NitrogenFixation = 6,
    /// Engulfs smaller cells: the first predators.
    Phagocytosis = 7,
}

pub const MODULE_COUNT: usize = 8;

impl Module {
    pub const ALL: [Module; MODULE_COUNT] = [
        Module::Fermentation,
        Module::Methanogenesis,
        Module::Chemosynthesis,
        Module::AnoxygenicPhotosynthesis,
        Module::OxygenicPhotosynthesis,
        Module::AerobicRespiration,
        Module::NitrogenFixation,
        Module::Phagocytosis,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Module::Fermentation => "Fermentation",
            Module::Methanogenesis => "Methanogenesis",
            Module::Chemosynthesis => "Chemosynthesis",
            Module::AnoxygenicPhotosynthesis => "Anoxygenic photosynthesis",
            Module::OxygenicPhotosynthesis => "Oxygenic photosynthesis",
            Module::AerobicRespiration => "Aerobic respiration",
            Module::NitrogenFixation => "Nitrogen fixation",
            Module::Phagocytosis => "Phagocytosis",
        }
    }

    /// Whether the module builds biomass from CO₂ (autotrophy).
    pub fn fixes_carbon(self) -> bool {
        matches!(
            self,
            Module::Methanogenesis
                | Module::Chemosynthesis
                | Module::AnoxygenicPhotosynthesis
                | Module::OxygenicPhotosynthesis
        )
    }

    /// Relative chance of a structural mutation producing this module, given
    /// the genome it lands in (0 = impossible). Oxygenic photosynthesis evolves
    /// only from an anoxygenic photosystem; engulfing needs a big cell.
    fn gain_weight(self, g: &Genome) -> f64 {
        match self {
            Module::OxygenicPhotosynthesis => {
                if g.has(Module::AnoxygenicPhotosynthesis) {
                    0.3
                } else {
                    0.0
                }
            }
            Module::Phagocytosis => {
                if g.size >= 2.5 {
                    1.0
                } else {
                    0.0
                }
            }
            _ => 1.0,
        }
    }
}

/// A strain's heritable makeup.
#[derive(Clone, Debug, PartialEq)]
pub struct Genome {
    /// Efficiency of each module, 0 when absent, up to 1.
    pub modules: [f32; MODULE_COUNT],
    /// Optimal temperature, K.
    pub temp_optimum: f32,
    /// Temperature tolerance (width of the comfort range), K.
    pub temp_tolerance: f32,
    /// Tolerance of oxygen, 0–1 (0: oxygen is poison).
    pub oxygen_tolerance: f32,
    /// Cell diameter, µm. Small cells grow fast; big cells can engulf others.
    pub size: f32,
    /// Swimming, 0–1: spreads faster and finds food better; costs energy.
    pub motility: f32,
    /// Spore-forming, 0–1: survives starvation, drought and freezing; costs energy.
    pub dormancy: f32,
    /// Stickiness, 0–1: cells clump into colonies that are hard to eat (and,
    /// one day, multicellular bodies). Costs energy.
    pub adhesion: f32,
    /// Toxin production, 0–1: kills competitors in the same place; costs energy.
    pub toxin: f32,
    /// Built from two cells by endosymbiosis.
    pub complex: bool,
}

impl Genome {
    /// The base bacterium: a warmth-loving fermenter, as early life near
    /// hydrothermal vents probably was.
    pub fn base_bacterium() -> Self {
        let mut modules = [0.0; MODULE_COUNT];
        modules[Module::Fermentation.index()] = 0.6;
        Genome {
            modules,
            temp_optimum: 320.0,
            temp_tolerance: 15.0,
            oxygen_tolerance: 0.05,
            size: 1.0,
            motility: 0.1,
            dormancy: 0.05,
            adhesion: 0.0,
            toxin: 0.0,
            complex: false,
        }
    }

    pub fn has(&self, m: Module) -> bool {
        self.modules[m.index()] > 0.0
    }

    pub fn efficiency(&self, m: Module) -> f64 {
        self.modules[m.index()] as f64
    }

    pub fn module_count(&self) -> usize {
        self.modules.iter().filter(|&&e| e > 0.0).count()
    }

    pub fn is_autotroph(&self) -> bool {
        Module::ALL.iter().any(|&m| m.fixes_carbon() && self.has(m))
    }

    /// Bitmask of modules present (bit = module index).
    pub fn module_mask(&self) -> u8 {
        Module::ALL
            .iter()
            .filter(|&&m| self.has(m))
            .fold(0, |acc, &m| acc | (1 << m.index()))
    }

    /// How different two genomes are (0 = identical). Missing or extra
    /// modules count most.
    pub fn distance(&self, o: &Genome) -> f64 {
        let modules: f64 = self
            .modules
            .iter()
            .zip(&o.modules)
            .map(|(&a, &b)| {
                if (a > 0.0) != (b > 0.0) {
                    1.0
                } else {
                    (a - b).abs() as f64
                }
            })
            .sum();
        modules
            + ((self.temp_optimum - o.temp_optimum).abs() / 20.0) as f64
            + ((self.size - o.size).abs() / 2.0) as f64
            + (self.oxygen_tolerance - o.oxygen_tolerance).abs() as f64
            + (self.motility - o.motility).abs() as f64
            + (self.dormancy - o.dormancy).abs() as f64
            + (self.adhesion - o.adhesion).abs() as f64
            + (self.toxin - o.toxin).abs() as f64
    }

    /// A copy with small random changes to existing genes.
    pub fn point_mutant(&self, rng: &mut Rng) -> Genome {
        let mut g = self.clone();
        // Nudge one or two genes at a time.
        let changes = 1 + rng.below(2);
        for _ in 0..changes {
            let n = |rng: &mut Rng, sd: f64| (rng.normal() * sd) as f32;
            match rng.below(9) {
                0 | 1 => {
                    // Improve or worsen one of the modules it has.
                    let present: Vec<usize> =
                        (0..MODULE_COUNT).filter(|&i| g.modules[i] > 0.0).collect();
                    if let Some(&i) = present.get(rng.below(present.len() as u64) as usize) {
                        g.modules[i] = (g.modules[i] + n(rng, 0.05)).clamp(0.01, 1.0);
                    }
                }
                2 => g.temp_optimum = (g.temp_optimum + n(rng, 3.0)).clamp(250.0, 400.0),
                3 => g.temp_tolerance = (g.temp_tolerance + n(rng, 1.0)).clamp(3.0, 30.0),
                4 => g.oxygen_tolerance = (g.oxygen_tolerance + n(rng, 0.05)).clamp(0.0, 1.0),
                5 => g.size = (g.size * (n(rng, 0.12) as f64).exp() as f32).clamp(0.3, 30.0),
                6 => g.motility = (g.motility + n(rng, 0.05)).clamp(0.0, 1.0),
                7 => g.dormancy = (g.dormancy + n(rng, 0.05)).clamp(0.0, 1.0),
                _ => {
                    if rng.chance(0.5) {
                        g.adhesion = (g.adhesion + n(rng, 0.05)).clamp(0.0, 1.0);
                    } else {
                        g.toxin = (g.toxin + n(rng, 0.05)).clamp(0.0, 1.0);
                    }
                }
            }
        }
        g
    }

    /// A copy that gains a new module (weakly at first) or loses one.
    pub fn structural_mutant(&self, rng: &mut Rng) -> Genome {
        let mut g = self.clone();
        let count = g.module_count();
        if count > 1 && rng.chance(0.35) {
            // Lose a module: streamlining saves upkeep.
            let present: Vec<usize> = (0..MODULE_COUNT).filter(|&i| g.modules[i] > 0.0).collect();
            let i = present[rng.below(present.len() as u64) as usize];
            g.modules[i] = 0.0;
            return g;
        }
        // Gain a module by duplicating and repurposing existing machinery.
        let weights: Vec<f64> = Module::ALL
            .iter()
            .map(|&m| if g.has(m) { 0.0 } else { m.gain_weight(&g) })
            .collect();
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            return g;
        }
        let mut pick = rng.f64() * total;
        for (i, w) in weights.iter().enumerate() {
            if pick < *w {
                g.modules[i] = rng.range_f64(0.05, 0.15) as f32;
                break;
            }
            pick -= w;
        }
        g
    }

    /// Endosymbiosis: this cell keeps a module from one it engulfed, and grows
    /// larger and more complex.
    pub fn with_symbiont(&self, prey: &Genome, rng: &mut Rng) -> Option<Genome> {
        let candidates: Vec<usize> = (0..MODULE_COUNT)
            .filter(|&i| prey.modules[i] > 0.0 && self.modules[i] < prey.modules[i])
            .collect();
        let &i = candidates.get(rng.below(candidates.len().max(1) as u64) as usize)?;
        let mut g = self.clone();
        g.modules[i] = prey.modules[i];
        g.size = (g.size * 1.5).min(30.0);
        g.complex = true;
        Some(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_bacterium_is_a_fermenter() {
        let g = Genome::base_bacterium();
        assert!(g.has(Module::Fermentation));
        assert_eq!(g.module_count(), 1);
        assert!(!g.is_autotroph());
        assert_eq!(g.module_mask(), 1);
    }

    #[test]
    fn oxygenic_photosynthesis_needs_an_anoxygenic_ancestor() {
        let mut rng = Rng::from_seed(1);
        let base = Genome::base_bacterium();
        for _ in 0..2000 {
            let m = base.structural_mutant(&mut rng);
            assert!(!m.has(Module::OxygenicPhotosynthesis));
            assert!(!m.has(Module::Phagocytosis), "small cells can't engulf");
        }
        let mut anox = base.clone();
        anox.modules[Module::AnoxygenicPhotosynthesis.index()] = 0.5;
        let gained = (0..4000)
            .filter(|_| {
                anox.structural_mutant(&mut rng)
                    .has(Module::OxygenicPhotosynthesis)
            })
            .count();
        assert!(gained > 0);
    }

    #[test]
    fn mutants_differ_but_stay_in_bounds() {
        let mut rng = Rng::from_seed(2);
        let mut g = Genome::base_bacterium();
        for _ in 0..5000 {
            g = if rng.chance(0.1) {
                g.structural_mutant(&mut rng)
            } else {
                g.point_mutant(&mut rng)
            };
            assert!(g.module_count() >= 1, "a strain always keeps one module");
            assert!(g.modules.iter().all(|&e| (0.0..=1.0).contains(&e)));
            assert!((250.0..=400.0).contains(&g.temp_optimum));
            assert!((0.3..=30.0).contains(&g.size));
        }
        assert!(Genome::base_bacterium().distance(&g) > 0.0);
        assert_eq!(g.distance(&g), 0.0);
    }

    #[test]
    fn endosymbiosis_combines_metabolisms() {
        let mut rng = Rng::from_seed(3);
        let mut host = Genome::base_bacterium();
        host.size = 4.0;
        host.modules[Module::Phagocytosis.index()] = 0.4;
        let mut prey = Genome::base_bacterium();
        prey.modules = [0.0; MODULE_COUNT];
        prey.modules[Module::OxygenicPhotosynthesis.index()] = 0.7;
        let merged = host.with_symbiont(&prey, &mut rng).unwrap();
        assert!(merged.complex);
        assert_eq!(
            merged.efficiency(Module::OxygenicPhotosynthesis),
            0.7f32 as f64
        );
        assert!(merged.size > host.size);
    }
}
