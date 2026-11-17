//! The accent colour Windows is set to, for showing what the daemon will light.

use egui::Color32;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What Windows starts with.
pub const WINDOWS_BLUE: Color32 = Color32::from_rgb(0x00, 0x78, 0xD4);

/// The colour now, looked at again every couple of seconds: it is drawn every
/// frame, and the registry need not be.
pub fn colour() -> Color32 {
    static SEEN: Mutex<Option<(Instant, Color32)>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, colour)) = *seen {
        if at.elapsed() < Duration::from_secs(2) {
            return colour;
        }
    }
    let colour = read().unwrap_or(WINDOWS_BLUE);
    *seen = Some((Instant::now(), colour));
    colour
}

fn read() -> Option<Color32> {
    let [r, g, b] = crate::windows_accent::read()?;
    Some(Color32::from_rgb(r, g, b))
}
