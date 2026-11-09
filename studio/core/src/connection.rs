//! Connection labels shared by Studio and the classic panel.
//!
//! Device names here are the original names from `data/devices.json`, not UI
//! nicknames. Their Wired/Wireless/Receiver suffixes identify distinct USB PIDs.
//! Battery support and charging state do not identify the active connection.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    /// USB transport without enough metadata to identify the link to the device.
    Usb,
    Wired,
    Wireless,
    Bluetooth,
}

pub fn detect(pid: u16, name: &str, protocol: Option<&str>) -> Connection {
    if protocol == Some("inzone") || name.starts_with("Sony INZONE") {
        return if matches!(name, "Sony INZONE H5" | "Sony INZONE H7" | "Sony INZONE H9" | "Sony INZONE H9 II" | "Sony INZONE H9 / H7" | "Sony INZONE Buds") {
            Connection::Wireless
        } else { Connection::Usb };
    }
    if protocol == Some("razer-ble") || name.contains("Bluetooth") {
        return Connection::Bluetooth;
    }
    // Check Wired first: e.g. "Razer Mamba Wireless (Wired)" is a USB cable.
    if name.contains("(Wired)") || name.ends_with(" Wired") || matches!(pid, 0x0086 | 0x00da) {
        return Connection::Wired;
    }
    if name.contains("(Wireless)")
        || name.contains("Receiver")
        || name.ends_with("2.4 Ghz Wireless")
        // Receiver PIDs whose catalog names lack a connection suffix:
        // Basilisk X, DeathAdder V2 X, HyperPolling dongle, Viper V3 HyperSpeed,
        // and Basilisk V3 X. Do not infer a connection from "HyperSpeed" alone.
        || matches!(pid, 0x0083 | 0x009c | 0x00b3 | 0x00b8 | 0x00b9)
    {
        return Connection::Wireless;
    }
    Connection::Usb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viper_v2_pro_changes_label_with_its_active_connection() {
        assert_eq!(detect(0x00a5, "Razer Viper V2 Pro (Wired)", None), Connection::Wired);
        assert_eq!(detect(0x00a6, "Razer Viper V2 Pro (Wireless)", None), Connection::Wireless);
        assert_eq!(detect(0x00b3, "Razer HyperPolling Wireless Dongle", None), Connection::Wireless);
    }

    #[test]
    fn wireless_product_names_do_not_override_a_wired_mode() {
        assert_eq!(detect(0x0073, "Razer Mamba Wireless (Wired)", None), Connection::Wired);
        assert_eq!(detect(0x0072, "Razer Mamba Wireless (Receiver)", None), Connection::Wireless);
        assert_eq!(detect(0x00da, "Razer Cobra HyperSpeed", None), Connection::Wired);
        assert_eq!(detect(0x00db, "Razer Cobra HyperSpeed Receiver", None), Connection::Wireless);
        assert_eq!(detect(0x0086, "Razer Basilisk Ultimate", None), Connection::Wired);
        assert_eq!(detect(0x0088, "RazerBasiliskUltimateReceiver", None), Connection::Wireless);
    }

    #[test]
    fn bluetooth_includes_hid_devices_and_takes_priority_over_model_names() {
        assert_eq!(detect(0x0095, "Razer Orochi V2 (Bluetooth)", None), Connection::Bluetooth);
        assert_eq!(detect(0x0094, "Razer Orochi V2 (Receiver)", None), Connection::Wireless);
        assert_eq!(detect(0x0565, "Razer BlackShark V2 HyperSpeed (Wireless)", Some("razer-ble")), Connection::Bluetooth);
    }

    #[test]
    fn usb_without_mode_metadata_stays_unspecified() {
        assert_eq!(detect(0x0032, "Razer Ouroboros", None), Connection::Usb);
        assert_eq!(detect(0xffff, "Unknown wireless mouse", None), Connection::Usb);
        assert_eq!(detect(0x0ebf, "Sony INZONE H5", Some("inzone")), Connection::Wireless);
        assert_eq!(detect(0x0dfd, "Sony INZONE receiver", Some("inzone")), Connection::Usb);
        assert_eq!(detect(0x0dfd, "Sony INZONE H3", Some("inzone")), Connection::Usb);
    }
}
