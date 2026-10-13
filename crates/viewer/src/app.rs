//! The viewer application: globe, time controls, world settings and the
//! performance (calibration) window.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, RichText, Sense};
use egui_glow::glow::{self, HasContext};

use dirtbox_core::consts::EARTH_RADIUS;
use dirtbox_core::time::QUANTUM_SECONDS;
use dirtbox_core::{SimTime, Vec3};
use dirtbox_grid::{CellId, GeodesicGrid};
use dirtbox_hw::{BudgetInputs, HardwareInfo, detect, recommend, speed_for_population};
use dirtbox_sim::{EvolutionSettings, MutationBoost, World, WorldConfig};

use crate::calibration::Calibration;
use crate::camera::OrbitCamera;
use crate::globe::{FrameUniforms, GlobeMesh, GlobeRenderer};
use crate::overlay::{self, Legend, Overlay, OverlayContext, Scale};

/// Grid subdivisions offered in the UI (cell counts 2,562 to 163,842).
const GRID_CHOICES: [u32; 5] = [16, 32, 64, 96, 128];
/// Sim seconds per wall second.
const SPEEDS: [(f64, &str); 6] = [
    (1.0, "1×"),
    (10.0, "10×"),
    (100.0, "100×"),
    (1_000.0, "1k×"),
    (10_000.0, "10k×"),
    (100_000.0, "100k×"),
];
const BUDGET_KEY: &str = "population_budget";
const BACKGROUND: Color32 = Color32::from_rgb(0x0e, 0x0f, 0x12);

pub struct DirtboxApp {
    world: World,
    mesh: GlobeMesh,
    renderer: Arc<Mutex<GlobeRenderer>>,
    camera: OrbitCamera,

    overlay: Overlay,
    values: Vec<f32>,
    legend: Legend,
    sun_at_last_colouring: Vec3,
    sunlit: bool,
    borders: bool,
    hovered: Option<CellId>,
    selected: Option<CellId>,

    paused: bool,
    speed: f64,
    /// Exact (unquantised) sim time the world clock is chasing.
    sim_target_seconds: f64,

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

        let world = World::new(WorldConfig::default())?;
        let mesh = GlobeMesh::build(&world.state().grid);
        let population_budget = cc
            .storage
            .and_then(|s| s.get_string(BUDGET_KEY))
            .and_then(|s| s.parse().ok());

