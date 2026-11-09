use mirage_hub_daemon::rpc::{self, Endpoint, RpcError, RpcServer, Service, SingleInstance};
use serde_json::{json, Map, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Echo {
    calls: AtomicUsize,
}
impl Service for Echo {
    fn dispatch(&self, method: &str, params: &Map<String, Value>) -> rpc::Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match method {
            "echo" => Ok(json!(params)),
            "daemon.version" => Ok(json!({"backend":"rust-test","state":"ready"})),
            "device.call" => Ok(
                json!({"method":params["method"],"serial":params["serial"],"args":params["args"],"kwargs":params["kwargs"]}),
            ),
            _ => Err(RpcError::new(
                rpc::METHOD_NOT_FOUND,
                format!("unknown method: {method}"),
            )),
        }
    }
}
struct Wire {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    token: String,
}
impl Wire {
    fn new(server: &RpcServer) -> Self {
        let stream =
            TcpStream::connect((server.endpoint().host.as_str(), server.endpoint().port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
            token: server.endpoint().token.clone(),
        }
    }
    fn send(&mut self, value: Value) {
        let mut line = serde_json::to_vec(&value).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).unwrap();
    }
    fn read(&mut self) -> Value {
        let mut line = String::new();
        assert!(self.reader.read_line(&mut line).unwrap() > 0);
        serde_json::from_str(&line).unwrap()
    }
    fn call(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(json!({"id":id,"method":method,"params":params,"token":self.token}));
        self.read()
    }
    fn closed(&mut self) {
        let mut byte = [0];
        match self.reader.read(&mut byte) {
            Ok(0) => (),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) => {}
            other => panic!("connection was not closed: {other:?}"),
        }
    }
}

#[test]
fn studio_client_connects_and_uses_the_existing_device_and_shutdown_methods() {
    let mut server = RpcServer::start(Arc::new(Echo::default()), 0).unwrap();
    let endpoint =
        serde_json::from_value(serde_json::to_value(server.endpoint()).unwrap()).unwrap();
    let mut client = mirage_hub_core::rpc::Client::connect_to(endpoint).unwrap();
    assert_eq!(
        client.call("daemon.version", json!({})).unwrap()["backend"],
        "rust-test"
    );
    assert_eq!(
        client
            .device("测试鼠标", "set_dpi", vec![json!(800), json!(1600)], None)
            .unwrap(),
        json!({"serial":"测试鼠标","method":"set_dpi","args":[800,1600],"kwargs":{}})
    );
    assert_eq!(
        client.call("daemon.shutdown", json!({})).unwrap(),
        json!({"stopping":true})
    );
    assert!(server.wait_for_shutdown(Duration::from_secs(1)));
    server.stop();
    assert!(!server.listening());
}

