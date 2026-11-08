use super::{integer, Arguments, Device};
use crate::{
    database::DeviceInfo, persistence::SharedStore, protocol::blackshark as protocol,
    transport::CommandChannel, Error, Result,
};
use serde_json::{json, Map, Value};
use std::time::{Duration, Instant};

pub struct BlackSharkDevice {
    pub metadata: DeviceInfo,
    pub channel: Box<dyn CommandChannel>,
    pub store: SharedStore,
    wired: bool,
    sequence: u8,
    serial: Option<String>,
    firmware: Option<String>,
    quiet_until: Option<Instant>,
}
impl BlackSharkDevice {
    pub fn new(metadata: DeviceInfo, channel: Box<dyn CommandChannel>, store: SharedStore) -> Self {
        let wired = metadata.pid == protocol::PID_WIRED;
        Self {
            metadata,
            channel,
            store,
            wired,
            sequence: 0,
            serial: None,
            firmware: None,
            quiet_until: None,
        }
    }
    fn exchange(&mut self, domain: u8, command: u8, payload: &[u8]) -> Result<Vec<u8>> {
        self.sequence = (self.sequence + 1) & 31;
        let sequence = self.sequence;
        let wired = self.wired;
        let request = protocol::request(sequence, domain, command, payload)?;
        let reply = self.channel.exchange(&request, &|answer| {
            protocol::answers(answer, sequence, domain, command, wired)
        })?;
        let Some(reply) = reply else {
            if domain == protocol::DOMAIN_HEADSET {
                self.quiet_until = Some(Instant::now() + Duration::from_secs(2));
                return Err(Error::Device("headset is switched off".into()));
            }
            return Err(Error::Device("headset did not answer".into()));
        };
        Ok(protocol::result(&reply, command)?.to_vec())
    }
    fn headset(&self) -> Result<u8> {
        if self.wired {
            return Ok(protocol::DOMAIN_LOCAL);
        }
        if self
            .quiet_until
            .is_some_and(|deadline| Instant::now() < deadline)
        {
            return Err(Error::Device("headset is switched off".into()));
        }
        Ok(protocol::DOMAIN_HEADSET)
    }
    fn read(&mut self, command: u8, domain: Option<u8>) -> Result<u8> {
        let domain = match domain {
            Some(d) => d,
            None => self.headset()?,
        };
        self.exchange(domain, command, &[])?
            .first()
            .copied()
            .ok_or_else(|| Error::Device(format!("empty reply to 0x{command:02x}")))
    }
    fn write(&mut self, command: u8, value: u8) -> Result<()> {
        self.exchange(self.headset()?, command | protocol::WRITE, &[value])?;
        Ok(())
    }
    fn headset_connected(&mut self) -> Result<bool> {
        if self.wired {
            return Ok(true);
        }
        match self.read(protocol::BATTERY, None) {
            Ok(_) => Ok(true),
            Err(Error::NotSupported(message)) => Err(Error::NotSupported(message)),
            Err(_) => Ok(self
                .quiet_until
                .is_none_or(|deadline| Instant::now() >= deadline)),
        }
    }
    fn name_of(names: &[(&str, u8)], value: u8) -> Value {
        names
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(name, _)| json!(name))
            .unwrap_or(Value::Null)
    }
    fn audio(&mut self) -> Result<Value> {
        let mut audio = Map::new();
        audio.insert("connected".into(), json!(false));
        if !self.wired {
            audio.insert(
                "dongle_led".into(),
                Self::name_of(
                    &protocol::DONGLE_LED_MODES,
                    self.read(protocol::DONGLE_LED, Some(protocol::DOMAIN_LOCAL))?,
                ),
            );
        }
        if !self.headset_connected()? {
            return Ok(Value::Object(audio));
        }
        audio.insert("connected".into(), json!(true));
        // Preserve the original read order; firmware serializes these registers.
        audio.insert(
            "microphone_muted".into(),
            json!(self.read(protocol::MIC_MUTED, None)? != 0),
        );
        audio.insert(
            "sidetone".into(),
            json!(self.read(protocol::SIDETONE, None)? != 0),
        );
        audio.insert(
            "sidetone_level".into(),
            json!(self.read(protocol::SIDETONE_LEVEL, None)?),
        );
        audio.insert(
            "equalizer".into(),
            json!(self.read(protocol::EQ_ENABLED, None)? != 0),
        );
        audio.insert(
            "equalizer_preset".into(),
            Self::name_of(&protocol::EQ_PRESETS, self.read(protocol::EQ_PRESET, None)?),
        );
        audio.insert(
            "equalizer_bands".into(),
            json!(protocol::decode_bands(&self.exchange(
                self.headset()?,
                protocol::EQ_BANDS,
                &[]
            )?)?),
        );
        audio.insert(
            "do_not_disturb".into(),
            json!(self.read(protocol::DO_NOT_DISTURB, None)? != 0),
        );
        Ok(Value::Object(audio))
    }
}
impl Device for BlackSharkDevice {
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
            .exchange(protocol::DOMAIN_LOCAL, protocol::SERIAL, &[])
            .ok()
            .map(|data| {
                data.iter()
                    .take_while(|b| **b != 0)
                    .filter(|b| b.is_ascii())
                    .map(|b| char::from(*b))
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("RZ{:04X}{:04X}", self.metadata.vid, self.metadata.pid));
        self.serial = Some(serial.clone());
        serial
    }
    fn firmware(&mut self) -> Result<String> {
        if let Some(firmware) = &self.firmware {
            return Ok(firmware.clone());
        }
        let reply = self.exchange(protocol::DOMAIN_LOCAL, protocol::FIRMWARE, &[])?;
        if reply.len() < 4 {
            return Err(Error::Device("short firmware reply".into()));
        }
        let firmware = format!("v{}.{}.{}.{}", reply[0], reply[1], reply[2], reply[3]);
        self.firmware = Some(firmware.clone());
        Ok(firmware)
    }
    fn capabilities(&self) -> Value {
        let i = &self.metadata;
        json!({"name":i.name,"type":i.device_type,"vid":i.vid,"pid":i.pid,"image":i.image,"protocol":"blackshark","zones":{},
            "matrix":false,"matrix_dimensions":null,"dedicated_macro_keys":false,"dpi":false,"dpi_stages":false,"max_dpi":null,
            "poll_rate":false,"supported_poll_rates":[],"battery":true,"idle_time":true,"idle_time_max":15300,
            "game_mode":false,"macro_mode":false,"keyboard_layout":false,"keyswitch_optimization":false,"scroll_mode":false,"custom_frame":false,"software_effects":false,"readback":true,
            "audio":{"sidetone_max":15,"equalizer_presets":protocol::EQ_PRESETS.iter().map(|(name,_)|*name).collect::<Vec<_>>(),
                "equalizer_frequencies":protocol::EQ_FREQUENCIES,"equalizer_range":[-9,6],
                "dongle_led_modes":if self.wired{vec![]}else{protocol::DONGLE_LED_MODES.iter().map(|(name,_)|*name).collect::<Vec<_>>()}},
            "transport":{"path":self.channel.info().path,"interface":self.channel.info().interface,"wait_us":0}})
    }
    fn enter_driver_mode(&mut self, _keys_handled: bool) -> Result<()> {
        Ok(())
    }
    fn leave_driver_mode(&mut self) -> Result<()> {
        Ok(())
    }
    fn restore(&mut self) -> Result<()> {
        Ok(())
    }
    fn close(&mut self) {
        self.channel.close();
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
            "available_effects" => return Ok(json!([])),
            "restore" => {}
            "get_battery_level" => return Ok(json!(self.read(protocol::BATTERY, None)?.min(100))),
            "is_charging" => return Ok(json!(self.read(protocol::CHARGING, None)? != 0)),
            "get_idle_time" => {
                return Ok(json!(
                    u32::from(self.read(protocol::IDLE_MINUTES, None)?) * 60
                ))
            }
            "set_idle_time" => {
                let minutes = (args.number("seconds", 0, None)? as f64 / 60.0).round_ties_even();
                if !(0.0..=255.0).contains(&minutes) {
                    return Err(Error::Invalid("sleep timer exceeds 255 minutes".into()));
                }
                self.write(protocol::IDLE_MINUTES, minutes as u8)?;
            }
            "get_audio" => return self.audio(),
            "get_microphone_muted" => return Ok(json!(self.read(protocol::MIC_MUTED, None)? != 0)),
            "set_sidetone" | "set_equalizer" | "set_do_not_disturb" => {
                let command = match method {
                    "set_sidetone" => protocol::SIDETONE,
                    "set_equalizer" => protocol::EQ_ENABLED,
                    _ => protocol::DO_NOT_DISTURB,
                };
                self.write(command, u8::from(args.boolean("enabled", 0, None)?))?;
            }
            "set_sidetone_level" => {
                let level = args.number("level", 0, None)?;
                if !(0..=15).contains(&level) {
                    return Err(Error::Invalid("sidetone must be 0..15".into()));
                }
                self.write(protocol::SIDETONE_LEVEL, level as u8)?;
            }
            "set_equalizer_preset" => {
                let preset = args.value("preset", 0, None)?;
                let code = protocol::EQ_PRESETS
                    .iter()
                    .find(|(name, _)| preset.as_str() == Some(*name))
                    .map(|(_, code)| *code)
                    .ok_or_else(|| Error::Invalid("unknown equalizer preset".into()))?;
                self.write(protocol::EQ_PRESET, code)?;
                self.write(protocol::EQ_ENABLED, 1)?;
            }
            "set_equalizer_bands" => {
                let gains = args.value("gains", 0, None)?;
                let gains = gains
                    .as_array()
                    .ok_or_else(|| Error::Invalid("equalizer gains must be an array".into()))?
                    .iter()
                    .map(integer)
                    .collect::<Result<Vec<_>>>()?;
                let bands = protocol::encode_bands(&gains)?;
                self.write(protocol::EQ_ENABLED, 1)?;
                self.write(protocol::EQ_PRESET, 255)?;
                self.exchange(
                    self.headset()?,
                    protocol::EQ_BANDS | protocol::WRITE,
                    &bands,
                )?;
                std::thread::sleep(Duration::from_millis(40));
                self.write(protocol::EQ_PRESET, 255)?;
            }
            "set_dongle_led" => {
                if self.wired {
                    return Err(Error::NotSupported("wired headset has no receiver".into()));
                }
                let mode = args.value("mode", 0, None)?;
                let code = protocol::DONGLE_LED_MODES
                    .iter()
                    .find(|(name, _)| mode.as_str() == Some(*name))
                    .map(|(_, code)| *code)
                    .ok_or_else(|| Error::Invalid("unknown dongle LED mode".into()))?;
                self.exchange(
                    protocol::DOMAIN_LOCAL,
                    protocol::DONGLE_LED | protocol::WRITE,
                    &[code],
                )?;
            }
            _ => {
                return Err(Error::NotSupported(format!(
                    "BlackShark does not support {method}"
                )))
            }
        }
        Ok(Value::Null)
    }
}
