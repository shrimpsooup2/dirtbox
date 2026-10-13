//! The viewer application: globe, time controls, planet editor, terraforming,
//! world creation and the performance (calibration) window.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Sense};
use egui_glow::glow::{self, HasContext};

use dirtbox_chemistry::{Milestone, Module};
use dirtbox_climate::{GlobalSummary, StepMode};
use dirtbox_core::time::QUANTUM_SECONDS;
use dirtbox_core::{SimTime, Vec3};
use dirtbox_grid::{CellId, GeodesicGrid};
use dirtbox_hw::{BudgetInputs, HardwareInfo, detect, recommend, speed_for_population};
use dirtbox_planet::{Brush, BrushMode, Preset};
use dirtbox_sim::{EvolutionSettings, MutationBoost, World};

use crate::calibration::Calibration;
use crate::camera::{Mat4, OrbitCamera, V3};
use crate::globe::{FrameUniforms, GlobeMesh, GlobeRenderer};
use crate::overlay::{self, Legend, Lifestyle, Overlay, Scale};
use crate::worldgen::{PendingWorld, WorldRecipe};

/// Grid subdivisions offered for new worlds (cell counts 2,562 to 163,842).
const GRID_CHOICES: [u32; 5] = [16, 32, 48, 64, 128];
/// Sim seconds per wall second.
const SPEEDS: [(f64, &str); 7] = [
    (1.0, "1×"),
    (10.0, "10×"),
    (100.0, "100×"),
    (1_000.0, "1k×"),
    (10_000.0, "10k×"),
    (100_000.0, "100k×"),
    (1_000_000.0, "1M×"),
];
/// Planet days per wall second above which the climate is sampled rather
/// than run every day (see `World::set_climate_stride`).
const STRIDE_THRESHOLD_DAYS: f64 = 30.0;
/// Climate stride while fast-forwarding to a milestone.
const FAST_FORWARD_STRIDE: u32 = 10;
/// Longest fast-forward, planet years.
const FAST_FORWARD_LIMIT: f64 = 500.0;
/// Above this many planet seconds per wall second the climate switches to
/// day-long steps (no day/night cycle) to keep up.
const DAILY_MODE_THRESHOLD: f64 = 6.0 * 3600.0;
/// Wall time the simulation may use per frame.
const FRAME_BUDGET: Duration = Duration::from_millis(25);
const BUDGET_KEY: &str = "population_budget";
const BACKGROUND: Color32 = Color32::from_rgb(0x0e, 0x0f, 0x12);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    View,
    Life,
    Planet,
    Terraform,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Raise,
    Lower,
    Smooth,
    Flatten,
}

struct Terraform {
    active: bool,
    tool: Tool,
    radius_km: f64,
    /// Metres per second of brushing (raise/lower), or blend per second
    /// (smooth/flatten).
    strength: f64,
    flatten_target: Option<f64>,
    sea_level: f64,
}

/// A run toward a milestone at full speed.
struct FastForward {
    target: Milestone,
    /// Planet seconds when it started.
    from: f64,
}

/// A loaded world plus everything derived from it for display.
struct Loaded {
    world: World,
    mesh: GlobeMesh,
    values: Vec<f32>,
    legend: Legend,
    colours_dirty: bool,
    positions_dirty: bool,
    last_colouring: Instant,
    summary: GlobalSummary,
    last_summary: Instant,
    /// Exact (unquantised) sim time the world clock is chasing.
    sim_target_seconds: f64,
    /// Sim seconds per wall second actually achieved (smoothed).
    achieved_speed: f64,
}

pub struct DirtboxApp {
    loaded: Option<Loaded>,
    pending: Option<PendingWorld>,
    error: Option<String>,
    renderer: Arc<Mutex<GlobeRenderer>>,
    camera: OrbitCamera,

    tab: Tab,
    overlay: Overlay,
    sunlit: bool,
    borders: bool,
    relief: f64,
    wind_arrows: bool,
    hovered: Option<CellId>,
    selected: Option<CellId>,
    terraform: Terraform,

    paused: bool,
    speed: f64,
    fast_forward: Option<FastForward>,
    fast_forward_choice: Milestone,
    /// A message for the player (e.g. a milestone reached), with when it appeared.
    notice: Option<(String, Instant)>,

    show_new_world: bool,
    recipe: WorldRecipe,

    hw: HardwareInfo,
    gl_info: String,
    calibration: Calibration,
    show_performance: bool,
    population_budget: Option<u64>,
    custom_budget: u64,
}

