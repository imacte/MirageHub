use super::{bindings, integer, Arguments, Device};
use crate::{
    database::DeviceInfo,
    persistence::{self, SharedStore, ZONES},
    protocol::{ble, builders},
    recipes::{self, Environment},
    transport::DeviceTransport,
    Error, Result,
};
use serde_json::{json, Map, Value};

pub const EFFECTS: [&str; 16] = [
    "none",
    "on",
    "static",
    "spectrum",
    "wave",
    "wheel",
    "reactive",
    "breath_random",
    "breath_single",
    "breath_dual",
    "breath_triple",
    "starlight_random",
    "starlight_single",
    "starlight_dual",
    "blinking",
    "custom",
];

pub struct StandardDevice {
    pub metadata: DeviceInfo,
    pub transport: Box<dyn DeviceTransport>,
    pub store: SharedStore,
    serial: Option<String>,
    firmware: Option<String>,
    in_driver_mode: bool,
}
impl StandardDevice {
    pub fn new(
        metadata: DeviceInfo,
        transport: Box<dyn DeviceTransport>,
        store: SharedStore,
    ) -> Self {
        Self {
            metadata,
            transport,
            store,
            serial: None,
            firmware: None,
            in_driver_mode: false,
        }
    }
    pub fn supports(&self, attribute: &str) -> bool {
        recipes::recipes().has(self.metadata.driver(), attribute, self.metadata.pid)
    }
    pub fn fallback_serial(&self) -> String {
        format!("RZ{:04X}{:04X}", self.metadata.vid, self.metadata.pid)
    }
    pub fn run(
        &mut self,
        attribute: &str,
        buf: Vec<u8>,
        vars: Map<String, Value>,
        require_response: bool,
    ) -> Result<Environment> {
        let (steps, mut merged) =
            recipes::recipes().lookup(self.metadata.driver(), attribute, self.metadata.pid)?;
        merged.extend(vars);
        merged.entry("varstore").or_insert(json!(1));
        let serial = self
            .serial
            .clone()
            .unwrap_or_else(|| self.fallback_serial());
        {
            let mut store = persistence::lock(&self.store);
            for (name, default) in [
                ("orochi2011_led", 3),
                ("orochi2011_poll", 2),
                ("orochi2011_dpi", 76),
            ] {
                merged
                    .entry(name)
                    .or_insert_with(|| store.get_device(&serial, name, json!(default)));
            }
        }
        let mut env = Environment::new(buf, merged);
        recipes::execute(steps, &mut env, self.transport.as_mut())?;
        if require_response && env.response.is_none() {
            return Err(Error::NotSupported(format!(
                "{attribute} produced no device response on {}",
                self.metadata.name
            )));
        }
        Ok(env)
    }
    fn read(&mut self, attribute: &str) -> Result<Environment> {
        self.run(&format!("read:{attribute}"), vec![], Map::new(), true)
    }
    fn write_number(&mut self, attribute: &str, variable: &str, value: i64) -> Result<()> {
        let mut vars = Map::new();
        vars.insert(variable.into(), json!(value));
        self.run(
            &format!("write:{attribute}"),
            value.to_string().into_bytes(),
            vars,
            false,
        )?;
        Ok(())
    }
    fn first(&self, attributes: &[String]) -> Result<String> {
        attributes
            .iter()
            .find(|name| self.supports(name))
            .cloned()
            .ok_or_else(|| {
                Error::NotSupported(format!(
                    "{} supports none of {attributes:?}",
                    self.metadata.name
                ))
            })
    }
    fn declares(&self, methods: &[String]) -> bool {
        self.metadata.methods.is_empty() || methods.iter().any(|m| self.metadata.has(m))
    }
    fn feature(&self, attribute: &str, methods: &[&str]) -> bool {
        self.supports(attribute)
            && self.declares(&methods.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    fn brightness_attribute(&self, zone: &str, write: bool) -> Result<String> {
        if !ZONES.contains(&zone) {
            return Err(Error::NotSupported(format!("unknown zone {zone}")));
        }
        let names = if zone == "backlight" {
            vec![
                "matrix_brightness".into(),
                "backlight_led_brightness".into(),
                "led_brightness".into(),
            ]
        } else {
            vec![format!("{zone}_led_brightness")]
        };
        self.first(
            &names
                .iter()
                .map(|name| format!("{}:{name}", if write { "write" } else { "read" }))
                .collect::<Vec<_>>(),
        )
    }
    fn effect_attribute(&self, zone: &str, effect: &str) -> Result<String> {
        if !ZONES.contains(&zone) || !EFFECTS.contains(&effect) {
            return Err(Error::NotSupported(format!(
                "unknown zone/effect {zone}/{effect}"
            )));
        }
        let suffix = if effect.starts_with("breath_") {
            "breath"
        } else if effect.starts_with("starlight_") {
            "starlight"
        } else {
            effect
        };
        let names = if zone == "backlight" {
            vec![
                format!("write:matrix_effect_{suffix}"),
                format!("write:backlight_matrix_effect_{suffix}"),
            ]
        } else {
            vec![format!("write:{zone}_matrix_effect_{suffix}")]
        };
        self.first(&names)
    }
    pub fn available_effects(&self, zone: &str) -> Vec<&str> {
        EFFECTS
            .iter()
            .copied()
            .filter(|effect| {
                let mut names = if zone == "backlight" {
                    vec![
                        format!("set_{effect}_effect"),
                        format!("set_backlight_{effect}"),
                    ]
                } else {
                    vec![format!("set_{zone}_{effect}")]
                };
                if *effect == "on" && zone != "backlight" {
                    names.push(format!("set_{zone}_active"));
                }
                self.declares(&names) && self.effect_attribute(zone, effect).is_ok()
            })
            .collect()
    }
    fn remember(&mut self, zone: &str, key: &str, value: Value) {
        let serial = self.serial();
        persistence::lock(&self.store).set(&serial, zone, key, value);
    }
    fn remember_device(&mut self, key: &str, value: Value) {
        let serial = self.serial();
        persistence::lock(&self.store).set_device(&serial, key, value);
    }
    fn remember_colours(&mut self, zone: &str, colours: &[Value]) {
        let serial = self.serial();
        let mut store = persistence::lock(&self.store);
        let mut saved = store
            .get(&serial, zone, "colors")
            .as_array()
            .cloned()
            .unwrap_or_default();
        saved.resize(9, json!(0));
        saved.truncate(9);
        for (to, from) in saved.iter_mut().zip(colours) {
            *to = from.clone();
        }
        store.set(&serial, zone, "colors", json!(saved));
    }
    fn apply(
        &mut self,
        zone: &str,
        effect: &str,
        payload: Vec<u8>,
        vars: Map<String, Value>,
    ) -> Result<()> {
        let attribute = self.effect_attribute(zone, effect)?;
        self.run(&attribute, payload, vars, false)?;
        self.remember(zone, "effect", json!(effect));
        Ok(())
    }
    fn lighting(&mut self, effect: &str, args: &Arguments<'_>) -> Result<Value> {
        let colour_count = match effect {
            "static" | "reactive" | "blinking" | "breath_single" | "starlight_single" => 3,
            "breath_dual" | "starlight_dual" => 6,
            "breath_triple" => 9,
            _ => 0,
        };
        let mut colours = Vec::new();
        for i in 0..colour_count {
            let channel = ["red", "green", "blue"][i % 3];
            let name = if colour_count > 3 {
                format!("{channel}{}", i / 3 + 1)
            } else {
                channel.into()
            };
            colours.push(json!(args.number(&name, i, None)?));
        }
        let mut payload = colours
            .iter()
            .map(|v| integer(v).map(|n| n.clamp(0, 255) as u8))
            .collect::<Result<Vec<_>>>()?;
        let mut vars = Map::new();
        let mut extras = Vec::new();
        let mut zone_position = colour_count;
        if effect == "reactive" || effect.starts_with("starlight_") {
            let speed = args
                .number("speed", colour_count, Some(1))?
                .clamp(1, if effect == "reactive" { 4 } else { 3 });
            payload.insert(0, speed as u8);
            vars.insert("speed".into(), json!(speed));
            extras.push(("speed", json!(speed)));
            zone_position += 1;
        } else if effect == "wave" || effect == "wheel" {
            let direction = args
                .number("direction", 0, Some(1))?
                .clamp(i64::from(effect == "wheel"), 2);
            payload = direction.to_string().into_bytes();
            vars.insert("direction".into(), json!(direction));
            extras.push(("wave_dir", json!(direction)));
            zone_position = 1;
        } else if effect == "breath_random" {
            payload = b"1".to_vec();
        } else if colour_count == 0 {
            payload.push(1);
        }
        let zone = args.zone(zone_position)?;
        self.apply(&zone, effect, payload, vars)?;
        for (key, value) in extras {
            self.remember(&zone, key, value);
        }
        if !colours.is_empty() {
            self.remember_colours(&zone, &colours);
        }
        Ok(Value::Null)
    }
    fn poll_rates(&self) -> Vec<u32> {
        self.metadata
            .poll_rates
            .clone()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec![125, 500, 1000])
    }
    fn set_mode(&mut self, mode: i64, param: i64) -> Result<()> {
        self.run(
            "write:device_mode",
            vec![mode as u8, param as u8],
            bindings(json!({"mode":mode,"param":param})),
            false,
        )?;
        Ok(())
    }
}

impl Device for StandardDevice {
    fn info(&self) -> &DeviceInfo {
        &self.metadata
    }
    fn persistence(&self) -> &SharedStore {
        &self.store
    }
    fn serial(&mut self) -> String {
        if let Some(serial) = &self.serial {
            return serial.clone();
        }
        let serial = self
            .read("device_serial")
            .ok()
            .and_then(|env| env.response)
            .map(|report| {
                report.arguments[..22]
                    .iter()
                    .take_while(|b| **b != 0)
                    .filter(|b| b.is_ascii())
                    .map(|b| char::from(*b))
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.fallback_serial());
        self.serial = Some(serial.clone());
        serial
    }
    fn firmware(&mut self) -> Result<String> {
        if let Some(firmware) = &self.firmware {
            return Ok(firmware.clone());
        }
        let report = self
            .read("firmware_version")?
            .response
            .expect("read requires response");
        let firmware = format!("v{}.{}", report.arguments[0], report.arguments[1]);
        self.firmware = Some(firmware.clone());
        Ok(firmware)
    }
    fn capabilities(&self) -> Value {
        let mut zones = Map::new();
        for zone in ZONES {
            let effects = self.available_effects(zone);
            let methods = if zone == "backlight" {
                vec!["set_brightness".into(), "set_backlight_brightness".into()]
            } else {
                vec![format!("set_{zone}_brightness")]
            };
            let brightness =
                self.declares(&methods) && self.brightness_attribute(zone, true).is_ok();
            if effects == ["custom"] && !brightness {
                continue;
            }
            if !effects.is_empty() || brightness {
                zones.insert(
                    zone.into(),
                    json!({"effects":effects,"brightness":brightness}),
                );
            }
        }
        let custom = self.feature("write:matrix_custom_frame", &["set_key_row"]);
        let dims = self.metadata.matrix_dims.unwrap_or([1, 1]);
        json!({
            "name":self.metadata.name,"type":self.metadata.device_type,"vid":self.metadata.vid,"pid":self.metadata.pid,
            "image":self.metadata.image,"zones":zones,"matrix":self.metadata.has_matrix,"matrix_dimensions":self.metadata.matrix_dims,
            "dedicated_macro_keys":self.metadata.dedicated_macro_keys,
            "dpi":self.feature("read:dpi", &["get_dpi_xy","get_dpi_xy_byte"]),"dpi_stages":self.feature("read:dpi_stages", &["get_dpi_stages"]),
            "max_dpi":self.metadata.dpi_max,"poll_rate":self.feature("read:poll_rate", &["get_poll_rate"]),"supported_poll_rates":self.poll_rates(),
            "battery":self.feature("read:charge_level", &["get_battery"]),"idle_time":self.feature("read:device_idle_time", &["get_idle_time"]),
            "game_mode":self.feature("read:game_led_state", &["get_game_mode"]),"macro_mode":self.feature("read:macro_led_state", &["get_macro_mode"]),
            "keyboard_layout":self.feature("read:kbd_layout", &["get_keyboard_layout"]),"scroll_mode":self.feature("read:scroll_mode", &["get_scroll_mode"]),
            "custom_frame":custom,"software_effects":custom && dims[0] * dims[1] > 1,
            "transport":{"path":self.transport.info().path,"interface":self.transport.info().interface,"wait_us":self.transport.wait().as_micros() as u64}
        })
    }
    fn enter_driver_mode(&mut self, keys_handled: bool) -> Result<()> {
        if self.supports("probe:device_mode") {
            self.run("probe:device_mode", vec![], Map::new(), false)?;
        }
        if self.metadata.driver_mode && (keys_handled || self.metadata.driver() != "kbd") {
            self.set_mode(3, 0)?;
            self.in_driver_mode = true;
        }
        Ok(())
    }
    fn leave_driver_mode(&mut self) -> Result<()> {
        if self.in_driver_mode {
            self.set_mode(0, 0)?;
            self.in_driver_mode = false;
        }
        Ok(())
    }
    fn restore(&mut self) {
        super::restore_lighting(self);
    }
    fn close(&mut self) {
        self.transport.close();
    }
    fn call(
        &mut self,
        method: &str,
        values: &[Value],
        kwargs: &Map<String, Value>,
    ) -> Result<Value> {
        let args = Arguments {
            args: values,
            kwargs,
        };
        if let Some(effect) = method
            .strip_prefix("set_")
            .filter(|effect| EFFECTS.contains(effect))
        {
            return self.lighting(effect, &args);
        }
        match method {
            "available_effects" => return Ok(json!(self.available_effects(&args.zone(0)?))),
            "get_device_mode" => {
                let r = self.read("device_mode")?.response.unwrap();
                return Ok(json!(format!("{}:{}", r.arguments[0], r.arguments[1])));
            }
            "set_device_mode" => self.set_mode(
                args.number("mode", 0, None)?,
                args.number("param", 1, Some(0))?,
            )?,
            "get_brightness" => {
                let zone = args.zone(0)?;
                let attribute = match self.brightness_attribute(&zone, false) {
                    Ok(attribute) => attribute,
                    Err(Error::NotSupported(_)) => {
                        let serial = self.serial();
                        return Ok(persistence::lock(&self.store).get(
                            &serial,
                            &zone,
                            "brightness",
                        ));
                    }
                    Err(error) => return Err(error),
                };
                let env = self.run(&attribute, vec![], Map::new(), true)?;
                let raw = env
                    .vars
                    .get("brightness")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(i64::from(env.response.unwrap().arguments[2]));
                return Ok(json!(ble::level_to_percentage(raw)));
            }
            "set_brightness" => {
                let percent = args.float("percent", 0, None)?.clamp(0.0, 100.0);
                let raw = ble::percentage_to_level(percent)?;
                let zone = args.zone(1)?;
                let attribute = self.brightness_attribute(&zone, true)?;
                self.run(
                    &attribute,
                    raw.to_string().into_bytes(),
                    bindings(json!({"brightness":raw,"saved_brightness":raw})),
                    false,
                )?;
                self.remember(&zone, "brightness", json!(percent));
            }
            "trigger_reactive" => {
                self.run(
                    "write:matrix_reactive_trigger",
                    b"1".to_vec(),
                    Map::new(),
                    false,
                )?;
            }
            "set_key_row" => {
                self.run(
                    "write:matrix_custom_frame",
                    builders::bytes(&args.value("payload", 0, None)?)?,
                    Map::new(),
                    false,
                )?;
            }
            "set_custom_frame" => {
                let rows = args.value("rows", 0, None)?;
                let rows = rows
                    .as_array()
                    .ok_or_else(|| Error::Invalid("rows must be an array".into()))?;
                let mut payloads = Vec::new();
                for row in rows {
                    let row = row.as_array().filter(|r| r.len() == 3).ok_or_else(|| {
                        Error::Invalid("frame row requires index, start and colours".into())
                    })?;
                    let index = integer(&row[0])?;
                    let start = integer(&row[1])?;
                    let colours = builders::bytes(&row[2])?;
                    let stop = start + (colours.len() / 3) as i64 - 1;
                    if !(0..=255).contains(&index)
                        || !(0..=255).contains(&start)
                        || !(start..=255).contains(&stop)
                        || colours.len() % 3 != 0
                    {
                        return Err(Error::Invalid("invalid custom frame row".into()));
                    }
                    let mut payload = vec![index as u8, start as u8, stop as u8];
                    payload.extend(colours);
                    payloads.push(payload);
                }
                let attribute = self.effect_attribute("backlight", "custom")?;
                let argb = self.transport.argb_info().is_some();
                if argb {
                    self.run(&attribute, b"1".to_vec(), Map::new(), false)?;
                }
                for payload in payloads {
                    self.run("write:matrix_custom_frame", payload, Map::new(), false)?;
                }
                if !argb {
                    self.run(&attribute, b"1".to_vec(), Map::new(), false)?;
                }
            }
            "get_dpi" => {
                let env = self.read("dpi")?;
                let a = env.response.unwrap().arguments;
                return Ok(if self.metadata.has("set_dpi_xy_byte") {
                    json!([u32::from(a[0]) * 100, u32::from(a[1]) * 100])
                } else if let Some(x) = env.vars.get("dpi_x") {
                    json!([integer(x)?, integer(env.vars.get("dpi_y").unwrap_or(x))?])
                } else {
                    json!([
                        u16::from_be_bytes([a[1], a[2]]),
                        u16::from_be_bytes([a[3], a[4]])
                    ])
                });
            }
            "set_dpi" => {
                let x = args.number("dpi_x", 0, None)?;
                let y = args.value("dpi_y", 1, Some(Value::Null))?;
                let y = if y.is_null() { x } else { integer(&y)? };
                let maximum = i64::from(self.metadata.dpi_max.unwrap_or(45000));
                let (x, y) = (x.clamp(100, maximum), y.clamp(100, maximum));
                let mut vars = bindings(json!({"dpi_x":x,"dpi_y":y}));
                let payload = if self.metadata.has("set_dpi_xy_byte") {
                    let (bx, by) = ((x / 100).clamp(1, 255), (y / 100).clamp(1, 255));
                    vars.insert("dpi_x_byte".into(), json!(bx));
                    vars.insert("dpi_y_byte".into(), json!(by));
                    vec![bx as u8, by as u8]
                } else {
                    [(x as u16).to_be_bytes(), (y as u16).to_be_bytes()].concat()
                };
                self.run("write:dpi", payload, vars, false)?;
                self.remember_device("dpi", json!([x, y]));
            }
            "get_dpi_stages" => {
                let a = self.read("dpi_stages")?.response.unwrap().arguments;
                let stages: Vec<_> = (0..usize::from(a[2]).min(10))
                    .map(|i| {
                        let o = 3 + i * 7;
                        [
                            u16::from_be_bytes([a[o + 1], a[o + 2]]),
                            u16::from_be_bytes([a[o + 3], a[o + 4]]),
                        ]
                    })
                    .collect();
                return Ok(json!([a[1], stages]));
            }
            "set_dpi_stages" => {
                let active = args.number("active_stage", 0, None)?;
                let stages = args.value("stages", 1, None)?;
                let stages = stages
                    .as_array()
                    .filter(|s| s.len() <= 10)
                    .ok_or_else(|| Error::Invalid("invalid DPI stages".into()))?;
                let mut payload = vec![active.clamp(1, stages.len().max(1) as i64) as u8];
                let mut flat = Vec::new();
                for stage in stages {
                    let pair = stage
                        .as_array()
                        .filter(|p| p.len() == 2)
                        .ok_or_else(|| Error::Invalid("DPI stage requires an x/y pair".into()))?;
                    for v in pair {
                        let v = u16::try_from(integer(v)?)
                            .map_err(|_| Error::Invalid("DPI stage out of range".into()))?;
                        payload.extend(v.to_be_bytes());
                        flat.push(v);
                    }
                }
                self.run(
                    "write:dpi_stages",
                    payload,
                    bindings(json!({"stages_count":stages.len(),"active_stage":active,"dpi":flat})),
                    false,
                )?;
                self.remember_device("dpi_stages", json!({"active":active,"stages":stages}));
            }
            "get_poll_rate" => {
                let env = self.run("read:poll_rate", vec![], Map::new(), false)?;
                return env
                    .vars
                    .get("polling_rate")
                    .map(|v| integer(v).map(|n| json!(n)))
                    .unwrap_or_else(|| {
                        Err(Error::NotSupported(
                            "device does not report polling rate".into(),
                        ))
                    });
            }
            "set_poll_rate" => {
                let rate = args.number("rate", 0, None)?;
                self.write_number("poll_rate", "polling_rate", rate)?;
                self.remember_device("poll_rate", json!(rate));
                self.remember_device(
                    "orochi2011_poll",
                    json!(match rate {
                        1000 => 1,
                        125 => 8,
                        _ => 2,
                    }),
                );
            }
            "supported_poll_rates" => return Ok(json!(self.poll_rates())),
            "get_battery_level" => {
                return Ok(json!(ble::level_to_percentage(i64::from(
                    self.read("charge_level")?.response.unwrap().arguments[1]
                ))))
            }
            "is_charging" => {
                return Ok(json!(
                    self.read("charge_status")?.response.unwrap().arguments[1] != 0
                ))
            }
            "get_idle_time" => {
                let a = self.read("device_idle_time")?.response.unwrap().arguments;
                return Ok(json!(u16::from_be_bytes([a[0], a[1]])));
            }
            "set_idle_time" => self.write_number(
                "device_idle_time",
                "idle_time",
                args.number("seconds", 0, None)?.clamp(60, 900),
            )?,
            "get_low_battery_threshold" => {
                return Ok(json!(ble::level_to_percentage(i64::from(
                    self.read("charge_low_threshold")?
                        .response
                        .unwrap()
                        .arguments[0]
                ))))
            }
            "set_low_battery_threshold" => self.write_number(
                "charge_low_threshold",
                "threshold",
                i64::from(ble::percentage_to_level(args.float("percent", 0, None)?)?),
            )?,
            "get_game_mode" | "get_macro_mode" => {
                let attr = if method == "get_game_mode" {
                    "game_led_state"
                } else {
                    "macro_led_state"
                };
                return Ok(json!(self.read(attr)?.response.unwrap().arguments[2] != 0));
            }
            "set_game_mode" | "set_macro_mode" => {
                let attr = if method == "set_game_mode" {
                    "game_led_state"
                } else {
                    "macro_led_state"
                };
                self.write_number(
                    attr,
                    "enabled",
                    i64::from(args.boolean("enabled", 0, None)?),
                )?;
            }
            "set_macro_effect" => self.write_number(
                "macro_led_effect",
                "enabled",
                args.number("effect", 0, None)?,
            )?,
            "set_fn_toggle" => {
                self.write_number("fn_toggle", "state", args.number("state", 0, None)?)?
            }
            "get_keyboard_layout" => {
                return Ok(json!(format!(
                    "layout_{:02x}",
                    self.read("kbd_layout")?.response.unwrap().arguments[2]
                )))
            }
            "get_scroll_mode" => {
                return Ok(json!(
                    self.read("scroll_mode")?.response.unwrap().arguments[1]
                ))
            }
            "set_scroll_mode" => {
                self.write_number("scroll_mode", "scroll_mode", args.number("mode", 0, None)?)?
            }
            "get_scroll_acceleration" | "get_scroll_smart_reel" => {
                return Ok(json!(
                    self.read(&method[4..])?.response.unwrap().arguments[1] != 0
                ))
            }
            "set_scroll_acceleration" | "set_scroll_smart_reel" => {
                self.write_number(
                    &method[4..],
                    if method.ends_with("acceleration") {
                        "acceleration"
                    } else {
                        "smart_reel"
                    },
                    i64::from(args.boolean("enabled", 0, None)?),
                )?;
            }
            "restore" => self.restore(),
            _ => {
                return Err(Error::NotSupported(format!(
                    "device method {method} is not supported"
                )))
            }
        }
        Ok(Value::Null)
    }
}
