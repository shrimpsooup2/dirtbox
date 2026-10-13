//! Map overlays: what each cell's colour means, and the legend that explains it.
//!
//! Colours follow one rule per job. Magnitudes use one blue hue, dark (low) to
//! light (high), so low values recede into the dark background. Signed values
//! use blue ↔ red with a neutral grey midpoint.

use dirtbox_core::Vec3;
use dirtbox_grid::GeodesicGrid;

pub type Rgb = [u8; 3];

/// Single-hue blue ramp, darkest to lightest (steps 700 → 100).
const SEQUENTIAL: [u32; 13] = [
    0x0d366b, 0x104281, 0x184f95, 0x1c5cab, 0x256abf, 0x2a78d6, 0x3987e5, 0x5598e7, 0x6da7ec,
    0x86b6ef, 0x9ec5f4, 0xb7d3f6, 0xcde2fb,
];
/// Diverging poles and the neutral midpoint for a dark background.
const DIVERGING_NEGATIVE: u32 = 0x6da7ec;
const DIVERGING_MIDPOINT: u32 = 0x383835;
const DIVERGING_POSITIVE: u32 = 0xe66767;
/// Neutral surface for the plain overlay.
const PLAIN: u32 = 0x77776f;

fn hex(c: u32) -> Rgb {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Plain,
    Latitude,
    CellArea,
    SunElevation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    None,
    Sequential,
    Diverging,
}

pub struct Legend {
    pub scale: Scale,
    pub low: String,
    pub mid: Option<String>,
    pub high: String,
    pub note: &'static str,
}

impl Overlay {
    pub const ALL: [Overlay; 4] = [
        Overlay::Plain,
        Overlay::Latitude,
        Overlay::CellArea,
        Overlay::SunElevation,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Overlay::Plain => "Plain",
            Overlay::Latitude => "Latitude",
            Overlay::CellArea => "Cell area",
            Overlay::SunElevation => "Sun elevation",
        }
    }

    /// Whether the overlay changes as time passes.
    pub fn is_time_dependent(self) -> bool {
        self == Overlay::SunElevation
    }

    /// Raw value per cell (in the overlay's display unit).
    pub fn values(self, grid: &GeodesicGrid, ctx: &OverlayContext) -> Vec<f32> {
        let r2 = ctx.radius_km * ctx.radius_km;
        grid.cells()
            .map(|c| match self {
                Overlay::Plain => 0.0,
                Overlay::Latitude => grid.latitude(c).to_degrees() as f32,
                Overlay::CellArea => (grid.area(c) * r2) as f32,
                Overlay::SunElevation => grid
                    .position(c)
                    .dot(ctx.sun)
                    .clamp(-1.0, 1.0)
                    .asin()
                    .to_degrees() as f32,
            })
            .collect()
    }

    /// Colour per cell for the given values, and the legend describing it.
    pub fn colours(self, values: &[f32]) -> (Vec<Rgb>, Legend) {
        match self {
            Overlay::Plain => (
                vec![hex(PLAIN); values.len()],
                Legend {
                    scale: Scale::None,
                    low: String::new(),
                    mid: None,
                    high: String::new(),
                    note: "The bare grid. Terrain and climate arrive in Phase 1.",
                },
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
                    Legend {
                        scale: Scale::Sequential,
                        low: format!("{} km²", group(lo)),
                        mid: None,
                        high: format!("{} km²", group(hi)),
                        note: "The smallest cells are the 12 pentagons at the icosahedron's corners.",
                    },
                )
            }
            Overlay::SunElevation => (
                values
                    .iter()
                    .map(|&deg| sequential(deg.max(0.0) / 90.0))
                    .collect(),
                Legend {
                    scale: Scale::Sequential,
                    low: "Night (≤ 0°)".into(),
                    mid: None,
                    high: "90° (overhead)".into(),
                    note: "Sun held in the equatorial plane until the orbit and axial tilt arrive in Phase 1.",
                },
            ),
        }
    }

    /// Text for a single cell's value.
    pub fn describe(self, value: f32) -> Option<String> {
        match self {
            Overlay::Plain => None,
            Overlay::Latitude => Some(format!(
                "{:.1}° {}",
                value.abs(),
                if value >= 0.0 { "N" } else { "S" }
            )),
            Overlay::CellArea => Some(format!("{} km²", group(value))),
            Overlay::SunElevation if value <= 0.0 => {
                Some(format!("night (sun {:.1}° below horizon)", -value))
            }
            Overlay::SunElevation => Some(format!("sun {value:.1}° above horizon")),
        }
    }
}

pub struct OverlayContext {
    pub sun: Vec3,
    pub radius_km: f64,
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
    fn sequential_gets_lighter() {
        let luma = |c: Rgb| c[0] as u32 * 3 + c[1] as u32 * 6 + c[2] as u32;
        let mut last = 0;
        for i in 0..=20 {
            let l = luma(sequential(i as f32 / 20.0));
            assert!(l >= last);
            last = l;
        }
    }

    #[test]
    fn grouping() {
        assert_eq!(group(1234567.0), "1,234,567");
        assert_eq!(group(999.0), "999");
        assert_eq!(group(-1200.0), "-1,200");
    }
}
