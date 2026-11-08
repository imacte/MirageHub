// SPDX-License-Identifier: GPL-2.0-or-later
//! Naga V2 Pro onboard commands, ported from Daniel Hobley's captures in
//! openrazer/openrazer#2962, commit 73071e54b3eb6f657518ea95fd223d22e97b2432.
//! Stores 1..5, standard/Hypershift layers; store 1 is live and nonvolatile.
//! BUSY acknowledges a write but carries no readable data. Never retry writes.
use super::report::{status_name, Report, BUSY, SUCCESSFUL};
use crate::{transport::DeviceTransport, Error, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    thread,
    time::{Duration, Instant},
};

pub const BUTTONS: [u8; 30] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x09, 0x0a, 0x0e, 0x34, 0x35, 0x60, 0x6d, 0x40, 0x41, 0x42, 0x43,
    0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55,
];
pub const MAX_MACRO_BYTES: usize = 4094;
pub const METHODS: &[&str] = &[
    "get_side_plate",
    "get_onboard_buttons",
    "get_button_action",
    "set_button_action",
    "set_button_key",
    "set_button_mouse",
    "set_button_macro",
    "get_onboard_macros",
    "get_onboard_macro_count",
    "get_onboard_macro_length",
    "get_onboard_macro_inventory",
    "get_onboard_macro_storage",
    "add_onboard_macro",
    "delete_onboard_macro",
];
const READ_TRIES: usize = 40;
const BUSY_WAIT: Duration = Duration::from_millis(25);
pub fn supported(pid: u16) -> bool {
    matches!(pid, 0x00a7 | 0x00a8)
}

pub fn button(store: u8, button: u8, layer: u8) -> Result<()> {
    if !(1..=5).contains(&store) {
        return Err(Error::Invalid("store must be 1..5 (1 is live)".into()));
    }
    if !BUTTONS.contains(&button) {
        return Err(Error::Invalid(format!(
            "unsupported onboard button 0x{button:02x}"
        )));
    }
    if layer > 1 {
        return Err(Error::Invalid(
            "layer must be 0 (standard) or 1 (Hypershift)".into(),
        ));
    }
    Ok(())
}
pub fn macro_id(id: u16) -> Result<()> {
    if id == 0 {
        return Err(Error::Invalid("macro ID must be 1..65535".into()));
    }
    Ok(())
}
pub fn validate_events(events: &[u8]) -> Result<()> {
    if events.is_empty() || events.len() > MAX_MACRO_BYTES {
        return Err(Error::Invalid(
            "macro events must contain 1..4094 bytes".into(),
        ));
    }
    let mut offset = 0;
    while offset < events.len() {
        let length = match events[offset] {
            0x01 | 0x02 | 0x11 => 2,
            0x12 => 3,
            code => {
                return Err(Error::Invalid(format!(
                    "unknown macro event 0x{code:02x} at byte {offset}"
                )))
            }
        };
        if offset + length > events.len() {
            return Err(Error::Invalid("truncated macro event".into()));
        }
        offset += length;
    }
    Ok(())
}

