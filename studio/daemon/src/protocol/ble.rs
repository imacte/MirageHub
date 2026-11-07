//! The vendor BLE frames observed on the Kraken Kitty V2 BT. These are sent
//! to the write-without-response characteristic, with replies via notifications.

use crate::{Error, Result};

pub const SERVICE_UUID: &str = "416d656c-2d52-617a-6572-424c4501f40a";
pub const WRITE_CHARACTERISTIC: &str = "416d0000-2d52-617a-6572-424c4501f40a";
pub const NOTIFY_CHARACTERISTIC: &str = "416d0001-2d52-617a-6572-424c4501f40a";
pub const COMPANY_ID: u16 = 0x068e;
pub const RESPONSE: u8 = 1;
pub const PUSH: u8 = 2;
pub const STATUS: u8 = 0x20;
pub const BATTERY: u8 = 0x21;
pub const IDLE_TIMER: u8 = 0x27;
pub const CHARGING: u8 = 0x29;
pub const TAKEOVER: u8 = 0xc0;
pub const BRIGHTNESS: u8 = 0xc1;
pub const COLOUR: u8 = 0xc4;
pub const SET_IDLE_TIMER: u8 = 0xa7;
pub const ZONES: [&str; 2] = ["left", "right"];

pub fn message(opcode: u8, payload: &[u8], kind: u8) -> Result<Vec<u8>> {
    let length = u8::try_from(payload.len())
        .map_err(|_| Error::Invalid("BLE payload exceeds 255 bytes".into()))?;
    let mut message = Vec::with_capacity(3 + payload.len());
    message.extend_from_slice(&[opcode, kind, length]);
    message.extend_from_slice(payload);
    Ok(message)
}

pub fn read_request(opcode: u8) -> [u8; 3] {
    [opcode, 0, 0]
}
pub fn read_opcode(write: u8) -> u8 {
    write & !0x80
}
pub fn write_opcode(read: u8) -> u8 {
    read | 0x80
}

pub fn parse_response(data: &[u8]) -> Result<(u8, u8, &[u8])> {
    if data.len() < 3 {
        return Err(Error::Invalid("truncated BLE response".into()));
    }
    let payload = data
        .get(3..3 + usize::from(data[2]))
        .ok_or_else(|| Error::Invalid("BLE response shorter than declared payload".into()))?;
    Ok((data[0], data[1], payload))
}

pub fn brightness_command(level: i64) -> [u8; 4] {
    [BRIGHTNESS, 0, 1, level.clamp(0, 255) as u8]
}
pub fn takeover_command() -> [u8; 4] {
    [TAKEOVER, 0, 1, 5]
}
pub fn percentage_to_level(percent: f64) -> Result<u8> {
    if !percent.is_finite() {
        return Err(Error::Invalid("brightness must be finite".into()));
    }
    Ok((percent.clamp(0.0, 100.0) * 255.0 / 100.0).round_ties_even() as u8)
}
pub fn level_to_percentage(level: i64) -> f64 {
    (level.clamp(0, 255) as f64 * 1000.0 / 255.0).round_ties_even() / 10.0
}

pub fn colour_command(colours: &[[u8; 3]]) -> Result<Vec<u8>> {
    let (left, right) = match colours {
        [single] => (single, single),
        [left, right] => (left, right),
        _ => {
            return Err(Error::Invalid(
                "BLE lighting requires one or two colours".into(),
            ))
        }
    };
    let mut payload = Vec::with_capacity(6);
    payload.extend_from_slice(left);
    payload.extend_from_slice(right);
    message(COLOUR, &payload, 0)
}
pub fn off_command() -> [u8; 9] {
    [COLOUR, 0, 6, 0, 0, 0, 0, 0, 0]
}
pub fn product_id(data: &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes([*data.first()?, *data.get(1)?]))
}
pub fn classic_address(data: &[u8]) -> Option<u64> {
    Some(
        data.get(3..9)?
            .iter()
            .rev()
            .fold(0, |address, byte| (address << 8) | u64::from(*byte)),
    )
}