impl DirtboxApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let gl = cc
            .gl
            .as_ref()
            .ok_or("the viewer needs the OpenGL (glow) backend")?;
        let renderer = GlobeRenderer::new(gl)?;
        let gl_info = unsafe {
            format!(
                "{} (OpenGL {})",
                gl.get_parameter_string(glow::RENDERER),
                gl.get_parameter_string(glow::VERSION)
            )
        };
        let population_budget = cc
            .storage
            .and_then(|s| s.get_string(BUDGET_KEY))
            .and_then(|s| s.parse().ok());
        let recipe = WorldRecipe {
            seed: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_secs() % 100_000),
            ..WorldRecipe::default()
        };
        Ok(Self {
            loaded: None,
            pending: Some(PendingWorld::start(recipe.clone(), cc.egui_ctx.clone())),
            error: None,
            renderer: Arc::new(Mutex::new(renderer)),
            camera: OrbitCamera::default(),
            tab: Tab::View,
            overlay: Overlay::Satellite,
            sunlit: true,
            borders: true,
            relief: 25.0,
            wind_arrows: false,
            hovered: None,
            selected: None,
            terraform: Terraform {
                active: false,
                tool: Tool::Raise,
                radius_km: 400.0,
                strength: 800.0,
                flatten_target: None,
                sea_level: 0.0,
            },
            paused: false,
            speed: 100.0,
            fast_forward: None,
            fast_forward_choice: Milestone::Metabolism(Module::OxygenicPhotosynthesis),
            notice: None,
            show_new_world: false,
            recipe,
            hw: detect(),
            gl_info,
            calibration: Calibration::NotRun,
            show_performance: population_budget.is_none(),
            population_budget,
            custom_budget: population_budget.unwrap_or(20_000),
        })
    }

    fn install(&mut self, world: World) {
        let mesh = GlobeMesh::build(&world.state().grid);
        self.renderer.lock().unwrap().set_mesh(&mesh);
        let sun = world
            .state()
            .planet()
            .sun_state(world.state().planet_seconds())
            .direction;
        self.camera.lon = sun.longitude() as f32;
        self.hovered = None;
        self.selected = None;
        self.fast_forward = None;
        self.terraform.sea_level = world.state().terrain.sea_level();
        self.loaded = Some(Loaded {
            summary: world
                .state()
                .climate
                .summary(&world.state().grid, &world.state().terrain),
            world,
            mesh,
            values: Vec::new(),
            legend: Legend::none(""),
            colours_dirty: true,
            positions_dirty: true,
            last_colouring: Instant::now(),
            last_summary: Instant::now(),
            sim_target_seconds: 0.0,
            achieved_speed: 0.0,
        });
    }

    /// Advances the world toward the speed the player asked for, within a
    /// per-frame time budget so the UI stays responsive.
    fn advance_time(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) && ctx.memory(|m| m.focused().is_none()) {
            self.paused = !self.paused;
        }
        let Some(l) = &mut self.loaded else { return };
        if let Some(ff) = &self.fast_forward {
            // As fast as this computer allows, until the milestone.
            l.world.set_climate_mode(StepMode::DailyMean);
            l.world.set_climate_stride(FAST_FORWARD_STRIDE);
            let state = l.world.state();
            let years_done = (state.planet_seconds() - ff.from) / state.calendar().year_length;
            let before = state.now().seconds();
            let reached = l.world.run_until_milestone(
                ff.target,
                (FAST_FORWARD_LIMIT - years_done).max(0.0),
                Some(FRAME_BUDGET),
            );
            let state = l.world.state();
            let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
            l.achieved_speed =
                0.9 * l.achieved_speed + 0.1 * (state.now().seconds() - before) / dt.max(1e-3);
            l.sim_target_seconds = state.now().seconds();
            let years_done = (state.planet_seconds() - ff.from) / state.calendar().year_length;
            let message = if reached {
                Some(format!(
                    "Reached “{}” after {:.1} planet years.",
                    ff.target.label(),
                    years_done
                ))
            } else if !state.life.is_alive() && state.life.has_reached(Milestone::LifeBegins) {
                Some("Stopped: all life has died out.".to_string())
            } else if years_done >= FAST_FORWARD_LIMIT {
                Some(format!(
                    "Stopped after {FAST_FORWARD_LIMIT:.0} planet years without reaching “{}”.",
                    ff.target.label()
                ))
            } else {
                None
            };
            if let Some(m) = message {
                self.notice = Some((m, Instant::now()));
                self.fast_forward = None;
                self.paused = true;
                l.world.set_climate_stride(1);
            }
            l.colours_dirty = true;
            ctx.request_repaint();
            return;
        }
        if self.paused {
            l.achieved_speed = 0.0;
            return;
        }
        let k = l.world.state().time_model().season_speed;
        let mode = if self.speed * k > DAILY_MODE_THRESHOLD {
            StepMode::DailyMean
        } else {
            StepMode::Diurnal
        };
        l.world.set_climate_mode(mode);
        let days_per_second = self.speed * k / 86_400.0;
        let stride = if mode == StepMode::DailyMean && days_per_second > STRIDE_THRESHOLD_DAYS {
            (days_per_second / STRIDE_THRESHOLD_DAYS)
                .round()
                .clamp(1.0, 10.0) as u32
        } else {
            1
        };
        l.world.set_climate_stride(stride);

        let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
        l.sim_target_seconds += dt * self.speed;
        let target = SimTime::from_quanta((l.sim_target_seconds / QUANTUM_SECONDS).floor() as u64);
        let before = l.world.state().now().seconds();
        let (_, reached) = l.world.run_until_within(target, Some(FRAME_BUDGET));
        if reached < target {
            // Can't keep up: drop the backlog rather than letting it grow.
            l.sim_target_seconds = reached.seconds();
        }
        let speed_now = (reached.seconds() - before) / dt.max(1e-3);
        l.achieved_speed = 0.9 * l.achieved_speed + 0.1 * speed_now;
        ctx.request_repaint();
    }

    /// Refreshes overlay colours and vertex heights when needed.
    fn refresh_display(&mut self) {
        let Some(l) = &mut self.loaded else { return };
        let state = l.world.state();
        let radius = state.planet().body.radius();
        if l.positions_dirty {
            let heights: Vec<f64> = state
                .grid
                .cells()
                .map(|c| state.terrain.height_above_sea(c).max(0.0))
                .collect();
            let positions = l.mesh.positions(&state.grid, &heights, radius, self.relief);
            self.renderer.lock().unwrap().set_positions(positions);
            l.positions_dirty = false;
        }
        let live = self.overlay.is_live()
            && !self.paused
            && l.last_colouring.elapsed() > Duration::from_millis(300);
        if l.colours_dirty || live {
            l.values = self.overlay.values(state, radius / 1000.0);
            let (colours, legend) = self.overlay.colours(state, &l.values);
            l.legend = legend;
            let expanded = l.mesh.expand_colours(&colours);
            self.renderer.lock().unwrap().set_colours(expanded);
            l.colours_dirty = false;
            l.last_colouring = Instant::now();
        }
        if l.last_summary.elapsed() > Duration::from_millis(500) {
            l.summary = state.climate.summary(&state.grid, &state.terrain);
            l.last_summary = Instant::now();
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Dirtbox").strong());
            ui.separator();
            if ui
                .button(if self.paused { "Play" } else { "Pause" })
                .on_hover_text("Space")
                .clicked()
            {
                self.paused = !self.paused;
            }
            for (speed, label) in SPEEDS {
                if ui
                    .selectable_label(self.speed == speed, label)
                    .on_hover_text(format!("{label}: {speed} sim seconds per real second"))
                    .clicked()
                {
                    self.speed = speed;
                }
            }
            if let Some(l) = &self.loaded {
                if !self.paused && l.achieved_speed < 0.8 * self.speed {
                    ui.label(
                        RichText::new(format!("running at {:.0}×", l.achieved_speed))
                            .color(ui.visuals().warn_fg_color),
                    )
                    .on_hover_text(
                        "This computer can't simulate this world any faster. Try a coarser grid.",
                    );
                }
                ui.separator();
                let state = l.world.state();
                ui.label(format!("Sim {}", sim_clock(state.now().seconds())));
                ui.separator();
                ui.label(planet_date(state)).on_hover_text(
                    if state.climate_stride() > 1 {
                        "Fast forward: the climate is simulated one day in every few, so seasons are milder than they should be. Slow down for full detail."
                    } else if state.climate_mode() == StepMode::DailyMean {
                        "At this speed the climate takes day-long steps (no day/night cycle)."
                    } else {
                        "The climate steps hourly, with day and night."
                    },
                );
                if self.fast_forward.is_some() {
                    ui.separator();
                    ui.label(RichText::new("Fast-forwarding…").color(ui.visuals().warn_fg_color));
                    if ui.button("Stop").clicked() {
                        self.fast_forward = None;
                        self.paused = true;
                    }
                }
            }
            if let Some((text, at)) = &self.notice {
                if at.elapsed() < Duration::from_secs(12) {
                    ui.separator();
                    ui.label(RichText::new(format!("★ {text}")).strong());
                } else {
                    self.notice = None;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Performance…").clicked() {
                    self.show_performance = !self.show_performance;
                }
                if ui.button("New world…").clicked() {
                    self.show_new_world = true;
                }
            });
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (tab, label) in [
                (Tab::View, "View"),
                (Tab::Life, "Life"),
                (Tab::Planet, "Planet"),
                (Tab::Terraform, "Terraform"),
                (Tab::Settings, "Settings"),
            ] {
                ui.selectable_value(&mut self.tab, tab, label);
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
            Tab::View => self.view_tab(ui),
            Tab::Life => self.life_tab(ui),
            Tab::Planet => self.planet_tab(ui),
            Tab::Terraform => self.terraform_tab(ui),
            Tab::Settings => self.settings_tab(ui),
        });
    }

    fn view_tab(&mut self, ui: &mut egui::Ui) {
        section(ui, "Overlay");
        let before = self.overlay;
        egui::ComboBox::from_id_salt("overlay")
            .selected_text(self.overlay.label())
            .width(220.0)
            .height(520.0)
            .show_ui(ui, |ui| {
                for o in Overlay::ALL {
                    ui.selectable_value(&mut self.overlay, o, o.label());
                }
            });
        if let Some(l) = &mut self.loaded {
            if self.overlay != before {
                l.colours_dirty = true;
            }
            ui.add_space(4.0);
            legend_widget(ui, &l.legend);
        }

        ui.add_space(8.0);
        section(ui, "Display");
        ui.checkbox(&mut self.sunlit, "Day and night lighting");
        ui.checkbox(&mut self.borders, "Cell borders");
        ui.checkbox(&mut self.wind_arrows, "Wind arrows");
        if ui
            .add(
                egui::Slider::new(&mut self.relief, 0.0..=80.0)
                    .text("relief exaggeration")
                    .suffix("×"),
            )
            .changed()
            && let Some(l) = &mut self.loaded
        {
            l.positions_dirty = true;
        }

        if let Some(l) = &self.loaded {
            ui.add_space(8.0);
            section(ui, "Planet now");
            let s = &l.summary;
            let state = l.world.state();
            egui::Grid::new("summary")
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    row(
                        ui,
                        "Mean temperature",
                        format!("{:.1} °C", s.mean_temperature - 273.15),
                    );
                    row(
                        ui,
                        "Range",
                        format!(
                            "{:.0} to {:.0} °C",
                            s.min_temperature - 273.15,
                            s.max_temperature - 273.15
                        ),
                    );
                    row(
                        ui,
                        "Ice and snow cover",
                        format!("{:.1}%", s.ice_fraction * 100.0),
                    );
                    row(
                        ui,
                        "Cloud cover",
                        format!("{:.0}%", s.cloud_fraction * 100.0),
                    );
                    row(
                        ui,
                        "Rain and snow",
                        format!("{:.2} mm/day", s.precipitation),
                    );
                    row(ui, "Planet albedo", format!("{:.2}", s.planetary_albedo));
                    row(
                        ui,
                        "Ocean",
                        format!(
                            "{:.0}% of surface",
                            state.terrain.ocean_fraction(&state.grid) * 100.0
                        ),
                    );
                    let a = &state.planet().atmosphere;
                    row(
                        ui,
                        "CO₂",
                        format!("{} ppm", overlay::group(a.ppm(a.co2) as f32)),
                    );
                    row(
                        ui,
                        "Methane",
                        format!("{} ppm", overlay::significant(a.ppm(a.ch4) as f32)),
                    );
                    row(
                        ui,
                        "Oxygen",
                        format!("{} ppm", overlay::significant(a.ppm(a.o2) as f32)),
                    );
                });
        }
    }

    fn life_tab(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &mut self.loaded else {
            ui.label("No world yet.");
            return;
        };
        let started = l.world.state().life.has_reached(Milestone::LifeBegins);
        let alive = l.world.state().life.is_alive();
        if !alive {
            ui.label(if started {
                "All life has died out."
            } else {
                "No life yet."
            });
            ui.label(
                RichText::new("Base bacteria are simple fermenters that like warm water. They start at the strongest hydrothermal vents and have to evolve everything else. Oxygen is poison to them.")
                    .weak()
                    .small(),
            );
            if ui.button("Seed base bacteria").clicked() {
                let cells = l.world.seed_life();
                self.notice = Some((
                    format!("Seeded base bacteria at {} vents.", cells.len()),
                    Instant::now(),
                ));
                l.colours_dirty = true;
            }
            if !started {
                return;
            }
        }
        let state = l.world.state();
        let life = &state.life;
        let st = life.stats();
        let cal = state.calendar();
        let year_of = |t: f64| t / cal.year_length;
        let accel = state.time_model().geologic_acceleration;
        let first = life.events().first().map_or(0.0, |e| e.time);
        let gt = |mol: f64| mol * 12.0 / 1e15;

        section(ui, "Biosphere");
        egui::Grid::new("life")
            .num_columns(2)
            .spacing([12.0, 2.0])
            .show(ui, |ui| {
                row(
                    ui,
                    "Living carbon",
                    format!("{} Gt C", overlay::significant(gt(st.biomass) as f32)),
                );
                if st.biomass > 0.0 {
                    row(
                        ui,
                        "In the sea / on land",
                        format!(
                            "{:.0}% / {:.0}%",
                            100.0 * st.ocean_biomass / st.biomass,
                            100.0 * st.land_biomass / st.biomass
                        ),
                    );
                }
                row(
                    ui,
                    "Cells with life",
                    format!(
                        "{} ({:.0}%)",
                        overlay::group(st.occupied_cells as f32),
                        100.0 * st.occupied_cells as f64 / state.grid.cell_count() as f64
                    ),
                );
                row(
                    ui,
                    "Strains",
                    format!(
                        "{} alive · {} ever",
                        overlay::group(st.strains as f32),
                        overlay::group(life.registry().total_ever() as f32)
                    ),
                );
                row(
                    ui,
                    "Carbon fixed",
                    format!(
                        "{} Gt C/yr",
                        overlay::significant(gt(st.productivity) as f32)
                    ),
                );
                row(
                    ui,
                    "Food in the water",
                    format!("{} Gt C", overlay::significant(gt(st.organics) as f32)),
                );
                row(
                    ui,
                    "Buried carbon",
                    format!("{} Gt C", overlay::significant(gt(st.buried) as f32)),
                );
                let years = year_of(state.planet_seconds() - first);
                row(
                    ui,
                    "Evolution so far",
                    format!(
                        "{:.1} planet years ≈ {} kyr",
                        years,
                        overlay::group((years * accel / 1e3) as f32)
                    ),
                );
            });
        ui.label(
            RichText::new("Microbes evolve on the geologic clock, so each planet year stands for many thousands of years of evolution.")
                .weak()
                .small(),
        );

        ui.add_space(8.0);
        section(ui, "Metabolisms");
        egui::Grid::new("modules")
            .num_columns(3)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                for m in Module::ALL {
                    let share = if st.biomass > 0.0 {
                        st.module_biomass[m.index()] / st.biomass
                    } else {
                        0.0
                    };
                    let established = life
                        .events()
                        .iter()
                        .find(|e| e.milestone == Milestone::Metabolism(m))
                        .map(|e| year_of(e.time));
                    let text = RichText::new(m.label()).small();
                    ui.label(if share > 0.0 { text } else { text.weak() });
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(70.0, 8.0), Sense::hover());
                    ui.painter().rect_filled(rect, 1.0, Color32::from_gray(45));
                    let mut fill = rect;
                    fill.set_width(rect.width() * share.clamp(0.0, 1.0) as f32);
                    let [r, g, b] = overlay::sequential(0.7);
                    ui.painter()
                        .rect_filled(fill, 1.0, Color32::from_rgb(r, g, b));
                    ui.label(
                        RichText::new(match (m, established) {
                            (Module::Fermentation, _) if started => "from the start".to_string(),
                            (_, Some(y)) => format!("year {y:.0}"),
                            (_, None) if share > 0.0 => format!("{:.1}%", share * 100.0),
                            _ => "—".to_string(),
                        })
                        .weak()
                        .small(),
                    );
                    ui.end_row();
                }
            });
        ui.label(
            RichText::new("Bars: share of living carbon in strains that carry each pathway.")
                .weak()
                .small(),
        );

        ui.add_space(8.0);
        section(ui, "Fast forward");
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("milestone")
                .selected_text(self.fast_forward_choice.label())
                .width(190.0)
                .show_ui(ui, |ui| {
                    for m in Milestone::targets() {
                        let done = life.has_reached(m);
                        ui.add_enabled_ui(!done, |ui| {
                            ui.selectable_value(&mut self.fast_forward_choice, m, m.label());
                        });
                    }
                });
            let target = self.fast_forward_choice;
            let can = alive && !life.has_reached(target) && self.fast_forward.is_none();
            if ui.add_enabled(can, egui::Button::new("Go")).clicked() {
                self.fast_forward = Some(FastForward {
                    target,
                    from: state.planet_seconds(),
                });
                self.paused = false;
            }
        });
        ui.label(
            RichText::new(format!(
                "Runs as fast as this computer can (climate sampled every {FAST_FORWARD_STRIDE} days) until it happens, for up to {FAST_FORWARD_LIMIT:.0} planet years. It may never happen."
            ))
            .weak()
            .small(),
        );

        ui.add_space(8.0);
        section(ui, "Most common strains");
        let mut top: Vec<_> = life
            .registry()
            .iter()
            .filter(|(_, r)| r.biomass > 0.0)
            .collect();
        top.sort_by(|a, b| b.1.biomass.total_cmp(&a.1.biomass));
        for (_, r) in top.iter().take(6) {
            let share = r.biomass / st.biomass.max(1e-30);
            ui.horizontal(|ui| {
                let [cr, cg, cb] = Lifestyle::of(&r.genome).colour();
                let (sw, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                ui.painter()
                    .rect_filled(sw, 2.0, Color32::from_rgb(cr, cg, cb));
                ui.label(RichText::new(format!("#{} {:.0}%", r.serial, share * 100.0)).small())
                    .on_hover_text(strain_details(&r.genome, r.parent, year_of(r.born)));
                ui.label(
                    RichText::new(Lifestyle::of(&r.genome).label())
                        .weak()
                        .small(),
                );
            });
        }

        ui.add_space(8.0);
        section(ui, "Events");
        for e in life.events().iter().rev().take(30) {
            ui.label(
                RichText::new(format!(
                    "Year {:.1}: {}",
                    year_of(e.time),
                    e.milestone.describe()
                ))
                .small(),
            );
        }
    }

    fn planet_tab(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &mut self.loaded else {
            ui.label("No world yet.");
            return;
        };
        ui.label(
            RichText::new("Changes apply live. The climate takes a few planet years to settle into its new balance.")
                .weak()
                .small(),
        );
        let mut planet = l.world.state().planet().clone();
        let earth_flux = Preset::EarthModern.params().mean_flux();
        if crate::planet_editor::planet_editor(ui, &mut planet, earth_flux) {
            match l.world.set_planet(planet) {
                Ok(()) => {
                    self.error = None;
                    l.positions_dirty = true;
                    l.colours_dirty = true;
                }
                Err(e) => self.error = Some(e),
            }
        }
        ui.add_space(8.0);
        section(ui, "Time");
        let mut k = l.world.state().time_model().season_speed;
        if ui
            .add(
                egui::Slider::new(&mut k, 1.0..=5_000.0)
                    .logarithmic(true)
                    .text("season speed"),
            )
            .on_hover_text(
                "How fast days and seasons pass compared with creatures' lives. \
                 1 = fully realistic. Creatures' bodies always run in real time.",
            )
            .changed()
            && let Err(e) = l.world.set_season_speed(k)
        {
            self.error = Some(e);
        }
        let cal = l.world.state().calendar();
        ui.label(
            RichText::new(format!(
                "1 planet day = {} · 1 planet year = {} of sim time",
                duration(cal.day_length / k),
                duration(cal.year_length / k)
            ))
            .weak()
            .small(),
        );
        ui.add_space(8.0);
        section(ui, "Geology");
        let params = &mut l.world.state_mut().climate.params;
        ui.checkbox(&mut params.carbon_cycle, "Carbon–silicate cycle").on_hover_text(
            "Volcanoes add CO₂; rain and warmth weather rock and remove it. Over geological time this \
             acts as a thermostat. Off by default because it slowly changes the atmosphere you set.",
        );
    }

    fn terraform_tab(&mut self, ui: &mut egui::Ui) {
        let t = &mut self.terraform;
        ui.checkbox(&mut t.active, RichText::new("Brush active").strong());
        ui.label(
            RichText::new("While active: left-drag paints, right-drag rotates, scroll zooms.")
                .weak()
                .small(),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (tool, label, help) in [
                (
                    Tool::Raise,
                    "Raise",
                    "Push the ground up: mountains, islands, continents.",
                ),
                (
                    Tool::Lower,
                    "Lower",
                    "Dig down: valleys, seas, ocean basins.",
                ),
                (Tool::Smooth, "Smooth", "Even out bumps."),
                (
                    Tool::Flatten,
                    "Flatten",
                    "Level to the height where the stroke starts.",
                ),
            ] {
                ui.selectable_value(&mut t.tool, tool, label)
                    .on_hover_text(help);
            }
        });
        ui.add(
            egui::Slider::new(&mut t.radius_km, 50.0..=3_000.0)
                .logarithmic(true)
                .text("radius")
                .suffix(" km"),
        );
        match t.tool {
            Tool::Raise | Tool::Lower => {
                ui.add(
                    egui::Slider::new(&mut t.strength, 50.0..=5_000.0)
                        .logarithmic(true)
                        .text("strength")
                        .suffix(" m/s"),
                );
            }
            Tool::Smooth | Tool::Flatten => {
                let mut rate = (t.strength / 5_000.0).clamp(0.01, 1.0);
                if ui
                    .add(egui::Slider::new(&mut rate, 0.01..=1.0).text("strength"))
                    .changed()
                {
                    t.strength = rate * 5_000.0;
                }
            }
        }
        ui.add_space(10.0);
        section(ui, "Sea level");
        let mut level = t.sea_level;
        let response = ui.add(
            egui::Slider::new(&mut level, -6_000.0..=6_000.0)
                .text("sea level")
                .suffix(" m")
                .step_by(10.0),
        );
        t.sea_level = level;
        if (response.drag_stopped() || (response.changed() && !response.dragged()))
            && let Some(l) = &mut self.loaded
        {
            l.world.set_sea_level(level);
            l.positions_dirty = true;
            l.colours_dirty = true;
        }
        ui.label(
            RichText::new(
                "Applies when you let go. Raising it floods the land; lowering it drains the seas.",
            )
            .weak()
            .small(),
        );
        if let Some(l) = &self.loaded {
            let state = l.world.state();
            ui.label(
                RichText::new(format!(
                    "Ocean covers {:.1}% of the surface.",
                    state.terrain.ocean_fraction(&state.grid) * 100.0
                ))
                .weak()
                .small(),
            );
        }
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &mut self.loaded else { return };
        section(ui, "World");
        let state = l.world.state();
        ui.label(format!(
            "Seed {} · {} cells (~{:.0} km apart)",
            state.seed(),
            overlay::group(state.grid.cell_count() as f32),
            cell_spacing_km(&state.grid, state.planet().body.radius_km)
        ));
        ui.label(
            RichText::new("Use “New world…” to change the preset, seed or grid.")
                .weak()
                .small(),
        );

        ui.add_space(8.0);
        section(ui, "Evolution");
        let mut evo = l.world.state().evolution().clone();
        let mut changed = false;
        changed |= ui
            .add(
                egui::Slider::new(
                    &mut evo.mutation_multiplier,
                    0.0..=EvolutionSettings::MAX_MULTIPLIER,
                )
                .logarithmic(true)
                .smallest_positive(0.1)
                .suffix("×")
                .text("mutation rate"),
            )
            .on_hover_text("Multiplies small gene changes. 1× is natural; 0 freezes evolution.")
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(
                    &mut evo.structural_multiplier,
                    0.0..=EvolutionSettings::MAX_MULTIPLIER,
                )
                .logarithmic(true)
                .smallest_positive(0.1)
                .suffix("×")
                .text("structural"),
            )
            .on_hover_text(
                "Multiplies new, lost or duplicated body parts, neurons and metabolic modules.",
            )
            .changed();
        ui.horizontal(|ui| {
            changed |= ui
                .radio_value(
                    &mut evo.boost,
                    MutationBoost::Environmental,
                    "Environmental",
                )
                .on_hover_text(
                    "Acts like radiation: lineages can evolve better DNA repair to resist it.",
                )
                .changed();
            changed |= ui
                .radio_value(&mut evo.boost, MutationBoost::Fixed, "Fixed")
                .on_hover_text("Applies after DNA repair, so it can't be evolved away.")
                .changed();
        });
        if changed && let Err(e) = l.world.set_evolution(evo) {
            self.error = Some(e);
        }
        ui.label(
            RichText::new("Microbes use these now; DNA repair, which the environmental boost can be evolved against, arrives with multicellular life.")
                .weak()
                .small(),
        );
    }

    fn globe(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let aspect = rect.width() / rect.height().max(1.0);
        let dt = ui.input(|i| i.stable_dt);

        let painting_tool = self.terraform.active && self.loaded.is_some();
        let rotate = if painting_tool {
            response.dragged_by(egui::PointerButton::Secondary)
                || response.dragged_by(egui::PointerButton::Middle)
        } else {
            response.dragged()
        };
        if rotate {
            let d = response.drag_delta();
            self.camera.drag(d.x, d.y, rect.height());
        }
        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let amount = scroll + zoom.ln() * 400.0;
            if amount != 0.0 {
                self.camera.zoom(amount);
            }
        }
        if self.camera.update(dt) {
            ui.ctx().request_repaint();
        }

        if self.loaded.is_none() {
            self.loading_screen(ui, rect);
            return;
        }
        let l = self.loaded.as_mut().expect("checked above");

        let picked = response.hover_pos().and_then(|pos| {
            let ndc_x = (pos.x - rect.left()) / rect.width() * 2.0 - 1.0;
            let ndc_y = 1.0 - (pos.y - rect.top()) / rect.height() * 2.0;
            self.camera.pick(ndc_x, ndc_y, aspect)
        });
        let grid = &l.world.state().grid;
        self.hovered = picked.map(|(x, y, z)| {
            let v = Vec3::new(x as f64, y as f64, z as f64);
            match self.hovered {
                Some(hint) if (hint as usize) < grid.cell_count() => grid.locate_from(hint, v),
                _ => grid.locate(v),
            }
        });
        if response.clicked() && !painting_tool {
            self.selected = self.hovered;
        }

        // Terraforming.
        let radius_km = l.world.state().planet().body.radius_km;
        let brush_radius = (self.terraform.radius_km / radius_km) as f32;
        let painting = painting_tool && response.dragged_by(egui::PointerButton::Primary);
        if painting_tool && !painting {
            self.terraform.flatten_target = None;
        }
        if painting && let Some((x, y, z)) = picked {
            let centre = Vec3::new(x as f64, y as f64, z as f64);
            let t = &mut self.terraform;
            if t.flatten_target.is_none() {
                let c = l.world.state().grid.locate(centre);
                t.flatten_target = Some(l.world.state().terrain.elevation[c as usize]);
            }
            let dt = dt as f64;
            let blend = (t.strength / 5_000.0 * dt * 4.0).clamp(0.0, 1.0);
            let (mode, amount) = match t.tool {
                Tool::Raise => (BrushMode::Raise, t.strength * dt),
                Tool::Lower => (BrushMode::Lower, t.strength * dt),
                Tool::Smooth => (BrushMode::Smooth, blend),
                Tool::Flatten => (
                    BrushMode::Flatten {
                        target: t.flatten_target.unwrap_or(0.0),
                    },
                    blend,
                ),
            };
            let changed = l.world.apply_brush(&Brush {
                mode,
                centre,
                radius: brush_radius as f64,
                amount,
            });
            if !changed.is_empty() {
                l.positions_dirty = true;
                l.colours_dirty = true;
            }
            ui.ctx().request_repaint();
        }

        let state = l.world.state();
        let sun = state.planet().sun_state(state.planet_seconds()).direction;
        let view_proj = self.camera.view_proj(aspect);
        let uniforms = FrameUniforms {
            view_proj,
            eye: self.camera.eye(),
            sun: (sun.x as f32, sun.y as f32, sun.z as f32),
            sunlit: self.sunlit,
            // Data overlays stay readable on the night side; the satellite
            // view shows a real night.
            night: if self.overlay == Overlay::Satellite {
                0.3
            } else {
                0.75
            },
            borders: if self.borders {
                let cells_across = (state.grid.cell_count() as f32).sqrt();
                let px_per_cell =
                    rect.height() / cells_across * (2.2 / (self.camera.altitude() + 0.2));
                ((px_per_cell - 10.0) / 12.0).clamp(0.0, 1.0)
            } else {
                0.0
            },
            relief: if self.relief > 0.0 { 1.0 } else { 0.0 },
            hovered: if painting_tool {
                None
            } else {
                self.hovered.or(self.selected)
            },
            brush: if painting_tool {
                picked.map(|p| (p, brush_radius))
            } else {
                None
            },
        };
        let renderer = self.renderer.clone();
        let callback = egui_glow::CallbackFn::new(move |info, painter| {
            let vp = info.viewport_in_pixels();
            renderer.lock().unwrap().paint(
                painter.gl(),
                &uniforms,
                [vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px],
            );
        });
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        });

        if self.wind_arrows || self.overlay == Overlay::Wind {
            draw_wind_arrows(ui, rect, &view_proj, self.camera.eye(), state);
        }

        let painter = ui.painter();
        let weak = ui.visuals().weak_text_color();
        painter.text(
            rect.left_bottom() + egui::vec2(12.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            if painting_tool {
                "Left-drag to terraform · right-drag to rotate · scroll to zoom"
            } else {
                "Drag to rotate · scroll to zoom · click a cell to select"
            },
            egui::FontId::proportional(12.0),
            weak,
        );
        painter.text(
            rect.right_bottom() + egui::vec2(-12.0, -10.0),
            egui::Align2::RIGHT_BOTTOM,
            format!(
                "Altitude {} km",
                overlay::group(self.camera.altitude() * radius_km as f32)
            ),
            egui::FontId::proportional(12.0),
            weak,
        );

        if let Some(cell) = self.hovered.or(self.selected) {
            let pinned = self.hovered.is_none();
            let overlay = self.overlay;
            egui::Area::new(egui::Id::new("inspector"))
                .fixed_pos(rect.right_top() + egui::vec2(-12.0, 12.0))
                .pivot(egui::Align2::RIGHT_TOP)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(260.0);
                        cell_inspector(ui, l, overlay, cell, pinned);
                    });
                });
        }
    }

    fn loading_screen(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let painter = ui.painter();
        let (text, progress) = match &self.pending {
            Some(p) => (
                format!("Creating {}… settling the climate", p.recipe.preset.name()),
                p.progress(),
            ),
            None => ("No world. Use “New world…” to make one.".to_string(), 0.0),
        };
        painter.text(
            rect.center() - egui::vec2(0.0, 18.0),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(18.0),
            ui.visuals().text_color(),
        );
        if self.pending.is_some() {
            let bar = egui::Rect::from_center_size(
                rect.center() + egui::vec2(0.0, 14.0),
                egui::vec2(320.0, 6.0),
            );
            painter.rect_filled(bar, 3.0, ui.visuals().extreme_bg_color);
            let mut done = bar;
            done.set_width(bar.width() * progress.clamp(0.02, 1.0) as f32);
            painter.rect_filled(done, 3.0, ui.visuals().selection.bg_fill);
        }
    }

    fn new_world_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_new_world;
        let mut create = false;
        egui::Window::new("New world")
            .open(&mut open)
            .resizable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                let r = &mut self.recipe;
                egui::Grid::new("recipe").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Start from");
                    egui::ComboBox::from_id_salt("preset")
                        .selected_text(r.preset.name())
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            for p in Preset::ALL {
                                ui.selectable_value(&mut r.preset, p, p.name()).on_hover_text(p.description());
                            }
                        });
                    ui.end_row();
                    ui.label("");
                    ui.add(egui::Label::new(RichText::new(r.preset.description()).weak().small()).wrap());
                    ui.end_row();
                    ui.label("Terrain seed");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut r.seed).range(0..=u32::MAX as u64));
                        if ui.button("Random").clicked() {
                            r.seed = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(7, |d| d.subsec_nanos() as u64 % 100_000);
                        }
                    });
                    ui.end_row();
                    ui.label("Grid");
                    egui::ComboBox::from_id_salt("grid")
                        .selected_text(format!(
                            "{} cells",
                            overlay::group(GeodesicGrid::cell_count_for(r.subdivision) as f32)
                        ))
                        .show_ui(ui, |ui| {
                            for n in GRID_CHOICES {
                                let cells = GeodesicGrid::cell_count_for(n);
                                let note = match n {
                                    16 => " · fastest",
                                    32 => " · recommended",
                                    128 => " · slow climate",
                                    _ => "",
                                };
                                ui.selectable_value(
                                    &mut r.subdivision,
                                    n,
                                    format!("{} cells{note}", overlay::group(cells as f32)),
                                );
                            }
                        });
                    ui.end_row();
                    ui.label("Climate spin-up");
                    ui.add(egui::Slider::new(&mut r.spin_up_years, 0..=20).suffix(" planet years"))
                        .on_hover_text("Simulated before the world appears, so it starts close to its natural balance.");
                    ui.end_row();
                    ui.label("Life");
                    ui.checkbox(&mut r.with_life, "Start with base bacteria")
                        .on_hover_text("Seeds simple fermenters at the strongest hydrothermal vents. You can also seed them later from the Life tab.");
                    ui.end_row();
                });
                ui.add_space(6.0);
                create = ui.add_enabled(self.pending.is_none(), egui::Button::new("Create world")).clicked();
            });
        if create {
            self.pending = Some(PendingWorld::start(self.recipe.clone(), ctx.clone()));
            open = false;
        }
        self.show_new_world = open;
    }

    fn performance_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_performance;
        egui::Window::new("Performance")
            .open(&mut open)
            .resizable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(
                    "Dirtbox measures how fast this computer can step organisms and recommends \
                     a population budget. The budget sets the size of the living world; ecology \
                     decides how many creatures actually live in it.",
                );
                ui.add_space(6.0);
                let ram = |b: Option<u64>| {
                    b.map_or("unknown".to_string(), |b| {
                        format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64)
                    })
                };
                egui::Grid::new("hw")
                    .num_columns(2)
                    .spacing([12.0, 2.0])
                    .show(ui, |ui| {
                        row(
                            ui,
                            "CPU",
                            format!(
                                "{} ({} logical cores)",
                                self.hw.cpu_model.as_deref().unwrap_or("unknown model"),
                                self.hw.logical_cores
                            ),
                        );
                        row(
                            ui,
                            "Memory",
                            format!(
                                "{} total, {} available",
                                ram(self.hw.total_ram_bytes),
                                ram(self.hw.available_ram_bytes)
                            ),
                        );
                        row(ui, "Graphics", self.gl_info.clone());
                    });
                ui.add_space(6.0);
                match &self.calibration {
                    Calibration::NotRun => {
                        if ui.button("Run calibration (about 4 seconds)").clicked() {
                            self.calibration = Calibration::start(&self.hw, ui.ctx().clone());
                        }
                    }
                    Calibration::Running(_) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Measuring… the simulation keeps running meanwhile.");
                        });
                    }
                    Calibration::Done(result) => {
                        let throughput = result.all.steps_per_second();
                        ui.label(format!(
                            "Measured {:.2} M organism-steps/s on 1 thread, {:.2} M on {}.",
                            result.single.steps_per_second() / 1e6,
                            throughput / 1e6,
                            result.all.threads
                        ));
                        let year_sim_seconds = self.loaded.as_ref().map_or(438_000.0, |l| {
                            let s = l.world.state();
                            s.calendar().year_length / s.time_model().season_speed
                        });
                        let inputs = BudgetInputs {
                            throughput,
                            mean_step_seconds: BudgetInputs::DEFAULT_MEAN_STEP_SECONDS,
                            headroom: BudgetInputs::DEFAULT_HEADROOM,
                            bytes_per_organism: BudgetInputs::DEFAULT_BYTES_PER_ORGANISM,
                            available_ram_bytes: self.hw.available_ram_bytes,
                            planet_year_sim_seconds: year_sim_seconds,
                        };
                        ui.add_space(4.0);
                        let mut choice = self.population_budget;
                        egui::Grid::new("tiers")
                            .num_columns(5)
                            .striped(true)
                            .spacing([14.0, 4.0])
                            .show(ui, |ui| {
                                for h in ["", "Tier", "Organisms", "Speed", "Planet year takes"] {
                                    ui.label(RichText::new(h).strong());
                                }
                                ui.end_row();
                                for r in recommend(&inputs) {
                                    ui.radio_value(&mut choice, Some(r.population), "");
                                    ui.label(r.tier.name).on_hover_text(r.tier.purpose);
                                    ui.label(overlay::group(r.population as f32)).on_hover_text(
                                        if r.ram_limited {
                                            "Limited by available memory"
                                        } else {
                                            "Limited by CPU speed"
                                        },
                                    );
                                    ui.label(format!("{}×", r.tier.speed));
                                    ui.label(duration(r.wall_seconds_per_planet_year));
                                    ui.end_row();
                                }
                            });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label("Custom:");
                            ui.add(
                                egui::DragValue::new(&mut self.custom_budget)
                                    .range(100..=10_000_000)
                                    .speed(100.0),
                            );
                            if ui.button("Use").clicked() {
                                choice = Some(self.custom_budget);
                            }
                            let (speed, over_ram) =
                                speed_for_population(&inputs, self.custom_budget);
                            ui.label(
                                RichText::new(format!(
                                    "≈ {speed:.0}× speed{}",
                                    if over_ram {
                                        ", over the memory budget"
                                    } else {
                                        ""
                                    }
                                ))
                                .weak(),
                            );
                        });
                        self.population_budget = choice;
                        ui.label(
                            RichText::new(
                                "Speed is sim seconds per real second at the current season speed. \
                                 Figures come from a stand-in workload until real organisms exist.",
                            )
                            .weak()
                            .small(),
                        );
                        if ui.button("Measure again").clicked() {
                            self.calibration = Calibration::start(&self.hw, ui.ctx().clone());
                        }
                    }
                }
                if let Some(n) = self.population_budget {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!(
                            "Current budget: {} organisms",
                            overlay::group(n as f32)
                        ))
                        .strong(),
                    );
                }
            });
        self.show_performance = open;
    }
}

