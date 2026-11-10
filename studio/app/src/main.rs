//! Mirage Hub: Razer and Sony INZONE device control on Windows.
//!
//! A client of the mirage-hub-daemon daemon, which owns the devices. Started with
//! `--background` (as it is at sign-in) it lives in the tray and makes sure
//! the daemon is running; otherwise it opens its window.

#![windows_subsystem = "windows"]

mod accent;
mod appwatch;
mod art;
mod chroma;
mod daemon;
mod images;
mod ipc;
#[cfg(windows)]
mod host;
mod layout;
mod notes;
mod microphone_test;
mod outline;
mod preview;
mod report;
mod settings;
mod shell;
mod ui;
mod updates;
mod worker;

use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        attach_console();
        println!("mirage-hub {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--self-test") {
        // For the release build's smoke test: everything short of a window.
        let icon = shell::icon_rgba(64);
        let drawn = art::Art::build_with("keyboard", &serde_json::json!({"matrix_dimensions": [6, 22], "custom_frame": true, "zones": {"backlight": {}}}), None, None);
        let ok = icon.len() == 64 * 64 * 4 && icon[(32 * 64 + 32) * 4 + 3] == 255 && drawn.shapes.len() > 100;
        std::process::exit(if ok { 0 } else { 1 });
    }
    mirage_hub_core::data_paths::migrate_rsrazer()?;
    let background = args.iter().any(|a| a == "--background" || a == "--tray");

    // A run that only takes a screenshot leaves a Mirage Hub already open alone.
    let capturing = std::env::var_os("OPENRAZER_STUDIO_CAPTURE").is_some();
    let managed_ui = args.iter().any(|a| a == "--ui") && std::env::var_os("MIRAGE_HUB_UI_CHILD").is_some();
    #[cfg(windows)]
    if !capturing && !managed_ui { return host::run(background).map_err(Into::into); }

    let icon = egui::IconData { rgba: shell::icon_rgba(64), width: 64, height: 64 };
    // For checking the design at other sizes, with the screenshot hook in
    // `ui::app`: `OPENRAZER_STUDIO_SIZE=1000x660`.
    let size = std::env::var("OPENRAZER_STUDIO_SIZE")
        .ok()
        .and_then(|s| s.split_once('x').and_then(|(w, h)| Some([w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?])))
        .unwrap_or([1320.0, 860.0]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(shell::WINDOW_TITLE)
            .with_decorations(!cfg!(windows))
            .with_inner_size(size)
            .with_min_inner_size([1000.0, 660.0])
            .with_visible(true)
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    eframe::run_native(shell::WINDOW_TITLE, options, Box::new(move |cc| Ok(Box::new(ui::App::new(cc, managed_ui)))))?;
    Ok(())
}

/// Print to the console that started us, if any: this is a windowed program.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}
