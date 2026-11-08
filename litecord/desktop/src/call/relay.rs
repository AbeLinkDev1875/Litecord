//! The relay connection: one WebSocket to `/relay?room=…&id=…&name=…` (the `Room` Durable
//! Object in `cloudflare/src/worker.js`). It reconnects after 2 seconds if it drops, like the
//! web app, and pings every 20 seconds.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::Message;

pub enum RelayIn {
    Open,
    Closed,
    Text(String),
    Binary(Vec<u8>),
}

pub enum RelayOut {
    Text(String),
    Binary(Vec<u8>),
}

pub fn url(base: &str, room: &str, id: &str, name: &str) -> String {
    let ws = base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    format!("{}/relay?room={}&id={}&name={}", ws.trim_end_matches('/'), enc(room), id, enc(name))
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        _ => format!("%{:02X}", b),
    }).collect()
}

/// Runs until `outbox` is dropped. Everything that arrives goes to `inbox` tagged with `tag`,
/// so a late message from an old room can be told apart.
pub async fn run<T: Copy + Send + 'static>(url: String, mut outbox: UnboundedReceiver<RelayOut>, inbox: UnboundedSender<(T, RelayIn)>, tag: T) {
    loop {
        if let Ok((ws, _)) = tokio_tungstenite::connect_async(url.as_str()).await {
            let (mut write, mut read) = ws.split();
            if inbox.send((tag, RelayIn::Open)).is_err() {
                return;
            }
            let mut ping = tokio::time::interval(Duration::from_secs(20));
            ping.tick().await;
            loop {
                tokio::select! {
                    msg = read.next() => match msg {
                        Some(Ok(Message::Text(t))) => {
                            if t.as_str() == "pong" { continue; }
                            if inbox.send((tag, RelayIn::Text(t.to_string()))).is_err() { return; }
                        }
                        Some(Ok(Message::Binary(b))) => {
                            if inbox.send((tag, RelayIn::Binary(b.to_vec()))).is_err() { return; }
                        }
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        Some(Ok(_)) => {}
                    },
                    out = outbox.recv() => match out {
                        Some(RelayOut::Text(s)) => { if write.send(Message::Text(s.into())).await.is_err() { break; } }
                        Some(RelayOut::Binary(b)) => { if write.send(Message::Binary(b.into())).await.is_err() { break; } }
                        None => { let _ = write.send(Message::Close(None)).await; return; }
                    },
                    _ = ping.tick() => {
                        if write.send(Message::Text("ping".into())).await.is_err() { break; }
                    }
                }
            }
            if inbox.send((tag, RelayIn::Closed)).is_err() {
                return;
            }
        }
        // Wait 2 seconds; drop whatever is sent meanwhile (the room starts over on reconnect).
        let sleep = tokio::time::sleep(Duration::from_millis(2000));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                out = outbox.recv() => if out.is_none() { return },
            }
        }
    }
}

/// Binary piece of a big message: [2-byte header length][JSON header][bytes].
pub fn frame(header: &serde_json::Value, body: &[u8]) -> Vec<u8> {
    let hd = header.to_string().into_bytes();
    let mut out = Vec::with_capacity(2 + hd.len() + body.len());
    out.push((hd.len() >> 8) as u8);
    out.push((hd.len() & 255) as u8);
    out.extend_from_slice(&hd);
    out.extend_from_slice(body);
    out
}

/// Relay binary: [to length][to id][payload].
pub fn addressed(to: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + to.len() + payload.len());
    out.push(to.len() as u8);
    out.extend_from_slice(to.as_bytes());
    out.extend_from_slice(payload);
    out
}

/// Splits relay binary that arrived: ([from], payload).
pub fn split_from(b: &[u8]) -> Option<(String, &[u8])> {
    let n = *b.first()? as usize;
    if b.len() < 1 + n {
        return None;
    }
    Some((String::from_utf8_lossy(&b[1..1 + n]).into_owned(), &b[1 + n..]))
}

/// Puts big messages back together (the web app sends text over 16 000 characters in pieces).
#[derive(Default)]
pub struct BigIn {
    parts: std::collections::HashMap<String, (Vec<Option<Vec<u8>>>, usize)>,
}

impl BigIn {
    /// Returns the whole text once the last piece is in.
    pub fn add(&mut self, from: &str, payload: &[u8]) -> Option<String> {
        if payload.len() < 2 {
            return None;
        }
        let n = ((payload[0] as usize) << 8) | payload[1] as usize;
        let hd: serde_json::Value = serde_json::from_slice(payload.get(2..2 + n)?).ok()?;
        if hd.get("t")?.as_str()? != "big" {
            return None;
        }
        let body = &payload[2 + n..];
        let total = hd.get("n")?.as_u64()? as usize;
        let i = hd.get("i")?.as_u64()? as usize;
        if total < 1 || total > 2048 || i >= total {
            return None;
        }
        let key = format!("{}:{}", from, hd.get("mid")?.as_str()?);
        let entry = self.parts.entry(key.clone()).or_insert_with(|| (vec![None; total], 0));
        if entry.0.len() != total || entry.0[i].is_some() {
            return None;
        }
        entry.0[i] = Some(body.to_vec());
        entry.1 += 1;
        if entry.1 < total {
            return None;
        }
        let (parts, _) = self.parts.remove(&key)?;
        let bytes: Vec<u8> = parts.into_iter().flatten().flatten().collect();
        String::from_utf8(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn big_messages_round_trip() {
        let text = "x".repeat(40000);
        let bytes = text.as_bytes();
        let chunks: Vec<&[u8]> = bytes.chunks(16 * 1024).collect();
        let mut big = BigIn::default();
        let mut got = None;
        for (i, c) in chunks.iter().enumerate() {
            let wire = addressed("abcd", &frame(&json!({"t": "big", "mid": "m1", "i": i, "n": chunks.len()}), c));
            let (from, payload) = split_from(&wire).unwrap();
            assert_eq!(from, "abcd");
            got = big.add(&from, payload);
        }
        assert_eq!(got.as_deref(), Some(text.as_str()));
    }

    #[test]
    fn relay_url() {
        assert_eq!(url("https://x.dev", "dm-ab", "id1234", "A B"), "wss://x.dev/relay?room=dm-ab&id=id1234&name=A%20B");
    }
}
