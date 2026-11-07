//! Run the data-driven programs extracted from the upstream kernel drivers.

mod expression;

use crate::protocol::{builders, report::Report};
use crate::{Error, Result};
use expression::{compare, number};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

#[derive(Debug, Default, Clone)]
pub struct Environment {
    pub buf: Vec<u8>,
    pub vars: Map<String, Value>,
    pub reports: BTreeMap<String, Report>,
    pub response: Option<Report>,
    pub out: [u8; 16],
    pub sent: usize,
}

impl Environment {
    pub fn new(buf: Vec<u8>, vars: Map<String, Value>) -> Self {
        Self {
            buf,
            vars,
            ..Self::default()
        }
    }
    pub fn buf_byte(&self, index: usize) -> u8 {
        self.buf.get(index).copied().unwrap_or(0)
    }
    pub fn response_arg(&self, index: usize) -> u8 {
        self.response
            .as_ref()
            .and_then(|r| r.arguments.get(index))
            .copied()
            .unwrap_or(0)
    }
    fn report(&mut self, target: &str) -> &mut Report {
        if target == "response" {
            self.response.get_or_insert_with(Report::default)
        } else {
            self.reports.entry(target.into()).or_default()
        }
    }
}

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid(format!("recipe missing string {field}")))
}
fn list(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| Error::Invalid("expected recipe array".into()))
}
fn optional_list(value: &Value) -> Result<&[Value]> {
    if value.is_null() {
        Ok(&[])
    } else {
        list(value)
    }
}
fn index(value: &Value) -> Result<usize> {
    usize::try_from(number(value)?).map_err(|_| Error::Invalid("invalid recipe offset".into()))
}

pub fn evaluate(value: &Value, env: &Environment) -> Result<Value> {
    if !value.is_object() {
        return Ok(value.clone());
    }
    Ok(match text(value, "k")? {
        "const" => value["value"].clone(),
        "buf" => json!(env.buf_byte(index(&value["offset"])?)),
        "response" => json!(env.response_arg(index(&value["offset"])?)),
        "rgb" => {
            let start = index(&value["offset"])?;
            json!([
                env.buf_byte(start),
                env.buf_byte(start.saturating_add(1)),
                env.buf_byte(start.saturating_add(2))
            ])
        }
        "blob" => {
            let offset = index(&evaluate(&value["offset"], env)?)?;
            json!(env.buf.get(offset..).unwrap_or(&[]))
        }
        "var" => {
            let name = text(value, "name")?;
            if name == "count" {
                json!(env.buf.len())
            } else {
                env.vars
                    .get(name)
                    .cloned()
                    .ok_or_else(|| Error::Invalid(format!("unbound variable: {name}")))?
            }
        }
        "ternary" => {
            let test = &value["test"];
            let result = compare(
                text(test, "op")?,
                &evaluate(&test["left"], env)?,
                &evaluate(&test["right"], env)?,
            )?;
            evaluate(&value[if result { "then" } else { "else" }], env)?
        }
        "raw" => expression::raw(text(value, "expr")?, env)?,
        kind => return Err(Error::Invalid(format!("unknown recipe value: {kind}"))),
    })
}

pub trait Sender {
    fn send(&mut self, request: &Report, want_response: bool) -> Result<Option<Report>>;
    fn send_argb(&mut self, _channel: u8, _data: &[u8]) -> Result<()> {
        Err(Error::NotSupported(
            "this transport cannot send ARGB frames".into(),
        ))
    }
}

pub fn execute(steps: &[Value], env: &mut Environment, sender: &mut dyn Sender) -> Result<()> {
    execute_inner(steps, env, sender, 0).map(|_| ())
}

