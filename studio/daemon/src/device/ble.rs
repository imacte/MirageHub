use super::{integer, Arguments, Device};
use crate::{
    ble::BleChannel,
    database::DeviceInfo,
    persistence::{self, SharedStore},
    protocol::ble,
    Error, Result,
};
use serde_json::{json, Map, Value};

pub struct BleDevice {
    metadata: DeviceInfo,
    transport: Box<dyn BleChannel>,
    store: SharedStore,
    pub classic_address: Option<u64>,
}
impl BleDevice {
    pub fn new(metadata: DeviceInfo, transport: Box<dyn BleChannel>, store: SharedStore) -> Self {
        Self {
            metadata,
            transport,
            store,
            classic_address: None,
        }
    }
    fn current_colours(&mut self) -> [[u8; 3]; 2] {
        let serial = self.serial();
        let value = persistence::lock(&self.store).get_device(&serial, "zone_colours", Value::Null);
        let stored = value.as_array().map(Vec::as_slice).unwrap_or_default();
        let mut colours = [[0; 3]; 2];
        for (i, colour) in colours.iter_mut().enumerate() {
            if let Some(chunk) = stored.get(i * 3..i * 3 + 3) {
                for (out, value) in colour.iter_mut().zip(chunk) {
                    *out = integer(value).unwrap_or(0).clamp(0, 255) as u8;
                }
            }
        }
        colours
    }
    fn set_colours(&mut self, colours: &[[u8; 3]], hold: bool) -> Result<()> {
        let packet = ble::colour_command(colours)?;
        let colours = if colours.len() == 1 {
            vec![colours[0]; 2]
        } else {
            colours.to_vec()
        };
        self.transport.write(&packet, hold)?;
        let flat: Vec<_> = colours.iter().flatten().copied().collect();
        let mut padded = flat.clone();
        padded.resize(9, 0);
        let serial = self.serial();
        let mut store = persistence::lock(&self.store);
        store.set(&serial, "backlight", "effect", json!("static"));
        store.set_device(&serial, "zone_colours", json!(flat));
        store.set(&serial, "backlight", "colors", json!(padded));
        Ok(())
    }
    fn static_colour(&mut self, colour: [u8; 3], zone: &str) -> Result<()> {
        let colours = if let Some(index) = ble::ZONES.iter().position(|name| *name == zone) {
            let mut colours = self.current_colours();
            colours[index] = colour;
            colours
        } else {
            [colour; 2]
        };
        self.set_colours(&colours, true)
    }
    fn read(&mut self, opcode: u8) -> Result<u8> {
        self.transport
            .request(opcode, &[])?
            .first()
            .copied()
            .ok_or_else(|| Error::NotSupported(format!("device did not report 0x{opcode:02x}")))
    }
}
impl Device for BleDevice {
    fn info(&self) -> &DeviceInfo {
        &self.metadata
    }
    fn persistence(&self) -> &SharedStore {
        &self.store
    }
    fn serial(&mut self) -> String {
        format!("BLE{:012X}", self.transport.address())
    }
    fn firmware(&mut self) -> Result<String> {
        Err(Error::NotSupported(
            "firmware is unavailable over Bluetooth".into(),
        ))
    }
    fn capabilities(&self) -> Value {
        let m = &self.metadata;
        json!({"name":m.name,"type":m.device_type,"vid":m.vid,"pid":m.pid,"image":m.image,
            "protocol":"razer-ble","zones":{
                "backlight":{"effects":["none","static"],"brightness":true},
                "left":{"effects":["none","static"],"brightness":false},
                "right":{"effects":["none","static"],"brightness":false}},
            "zone_order":ble::ZONES,"zone_colours":true,"matrix":true,"matrix_dimensions":[1,2],
            "dedicated_macro_keys":false,"dpi":false,"dpi_stages":false,"max_dpi":null,
            "poll_rate":false,"supported_poll_rates":[],"battery":true,"idle_time":false,
            "game_mode":false,"macro_mode":false,"keyboard_layout":false,"keyswitch_optimization":false,"scroll_mode":false,
            "custom_frame":true,"software_effects":true,"readback":true,
            "transport":{"path":format!("{:012X}",self.transport.address()),"interface":null,"wait_us":0},
            "bluetooth_address":self.classic_address.map(|a| format!("{a:012X}"))})
    }
    fn enter_driver_mode(&mut self, _keys_handled: bool) -> Result<()> {
        let _ = self.transport.write(&ble::takeover_command(), false);
        Ok(())
    }
    fn leave_driver_mode(&mut self) -> Result<()> {
        Ok(())
    }
    fn restore(&mut self) -> Result<()> {
        let serial = self.serial();
        let state = persistence::lock(&self.store).saved_zone(&serial, "backlight");
        if state
            .as_ref()
            .and_then(|s| s.get("effect"))
            .and_then(Value::as_str)
            == Some("static")
        {
            let colours = self.current_colours();
            self.set_colours(&colours, true)?;
        }
        Ok(())
    }
    fn close(&mut self) {
        self.transport.close();
    }
    fn is_connected(&self) -> bool {
        self.transport.is_connected()
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
        match method {
            "available_effects" => return Ok(json!(["none", "static"])),
            "zone_colours_now" => return Ok(json!(self.current_colours())),
            "set_static" => {
                let colour = [
                    args.number("red", 0, None)?,
                    args.number("green", 1, None)?,
                    args.number("blue", 2, None)?,
                ]
                .map(|v| v.clamp(0, 255) as u8);
                self.static_colour(colour, &args.zone(3)?)?;
            }
            "set_none" => {
                let zone = args.zone(0)?;
                if ble::ZONES.contains(&zone.as_str()) {
                    self.static_colour([0; 3], &zone)?;
                } else {
                    self.transport.write(&ble::off_command(), false)?;
                    self.transport.release();
                    let serial = self.serial();
                    let mut store = persistence::lock(&self.store);
                    store.set(&serial, "backlight", "effect", json!("none"));
                    store.set_device(&serial, "zone_colours", json!([0, 0, 0, 0, 0, 0]));
                }
            }
            "set_zone_colours" => {
                let value = args.value("colours", 0, None)?;
                let colours = value
                    .as_array()
                    .ok_or_else(|| Error::Invalid("expected an array of colours".into()))?
                    .iter()
                    .map(|v| {
                        let a = v
                            .as_array()
                            .filter(|v| v.len() >= 3)
                            .ok_or_else(|| Error::Invalid("expected RGB colour".into()))?;
                        Ok([integer(&a[0])?, integer(&a[1])?, integer(&a[2])?]
                            .map(|v| v.clamp(0, 255) as u8))
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.set_colours(&colours, args.boolean("hold", 1, Some(true))?)?;
            }
            "set_key_row" => {
                let value = args.value("payload", 0, None)?;
                let data: Vec<u8> = value
                    .as_array()
                    .ok_or_else(|| Error::Invalid("expected byte array".into()))?
                    .iter()
                    .map(|v| {
                        u8::try_from(integer(v)?)
                            .map_err(|_| Error::Invalid("byte outside 0..255".into()))
                    })
                    .collect::<Result<_>>()?;
                if data.len() < 3 {
                    return Err(Error::Invalid("frame too short".into()));
                }
                let mut colours = self.current_colours();
                for (offset, column) in (usize::from(data[1])..=usize::from(data[2])).enumerate() {
                    if let (Some(out), Some(chunk)) = (
                        colours.get_mut(column),
                        data.get(3 + offset * 3..6 + offset * 3),
                    ) {
                        out.copy_from_slice(chunk);
                    }
                }
                self.set_colours(&colours, false)?;
            }
            "set_custom" => {}
            "get_brightness" => {
                return Ok(json!(ble::level_to_percentage(i64::from(
                    self.read(ble::read_opcode(ble::BRIGHTNESS))?
                ))))
            }
            "set_brightness" => {
                let level = ble::percentage_to_level(args.float("percent", 0, None)?)?;
                self.transport
                    .write(&ble::brightness_command(level.into()), false)?;
                let serial = self.serial();
                persistence::lock(&self.store).set(
                    &serial,
                    "backlight",
                    "brightness",
                    json!(ble::level_to_percentage(level.into())),
                );
            }
            "get_battery_level" => return Ok(json!(self.read(ble::BATTERY)?)),
            "is_charging" => return Ok(json!(self.read(ble::CHARGING)? != 0)),
            "restore" => self.restore()?,
            _ => {
                return Err(Error::NotSupported(format!(
                    "Bluetooth does not support {method}"
                )))
            }
        }
        Ok(Value::Null)
    }
}
