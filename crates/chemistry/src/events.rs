//! Milestones of the microbial era (design §6.4: run until milestone).

use crate::genome::Module;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Milestone {
    /// Base bacteria seeded.
    LifeBegins,
    /// A new metabolism becomes established: strains running it well hold at
    /// least 10⁹ mol C and 0.5% of all life.
    Metabolism(Module),
    /// Oxygen in the air reaches 0.1% of today's level.
    OxygenAppears,
    /// Oxygen reaches 10% of today's level.
    OxygenRich,
    /// Cells built by endosymbiosis become established.
    ComplexCells,
    /// Land holds ≥ 10¹³ mol of living carbon.
    LandColonised,
    /// Every living thing died.
    Extinction,
}

impl Milestone {
    /// Milestones the player can fast-forward to.
    pub fn targets() -> Vec<Milestone> {
        let mut v: Vec<Milestone> = Module::ALL
            .iter()
            .filter(|&&m| m != Module::Fermentation)
            .map(|&m| Milestone::Metabolism(m))
            .collect();
        v.extend([
            Milestone::OxygenAppears,
            Milestone::OxygenRich,
            Milestone::ComplexCells,
            Milestone::LandColonised,
        ]);
        v
    }

    pub fn label(&self) -> String {
        match self {
            Milestone::LifeBegins => "Life begins".into(),
            Milestone::Metabolism(m) => m.label().into(),
            Milestone::OxygenAppears => "Oxygen in the air".into(),
            Milestone::OxygenRich => "Oxygen-rich air".into(),
            Milestone::ComplexCells => "Complex cells".into(),
            Milestone::LandColonised => "Life on land".into(),
            Milestone::Extinction => "Extinction".into(),
        }
    }

    /// One line for the event log.
    pub fn describe(&self) -> String {
        match self {
            Milestone::LifeBegins => "Base bacteria appear at hydrothermal vents.".into(),
            Milestone::Metabolism(m) => match m {
                Module::Fermentation => "Fermenters spread.".into(),
                Module::Methanogenesis => "Methanogens appear: life starts making methane.".into(),
                Module::Chemosynthesis => "Chemosynthesis: cells live off vent chemicals.".into(),
                Module::AnoxygenicPhotosynthesis => {
                    "Photosynthesis (anoxygenic): life harvests sunlight.".into()
                }
                Module::OxygenicPhotosynthesis => {
                    "Oxygenic photosynthesis: cells split water and release oxygen.".into()
                }
                Module::AerobicRespiration => {
                    "Aerobic respiration: cells burn food with oxygen.".into()
                }
                Module::NitrogenFixation => {
                    "Nitrogen fixation: cells pull nitrogen from the air.".into()
                }
                Module::Phagocytosis => {
                    "The first predators: cells that engulf other cells.".into()
                }
            },
            Milestone::OxygenAppears => {
                "Oxygen builds up in the air (0.1% of today's Earth).".into()
            }
            Milestone::OxygenRich => "The air is oxygen-rich (10% of today's Earth).".into(),
            Milestone::ComplexCells => {
                "Complex cells: a predator kept its prey as a partner.".into()
            }
            Milestone::LandColonised => "Microbial crusts spread across the land.".into(),
            Milestone::Extinction => "All life has died out.".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LifeEvent {
    /// Planet time, s.
    pub time: f64,
    pub milestone: Milestone,
}
