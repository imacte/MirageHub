//! Read-only discovery. Never opens a GATT connection or changes a device.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use rsrazer_daemon::{
        ble::{windows::NativeBle, BleBackend, SCAN_SECONDS},
        protocol::ble::COMPANY_ID,
    };
    println!(
        "{}",
        serde_json::to_string(&NativeBle.scan(SCAN_SECONDS, Some(COMPANY_ID))?)?
    );
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    println!("[]");
}