impl eframe::App for DirtboxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.calibration.poll();
        if self.calibration.is_running() {
            ctx.request_repaint();
        }
        if let Some(p) = &self.pending {
            match p.poll() {
                Some(Ok(world)) => {
                    self.pending = None;
                    self.install(world);
                }
                Some(Err(e)) => {
                    self.pending = None;
                    self.error = Some(e);
                }
                None => ctx.request_repaint_after(Duration::from_millis(100)),
            }
        }
        self.advance_time(&ctx);
        self.refresh_display();

        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("side")
            .resizable(false)
            .exact_size(320.0)
            .show(ui, |ui| {
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                }
                self.side_panel(ui);
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND))
            .show(ui, |ui| self.globe(ui));
        self.new_world_window(&ctx);
        self.performance_window(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(n) = self.population_budget {
            storage.set_string(BUDGET_KEY, n.to_string());
        }
    }

    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        if let Some(gl) = gl {
            self.renderer.lock().unwrap().destroy(gl);
        }
    }
}

fn cell_inspector(ui: &mut egui::Ui, l: &Loaded, overlay: Overlay, cell: CellId, pinned: bool) {
    let state = l.world.state();
    let i = cell as usize;
    let c = &state.climate;
    let t = &state.terrain;
    let p = state.grid.position(cell);
    let ocean = t.is_ocean(cell);
    let surface = if ocean {
        if c.is_ice_covered(cell) {
            "Sea ice"
        } else {
            "Ocean"
        }
    } else if c.lake[i] {
        "Lake"
    } else if c.snow[i] > 100.0 {
        "Snow-covered land"
    } else {
        "Land"
    };
    ui.label(
        RichText::new(format!(
            "{surface}{}",
            if pinned { " (selected)" } else { "" }
        ))
        .strong(),
    );
    egui::Grid::new("cell")
        .num_columns(2)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            row(ui, "Position", lat_lon(p));
            let h = t.height_above_sea(cell);
            row(
                ui,
                if h < 0.0 { "Depth" } else { "Height" },
                format!("{} m", overlay::group(h.abs() as f32)),
            );
            row(
                ui,
                "Temperature",
                format!("{:.1} °C", c.temperature[i] - 273.15),
            );
            row(
                ui,
                "Rain and snow",
                format!("{:.1} mm/day", c.precipitation[i] * 86_400.0),
            );
            if let Some(year) = &c.stats.last_year {
                row(
                    ui,
                    "Per year",
                    format!("{} mm", overlay::group(year.annual_precipitation(i) as f32)),
                );
                row(ui, "Climate", year.zones[i].label().to_string());
            }
            row(
                ui,
                "Wind",
                format!(
                    "{:.1} m/s from {}",
                    c.wind[i].length(),
                    overlay::wind_from(&state.grid, cell, c.wind[i])
                ),
            );
            row(ui, "Cloud", format!("{:.0}%", c.cloud[i] * 100.0));
            row(ui, "Sunlight", format!("{:.0} W/m²", c.insolation[i]));
            if ocean && c.sea_ice[i] > 0.01 {
                row(ui, "Sea ice", format!("{:.2} m", c.sea_ice[i]));
            }
            if !ocean {
                if c.snow[i] > 1.0 {
                    row(ui, "Snow", format!("{:.0} cm water", c.snow[i] / 10.0));
                }
                row(
                    ui,
                    "Soil water",
                    format!("{:.0}%", c.soil[i] / c.params.soil_capacity * 100.0),
                );
                if c.river_flow[i] > 1.0 {
                    row(
                        ui,
                        "River",
                        format!("{} m³/s", overlay::group(c.river_flow[i] as f32)),
                    );
                }
            }
            if t.vents[i] > 0.0 {
                row(
                    ui,
                    "Hydrothermal vents",
                    format!("{:.0}% strength", t.vents[i] * 100.0),
                );
            }
            let life = &state.life;
            if ocean {
                row(
                    ui,
                    "Organic food",
                    format!(
                        "{} mol C/m²",
                        overlay::significant(
                            life.pool(cell, dirtbox_chemistry::pool::ORGANICS) as f32
                        )
                    ),
                );
                row(
                    ui,
                    "Dissolved O₂",
                    format!(
                        "{}% of saturation",
                        overlay::significant(
                            (100.0 * life.pool(cell, dirtbox_chemistry::pool::O2)
                                / (0.25 * life.params.mixed_layer))
                                as f32
                        )
                    ),
                );
            }
            if let Some(text) = l.values.get(i).and_then(|&v| overlay::describe(overlay, v))
                && !matches!(
                    overlay,
                    Overlay::Temperature
                        | Overlay::Precipitation
                        | Overlay::Clouds
                        | Overlay::Sunlight
                        | Overlay::Elevation
                )
            {
                row(ui, overlay.label(), text);
            }
        });
    let life = &state.life;
    let strains = life.strains_in(cell);
    if !strains.is_empty() {
        let total: f64 = strains.iter().map(|s| s.1).sum();
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "Life: {} g C/m² in {} strain{}",
                overlay::significant((total * 12.0) as f32),
                strains.len(),
                if strains.len() == 1 { "" } else { "s" }
            ))
            .strong(),
        );
        let cal = state.calendar();
        for (id, b) in strains {
            let Some(r) = life.strain(id) else { continue };
            ui.horizontal(|ui| {
                let [cr, cg, cb] = Lifestyle::of(&r.genome).colour();
                let (sw, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                ui.painter()
                    .rect_filled(sw, 2.0, Color32::from_rgb(cr, cg, cb));
                ui.label(RichText::new(format!("#{} {:.0}%", r.serial, 100.0 * b / total)).small())
                    .on_hover_text(strain_details(
                        &r.genome,
                        r.parent,
                        r.born / cal.year_length,
                    ));
                ui.label(
                    RichText::new(Lifestyle::of(&r.genome).label())
                        .weak()
                        .small(),
                );
            });
        }
    }
}