// true propagates the driver's `return` through nested branches and row loops.
fn execute_inner(
    steps: &[Value],
    env: &mut Environment,
    sender: &mut dyn Sender,
    depth: usize,
) -> Result<bool> {
    if depth > 64 {
        return Err(Error::Invalid("recipe nesting limit exceeded".into()));
    }
    for step in steps {
        match text(step, "op")? {
            "stop" => return Ok(true),
            "build" => {
                let args = list(&step["args"])?
                    .iter()
                    .map(|v| evaluate(v, env))
                    .collect::<Result<Vec<_>>>()?;
                env.reports.insert(
                    text(step, "target")?.into(),
                    builders::build(text(step, "fn")?, &args)?,
                );
            }
            "txid" => {
                let value = number(&evaluate(&step["value"], env)?)? as u8;
                env.report(text(step, "target")?).transaction_id = value;
            }
            "set_arg" => {
                let index = index(&evaluate(&step["index"], env)?)?;
                let value = evaluate(&step["value"], env)?;
                let report = env.report(text(step, "target")?);
                if value.is_array() {
                    report.set_bytes(index, &builders::bytes(&value)?);
                } else {
                    report.set(index, number(&value)?);
                }
            }
            "set_field" => {
                let value = number(&evaluate(&step["value"], env)?)?;
                let report = env.report(text(step, "target")?);
                match text(step, "field")? {
                    "command_id" => report.command_id = value as u8,
                    "command_class" => report.command_class = value as u8,
                    "data_size" => report.data_size = value as u8,
                    "remaining_packets" => report.remaining_packets = value as u16,
                    field => return Err(Error::Invalid(format!("unknown report field: {field}"))),
                }
            }
            "send" | "send_no_response" => {
                let request = env.reports.get("request").ok_or_else(|| {
                    Error::NotSupported("recipe reached send without a report".into())
                })?;
                let response = sender.send(request, step["op"] == "send")?;
                env.sent += 1;
                if response.is_some() {
                    env.response = response;
                }
            }
            "call" => {
                let bindings = step["vars"]
                    .as_object()
                    .ok_or_else(|| Error::Invalid("missing helper bindings".into()))?
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), evaluate(value, env)?)))
                    .collect::<Result<Map<_, _>>>()?;
                let saved_vars = env.vars.clone();
                let saved_reports = std::mem::take(&mut env.reports);
                let saved_response = env.response.clone();
                env.vars.extend(bindings);
                let result = optional_list(&step["steps"])
                    .and_then(|steps| execute_inner(steps, env, sender, depth + 1));
                let returned = env.vars.get("__return__").cloned().unwrap_or(Value::Null);
                env.vars = saved_vars;
                env.reports = saved_reports;
                env.response = saved_response;
                result?;
                if let Some(name) = step["result"].as_str().filter(|s| !s.is_empty()) {
                    env.vars.insert(name.into(), returned);
                }
            }
            "let" => {
                let value = evaluate(&step["value"], env)?;
                env.vars.insert(text(step, "name")?.into(), value);
            }
            "xor" => {
                let name = text(step, "name")?;
                let current = env.vars.get(name).map(number).unwrap_or(Ok(0))?;
                env.vars
                    .insert(name.into(), json!(current ^ number(&step["value"])?));
            }
            "out" => {
                let value = env.response_arg(index(&step["arg"])?);
                let target = env
                    .out
                    .get_mut(index(&step["index"])?)
                    .ok_or_else(|| Error::Invalid("output offset out of range".into()))?;
                *target = value;
            }
            "if_count" => {
                let test = &step["test"];
                let branch = if compare(text(test, "op")?, &json!(env.buf.len()), &test["value"])? {
                    "then"
                } else {
                    "else"
                };
                if execute_inner(optional_list(&step[branch])?, env, sender, depth + 1)? {
                    return Ok(true);
                }
            }
            "switch_count" | "switch_var" => {
                let count = step["op"] == "switch_count";
                let value = if count {
                    json!(env.buf.len())
                } else {
                    let variable = text(step, "var")?;
                    expression::raw(variable, env)
                        .unwrap_or_else(|_| env.vars.get(variable).cloned().unwrap_or(Value::Null))
                };
                let mut branch = &step["default"];
                for case in list(&step["cases"])? {
                    if compare("==", &case[if count { "count" } else { "value" }], &value)? {
                        branch = &case["steps"];
                        break;
                    }
                }
                if execute_inner(optional_list(branch)?, env, sender, depth + 1)? {
                    return Ok(true);
                }
            }
            "for_each_row" => {
                let mut cursor = 0;
                while cursor + 3 <= env.buf.len() {
                    let (row, start, stop) =
                        (env.buf[cursor], env.buf[cursor + 1], env.buf[cursor + 2]);
                    if start > stop {
                        return Err(Error::Invalid("start column is past stop column".into()));
                    }
                    let length = (usize::from(stop) + 1 - usize::from(start)) * 3;
                    if cursor + 3 + length > env.buf.len() {
                        return Err(Error::Invalid("frame row is missing colour bytes".into()));
                    }
                    env.vars.extend([
                        ("row_id".into(), json!(row)),
                        ("start_col".into(), json!(start)),
                        ("stop_col".into(), json!(stop)),
                        ("row_length".into(), json!(length)),
                        ("offset".into(), json!(cursor + 3)),
                    ]);
                    if execute_inner(list(&step["steps"])?, env, sender, depth + 1)? {
                        return Ok(true);
                    }
                    cursor += 3 + length;
                }
            }
            "send_argb" => {
                let channel = number(&evaluate(&step["channel"], env)?)?;
                let data = builders::bytes(&evaluate(&step["data"], env)?)?;
                let size = index(&evaluate(&step["size"], env)?)?
                    .saturating_mul(3)
                    .min(data.len());
                sender.send_argb(channel as u8, &data[..size])?;
                env.sent += 1;
            }
            op => return Err(Error::Invalid(format!("unknown recipe op: {op}"))),
        }
    }
    Ok(false)
}

