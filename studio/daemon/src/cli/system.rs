use super::{print_json, version, Arguments, ConsoleHandler, EXIT_REQUESTED};
use crate::{
    database,
    hid::HidBackend,
    keys::service::{is_event_collection, parse_report},
    Error, Result,
};
use serde_json::json;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

fn hid(args: &Arguments) -> Result<Arc<dyn HidBackend>> {
    if args.flag("--emulate") {
        return Ok(Arc::new(crate::demo::DemoBackend::new()?));
    }
    #[cfg(windows)]
    {
        Ok(Arc::new(crate::hid::windows::NativeHid))
    }
    #[cfg(not(windows))]
    {
        Err(Error::NotSupported(
            "native hardware requires Windows".into(),
        ))
    }
}

pub(super) fn doctor(args: &Arguments) -> Result<i32> {
    let backend = hid(args)?;
    let mut collections = backend.enumerate(Some(0x1532), None)?;
    collections.extend(backend.enumerate(Some(0x054c), None)?);
    let mut result = json!({"version":env!("CARGO_PKG_VERSION"),"backend":"native Rust",
        "platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,
        "database":database::devices().len(),"endpoint":args.directory().join("daemon.json"),
        "log":args.directory().join("daemon.log"),"daemon":version(args).ok(),
        "hid":collections,"attached":[],"bluetooth":[]});
    #[cfg(windows)]
    if !args.flag("--emulate") {
        use crate::ble::BleBackend;
        match crate::hid::windows::attached_devices(0x1532) {
            Ok(mut devices) => { devices.extend(crate::hid::windows::attached_devices(0x054c).unwrap_or_default()); result["attached"] = json!(devices); },
            Err(error) => result["attached_error"] = json!(error.to_string()),
        }
        if !args.flag("--json") {
            eprintln!("Scanning Bluetooth LE advertisements (4 seconds)...");
        }
        match crate::ble::windows::NativeBle.scan(
            Duration::from_secs(4),
            Some(crate::protocol::ble::COMPANY_ID),
        ) {
            Ok(devices) => result["bluetooth"] = json!(devices),
            Err(error) => result["bluetooth_error"] = json!(error.to_string()),
        }
    }
    if args.flag("--json") {
        print_json(&result);
        return Ok(0);
    }
    println!(
        "mirage-hub-daemon {} — native Rust backend ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH
    );
    println!(
        "database     {} supported product IDs",
        database::devices().len()
    );
    println!(
        "endpoint     {}",
        args.directory().join("daemon.json").display()
    );
    println!(
        "daemon log   {}",
        args.directory().join("daemon.log").display()
    );
    println!(
        "daemon       {}",
        if result["daemon"].is_null() {
            "not running"
        } else {
            "running"
        }
    );
    println!("\nRazer / Sony HID collections ({}):", collections.len());
    for info in &collections {
        let control = info.feature_length == 91
            || crate::transport::select_blackshark(std::slice::from_ref(info)).is_some();
        println!(
            "  {:04x}:{:04x} mi_{:02} usage {:04x}:{:04x} feature/input/output {}/{}/{}{}",
            info.vendor_id,
            info.product_id,
            info.interface.unwrap_or(0),
            info.usage_page,
            info.usage,
            info.feature_length,
            info.input_length,
            info.output_length,
            if control { " <- control interface" } else { "" }
        );
        println!(
            "      {}",
            database::lookup(info.vendor_id, info.product_id)
                .map_or("NOT IN DATABASE", |d| d.name.as_str())
        );
    }
    if collections.is_empty() {
        println!("  none — check the USB data cable or wireless receiver");
    }
    let mut seen = std::collections::BTreeSet::new();
    for device in result["attached"].as_array().into_iter().flatten() {
        let pid = device["product_id"].as_u64().unwrap_or(0) as u16;
        let transport = device["transport"].as_str().unwrap_or_default();
        if collections.iter().any(|i| i.product_id == pid) || !seen.insert((pid, transport)) {
            continue;
        }
        println!(
            "\nOther Razer device: {transport} {pid:04x} {}",
            device["description"].as_str().unwrap_or_default()
        );
        if transport.starts_with("Bluetooth") {
            println!("  BLE lighting support is built in. For USB-only models, connect a USB data cable or receiver.");
        } else {
            println!(
                "  Windows sees the device, but it exposes no supported HID lighting collection."
            );
        }
    }
    println!("\nBluetooth LE: native Windows support is built in (no Python package required).");
    if let Some(error) = result["bluetooth_error"].as_str() {
        println!("  scan failed: {error}");
    }
    for advertiser in result["bluetooth"].as_array().into_iter().flatten() {
        println!(
            "  {:012X}  {} dBm  {}",
            advertiser["address"].as_u64().unwrap_or(0),
            advertiser["rssi"],
            advertiser["name"].as_str().unwrap_or_default()
        );
    }
    Ok(0)
}