pub struct Onboard<'a> {
    transport: &'a mut dyn DeviceTransport,
    deadline: Instant,
}
impl<'a> Onboard<'a> {
    pub fn new(transport: &'a mut dyn DeviceTransport) -> Self {
        Self {
            transport,
            deadline: Instant::now() + Duration::from_secs(30),
        }
    }
    fn send(&mut self, class: u8, command: u8, size: u8, arguments: &[u8]) -> Result<Report> {
        let mut request = Report::new(class, command, size);
        request.transaction_id = 0x1f;
        request.set_bytes(0, arguments);
        let read = command & 0x80 != 0;
        for attempt in 0..READ_TRIES {
            if Instant::now() >= self.deadline {
                return Err(Error::Device(
                    "onboard operation exceeded 30 seconds".into(),
                ));
            }
            let reply = self.transport.exchange_once(&request)?;
            if !request.matches(&reply) {
                return Err(Error::Device(
                    "onboard response does not match request".into(),
                ));
            }
            if reply.status == SUCCESSFUL || (!read && reply.status == BUSY) {
                if read && reply.data_size < size {
                    return Err(Error::Device("short onboard response".into()));
                }
                return Ok(reply);
            }
            if reply.status != BUSY {
                return Err(Error::Device(format!(
                    "onboard command replied {}",
                    status_name(reply.status)
                )));
            }
            if attempt + 1 < READ_TRIES {
                thread::sleep(BUSY_WAIT);
            }
        }
        Err(Error::Device("onboard memory remained busy".into()))
    }
    pub fn side_plate(&mut self) -> Result<u8> {
        Ok(self.send(0, 0xb9, 1, &[])?.arguments[0])
    }
    pub fn get_action(&mut self, store: u8, id: u8, layer: u8) -> Result<Vec<u8>> {
        button(store, id, layer)?;
        let a = self.send(2, 0x8c, 10, &[store, id, layer])?.arguments;
        if a[0] != store || a[1] != id {
            return Err(Error::Device(
                "onboard reply selected another button".into(),
            ));
        }
        // Byte 2 is the Hypershift-key flag, not an echo of the selected layer.
        // Factory actions can claim length 1 while carrying extra nonzero data.
        if a[4] > 5 {
            return Err(Error::Device("invalid onboard action length".into()));
        }
        let last = a[..10].iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
        let end = (5 + usize::from(a[4])).max(last);
        Ok(a[3..end].to_vec())
    }
    pub fn set_action(&mut self, store: u8, id: u8, layer: u8, action: &[u8]) -> Result<()> {
        button(store, id, layer)?;
        if !(2..=7).contains(&action.len()) || usize::from(action[1]) > action.len() - 2 {
            return Err(Error::Invalid(
                "action must contain type, length and 0..5 data bytes".into(),
            ));
        }
        if matches!(action[0], 3 | 4) {
            if action.len() < 4 {
                return Err(Error::Invalid("macro action needs a two-byte ID".into()));
            }
            let macro_id = u16::from_be_bytes([action[2], action[3]]);
            if self.macro_length(macro_id)? == 0 {
                return Err(Error::Invalid(format!(
                    "macro {macro_id} is not stored on the device"
                )));
            }
        } else {
            // Wait out an earlier flash operation before issuing a new write.
            self.side_plate()?;
        }
        let mut args = vec![store, id, layer];
        args.extend_from_slice(action);
        let written = self.send(2, 0x0c, 10, &args);
        // A missing acknowledgement can still mean the binding was changed.
        // Release a possibly held old key without ever resending the binding.
        let released = if store == 1 {
            self.release_keys()
        } else {
            Ok(())
        };
        written?;
        released?;
        // Drain BUSY even for inactive stores before the next command.
        self.get_action(store, id, layer)?;
        Ok(())
    }
    fn release_keys(&mut self) -> Result<()> {
        if self.send(0, 0x84, 2, &[])?.arguments[0] != 0 {
            return Ok(());
        }
        let entered = self.send(0, 4, 2, &[3, 0]);
        // If the first acknowledgement was lost, still try to restore normal
        // mode. Each of these writes is sent exactly once.
        let left = self.send(0, 4, 2, &[0, 0]);
        entered?;
        left?;
        Ok(())
    }
    pub fn macro_count(&mut self) -> Result<u16> {
        let a = self.send(6, 0x80, 2, &[])?.arguments;
        Ok(u16::from_be_bytes([a[0], a[1]]))
    }
    pub fn macro_length(&mut self, id: u16) -> Result<u32> {
        macro_id(id)?;
        let a = self.send(6, 0x88, 6, &id.to_be_bytes())?.arguments;
        Ok(u32::from_be_bytes(a[2..6].try_into().unwrap()))
    }
    pub fn inventory(&mut self) -> Result<Value> {
        let count = self.macro_count()?;
        if count > 2048 {
            return Err(Error::Device("invalid onboard macro count".into()));
        }
        let mut ids = BTreeSet::new();
        for start in (0..count).step_by(38) {
            let n = (count - start).min(38);
            let a = self
                .send(6, 0x8b, (4 + 2 * n) as u8, &start.to_be_bytes())?
                .arguments;
            for pair in a[4..4 + 2 * usize::from(n)].chunks_exact(2) {
                let id = u16::from_be_bytes([pair[0], pair[1]]);
                if id != 0 {
                    ids.insert(id);
                }
            }
        }
        // The firmware can omit live IDs after deletion; do not present an
        // incomplete list as evidence that an ID is unused.
        Ok(json!({"count":count,"complete":ids.len()==usize::from(count),"ids":ids}))
    }
    pub fn storage(&mut self) -> Result<Value> {
        let a = self.send(6, 0x8e, 14, &[])?.arguments;
        let size = |offset| u32::from_be_bytes(a[offset..offset + 4].try_into().unwrap());
        Ok(
            json!({"max_macros":u16::from_be_bytes([a[0],a[1]]),"total":size(2),"free":size(6),"deleted":size(10)}),
        )
    }
    pub fn upload(&mut self, id: u16, events: &[u8]) -> Result<()> {
        macro_id(id)?;
        validate_events(events)?;
        if self.macro_length(id)? != 0 {
            return Err(Error::Invalid(format!(
                "macro {id} already exists; choose an unused ID"
            )));
        }
        let storage = self.storage()?;
        if storage["free"].as_u64().unwrap_or(0) < events.len() as u64 {
            return Err(Error::Device("not enough free onboard macro memory".into()));
        }
        if u64::from(self.macro_count()?) >= storage["max_macros"].as_u64().unwrap_or(0) {
            return Err(Error::Device("onboard macro slots are full".into()));
        }
        let mut create = id.to_be_bytes().to_vec();
        create.extend_from_slice(&(events.len() as u32).to_be_bytes());
        let uploaded = (|| -> Result<()> {
            self.send(6, 8, 6, &create)?;
            for (chunk, bytes) in events.chunks(73).enumerate() {
                let mut args = id.to_be_bytes().to_vec();
                args.extend_from_slice(&((chunk * 73) as u32).to_be_bytes());
                args.push(bytes.len() as u8);
                args.extend_from_slice(bytes);
                self.send(6, 9, args.len() as u8, &args)?;
            }
            self.wait_length(id, events.len() as u32)
        })();
        if let Err(error) = uploaded {
            // Best effort cleanup of this newly allocated, unbound ID. Wait
            // for the flash first, then delete once; never repeat an upload.
            let cleanup = self
                .macro_count()
                .and_then(|_| self.send(6, 3, 2, &id.to_be_bytes()));
            return Err(Error::Device(format!(
                "macro upload failed: {error}{}",
                cleanup.err().map_or(String::new(), |e| format!(
                    "; partial macro {id} could not be removed: {e}"
                ))
            )));
        }
        Ok(())
    }
    fn wait_length(&mut self, id: u16, expected: u32) -> Result<()> {
        for attempt in 0..READ_TRIES {
            if self.macro_length(id)? == expected {
                return Ok(());
            }
            if attempt + 1 < READ_TRIES {
                thread::sleep(BUSY_WAIT);
            }
        }
        Err(Error::Device(format!(
            "macro {id} did not read back with length {expected}"
        )))
    }
    pub fn delete(&mut self, id: u16) -> Result<()> {
        macro_id(id)?;
        // Scan every side plate, both layers and every store, not just the
        // attached plate or the unreliable macro list.
        for store in 1..=5 {
            for button in BUTTONS {
                for layer in 0..=1 {
                    let action = self.get_action(store, button, layer)?;
                    if matches!(action[0], 3 | 4)
                        && action.len() >= 4
                        && u16::from_be_bytes([action[2], action[3]]) == id
                    {
                        return Err(Error::Invalid(format!("macro {id} is still bound to store {store}, button 0x{button:02x}, layer {layer}; rebind it first")));
                    }
                }
            }
        }
        self.send(6, 3, 2, &id.to_be_bytes())?;
        self.wait_length(id, 0)
    }
}
