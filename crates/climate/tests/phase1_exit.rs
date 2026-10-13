//! Phase 1 exit checks (design §13): the behaviours the climate must produce
//! from physics alone, with nothing scripted.

use dirtbox_climate::{Climate, ClimateParams, ClimateZone, StepInputs, StepMode, YearSummary};
use dirtbox_core::Vec3;
use dirtbox_grid::GeodesicGrid;
use dirtbox_planet::{PlanetParams, Preset, Terrain};

struct Run {
    grid: GeodesicGrid,
    planet: PlanetParams,
    terrain: Terrain,
    climate: Climate,
    t: f64,
}

impl Run {
    fn new(subdivision: u32, planet: PlanetParams, terrain: Option<Terrain>, frozen: bool) -> Self {
        let grid = GeodesicGrid::new(subdivision);
        let terrain = terrain.unwrap_or_else(|| Terrain::generate(&grid, &planet, 1));
        let climate = Climate::new(&grid, &planet, &terrain, ClimateParams::default(), frozen);
        Run {
            grid,
            planet,
            terrain,
            climate,
            t: 0.0,
        }
    }

    fn years(&mut self, years: f64) {
        let dt = self.planet.calendar().day_length.min(86_400.0);
        let steps = (years * self.planet.year_seconds() / dt).round() as usize;
        for _ in 0..steps {
            self.climate.step(&StepInputs {
                grid: &self.grid,
                planet: &self.planet,
                terrain: &self.terrain,
                start: self.t,
                dt,
                mode: StepMode::DailyMean,
                geologic_rate: 0.0,
            });
            self.t += dt;
        }
    }

    fn year(&self) -> &YearSummary {
        self.climate
            .stats
            .last_year
            .as_ref()
            .expect("no full year recorded")
    }

    fn ice_fraction(&self) -> f64 {
        self.climate.summary(&self.grid, &self.terrain).ice_fraction
    }

    /// Area-weighted mean of `f` over cells matching `keep`.
    fn mean(&self, keep: impl Fn(u32) -> bool, f: impl Fn(usize) -> f64) -> f64 {
        let (mut s, mut w) = (0.0, 0.0);
        for c in self.grid.cells().filter(|&c| keep(c)) {
            s += f(c as usize) * self.grid.area(c);
            w += self.grid.area(c);
        }
        assert!(w > 0.0, "no cells selected");
        s / w
    }

    fn lat(&self, c: u32) -> f64 {
        self.grid.latitude(c).to_degrees()
    }
}

#[test]
fn seasons_flip_between_hemispheres() {
    let mut run = Run::new(16, Preset::EarthModern.params(), None, false);
    run.years(3.0);
    let year = run.year();
    // t = 0 is the March equinox, so months 3–5 are June–August and 9–11 are December–February.
    let season = |north: bool, months: [usize; 3]| {
        run.mean(
            |c| {
                if north {
                    run.lat(c) > 30.0
                } else {
                    run.lat(c) < -30.0
                }
            },
            |i| {
                months
                    .iter()
                    .map(|&m| year.temperature[i][m] as f64)
                    .sum::<f64>()
                    / 3.0
            },
        )
    };
    let (north_jja, north_djf) = (season(true, [3, 4, 5]), season(true, [9, 10, 11]));
    let (south_jja, south_djf) = (season(false, [3, 4, 5]), season(false, [9, 10, 11]));
    assert!(
        north_jja > north_djf + 8.0,
        "north: Jun–Aug {north_jja:.1} K vs Dec–Feb {north_djf:.1} K"
    );
    assert!(
        south_djf > south_jja + 4.0,
        "south: Dec–Feb {south_djf:.1} K vs Jun–Aug {south_jja:.1} K"
    );
}

#[test]
fn deserts_form_in_the_subtropics() {
    let mut run = Run::new(16, Preset::EarthModern.params(), None, false);
    run.years(6.0);
    let year = run.year();
    let land_precip = |lo: f64, hi: f64| {
        run.mean(
            |c| !run.terrain.is_ocean(c) && (lo..hi).contains(&run.lat(c).abs()),
            |i| year.annual_precipitation(i),
        )
    };
    let tropics = land_precip(0.0, 10.0);
    let subtropics = land_precip(18.0, 32.0);
    let midlatitudes = land_precip(42.0, 60.0);
    assert!(
        subtropics < 0.4 * tropics,
        "subtropics {subtropics:.0} mm vs tropics {tropics:.0} mm"
    );
    assert!(
        subtropics < 0.7 * midlatitudes,
        "subtropics {subtropics:.0} mm vs mid-latitudes {midlatitudes:.0} mm"
    );
    // Deserts are found mostly away from the equator and the poles.
    let desert_lat = run.mean(
        |c| year.zones[c as usize] == ClimateZone::Desert,
        |i| run.grid.latitude(i as u32).to_degrees().abs(),
    );
    assert!(
        (12.0..42.0).contains(&desert_lat),
        "deserts centred at {desert_lat:.1}°"
    );
    assert!(
        run.climate.total_water().is_finite(),
        "water budget must stay finite"
    );
}