/// Hover text describing a strain's genome.
fn strain_details(g: &dirtbox_chemistry::Genome, parent: Option<u64>, born_year: f64) -> String {
    let modules: Vec<String> = Module::ALL
        .iter()
        .filter(|&&m| g.has(m))
        .map(|&m| format!("{} {:.0}%", m.label(), g.efficiency(m) * 100.0))
        .collect();
    format!(
        "{}\nBest at {:.0} °C (±{:.0}) · O₂ tolerance {:.0}%\nSize {:.1} µm · motility {:.0}% · spores {:.0}% · sticky {:.0}% · toxins {:.0}% · specialised cells {:.0}%{}\nArose in year {:.1}{}",
        modules.join(", "),
        g.temp_optimum - 273.15,
        g.temp_tolerance,
        g.oxygen_tolerance * 100.0,
        g.size,
        g.motility * 100.0,
        g.dormancy * 100.0,
        g.adhesion * 100.0,
        g.toxin * 100.0,
        g.differentiation * 100.0,
        if g.complex { " · complex cell" } else { "" },
        born_year,
        parent.map_or(String::new(), |p| format!(" from #{p}")),
    )
}

fn draw_wind_arrows(
    ui: &egui::Ui,
    rect: egui::Rect,
    view_proj: &Mat4,
    eye: V3,
    state: &dirtbox_sim::WorldState,
) {
    let grid = &state.grid;
    let eye_len = (eye.0 * eye.0 + eye.1 * eye.1 + eye.2 * eye.2).sqrt();
    let eye_dir = Vec3::new(
        (eye.0 / eye_len) as f64,
        (eye.1 / eye_len) as f64,
        (eye.2 / eye_len) as f64,
    );
    // Visible cap: points whose angle to the eye is inside the horizon.
    let horizon = (1.0 / eye_len as f64).min(1.0);
    let visible_fraction = ((1.0 - horizon) / 2.0).max(1e-3);
    let step = ((grid.cell_count() as f64 * visible_fraction / 900.0).ceil() as usize).max(1);
    let spacing = (4.0 * std::f64::consts::PI / grid.cell_count() as f64).sqrt();
    let project = |p: Vec3| -> Option<egui::Pos2> {
        let m = &view_proj.0;
        let v = [p.x as f32, p.y as f32, p.z as f32, 1.0];
        let clip: Vec<f32> = (0..4)
            .map(|r| (0..4).map(|k| m[k * 4 + r] * v[k]).sum())
            .collect();
        if clip[3] <= 0.0 {
            return None;
        }
        Some(egui::pos2(
            rect.left() + (clip[0] / clip[3] + 1.0) / 2.0 * rect.width(),
            rect.top() + (1.0 - clip[1] / clip[3]) / 2.0 * rect.height(),
        ))
    };
    let painter = ui.painter_at(rect);
    let stroke = egui::Stroke::new(1.2, Color32::from_rgba_unmultiplied(255, 255, 255, 170));
    for c in grid.cells().step_by(step) {
        let p = grid.position(c);
        if p.dot(eye_dir) < horizon + 0.05 {
            continue;
        }
        let w = state.climate.wind[c as usize];
        if w.length() < 0.3 {
            continue;
        }
        let lift = 1.002;
        let end = (p + w * (spacing * step as f64 * 0.8 / 12.0)).normalized();
        let (Some(a), Some(b)) = (project(p * lift), project(end * lift)) else {
            continue;
        };
        painter.line_segment([a, b], stroke);
        let dir = (b - a).normalized();
        let side = egui::vec2(-dir.y, dir.x);
        painter.line_segment([b, b - dir * 4.0 + side * 2.4], stroke);
        painter.line_segment([b, b - dir * 4.0 - side * 2.4], stroke);
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).strong());
}

