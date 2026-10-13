//! Hardware detection, calibration benchmark and population budget
//! recommendation (design §12.5).

pub mod bench;
pub mod detect;
pub mod recommend;

pub use bench::{BenchConfig, BenchResult, run_benchmark};
pub use detect::{HardwareInfo, detect};
pub use recommend::{BudgetInputs, Recommendation, TIERS, Tier, recommend, speed_for_population};
