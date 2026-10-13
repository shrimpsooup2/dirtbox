//! Monthly climate statistics and climate-zone classification.
//!
//! Zones follow the Köppen–Geiger scheme (Peel et al. 2007), simplified to
//! its main groups. They are *diagnosed* from the simulated year (monthly
//! temperature and rainfall), never assigned, so deserts appear wherever the
//! circulation and terrain make them.

use dirtbox_core::consts::FREEZING_FRESH;

/// Precipitation rates are converted to "mm per 30.44 Earth days" so the
/// Köppen thresholds mean the same thing on planets with any year length.
const MONTH_SECONDS: f64 = 30.44 * 86_400.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClimateZone {
    Ocean,
    SeaIce,
    Rainforest,
    Monsoon,
    Savanna,
    Desert,
    Steppe,
    Temperate,
    Continental,
    Tundra,
    IceCap,
}

impl ClimateZone {
    pub const ALL: [ClimateZone; 11] = [
        ClimateZone::Ocean,
        ClimateZone::SeaIce,
        ClimateZone::Rainforest,
        ClimateZone::Monsoon,
        ClimateZone::Savanna,
        ClimateZone::Desert,
        ClimateZone::Steppe,
        ClimateZone::Temperate,
        ClimateZone::Continental,
        ClimateZone::Tundra,
        ClimateZone::IceCap,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ClimateZone::Ocean => "Ocean",
            ClimateZone::SeaIce => "Sea ice",
            ClimateZone::Rainforest => "Tropical rainforest",
            ClimateZone::Monsoon => "Tropical monsoon",
            ClimateZone::Savanna => "Savanna",
            ClimateZone::Desert => "Desert",
            ClimateZone::Steppe => "Steppe",
            ClimateZone::Temperate => "Temperate",
            ClimateZone::Continental => "Continental",
            ClimateZone::Tundra => "Tundra",
            ClimateZone::IceCap => "Ice cap",
        }
    }
}

/// Twelve monthly means for one year.
#[derive(Clone, Debug)]
pub struct YearSummary {
    /// Monthly mean temperature per cell, K.
    pub temperature: Vec<[f32; 12]>,
    /// Monthly precipitation rate per cell, mm per 30.44 days.
    pub precipitation: Vec<[f32; 12]>,
    pub zones: Vec<ClimateZone>,
}

impl YearSummary {
    pub fn annual_mean_temperature(&self, cell: usize) -> f64 {
        self.temperature[cell]
            .iter()
            .map(|&t| t as f64)
            .sum::<f64>()
            / 12.0
    }

    /// Annual precipitation, mm per 365.25 days.
    pub fn annual_precipitation(&self, cell: usize) -> f64 {
        self.precipitation[cell]
            .iter()
            .map(|&p| p as f64)
            .sum::<f64>()
            * 365.25
            / (12.0 * 30.44)
    }
}

pub struct MonthlyStats {
    year: Option<i64>,
    sum_t: Vec<[f64; 12]>,
    sum_p: Vec<[f64; 12]>,
    time: [f64; 12],
    pub last_year: Option<YearSummary>,
}

impl MonthlyStats {
    pub fn new(cells: usize) -> Self {
        Self {
            year: None,
            sum_t: vec![[0.0; 12]; cells],
            sum_p: vec![[0.0; 12]; cells],
            time: [0.0; 12],
            last_year: None,
        }
    }

    /// Adds `dt` planet seconds of temperature (K) and precipitation rate
    /// (kg m⁻² s⁻¹). Completes a year summary when the year rolls over and
    /// every month has data.
    pub fn record(
        &mut self,
        year: i64,
        year_fraction: f64,
        dt: f64,
        t: &[f64],
        p: &[f64],
        ocean: &[bool],
    ) {
        if self.year != Some(year) {
            if self.year.is_some() && self.time.iter().all(|&m| m > 0.0) {
                self.last_year = Some(self.summarise(ocean));
            }
            self.year = Some(year);
            self.time = [0.0; 12];
            self.sum_t.iter_mut().for_each(|m| *m = [0.0; 12]);
            self.sum_p.iter_mut().for_each(|m| *m = [0.0; 12]);
        }
        let month = ((year_fraction * 12.0) as usize).min(11);
        self.time[month] += dt;
        for i in 0..t.len() {
            self.sum_t[i][month] += t[i] * dt;
            self.sum_p[i][month] += p[i] * dt;
        }
    }

