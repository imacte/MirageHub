//! Kraken controller RAM writes. Each report includes the HID report ID.

use crate::{Error, Result};

pub const REPORT_SIZE: usize = 37;
pub const REPORT_ID: u8 = 4;
pub const WRITE_SETTLE: std::time::Duration = std::time::Duration::from_millis(15);
pub type Packet = [u8; REPORT_SIZE];

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub led_mode: u16,
    pub custom: u16,
    pub breathing: &'static [u16],
}
pub const KYLIE: Layout = Layout {
    led_mode: 0x172d,
    custom: 0x1189,
    breathing: &[0x1741, 0x1745, 0x174d],
};
pub const RAINIE: Layout = Layout {
    led_mode: 0x1008,
    custom: 0x1189,
    breathing: &[0x15de],
};

pub fn layout_for(pid: u16) -> Option<Layout> {
    match pid {
        0x0501 | 0x0506 | 0x0504 => Some(RAINIE),
        0x0510 | 0x0520 | 0x0527 | 0x0560 => Some(KYLIE),
        _ => None,
    }
}
pub fn has_colour(pid: u16) -> bool {
    ![0x0501, 0x0506].contains(&pid)
}

pub fn request(destination: u8, length: usize, address: u16, arguments: &[u8]) -> Result<Packet> {
    if length > 32 {
        return Err(Error::Invalid("Kraken request exceeds 32 bytes".into()));
    }
    let mut report = [0; REPORT_SIZE];
    report[..5].copy_from_slice(&[
        REPORT_ID,
        destination,
        length as u8,
        (address >> 8) as u8,
        address as u8,
    ]);
    let size = arguments.len().min(32);
    report[5..5 + size].copy_from_slice(&arguments[..size]);
    Ok(report)
}
pub fn write_ram(address: u16, values: &[u8]) -> Result<Packet> {
    request(0x40, values.len(), address, values)
}
pub fn read_ram(address: u16, length: usize) -> Result<Packet> {
    request(0, length, address, &[])
}
pub fn set_effect(layout: Layout, effect: u8) -> Result<Packet> {
    write_ram(layout.led_mode, &[effect])
}
pub fn effect_none(layout: Layout) -> Result<Vec<Packet>> {
    Ok(vec![set_effect(layout, 0)?])
}
pub fn effect_spectrum(layout: Layout) -> Result<Vec<Packet>> {
    Ok(vec![set_effect(layout, 5)?])
}

pub fn effect_static(
    layout: Layout,
    colour: [u8; 3],
    intensity: Option<u8>,
    with_colour: bool,
) -> Result<Vec<Packet>> {
    let mut reports = Vec::new();
    if with_colour {
        let mut data = colour.to_vec();
        data.extend(intensity);
        let address = *layout
            .breathing
            .first()
            .ok_or_else(|| Error::Invalid("Kraken layout has no colour address".into()))?;
        reports.push(write_ram(address, &data)?);
    }
    reports.push(set_effect(layout, 1)?);
    Ok(reports)
}
pub fn effect_custom(
    layout: Layout,
    colour: [u8; 3],
    intensity: Option<u8>,
) -> Result<Vec<Packet>> {
    let mut data = colour.to_vec();
    data.extend(intensity);
    Ok(vec![
        write_ram(layout.custom, &data)?,
        set_effect(layout, 1)?,
    ])
}
pub fn effect_breathing(layout: Layout, colours: &[[u8; 3]]) -> Result<Vec<Packet>> {
    let count = colours.len();
    if !(1..=3).contains(&count) || count > layout.breathing.len() {
        return Err(Error::NotSupported(format!(
            "this Kraken cannot breathe with {count} colours"
        )));
    }
    let base = layout.breathing[count - 1];
    let mut reports = colours
        .iter()
        .enumerate()
        .map(|(i, rgb)| write_ram(base + i as u16 * 4, rgb))
        .collect::<Result<Vec<_>>>()?;
    reports.push(set_effect(layout, 1 | 8 | [2, 16, 32][count - 1])?);
    Ok(reports)
}
