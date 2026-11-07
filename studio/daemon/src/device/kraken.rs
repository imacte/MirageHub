use super::{bindings, integer, Arguments, Device, StandardDevice};
use crate::{
    database::DeviceInfo,
    persistence::{self, SharedStore},
    protocol::kraken as protocol,
    transport::DeviceTransport,
    Error, Result,
};
use serde_json::{json, Map, Value};

pub struct KrakenDevice {
    base: StandardDevice,
}
impl KrakenDevice {
    pub fn new(
        metadata: DeviceInfo,
        transport: Box<dyn DeviceTransport>,
        store: SharedStore,
    ) -> Self {
        Self {
            base: StandardDevice::new(metadata, transport, store),
        }
    }
    fn effects(&self) -> Vec<&str> {
        let Some(layout) = protocol::layout_for(self.base.metadata.pid) else {
            return vec![];
        };
        [
            "none",
            "static",
            "spectrum",
            "breath_single",
            "breath_dual",
            "breath_triple",
            "custom",
        ]
        .into_iter()
        .filter(|e| {
            if !protocol::has_colour(self.base.metadata.pid) && !["none", "spectrum"].contains(e) {
                return false;
            }
            match *e {
                "breath_dual" => layout.breathing.len() >= 2,
                "breath_triple" => layout.breathing.len() >= 3,
                _ => true,
            }
        })
        .collect()
    }
    fn remember(&mut self, effect: &str, colours: &[u8]) {
        let serial = self.serial();
        let mut store = persistence::lock(&self.base.store);
        store.set(&serial, "backlight", "effect", json!(effect));
        if !colours.is_empty() {
            let mut padded = colours.to_vec();
            padded.resize(9, 0);
            store.set(&serial, "backlight", "colors", json!(padded));
        }
    }
}
impl Device for KrakenDevice {
    fn info(&self) -> &DeviceInfo {
        &self.base.metadata
    }
    fn persistence(&self) -> &SharedStore {
        &self.base.store
    }
    fn serial(&mut self) -> String {
        self.base.fallback_serial()
    }
    fn firmware(&mut self) -> Result<String> {
        Err(Error::NotSupported(
            "Kraken firmware readback is not supported".into(),
        ))
    }
    fn capabilities(&self) -> Value {
        let info = &self.base.metadata;
        let effects = self.effects();
        json!({"name":info.name,"type":info.device_type,"vid":info.vid,"pid":info.pid,"image":info.image,"protocol":"kraken",
            "zones":if effects.is_empty(){json!({})}else{json!({"backlight":{"effects":effects,"brightness":false}})},
            "matrix":false,"matrix_dimensions":null,"dedicated_macro_keys":false,"dpi":false,"dpi_stages":false,"max_dpi":null,
            "poll_rate":false,"supported_poll_rates":[],"battery":false,"idle_time":false,"game_mode":false,"macro_mode":false,
            "keyboard_layout":false,"scroll_mode":false,"custom_frame":false,"software_effects":false,"readback":false,
            "transport":{"path":self.base.transport.kraken_info().map(|i|&i.path),"interface":self.base.transport.kraken_info().and_then(|i|i.interface),"wait_us":15000}})
    }
    fn enter_driver_mode(&mut self, _keys_handled: bool) -> Result<()> {
        Ok(())
    }
    fn leave_driver_mode(&mut self) -> Result<()> {
        Ok(())
    }
    fn close(&mut self) {
        self.base.close();
    }
    fn restore(&mut self) {
        let serial = self.serial();
        let state = persistence::lock(&self.base.store).saved_zone(&serial, "backlight");
        let Some(state) = state else { return };
        let Some(effect) = state.get("effect").and_then(Value::as_str) else {
            return;
        };
        let mut colours = state
            .get("colors")
            .and_then(Value::as_array)
            .cloned()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                vec![
                    json!(0),
                    json!(255),
                    json!(0),
                    json!(0),
                    json!(255),
                    json!(255),
                    json!(0),
                    json!(0),
                    json!(255),
                ]
            });
        colours.resize(9, json!(0));
        let (args, kwargs) = match effect {
            "static" | "breath_single" => (colours[..3].to_vec(), Map::new()),
            "breath_dual" => (colours[..6].to_vec(), Map::new()),
            "breath_triple" => (colours, Map::new()),
            "custom" => (
                vec![],
                bindings(json!({"red":colours[0],"green":colours[1],"blue":colours[2]})),
            ),
            "none" | "spectrum" => (vec![], Map::new()),
            _ => return,
        };
        let _ = self.call(&format!("set_{effect}"), &args, &kwargs);
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
        if method == "available_effects" {
            return Ok(json!(if args.zone(0)? == "backlight" {
                self.effects()
            } else {
                vec![]
            }));
        }
        if method == "supported_poll_rates" {
            return Ok(json!(self
                .base
                .metadata
                .poll_rates
                .clone()
                .unwrap_or_else(|| vec![125, 500, 1000])));
        }
        if method == "restore" {
            self.restore();
            return Ok(Value::Null);
        }
        let effect = method
            .strip_prefix("set_")
            .filter(|e| self.effects().contains(e))
            .ok_or_else(|| Error::NotSupported(format!("Kraken does not support {method}")))?;
        let layout = protocol::layout_for(self.base.metadata.pid)
            .ok_or_else(|| Error::NotSupported("unknown Kraken layout".into()))?;
        let count = match effect {
            "static" | "custom" | "breath_single" => 3,
            "breath_dual" => 6,
            "breath_triple" => 9,
            _ => 0,
        };
        let mut colours = Vec::new();
        for i in 0..count {
            let channel = ["red", "green", "blue"][i % 3];
            let name = if count > 3 {
                format!("{channel}{}", i / 3 + 1)
            } else {
                channel.into()
            };
            let default = if effect == "custom" {
                Some(if i == 1 { 255 } else { 0 })
            } else {
                None
            };
            colours.push(
                args.number(&name, i + usize::from(effect == "custom"), default)?
                    .clamp(0, 255) as u8,
            );
        }
        let intensity = if effect == "static" || effect == "custom" {
            let value = args.value("intensity", 4, Some(Value::Null))?;
            if value.is_null() {
                None
            } else {
                Some(integer(&value)?.clamp(0, 255) as u8)
            }
        } else {
            None
        };
        let reports = match effect {
            "none" => protocol::effect_none(layout)?,
            "spectrum" => protocol::effect_spectrum(layout)?,
            "static" => protocol::effect_static(
                layout,
                colours[..3].try_into().unwrap(),
                intensity,
                protocol::has_colour(self.base.metadata.pid),
            )?,
            "custom" => {
                protocol::effect_custom(layout, colours[..3].try_into().unwrap(), intensity)?
            }
            _ => protocol::effect_breathing(
                layout,
                &colours
                    .chunks_exact(3)
                    .map(|v| v.try_into().unwrap())
                    .collect::<Vec<_>>(),
            )?,
        };
        self.base
            .transport
            .send_kraken(&reports, protocol::WRITE_SETTLE)?;
        self.remember(effect, &colours);
        Ok(Value::Null)
    }
}
