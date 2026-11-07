use openrazer_daemon::{
    integrations::{AccentWatch, IntegrationSettings},
    Error,
};
use serde_json::json;
use std::{
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

#[test]
fn accent_changes_apply_once_and_one_failure_does_not_stop_future_updates() {
    let colour = Arc::new(Mutex::new(None));
    let source = colour.clone();
    let (send, receive) = mpsc::channel();
    let mut watch = AccentWatch::with_reader(
        Arc::new(move |value| {
            send.send(value).unwrap();
            Err(Error::Device("one device gone".into()))
        }),
        Arc::new(move || *source.lock().unwrap()),
        Duration::from_millis(5),
    );
    assert_eq!(watch.check(false), None);
    *colour.lock().unwrap() = Some([1, 2, 3]);
    watch.check(false);
    assert_eq!(receive.recv().unwrap(), [1, 2, 3]);
    watch.check(false);
    assert!(receive.try_recv().is_err());
    watch.start().unwrap();
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(3)).unwrap(),
        [1, 2, 3]
    );
    *colour.lock().unwrap() = Some([4, 5, 6]);
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(3)).unwrap(),
        [4, 5, 6]
    );
    watch.stop();
    assert!(!watch.running());
    assert_eq!(watch.colour(), None);
}
#[test]
fn integrations_default_off_and_unknown_settings_are_refused() {
    let mut settings = IntegrationSettings::memory();
    assert_eq!(settings.as_json(), json!({"accent":false,"chroma":false}));
    settings.set("accent", true).unwrap();
    assert!(settings.enabled("accent"));
    assert!(!settings.enabled("chroma"));
    assert!(settings.set("missing", true).is_err());
    settings.set("chroma", true).unwrap();
    assert!(settings.enabled("chroma"));
}
