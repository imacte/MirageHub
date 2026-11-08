//! JSON device API for the Naga V2 Pro's onboard memory.
use super::Arguments;
use crate::{protocol::onboard::Onboard, transport::DeviceTransport, Error, Result};
use serde_json::{json, Value};

fn number(
    args: &Arguments<'_>,
    name: &str,
    index: usize,
    default: Option<u64>,
    max: u64,
) -> Result<u64> {
    args.value(name, index, default.map(|v| json!(v)))?
        .as_u64()
        .filter(|n| *n <= max)
        .ok_or_else(|| Error::Invalid(format!("{name} must be an integer from 0 to {max}")))
}
fn bytes(value: Value, name: &str) -> Result<Vec<u8>> {
    value
        .as_array()
        .ok_or_else(|| Error::Invalid(format!("{name} must be an array of bytes")))?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| Error::Invalid(format!("{name} contains an invalid byte")))
        })
        .collect()
}
pub(super) fn call(
    transport: &mut dyn DeviceTransport,
    method: &str,
    args: &Arguments<'_>,
) -> Result<Value> {
    let parameters: &[&str] = match method {
        "get_button_action" => &["button", "layer", "store"],
        "set_button_action" => &["button", "action", "layer", "store"],
        "set_button_key" => &["button", "usage", "modifiers", "layer", "store"],
        "set_button_mouse" => &["button", "mouse_button", "layer", "store"],
        "set_button_macro" => &["button", "macro_id", "layer", "while_held", "store"],
        "add_onboard_macro" => &["macro_id", "events"],
        "delete_onboard_macro" | "get_onboard_macro_length" => &["macro_id"],
        _ => &[],
    };
    if args.args.len() > parameters.len()
        || args
            .kwargs
            .keys()
            .any(|k| !parameters.contains(&k.as_str()))
    {
        return Err(Error::Invalid(format!("unexpected argument for {method}")));
    }
    let mut board = Onboard::new(transport);
    match method {
        "get_side_plate" => return Ok(json!(board.side_plate()?)),
        "get_onboard_buttons" => return Ok(json!(crate::protocol::onboard::BUTTONS)),
        "get_onboard_macros" => return Ok(board.inventory()?["ids"].clone()),
        "get_onboard_macro_inventory" => return board.inventory(),
        "get_onboard_macro_count" => return Ok(json!(board.macro_count()?)),
        "get_onboard_macro_storage" => return board.storage(),
        "get_onboard_macro_length" | "delete_onboard_macro" | "add_onboard_macro" => {
            let id = number(args, "macro_id", 0, None, u16::MAX.into())? as u16;
            match method {
                "get_onboard_macro_length" => return Ok(json!(board.macro_length(id)?)),
                "delete_onboard_macro" => board.delete(id)?,
                _ => {
                    let value = args.value("events", 1, None)?;
                    let events = if let Some(text) = value.as_str() {
                        crate::protocol::onboard_text::events(text)?
                    } else {
                        bytes(value, "events")?
                    };
                    board.upload(id, &events)?;
                }
            }
        }
        _ => {
            let id = number(args, "button", 0, None, 255)? as u8;
            let (layer_index, store_index) = match method {
                "get_button_action" => (1, 2),
                "set_button_key" => (3, 4),
                "set_button_macro" => (2, 4),
                _ => (2, 3),
            };
            let layer = number(args, "layer", layer_index, Some(0), 1)? as u8;
            let store = number(args, "store", store_index, Some(1), 5)? as u8;
            if method == "get_button_action" {
                return Ok(json!(board.get_action(store, id, layer)?));
            }
            let action = match method {
                "set_button_action" => bytes(args.value("action", 1, None)?, "action")?,
                "set_button_key" => {
                    let value = args.value("usage", 1, None)?;
                    let usage = if let Some(name) = value.as_str() {
                        crate::protocol::onboard_text::key_usage(name)?
                    } else {
                        number(args, "usage", 1, None, 255)? as u8
                    };
                    vec![
                        2,
                        2,
                        number(args, "modifiers", 2, Some(0), 255)? as u8,
                        usage,
                    ]
                }
                "set_button_mouse" => {
                    let mouse = number(args, "mouse_button", 1, None, 5)? as u8;
                    if mouse == 0 {
                        return Err(Error::Invalid("mouse_button must be 1..5".into()));
                    }
                    vec![1, 1, mouse]
                }
                "set_button_macro" => {
                    let macro_id = number(args, "macro_id", 1, None, u16::MAX.into())? as u16;
                    crate::protocol::onboard::macro_id(macro_id)?;
                    let held = args
                        .value("while_held", 3, Some(json!(false)))?
                        .as_bool()
                        .ok_or_else(|| Error::Invalid("while_held must be boolean".into()))?;
                    let mut action = if held { vec![4, 2] } else { vec![3, 3] };
                    action.extend_from_slice(&macro_id.to_be_bytes());
                    if !held {
                        action.push(1);
                    }
                    action
                }
                _ => {
                    return Err(Error::NotSupported(format!(
                        "unknown onboard method {method}"
                    )))
                }
            };
            board.set_action(store, id, layer, &action)?;
        }
    }
    Ok(Value::Null)
}
