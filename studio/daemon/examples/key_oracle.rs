//! Offline key and macro compatibility runner; never installs a hook or sends input.
use openrazer_daemon::{
    database,
    keys::{
        codes::{self, Output},
        inject::{self, Injection},
        macros::{self, MacroEvent},
        service,
    },
    Error, Result,
};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn output(value: Option<Output>) -> Value {
    match value {
        Some(Output::Physical(k)) => json!([k.scan, k.extended, k.vk]),
        Some(Output::Virtual(vk)) => json!(vk),
        None => Value::Null,
    }
}
fn injections(events: Vec<Injection>) -> Value {
    json!(events
        .into_iter()
        .map(|e| (e.vk, e.scan, e.flags))
        .collect::<Vec<_>>())
}
fn run(request: Value) -> Value {
    let result = (|| -> Result<Value> {
        let name = request["name"].as_str().unwrap_or("");
        Ok(match request["op"].as_str().unwrap() {
            "source" => json!(codes::source_name(request["scan"].as_u64().unwrap() as u32,request["ext"].as_bool().unwrap(),request["vk"].as_u64().unwrap() as u32)),
            "output" => output(codes::output_for(name)),
            "named" => output(Some(codes::key_named(name)?)),
            "sequence" => {
                let events=codes::parse_sequence(request["text"].as_str().unwrap())?;
                json!({"description":events.iter().map(codes::describe_event).collect::<Vec<_>>(),"events":events})
            }
            "key" => {
                let target = if let Some(vk) = request["target"].as_u64() { Output::Virtual(vk as u16) }
                    else { codes::output_for(request["target"].as_str().unwrap()).ok_or_else(|| Error::Invalid("unknown target".into()))? };
                injections(vec![inject::key_input(target,request["down"].as_bool().unwrap())])
            }
            "raw" => injections(vec![inject::raw_input(&serde_json::from_value::<MacroEvent>(request["event"].clone())?)]),
            "text" => injections(inject::text_inputs(request["text"].as_str().unwrap())),
            "validate" => serde_json::to_value(macros::validate(&request["action"])? )?,
            "report" => json!(service::parse_report(&serde_json::from_value::<Vec<u8>>(request["report"].clone())?,4)),
            "metadata" => json!(database::devices().iter().map(|info|json!({"pid":info.pid,"wants":service::wants(info),"table":service::table_for(info.pid)})).collect::<Vec<_>>()),
            _ => return Err(Error::Invalid("unknown operation".into())),
        })
    })();
    match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(_) => json!({"ok":false}),
    }
}
fn main() -> Result<()> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        serde_json::to_writer(&mut stdout, &run(serde_json::from_str(&line?)?))?;
        writeln!(&mut stdout)?;
        stdout.flush()?;
    }
    Ok(())
}