fn row(ui: &mut egui::Ui, label: &str, value: String) {
    ui.label(RichText::new(label).weak());
    ui.label(value);
    ui.end_row();
}

fn legend_widget(ui: &mut egui::Ui, legend: &Legend) {
    match legend.scale {
        Scale::Sequential | Scale::Diverging => {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 10.0), Sense::hover());
            let steps = 64;
            for i in 0..steps {
                let t = i as f32 / (steps - 1) as f32;
                let [r, g, b] = match legend.scale {
                    Scale::Diverging => overlay::diverging(t * 2.0 - 1.0),
                    _ => overlay::sequential(t),
                };
                let x0 = rect.left() + rect.width() * i as f32 / steps as f32;
                let x1 = rect.left() + rect.width() * (i + 1) as f32 / steps as f32;
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x0, rect.top()),
                        egui::pos2(x1 + 0.5, rect.bottom()),
                    ),
                    0.0,
                    Color32::from_rgb(r, g, b),
                );
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(&legend.low).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(&legend.high).small());
                    if let Some(mid) = &legend.mid {
                        ui.centered_and_justified(|ui| ui.label(RichText::new(mid).small()));
                    }
                });
            });
        }
        Scale::Categorical => {
            for &(label, [cr, cg, cb]) in &legend.categories {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
                    ui.painter()
                        .rect_filled(r, 2.0, Color32::from_rgb(cr, cg, cb));
                    ui.label(RichText::new(label).small());
                });
            }
        }
        Scale::None => {}
    }
    if !legend.note.is_empty() {
        ui.label(RichText::new(legend.note).weak().small());
    }
}

