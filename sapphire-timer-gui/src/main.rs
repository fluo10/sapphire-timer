//! Desktop entry point for the sapphire-timer GUI.
//!
//! The UI itself lives in the library (`sapphire_timer_gui`) so the future
//! mobile / WASM binaries can reuse it.

fn main() -> eframe::Result<()> {
    let _ = tracing_subscriber::fmt().try_init();

    let app = match sapphire_timer_gui::TimerApp::new() {
        Ok(app) => app,
        Err(e) => {
            eprintln!("failed to open the timer workspace: {e:#}");
            std::process::exit(1);
        }
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("sapphire-timer")
            .with_inner_size([720.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        "sapphire-timer",
        options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
}
