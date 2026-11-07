//! Packet builders corresponding to OpenRazer's razerchromacommon helpers.
//! Keep the firmware's unusual lengths and reserved bytes intact.

use super::report::Report;
use crate::{Error, Result};
use serde_json::Value;

fn integer(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .or_else(|| value.as_bool().map(i64::from))
        .ok_or_else(|| Error::Invalid("expected an integer builder argument".into()))
}

pub fn bytes(value: &Value) -> Result<Vec<u8>> {
    value
        .as_array()
        .ok_or_else(|| Error::Invalid("expected byte array".into()))?
        .iter()
        .map(|v| {
            u8::try_from(integer(v)?).map_err(|_| Error::Invalid("byte outside 0..255".into()))
        })
        .collect()
}

struct Args<'a>(&'a [Value]);
impl Args<'_> {
    fn get(&self, index: usize) -> Result<&Value> {
        self.0
            .get(index)
            .ok_or_else(|| Error::Invalid(format!("missing builder argument {index}")))
    }
    fn n(&self, index: usize) -> Result<i64> {
        integer(self.get(index)?)
    }
    fn default(&self, index: usize, default: i64) -> Result<i64> {
        self.0.get(index).map(integer).unwrap_or(Ok(default))
    }
    fn rgb(&self, index: usize) -> Result<[u8; 3]> {
        let mut rgb = [0; 3];
        let value = self.get(index)?;
        if value.is_null() {
            return Ok(rgb);
        }
        let values = value
            .as_array()
            .ok_or_else(|| Error::Invalid("expected RGB array".into()))?;
        for (to, from) in rgb.iter_mut().zip(values) {
            *to = integer(from)? as u8;
        }
        Ok(rgb)
    }
    fn bytes(&self, index: usize) -> Result<Vec<u8>> {
        bytes(self.get(index)?)
    }
}

fn report(class: u8, command: u8, length: u8, data: &[u8]) -> Report {
    let mut report = Report::new(class, command, length);
    report.set_bytes(0, data);
    report
}