#[test]
fn every_message_requires_a_token_and_first_failure_closes_the_connection() {
    let service = Arc::new(Echo::default());
    let server = RpcServer::start(service.clone(), 0).unwrap();
    for token in [
        Value::Null,
        json!(1),
        json!("bad"),
        json!(format!("{}x", server.endpoint().token)),
    ] {
        let mut wire = Wire::new(&server);
        wire.send(json!({"id":7,"method":"echo","token":token}));
        assert_eq!(wire.read()["error"]["code"], rpc::UNAUTHORIZED);
        wire.closed();
    }
    assert_eq!(service.calls.load(Ordering::SeqCst), 0);
    let mut wire = Wire::new(&server);
    assert_eq!(
        wire.call(json!("中文 ID"), "echo", json!({"colour":[3,2,1]}))["id"],
        "中文 ID"
    );
    wire.send(json!({"id":8,"method":"echo","token":"bad"}));
    assert_eq!(wire.read()["error"]["code"], rpc::UNAUTHORIZED);
    assert_eq!(
        wire.call(json!(9), "echo", json!({"ok":true}))["result"]["ok"],
        true
    );
    assert_eq!(service.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn notifications_pipelining_and_old_empty_parameter_coercions_are_preserved() {
    let service = Arc::new(Echo::default());
    let server = RpcServer::start(service.clone(), 0).unwrap();
    let mut wire = Wire::new(&server);
    wire.send(json!({"method":"echo","token":wire.token}));
    wire.send(json!({"id":null,"method":"echo","token":wire.token}));
    for value in [
        json!({}),
        Value::Null,
        json!(false),
        json!(0),
        json!(""),
        json!([]),
    ] {
        assert_eq!(
            wire.call(json!(3), "echo", value),
            json!({"jsonrpc":"2.0","id":3,"result":{}})
        );
    }
    for value in [json!(true), json!(1), json!("x"), json!([1])] {
        assert_eq!(
            wire.call(json!(4), "echo", value)["error"]["code"],
            rpc::INVALID_PARAMS
        );
    }
    // The original protocol sends errors even for notifications.
    wire.send(json!({"method":"missing","token":wire.token}));
    let error = wire.read();
    assert_eq!(error["id"], Value::Null);
    assert_eq!(error["error"]["code"], rpc::METHOD_NOT_FOUND);
    wire.send(json!({"id":5,"token":wire.token}));
    assert_eq!(wire.read()["error"]["code"], rpc::INVALID_REQUEST);
    assert_eq!(
        wire.call(json!(6), "echo", json!({"alive":true}))["result"]["alive"],
        true
    );
    assert_eq!(service.calls.load(Ordering::SeqCst), 10);
}

#[test]
fn malformed_oversized_and_non_object_messages_close_without_dispatch() {
    let service = Arc::new(Echo::default());
    let server = RpcServer::start(service.clone(), 0).unwrap();
    for (bytes, code) in [
        (b"{bad}\n".to_vec(), rpc::PARSE_ERROR),
        (b"{\"text\":\"\xff\"}\n".to_vec(), rpc::PARSE_ERROR),
        (b"[]\n".to_vec(), rpc::INVALID_REQUEST),
        (b"null\n".to_vec(), rpc::INVALID_REQUEST),
        (vec![b'x'; rpc::MAX_MESSAGE_SIZE + 1], rpc::INVALID_REQUEST),
    ] {
        let mut wire = Wire::new(&server);
        wire.writer.write_all(&bytes).unwrap();
        assert_eq!(wire.read()["error"]["code"], code);
        wire.closed();
    }
    assert_eq!(service.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn fragmented_requests_and_idle_connections_survive_and_stop_promptly() {
    let mut server = RpcServer::start(Arc::new(Echo::default()), 0).unwrap();
    let mut wire = Wire::new(&server);
    let message = format!(
        "{{\"id\":1,\"method\":\"echo\",\"token\":\"{}\",\"params\":{{\"text\":\"中文\"}}}}\n",
        wire.token
    );
    let bytes = message.as_bytes();
    wire.writer.write_all(&bytes[..20]).unwrap();
    thread::sleep(Duration::from_millis(150));
    wire.writer.write_all(&bytes[20..bytes.len() - 5]).unwrap();
    thread::sleep(Duration::from_millis(150));
    wire.writer.write_all(&bytes[bytes.len() - 5..]).unwrap();
    assert_eq!(wire.read()["result"]["text"], "中文");
    thread::sleep(Duration::from_millis(250));
    assert_eq!(wire.call(json!(2), "echo", json!({}))["result"], json!({}));
    let mut partial = Wire::new(&server);
    partial.writer.write_all(b"{\"id\":").unwrap();
    let _idle = Wire::new(&server);
    let start = Instant::now();
    server.stop();
    assert!(start.elapsed() < Duration::from_secs(1));
    wire.closed();
}

#[test]
fn a_slow_service_call_does_not_block_other_clients() {
    struct Slow {
        gate: (Mutex<bool>, Condvar),
        entered: AtomicUsize,
    }
    impl Service for Slow {
        fn dispatch(&self, method: &str, _: &Map<String, Value>) -> rpc::Result<Value> {
            if method == "slow" {
                self.entered.store(1, Ordering::SeqCst);
                let _guard = self
                    .gate
                    .1
                    .wait_while(self.gate.0.lock().unwrap(), |open| !*open)
                    .unwrap();
            }
            Ok(json!({"ok":true}))
        }
    }
    let service = Arc::new(Slow {
        gate: (Mutex::new(false), Condvar::new()),
        entered: AtomicUsize::new(0),
    });
    let server = RpcServer::start(service.clone(), 0).unwrap();
    let mut slow = Wire::new(&server);
    slow.send(json!({"id":1,"method":"slow","token":slow.token}));
    let deadline = Instant::now() + Duration::from_secs(2);
    while service.entered.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let mut other = Wire::new(&server);
    let response = other.call(json!(2), "daemon.version", json!({}));
    *service.gate.0.lock().unwrap() = true;
    service.gate.1.notify_all();
    assert_eq!(response["result"]["ok"], true);
    assert_eq!(slow.read()["result"]["ok"], true);
}

#[test]
fn rpc_port_cannot_be_claimed_by_a_second_listener() {
    let server = RpcServer::start(Arc::new(Echo::default()), 0).unwrap();
    assert!(RpcServer::start(Arc::new(Echo::default()), server.endpoint().port).is_err());
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "openrazer-rpc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn publishing_replaces_old_files_and_only_removes_its_own_endpoint() {
    let dir = Directory::new();
    let path = dir.0.join("daemon.json");
    fs::write(&path, b"old file").unwrap();
    let server = RpcServer::start(Arc::new(Echo::default()), 0).unwrap();
    let first = server.endpoint().publish(&path).unwrap();
    assert_eq!(Endpoint::read(&path).unwrap(), *server.endpoint());
    let mut replacement = server.endpoint().clone();
    replacement.token = rpc::new_token().unwrap();
    let second = replacement.publish(&path).unwrap();
    drop(first);
    assert_eq!(Endpoint::read(&path).unwrap(), replacement);
    drop(second);
    assert!(!path.exists());
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
fn native_mutex_excludes_a_second_daemon_until_the_first_handle_is_closed() {
    let name = format!(
        "Local\\openrazer-rust-test-{}-{}",
        std::process::id(),
        rpc::new_token().unwrap()
    );
    let first = SingleInstance::acquire(&name).unwrap().unwrap();
    assert!(SingleInstance::acquire(&name).unwrap().is_none());
    drop(first);
    assert!(SingleInstance::acquire(&name).unwrap().is_some());
}

#[cfg(windows)]
#[test]
fn endpoint_has_a_protected_dacl_for_the_current_user_and_system() {
    use std::{
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
            CreateWellKnownSid, EqualSid, GetAce, GetSecurityDescriptorControl,
            GetTokenInformation, TokenUser, WinLocalSystemSid, ACCESS_ALLOWED_ACE,
            DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
        },
        Storage::FileSystem::FILE_ALL_ACCESS,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let dir = Directory::new();
    let path = dir.0.join("daemon.json");
    let server = RpcServer::start(Arc::new(Echo::default()), 0).unwrap();
    let _published = server.endpoint().publish(&path).unwrap();
    let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut token = ptr::null_mut();
        assert_ne!(
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token),
            0
        );
        let token = OwnedHandle::from_raw_handle(token);
        let mut size = 0;
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &mut size,
        );
        let mut user_data = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        assert_ne!(
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                user_data.as_mut_ptr().cast(),
                size,
                &mut size
            ),
            0
        );
        let user = &*user_data.as_ptr().cast::<TOKEN_USER>();
        let mut system = [0u32; 17];
        let mut system_size = size_of_val(&system) as u32;
        assert_ne!(
            CreateWellKnownSid(
                WinLocalSystemSid,
                ptr::null_mut(),
                system.as_mut_ptr().cast(),
                &mut system_size
            ),
            0
        );
        let mut descriptor = ptr::null_mut();
        let mut acl = ptr::null_mut();
        assert_eq!(
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor
            ),
            0
        );
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            GetSecurityDescriptorControl(descriptor, &mut control, &mut revision),
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);
        assert!(!acl.is_null());
        assert_eq!((*acl).AceCount, 2);
        let (mut found_user, mut found_system) = (false, false);
        for index in 0..2 {
            let mut entry = ptr::null_mut();
            assert_ne!(GetAce(acl, index, &mut entry), 0);
            let ace = &*entry.cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!(ace.Header.AceType, 0); // ACCESS_ALLOWED_ACE_TYPE
            assert_eq!(ace.Header.AceFlags, 0);
            assert_eq!(ace.Mask, FILE_ALL_ACCESS);
            let sid = (&ace.SidStart as *const u32).cast_mut().cast();
            let is_user = EqualSid(sid, user.User.Sid) != 0;
            let is_system = EqualSid(sid, system.as_mut_ptr().cast()) != 0;
            assert!(
                is_user || is_system,
                "endpoint grants access to an unexpected principal"
            );
            found_user |= is_user;
            found_system |= is_system;
        }
        // An elevated process may create a file owned by Administrators (BA),
        // while its TokenUser is the local administrator (LA). Compare the
        // actual SID values, not owner text or Windows' optional SDDL aliases.
        assert!(found_user && found_system);
        LocalFree(descriptor);
    }
}
