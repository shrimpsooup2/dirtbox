//! `dirtbox-viewer`: the 3D globe and player controls.

mod app;
mod calibration;
mod camera;
mod globe;
mod overlay;

use eframe::egui;

fn options(multisampling: u16) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Dirtbox")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 600.0]),
        renderer: eframe::Renderer::Glow,
        depth_buffer: 24,
        multisampling,
        ..Default::default()
    }
}

fn run(multisampling: u16) -> eframe::Result {
    eframe::run_native(
        "Dirtbox",
        options(multisampling),
        Box::new(|cc| Ok(Box::new(app::DirtboxApp::new(cc)?))),
    )
}

fn main() -> eframe::Result {
    // Prefer 4× antialiasing, but some drivers (and virtual displays) offer no
    // multisampled configuration; fall back to none rather than failing.
    match run(4) {
        Err(eframe::Error::NoGlutinConfigs(_, e)) => {
            eprintln!(
                "dirtbox-viewer: no 4× multisampling available ({e}); retrying without antialiasing"
            );
            run(0)
        }
        other => other,
    }
}
