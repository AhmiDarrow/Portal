//! A stand-in for `portal.exe` on a free loopback port, for the launcher's tests: it answers
//! the routes the tray reads as a running Portal does, records every request, and can go
//! silent (accept, never answer: a hung server) or away (the port closes). Its ping names
//! no process, so the launcher never ends a process for it, and `/api/shutdown` is never
//! sent to it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct FakePortal {
    pub port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    silent: Arc<AtomicBool>,
    gone: Arc<AtomicBool>,
}

impl FakePortal {
    pub fn start() -> FakePortal {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_ne!(port, 8490, "never the real Portal's port");
        let fake = FakePortal {
            port,
            requests: Arc::default(),
            silent: Arc::default(),
            gone: Arc::default(),
        };
        let (requests, silent, gone) = (
            fake.requests.clone(),
            fake.silent.clone(),
            fake.gone.clone(),
        );
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                if gone.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                if silent.load(Ordering::SeqCst) {
                    // Accepted and never answered.
                    held.push(stream);
                    continue;
                }
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let line = head.lines().next().unwrap_or("").to_string();
                requests.lock().unwrap().push(line.clone());
                let path = line.split_whitespace().nth(1).unwrap_or("");
                let body = match path {
                    "/api/ping" => r#"{"app":"portal"}"#,
                    "/api/status" => r#"{"state":"running"}"#,
                    "/api/cast" => r#"{"lan":false}"#,
                    "/api/app/encoder" => r#"{"hardware":true,"card":"nvidia"}"#,
                    "/api/update" => {
                        r#"{"current":"0.3.10","latest":"0.3.10","available":false,"state":"idle"}"#
                    }
                    _ => r#"{"detail":"Not Found"}"#,
                };
                let status = if body.contains("Not Found") {
                    "404 Not Found"
                } else {
                    "200 OK"
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
            drop(held);
        });
        fake
    }

    /// The request lines seen so far ("GET /api/ping HTTP/1.1").
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// Stops answering, as a hung server does.
    pub fn go_silent(&self) {
        self.silent.store(true, Ordering::SeqCst);
    }

    /// Closes the port, as a server that went away does.
    pub fn go_away(&self) {
        self.gone.store(true, Ordering::SeqCst);
        // Wakes the accept loop so it sees the flag and drops the listener.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while TcpListener::bind(("127.0.0.1", self.port)).is_err() {
            assert!(std::time::Instant::now() < deadline, "the port stays held");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
