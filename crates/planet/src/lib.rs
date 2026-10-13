//! The planetary system: star, orbit, rotation, body, atmosphere (design §3),
//! the astronomy that turns them into seasons and sunlight, and terrain.

pub mod astronomy;
pub mod noise;
pub mod params;
pub mod presets;
pub mod terrain;

pub use astronomy::{SunState, daily_mean_insolation, instantaneous_insolation};
pub use params::{Atmosphere, Body, Orbit, PlanetParams, Rotation, Star, TerrainParams};
pub use presets::Preset;
pub use terrain::{Brush, BrushMode, Terrain};
