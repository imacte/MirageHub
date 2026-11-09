//! Read-only diagnostic of INZONE control collections and capability replies.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use mirage_hub_daemon::{
        hid::{windows::NativeHid, HidBackend},
        inzone::{HidIo, Session},
        protocol::inzone,
    };
    use std::sync::Arc;
    let backend = Arc::new(NativeHid);
    for info in backend.enumerate(Some(inzone::VENDOR), None)? {
        println!(
            "{:04X}:{:04X} page={:04X} input={} output={} {}",
            info.vendor_id,
            info.product_id,
            info.usage_page,
            info.input_length,
            info.output_length,
            info.product
        );
        if info.usage_page != 0xff04 || info.input_length != 64 || info.output_length != 64 {
            continue;
        }
        if std::env::args().any(|arg|arg=="--audio") {
            use mirage_hub_daemon::{database,device::{Device,InzoneDevice},persistence::Persistence};
            let mut device=InzoneDevice::new(database::lookup(info.vendor_id,info.product_id).ok_or("Unknown INZONE receiver")?.clone(),info.clone(),Box::new(HidIo::new(backend.clone(),info.clone())),Persistence::memory().shared()).with_native_microphone(true);
            println!("{}",serde_json::to_string_pretty(&device.call("get_audio",&[],&serde_json::Map::new())?)?);
            device.close();continue;
        }
        let mut session = Session::new(Box::new(HidIo::new(backend.clone(), info)));
        for event in [1, 2, 4, 6, 7, 8, 0x21, 0x22, 0x23, 0x24, 0x81, 0x83] {
            match session.request(event, None) {
                Ok(bytes) => {
                    let bytes = if event == 2 {
                        &bytes[..bytes.len().min(6)]
                    } else {
                        &bytes
                    };
                    println!("{event:02X}: {bytes:02X?}");
                }
                Err(error) => println!("{event:02X}: {error}"),
            }
        }
        if std::env::args().any(|arg| arg == "--verify-h5-writes") {
            if session.request(2, None)?.first() != Some(&3) {
                return Err("Write verification is restricted to INZONE H5".into());
            }
            // Save each register before the first write and restore it even if
            // its acknowledgement or subsequent read fails.
            for (event, index, values) in [
                (0x21, 1, vec![14]),
                (0x22, 0, vec![40]),
                (0x23, 0, vec![9]),
                (0x81, 0, vec![0, 5, 15, 30, 60, 180]),
            ] {
                let original = session.request(event, None)?;
                let verify = (|| -> Result<(), Box<dyn std::error::Error>> {
                    for value in values {
                        let value = if event == 0x21 {
                            original[index].saturating_sub(1)
                        } else if event == 0x23 {
                            value.min(original[index])
                        } else {
                            value
                        };
                        let mut next = original.clone();
                        next[index] = value;
                        let ack = session.request(event, Some(&next))?;
                        let read = session.request(event, None)?;
                        println!("verify {event:02X}={value}: ack={ack:02X?} read={read:02X?}");
                        if ack.get(index) != Some(&value) || read.get(index) != Some(&value) {
                            return Err("INZONE write readback differs".into());
                        }
                    }
                    Ok(())
                })();
                let restored = session.request(event, Some(&original));
                let actual = session.request(event, None);
                println!(
                    "restore {event:02X}: {}",
                    restored.is_ok()
                        && actual
                            .as_ref()
                            .is_ok_and(|v| v.get(index) == original.get(index))
                );
                restored?;
                let actual = actual?;
                if actual.get(index) != original.get(index) {
                    return Err("INZONE restore needs attention".into());
                }
                verify?;
            }
        }
        session.close();
    }
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("INZONE hardware probing requires Windows");
}
