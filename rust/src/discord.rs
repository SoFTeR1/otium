//! Discord Rich Presence over Discord's local named pipe — no SDK needed.
//! Frames are: u32 opcode, u32 length (little endian), JSON. Opcode 0 = handshake, 1 = command.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::Duration;

pub const APP_ID: &str = "1567308312947728414";
pub const APP_ICON: &str = "https://raw.githubusercontent.com/SoFTeR1/otium/main/build/icon.png";

enum Msg {
    Configure(Option<String>),
    Activity(Option<Value>),
}

static TX: OnceLock<Sender<Msg>> = OnceLock::new();
static CONNECTED: AtomicBool = AtomicBool::new(false);

pub fn connected() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

fn tx() -> &'static Sender<Msg> {
    TX.get_or_init(|| {
        let (tx, rx) = channel::<Msg>();
        std::thread::Builder::new()
            .name("discord".into())
            .spawn(move || {
                let mut client: Option<String> = None;
                let mut pipe: Option<std::fs::File> = None;
                let mut activity: Option<Value> = None;
                let mut dirty = false;
                loop {
                    // reconnect every 20 s while Discord isn't running
                    match rx.recv_timeout(Duration::from_secs(20)) {
                        Ok(Msg::Configure(id)) => {
                            if id != client {
                                client = id;
                                pipe = None; // closing the pipe clears the status in Discord
                                CONNECTED.store(false, Ordering::Relaxed);
                                dirty = true;
                            }
                        }
                        Ok(Msg::Activity(a)) => {
                            activity = a;
                            dirty = true;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    let Some(id) = &client else { continue };
                    if pipe.is_none() {
                        pipe = connect(id);
                        CONNECTED.store(pipe.is_some(), Ordering::Relaxed);
                        dirty = pipe.is_some();
                    }
                    if dirty {
                        if let Some(p) = pipe.as_mut() {
                            let cmd = json!({
                                "cmd": "SET_ACTIVITY",
                                "args": { "pid": std::process::id(), "activity": activity },
                                "nonce": format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)),
                            });
                            // read the reply too, or the pipe fills up
                            if write_frame(p, 1, &cmd).and_then(|_| read_frame(p)).is_err() {
                                pipe = None;
                                CONNECTED.store(false, Ordering::Relaxed);
                            } else {
                                dirty = false;
                            }
                        }
                    }
                }
            })
            .ok();
        tx
    })
}

fn connect(client: &str) -> Option<std::fs::File> {
    for i in 0..10 {
        let Ok(mut p) = std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\?\pipe\discord-ipc-{i}")) else { continue };
        if write_frame(&mut p, 0, &json!({ "v": 1, "client_id": client })).is_err() {
            continue;
        }
        // the first DISPATCH (READY) after the handshake means we can set activity
        if let Ok(msg) = read_frame(&mut p) {
            if msg["evt"] == "READY" {
                return Some(p);
            }
        }
    }
    None
}

fn write_frame(p: &mut std::fs::File, op: u32, v: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(v)?;
    let mut buf = Vec::with_capacity(8 + body.len());
    buf.extend_from_slice(&op.to_le_bytes());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    p.write_all(&buf)
}

fn read_frame(p: &mut std::fs::File) -> std::io::Result<Value> {
    let mut head = [0u8; 8];
    p.read_exact(&mut head)?;
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > 1 << 20 {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; len];
    p.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// Turns the status on (Some(app id)) or off (None).
pub fn configure(app_id: Option<&str>) {
    let _ = tx().send(Msg::Configure(app_id.map(str::to_string)));
}

pub fn set_activity(a: Option<Value>) {
    let _ = tx().send(Msg::Activity(a));
}
