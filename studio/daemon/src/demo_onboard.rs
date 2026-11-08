//! Stateful onboard memory for the HID emulator. Includes BUSY flash replies
//! and the firmware's incomplete macro list after deletion.
use crate::protocol::report::{Report, BUSY, SUCCESSFUL};
use std::collections::BTreeMap;

struct Macro {
    bytes: Vec<u8>,
    received: usize,
}
#[derive(Default)]
pub(super) struct State {
    buttons: BTreeMap<(u8, u8, u8), Vec<u8>>,
    macros: BTreeMap<u16, Macro>,
    deleted: u32,
    busy: u8,
    list_hole: bool,
}
impl State {
    pub fn answer(&mut self, request: &Report) -> Option<Report> {
        let command = (request.command_class, request.command_id);
        if !matches!(
            command,
            (0, 0xb9) | (2, 0x0c | 0x8c) | (6, 0x80 | 0x8b | 0x88 | 0x8e | 8 | 9 | 3)
        ) {
            return None;
        }
        let mut reply = request.clone();
        reply.status = SUCCESSFUL;
        reply.arguments.fill(0);
        if self.busy > 0 {
            if request.command_id & 0x80 != 0 {
                self.busy -= 1;
            }
            reply.status = BUSY;
            return Some(reply);
        }
        let a = &request.arguments;
        let id = u16::from_be_bytes([a[0], a[1]]);
        match command {
            (0, 0xb9) => reply.arguments[0] = 4, // six-button plate
            (2, 0x0c) => {
                self.buttons.insert((a[0], a[1], a[2]), a[3..10].to_vec());
            }
            (2, 0x8c) => {
                reply.set_bytes(0, &[a[0], a[1], 0]);
                let default = if a[1] <= 5 {
                    vec![1, 1, a[1]]
                } else {
                    vec![0, 0]
                };
                let action = self.buttons.get(&(a[0], a[1], a[2])).unwrap_or(&default);
                reply.set_bytes(3, action);
                // Hypershift key flag is not the requested layer.
                reply.arguments[2] = u8::from(action[0] == 0x0c);
            }
            (6, 0x80) => reply.set_bytes(0, &(self.macros.len() as u16).to_be_bytes()),
            (6, 0x8b) => {
                reply.set_bytes(0, &id.to_be_bytes());
                reply.set_bytes(2, &(self.macros.len() as u16).to_be_bytes());
                let count = usize::from(request.data_size.saturating_sub(4)) / 2;
                for (index, macro_id) in self
                    .macros
                    .keys()
                    .enumerate()
                    .skip(usize::from(id))
                    .take(count)
                {
                    if !(self.list_hole && index == 0) {
                        reply.set_bytes(4 + 2 * (index - usize::from(id)), &macro_id.to_be_bytes());
                    }
                }
            }
            (6, 0x88) => {
                reply.set_bytes(0, &id.to_be_bytes());
                let length = self
                    .macros
                    .get(&id)
                    .filter(|m| m.received == m.bytes.len())
                    .map_or(0, |m| m.bytes.len() as u32);
                reply.set_bytes(2, &length.to_be_bytes());
            }
            (6, 0x8e) => {
                let used = self
                    .macros
                    .values()
                    .map(|m| m.bytes.len() as u32)
                    .sum::<u32>();
                reply.set_bytes(0, &256u16.to_be_bytes());
                reply.set_bytes(2, &65536u32.to_be_bytes());
                reply.set_bytes(
                    6,
                    &65536u32.saturating_sub(used + self.deleted).to_be_bytes(),
                );
                reply.set_bytes(10, &self.deleted.to_be_bytes());
            }
            (6, 8) => {
                let length = u32::from_be_bytes(a[2..6].try_into().unwrap()) as usize;
                if length > crate::protocol::onboard::MAX_MACRO_BYTES
                    || self.macros.contains_key(&id)
                {
                    reply.status = 3;
                } else {
                    self.macros.insert(
                        id,
                        Macro {
                            bytes: vec![0; length],
                            received: 0,
                        },
                    );
                    reply.status = BUSY; // accepted once, not a reason to resend
                }
            }
            (6, 9) => {
                let offset = u32::from_be_bytes(a[2..6].try_into().unwrap()) as usize;
                let length = usize::from(a[6]);
                if let Some(mac) = self.macros.get_mut(&id).filter(|m| {
                    length <= 73 && offset == m.received && offset + length <= m.bytes.len()
                }) {
                    mac.bytes[offset..offset + length].copy_from_slice(&a[7..7 + length]);
                    mac.received += length;
                    if mac.received == mac.bytes.len() {
                        self.busy = 2;
                        reply.status = BUSY;
                    }
                } else {
                    reply.status = 3;
                }
            }
            (6, 3) => {
                if let Some(mac) = self.macros.remove(&id) {
                    self.deleted += mac.bytes.len() as u32;
                    self.list_hole = true;
                }
            }
            _ => unreachable!(),
        }
        Some(reply)
    }
}
