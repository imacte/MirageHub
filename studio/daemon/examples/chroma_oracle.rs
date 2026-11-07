//! Offline Chroma conversion and session comparison; no listener or hardware.
use openrazer_daemon::chroma::{bridge, grid, ChromaBridge, ChromaError, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    sync::{Arc, Mutex},
};
fn sessions(request: &Value) -> Value {
    let now = Arc::new(Mutex::new(0.0));
    let clock = now.clone();
    let bridge = ChromaBridge::with_clock(
        Arc::new(Vec::new),
        None,
        None,
        None,
        Arc::new(move || *clock.lock().unwrap()),
    );
    let mut effects = BTreeMap::<String, String>::new();
    let mut answers = Vec::new();
    for action in request["actions"].as_array().unwrap() {
        *now.lock().unwrap() = action["time"].as_f64().unwrap();
        let number = action["number"].as_u64().unwrap_or(0);
        let ids = {
            action["ids"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| {
                    let name = v.as_str().unwrap();
                    effects.get(name).cloned().unwrap_or_else(|| name.into())
                })
                .collect::<Vec<_>>()
        };
        let result = (|| -> Result<Value> {
            Ok(match action["op"].as_str().unwrap() {
                "open" => json!(bridge.open(&action["info"])),
                "close" => {
                    bridge.close(number);
                    Value::Null
                }
                "heartbeat" => json!(bridge.heartbeat(number)?),
                "show" => {
                    bridge.show(number, action["kind"].as_str().unwrap(), &action["body"])?;
                    Value::Null
                }
                "create" => {
                    let id =
                        bridge.create(number, action["kind"].as_str().unwrap(), &action["body"])?;
                    effects.insert(action["label"].as_str().unwrap().into(), id);
                    action["label"].clone()
                }
                "apply" => {
                    bridge.apply(number, &ids)?;
                    Value::Null
                }
                "delete" => {
                    bridge.delete(number, &ids)?;
                    Value::Null
                }
                "expire" => {
                    bridge.expire();
                    Value::Null
                }
                _ => return Err(ChromaError::Invalid("unknown operation".into())),
            })
        })();
        let result = match result {
            Ok(result) => json!({"ok":true,"result":result}),
            Err(_) => json!({"ok":false}),
        };
        answers.push(json!({"result":result,"status":bridge.status()}));
    }
    json!(answers)
}
fn run(request: Value) -> Value {
    let result = (|| -> Result<Value> {
        Ok(match request["op"].as_str().unwrap() {
            "parse" => json!(grid::parse(
                request["kind"].as_str().unwrap(),
                &request["body"]
            )?),
            "project" => {
                let look = grid::parse(request["kind"].as_str().unwrap(), &request["body"])?;
                bridge::output(
                    &request["capabilities"],
                    request["kind"].as_str().unwrap(),
                    &look,
                )?
                .map(|o| o.to_json())
                .unwrap_or(Value::Null)
            }
            "resample" => json!(grid::resample(
                &serde_json::from_value::<Vec<[u8; 3]>>(request["colours"].clone()).unwrap(),
                request["count"].as_u64().unwrap() as usize
            )),
            "sessions" => sessions(&request),
            _ => return Err(ChromaError::Invalid("unknown operation".into())),
        })
    })();
    match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(_) => json!({"ok":false}),
    }
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        serde_json::to_writer(&mut output, &run(serde_json::from_str(&line?)?))?;
        writeln!(&mut output)?;
        output.flush()?;
    }
    Ok(())
}
