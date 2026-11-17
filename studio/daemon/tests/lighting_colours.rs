use mirage_hub_daemon::{
    database,
    device::{Device, StandardDevice},
    hid::HidInfo,
    persistence::Persistence,
    protocol::report::Report,
    recipes::Sender,
    transport::DeviceTransport,
    Result,
};
use serde_json::{json, Map};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

struct Recorder {
    info: HidInfo,
    reports: Arc<Mutex<Vec<Report>>>,
}

impl Sender for Recorder {
    fn send(&mut self, request: &Report, response: bool) -> Result<Option<Report>> {
        self.reports.lock().unwrap().push(request.clone());
        Ok(response.then(|| {
            let mut reply = request.clone();
            reply.status = 2;
            reply.arguments.fill(0);
            reply
        }))
    }
}
impl DeviceTransport for Recorder {
    fn info(&self) -> &HidInfo {
        &self.info
    }
    fn wait(&self) -> Duration {
        Duration::ZERO
    }
    fn close(&mut self) {}
}

#[test]
fn v4_mini_static_colours_reach_the_device_report_unchanged() {
    for (pid, transaction) in [(0x02b9, 0x1f), (0x02ba, 0x9f)] {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let transport = Recorder {
            info: HidInfo::default(),
            reports: reports.clone(),
        };
        let mut device = StandardDevice::new(
            database::lookup(0x1532, pid).unwrap().clone(),
            Box::new(transport),
            Persistence::memory().shared(),
        );
        for colour in [
            [255, 0, 0],
            [255, 128, 0],
            [255, 255, 0],
            [255, 0, 255],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 255],
        ] {
            device
                .call(
                    "set_static",
                    &colour.map(|channel| json!(channel)),
                    &Map::new(),
                )
                .unwrap();
            let reports = reports.lock().unwrap();
            let report = reports
                .iter()
                .rev()
                .find(|report| report.command_class == 15 && report.command_id == 2)
                .unwrap();
            assert_eq!(report.transaction_id, transaction);
            assert_eq!(
                report.payload(),
                &[1, 5, 1, 0, 0, 1, colour[0], colour[1], colour[2]]
            );
        }
    }
}
