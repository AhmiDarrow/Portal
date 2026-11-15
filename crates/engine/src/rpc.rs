//! One JSON-RPC connection to Kodi. Objects are concatenated, with no delimiter.
//! A connection is serial: one call is in flight at a time.

use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio::time::{timeout, timeout_at, Instant};

#[derive(Debug, thiserror::Error)]
pub enum RpcFail {
    #[error("Kodi is not connected")]
    NotConnected,
    #[error("Kodi JSON-RPC connect timeout")]
    ConnectTimeout,
    #[error("Kodi connection closed")]
    Closed,
    #[error("Kodi RPC timeout: {0}")]
    Timeout(String),
    #[error("{message}")]
    Remote { code: i64, message: String },
    #[error("{0}")]
    Protocol(String),
}

#[derive(Debug, Clone)]
pub enum Note {
    Notification { method: String, params: Value },
    Disconnected,
}

/// Splits a byte stream of concatenated JSON objects. Brace depth survives
/// across chunks. Braces inside strings do not end an object.
#[derive(Debug, Default)]
pub struct JsonObjectStream {
    buf: String,
    scan: usize,
    depth: i32,
    in_str: bool,
    esc: bool,
}

impl JsonObjectStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// Only the new text is scanned. The markers are ASCII, and no byte of a
    /// multi-byte UTF-8 character is ASCII, so byte offsets are char boundaries.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let bytes = self.buf.as_bytes();
        let mut out = Vec::new();
        let mut start = 0usize;
        let mut index = self.scan;
        let mut depth = self.depth;
        let mut in_str = self.in_str;
        let mut esc = self.esc;
        while index < bytes.len() {
            let byte = bytes[index];
            if in_str {
                if esc {
                    esc = false;
                } else if byte == b'\\' {
                    esc = true;
                } else if byte == b'"' {
                    in_str = false;
                }
                index += 1;
                continue;
            }
            if byte == b'"' {
                in_str = true;
            } else if byte == b'{' {
                if depth == 0 {
                    start = index;
                }
                depth += 1;
            } else if byte == b'}' {
                if depth == 0 {
                    index += 1;
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    out.push(self.buf[start..=index].trim().to_string());
                    start = index + 1;
                }
            }
            index += 1;
        }
        // Outside an object nothing is kept, so stray text cannot pile up.
        if depth == 0 {
            start = self.buf.len();
        }
        self.buf.drain(..start);
        self.scan = self.buf.len();
        self.depth = depth;
        self.in_str = in_str;
        self.esc = esc;
        out
    }
}

struct Pending(oneshot::Sender<Result<Value, RpcFail>>);

struct Inner {
    host: String,
    port: u16,
    writer: Mutex<Option<OwnedWriteHalf>>,
    pending: Mutex<Option<(i64, Pending)>>,
    next_id: AtomicI64,
    era: AtomicU64,
    connected: AtomicBool,
    write: Mutex<()>,
    notes: broadcast::Sender<Note>,
}

#[derive(Clone)]
pub struct KodiClient {
    inner: std::sync::Arc<Inner>,
}

impl KodiClient {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        let (notes, _) = broadcast::channel(64);
        Self {
            inner: std::sync::Arc::new(Inner {
                host: host.into(),
                port,
                writer: Mutex::new(None),
                pending: Mutex::new(None),
                next_id: AtomicI64::new(1),
                era: AtomicU64::new(0),
                connected: AtomicBool::new(false),
                write: Mutex::new(()),
                notes,
            }),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Note> {
        self.inner.notes.subscribe()
    }

    pub async fn connected(&self) -> bool {
        self.inner.connected.load(Ordering::SeqCst)
    }

    pub async fn connect(&self) -> Result<(), RpcFail> {
        self.connect_timeout(Duration::from_secs(2)).await
    }

    pub async fn connect_timeout(&self, limit: Duration) -> Result<(), RpcFail> {
        if self.connected().await {
            return Ok(());
        }
        let _gate = self.inner.write.lock().await;
        if self.inner.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        let era = self.inner.era.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner.connected.store(false, Ordering::SeqCst);
        *self.inner.writer.lock().await = None;
        *self.inner.pending.lock().await = None;

        let address = format!("{}:{}", self.inner.host, self.inner.port);
        let stream = match timeout(limit, TcpStream::connect(address)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) => return Err(RpcFail::Closed),
            Err(_) => return Err(RpcFail::ConnectTimeout),
        };
        stream.set_nodelay(true).ok();
        let (reader, writer) = stream.into_split();
        if self.inner.era.load(Ordering::SeqCst) != era {
            return Err(RpcFail::Closed);
        }
        *self.inner.writer.lock().await = Some(writer);
        self.inner.connected.store(true, Ordering::SeqCst);
        let inner = self.inner.clone();
        tokio::spawn(async move {
            read_loop(inner, reader, era).await;
        });
        Ok(())
    }