#[test]
fn mountains_cast_rain_shadows() {
    // An ocean planet with one north–south ridge across the westerlies.
    let grid = GeodesicGrid::new(32);
    let elevation: Vec<f64> = grid
        .cells()
        .map(|c| {
            let lat = grid.latitude(c).to_degrees();
            let lon = grid.longitude(c).to_degrees();
            if (30.0..60.0).contains(&lat) && lon.abs() < 9.0 {
                200.0 + 4_000.0 * (1.0 - lon.abs() / 9.0)
            } else {
                -4_000.0
            }
        })
        .collect();
    let terrain = Terrain::from_elevation(&grid, elevation, 0.0);
    let mut run = Run::new(32, Preset::EarthModern.params(), Some(terrain), false);
    run.years(2.2);
    let year = run.year();
    let slope = |west: bool| {
        run.mean(
            |c| {
                let lon = run.grid.longitude(c).to_degrees();
                let lat = run.lat(c);
                !run.terrain.is_ocean(c)
                    && (35.0..55.0).contains(&lat)
                    && if west {
                        (-9.0..-1.5).contains(&lon)
                    } else {
                        (1.5..9.0).contains(&lon)
                    }
            },
            |i| year.annual_precipitation(i),
        )
    };
    let (windward, lee) = (slope(true), slope(false));
    assert!(
        windward > 1.5 * lee,
        "windward {windward:.0} mm vs lee {lee:.0} mm"
    );
}

#[test]
fn a_dimmer_sun_triggers_runaway_glaciation() {
    let mut control = Run::new(16, Preset::EarthModern.params(), None, false);
    control.years(8.0);
    let control_ice = control.ice_fraction();
    assert!(
        control_ice < 0.25,
        "today's Earth should not freeze: {:.0}% ice",
        control_ice * 100.0
    );

    let mut dim = Preset::EarthModern.params();
    dim.star.luminosity_factor = 0.88;
    let mut dim = Run::new(16, dim, None, false);
    dim.years(15.0);
    let dim_ice = dim.ice_fraction();
    assert!(
        dim_ice > 0.5,
        "12% less sunlight should start ice-albedo runaway: {:.0}% ice",
        dim_ice * 100.0
    );
}

#[test]
fn a_snowball_stays_frozen() {
    let mut run = Run::new(16, Preset::Snowball.params(), None, true);
    run.years(6.0);
    let ice = run.ice_fraction();
    assert!(ice > 0.8, "snowball thawed: {:.0}% ice", ice * 100.0);
}

#[test]
fn water_is_conserved_and_terraforming_reshapes_climate() {
    let mut run = Run::new(16, Preset::EarthModern.params(), None, false);
    run.years(1.0);
    let w0 = run.climate.total_water();
    run.years(0.5);
    let w1 = run.climate.total_water();
    assert!(
        ((w1 - w0) / w0.abs()).abs() < 1e-9,
        "water drift {}",
        (w1 - w0) / w0
    );

    // Raise a high plateau out of the tropical ocean: it should become dry
    // land that is much colder than the sea around it.
    let target = (0..run.grid.cell_count() as u32)
        .filter(|&c| run.terrain.is_ocean(c) && run.lat(c).abs() < 10.0)
        .min_by(|&a, &b| {
            run.terrain.elevation[a as usize].total_cmp(&run.terrain.elevation[b as usize])
        })
        .unwrap();
    let centre: Vec3 = run.grid.position(target);
    let before = run.terrain.clone();
    run.terrain.apply_brush(
        &run.grid,
        &dirtbox_planet::Brush {
            mode: dirtbox_planet::BrushMode::Raise,
            centre,
            radius: 0.12,
            amount: 10_000.0,
        },
    );
    run.climate
        .terrain_changed(&run.grid, &run.planet, &before, &run.terrain);
    assert!(!run.terrain.is_ocean(target));
    run.years(1.2);
    let peak = run.climate.temperature[target as usize];
    let sea = run.mean(
        |c| run.terrain.is_ocean(c) && run.lat(c).abs() < 10.0,
        |i| run.climate.temperature[i],
    );
    assert!(
        peak < sea - 25.0,
        "plateau {peak:.1} K vs tropical sea {sea:.1} K"
    );
}

#[test]
fn doubling_co2_warms_by_a_realistic_amount() {
    let mean_after = |co2_bar: f64| {
        let mut planet = Preset::EarthModern.params();
        planet.atmosphere.co2 = co2_bar;
        let mut run = Run::new(16, planet, None, false);
        run.years(12.0);
        run.climate.summary(&run.grid, &run.terrain).mean_temperature
    };
    let warming = mean_after(560e-6) - mean_after(280e-6);
    // Observed equilibrium climate sensitivity: likely 2.5–4 K, very likely 2–5 K.
    assert!((2.0..5.0).contains(&warming), "doubling CO₂ warmed by {warming:.2} K");
}