fn planet_date(state: &dirtbox_sim::WorldState) -> String {
    let cal = state.calendar();
    let planet = state.planet_seconds();
    let year = (planet / cal.year_length).floor() as u64 + 1;
    let p = state.planet();
    if p.rotation.tidally_locked {
        return format!(
            "Planet year {year}, {:.0}% through",
            cal.year_fraction(planet) * 100.0
        );
    }
    let day = ((planet % cal.year_length) / cal.day_length).floor() as u64 + 1;
    let local = cal.day_fraction(planet) * 24.0;
    let season = if p.orbit.obliquity.to_degrees() < 3.0 {
        "no seasons".to_string()
    } else {
        let lambda = p.sun_state(planet).solar_longitude.to_degrees();
        let north = ["spring", "summer", "autumn", "winter"][((lambda / 90.0) as usize).min(3)];
        format!("northern {north}")
    };
    format!(
        "Year {year}, day {day} · {:02}:{:02} at 0° · {season}",
        local.floor() as u32,
        (local.fract() * 60.0).floor() as u32
    )
}

fn cell_spacing_km(grid: &GeodesicGrid, radius_km: f64) -> f64 {
    (4.0 * std::f64::consts::PI / grid.cell_count() as f64 * 2.0 / 3f64.sqrt()).sqrt() * radius_km
}

