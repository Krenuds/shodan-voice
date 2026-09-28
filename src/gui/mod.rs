mod app;
mod knob;
#[cfg(feature = "clap-host")]
mod plugin_panel;
mod theme;

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title("SHODAN Voice").with_inner_size([1180.0, 820.0]).with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native("SHODAN Voice", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