    pub async fn close(&self) {
        self.inner.era.fetch_add(1, Ordering::SeqCst);
        self.inner.connected.store(false, Ordering::SeqCst);
        if let Some((_, pending)) = self.inner.pending.lock().await.take() {
            let _ = pending.0.send(Err(RpcFail::Closed));
        }
        *self.inner.writer.lock().await = None;
        let _ = self.inner.notes.send(Note::Disconnected);
    }

    pub async fn call(
        &self,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, RpcFail> {
        // Waiting behind another call counts toward this call's limit.
        let deadline = Instant::now() + limit;
        let Ok(_write) = timeout_at(deadline, self.inner.write.lock()).await else {
            return Err(RpcFail::Timeout(method.to_string()));
        };
        if !self.inner.connected.load(Ordering::SeqCst) {
            return Err(RpcFail::NotConnected);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let bytes =
            serde_json::to_vec(&payload).map_err(|err| RpcFail::Protocol(err.to_string()))?;
        let (tx, rx) = oneshot::channel();
        *self.inner.pending.lock().await = Some((id, Pending(tx)));
        let write_result = {
            let mut slot = self.inner.writer.lock().await;
            let writer = slot.as_mut().ok_or(RpcFail::NotConnected)?;
            writer.write_all(&bytes).await.and(writer.flush().await)
        };
        if write_result.is_err() {
            self.clear_pending(id).await;
            self.inner.connected.store(false, Ordering::SeqCst);
            return Err(RpcFail::Closed);
        }
        let result = timeout_at(deadline, rx).await;
        self.clear_pending(id).await;
        match result {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => Err(RpcFail::Closed),
            // The connection stays open. A late reply carries this id and is
            // dropped by `handle_object`.
            Err(_) => Err(RpcFail::Timeout(method.to_string())),
        }
    }

    async fn clear_pending(&self, id: i64) {
        let mut pending = self.inner.pending.lock().await;
        if pending
            .as_ref()
            .is_some_and(|(pending_id, _)| *pending_id == id)
        {
            *pending = None;
        }
    }
}

async fn read_loop(inner: std::sync::Arc<Inner>, mut reader: OwnedReadHalf, era: u64) {
    let mut raw = Vec::new();
    let mut text = String::new();
    let mut stream = JsonObjectStream::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let read = reader.read(&mut buf).await;
        let still = inner.era.load(Ordering::SeqCst) == era;
        if !still {
            return;
        }
        let count = match read {
            Ok(0) | Err(_) => {
                fail_connection(&inner).await;
                return;
            }
            Ok(count) => count,
        };
        raw.extend_from_slice(&buf[..count]);
        drain_utf8(&mut raw, &mut text);
        for object in stream.push(&std::mem::take(&mut text)) {
            handle_object(&inner, &object).await;
        }
    }
}

/// Moves every complete character from `raw` onto `text`. Each invalid
/// sequence becomes U+FFFD, and an incomplete one at the end stays in `raw`
/// for the next read.
fn drain_utf8(raw: &mut Vec<u8>, text: &mut String) {
    let mut start = 0;
    while start < raw.len() {
        match std::str::from_utf8(&raw[start..]) {
            Ok(valid) => {
                text.push_str(valid);
                start = raw.len();
            }
            Err(err) => {
                let valid = err.valid_up_to();
                text.push_str(std::str::from_utf8(&raw[start..start + valid]).unwrap_or(""));
                start += valid;
                match err.error_len() {
                    Some(len) => {
                        text.push('\u{FFFD}');
                        start += len;
                    }
                    None => break,
                }
            }
        }
    }
    raw.drain(..start);
}

async fn handle_object(inner: &Inner, object: &str) {
    let value: Value = match serde_json::from_str(object) {
        Ok(value) => value,
        Err(_) => return,
    };
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        let _ = inner.notes.send(Note::Notification {
            method: method.to_string(),
            params,
        });
        return;
    }
    let Some(id) = value.get("id").and_then(Value::as_i64) else {
        return;
    };
    let pending = {
        let mut slot = inner.pending.lock().await;
        let matches = slot
            .as_ref()
            .is_some_and(|(pending_id, _)| *pending_id == id);
        if !matches {
            return;
        }
        slot.take()
    };
    let Some((_, pending)) = pending else {
        return;
    };
    if let Some(error) = value.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(-1);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let _ = pending.0.send(Err(RpcFail::Remote {
            code,
            message: crate::error::public_error(&message),
        }));
        return;
    }
    let result = value.get("result").cloned().unwrap_or(Value::Null);
    let _ = pending.0.send(Ok(result));
}

async fn fail_connection(inner: &Inner) {
    inner.connected.store(false, Ordering::SeqCst);
    *inner.writer.lock().await = None;
    if let Some((_, pending)) = inner.pending.lock().await.take() {
        let _ = pending.0.send(Err(RpcFail::Closed));
    }
    let _ = inner.notes.send(Note::Disconnected);
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/rpc.rs"]
mod tests;