fn lat_lon(p: Vec3) -> String {
    let lat = p.latitude().to_degrees();
    let lon = p.longitude().to_degrees();
    format!(
        "{:.1}° {}, {:.1}° {}",
        lat.abs(),
        if lat >= 0.0 { "N" } else { "S" },
        lon.abs(),
        if lon >= 0.0 { "E" } else { "W" }
    )
}

/// Sim clock as "3 d 04:12:09".
fn sim_clock(seconds: f64) -> String {
    let s = seconds.floor() as u64;
    let (d, h, m, s) = (s / 86_400, s / 3600 % 24, s / 60 % 60, s % 60);
    if d > 0 {
        format!("{d} d {h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}")
    }
}

fn duration(seconds: f64) -> String {
    match seconds {
        s if s < 90.0 => format!("{s:.0} s"),
        s if s < 90.0 * 60.0 => format!("{:.1} min", s / 60.0),
        s if s < 48.0 * 3600.0 => format!("{:.1} h", s / 3600.0),
        s => format!("{:.2} days", s / 86_400.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formatting() {
        assert_eq!(sim_clock(59.9), "00:00:59");
        assert_eq!(sim_clock(90_061.0), "1 d 01:01:01");
        assert_eq!(duration(1200.0), "20.0 min");
        assert_eq!(duration(438_302.0), "5.07 days");
    }
}
