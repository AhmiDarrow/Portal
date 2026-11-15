//! Ask Kodi to run one settings `RunPlugin` builtin on this computer.
//!
//! `Addons.ExecuteAddon` opens a video plugin with `ActivateWindow` and waits
//! for a directory, which covers the sign-in dialog. Kodi's event server runs
//! the builtin string itself. One localhost datagram, no greeting.

use crate::error::EngineError;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::Path;
use std::time::Duration;

const HEADER: usize = 32;
const MAX_PAYLOAD: usize = 992;
const MAX_BUILTIN: usize = 900;
/// Packet type `PT_ACTION`.
const ACTION: u16 = 0x0A;
/// Payload byte `AT_EXEC_BUILTIN`.
const EXEC_BUILTIN: u8 = 0x01;
/// Kodi keys the client by this id. It is not a secret.
const CLIENT_ID: u32 = 0x504F_5254;
const EVENT_WAIT: Duration = Duration::from_secs(8);
const NOT_ACCEPTING: &str = "Kodi is not accepting add-on actions from this computer.";

/// Send `builtin` to the event server port in `guisettings.xml`.
/// A missing file uses port 9777. An explicit disable is an error.
/// This does not take the supervisor lock.
pub async fn run_settings_builtin(guisettings: &Path, builtin: &str) -> Result<(), EngineError> {
    deliver(guisettings, builtin, EVENT_WAIT).await
}

async fn deliver(guisettings: &Path, builtin: &str, wait: Duration) -> Result<(), EngineError> {
    let packet = action_packet(builtin)
        .ok_or_else(|| EngineError::BadRequest("That action is not available.".into()))?;
    let port = listen_port(guisettings)?;
    if !wait_until_bound(port, wait).await {
        return Err(EngineError::Message(NOT_ACCEPTING.into()));
    }
    send_action(port, &packet)
}

fn listen_port(path: &Path) -> Result<u16, EngineError> {
    let xml = std::fs::read_to_string(path).unwrap_or_default();
    let xml = xml.trim_start_matches('\u{feff}');
    if !event_enabled(xml) {
        return Err(EngineError::Message(NOT_ACCEPTING.into()));
    }
    Ok(event_port(xml))
}

/// Missing or blank means the event server is on. `false` and `0` mean off.
fn event_enabled(xml: &str) -> bool {
    match setting_body(xml, "services.esenabled") {
        Some(text) => {
            let lower = text.trim().to_ascii_lowercase();
            !(lower == "false" || lower == "0")
        }
        None => true,
    }
}

/// Element text wins over a missing `value`. The `default` attribute is ignored.
fn event_port(xml: &str) -> u16 {
    setting_body(xml, "services.esport")
        .and_then(|text| text.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(9777)
}

fn setting_body(xml: &str, id: &str) -> Option<String> {
    let at = [format!("id=\"{id}\""), format!("id='{id}'")]
        .into_iter()
        .find_map(|needle| xml.find(&needle))?;
    let start = xml[..at].rfind('<')?;
    let end = at + xml[at..].find('>')?;
    let tag = &xml[start..=end];
    if let Some(value) = attr_value(tag, "value") {
        return Some(value.trim().to_string());
    }
    if tag.ends_with("/>") {
        return Some(String::new());
    }
    let rest = &xml[end + 1..];
    let close = rest.find("</setting>")?;
    Some(rest[..close].trim().to_string())
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let mut from = 0;
    while let Some(rel) = tag[from..].find(&key) {
        let start = from + rel;
        let boundary = start == 0 || tag.as_bytes()[start - 1].is_ascii_whitespace();
        if !boundary {
            from = start + key.len();
            continue;
        }
        let after = tag[start + key.len()..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let inner = &after[quote.len_utf8()..];
        let stop = inner.find(quote)?;
        return Some(inner[..stop].to_string());
    }
    None
}

/// One `RunPlugin(plugin://...)` and nothing after it.
pub fn action_packet(builtin: &str) -> Option<Vec<u8>> {
    let builtin = builtin.trim();
    if !settings_builtin_allowed(builtin) {
        return None;
    }
    let bytes = builtin.as_bytes();
    let payload_len = 1 + bytes.len() + 1;
    if payload_len > MAX_PAYLOAD {
        return None;
    }
    let mut packet = Vec::with_capacity(HEADER + payload_len);
    packet.extend_from_slice(b"XBMC");
    packet.push(2);
    packet.push(0);
    packet.extend_from_slice(&ACTION.to_be_bytes());
    packet.extend_from_slice(&1u32.to_be_bytes());
    packet.extend_from_slice(&1u32.to_be_bytes());
    packet.extend_from_slice(&(payload_len as u16).to_be_bytes());
    packet.extend_from_slice(&CLIENT_ID.to_be_bytes());
    packet.extend_from_slice(&[0u8; 10]);
    packet.push(EXEC_BUILTIN);
    packet.extend_from_slice(bytes);
    packet.push(0);
    Some(packet)
}

/// Move focus to one standard yes/no button. `SetFocus` takes a control id
/// on the active dialog: 11 is Yes, 10 is No, 12 is the custom button.
pub async fn send_dialog_focus(guisettings: &Path, control: i64) -> Result<(), EngineError> {
    let builtin = format!("SetFocus({control})");
    let packet = action_packet(&builtin)
        .ok_or_else(|| EngineError::BadRequest("That action is not available.".into()))?;
    if !guisettings.is_file() {
        return Err(EngineError::Message(NOT_ACCEPTING.into()));
    }
    let port = listen_port(guisettings)?;
    if !udp_busy(port) {
        return Err(EngineError::Message(NOT_ACCEPTING.into()));
    }
    send_action(port, &packet)
}

fn settings_builtin_allowed(builtin: &str) -> bool {
    if builtin.is_empty() || builtin.len() > MAX_BUILTIN || builtin.chars().any(char::is_control) {
        return false;
    }
    if matches!(builtin, "SetFocus(10)" | "SetFocus(11)" | "SetFocus(12)") {
        return true;
    }
    let Some(inner) = builtin
        .strip_prefix("RunPlugin(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    if inner.is_empty() || inner.contains(['(', ')']) {
        return false;
    }
    // The command must run an add-on: `plugin://<add-on id>/...`, with nothing
    // in front, quoted or not.
    let inner = inner.trim().trim_matches('"');
    let Some(rest) = inner
        .get(..9)
        .filter(|scheme| scheme.eq_ignore_ascii_case("plugin://"))
        .map(|_| &inner[9..])
    else {
        return false;
    };
    let host = rest.split(['/', '?']).next().unwrap_or("");
    !host.is_empty()
        && host.len() <= 80
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

async fn wait_until_bound(port: u16, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if udp_busy(port) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return udp_busy(port);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// True when something already holds `127.0.0.1:port`. A successful bind means
/// the port is free, and that socket is closed before this returns.
fn udp_busy(port: u16) -> bool {
    match UdpSocket::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(socket) => {
            drop(socket);
            false
        }
        Err(_) => true,
    }
}

fn send_action(port: u16, packet: &[u8]) -> Result<(), EngineError> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| EngineError::Message(NOT_ACCEPTING.into()))?;
    socket
        .send_to(packet, (Ipv4Addr::LOCALHOST, port))
        .map_err(|_| EngineError::Message(NOT_ACCEPTING.into()))?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/event.rs"]
mod tests;
