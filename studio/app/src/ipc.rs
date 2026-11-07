//! Private parent/child pipes: the tray owns all long-lived work, while the
//! disposable UI receives snapshots and sends commands. No listening port or
//! discovery token is needed: only the spawned child inherits these handles.

use crate::{settings::Settings, updates::{self, Updater}, worker::{self, Shared, Snapshot}};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{io::{self, BufRead, BufReader, Read, Write}, sync::{Arc, Mutex}};

const MAX_MESSAGE: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
pub enum Request {
    Command(worker::Command),
    Settings(Settings),
    Update(updates::Action),
    Closing { quit_host: bool },
    CancelQuit,
}

#[derive(Serialize, Deserialize)]
pub struct State {
    pub snapshot: Snapshot,
    pub updates: updates::Status,
    pub just_updated: Option<String>,
    pub shutdown: bool,
}

pub fn read<T: DeserializeOwned>(reader: &mut impl BufRead) -> io::Result<T> {
    let mut line = Vec::new();
    let count = reader.take(MAX_MESSAGE + 1).read_until(b'\n', &mut line)?;
    if count == 0 { return Err(io::ErrorKind::UnexpectedEof.into()); }
    if count as u64 > MAX_MESSAGE || line.last() != Some(&b'\n') {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid UI message length"));
    }
    serde_json::from_slice(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 >= MAX_MESSAGE { return Err(io::Error::new(io::ErrorKind::InvalidData, "UI message too large")); }
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

#[derive(Clone)]
pub struct Sender(Arc<Mutex<io::Stdout>>);

impl Sender {
    pub fn stdout() -> Self { Self(Arc::new(Mutex::new(io::stdout()))) }
    pub fn send(&self, request: &Request) -> io::Result<()> { write(&mut *self.0.lock().unwrap(), request) }
}

pub fn attach(ctx: egui::Context, shared: Shared, updates: Arc<Updater>) {
    std::thread::Builder::new().name("tray-state".into()).spawn(move || {
        let mut reader = BufReader::new(io::stdin());
        while let Ok(mut state) = read::<State>(&mut reader) {
            {
                let mut current = shared.lock().unwrap();
                // Local notices (e.g. an exported file) are not in the host's
                // snapshot. Keep them until a newer host notice replaces them.
                if current.toast.as_ref().is_some_and(|local| state.snapshot.toast.as_ref().is_none_or(|incoming| local.at > incoming.at)) {
                    state.snapshot.toast = current.toast.take();
                }
                *current = state.snapshot;
            }
            updates.receive(state.updates, state.just_updated);
            if state.shutdown { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
            if crate::shell::is_visible() { ctx.request_repaint(); }
        }
        // A stopped/crashed host must not leave an orphaned UI pretending it
        // can control devices. Normal close saves the draft session locally.
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }).expect("the tray reader starts");
}

pub mod elapsed {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{Duration, Instant};
    pub fn serialize<S: Serializer>(at: &Instant, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(at.elapsed().as_millis().min(u64::MAX as u128) as u64)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Instant, D::Error> {
        let elapsed = Duration::from_millis(u64::deserialize(deserializer)?.min(86_400_000));
        Ok(Instant::now().checked_sub(elapsed).unwrap_or_else(Instant::now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pipe_messages_preserve_commands_and_bound_input() {
        let mut bytes = Vec::new();
        write(&mut bytes, &Request::Command(worker::Command::Profile { action: "save".into(), serial: "鼠标".into(), name: "游戏".into(), quiet: false })).unwrap();
        let Request::Command(worker::Command::Profile { action, name, .. }) = read(&mut &bytes[..]).unwrap() else { panic!() };
        assert_eq!((action.as_str(), name.as_str()), ("save", "游戏"));
        assert!(read::<Request>(&mut &b"{}"[..]).is_err());
        assert!(read::<Request>(&mut &b"{\"Command\":{\"Profile\":{\"action\":\"invalid\"}}}\n"[..]).is_err());
    }

    #[test]
    fn toast_age_survives_snapshot_transfer_instead_of_restarting() {
        let snapshot = Snapshot { toast: Some(worker::Toast { text: "已保存".into(), error: false, at: std::time::Instant::now() - std::time::Duration::from_secs(10) }), ..Default::default() };
        let copy: Snapshot = serde_json::from_value(serde_json::to_value(snapshot).unwrap()).unwrap();
        assert!(copy.toast.unwrap().at.elapsed().as_secs() >= 10);
    }
}
