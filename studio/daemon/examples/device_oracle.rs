//! Development-only differential runner. It never opens hardware or user files.
use openrazer_daemon::{
    ble::BleChannel,
    database,
    device::{BlackSharkDevice, BleDevice, Device, KrakenDevice, StandardDevice},
    hid::HidInfo,
    persistence::Persistence,
    profiles,
    protocol::{blackshark, report::Report},
    recipes::{self, Sender},
    transport::{CommandChannel, DeviceTransport},
    Result,
};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

struct RecordingTransport {
    info: HidInfo,
    wait: Duration,
    sent: Arc<Mutex<Vec<Value>>>,
    argb: Option<HidInfo>,
}
impl Sender for RecordingTransport {
    fn send(&mut self, request: &Report, want_response: bool) -> Result<Option<Report>> {
        self.sent
            .lock()
            .unwrap()
            .push(json!({"packet":request.pack().as_slice(),"response":want_response}));
        if !want_response {
            return Ok(None);
        }
        let mut reply = request.clone();
        reply.status = 2;
        for (i, byte) in reply.arguments.iter_mut().enumerate() {
            *byte = (i * 17 + 23) as u8;
        }
        if request.command_class == 0 && request.command_id == 0x82 {
            reply.arguments.fill(0);
            reply.set_bytes(0, format!("ORACLE{:04X}", self.info.product_id).as_bytes());
        } else if request.command_class == 0 && request.command_id == 0x81 {
            reply.set_bytes(0, &[2, 7]);
        }
        Ok(Some(reply))
    }
    fn send_argb(&mut self, channel: u8, data: &[u8]) -> Result<()> {
        self.sent
            .lock()
            .unwrap()
            .push(json!({"channel":channel,"argb":data}));
        Ok(())
    }
}
impl DeviceTransport for RecordingTransport {
    fn info(&self) -> &HidInfo {
        &self.info
    }
    fn wait(&self) -> Duration {
        self.wait
    }
    fn argb_info(&self) -> Option<&HidInfo> {
        self.argb.as_ref()
    }
    fn kraken_info(&self) -> Option<&HidInfo> {
        (self.info.output_length == 37).then_some(&self.info)
    }
    fn send_kraken(&mut self, reports: &[[u8; 37]], _settle: Duration) -> Result<()> {
        for report in reports {
            self.sent
                .lock()
                .unwrap()
                .push(json!({"output":report.as_slice()}));
        }
        Ok(())
    }
    fn close(&mut self) {}
}

struct RecordingChannel {
    info: HidInfo,
    sent: Arc<Mutex<Vec<Value>>>,
}
impl CommandChannel for RecordingChannel {
    fn info(&self) -> &HidInfo {
        &self.info
    }
    fn close(&mut self) {}
    fn exchange(
        &mut self,
        request: &[u8],
        accept: &dyn Fn(&[u8]) -> bool,
    ) -> Result<Option<Vec<u8>>> {
        self.sent.lock().unwrap().push(json!({"output":request}));
        let payload = if request[10] & 128 != 0 {
            vec![0]
        } else {
            match request[10] {
                blackshark::SERIAL => format!("ORACLE{:04X}", self.info.product_id).into_bytes(),
                blackshark::FIRMWARE => vec![1, 7, 0, 0],
                blackshark::EQ_BANDS => (-9_i8..=0).map(|n| n as u8).collect(),
                blackshark::BATTERY => vec![87],
                blackshark::MIC_MUTED | blackshark::SIDETONE | blackshark::EQ_ENABLED => vec![1],
                blackshark::SIDETONE_LEVEL => vec![9],
                blackshark::EQ_PRESET => vec![7],
                blackshark::IDLE_MINUTES => vec![15],
                blackshark::DONGLE_LED => vec![2],
                _ => vec![0],
            }
        };
        let mut reply = blackshark::request(request[2], request[9], request[10], &payload)?;
        reply[1] = 2;
        reply[8] = 128;
        reply[11] = 1;
        if self.info.product_id == blackshark::PID_WIRED {
            reply[9] = 128;
        }
        reply[62] = reply[1..62].iter().fold(0, |sum, b| sum ^ b);
        assert!(
            accept(&reply),
            "native channel rejected a matching oracle reply"
        );
        Ok(Some(reply.to_vec()))
    }
}