pub(super) fn watch(args: &Arguments) -> Result<i32> {
    let backend = hid(args)?;
    let mut readers = Vec::new();
    for info in backend
        .enumerate(Some(0x1532), None)?
        .into_iter()
        .filter(is_event_collection)
    {
        if args
            .value("--device")
            .is_some_and(|s| !format!("{:04x}", info.product_id).contains(&s.to_lowercase()))
        {
            continue;
        }
        match backend.open_input(&info) {
            Ok(reader) => {
                println!("listening on {}", info.path);
                readers.push((info.product_id, reader));
            }
            Err(error) => eprintln!("cannot read {}: {error}", info.path),
        }
    }
    if readers.is_empty() {
        println!("No readable Razer key collections. Is the keyboard plugged in?");
        return Ok(1);
    }
    println!("Press macro keys and Fn combinations; Ctrl+C to stop. Keep the daemon running so the keyboard stays in driver mode.");
    let _console = ConsoleHandler::install();
    std::thread::scope(|scope| {
        let workers: Vec<_> = readers
            .into_iter()
            .map(|(pid, mut reader)| {
                scope.spawn(move || {
                    while !EXIT_REQUESTED.load(Ordering::Acquire) {
                        match reader.read(Duration::from_millis(500)) {
                            Ok(Some(report)) => {
                                let bytes = report
                                    .iter()
                                    .map(|b| format!("{b:02x}"))
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                let codes = parse_report(
                                    &report,
                                    crate::keys::service::key_data().report_id,
                                );
                                println!(
                                    "{pid:04x}  {bytes}  {}",
                                    codes.map_or(String::new(), |codes| format!(
                                        "held: {}",
                                        codes
                                            .iter()
                                            .map(|c| format!("{c:02x}"))
                                            .collect::<Vec<_>>()
                                            .join(" ")
                                    ))
                                );
                            }
                            Ok(None) => {}
                            Err(error) => {
                                eprintln!("{pid:04x}: {error}");
                                break;
                            }
                        }
                    }
                })
            })
            .collect();
        for worker in workers {
            let _ = worker.join();
        }
    });
    Ok(0)
}

#[cfg(windows)]
pub(super) fn autostart(args: &Arguments) -> Result<i32> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE},
        RegKey,
    };
    const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const NAME: &str = "OpenRazerWin";
    let root = RegKey::predef(HKEY_CURRENT_USER);
    let action = args.positional.get(1).map_or("status", String::as_str);
    let command = match action {
        "status" => match root
            .open_subkey_with_flags(RUN, KEY_READ)
            .and_then(|key| key.get_value::<String, _>(NAME))
        {
            Ok(command) => Some(command),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        },
        "disable" => {
            match root
                .open_subkey_with_flags(RUN, KEY_SET_VALUE)
                .and_then(|key| key.delete_value(NAME))
            {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            None
        }
        "enable" => {
            let exe = std::env::current_exe()?;
            // Explicit daemon flags must reach the backend. Otherwise prefer
            // Studio's tray startup when the complete application is installed.
            let studio = exe.with_file_name("mirage-hub.exe");
            let gui = exe.with_file_name("mirage-hub-classic.exe");
            let (program, mut flags) = if !args.flag("--no-effects") && studio.is_file() {
                (studio, vec!["--background".into()])
            } else if !args.flag("--no-effects") && gui.is_file() {
                (gui, vec!["--background".into()])
            } else {
                (exe, vec!["daemon".into(), "run".into()])
            };
            if args.flag("--no-effects") {
                flags.push("--no-effects".into());
            }
            let wide = crate::process::quoted_line(&program, &flags)?;
            let command = String::from_utf16(&wide[..wide.len() - 1])
                .map_err(|_| Error::Invalid("startup command is not valid Unicode".into()))?;
            let (key, _) = root.create_subkey_with_flags(RUN, KEY_SET_VALUE)?;
            key.set_value(NAME, &command)?;
            Some(command)
        }
        _ => unreachable!("validated startup action"),
    };
    if args.flag("--json") {
        print_json(&json!({"enabled":command.is_some(),"command":command}));
    } else if !args.flag("--quiet") {
        println!(
            "autostart: {}",
            if command.is_some() { "on" } else { "off" }
        );
        if let Some(command) = &command {
            println!("command:   {command}");
        }
    }
    Ok(i32::from(
        action == "status" && command.is_none() && !args.flag("--json"),
    ))
}
#[cfg(not(windows))]
pub(super) fn autostart(_args: &Arguments) -> Result<i32> {
    Err(Error::NotSupported("autostart requires Windows".into()))
}
