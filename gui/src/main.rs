//! Mirage Hub: the control panel.
//!
//! A client of the mirage-hub-daemon daemon, which owns the devices. Started with
//! `--background` (as it is at sign-in) it lives in the tray and makes sure
//! the daemon is running; otherwise it opens its window.

#![windows_subsystem = "windows"]

mod daemon;
#[path = "../../assets/data_paths.rs"]
mod data_paths;
#[path = "../../studio/core/src/connection.rs"]
mod connection;
mod images;
mod rpc;
mod shell;
mod ui;
mod worker;

use std::sync::Arc;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        attach_console();
        println!("mirage-hub-classic {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--self-test") {
        // For the release build's smoke test: everything short of a window.
        let icon = shell::icon_rgba(64);
        let icon_ok = icon.len() == 64 * 64 * 4 && icon[(32 * 64 + 32) * 4 + 3] == 255;
        std::process::exit(if icon_ok && daemon::embedded_daemon_ok(args.iter().any(|a| a == "--require-daemon")) { 0 } else { 1 });
    }
    if let Err(error) = data_paths::migrate_rsrazer() { eprintln!("Settings migration failed: {error}"); return Ok(()); }
    // Older autostart entries ran this program as "daemon run"; they mean the
    // same thing now: come up in the background and keep the daemon running.
    let background = args.iter().any(|a| a == "--background" || a == "--tray")
        || (args.first().map(String::as_str) == Some("daemon") && args.get(1).map(String::as_str) == Some("run"));

    if std::env::var_os("OPENRAZER_GUI_CAPTURE").is_none() && !shell::claim_single_instance() {
        if !background {
            shell::show();
        }
        return Ok(());
    }

    let icon = egui::IconData { rgba: shell::icon_rgba(64), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(shell::WINDOW_TITLE)
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([940.0, 620.0])
            .with_visible(!background)
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    eframe::run_native(shell::WINDOW_TITLE, options, Box::new(move |cc| Ok(Box::new(ui::App::new(cc, background)))))
}

/// Print to the console that started us, if any: this is a windowed program.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}
