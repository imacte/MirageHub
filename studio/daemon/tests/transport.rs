use mirage_hub_daemon::{
    hid::{HidBackend, HidHandle, HidInfo, InputReader},
    protocol::report::Report,
    recipes::Sender,
    transport::{self, DeviceTransport, ReportChannel, Transport},
    Error, Result,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct State {
    features: Vec<Vec<u8>>,
    outputs: Vec<Vec<u8>>,
    replies: VecDeque<Result<Vec<u8>>>,
    input: VecDeque<Result<Option<Vec<u8>>>>,
    opens: Vec<&'static str>,
    drops: usize,
}
#[derive(Clone, Default)]
struct Backend(Arc<Mutex<State>>);
struct Handle(Backend);
struct Reader(Backend);
impl Drop for Handle {
    fn drop(&mut self) {
        self.0 .0.lock().unwrap().drops += 1;
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.0 .0.lock().unwrap().drops += 1;
    }
}
impl HidHandle for Handle {
    fn send_feature(&mut self, data: &[u8]) -> Result<()> {
        self.0 .0.lock().unwrap().features.push(data.to_vec());
        Ok(())
    }
    fn get_feature(&mut self, length: usize, report_id: u8) -> Result<Vec<u8>> {
        assert_eq!((length, report_id), (91, 0));
        self.0
             .0
            .lock()
            .unwrap()
            .replies
            .pop_front()
            .unwrap_or_else(|| Err(Error::Io("no scripted response".into())))
    }
    fn send_output(&mut self, data: &[u8]) -> Result<()> {
        self.0 .0.lock().unwrap().outputs.push(data.to_vec());
        Ok(())
    }
}
impl InputReader for Reader {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        if let Some(reply) = self.0 .0.lock().unwrap().input.pop_front() {
            return reply;
        }
        std::thread::sleep(timeout);
        Ok(None)
    }
}
impl HidBackend for Backend {
    fn enumerate(&self, _vendor: Option<u16>, _product: Option<u16>) -> Result<Vec<HidInfo>> {
        Ok(vec![])
    }
    fn open(&self, _info: &HidInfo) -> Result<Box<dyn HidHandle>> {
        self.0.lock().unwrap().opens.push("writer");
        Ok(Box::new(Handle(self.clone())))
    }
    fn open_input(&self, _info: &HidInfo) -> Result<Box<dyn InputReader>> {
        self.0.lock().unwrap().opens.push("reader");
        Ok(Box::new(Reader(self.clone())))
    }
}
fn response(request: &Report, status: u8) -> Vec<u8> {
    let mut r = request.clone();
    r.status = status;
    let mut bytes = vec![0];
    bytes.extend(r.pack());
    bytes
}
fn transport(backend: &Backend) -> Transport {
    Transport::new(Arc::new(backend.clone()), HidInfo::default(), 0, None, None)
}

