use super::{control::Backend, Arguments};
use crate::{
    protocol::{onboard, onboard_text},
    Error, Result,
};
use serde_json::{json, Value};

enum Request {
    Status,
    Call(&'static str, Value),
}
fn number(text: &str, maximum: u64) -> Result<u64> {
    let value = if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
    } else {
        text.parse()
    };
    value
        .ok()
        .filter(|v| *v <= maximum)
        .ok_or_else(|| Error::Invalid(format!("{text:?} must be a number from 0 to {maximum}")))
}
fn bytes(text: &str) -> Result<Vec<u8>> {
    let text = text.replace([' ', ',', ':', '-'], "");
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err(Error::Invalid(
            "use hexadecimal byte pairs, such as 02 02 00 04".into(),
        ));
    }
    text.as_bytes()
        .chunks(2)
        .map(|b| {
            u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16)
                .map_err(|_| Error::Invalid("invalid hexadecimal action byte".into()))
        })
        .collect()
}
fn request(args: &Arguments) -> Result<Request> {
    let p = &args.positional;
    let action = p.get(1).map_or("status", String::as_str);
    let (min, max) = match action {
        "status" | "buttons" | "macros" => (0, 0),
        "action" => (1, 2),
        "key" | "mouse" | "macro-bind" | "macro-add" => (2, 2),
        "macro-delete" | "macro-length" => (1, 1),
        _ => return Err(Error::Invalid(format!("unknown onboard action {action}"))),
    };
    let supplied = p.len().saturating_sub(2);
    if !(min..=max).contains(&supplied) {
        return Err(Error::Invalid(format!(
            "invalid arguments for onboard {action}"
        )));
    }
    let binding = matches!(action, "action" | "key" | "mouse" | "macro-bind");
    if !binding && (args.value("--store").is_some() || args.value("--layer").is_some()) {
        return Err(Error::Invalid(
            "--store and --layer apply to button mappings".into(),
        ));
    }
    if (args.value("--modifiers").is_some() && action != "key")
        || (args.flag("--while-held") && action != "macro-bind")
    {
        return Err(Error::Invalid(
            "--modifiers is for key; --while-held is for macro-bind".into(),
        ));
    }
    let layer = number(args.value("--layer").unwrap_or("0"), 1)?;
    let store = number(args.value("--store").unwrap_or("1"), 5)?;
    let id = p.get(2).map(|v| number(v, 65535)).transpose()?.unwrap_or(0);
    if binding {
        onboard::button(
            store as u8,
            u8::try_from(id).map_err(|_| Error::Invalid("button ID exceeds 255".into()))?,
            layer as u8,
        )?;
    }
    let request = match action {
        "status" => Request::Status,
        "buttons" => Request::Call("get_onboard_buttons", json!([])),
        "macros" => Request::Call("get_onboard_macro_inventory", json!([])),
        "action" if p.len() == 3 => Request::Call("get_button_action", json!([id, layer, store])),
        "action" => {
            let action = bytes(&p[3])?;
            if !(2..=7).contains(&action.len()) || usize::from(action[1]) > action.len() - 2 {
                return Err(Error::Invalid(
                    "action must be type, length and up to five bytes".into(),
                ));
            }
            Request::Call("set_button_action", json!([id, action, layer, store]))
        }
        "key" => {
            let usage = if p[3].starts_with("0x") || p[3].starts_with("0X") {
                number(&p[3], 255)? as u8
            } else {
                onboard_text::key_usage(&p[3])?
            };
            let modifiers = number(args.value("--modifiers").unwrap_or("0"), 255)?;
            Request::Call(
                "set_button_key",
                json!([id, usage, modifiers, layer, store]),
            )
        }
        "mouse" => {
            let mouse = number(&p[3], 5)?;
            if mouse == 0 {
                return Err(Error::Invalid("mouse button must be 1..5".into()));
            }
            Request::Call("set_button_mouse", json!([id, mouse, layer, store]))
        }
        "macro-bind" => {
            let macro_id = number(&p[3], 65535)? as u16;
            onboard::macro_id(macro_id)?;
            Request::Call(
                "set_button_macro",
                json!([id, macro_id, layer, args.flag("--while-held"), store]),
            )
        }
        "macro-add" => {
            onboard::macro_id(id as u16)?;
            let events = if let Some(hex) = p[3].strip_prefix("hex:") {
                bytes(hex)?
            } else {
                onboard_text::events(&p[3])?
            };
            onboard::validate_events(&events)?;
            Request::Call("add_onboard_macro", json!([id, events]))
        }
        "macro-delete" | "macro-length" => {
            onboard::macro_id(id as u16)?;
            Request::Call(
                if action == "macro-delete" {
                    "delete_onboard_macro"
                } else {
                    "get_onboard_macro_length"
                },
                json!([id]),
            )
        }
        _ => unreachable!(),
    };
    Ok(request)
}
pub(super) fn validate(args: &Arguments) -> Result<()> {
    request(args).map(|_| ())
}
pub(super) fn run(
    backend: &mut Backend,
    args: &Arguments,
    serial: &str,
    caps: &Value,
) -> Result<Option<Value>> {
    if caps["onboard_buttons"] != true {
        return Ok(None);
    }
    let value = match request(args)? {
        Request::Status => json!({
            "side_plate":backend.device(serial,"get_side_plate",json!([]),None)?,
            "macros":backend.device(serial,"get_onboard_macro_inventory",json!([]),None)?,
            "storage":backend.device(serial,"get_onboard_macro_storage",json!([]),None)?,
            "buttons":onboard::BUTTONS,
        }),
        Request::Call(method, values) => {
            json!({"method":method,"result":backend.device(serial,method,values,None)?})
        }
    };
    Ok(Some(value))
}