        let mut app = Self {
            world,
            mesh,
            renderer: Arc::new(Mutex::new(renderer)),
            camera: OrbitCamera::default(),
            overlay: Overlay::Latitude,
            values: Vec::new(),
            legend: Overlay::Plain.colours(&[]).1,
            sun_at_last_colouring: Vec3::ZERO,
            sunlit: true,
            borders: true,
            hovered: None,
            selected: None,
            paused: false,
            speed: 10.0,
            sim_target_seconds: 0.0,
            hw: detect(),
            gl_info,
            calibration: Calibration::NotRun,
            // First run: no stored budget yet, so open the performance window.
            show_performance: population_budget.is_none(),
            population_budget,
            custom_budget: population_budget.unwrap_or(20_000),
        };
        // Start looking at the day side.
        app.camera.lon = app.sun().longitude() as f32;
        app.renderer.lock().unwrap().set_mesh(&app.mesh);
        app.recolour();
        Ok(app)
    }

    fn grid(&self) -> &GeodesicGrid {
        &self.world.state().grid
    }

    /// Placeholder sun: overhead at the equator, circling once per planet day.
    /// Phase 1 replaces this with the real orbit and axial tilt.
    fn sun(&self) -> Vec3 {
        let state = self.world.state();
        let f = state.calendar().day_fraction(state.planet_seconds());
        // Midnight at 0° longitude when f = 0; local noon there when f = 0.5.
        Vec3::from_lat_lon(0.0, std::f64::consts::PI * (1.0 - 2.0 * f))
    }

    fn recolour(&mut self) {
        let ctx = OverlayContext {
            sun: self.sun(),
            radius_km: EARTH_RADIUS / 1000.0,
        };
        self.values = self.overlay.values(self.grid(), &ctx);
        let (colours, legend) = self.overlay.colours(&self.values);
        self.legend = legend;
        let expanded = self.mesh.expand_colours(&colours);
        self.renderer.lock().unwrap().set_colours(expanded);
        self.sun_at_last_colouring = ctx.sun;
    }

    fn rebuild_world(&mut self, subdivision: u32) {
        let state = self.world.state();
        let config = WorldConfig {
            seed: state.seed(),
            time: state.time_model().clone(),
            calendar: state.calendar().clone(),
            grid_subdivision: subdivision,
            evolution: state.evolution().clone(),
        };
        match World::new(config) {
            Ok(world) => {
                self.world = world;
                self.sim_target_seconds = 0.0;
                self.hovered = None;
                self.selected = None;
                self.mesh = GlobeMesh::build(self.grid());
                self.renderer.lock().unwrap().set_mesh(&self.mesh);
                self.recolour();
            }
            Err(e) => log_error(&e),
        }
    }

    fn advance_time(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) && ctx.memory(|m| m.focused().is_none()) {
            self.paused = !self.paused;
        }
        if self.paused {
            return;
        }
        let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
        self.sim_target_seconds += dt * self.speed;
        let quanta = (self.sim_target_seconds / QUANTUM_SECONDS).floor() as u64;
        self.world.run_until(SimTime::from_quanta(quanta));

        // Re-colour time-dependent overlays once the sun has moved ~0.25°.
        if self.overlay.is_time_dependent()
            && self.sun().dot(self.sun_at_last_colouring) < 0.25f64.to_radians().cos()
        {
            self.recolour();
        }
        ctx.request_repaint();
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
                    .on_hover_text(format!("{label}: {} sim seconds per real second", speed))
                    .clicked()
                {
                    self.speed = speed;
                }
            }
            ui.separator();
            let state = self.world.state();
            let cal = state.calendar();
            let planet = state.planet_seconds();
            let year = (planet / cal.year_length).floor() as u64 + 1;
            let day_of_year = ((planet % cal.year_length) / cal.day_length).floor() as u64 + 1;
            let local = cal.day_fraction(planet) * 24.0;
            ui.label(format!("Sim time {}", sim_clock(state.now().seconds())));
            ui.separator();
            ui.label(format!(
                "Planet: year {year}, day {day_of_year}, {:02}:{:02} at 0° longitude",
                local.floor() as u32,
                ((local.fract()) * 60.0).floor() as u32
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Performance…").clicked() {
                    self.show_performance = !self.show_performance;
                }
                if let Some(n) = self.population_budget {
                    ui.label(
                        RichText::new(format!("Budget {} organisms", overlay::group(n as f32)))
                            .weak(),
                    );
                }
            });
        });
    }

    fn world_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        section(ui, "Overlay");
        let before = self.overlay;
        for o in Overlay::ALL {
            ui.radio_value(&mut self.overlay, o, o.label());
        }
        if self.overlay != before {
            self.recolour();
        }
        ui.add_space(4.0);
        legend_widget(ui, &self.legend);

        ui.add_space(8.0);
        section(ui, "View");
        ui.checkbox(&mut self.sunlit, "Day and night lighting");
        ui.checkbox(&mut self.borders, "Cell borders");

        ui.add_space(8.0);
        section(ui, "Time");
        let mut k = self.world.state().time_model().season_speed;
        let slider = ui
            .add(
                egui::Slider::new(&mut k, 1.0..=5_000.0)
                    .logarithmic(true)
                    .text("season speed"),
            )
            .on_hover_text(
                "How fast days and seasons pass compared with creatures' lives. \
                 1 = fully realistic. Creatures' bodies always run in real time.",
            );
        if slider.changed() {
            if let Err(e) = self.world.set_season_speed(k) {
                log_error(&e);
            }
        }
        let cal = self.world.state().calendar();
        ui.label(
            RichText::new(format!(
                "1 planet day = {}\n1 planet year = {}",
                duration(cal.day_length / k),
                duration(cal.year_length / k)
            ))
            .weak(),
        );

        ui.add_space(8.0);
        section(ui, "Grid");
        let current = self.grid().subdivision();
        let mut chosen = current;
        egui::ComboBox::from_id_salt("grid")
            .selected_text(format!(
                "{} cells",
                overlay::group(GeodesicGrid::cell_count_for(current) as f32)
            ))
            .show_ui(ui, |ui| {
                for n in GRID_CHOICES {
                    let cells = GeodesicGrid::cell_count_for(n);
                    let spacing = (4.0 * std::f64::consts::PI / cells as f64 * 2.0 / 3f64.sqrt())
                        .sqrt()
                        * EARTH_RADIUS
                        / 1000.0;
                    ui.selectable_value(
                        &mut chosen,
                        n,
                        format!(
                            "{} cells (~{:.0} km)",
                            overlay::group(cells as f32),
                            spacing
                        ),
                    );
                }
            });
        if chosen != current {
            self.rebuild_world(chosen);
        }
        ui.label(
            RichText::new("Changing the grid restarts the world.")
                .weak()
                .small(),
        );

        ui.add_space(8.0);
        section(ui, "Evolution");
        let mut evo = self.world.state().evolution().clone();
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
        if changed {
            if let Err(e) = self.world.set_evolution(evo) {
                log_error(&e);
            }
        }
        ui.label(
            RichText::new("Takes effect once life exists (Phase 2).")
                .weak()
                .small(),
        );

        ui.add_space(8.0);
        section(ui, "Cell");
        self.cell_inspector(ui);
    }

    fn cell_inspector(&self, ui: &mut egui::Ui) {
        let Some(cell) = self.hovered.or(self.selected) else {
            ui.label(RichText::new("Hover over or click a cell.").weak());
            return;
        };
        let g = self.grid();
        let p = g.position(cell);
        let r_km = EARTH_RADIUS / 1000.0;
        egui::Grid::new("cell")
            .num_columns(2)
            .spacing([12.0, 2.0])
            .show(ui, |ui| {
                ui.label("Cell");
                ui.label(format!(
                    "#{cell}{}",
                    if Some(cell) == self.selected && self.hovered != self.selected {
                        " (selected)"
                    } else {
                        ""
                    }
                ));
                ui.end_row();
                ui.label("Position");
                ui.label(lat_lon(p));
                ui.end_row();
                ui.label("Area");
                ui.label(format!(
                    "{} km²",
                    overlay::group((g.area(cell) * r_km * r_km) as f32)
                ));
                ui.end_row();
                ui.label("Shape");
                ui.label(if g.neighbours(cell).len() == 5 {
                    "pentagon"
                } else {
                    "hexagon"
                });
                ui.end_row();
                // Area already has its own row above.
                if let Some(text) = self
                    .overlay
                    .describe(self.values[cell as usize])
                    .filter(|_| self.overlay != Overlay::CellArea)
                {
                    ui.label(self.overlay.label());
                    ui.label(text);
                    ui.end_row();
                }
            });
    }

    fn globe(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let aspect = rect.width() / rect.height().max(1.0);

        if response.dragged() {
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
        if self.camera.update(ui.input(|i| i.stable_dt)) {
            ui.ctx().request_repaint();
        }

        let picked = response.hover_pos().and_then(|pos| {
            let ndc_x = (pos.x - rect.left()) / rect.width() * 2.0 - 1.0;
            let ndc_y = 1.0 - (pos.y - rect.top()) / rect.height() * 2.0;
            self.camera.pick(ndc_x, ndc_y, aspect)
        });
        self.hovered = picked.map(|(x, y, z)| {
            let v = Vec3::new(x as f64, y as f64, z as f64);
            match self.hovered {
                Some(hint) => self.grid().locate_from(hint, v),
                None => self.grid().locate(v),
            }
        });
        if response.clicked() {
            self.selected = self.hovered;
        }

        let sun = self.sun();
        let uniforms = FrameUniforms {
            view_proj: self.camera.view_proj(aspect),
            eye: self.camera.eye(),
            sun: (sun.x as f32, sun.y as f32, sun.z as f32),
            sunlit: self.sunlit,
            // Fade borders out from far away, where cells are only a few pixels.
            borders: if self.borders {
                let cells_across = (self.grid().cell_count() as f32).sqrt();
                let px_per_cell =
                    rect.height() / cells_across * (2.2 / (self.camera.altitude() + 0.2));
                ((px_per_cell - 4.0) / 8.0).clamp(0.0, 1.0)
            } else {
                0.0
            },
            hovered: self.hovered.or(self.selected),
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

        let painter = ui.painter();
        let weak = ui.visuals().weak_text_color();
        painter.text(
            rect.left_bottom() + egui::vec2(12.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            "Drag to rotate · scroll to zoom · click a cell to select",
            egui::FontId::proportional(12.0),
            weak,
        );
        painter.text(
            rect.right_top() + egui::vec2(-12.0, 10.0),
            egui::Align2::RIGHT_TOP,
            format!(
                "Altitude {} km",
                overlay::group(self.camera.altitude() * (EARTH_RADIUS / 1000.0) as f32)
            ),
            egui::FontId::proportional(12.0),
            weak,
        );
    }

    fn performance_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_performance;
        egui::Window::new("Performance")
            .open(&mut open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .resizable(false)
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
                        ui.label("CPU");
                        ui.label(format!(
                            "{} ({} logical cores)",
                            self.hw.cpu_model.as_deref().unwrap_or("unknown model"),
                            self.hw.logical_cores
                        ));
                        ui.end_row();
                        ui.label("Memory");
                        ui.label(format!(
                            "{} total, {} available",
                            ram(self.hw.total_ram_bytes),
                            ram(self.hw.available_ram_bytes)
                        ));
                        ui.end_row();
                        ui.label("Graphics");
                        ui.label(&self.gl_info);
                        ui.end_row();
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
                        let state = self.world.state();
                        let k = state.time_model().season_speed;
                        let inputs = BudgetInputs {
                            throughput,
                            mean_step_seconds: BudgetInputs::DEFAULT_MEAN_STEP_SECONDS,
                            headroom: BudgetInputs::DEFAULT_HEADROOM,
                            bytes_per_organism: BudgetInputs::DEFAULT_BYTES_PER_ORGANISM,
                            available_ram_bytes: self.hw.available_ram_bytes,
                            planet_year_sim_seconds: state.calendar().year_length / k,
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
        self.advance_time(&ctx);

        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("world")
            .resizable(false)
            .exact_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.world_panel(ui));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND))
            .show(ui, |ui| self.globe(ui));
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

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).strong());
}

fn legend_widget(ui: &mut egui::Ui, legend: &Legend) {
    if legend.scale != Scale::None {
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
    if !legend.note.is_empty() {
        ui.label(RichText::new(legend.note).weak().small());
    }
}

fn lat_lon(p: Vec3) -> String {
    let lat = p.latitude().to_degrees();
    let lon = p.longitude().to_degrees();
    format!(
        "{:.2}° {}, {:.2}° {}",
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

fn log_error(e: &str) {
    eprintln!("dirtbox-viewer: {e}");
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
