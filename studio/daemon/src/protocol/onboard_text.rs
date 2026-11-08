//! Human-readable HID key names and macro steps; shared by the CLI and RPC.
use crate::{Error, Result};

pub fn key_usage(name: &str) -> Result<u8> {
    let name = name
        .trim()
        .to_ascii_uppercase()
        .replace([' ', '_', '-'], "");
    if name.len() == 1 {
        let b = name.as_bytes()[0];
        if b.is_ascii_uppercase() {
            return Ok(b - b'A' + 4);
        }
        if (b'1'..=b'9').contains(&b) {
            return Ok(b - b'1' + 30);
        }
        if b == b'0' {
            return Ok(39);
        }
    }
    if let Some(function) = name
        .strip_prefix('F')
        .and_then(|f| f.parse::<u8>().ok())
        .filter(|f| (1..=24).contains(f))
    {
        return Ok(if function <= 12 {
            function + 57
        } else {
            function + 91
        });
    }
    let usage = match name.as_str() {
        "ENTER" | "RETURN" => 40,
        "ESC" | "ESCAPE" => 41,
        "BACKSPACE" => 42,
        "TAB" => 43,
        "SPACE" => 44,
        "MINUS" => 45,
        "EQUAL" | "EQUALS" => 46,
        "LEFTBRACKET" => 47,
        "RIGHTBRACKET" => 48,
        "BACKSLASH" => 49,
        "SEMICOLON" => 51,
        "QUOTE" => 52,
        "GRAVE" => 53,
        "COMMA" => 54,
        "PERIOD" => 55,
        "SLASH" => 56,
        "CAPSLOCK" => 57,
        "PRINTSCREEN" => 70,
        "SCROLLLOCK" => 71,
        "PAUSE" => 72,
        "INSERT" => 73,
        "HOME" => 74,
        "PAGEUP" => 75,
        "DELETE" => 76,
        "END" => 77,
        "PAGEDOWN" => 78,
        "RIGHT" | "ARROWRIGHT" => 79,
        "LEFT" | "ARROWLEFT" => 80,
        "DOWN" | "ARROWDOWN" => 81,
        "UP" | "ARROWUP" => 82,
        "NUMLOCK" => 83,
        "CTRL" | "CONTROL" | "LEFTCTRL" => 0xe0,
        "SHIFT" | "LEFTSHIFT" => 0xe1,
        "ALT" | "LEFTALT" => 0xe2,
        "WIN" | "LEFTWIN" => 0xe3,
        "RIGHTCTRL" => 0xe4,
        "RIGHTSHIFT" => 0xe5,
        "RIGHTALT" => 0xe6,
        "RIGHTWIN" => 0xe7,
        _ => {
            return Err(Error::Invalid(format!(
                "unknown HID key {name:?}; use a key name or a numeric HID usage"
            )))
        }
    };
    Ok(usage)
}

/// `down A; wait 50; up A`, with semicolons or line breaks between steps.
pub fn events(text: &str) -> Result<Vec<u8>> {
    let mut events = Vec::new();
    for line in text
        .split(['\n', ';'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        let Some((kind, value)) = line.split_once(char::is_whitespace) else {
            return Err(Error::Invalid(format!(
                "invalid macro step {line:?}; use down KEY, up KEY or wait MS"
            )));
        };
        match kind.to_ascii_lowercase().as_str() {
            "down" | "up" => events.extend([
                if kind.eq_ignore_ascii_case("down") {
                    1
                } else {
                    2
                },
                key_usage(value)?,
            ]),
            "wait" => {
                let ms = value.trim().parse::<u16>().map_err(|_| {
                    Error::Invalid("macro wait must be 0..65535 milliseconds".into())
                })?;
                if let Ok(short) = u8::try_from(ms) {
                    events.extend([0x11, short]);
                } else {
                    events.push(0x12);
                    events.extend(ms.to_be_bytes());
                }
            }
            _ => return Err(Error::Invalid(format!("unknown macro step {kind:?}"))),
        }
        if events.len() > super::onboard::MAX_MACRO_BYTES {
            return Err(Error::Invalid("macro exceeds 4094 bytes".into()));
        }
    }
    super::onboard::validate_events(&events)?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hid_usages_and_delays_match_the_wire_format_without_using_virtual_keys() {
        assert_eq!(
            events("down LeftCtrl; down A\nwait 300; up A; up LeftCtrl").unwrap(),
            [1, 0xe0, 1, 4, 0x12, 1, 44, 2, 4, 2, 0xe0]
        );
        assert_eq!(key_usage("F1").unwrap(), 58);
        assert_eq!(key_usage("F24").unwrap(), 115);
        for text in [
            "",
            "press A",
            "wait -1",
            "wait 65536",
            "down nonexistent",
            "down A B",
            "down",
        ] {
            assert!(events(text).is_err());
        }
    }
}