pub fn is_actionable(steps: &[Value]) -> bool {
    steps
        .iter()
        .any(|step| match step["op"].as_str().unwrap_or("") {
            "build" | "for_each_row" | "send_argb" => true,
            "call" => step["steps"].as_array().is_some_and(|s| is_actionable(s)),
            "if_count" => ["then", "else"]
                .iter()
                .any(|key| step[key].as_array().is_some_and(|s| is_actionable(s))),
            "switch_count" | "switch_var" => {
                step["cases"].as_array().is_some_and(|cases| {
                    cases
                        .iter()
                        .any(|case| case["steps"].as_array().is_some_and(|s| is_actionable(s)))
                }) || step["default"].as_array().is_some_and(|s| is_actionable(s))
            }
            _ => false,
        })
}

pub struct RecipeTable {
    payload: Value,
    actionable: Vec<bool>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct TransportParams {
    pub index: u8,
    pub wait_us: u64,
}

impl RecipeTable {
    pub fn from_json(payload: Value) -> Result<Self> {
        let actionable = list(&payload["pool"])?
            .iter()
            .map(|s| list(s).map(is_actionable))
            .collect::<Result<_>>()?;
        Ok(Self {
            payload,
            actionable,
        })
    }
    pub fn attributes(&self, driver: &str) -> Vec<&str> {
        self.payload["drivers"][driver]
            .as_object()
            .map(|v| v.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }
    pub fn has(&self, driver: &str, attribute: &str, pid: u16) -> bool {
        self.lookup(driver, attribute, pid).is_ok()
    }
    pub fn lookup(
        &self,
        driver: &str,
        attribute: &str,
        pid: u16,
    ) -> Result<(&[Value], Map<String, Value>)> {
        let table = &self.payload["drivers"][driver];
        let mut attribute = attribute;
        let mut seen = BTreeSet::new();
        let mut bindings = Map::new();
        loop {
            let entry = &table[attribute];
            if entry.is_null() {
                return Err(Error::NotSupported(format!(
                    "driver {driver} has no attribute {attribute}"
                )));
            }
            if let Some(alias) = entry["alias"].as_str() {
                if !seen.insert(attribute) {
                    return Err(Error::Invalid(format!("alias loop at {attribute}")));
                }
                if let Some(values) = entry["bind"].as_object() {
                    for (key, value) in values {
                        bindings.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
                attribute = alias;
                continue;
            }
            let key = format!("{pid:04x}");
            let reference = index(entry["pids"].get(&key).unwrap_or(&entry["default"]))?;
            if !self.actionable.get(reference).copied().unwrap_or(false) {
                return Err(Error::NotSupported(format!(
                    "device 0x{pid:04x} does not implement {attribute}"
                )));
            }
            return Ok((list(&self.payload["pool"][reference])?, bindings));
        }
    }
    pub fn transport_params(&self, driver: &str, pid: u16) -> Result<TransportParams> {
        let entry = &self.payload["transport"][driver];
        if entry.is_null() {
            return Ok(TransportParams {
                index: 0,
                wait_us: 600,
            });
        }
        let key = format!("{pid:04x}");
        Ok(serde_json::from_value(
            entry["pids"].get(&key).unwrap_or(&entry["default"]).clone(),
        )?)
    }
    pub fn is_blade(&self, pid: u16) -> bool {
        self.pid_in("blade_pids", pid)
    }
    pub fn is_kraken(&self, pid: u16) -> bool {
        self.pid_in("kraken_pids", pid)
    }
    fn pid_in(&self, field: &str, pid: u16) -> bool {
        self.payload[field].as_array().is_some_and(|values| {
            values
                .iter()
                .any(|v| v.as_str().and_then(|s| u16::from_str_radix(s, 16).ok()) == Some(pid))
        })
    }
}

pub fn recipes() -> &'static RecipeTable {
    static RECIPES: OnceLock<RecipeTable> = OnceLock::new();
    RECIPES.get_or_init(|| {
        RecipeTable::from_json(
            serde_json::from_str(include_str!(
                "../../../../openrazer_win/devices/data/recipes.json"
            ))
            .expect("embedded recipes must be valid JSON"),
        )
        .expect("embedded recipes must be valid programs")
    })
}
