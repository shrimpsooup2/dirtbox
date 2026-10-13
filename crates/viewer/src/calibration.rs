//! Runs the hardware benchmark on a background thread so the UI stays live.

use eframe::egui;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use dirtbox_hw::{BenchConfig, BenchResult, HardwareInfo, run_benchmark};

pub struct CalibrationResult {
    pub single: BenchResult,
    pub all: BenchResult,
}

#[derive(Default)]
pub enum Calibration {
    #[default]
    NotRun,
    Running(Receiver<CalibrationResult>),
    Done(CalibrationResult),
}

impl Calibration {
    /// Seconds of measurement per pass (one single-thread pass, one all-core pass).
    const SECONDS_PER_PASS: f64 = 2.0;

    pub fn start(hw: &HardwareInfo, repaint: egui::Context) -> Self {
        let (tx, rx) = channel();
        let threads = hw.logical_cores;
        std::thread::spawn(move || {
            let measure = Duration::from_secs_f64(Self::SECONDS_PER_PASS);
            let single = run_benchmark(&BenchConfig {
                threads: 1,
                measure,
                ..Default::default()
            });
            let all = if threads > 1 {
                run_benchmark(&BenchConfig {
                    threads,
                    measure,
                    ..Default::default()
                })
            } else {
                single.clone()
            };
            let _ = tx.send(CalibrationResult { single, all });
            repaint.request_repaint();
        });
        Calibration::Running(rx)
    }

    /// Picks up a finished result, if any.
    pub fn poll(&mut self) {
        if let Calibration::Running(rx) = self {
            match rx.try_recv() {
                Ok(result) => *self = Calibration::Done(result),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => *self = Calibration::NotRun,
            }
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Calibration::Running(_))
    }
}
