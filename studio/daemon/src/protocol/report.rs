//! Razer's 90-byte vendor control report, excluding the HID report ID.

use crate::{Error, Result};

pub const REPORT_SIZE: usize = 90;
pub const ARGUMENT_SIZE: usize = 80;
pub const VENDOR_ID: u16 = 0x1532;
pub const BUSY: u8 = 1;
pub const SUCCESSFUL: u8 = 2;
pub const NOT_SUPPORTED: u8 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub status: u8,
    pub transaction_id: u8,
    pub remaining_packets: u16,
    pub protocol_type: u8,
    pub data_size: u8,
    pub command_class: u8,
    pub command_id: u8,
    pub arguments: [u8; ARGUMENT_SIZE],
    pub reserved: u8,
}

impl Default for Report {
    fn default() -> Self {
        Self::new(0, 0, 0)
    }
}

impl Report {
    pub fn new(command_class: u8, command_id: u8, data_size: u8) -> Self {
        Self {
            status: 0,
            transaction_id: 0,
            remaining_packets: 0,
            protocol_type: 0,
            data_size,
            command_class,
            command_id,
            arguments: [0; ARGUMENT_SIZE],
            reserved: 0,
        }
    }

    pub fn pack(&self) -> [u8; REPORT_SIZE] {
        let mut bytes = [0; REPORT_SIZE];
        bytes[0] = self.status;
        bytes[1] = self.transaction_id;
        bytes[2..4].copy_from_slice(&self.remaining_packets.to_be_bytes());
        bytes[4] = self.protocol_type;
        bytes[5] = self.data_size;
        bytes[6] = self.command_class;
        bytes[7] = self.command_id;
        bytes[8..88].copy_from_slice(&self.arguments);
        bytes[88] = bytes[2..88].iter().fold(0, |crc, byte| crc ^ byte);
        bytes[89] = self.reserved;
        bytes
    }

    pub fn unpack(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < REPORT_SIZE {
            return Err(Error::Invalid(format!(
                "report too short: {} bytes, expected {REPORT_SIZE}",
                bytes.len()
            )));
        }
        let mut arguments = [0; ARGUMENT_SIZE];
        arguments.copy_from_slice(&bytes[8..88]);
        // As in the original driver, do not reject replies from devices that
        // return a stale CRC. Matching is done against command and packet ID.
        Ok(Self {
            status: bytes[0],
            transaction_id: bytes[1],
            remaining_packets: u16::from_be_bytes([bytes[2], bytes[3]]),
            protocol_type: bytes[4],
            data_size: bytes[5],
            command_class: bytes[6],
            command_id: bytes[7],
            arguments,
            reserved: bytes[89],
        })
    }

    pub fn set(&mut self, index: usize, value: i64) {
        if let Some(byte) = self.arguments.get_mut(index) {
            *byte = value as u8;
        }
    }

    pub fn set_bytes(&mut self, index: usize, values: &[u8]) {
        if index >= ARGUMENT_SIZE {
            return;
        }
        let size = values.len().min(ARGUMENT_SIZE - index);
        self.arguments[index..index + size].copy_from_slice(&values[..size]);
    }

    pub fn matches(&self, other: &Self) -> bool {
        self.remaining_packets == other.remaining_packets
            && self.command_class == other.command_class
            && self.command_id == other.command_id
    }

    pub fn payload(&self) -> &[u8] {
        &self.arguments[..usize::from(self.data_size).min(ARGUMENT_SIZE)]
    }
}

pub fn status_name(status: u8) -> String {
    match status {
        0 => "new command".into(),
        1 => "busy".into(),
        2 => "successful".into(),
        3 => "failure".into(),
        4 => "timeout".into(),
        5 => "not supported".into(),
        _ => format!("unknown (0x{status:02x})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_wire_layout_and_checksum() {
        let mut report = Report::new(3, 10, 4);
        report.transaction_id = 255;
        report.set_bytes(0, &[6, 255, 0, 0]);
        let bytes = report.pack();
        assert_eq!(&bytes[..12], &[0, 255, 0, 0, 0, 4, 3, 10, 6, 255, 0, 0]);
        assert_eq!(bytes[88], 0xf4);
        assert_eq!(Report::unpack(&bytes).unwrap(), report);
        assert!(Report::unpack(&bytes[..89]).is_err());
    }

    #[test]
    fn writes_cannot_cross_the_payload_boundary() {
        let mut report = Report::default();
        report.set_bytes(79, &[1, 2, 3]);
        report.set_bytes(usize::MAX, &[5]);
        report.set(80, 7);
        assert_eq!(report.arguments[79], 1);
        assert_eq!(report.pack().len(), 90);
    }
}
