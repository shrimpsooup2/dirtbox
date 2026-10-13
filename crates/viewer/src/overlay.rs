//! Map overlays: what each cell's colour means, and the legend that explains it.
//!
//! Colour follows the job it does:
//! - magnitudes use a single blue hue, dark (low) to light (high), so low
//!   values recede into the dark background;
//! - signed quantities use blue ↔ red around a neutral grey midpoint;
//! - climate zones use a handful of categorical hues, always with their name
//!   in the legend and on hover.
//!
//! The "Satellite" view isn't a data encoding: it approximates what the
//! planet looks like from orbit (bare ground, water, ice and clouds).

use dirtbox_climate::ClimateZone;
use dirtbox_core::Vec3;
use dirtbox_grid::GeodesicGrid;
use dirtbox_sim::WorldState;

pub type Rgb = [u8; 3];

/// Single-hue blue ramp, darkest to lightest (steps 700 → 100).
const SEQUENTIAL: [u32; 13] = [
    0x0d366b, 0x104281, 0x184f95, 0x1c5cab, 0x256abf, 0x2a78d6, 0x3987e5, 0x5598e7, 0x6da7ec,
    0x86b6ef, 0x9ec5f4, 0xb7d3f6, 0xcde2fb,
];
const DIVERGING_NEGATIVE: u32 = 0x6da7ec;
const DIVERGING_MIDPOINT: u32 = 0x383835;
const DIVERGING_POSITIVE: u32 = 0xe66767;
const PLAIN: u32 = 0x77776f;
/// Non-data surface tones for the zone map.
const ZONE_OCEAN: u32 = 0x2b3440;
const ZONE_SEA_ICE: u32 = 0x9aa3ad;
const ZONE_ICE_CAP: u32 = 0xe6eaee;

fn hex(c: u32) -> Rgb {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| {
        (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t.clamp(0.0, 1.0)).round() as u8
    })
}

/// Sequential colour for t in [0, 1].
pub fn sequential(t: f32) -> Rgb {
    let x = t.clamp(0.0, 1.0) * (SEQUENTIAL.len() - 1) as f32;
    let i = (x as usize).min(SEQUENTIAL.len() - 2);
    lerp(hex(SEQUENTIAL[i]), hex(SEQUENTIAL[i + 1]), x - i as f32)
}

/// Diverging colour for t in [−1, 1]; equal-length arms around grey.
pub fn diverging(t: f32) -> Rgb {
    let t = t.clamp(-1.0, 1.0);
    if t < 0.0 {
        lerp(hex(DIVERGING_MIDPOINT), hex(DIVERGING_NEGATIVE), -t)
    } else {
        lerp(hex(DIVERGING_MIDPOINT), hex(DIVERGING_POSITIVE), t)
    }
}

/// The five main climate groups shown on the zone map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneGroup {
    Tropical,
    Arid,
    Temperate,
    Continental,
    Tundra,
}

