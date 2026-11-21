use super::*;
use std::path::PathBuf;

const SAMPLE: &str = "RunPlugin(plugin://plugin.video.example/?action=auth)";

#[test]
fn action_packet_is_one_kodi_builtin_datagram() {
    let packet = action_packet(SAMPLE).unwrap();
    let payload = 1 + SAMPLE.len() + 1;
    assert_eq!(&packet[0..4], b"XBMC");
    assert_eq!(packet[4], 2);
    assert_eq!(packet[5], 0);
    assert_eq!(&packet[6..8], &ACTION.to_be_bytes());
    assert_eq!(&packet[8..12], &1u32.to_be_bytes());
    assert_eq!(&packet[12..16], &1u32.to_be_bytes());
    assert_eq!(&packet[16..18], &(payload as u16).to_be_bytes());
    assert_eq!(&packet[18..22], &CLIENT_ID.to_be_bytes());
    assert_eq!(&packet[22..32], &[0u8; 10]);
    assert_eq!(packet.len(), HEADER + payload);
    assert_eq!(packet[32], EXEC_BUILTIN);
    assert_eq!(&packet[33..packet.len() - 1], SAMPLE.as_bytes());
    assert_eq!(*packet.last().unwrap(), 0);
    let padded = action_packet(&format!("  {SAMPLE}  ")).unwrap();
    assert_eq!(padded, packet);
}

#[test]
fn only_one_runplugin_is_accepted() {
    assert!(action_packet(SAMPLE).is_some());
    let oversize = format!("RunPlugin(plugin://{})", "a".repeat(MAX_BUILTIN));
    let rejected = [
        "ActivateWindow(10025,\"plugin://plugin.video.example/\",return)",
        "RunPlugin(plugin://plugin.video.example/?action=auth)\nSystem.Exec(calc)",
        "RunPlugin(plugin://plugin.video.example/?action=auth)System.Exec(calc)",
        "RunPlugin(plugin://plugin.video.example/?action=auth);System.Exec(calc)",
        "RunPlugin(plugin://x",
        "RunPlugin(special://home/file.py)",
        "RunPlugin(plugin://)",
        "RunPlugin(plugin://plugin.video.example/?q=(1))",
        oversize.as_str(),
    ];
    for builtin in rejected {
        assert!(action_packet(builtin).is_none(), "{builtin}");
    }
    assert!(action_packet("SetFocus(11)").is_some());
    assert!(action_packet("SetFocus(10)").is_some());
    assert!(action_packet("SetFocus(12)").is_some());
    assert!(action_packet("SetFocus(99)").is_none());
    assert!(action_packet("SetFocus(11);System.Exec(calc)").is_none());
}

#[test]
fn event_port_ignores_the_default_attribute_and_the_range() {
    let xml = r#"<settings>
            <setting id="services.esportrange" default="true">10</setting>
            <setting id="services.esport" default="true">9778</setting>
            <setting id="services.esenabled" default="true">false</setting>
        </settings>"#;
    assert_eq!(event_port(xml), 9778);
    assert!(!event_enabled(xml));
    assert_eq!(event_port(""), 9777);
    assert!(event_enabled(""));
    assert!(event_enabled(
        r#"<setting id="services.esenabled" default="true"></setting>"#
    ));
    let valued = r#"<setting id="services.esport" value="9779" default="true"/>"#;
    assert_eq!(event_port(valued), 9779);
    assert_eq!(
        event_port(r#"<setting id="services.esport" default="true">0</setting>"#),
        9777
    );
}

#[tokio::test]
async fn a_disabled_event_server_returns_before_any_send() {
    let dir = scratch("disabled");
    let path = dir.join("guisettings.xml");
    std::fs::write(
        &path,
        r#"<settings><setting id="services.esenabled" default="true">false</setting></settings>"#,
    )
    .unwrap();
    let err = deliver(&path, SAMPLE, Duration::from_millis(1))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), NOT_ACCEPTING);
    assert!(!err.to_string().to_ascii_lowercase().contains("password"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_listening_port_receives_one_action_packet() {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = socket.local_addr().unwrap().port();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let dir = scratch("listen");
    let path = dir.join("guisettings.xml");
    std::fs::write(
        &path,
        format!(
            r#"<settings><setting id="services.esport" default="true">{port}</setting></settings>"#
        ),
    )
    .unwrap();
    deliver(&path, SAMPLE, Duration::from_millis(200))
        .await
        .unwrap();
    let mut buf = [0u8; 1200];
    let (size, from) = socket.recv_from(&mut buf).unwrap();
    assert_eq!(&buf[..size], action_packet(SAMPLE).unwrap());
    assert!(from.ip().is_loopback());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_unopened_port_is_an_error() {
    let probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let dir = scratch("closed");
    let path = dir.join("guisettings.xml");
    std::fs::write(
        &path,
        format!(r#"<settings><setting id="services.esport">{port}</setting></settings>"#),
    )
    .unwrap();
    let err = deliver(&path, SAMPLE, Duration::from_millis(200))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), NOT_ACCEPTING);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_held_udp_port_is_busy_and_a_free_one_is_not() {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = socket.local_addr().unwrap().port();
    assert!(udp_busy(port));
    drop(socket);
    assert!(!udp_busy(port));
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("portal-event-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_settings_action_must_run_an_add_on() {
    assert!(settings_builtin_allowed(
        "RunPlugin(plugin://plugin.video.example/?mode=auth)"
    ));
    assert!(settings_builtin_allowed(
        "RunPlugin(\"plugin://plugin.video.example/?a=1\")"
    ));
    assert!(!settings_builtin_allowed(
        "RunPlugin(special://home/x?plugin://plugin.video.example/)"
    ));
    assert!(!settings_builtin_allowed("RunPlugin(plugin://)"));
    assert!(!settings_builtin_allowed("RunPlugin(plugin://bad host/)"));
    assert!(!settings_builtin_allowed(
        "RunScript(plugin://plugin.video.example/)"
    ));
}