#[test]
fn retries_stale_and_failed_replies_but_stops_on_unsupported() {
    let request = Report::new(3, 10, 4);
    let backend = Backend::default();
    let mut channel = transport(&backend);
    let unrelated = Report::new(3, 11, 4);
    backend.0.lock().unwrap().replies.extend([
        Err(Error::Io("busy bus".into())),
        Ok(response(&unrelated, 2)),
        Ok(response(&request, 3)),
        Ok(response(&request, 1)),
    ]);
    assert_eq!(channel.send(&request, true).unwrap().unwrap().status, 1);
    assert_eq!(backend.0.lock().unwrap().features.len(), 4);
    backend
        .0
        .lock()
        .unwrap()
        .replies
        .push_back(Ok(response(&request, 5)));
    assert!(matches!(
        channel.send(&request, true),
        Err(Error::NotSupported(_))
    ));
    assert_eq!(backend.0.lock().unwrap().features.len(), 5);
    assert_eq!(channel.wait(), Duration::from_millis(1));
}
#[test]
fn single_attempt_flash_exchange_never_repeats_a_write_after_a_lost_or_bad_reply() {
    let request = Report::new(6, 9, 10);
    for reply in [
        Err(Error::Io("lost after write".into())),
        Ok(vec![0; 4]),
        Ok(response(&Report::new(6, 8, 6), 2)),
        Ok(response(&request, 3)),
    ] {
        let backend = Backend::default();
        backend
            .0
            .lock()
            .unwrap()
            .replies
            .extend([reply, Ok(response(&request, 2))]);
        assert!(transport(&backend).exchange_once(&request).is_err());
        assert_eq!(backend.0.lock().unwrap().features.len(), 1);
        assert_eq!(backend.0.lock().unwrap().replies.len(), 1);
    }
    let backend = Backend::default();
    backend
        .0
        .lock()
        .unwrap()
        .replies
        .push_back(Ok(response(&request, 1)));
    assert_eq!(
        transport(&backend).exchange_once(&request).unwrap().status,
        1
    );
    assert_eq!(backend.0.lock().unwrap().features.len(), 1);
}
#[test]
fn retries_are_bounded_and_short_packets_are_rejected() {
    let backend = Backend::default();
    let mut channel = transport(&backend);
    backend
        .0
        .lock()
        .unwrap()
        .replies
        .extend((0..5).map(|_| Ok(vec![0; 4])));
    assert!(channel.send(&Report::default(), true).is_err());
    assert_eq!(backend.0.lock().unwrap().features.len(), 5);
    channel.close();
    assert_eq!(backend.0.lock().unwrap().drops, 1);
    assert!(channel.send(&Report::default(), false).unwrap().is_none());
    assert_eq!(backend.0.lock().unwrap().opens.len(), 2);
}
#[test]
fn argb_and_kraken_use_their_own_report_formats_and_handles() {
    let backend = Backend::default();
    let mut channel = Transport::new(
        Arc::new(backend.clone()),
        HidInfo::default(),
        0,
        Some(HidInfo::default()),
        Some(HidInfo::default()),
    );
    channel.send_argb(5, &[1, 2, 3, 4, 5, 6]).unwrap();
    let report = backend.0.lock().unwrap().features[0].clone();
    assert_eq!(report.len(), 321);
    assert_eq!(&report[..12], &[0x5b, 0x84, 5, 5, 0, 1, 1, 2, 3, 4, 5, 6]);
    assert!(channel.send_argb(1, &[0; 318]).is_err());
    channel
        .send_kraken(&[[4; 37], [5; 37]], Duration::ZERO)
        .unwrap();
    assert_eq!(
        backend.0.lock().unwrap().outputs,
        vec![vec![4; 37], vec![5; 37]]
    );
    channel.close();
    assert_eq!(backend.0.lock().unwrap().drops, 2);
}
#[test]
fn input_is_opened_before_output_and_unrelated_reports_are_skipped() {
    let backend = Backend::default();
    let info = HidInfo {
        output_length: 64,
        ..Default::default()
    };
    let mut channel = ReportChannel::new(Arc::new(backend.clone()), info);
    channel.timeout = Duration::from_millis(10);
    backend
        .0
        .lock()
        .unwrap()
        .input
        .extend([Ok(Some(vec![1])), Ok(Some(vec![2]))]);
    assert_eq!(
        channel.exchange(&[2, 3], &|v| v == [2]).unwrap(),
        Some(vec![2])
    );
    assert_eq!(backend.0.lock().unwrap().opens, vec!["reader", "writer"]);
    assert_eq!(backend.0.lock().unwrap().outputs[0].len(), 64);
    assert!(channel.exchange(&[2], &|_| true).unwrap().is_none());
    backend
        .0
        .lock()
        .unwrap()
        .input
        .push_back(Err(Error::Io("disconnected".into())));
    assert!(channel.exchange(&[2], &|_| true).is_err());
    assert_eq!(backend.0.lock().unwrap().drops, 2);
    backend.0.lock().unwrap().input.push_back(Ok(Some(vec![9])));
    assert_eq!(channel.exchange(&[2], &|_| true).unwrap(), Some(vec![9]));
    assert_eq!(
        backend.0.lock().unwrap().opens,
        vec!["reader", "writer", "reader", "writer"]
    );
}
#[test]
fn interface_selection_uses_descriptor_lengths_and_firmware_preferences() {
    let candidates = vec![
        HidInfo {
            interface: Some(1),
            collection: Some(1),
            feature_length: 0,
            ..Default::default()
        },
        HidInfo {
            interface: Some(2),
            collection: Some(2),
            feature_length: 91,
            ..Default::default()
        },
        HidInfo {
            interface: Some(1),
            collection: Some(3),
            feature_length: 91,
            ..Default::default()
        },
        HidInfo {
            interface: Some(1),
            collection: Some(1),
            feature_length: 91,
            ..Default::default()
        },
    ];
    assert_eq!(
        transport::select_control(&candidates, Some(2)),
        Some(&candidates[1])
    );
    assert_eq!(
        transport::select_control(&candidates, Some(8)),
        Some(&candidates[3])
    );
    let mut special = candidates.clone();
    special[0].output_length = 37;
    special[1].output_length = 37;
    special[1].interface = Some(3);
    assert_eq!(transport::select_kraken(&special), Some(&special[1]));
    for info in &mut special {
        info.input_length = 64;
        info.output_length = 64;
        info.usage_page = 0xff00;
    }
    special[2].usage_page = 0xff14;
    assert_eq!(transport::select_blackshark(&special), Some(&special[2]));
    assert_eq!(
        HidInfo::path_index(r"\\?\HID#VID_1532&PID_0203&MI_02&COL01", "&mi_"),
        Some(2)
    );
    assert_eq!(HidInfo::path_index("&mi_", "&mi_"), None);
}
