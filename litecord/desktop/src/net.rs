//! The mailbox connection: one WebSocket to the LiteCord server's Inbox.
//!
//! It runs on its own thread with a small Tokio runtime, so the UI thread never waits on the
//! network. The UI sends JSON text in through `NetHandle::send`, and gets `NetEvent`s back.
//! If the connection drops, it reconnects with a growing delay, like the web app does.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub enum NetEvent {
    Connected,
    Disconnected { replaced: bool },
    Text(String),
}

#[derive(Clone)]
pub struct NetHandle {
    tx: mpsc::UnboundedSender<String>,
}

impl NetHandle {
    pub fn send(&self, json: String) {
        let _ = self.tx.send(json);
    }
}

/// Starts the connection thread. `events` gets everything that arrives.
pub fn start(url: String, events: async_channel::Sender<NetEvent>) -> NetHandle {
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    std::thread::Builder::new()
        .name("litecord-inbox".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
            rt.block_on(run(url, rx, events));
        })
        .expect("network thread");
    NetHandle { tx }
}

async fn run(url: String, mut outbox: mpsc::UnboundedReceiver<String>, events: async_channel::Sender<NetEvent>) {
    let mut fails: u32 = 0;
    // Messages the UI sent while we were offline wait here (the web app keeps up to 2000 too).
    let mut waiting: Vec<String> = Vec::new();
    loop {
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((ws, _)) => {
                fails = 0;
                let (mut write, mut read) = ws.split();
                if events.send(NetEvent::Connected).await.is_err() {
                    return;
                }
                // The UI sends "hello" right after Connected, then anything that was waiting.
                let mut ping = tokio::time::interval(Duration::from_secs(25));
                ping.tick().await;
                let mut replaced = false;
                let mut flushed = false;
                loop {
                    tokio::select! {
                        msg = read.next() => match msg {
                            Some(Ok(Message::Text(t))) => {
                                if t.as_str() == "pong" { continue; }
                                if events.send(NetEvent::Text(t.to_string())).await.is_err() { return; }
                            }
                            Some(Ok(Message::Close(frame))) => {
                                replaced = frame.map(|f| u16::from(f.code) == 4000).unwrap_or(false);
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => break,
                        },
                        out = outbox.recv() => match out {
                            Some(s) => {
                                // Let the UI's hello go first, then send what queued up while offline.
                                let hello = s.contains("\"type\":\"hello\"");
                                if write.send(Message::Text(s.clone().into())).await.is_err() { waiting.push(s); break; }
                                if hello && !flushed {
                                    flushed = true;
                                    for w in std::mem::take(&mut waiting) {
                                        if w.contains("\"type\":\"hello\"") { continue; }
                                        let _ = write.send(Message::Text(w.into())).await;
                                    }
                                }
                            }
                            None => return,
                        },
                        _ = ping.tick() => {
                            if write.send(Message::Text("ping".into())).await.is_err() { break; }
                        }
                    }
                }
                if events.send(NetEvent::Disconnected { replaced }).await.is_err() {
                    return;
                }
                if replaced {
                    return; // too many windows open on this account: this one gave way
                }
            }
            Err(_) => {
                fails += 1;
            }
        }
        // While offline, keep collecting what the UI wants to send.
        let delay = Duration::from_millis((2000 * fails.max(1) as u64).min(30_000) + rand::random::<u64>() % 1000);
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                out = outbox.recv() => match out {
                    Some(s) => if waiting.len() < 2000 { waiting.push(s) },
                    None => return,
                },
            }
        }
    }
}

/// Looks up who owns a username, like the web app's Add Friend box does.
pub fn lookup(base: &str, name: &str) -> Result<Option<String>, String> {
    let url = format!("{base}/api/lookup?name={}", urlencode(name));
    let mut res = ureq::get(&url).call().map_err(|e| e.to_string())?;
    let v: serde_json::Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(v.get("uid").and_then(|u| u.as_str()).map(String::from))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