/// Resolve the names stored in recipes.json. No dynamic code is evaluated.
pub fn build(name: &str, values: &[Value]) -> Result<Report> {
    let name = name
        .strip_prefix("razer_chroma_")
        .or_else(|| name.strip_prefix("razer_"))
        .unwrap_or(name);
    let a = Args(values);
    macro_rules! n {
        ($i:expr) => {
            a.n($i)?
        };
    }
    macro_rules! b {
        ($i:expr) => {
            n!($i) as u8
        };
    }
    macro_rules! rgb {
        ($i:expr) => {
            a.rgb($i)?
        };
    }
    let result = match name {
        "get_razer_report" => Report::new(b!(0), b!(1), b!(2)),
        "standard_set_device_mode" => report(0, 4, 2, &[if n!(0) == 3 { 3 } else { 0 }, 0]),
        "standard_get_device_mode" => Report::new(0, 0x84, 2),
        "standard_get_serial" => Report::new(0, 0x82, 0x16),
        "standard_get_firmware_version" => Report::new(0, 0x81, 2),
        "standard_set_led_state" => report(3, 0, 3, &[b!(0), b!(1), n!(2).clamp(0, 1) as u8]),
        "standard_get_led_state" => report(3, 0x80, 3, &[b!(0), b!(1)]),
        "standard_set_led_blinking" => report(3, 4, 4, &[b!(0), b!(1), 5, 5]),
        "standard_set_led_rgb" => {
            let mut r = report(3, 1, 5, &[b!(0), b!(1)]);
            r.set_bytes(2, &rgb!(2));
            r
        }
        "standard_get_led_rgb" => report(3, 0x81, 5, &[b!(0), b!(1)]),
        "standard_set_led_effect" => report(3, 2, 3, &[b!(0), b!(1), n!(2).clamp(0, 5) as u8]),
        "standard_get_led_effect" => report(3, 0x82, 3, &[b!(0), b!(1)]),
        "standard_set_led_brightness" => report(3, 3, 3, &[b!(0), b!(1), b!(2)]),
        "standard_get_led_brightness" => report(3, 0x83, 3, &[b!(0), b!(1)]),
        "standard_matrix_set_custom_frame"
        | "extended_matrix_set_custom_frame"
        | "extended_matrix_set_custom_frame2"
        | "misc_one_row_set_custom_frame" => {
            let single = name == "misc_one_row_set_custom_frame";
            let extended = name.starts_with("extended_");
            let start = n!(usize::from(!single));
            let stop = n!(usize::from(!single) + 1);
            if start < 0 || stop < start || stop > 255 {
                return Err(Error::Invalid("invalid custom frame column range".into()));
            }
            let offset = if extended {
                5
            } else if single {
                2
            } else {
                4
            };
            let length = ((stop + 1 - start) * 3) as usize;
            let length = if length > 80 - offset { 0 } else { length };
            let size = if extended {
                let size = if name.ends_with('2') {
                    a.default(4, 0x47)?
                } else {
                    0x47
                };
                if size == 0 {
                    (length + 5) as u8
                } else {
                    size as u8
                }
            } else if single {
                0x32
            } else {
                0x46
            };
            let mut r = if extended {
                report(15, 3, size, &[0, 0, b!(0), start as u8, stop as u8])
            } else if single {
                report(3, 12, size, &[start as u8, stop as u8])
            } else {
                report(3, 11, size, &[255, b!(0), start as u8, stop as u8])
            };
            let data = a.bytes(if single { 2 } else { 3 })?;
            r.set_bytes(offset, &data[..length.min(data.len())]);
            r
        }
        "extended_matrix_brightness" => report(15, 4, 3, &[b!(0), b!(1), b!(2)]),
        "extended_matrix_get_brightness" => report(15, 0x84, 3, &[b!(0), b!(1)]),
        "misc_fn_key_toggle" => report(2, 6, 2, &[0, n!(0).clamp(0, 1) as u8]),
        "misc_set_keyswitch_optimization_command1" => {
            report(2, 2, 4, if n!(0) == 0 { &[0, 20, 0, 40, 0] } else { &[] })
        }
        "misc_set_keyswitch_optimization_command2" => report(
            2,
            21,
            5,
            match n!(0) {
                0 => &[1, 0, 20, 0, 40, 0],
                1 => &[1],
                _ => &[],
            },
        ),
        "misc_get_keyswitch_optimization" => Report::new(2, 0x82, 4),
        "misc_set_blade_brightness" => report(14, 4, 2, &[1, b!(0)]),
        "misc_get_blade_brightness" => report(14, 0x84, 2, &[1]),
        "misc_matrix_reactive_trigger" => report(3, 10, 5, &[2]),
        "misc_get_battery_level" => Report::new(7, 0x80, 2),
        "misc_get_charging_status" => Report::new(7, 0x84, 2),
        "misc_set_dock_charge_type" => report(3, 16, 1, &[n!(0).clamp(0, 1) as u8]),
        "misc_get_polling_rate" => Report::new(0, 0x85, 1),
        "misc_set_polling_rate" => report(0, 5, 1, &[poll_code(n!(0))]),
        "misc_get_polling_rate2" => Report::new(0, 0xc0, 1),
        "misc_set_polling_rate2" => report(
            0,
            0x40,
            2,
            &[
                a.default(1, 0)? as u8,
                match n!(0) {
                    8000 => 1,
                    4000 => 2,
                    2000 => 4,
                    1000 => 8,
                    250 => 32,
                    125 => 64,
                    _ => 16,
                },
            ],
        ),
        "misc_get_dock_brightness" => Report::new(7, 0x82, 1),
        "misc_set_dock_brightness" => report(7, 2, 1, &[b!(0)]),
        "misc_set_dpi_xy" => {
            let mut r = report(4, 5, 7, &[1]);
            r.set_bytes(1, &(n!(1).clamp(100, 45000) as u16).to_be_bytes());
            r.set_bytes(3, &(n!(2).clamp(100, 45000) as u16).to_be_bytes());
            r
        }
        "misc_get_dpi_xy" => report(4, 0x85, 7, &[b!(0)]),
        "misc_set_dpi_xy_byte" => report(4, 1, 3, &[b!(0), b!(1), 0]),
        "misc_get_dpi_xy_byte" => Report::new(4, 0x81, 3),
        "misc_set_dpi_stages" => {
            let count = n!(1);
            if !(0..=11).contains(&count) {
                return Err(Error::Invalid("too many DPI stages".into()));
            }
            let dpi = a
                .get(3)?
                .as_array()
                .ok_or_else(|| Error::Invalid("expected DPI pairs".into()))?;
            if dpi.len() < count as usize * 2 {
                return Err(Error::Invalid("missing DPI pairs".into()));
            }
            let mut r = report(4, 6, 0x26, &[b!(0), b!(2), count as u8]);
            for i in 0..count as usize {
                let offset = 3 + i * 7;
                r.set(offset, i as i64);
                r.set_bytes(offset + 1, &(integer(&dpi[i * 2])? as u16).to_be_bytes());
                r.set_bytes(
                    offset + 3,
                    &(integer(&dpi[i * 2 + 1])? as u16).to_be_bytes(),
                );
            }
            r
        }
        "misc_get_dpi_stages" => report(4, 0x86, 0x26, &[b!(0)]),
        "misc_get_idle_time" => Report::new(7, 0x83, 2),
        "misc_set_idle_time" => report(7, 3, 2, &(n!(0).clamp(60, 900) as u16).to_be_bytes()),
        "misc_get_low_battery_threshold" => Report::new(7, 0x81, 1),
        "misc_set_low_battery_threshold" => report(7, 1, 1, &[n!(0).clamp(12, 63) as u8]),
        "misc_set_orochi2011_led" => {
            let mut r = Report::unpack(&OROCHI_LED)?;
            r.set(1, n!(0));
            r
        }
        "misc_set_orochi2011_poll_dpi" => {
            let mut r = Report::unpack(&OROCHI_DPI)?;
            r.set(1, poll_code(n!(0)) as i64);
            r.set(3, n!(1).clamp(0x15, 0x9c));
            r.set(4, n!(2).clamp(0x15, 0x9c));
            r
        }
        "basilisk_mobile_effect_static" | "naga_trinity_effect_static" => {
            let naga = name.starts_with("naga_");
            let mut r = report(
                15,
                3,
                if naga { 14 } else { 11 },
                &[0, 0, 0, 0, if naga { 2 } else { 1 }],
            );
            r.set_bytes(5, &rgb!(0));
            r.set_bytes(8, &rgb!(0));
            if naga {
                r.set_bytes(11, &rgb!(0));
            }
            r
        }
        "misc_get_scroll_mode" => report(2, 0x94, 2, &[1]),
        "misc_set_scroll_mode" => report(2, 0x14, 2, &[1, b!(0)]),
        "misc_get_scroll_acceleration" => report(2, 0x96, 2, &[1]),
        "misc_set_scroll_acceleration" => report(2, 0x16, 2, &[1, u8::from(n!(0) != 0)]),
        "misc_get_scroll_smart_reel" => report(2, 0x97, 2, &[1]),
        "misc_set_scroll_smart_reel" => report(2, 0x17, 2, &[1, u8::from(n!(0) != 0)]),
        "misc_set_hyperpolling_wireless_dongle_indicator_led_mode" => report(
            7,
            16,
            1,
            &[if (1..=3).contains(&n!(0)) { b!(0) } else { 1 }],
        ),
        "misc_set_hyperpolling_wireless_dongle_pair_step1" => report(0, 0x46, 1, &[1]),
        "misc_set_hyperpolling_wireless_dongle_pair_step2" => {
            report(0, 0x41, 3, &[1, (n!(0) >> 8) as u8, b!(0)])
        }
        "misc_set_hyperpolling_wireless_dongle_unpair" => {
            report(0, 0x42, 2, &(n!(0) as u16).to_be_bytes())
        }
        _ if name.starts_with("standard_matrix_effect_") => standard_effect(&name[23..], &a)?,
        _ if name.starts_with("extended_matrix_effect_") => extended_effect(&name[23..], &a)?,
        _ if name.starts_with("mouse_extended_matrix_effect_") => mouse_effect(&name[29..], &a)?,
        _ if name.starts_with("naga_trinity_effect_") => naga_effect(&name[20..], &a)?,
        _ => {
            return Err(Error::NotSupported(format!(
                "unknown report builder: {name}"
            )))
        }
    };
    Ok(result)
}