struct RecordingBle {
    sent: Arc<Mutex<Vec<Value>>>,
}
impl BleChannel for RecordingBle {
    fn address(&self) -> u64 {
        0xaabbccddeeff
    }
    fn write(&mut self, payload: &[u8], hold: bool) -> Result<()> {
        self.sent
            .lock()
            .unwrap()
            .push(json!({"ble":payload,"hold":hold}));
        Ok(())
    }
    fn request(&mut self, opcode: u8, payload: &[u8]) -> Result<Vec<u8>> {
        self.sent
            .lock()
            .unwrap()
            .push(json!({"read":opcode,"payload":payload}));
        Ok(vec![match opcode {
            0x41 => 127,
            0x21 => 87,
            _ => 1,
        }])
    }
    fn release(&mut self) {
        self.sent.lock().unwrap().push(json!({"release":true}));
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn describe(&mut self) -> Result<Value> {
        Ok(Value::Null)
    }
    fn close(&mut self) {}
}

fn run(request: Value) -> Result<Value> {
    let pid = request["pid"].as_u64().unwrap() as u16;
    let metadata = database::lookup(0x1532, pid).unwrap().clone();
    let params = recipes::recipes().transport_params(metadata.driver(), pid)?;
    let mut info = HidInfo {
        path: format!("oracle:{pid:04x}"),
        product_id: pid,
        vendor_id: 0x1532,
        interface: Some(params.index),
        feature_length: 91,
        ..Default::default()
    };
    let kraken = recipes::recipes().is_kraken(pid);
    let shark = [blackshark::PID_WIRED, blackshark::PID_WIRELESS].contains(&pid);
    if kraken {
        info.output_length = 37;
        info.interface = Some(3);
    }
    if shark {
        info.output_length = 64;
        info.input_length = 64;
        info.interface = Some(3);
    }
    let argb = request["argb"]
        .as_bool()
        .unwrap_or(false)
        .then(|| info.clone());
    let sent = Arc::new(Mutex::new(Vec::new()));
    let store = Persistence::memory().shared();
    let mut device: Box<dyn Device> = if metadata.transport == "ble" {
        let mut device = BleDevice::new(
            metadata,
            Box::new(RecordingBle { sent: sent.clone() }),
            store.clone(),
        );
        device.classic_address = Some(0xaabbccddeefe);
        Box::new(device)
    } else if shark {
        Box::new(BlackSharkDevice::new(
            metadata,
            Box::new(RecordingChannel {
                info,
                sent: sent.clone(),
            }),
            store.clone(),
        ))
    } else {
        let transport = Box::new(RecordingTransport {
            info,
            wait: Duration::from_micros(params.wait_us.max(1000)),
            sent: sent.clone(),
            argb,
        });
        if kraken {
            Box::new(KrakenDevice::new(metadata, transport, store.clone()))
        } else {
            Box::new(StandardDevice::new(metadata, transport, store.clone()))
        }
    };
    let serial = device.serial();
    let firmware = device.firmware().map(Value::String).unwrap_or(Value::Null);
    let capabilities = device.capabilities();
    let mut steps = Vec::new();
    for command in request["commands"].as_array().unwrap() {
        sent.lock().unwrap().clear();
        let result = match command["method"].as_str().unwrap() {
            "profile_snapshot" => Ok(serde_json::to_value(profiles::snapshot(device.as_mut()))?),
            "profile_apply" => profiles::apply(
                device.as_mut(),
                &serde_json::from_value(command["args"][0].clone())?,
            )
            .map(|_| Value::Null),
            "enter_driver_mode" => device
                .enter_driver_mode(command["args"][0].as_bool().unwrap())
                .map(|_| Value::Null),
            "leave_driver_mode" => device.leave_driver_mode().map(|_| Value::Null),
            method => device.call(
                method,
                command["args"].as_array().unwrap(),
                command["kwargs"].as_object().unwrap(),
            ),
        };
        steps.push(json!({"ok":result.is_ok(),"result":result.unwrap_or(Value::Null),"sent":*sent.lock().unwrap(),"state":store.lock().unwrap().as_json()}));
    }
    Ok(json!({"serial":serial,"firmware":firmware,"capabilities":capabilities,"steps":steps}))
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let result = run(serde_json::from_str(&line?)?)?;
        serde_json::to_writer(&mut output, &result)?;
        writeln!(&mut output)?;
        output.flush()?;
    }
    Ok(())
}
