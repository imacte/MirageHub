//! The window's life outside egui: one instance, the tray icon and activation.
//!
//! Asynchronous Win32 activation can restore a sleeping UI without blocking
//! the tray thread or applying the tray process's hidden startup state.

use rsrazer_core::t;

pub const WINDOW_TITLE: &str = "RsRazer";

/// External tools must not inherit the UI's private pipes. Otherwise an open
/// log viewer could keep the closed UI's command pipe alive indefinitely.
pub fn external(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    use std::process::{Command, Stdio};
    let mut command = Command::new(program);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command
}

#[cfg(windows)]
mod win {
    use super::WINDOW_TITLE;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindowAsync, SW_RESTORE, SW_SHOW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn window() -> Option<HWND> {
        let title = wide(WINDOW_TITLE);
        let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if hwnd.is_null() {
            None
        } else {
            Some(hwnd)
        }
    }

    /// True if this is the only control panel in the session.
    pub fn claim_single_instance() -> bool {
        let name = wide("Local\\rsrazer");
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            // Deliberately never closed: held for the life of the process.
            !handle.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
        }
    }

    pub fn show() {
        if let Some(hwnd) = window() {
            unsafe {
                if IsIconic(hwnd) != 0 {
                    ShowWindowAsync(hwnd, SW_RESTORE);
                } else {
                    ShowWindowAsync(hwnd, SW_SHOW);
                }
                SetForegroundWindow(hwnd);
            }
        }
    }

    pub fn is_visible() -> bool {
        window().is_some_and(|hwnd| unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 })
    }
}

#[cfg(not(windows))]
mod win {
    pub fn claim_single_instance() -> bool {
        true
    }
    pub fn show() {}
    pub fn is_visible() -> bool { true }
}

pub use win::{claim_single_instance, is_visible, show};

pub use rsrazer_core::branding::icon_rgba;

pub enum TrayAction {
    Open,
    RestartDaemon,
    Quit,
}

/// The tray icon and its menu.  Must be built on the UI thread and kept alive.
pub struct Tray {
    _icon: tray_icon::TrayIcon,
    open: tray_icon::menu::MenuItem,
    restart: tray_icon::menu::MenuItem,
    quit: tray_icon::menu::MenuItem,
}

impl Tray {
    pub fn new(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> Option<Tray> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

        let open = MenuItem::new(t("Open RsRazer"), true, None);
        let restart = MenuItem::new(t("Restart the daemon"), true, None);
        let quit = MenuItem::new(t("Quit"), true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &restart, &PredefinedMenuItem::separator(), &quit]).ok()?;
        let icon = Icon::from_rgba(icon_rgba(32), 32, 32).ok()?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(WINDOW_TITLE)
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        let on_action = std::sync::Arc::new(on_action);
        let (open_id, restart_id, quit_id) = (open.id().clone(), restart.id().clone(), quit.id().clone());
        let menu_action = on_action.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == open_id {
                menu_action(TrayAction::Open);
            } else if event.id == restart_id {
                menu_action(TrayAction::RestartDaemon);
            } else if event.id == quit_id {
                menu_action(TrayAction::Quit);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                on_action(TrayAction::Open);
            }
        }));
        Some(Tray { _icon: tray, open, restart, quit })
    }

    pub fn refresh_language(&self) {
        self.open.set_text(t("Open RsRazer"));
        self.restart.set_text(t("Restart the daemon"));
        self.quit.set_text(t("Quit"));
    }
}