fn poll_code(rate: i64) -> u8 {
    match rate {
        1000 => 1,
        125 => 8,
        _ => 2,
    }
}

fn unknown_effect(name: &str) -> Error {
    Error::NotSupported(format!("unknown effect builder: {name}"))
}

fn standard_effect(name: &str, a: &Args<'_>) -> Result<Report> {
    let (size, effect) = match name {
        "none" => (1, 0),
        "wave" => (2, 1),
        "spectrum" => (1, 4),
        "reactive" => (5, 2),
        "static" => (4, 6),
        "starlight_single" | "starlight_dual" | "starlight_random" => (1, 25),
        "breathing_random" | "breathing_single" | "breathing_dual" => (8, 3),
        "custom_frame" => (2, 5),
        _ => return Err(unknown_effect(name)),
    };
    let mut r = report(3, 10, size, &[effect]);
    match name {
        "wave" => r.set(1, a.n(0)?.clamp(1, 2)),
        "reactive" => {
            r.set(1, a.n(0)?.clamp(1, 4));
            r.set_bytes(2, &a.rgb(1)?);
        }
        "static" => r.set_bytes(1, &a.rgb(0)?),
        "starlight_single" | "starlight_dual" | "starlight_random" => {
            r.set(
                1,
                if name.ends_with("single") {
                    1
                } else if name.ends_with("dual") {
                    2
                } else {
                    3
                },
            );
            r.set(2, a.n(0)?.clamp(1, 3));
            if !name.ends_with("random") {
                r.set_bytes(3, &a.rgb(1)?);
            }
            if name.ends_with("dual") {
                r.set_bytes(6, &a.rgb(2)?);
            }
        }
        "breathing_single" | "breathing_dual" | "breathing_random" => {
            r.set(
                1,
                if name.ends_with("single") {
                    1
                } else if name.ends_with("dual") {
                    2
                } else {
                    3
                },
            );
            if !name.ends_with("random") {
                r.set_bytes(2, &a.rgb(0)?);
            }
            if name.ends_with("dual") {
                r.set_bytes(5, &a.rgb(1)?);
            }
        }
        "custom_frame" => r.set(1, a.n(0)?),
        _ => {}
    }
    Ok(r)
}

