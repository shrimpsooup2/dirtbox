//! Thermodynamic and radiative relations used by the column physics.

use dirtbox_core::consts::{BAR, FREEZING_FRESH, STEFAN_BOLTZMANN};
use dirtbox_planet::Atmosphere;

/// Saturation vapour pressure over water or ice, Pa (Tetens formula).
pub fn saturation_vapour_pressure(t: f64) -> f64 {
    let c = t - FREEZING_FRESH;
    if t >= FREEZING_FRESH {
        611.0 * (17.27 * c / (t - 35.86)).exp()
    } else {
        611.0 * (21.875 * c / (t - 7.66)).exp()
    }
}

/// Column water vapour at saturation, kg m⁻². Vapour falls off with height
/// much faster than air (its scale height is ~2 km against ~8 km), so the
/// column holds about a third of what a well-mixed layer would:
/// W_sat ≈ 0.35 · 0.622 · e_s / g, about 38 kg m⁻² at 288 K on Earth.
pub fn saturated_column_water(t: f64, gravity: f64) -> f64 {
    0.35 * 0.622 * saturation_vapour_pressure(t) / gravity
}

/// Saturation specific humidity at the surface, kg kg⁻¹.
pub fn saturation_specific_humidity(t: f64, pressure: f64) -> f64 {
    (0.622 * saturation_vapour_pressure(t) / pressure.max(1.0)).min(1.0)
}

/// Gray infrared optical depth of the atmosphere.
///
/// Calibrated to pre-industrial Earth's greenhouse budget: about half from
/// water vapour, a quarter from clouds, a fifth from CO₂ (280 ppm), the rest
/// from methane. With ~25 kg m⁻² of vapour and 60% cloud this gives τ ≈ 0.82,
/// and with OLR = σT⁴/(1 + ¾τ) a 288 K mean at Earth's 0.30 albedo. Gas terms
/// grow logarithmically with partial pressure and are pressure-broadened, so
/// thin air (Mars) has a weak greenhouse even when it is nearly pure CO₂.
pub fn optical_depth(atm: &Atmosphere, column_water: f64, cloud_fraction: f64) -> f64 {
    let p = atm.pressure_bar();
    let broadening = p.max(1e-6).powf(0.6);
    let co2 = 0.08 * (1.0 + atm.co2 / 4e-5).ln() * broadening;
    let ch4 = 0.01 * (1.0 + atm.ch4 / 1e-7).ln() * broadening;
    // W^0.35 rather than √W: a gentler water-vapour feedback that stands in for
    // the offsetting lapse-rate feedback a single-layer model lacks.
    let h2o = 0.152 * column_water.max(0.0).powf(0.35) * p.max(1e-6).powf(0.3);
    let cloud = 0.35 * cloud_fraction;
    co2 + ch4 + h2o + cloud
}

/// Outgoing longwave radiation for surface temperature `t` and optical depth `tau`.
pub fn outgoing_longwave(t: f64, tau: f64) -> f64 {
    STEFAN_BOLTZMANN * t.powi(4) / (1.0 + 0.75 * tau)
}

/// d(OLR)/dT, for the implicit solve.
pub fn outgoing_longwave_derivative(t: f64, tau: f64) -> f64 {
    4.0 * STEFAN_BOLTZMANN * t.powi(3) / (1.0 + 0.75 * tau)
}

/// Clear-sky scattering albedo of the air itself (Rayleigh), growing with pressure.
pub fn rayleigh_albedo(atm: &Atmosphere) -> f64 {
    (0.04 * (atm.pressure() / BAR).sqrt()).min(0.3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirtbox_planet::Preset;

    #[test]
    fn saturation_vapour_pressure_known_points() {
        assert!((saturation_vapour_pressure(273.15) - 611.0).abs() < 1.0);
        assert!((saturation_vapour_pressure(293.15) - 2339.0).abs() < 30.0);
        assert!((saturation_vapour_pressure(373.15) - 101_325.0).abs() < 3000.0);
    }

    #[test]
    fn clausius_clapeyron_about_seven_percent_per_kelvin() {
        let r = saturated_column_water(289.0, 9.81) / saturated_column_water(288.0, 9.81);
        assert!((1.06..1.075).contains(&r), "{r}");
    }

    #[test]
    fn earth_greenhouse_gives_288_kelvin() {
        let atm = Preset::EarthModern.params().atmosphere;
        let tau = optical_depth(&atm, 25.0, 0.6);
        assert!((0.8..0.9).contains(&tau), "{tau}");
        // Global balance: 1361/4 · (1 − 0.30) = σT⁴/(1 + ¾τ).
        let absorbed: f64 = 1361.0 / 4.0 * 0.70;
        let t = (absorbed * (1.0 + 0.75 * tau) / STEFAN_BOLTZMANN).powf(0.25);
        assert!((t - 288.0).abs() < 3.0, "{t}");
    }

    #[test]
    fn thin_co2_air_has_a_weak_greenhouse() {
        let mars = Preset::ColdDesert.params().atmosphere;
        assert!(optical_depth(&mars, 0.0, 0.0) < 0.1);
    }
}