    fn summarise(&self, ocean: &[bool]) -> YearSummary {
        let temperature: Vec<[f32; 12]> = self
            .sum_t
            .iter()
            .map(|m| std::array::from_fn(|k| (m[k] / self.time[k]) as f32))
            .collect();
        let precipitation: Vec<[f32; 12]> = self
            .sum_p
            .iter()
            .map(|m| std::array::from_fn(|k| (m[k] / self.time[k] * MONTH_SECONDS) as f32))
            .collect();
        let zones = (0..temperature.len())
            .map(|i| classify(&temperature[i], &precipitation[i], ocean[i]))
            .collect();
        YearSummary {
            temperature,
            precipitation,
            zones,
        }
    }
}

/// Köppen–Geiger main groups from monthly temperature (K) and precipitation
/// (mm per 30.44 days).
pub fn classify(t: &[f32; 12], p: &[f32; 12], ocean: bool) -> ClimateZone {
    let c: Vec<f64> = t.iter().map(|&k| k as f64 - FREEZING_FRESH).collect();
    let p: Vec<f64> = p.iter().map(|&v| v as f64).collect();
    let t_max = c.iter().copied().fold(f64::MIN, f64::max);
    let t_min = c.iter().copied().fold(f64::MAX, f64::min);
    if ocean {
        return if t_max < -1.0 {
            ClimateZone::SeaIce
        } else {
            ClimateZone::Ocean
        };
    }
    if t_max < 10.0 {
        return if t_max > 0.0 {
            ClimateZone::Tundra
        } else {
            ClimateZone::IceCap
        };
    }
    let mat = c.iter().sum::<f64>() / 12.0;
    let map: f64 = p.iter().sum::<f64>() * 365.25 / (12.0 * 30.44);
    // Summer = the six warmest months.
    let mut idx: Vec<usize> = (0..12).collect();
    idx.sort_by(|&a, &b| c[b].total_cmp(&c[a]));
    let summer: f64 = idx[..6].iter().map(|&k| p[k]).sum();
    let total: f64 = p.iter().sum::<f64>().max(1e-9);
    let offset = if summer / total >= 0.7 {
        280.0
    } else if summer / total <= 0.3 {
        0.0
    } else {
        140.0
    };
    let threshold = 20.0 * mat + offset;
    if map < threshold {
        return if map < 0.5 * threshold {
            ClimateZone::Desert
        } else {
            ClimateZone::Steppe
        };
    }
    if t_min >= 18.0 {
        let driest = p.iter().copied().fold(f64::MAX, f64::min);
        return if driest >= 60.0 {
            ClimateZone::Rainforest
        } else if driest >= 100.0 - map / 25.0 {
            ClimateZone::Monsoon
        } else {
            ClimateZone::Savanna
        };
    }
    if t_min > 0.0 {
        ClimateZone::Temperate
    } else {
        ClimateZone::Continental
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: f32) -> f32 {
        c + 273.15
    }

    #[test]
    fn classic_climates() {
        let hot = [k(27.0); 12];
        assert_eq!(classify(&hot, &[200.0; 12], false), ClimateZone::Rainforest);
        assert_eq!(classify(&hot, &[2.0; 12], false), ClimateZone::Desert);
        let mut savanna_p = [150.0f32; 12];
        savanna_p[..5].copy_from_slice(&[5.0; 5]);
        assert_eq!(classify(&hot, &savanna_p, false), ClimateZone::Savanna);
        let seasonal: [f32; 12] = std::array::from_fn(|m| {
            k(10.0 + 9.0 * ((m as f32) * std::f32::consts::FRAC_PI_6).cos())
        });
        assert_eq!(
            classify(&seasonal, &[80.0; 12], false),
            ClimateZone::Temperate
        );
        let cold: [f32; 12] = std::array::from_fn(|m| {
            k(-5.0 + 20.0 * ((m as f32) * std::f32::consts::FRAC_PI_6).cos())
        });
        assert_eq!(
            classify(&cold, &[60.0; 12], false),
            ClimateZone::Continental
        );
        assert_eq!(
            classify(&[k(-20.0); 12], &[10.0; 12], false),
            ClimateZone::IceCap
        );
        assert_eq!(
            classify(&[k(-5.0); 12], &[10.0; 12], true),
            ClimateZone::SeaIce
        );
    }

    #[test]
    fn year_summary_completes_on_rollover() {
        let mut s = MonthlyStats::new(1);
        for step in 0..12 {
            let frac = step as f64 / 12.0 + 0.01;
            s.record(0, frac, 1.0, &[300.0], &[1e-5], &[false]);
        }
        assert!(s.last_year.is_none(), "year not finished yet");
        s.record(1, 0.0, 1.0, &[300.0], &[1e-5], &[false]);
        let y = s.last_year.as_ref().unwrap();
        assert!((y.annual_mean_temperature(0) - 300.0).abs() < 1e-3);
        // 1e-5 kg/m²/s ≈ 315 mm per year.
        assert!(
            (y.annual_precipitation(0) - 315.6).abs() < 1.0,
            "{}",
            y.annual_precipitation(0)
        );
    }
}
