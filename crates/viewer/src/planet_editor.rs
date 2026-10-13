//! Live editor for the planetary system: star, orbit, rotation, body and
//! atmosphere. Every change applies to the running world immediately.

use eframe::egui::{self, RichText};

use dirtbox_core::consts::{EARTH_GRAVITY, SOLAR_LUMINOSITY};
use dirtbox_planet::PlanetParams;

/// Draws the editor. Returns true if any parameter changed.
pub fn planet_editor(ui: &mut egui::Ui, p: &mut PlanetParams, earth_flux: f64) -> bool {
    let mut changed = false;

    heading(ui, "Star");
    changed |= ui
        .add(slider(&mut p.star.mass, 0.1..=3.0, "mass", " M☉").logarithmic(true))
        .on_hover_text("Bigger stars are hotter, brighter and bluer.")
        .changed();
    changed |= ui
        .add(slider(&mut p.star.luminosity_factor, 0.5..=1.5, "brightness", "×"))
        .on_hover_text("Multiplies the star's normal brightness: dim it for an ice age, brighten it for a heat wave.")
        .changed();
    note(
        ui,
        format!(
            "{:.0} K surface · {:.3} L☉ · {:.0}% usable light",
            p.star.temperature(),
            p.star.luminosity() / SOLAR_LUMINOSITY,
            p.star.par_fraction() * 100.0
        ),
    );

    heading(ui, "Orbit");
    changed |= ui
        .add(slider(&mut p.orbit.semi_major_axis, 0.02..=5.0, "distance", " AU").logarithmic(true))
        .changed();
    changed |= ui
        .add(slider(&mut p.orbit.eccentricity, 0.0..=0.9, "eccentricity", ""))
        .on_hover_text("0 is circular. Higher values swing the planet closer to and farther from its star each year.")
        .changed();
    changed |= angle(ui, &mut p.orbit.obliquity, 0.0..=180.0, "axial tilt")
        .on_hover_text(
            "Tilt makes seasons. 0° has none; 90° turns the poles toward the star in summer.",
        )
        .changed();
    changed |= angle(
        ui,
        &mut p.orbit.perihelion_longitude,
        0.0..=360.0,
        "closest approach",
    )
    .on_hover_text(
        "Where in the seasonal cycle the planet passes closest to its star (0° = northern spring).",
    )
    .changed();
    note(
        ui,
        format!(
            "Year: {:.1} Earth days · Sunlight: {:.2}× Earth",
            p.year_seconds() / 86_400.0,
            p.mean_flux() / earth_flux
        ),
    );

    heading(ui, "Rotation");
    changed |= ui
        .checkbox(&mut p.rotation.tidally_locked, "Tidally locked")
        .on_hover_text("One side always faces the star.")
        .changed();
    ui.add_enabled_ui(!p.rotation.tidally_locked, |ui| {
        changed |= ui
            .add(
                slider(
                    &mut p.rotation.sidereal_day_hours,
                    2.0..=2_000.0,
                    "spin period",
                    " h",
                )
                .logarithmic(true),
            )
            .changed();
    });
    note(
        ui,
        match p.solar_day_seconds() {
            Some(d) => format!("Sunrise to sunrise: {:.1} hours", d / 3600.0),
            None => "The star never moves in the sky.".into(),
        },
    );

    heading(ui, "Size");
    changed |= ui
        .add(slider(&mut p.body.radius_km, 1_000.0..=20_000.0, "radius", " km").logarithmic(true))
        .changed();
    changed |= ui
        .add(slider(
            &mut p.body.density,
            1_000.0..=12_000.0,
            "density",
            " kg/m³",
        ))
        .on_hover_text("Rock is ~3,000–5,500; iron-rich worlds are denser.")
        .changed();
    let escape = p.body.escape_velocity() / 1000.0;
    note(
        ui,
        format!(
            "Gravity {:.2} g · escape velocity {:.1} km/s",
            p.body.gravity() / EARTH_GRAVITY,
            escape
        ),
    );
    if escape < 5.0 {
        ui.label(
            RichText::new(
                "⚠ Gravity this weak would let a thick atmosphere leak into space over time.",
            )
            .small()
            .color(ui.visuals().warn_fg_color),
        );
    }

    heading(ui, "Atmosphere (partial pressures)");
    let gases: [(&mut f64, &str, f64, &str); 5] = [
        (
            &mut p.atmosphere.n2,
            "nitrogen",
            10.0,
            "Inert filler: thickens the air and spreads heat.",
        ),
        (
            &mut p.atmosphere.o2,
            "oxygen",
            2.0,
            "Starts near zero on a world without life.",
        ),
        (
            &mut p.atmosphere.co2,
            "CO₂",
            10.0,
            "The main long-lived greenhouse gas.",
        ),
        (
            &mut p.atmosphere.ch4,
            "methane",
            0.1,
            "A strong greenhouse gas; life can make it.",
        ),
        (&mut p.atmosphere.argon, "argon", 1.0, "Inert."),
    ];
    for (value, name, max, help) in gases {
        changed |= ui
            .add(
                egui::Slider::new(value, 0.0..=max)
                    .logarithmic(true)
                    .smallest_positive(1e-7)
                    .text(name)
                    .custom_formatter(|v, _| bar(v)),
            )
            .on_hover_text(help)
            .changed();
    }
    note(
        ui,
        format!(
            "Surface pressure {} · CO₂ {} ppm",
            bar(p.atmosphere.pressure_bar()),
            crate::overlay::group(p.atmosphere.ppm(p.atmosphere.co2) as f32)
        ),
    );
    changed
}

fn slider<'a>(
    v: &'a mut f64,
    range: std::ops::RangeInclusive<f64>,
    text: &str,
    suffix: &str,
) -> egui::Slider<'a> {
    egui::Slider::new(v, range).text(text).suffix(suffix)
}

/// Slider over an angle stored in radians but shown in degrees.
fn angle(
    ui: &mut egui::Ui,
    radians: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    text: &str,
) -> egui::Response {
    let mut deg = radians.to_degrees();
    let r = ui.add(egui::Slider::new(&mut deg, range).text(text).suffix("°"));
    if r.changed() {
        *radians = deg.to_radians();
    }
    r
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).strong());
}

fn note(ui: &mut egui::Ui, text: String) {
    ui.label(RichText::new(text).weak().small());
}

/// Pressure with a readable unit: bar, mbar or µbar.
fn bar(v: f64) -> String {
    if v == 0.0 {
        "0 bar".into()
    } else if v >= 0.1 {
        format!("{v:.2} bar")
    } else if v >= 1e-3 {
        format!("{:.2} mbar", v * 1e3)
    } else {
        format!("{:.2} µbar", v * 1e6)
    }
}
