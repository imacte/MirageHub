use mirage_hub_daemon::{
    protocol::{builders, report::Report},
    recipes::{self, Environment, Sender},
    Result,
};
use serde_json::{json, Value};

#[derive(Default)]
struct Recorder {
    sent: Vec<Value>,
}

impl Sender for Recorder {
    fn send(&mut self, request: &Report, want_response: bool) -> Result<Option<Report>> {
        self.sent
            .push(json!({"packet": request.pack().as_slice(), "response": want_response}));
        if !want_response {
            return Ok(None);
        }
        let mut reply = request.clone();
        reply.status = 2;
        for (index, arg) in reply.arguments.iter_mut().enumerate() {
            *arg = (index * 17 + 23) as u8;
        }
        Ok(Some(reply))
    }
    fn send_argb(&mut self, channel: u8, data: &[u8]) -> Result<()> {
        self.sent.push(json!({"channel": channel, "argb": data}));
        Ok(())
    }
}

#[test]
fn all_devices_resolve_the_same_recipes_and_transport_settings() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/recipes.json")).unwrap();
    let table = recipes::recipes();
    for device in fixture["lookups"].as_array().unwrap() {
        let driver = device["driver"].as_str().unwrap();
        let pid = device["pid"].as_u64().unwrap() as u16;
        let expected = device["attributes"].as_object().unwrap();
        let mut supported = 0;
        for attribute in table.attributes(driver) {
            match table.lookup(driver, attribute, pid) {
                Ok((steps, bindings)) => {
                    let lookup = expected
                        .get(attribute)
                        .unwrap_or_else(|| panic!("unexpected {driver} {pid:04x} {attribute}"));
                    assert_eq!(
                        steps,
                        fixture["pool"][lookup[0].as_u64().unwrap() as usize]
                            .as_array()
                            .unwrap()
                    );
                    assert_eq!(bindings, *lookup[1].as_object().unwrap());
                    supported += 1;
                }
                Err(_) => assert!(
                    !expected.contains_key(attribute),
                    "missing {driver} {pid:04x} {attribute}"
                ),
            }
        }
        assert_eq!(supported, expected.len());
        let params = table.transport_params(driver, pid).unwrap();
        assert_eq!(
            json!({"index": params.index, "wait_us": params.wait_us}),
            device["transport"]
        );
        assert_eq!(table.is_blade(pid), device["blade"].as_bool().unwrap());
        assert_eq!(table.is_kraken(pid), device["kraken"].as_bool().unwrap());
    }
}

#[test]
fn execution_matches_the_python_recipe_interpreter() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/recipes.json")).unwrap();
    let table = recipes::recipes();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() > 5000);
    for (index, case) in cases.iter().enumerate() {
        let driver = case["driver"].as_str().unwrap();
        let pid = case["pid"].as_u64().unwrap() as u16;
        let attribute = case["attribute"].as_str().unwrap();
        let label = format!(
            "case {index}: {driver} {pid:04x} {attribute} {}",
            case["buf"]
        );
        let (steps, _) = table.lookup(driver, attribute, pid).unwrap();
        let mut vars = fixture["variables"].as_object().unwrap().clone();
        vars.extend(case["bindings"].as_object().unwrap().clone());
        let mut expected_vars = vars.clone();
        expected_vars.extend(case["changed_vars"].as_object().unwrap().clone());
        for key in case["removed_vars"].as_array().unwrap() {
            expected_vars.remove(key.as_str().unwrap());
        }
        let mut env = Environment::new(builders::bytes(&case["buf"]).unwrap(), vars);
        let mut recorder = Recorder::default();
        let result = recipes::execute(steps, &mut env, &mut recorder);
        assert_eq!(
            result.is_ok(),
            case["error"].is_null(),
            "{label}: {result:?}"
        );
        assert_eq!(recorder.sent, *case["sent"].as_array().unwrap(), "{label}");
        assert_eq!(json!(env.out), case["out"], "{label}");
        assert_eq!(env.vars, expected_vars, "{label}");
        let reply = env
            .response
            .map(|r| json!(r.pack().as_slice()))
            .unwrap_or(Value::Null);
        assert_eq!(reply, case["reply"], "{label}");
    }
}

#[test]
fn stop_and_helper_return_have_distinct_scopes() {
    let steps = json!([
        {"op":"let", "name":"outer", "value":7},
        {"op":"call", "vars":{"outer":9}, "result":"answer", "steps":[
            {"op":"let", "name":"__return__", "value":42}, {"op":"stop"},
            {"op":"let", "name":"unreachable", "value":1}
        ]},
        {"op":"let", "name":"after_helper", "value":1},
        {"op":"if_count", "test":{"op":"==", "value":0}, "then":[{"op":"stop"}]},
        {"op":"let", "name":"unreachable", "value":2}
    ]);
    let mut env = Environment::default();
    recipes::execute(
        steps.as_array().unwrap(),
        &mut env,
        &mut Recorder::default(),
    )
    .unwrap();
    assert_eq!(
        json!(env.vars),
        json!({"outer":7,"answer":42,"after_helper":1})
    );
}
