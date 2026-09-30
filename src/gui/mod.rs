mod app;
mod inspector;
mod knob;
mod param_ui;
mod patchbay;
mod rack;
mod state;
mod statusbar;
mod theme;
mod toolbar;
mod widgets;

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title("SHODAN Voice").with_inner_size([1180.0, 820.0]).with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("SHODAN Voice", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
