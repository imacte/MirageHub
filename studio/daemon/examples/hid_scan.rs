//! Read-only diagnostic: enumerate HID descriptors without sending commands.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use openrazer_daemon::hid::{windows::NativeHid, HidBackend};
    let devices = NativeHid.enumerate(Some(0x1532), None)?;
    println!("{}", serde_json::to_string(&devices)?);
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    println!("[]");
}
