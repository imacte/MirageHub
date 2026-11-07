//! Native dragging and edge resizing for the integrated window controls.

use crate::{chrome, theme::BORDER};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND}, Gdi::ScreenToClient},
    UI::{HiDpi::GetDpiForWindow, Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass}, WindowsAndMessaging::*},
};

const SUBCLASS: usize = 0x52535A52;

fn hwnd(window: &impl HasWindowHandle) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return None };
    Some(handle.hwnd.get() as HWND)
}

/// Fall back to a decorated window if subclassing is unavailable.
pub fn install(window: &impl HasWindowHandle, resizable: bool) -> bool {
    let Some(hwnd) = hwnd(window) else { return false };
    let border = u32::from(BORDER.r()) | (u32::from(BORDER.g()) << 8) | (u32::from(BORDER.b()) << 16);
    // All calls run on the window's owning UI thread; DWM copies these values.
    unsafe {
        for (attribute, value) in [(DWMWA_USE_IMMERSIVE_DARK_MODE, 1_u32), (DWMWA_BORDER_COLOR, border), (DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND as u32)] {
            DwmSetWindowAttribute(hwnd, attribute as u32, (&value as *const u32).cast(), size_of::<u32>() as u32);
        }
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS, data(scale, resizable)) != 0
    }
}

/// Keep native hit testing aligned with both monitor DPI and egui zoom.
pub fn sync(window: &impl HasWindowHandle, scale: f32, resizable: bool) {
    if let Some(hwnd) = hwnd(window) {
        unsafe { SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS, data(scale, resizable)); }
    }
}

fn data(scale: f32, resizable: bool) -> usize {
    (((scale.clamp(0.25, 16.0) * 1024.0).round() as usize) << 1) | usize::from(resizable)
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, id: usize, data: usize) -> LRESULT {
    if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(window_proc), id);
    } else if message == WM_NCHITTEST {
        // LPARAM coordinates are signed, including monitors left/above the primary.
        let mut point = POINT { x: lparam as i16 as i32, y: (lparam >> 16) as i16 as i32 };
        let mut rect: RECT = std::mem::zeroed();
        if ScreenToClient(hwnd, &mut point) != 0 && GetClientRect(hwnd, &mut rect) != 0 {
            let scale = (data >> 1) as f32 / 1024.0;
            let resizable = data & 1 != 0;
            let edge = (6.0 * scale).round() as i32;
            if resizable && IsZoomed(hwnd) == 0 {
                let left = point.x < edge;
                let right = point.x >= rect.right - edge;
                let top = point.y < edge;
                let bottom = point.y >= rect.bottom - edge;
                let hit = match (left, right, top, bottom) {
                    (true, _, true, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, true, _, true) => HTBOTTOMRIGHT,
                    (true, _, _, _) => HTLEFT,
                    (_, true, _, _) => HTRIGHT,
                    (_, _, true, _) => HTTOP,
                    (_, _, _, true) => HTBOTTOM,
                    _ => HTCLIENT,
                };
                if hit != HTCLIENT { return hit as LRESULT; }
            }
            let buttons = chrome::BUTTON_WIDTH * if resizable { 3.0 } else { 2.0 };
            if point.y >= 0 && point.y < (chrome::HEIGHT * scale) as i32 && point.x < rect.right - (buttons * scale) as i32 {
                return HTCAPTION as LRESULT;
            }
            // The buttons are client controls, even where the old caption was.
            return HTCLIENT as LRESULT;
        }
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}
