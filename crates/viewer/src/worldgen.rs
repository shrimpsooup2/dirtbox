//! Builds new worlds on a background thread (terrain generation plus a few
//! years of climate spin-up), reporting progress to the UI.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;

use dirtbox_planet::Preset;
use dirtbox_sim::{World, WorldConfig};

/// What the player chose in the New World dialog.
#[derive(Clone, Debug)]
pub struct WorldRecipe {
    pub preset: Preset,
    pub seed: u64,
    pub subdivision: u32,
    /// Planet years of climate spin-up before the world appears.
    pub spin_up_years: u32,
}

impl Default for WorldRecipe {
    fn default() -> Self {
        Self {
            preset: Preset::EarthModern,
            seed: 1,
            subdivision: 32,
            spin_up_years: 6,
        }
    }
}

impl WorldRecipe {
    pub fn config(&self) -> WorldConfig {
        WorldConfig {
            seed: self.seed,
            grid_subdivision: self.subdivision,
            ..WorldConfig::from_preset(self.preset)
        }
    }
}

pub struct PendingWorld {
    pub recipe: WorldRecipe,
    progress: Arc<AtomicU64>,
    result: Receiver<Result<World, String>>,
}

impl PendingWorld {
    pub fn start(recipe: WorldRecipe, repaint: egui::Context) -> Self {
        let progress = Arc::new(AtomicU64::new(0f64.to_bits()));
        let (tx, rx) = channel();
        let config = recipe.config();
        let years = recipe.spin_up_years as f64;
        let p = progress.clone();
        std::thread::spawn(move || {
            let mut report = |f: f64| {
                p.store(f.to_bits(), Ordering::Relaxed);
                repaint.request_repaint();
            };
            let _ = tx.send(World::with_spin_up(config, years, &mut report));
            repaint.request_repaint();
        });
        Self {
            recipe,
            progress,
            result: rx,
        }
    }

    pub fn progress(&self) -> f64 {
        f64::from_bits(self.progress.load(Ordering::Relaxed))
    }

    /// The finished world, if ready.
    pub fn poll(&self) -> Option<Result<World, String>> {
        match self.result.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("world generation stopped unexpectedly".into()))
            }
        }
    }
}
