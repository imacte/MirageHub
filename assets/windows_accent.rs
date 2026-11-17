//! Read the Windows personalization accent, shared by the UI and daemon.

#[cfg(windows)]
pub fn read() -> Option<[u8; 3]> {
    use std::sync::OnceLock;
    use windows::{
        Win32::{
            Foundation::RPC_E_CHANGED_MODE,
            System::{
                Com::CoIncrementMTAUsage,
                WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
            },
        },
        UI::ViewManagement::{UIColorType, UISettings},
    };

    // windows-rs caches WinRT factories. Keep the MTA alive for their lifetime,
    // including when the first call came from a short-lived effect worker.
    static MTA: OnceLock<windows::core::Result<()>> = OnceLock::new();
    MTA.get_or_init(|| unsafe { CoIncrementMTAUsage().map(|_| ()) })
        .as_ref()
        .ok()?;

    let initialized = match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        Ok(()) => true,
        // An existing STA, such as the UI thread, can also use UISettings.
        Err(error) if error.code() == RPC_E_CHANGED_MODE => false,
        Err(_) => return None,
    };
    let colour = UISettings::new()
        .and_then(|settings| settings.GetColorValue(UIColorType::Accent))
        .ok()
        .map(|colour| [colour.R, colour.G, colour.B]);
    if initialized {
        unsafe { RoUninitialize() };
    }
    colour
}

#[cfg(not(windows))]
pub fn read() -> Option<[u8; 3]> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn reads_the_accent_from_successive_worker_threads() {
        let available = std::thread::spawn(super::read).join().unwrap().is_some();
        for _ in 0..2 {
            assert_eq!(
                std::thread::spawn(super::read).join().unwrap().is_some(),
                available
            );
        }
        let sta_available = std::thread::spawn(|| {
            use windows::Win32::System::WinRT::{
                RoInitialize, RoUninitialize, RO_INIT_SINGLETHREADED,
            };
            unsafe { RoInitialize(RO_INIT_SINGLETHREADED).unwrap() };
            let available = super::read().is_some();
            unsafe { RoUninitialize() };
            available
        })
        .join()
        .unwrap();
        assert_eq!(sta_available, available);
    }
}
