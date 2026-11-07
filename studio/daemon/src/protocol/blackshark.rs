//! BlackShark V2 HyperSpeed output/input report protocol.

use crate::{Error, Result};

pub const PID_WIRELESS: u16 = 0x0565;
pub const PID_WIRED: u16 = 0x056e;
pub const REPORT_SIZE: usize = 64;
pub const MAX_PAYLOAD: usize = 48;
pub const INTERFACE: u8 = 3;
pub const USAGE_PAGE: u16 = 0xff14;
pub const OTHER_USAGE_PAGE: u16 = 0xff00;
pub const DOMAIN_LOCAL: u8 = 0;
pub const DOMAIN_HEADSET: u8 = 0x80;
pub const WRITE: u8 = 0x80;
pub const SERIAL: u8 = 0x00;
pub const FIRMWARE: u8 = 0x02;
pub const EQ_PRESET: u8 = 0x13;
pub const EQ_BANDS: u8 = 0x15;
pub const SIDETONE: u8 = 0x18;
pub const SIDETONE_LEVEL: u8 = 0x19;
pub const EQ_ENABLED: u8 = 0x1e;
pub const BATTERY: u8 = 0x21;
pub const DO_NOT_DISTURB: u8 = 0x27;
pub const CHARGING: u8 = 0x2a;
pub const IDLE_MINUTES: u8 = 0x2c;
pub const MIC_MUTED: u8 = 0x55;
pub const DONGLE_LED: u8 = 0x66;
pub const EQ_PRESETS: [(&str, u8); 5] = [
    ("flat", 0),
    ("game", 7),
    ("music", 8),
    ("movie", 9),
    ("custom", 255),
];
pub const EQ_FREQUENCIES: [u32; 10] = [31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
pub const DONGLE_LED_MODES: [(&str, u8); 4] =
    [("off", 0), ("link", 1), ("battery", 2), ("warning", 3)];

pub fn request(sequence: u8, domain: u8, command: u8, payload: &[u8]) -> Result<[u8; REPORT_SIZE]> {
    if payload.len() > MAX_PAYLOAD {
        return Err(Error::Invalid("BlackShark request exceeds 48 bytes".into()));
    }
    let mut report = [0; REPORT_SIZE];
    report[0] = 2;
    report[2] = 0x60 | (sequence & 31);
    report[6] = 4 + payload.len() as u8;
    report[9] = domain;
    report[10] = command;
    report[12] = payload.len() as u8;
    report[13..13 + payload.len()].copy_from_slice(payload);
    report[62] = xor(&report[..62]);
    Ok(report)
}
fn xor(data: &[u8]) -> u8 {
    data.iter().fold(0, |sum, byte| sum ^ byte)
}
pub fn is_reply(report: &[u8]) -> bool {
    if report.len() < REPORT_SIZE
        || report[0] != 2
        || report[8] != 0x80
        || usize::from(report[12]) > MAX_PAYLOAD
        || report[6] != 4 + report[12]
    {
        return false;
    }
    let checksum = xor(&report[1..62]);
    report[62] == checksum || report[62] == checksum ^ 2
}
pub fn answers(report: &[u8], sequence: u8, domain: u8, command: u8, wired: bool) -> bool {
    is_reply(report)
        && report[10] == command
        && report[2] & 31 == sequence & 31
        && (report[9] == domain || wired && domain == DOMAIN_LOCAL && report[9] == DOMAIN_HEADSET)
}
pub fn result(report: &[u8], command: u8) -> Result<&[u8]> {
    if !is_reply(report) {
        return Err(Error::Device("invalid BlackShark response".into()));
    }
    match report[1] {
        5 => {
            return Err(Error::NotSupported(format!(
                "command 0x{command:02x} is not supported"
            )))
        }
        1 => return Err(Error::Device("the headset is busy".into())),
        4 => {
            return Err(Error::Device(
                "the headset did not answer the receiver".into(),
            ))
        }
        2 if report[11] == 1 => {}
        status => {
            return Err(Error::Device(format!(
                "command 0x{command:02x} failed (status 0x{status:02x})"
            )))
        }
    }
    let payload = &report[13..13 + usize::from(report[12])];
    if command & WRITE != 0 && payload != [0] {
        return Err(Error::Device(format!(
            "command 0x{command:02x} was refused"
        )));
    }
    Ok(payload)
}
pub fn encode_bands(gains: &[i64]) -> Result<[u8; 10]> {
    if gains.len() != 10 {
        return Err(Error::Invalid("the equalizer requires ten bands".into()));
    }
    let mut encoded = [0; 10];
    for (to, from) in encoded.iter_mut().zip(gains) {
        if !(-9..=6).contains(from) {
            return Err(Error::Invalid("equalizer gain must be -9..6 dB".into()));
        }
        *to = (from + 5) as u8;
    }
    Ok(encoded)
}
pub fn decode_bands(payload: &[u8]) -> Result<[i8; 10]> {
    if payload.len() != 10 {
        return Err(Error::Device("expected ten equalizer bands".into()));
    }
    let mut gains = [0; 10];
    for (to, from) in gains.iter_mut().zip(payload) {
        *to = *from as i8;
    }
    Ok(gains)
}