impl ZoneGroup {
    pub const ALL: [ZoneGroup; 5] = [
        ZoneGroup::Tropical,
        ZoneGroup::Arid,
        ZoneGroup::Temperate,
        ZoneGroup::Continental,
        ZoneGroup::Tundra,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ZoneGroup::Tropical => "Tropical",
            ZoneGroup::Arid => "Arid",
            ZoneGroup::Temperate => "Temperate",
            ZoneGroup::Continental => "Continental",
            ZoneGroup::Tundra => "Tundra",
        }
    }

    /// Hues from the categorical palette (dark-surface steps), chosen to echo
    /// familiar map conventions: green tropics, yellow deserts.
    pub fn colour(self) -> Rgb {
        hex(match self {
            ZoneGroup::Tropical => 0x199e70,
            ZoneGroup::Arid => 0xc98500,
            ZoneGroup::Temperate => 0x3987e5,
            ZoneGroup::Continental => 0x9085e9,
            ZoneGroup::Tundra => 0xd55181,
        })
    }

    fn of(zone: ClimateZone) -> Option<ZoneGroup> {
        Some(match zone {
            ClimateZone::Rainforest | ClimateZone::Monsoon | ClimateZone::Savanna => {
                ZoneGroup::Tropical
            }
            ClimateZone::Desert | ClimateZone::Steppe => ZoneGroup::Arid,
            ClimateZone::Temperate => ZoneGroup::Temperate,
            ClimateZone::Continental => ZoneGroup::Continental,
            ClimateZone::Tundra => ZoneGroup::Tundra,
            ClimateZone::Ocean | ClimateZone::SeaIce | ClimateZone::IceCap => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Satellite,
    Elevation,
    Temperature,
    Precipitation,
    AnnualPrecipitation,
    Wind,
    Clouds,
    SnowIce,
    Rivers,
    ClimateZones,
    Sunlight,
    Latitude,
    CellArea,
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    None,
    Sequential,
    Diverging,
    Categorical,
}

pub struct Legend {
    pub scale: Scale,
    pub low: String,
    pub mid: Option<String>,
    pub high: String,
    pub note: &'static str,
}

impl Legend {
    fn none(note: &'static str) -> Self {
        Legend {
            scale: Scale::None,
            low: String::new(),
            mid: None,
            high: String::new(),
            note,
        }
    }

    fn sequential(low: impl Into<String>, high: impl Into<String>, note: &'static str) -> Self {
        Legend {
            scale: Scale::Sequential,
            low: low.into(),
            mid: None,
            high: high.into(),
            note,
        }
    }
}

impl Overlay {
    pub const ALL: [Overlay; 14] = [
        Overlay::Satellite,
        Overlay::Elevation,
        Overlay::Temperature,
        Overlay::Precipitation,
        Overlay::AnnualPrecipitation,
        Overlay::Wind,
        Overlay::Clouds,
        Overlay::SnowIce,
        Overlay::Rivers,
        Overlay::ClimateZones,
        Overlay::Sunlight,
        Overlay::Latitude,
        Overlay::CellArea,
        Overlay::Plain,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Overlay::Satellite => "Satellite",
            Overlay::Elevation => "Elevation",
            Overlay::Temperature => "Temperature",
            Overlay::Precipitation => "Rain and snow (now)",
            Overlay::AnnualPrecipitation => "Rain and snow (yearly)",
            Overlay::Wind => "Wind speed",
            Overlay::Clouds => "Cloud cover",
            Overlay::SnowIce => "Snow and ice",
            Overlay::Rivers => "Rivers",
            Overlay::ClimateZones => "Climate zones",
            Overlay::Sunlight => "Sunlight",
            Overlay::Latitude => "Latitude",
            Overlay::CellArea => "Cell area",
            Overlay::Plain => "Plain",
        }
    }

    /// Whether colours change as the simulation runs.
    pub fn is_live(self) -> bool {
        !matches!(
            self,
            Overlay::Latitude | Overlay::CellArea | Overlay::Plain | Overlay::Elevation
        )
    }

    /// Raw value per cell (in the overlay's display unit).
    pub fn values(self, state: &WorldState, radius_km: f64) -> Vec<f32> {
        let grid = &state.grid;
        let c = &state.climate;
        let t = &state.terrain;
        let year = c.stats.last_year.as_ref();
        grid.cells()
            .map(|cell| {
                let i = cell as usize;
                (match self {
                    Overlay::Satellite | Overlay::Plain | Overlay::ClimateZones => 0.0,
                    Overlay::Elevation => t.height_above_sea(cell),
                    Overlay::Temperature => c.temperature[i] - 273.15,
                    Overlay::Precipitation => c.precipitation[i] * 86_400.0,
                    Overlay::AnnualPrecipitation => {
                        year.map_or(f64::NAN, |y| y.annual_precipitation(i))
                    }
                    Overlay::Wind => c.wind[i].length(),
                    Overlay::Clouds => c.cloud[i] * 100.0,
                    Overlay::SnowIce => {
                        if t.is_ocean(cell) {
                            c.sea_ice[i]
                        } else {
                            c.snow[i] / 1000.0
                        }
                    }
                    Overlay::Rivers => c.river_flow[i],
                    Overlay::Sunlight => c.insolation[i],
                    Overlay::Latitude => grid.latitude(cell).to_degrees(),
                    Overlay::CellArea => grid.area(cell) * radius_km * radius_km,
                }) as f32
            })
            .collect()
    }

    /// Colour per cell and the legend describing it.
    pub fn colours(self, state: &WorldState, values: &[f32]) -> (Vec<Rgb>, Legend) {
        let terrain = &state.terrain;
        let n = values.len();
        let ocean = terrain.ocean_mask();
        match self {
            Overlay::Satellite => (
                (0..n).map(|i| satellite(state, i)).collect(),
                Legend::none(
                    "How the planet would look from orbit. No life yet, so the land is bare rock, sand and soil.",
                ),
            ),
            Overlay::Plain => (vec![hex(PLAIN); n], Legend::none("The bare grid.")),
            Overlay::Elevation => {
                let max_land = values.iter().copied().fold(1.0f32, f32::max);
                let max_depth = values.iter().copied().fold(-1.0f32, f32::min).abs();
                (
                    values
                        .iter()
                        .map(|&h| elevation_colour(h, max_land, max_depth))
                        .collect(),
                    Legend {
                        scale: Scale::Diverging,
                        low: format!("{} m deep", group(max_depth)),
                        mid: Some("sea level".into()),
                        high: format!("{} m high", group(max_land)),
                        note: "Below sea level in blue, land in red.",
                    },
                )
            }
            Overlay::Temperature => (
                values.iter().map(|&c| diverging(c / 40.0)).collect(),
                Legend {
                    scale: Scale::Diverging,
                    low: "−40 °C".into(),
                    mid: Some("0 °C".into()),
                    high: "+40 °C".into(),
                    note: "",
                },
            ),
            Overlay::Precipitation => (
                values
                    .iter()
                    .map(|&p| sequential(log_scale(p, 0.1, 30.0)))
                    .collect(),
                Legend::sequential("0.1 mm/day", "30 mm/day", "Logarithmic scale."),
            ),
            Overlay::AnnualPrecipitation => {
                if values.iter().all(|v| v.is_nan()) {
                    (
                        vec![hex(PLAIN); n],
                        Legend::none(
                            "Appears after the first full planet year has been simulated.",
                        ),
                    )
                } else {
                    (
                        values
                            .iter()
                            .map(|&p| sequential(log_scale(p, 50.0, 4000.0)))
                            .collect(),
                        Legend::sequential(
                            "50 mm/yr",
                            "4,000 mm/yr",
                            "Logarithmic scale, over the last full planet year.",
                        ),
                    )
                }
            }
            Overlay::Wind => (
                values.iter().map(|&v| sequential(v / 15.0)).collect(),
                Legend::sequential(
                    "0 m/s",
                    "15 m/s",
                    "Surface wind. Arrows show direction when zoomed in.",
                ),
            ),
            Overlay::Clouds => (
                values.iter().map(|&v| sequential(v / 100.0)).collect(),
                Legend::sequential("clear", "overcast", ""),
            ),
            Overlay::SnowIce => (
                values
                    .iter()
                    .map(|&v| sequential(log_scale(v, 0.01, 5.0)))
                    .collect(),
                Legend::sequential(
                    "1 cm",
                    "5 m",
                    "Sea ice thickness over water, snow (water equivalent) on land.",
                ),
            ),
            Overlay::Rivers => (
                (0..n)
                    .map(|i| {
                        if ocean[i] {
                            hex(ZONE_OCEAN)
                        } else {
                            sequential(log_scale(values[i], 10.0, 100_000.0))
                        }
                    })
                    .collect(),
                Legend::sequential(
                    "10 m³/s",
                    "100,000 m³/s",
                    "River discharge on land, logarithmic.",
                ),
            ),
            Overlay::ClimateZones => match state.climate.stats.last_year.as_ref() {
                None => (
                    vec![hex(PLAIN); n],
                    Legend::none("Appears after the first full planet year has been simulated."),
                ),
                Some(year) => (
                    year.zones
                        .iter()
                        .map(|&z| match z {
                            ClimateZone::Ocean => hex(ZONE_OCEAN),
                            ClimateZone::SeaIce => hex(ZONE_SEA_ICE),
                            ClimateZone::IceCap => hex(ZONE_ICE_CAP),
                            other => ZoneGroup::of(other).map_or(hex(PLAIN), ZoneGroup::colour),
                        })
                        .collect(),
                    Legend {
                        scale: Scale::Categorical,
                        low: String::new(),
                        mid: None,
                        high: String::new(),
                        note: "Köppen groups, diagnosed from the last simulated year. Hover a cell for its exact zone.",
                    },
                ),
            },
            Overlay::Sunlight => (
                values.iter().map(|&v| sequential(v / 550.0)).collect(),
                Legend::sequential(
                    "0 W/m²",
                    "550 W/m²",
                    "Sunlight at the top of the atmosphere right now.",
                ),
            ),
            Overlay::Latitude => (
                values.iter().map(|&lat| diverging(lat / 90.0)).collect(),
                Legend {
                    scale: Scale::Diverging,
                    low: "90° S".into(),
                    mid: Some("Equator".into()),
                    high: "90° N".into(),
                    note: "",
                },
            ),
            Overlay::CellArea => {
                let (lo, hi) = values
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
                let span = (hi - lo).max(f32::EPSILON);
                (
                    values
                        .iter()
                        .map(|&v| sequential((v - lo) / span))
                        .collect(),
                    Legend::sequential(
                        format!("{} km²", group(lo)),
                        format!("{} km²", group(hi)),
                        "The smallest cells are the 12 pentagons.",
                    ),
                )
            }
        }
    }
}

/// Depth in the blue arm, height in the red arm; square-root scaled so
/// shallow seas and lowlands still separate.
fn elevation_colour(h: f32, max_land: f32, max_depth: f32) -> Rgb {
    if h < 0.0 {
        diverging(-(-h / max_depth).sqrt())
    } else {
        diverging((h / max_land).sqrt().max(0.02))
    }
}

/// Maps v to 0–1 on a log scale between lo and hi.
fn log_scale(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_nan() || v <= lo {
        return 0.0;
    }
    (v / lo).ln() / (hi / lo).ln()
}

/// Approximate true colour of a lifeless surface, with clouds on top.
fn satellite(state: &WorldState, i: usize) -> Rgb {
    let c = &state.climate;
    let t = &state.terrain;
    let cell = i as u32;
    let surface = if t.is_ocean(cell) {
        if c.is_ice_covered(cell) {
            hex(0xdde4ea)
        } else {
            let depth = (-t.height_above_sea(cell) / 5000.0).clamp(0.0, 1.0) as f32;
            lerp(hex(0x2f6d8f), hex(0x0b2340), depth.sqrt())
        }
    } else if c.lake[i] {
        hex(0x2f6d8f)
    } else {
        let wet = (c.soil[i] / c.params.soil_capacity).clamp(0.0, 1.0) as f32;
        let height = (t.height_above_sea(cell) / 5000.0).clamp(0.0, 1.0) as f32;
        let ground = lerp(hex(0xc8aa78), hex(0x6e5b48), wet);
        let ground = lerp(ground, hex(0x8c8782), height);
        let snow = (c.snow[i] / 60.0).clamp(0.0, 1.0) as f32;
        let ground = lerp(ground, hex(0xf2f4f6), snow);
        // Big rivers show as blue threads.
        let river = log_scale(c.river_flow[i] as f32, 2_000.0, 200_000.0);
        lerp(ground, hex(0x2f6d8f), river * 0.8)
    };
    // Only properly cloudy skies show, so storm belts and clear deserts stand out.
    let cloud = c.cloud[i] as f32;
    let cover = ((cloud - 0.5) / 0.45).clamp(0.0, 1.0);
    lerp(
        surface,
        hex(0xf4f6f8),
        cover * cover * (3.0 - 2.0 * cover) * 0.85,
    )
}

/// Text for one cell's overlay value.
pub fn describe(overlay: Overlay, value: f32) -> Option<String> {
    if value.is_nan() {
        return None;
    }
    Some(match overlay {
        Overlay::Satellite | Overlay::Plain | Overlay::ClimateZones | Overlay::CellArea => {
            return None;
        }
        Overlay::Elevation if value < 0.0 => format!("{} m below sea level", group(-value)),
        Overlay::Elevation => format!("{} m above sea level", group(value)),
        Overlay::Temperature => format!("{value:.1} °C"),
        Overlay::Precipitation => format!("{value:.1} mm/day"),
        Overlay::AnnualPrecipitation => format!("{} mm/yr", group(value)),
        Overlay::Wind => format!("{value:.1} m/s"),
        Overlay::Clouds => format!("{value:.0}%"),
        Overlay::SnowIce => format!("{value:.2} m"),
        Overlay::Rivers => format!("{} m³/s", group(value)),
        Overlay::Sunlight => format!("{value:.0} W/m²"),
        Overlay::Latitude => format!(
            "{:.1}° {}",
            value.abs(),
            if value >= 0.0 { "N" } else { "S" }
        ),
    })
}

/// 12345.6 → "12,346".
pub fn group(v: f32) -> String {
    let s = format!("{:.0}", v.abs());
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if v < 0.0 { format!("-{out}") } else { out }
}

/// Compass direction a wind blows *from*, for a tangent vector at a point.
pub fn wind_from(grid: &GeodesicGrid, cell: u32, wind: Vec3) -> String {
    let (east, north) = grid.east_north(cell);
    let (u, v) = (wind.dot(east), wind.dot(north));
    if (u * u + v * v).sqrt() < 0.2 {
        return "calm".into();
    }
    // Direction the air comes from, clockwise from north.
    let bearing = (-u).atan2(-v).to_degrees().rem_euclid(360.0);
    const NAMES: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    NAMES[((bearing + 22.5) / 45.0) as usize % 8].into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_hit_their_endpoints() {
        assert_eq!(sequential(0.0), hex(SEQUENTIAL[0]));
        assert_eq!(sequential(1.0), hex(SEQUENTIAL[12]));
        assert_eq!(diverging(0.0), hex(DIVERGING_MIDPOINT));
        assert_eq!(diverging(-1.0), hex(DIVERGING_NEGATIVE));
        assert_eq!(diverging(1.0), hex(DIVERGING_POSITIVE));
    }

    #[test]
    fn log_scale_bounds() {
        assert_eq!(log_scale(0.0, 1.0, 100.0), 0.0);
        assert_eq!(log_scale(f32::NAN, 1.0, 100.0), 0.0);
        assert!((log_scale(10.0, 1.0, 100.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn wind_direction_names() {
        let grid = GeodesicGrid::new(4);
        let (east, north) = grid.east_north(10);
        // Air moving east comes from the west.
        assert_eq!(wind_from(&grid, 10, east * 5.0), "W");
        assert_eq!(wind_from(&grid, 10, north * -5.0), "N");
        assert_eq!(wind_from(&grid, 10, Vec3::ZERO), "calm");
    }

    #[test]
    fn grouping() {
        assert_eq!(group(1234567.0), "1,234,567");
        assert_eq!(group(-1200.0), "-1,200");
    }
}
