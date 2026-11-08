use super::{connect, print_json, rpc_error, Arguments};
use crate::{
    keys::snaptap_settings::{clean_pairs, clean_programs, WARNING},
    Error, Result,
};
use serde_json::{json, Value};

pub fn validate(args: &Arguments) -> Result<()> {
    let p = &args.positional;
    let action = p.get(1).map_or("status", String::as_str);
    match action {
        "status" | "on" | "off" if p.len() <= 2 => {}
        "pair" if p.len() == 5 && ["add", "remove"].contains(&p[2].as_str()) => {
            clean_pairs(&json!([[p[3], p[4]]]))?;
        }
        "deny" | "allow" if p.len() == 4 && ["add", "remove"].contains(&p[2].as_str()) => {
            clean_programs(&json!([p[3]]))?;
        }
        _ => return Err(Error::Invalid(
            "use snaptap [status|on|off], pair add|remove A D, or deny|allow add|remove game.exe"
                .into(),
        )),
    }
    if args.flag("--accept") && action != "on" {
        return Err(Error::Invalid(
            "--accept is only valid with snaptap on".into(),
        ));
    }
    Ok(())
}

pub fn run(args: &Arguments) -> Result<i32> {
    let mut client = connect(args)?;
    let status = client
        .call("snaptap.status", json!({}))
        .map_err(rpc_error)?;
    let p = &args.positional;
    let action = p.get(1).map_or("status", String::as_str);
    let changes = match action {
        "status" => None,
        "on" => {
            if status["acknowledged"] != true && !args.flag("--accept") {
                return Err(Error::Invalid(format!(
                    "{WARNING}\nUse snaptap on --accept to acknowledge this warning."
                )));
            }
            Some(json!({"enabled":true,"acknowledged":args.flag("--accept")}))
        }
        "off" => Some(json!({"enabled":false})),
        "pair" => {
            let mut pairs = clean_pairs(&status["pairs"])?;
            let pair = clean_pairs(&json!([[p[3], p[4]]]))?.remove(0);
            if p[2] == "add" {
                pairs.push(pair);
            } else {
                let length = pairs.len();
                pairs.retain(|existing| {
                    existing != &pair && !(existing[0] == pair[1] && existing[1] == pair[0])
                });
                if pairs.len() == length {
                    return Err(Error::Invalid("that Snap Tap pair does not exist".into()));
                }
            }
            // Validate overlaps before sending the replacement list.
            clean_pairs(&json!(pairs))?;
            Some(json!({"pairs":pairs}))
        }
        "deny" | "allow" => {
            let mut programs = clean_programs(&status[action])?;
            let name = clean_programs(&json!([p[3]]))?.remove(0);
            if p[2] == "add" {
                if !programs.contains(&name) {
                    programs.push(name);
                }
            } else {
                programs.retain(|program| program != &name);
            }
            Some(json!({action:programs}))
        }
        _ => unreachable!("validated command"),
    };
    let status = match changes {
        Some(changes) => client.call("snaptap.set", changes).map_err(rpc_error)?,
        None => status,
    };
    if args.flag("--json") {
        print_json(&status);
    } else if !args.flag("--quiet") {
        println!(
            "Snap Tap: {} ({})",
            if status["enabled"] == true {
                "on"
            } else {
                "off"
            },
            status["scope"].as_str().unwrap_or_default()
        );
        if let Some(reason) = status["paused"].as_str() {
            println!("Paused: {reason}");
        }
        for pair in status["pairs"].as_array().into_iter().flatten() {
            println!(
                "  {} / {}",
                pair[0].as_str().unwrap_or_default(),
                pair[1].as_str().unwrap_or_default()
            );
        }
        for field in ["deny", "allow"] {
            let programs: Vec<_> = status[field]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            println!("{field}: {}", programs.join(", "));
        }
    }
    Ok(0)
}
