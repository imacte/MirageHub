// SPDX-License-Identifier: GPL-3.0-only
// INZONE protocol port based on OpenInzone, Copyright (C) 2026 penguinwokrs.
//! Sony's HCI-shaped control protocol. No Razer reports reach these devices.

use crate::{Error, Result};
use std::collections::BTreeMap;

pub const VENDOR: u16 = 0x054c;
pub const PRODUCTS: &[u16] = &[
    0x0dfd, 0x0e47, 0x0e53, 0x0ebf, 0x0ec2, 0x0f80, 0x0f81, 0x0fa8, 0x0fc0, 0x0fc1,
];
pub const METHODS: &[&str] = &[
    "set_headphone_volume",
    "set_headphone_muted",
    "set_game_chat_balance",
    "set_microphone_muted",
    "set_microphone_level",
    "set_ambient_mode",
    "set_ambient_level",
    "set_voice_focus",
    "set_auto_power_off",
    "set_voice_guidance",
    "set_voice_guidance_language",
    "set_bluetooth_auto_switch",
    "get_battery_parts",
];

pub fn destination(event: u8) -> u8 {
    if matches!(event, 0x01 | 0x09) {
        2
    } else {
        4
    }
}

pub fn command(event: u8, set: bool, transaction: u16, param: &[u8]) -> Result<Vec<u8>> {
    if param.len() > 50 {
        return Err(Error::Invalid(
            "INZONE parameter exceeds one command".into(),
        ));
    }
    let mut bytes = vec![
        1,
        0,
        0xfc,
        (8 + param.len()) as u8,
        0x96,
        0xc3,
        destination(event) << 4 | 1,
        event,
        if set { 2 } else { 1 },
        transaction as u8,
        (transaction >> 8) as u8,
    ];
    bytes.extend_from_slice(param);
    bytes.push(bytes[4..].iter().fold(0u8, |sum, b| sum.wrapping_add(*b)));
    Ok(bytes)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Packet {
    pub source: u8,
    pub destination: u8,
    pub event: u8,
    pub kind: u8,
    pub transaction: u16,
    pub param: Vec<u8>,
}
impl Packet {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 12
            || data[0] != 4
            || data[1] != 0xff
            || data[2] as usize + 3 != data.len()
            || data[3] != 0
            || data[4..6] != [0x96, 0xc3]
            || data[3..data.len() - 1]
                .iter()
                .fold(0u8, |sum, b| sum.wrapping_add(*b))
                != *data.last()?
        {
            return None;
        }
        Some(Self {
            source: data[6] & 15,
            destination: data[6] >> 4,
            event: data[7],
            kind: data[8],
            transaction: u16::from_le_bytes([data[9], data[10]]),
            param: data[11..data.len() - 1].to_vec(),
        })
    }
    pub fn matches(&self, event: u8, transaction: u16, set: bool) -> bool {
        self.source == destination(event)
            && self.destination == 1
            && self.event == event
            && self.transaction == transaction
            && self.kind == if set { 0x20 } else { 0x10 }
    }
}

#[derive(Default)]
pub struct Reassembler {
    bytes: Vec<u8>,
}
impl Reassembler {
    pub fn add(&mut self, input: &[u8]) -> Vec<Packet> {
        self.bytes.extend_from_slice(input);
        let mut packets = Vec::new();
        while !self.bytes.is_empty() {
            if self.bytes[0] != 4 || (self.bytes.len() > 1 && self.bytes[1] != 0xff) {
                self.bytes.remove(0);
                continue;
            }
            if self.bytes.len() < 3 {
                break;
            }
            let len = self.bytes[2] as usize + 3;
            if len < 12 {
                self.bytes.remove(0);
                continue;
            }
            if self.bytes.len() < len {
                break;
            }
            if let Some(packet) = Packet::parse(&self.bytes[..len]) {
                packets.push(packet);
                self.bytes.drain(..len);
            } else {
                self.bytes.remove(0);
            }
        }
        packets
    }
}

pub fn hid_reports(packet: &[u8]) -> Vec<Vec<u8>> {
    packet
        .chunks(62)
        .map(|chunk| {
            let mut report = vec![0; 64];
            report[0] = 2;
            report[1] = chunk.len() as u8;
            report[2..2 + chunk.len()].copy_from_slice(chunk);
            report
        })
        .collect()
}
pub fn hid_payload(report: &[u8]) -> Option<&[u8]> {
    if report.len() != 64 || report[0] != 2 || report[1] > 62 {
        return None;
    }
    Some(&report[2..2 + report[1] as usize])
}

pub fn model_name(id: u8) -> Option<&'static str> {
    [
        "Sony INZONE H9",
        "Sony INZONE H7",
        "Sony INZONE H3",
        "Sony INZONE H5",
        "Sony INZONE Buds",
        "Sony INZONE H9 II",
        "Sony INZONE E9",
        "Sony INZONE H6 Air",
    ]
    .get(id as usize)
    .copied()
}

/// Unknown maps remain unknown; FF slots are the only definite absence.
pub fn capability_map(parts: [Option<&[u8]>; 3], model: Option<u8>) -> BTreeMap<u8, Vec<u8>> {
    let mut map = BTreeMap::new();
    let mut take = |part: Option<&[u8]>, layout: &[(u8, usize)], offset: usize, short: bool| {
        let Some(part) = part else { return };
        let mut end = offset;
        let mut taken = 0;
        for (_, width) in layout {
            if end + width > part.len() {
                break;
            }
            end += width;
            taken += 1;
        }
        if taken == 0 || end != part.len() || (taken < layout.len() && !short) {
            return;
        }
        let mut pos = offset;
        for (index, &(event, width)) in layout.iter().enumerate() {
            let bytes = if index < taken {
                let bytes = part[pos..pos + width].to_vec();
                pos += width;
                bytes
            } else {
                vec![0xff; width]
            };
            map.insert(event, bytes);
        }
    };
    if let Some(part) = parts[0] {
        if let Some(battery @ (2 | 6)) = part.len().checked_sub(7) {
            take(
                Some(part),
                &[(4, battery), (0x21, 3), (0x22, 1), (0x23, 2)],
                1,
                false,
            );
        }
    }
    take(
        parts[1],
        &[(0x24, 3), (0x41, 4), (0x42, 3)],
        0,
        model == Some(0),
    );
    // H5's eight-byte part 3 was captured and cross-checked on 054C:0EBF.
    let short3 =
        model == Some(0) || (model == Some(3) && parts[2].is_some_and(|part| part.len() == 8));
    take(
        parts[2],
        &[
            (0x43, 1),
            (0x61, 4),
            (0x81, 1),
            (0x82, 1),
            (0x83, 1),
            (0x84, 1),
            (0x85, 1),
        ],
        0,
        short3,
    );
    map
}