fn extended_effect(name: &str, a: &Args<'_>) -> Result<Report> {
    if name == "custom_frame" {
        return Ok(report(15, 2, 12, &[0, 0, 8]));
    }
    let (size, effect) = match name {
        "none" => (6, 0),
        "static" => (9, 1),
        "wave" => (6, 4),
        "spectrum" => (6, 3),
        "wheel" => (6, 10),
        "reactive" => (9, 5),
        "starlight_random" => (6, 7),
        "starlight_single" => (9, 7),
        "starlight_dual" => (12, 7),
        "breathing_random" => (6, 2),
        "breathing_single" => (9, 2),
        "breathing_dual" => (12, 2),
        _ => return Err(unknown_effect(name)),
    };
    let mut r = report(15, 2, size, &[a.n(0)? as u8, a.n(1)? as u8, effect]);
    match name {
        "static" => {
            r.set(5, 1);
            r.set_bytes(6, &a.rgb(2)?);
        }
        "wave" | "wheel" => {
            r.set(3, a.n(2)?.clamp(i64::from(name == "wheel"), 2));
            r.set(4, 40);
        }
        "reactive" => {
            r.set(4, a.n(2)?.clamp(1, 4));
            r.set(5, 1);
            r.set_bytes(6, &a.rgb(3)?);
        }
        "starlight_random" | "starlight_single" | "starlight_dual" => {
            r.set(4, a.n(2)?.clamp(1, 3));
            if !name.ends_with("random") {
                r.set(5, 1);
                r.set_bytes(6, &a.rgb(3)?);
            }
            if name.ends_with("dual") {
                r.set(5, 2);
                r.set_bytes(9, &a.rgb(4)?);
            }
        }
        "breathing_single" | "breathing_dual" => {
            let count = if name.ends_with("dual") { 2 } else { 1 };
            r.set(3, count);
            r.set(5, count);
            r.set_bytes(6, &a.rgb(2)?);
            if count == 2 {
                r.set_bytes(9, &a.rgb(3)?);
            }
        }
        _ => {}
    }
    Ok(r)
}

