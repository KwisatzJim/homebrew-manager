// Homebrew Manager: a native GUI for the `brew` CLI, built with eframe/egui.
//
// Layout:
//   src/brew.rs -- all process-spawning / brew-CLI logic (no GUI deps)
//   src/app.rs  -- eframe::App implementation and all UI code
//   src/main.rs -- entry point

mod app;
mod brew;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Homebrew Manager",
        options,
        Box::new(|cc| Ok(Box::new(app::HomebrewManagerApp::new(cc)))),
    )
}