fn mouse_effect(name: &str, a: &Args<'_>) -> Result<Report> {
    let (size, effect) = match name {
        "none" => (3, 0),
        "static" => (6, 6),
        "spectrum" => (3, 4),
        "reactive" => (7, 2),
        "breathing_random" | "breathing_single" | "breathing_dual" => (10, 3),
        _ => return Err(unknown_effect(name)),
    };
    let mut r = report(3, 13, size, &[a.n(0)? as u8, a.n(1)? as u8, effect]);
    match name {
        "static" => r.set_bytes(3, &a.rgb(2)?),
        "reactive" => {
            r.set(3, a.n(2)?.clamp(1, 4));
            r.set_bytes(4, &a.rgb(3)?);
        }
        "breathing_single" | "breathing_dual" | "breathing_random" => {
            r.set(
                3,
                if name.ends_with("single") {
                    1
                } else if name.ends_with("dual") {
                    2
                } else {
                    3
                },
            );
            if !name.ends_with("random") {
                r.set_bytes(4, &a.rgb(2)?);
            }
            if name.ends_with("dual") {
                r.set_bytes(7, &a.rgb(3)?);
            }
        }
        _ => {}
    }
    Ok(r)
}

fn naga_effect(name: &str, a: &Args<'_>) -> Result<Report> {
    let (size, effect) = match name {
        "none" => (6, 0),
        "spectrum" => (6, 3),
        "breathing_random" => (6, 2),
        "breathing_single" => (9, 2),
        "breathing_dual" => (12, 2),
        _ => return Err(unknown_effect(name)),
    };
    let mut r = report(15, 2, size, &[0, 0, effect]);
    if name == "breathing_single" || name == "breathing_dual" {
        let count = if name.ends_with("dual") { 2 } else { 1 };
        r.set(3, count);
        r.set(5, count);
        r.set_bytes(6, &a.rgb(0)?);
        if count == 2 {
            r.set_bytes(9, &a.rgb(1)?);
        }
    }
    Ok(r)
}

const OROCHI_LED: [u8; 90] = [
    1, 0, 0, 6, 72, 0, 0, 0, 1, 255, 3, 5, 6, 6, 16, 16, 16, 16, 36, 36, 76, 76, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 2, 2, 1, 1, 3, 3, 4, 1, 4, 4, 1, 1, 5, 5, 1, 1, 6, 49, 136, 0, 7, 49, 135, 0, 8, 8,
    1, 1, 9, 9, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 68,
    1,
];
const OROCHI_DPI: [u8; 90] = [
    1, 0, 0, 5, 5, 0, 0, 0, 1, 2, 3, 76, 76, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
