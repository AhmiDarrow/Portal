use super::*;

#[test]
fn concatenated_objects_and_braces_inside_strings() {
    let mut stream = JsonObjectStream::new();
    let first = stream.push("{\"a\":1}{\"b\":\"}");
    assert_eq!(first.len(), 1);
    let second = stream.push("\"}");
    assert_eq!(second.len(), 1);
    assert!(first[0].contains("\"a\""));
    assert!(second[0].contains('}'));
}

#[test]
fn a_split_object_is_completed_by_the_next_chunk() {
    let mut stream = JsonObjectStream::new();
    assert!(stream.push("{\"hello\":").is_empty());
    let done = stream.push("\"world\"}");
    assert_eq!(done.len(), 1);
    assert!(done[0].contains("world"));
}

#[test]
fn many_small_chunks_and_multibyte_text() {
    let mut stream = JsonObjectStream::new();
    let body = "noise {\"t\":\"caf\u{e9} {\\\"}\u{1f3ac}\"}\n{\"n\":[1,{\"x\":2}]}";
    let mut done = Vec::new();
    for ch in body.chars() {
        done.extend(stream.push(ch.encode_utf8(&mut [0; 4])));
    }
    assert_eq!(done.len(), 2);
    let first: Value = serde_json::from_str(&done[0]).unwrap();
    assert_eq!(first["t"], "caf\u{e9} {\"}\u{1f3ac}");
    assert_eq!(done[1], "{\"n\":[1,{\"x\":2}]}");
    assert!(stream.buf.is_empty());
    assert!(stream.push("stray text }").is_empty());
    assert!(stream.buf.is_empty());
}

#[test]
fn an_invalid_byte_keeps_the_rest_of_the_object() {
    let mut raw = Vec::new();
    let mut text = String::new();
    let mut stream = JsonObjectStream::new();
    raw.extend_from_slice(b"{\"t\":\"a\xffb\"}{\"u\":\"caf\xc3");
    drain_utf8(&mut raw, &mut text);
    assert_eq!(raw, b"\xc3");
    let done = stream.push(&std::mem::take(&mut text));
    assert_eq!(done.len(), 1);
    let first: Value = serde_json::from_str(&done[0]).unwrap();
    assert_eq!(first["t"], "a\u{fffd}b");
    raw.extend_from_slice(b"\xa9\"}");
    drain_utf8(&mut raw, &mut text);
    assert!(raw.is_empty());
    let done = stream.push(&std::mem::take(&mut text));
    assert_eq!(done.len(), 1);
    let second: Value = serde_json::from_str(&done[0]).unwrap();
    assert_eq!(second["u"], "caf\u{e9}");
}

#[tokio::test]
async fn a_fixture_socket_answers_one_call() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        let request: Value = serde_json::from_slice(&buf[..n]).unwrap();
        let id = request["id"].as_i64().unwrap();
        let body = json!({"jsonrpc":"2.0","id":id,"result":{"ok":true}});
        socket
            .write_all(serde_json::to_vec(&body).unwrap().as_slice())
            .await
            .unwrap();
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let result = client
        .call("JSONRPC.Ping", json!({}), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(result["ok"], true);
    client.close().await;
}

#[tokio::test]
async fn a_timed_out_call_leaves_the_connection_open() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut stream = JsonObjectStream::new();
        let mut ids = Vec::new();
        let mut buf = vec![0u8; 4096];
        while ids.len() < 2 {
            let n = socket.read(&mut buf).await.unwrap();
            for object in stream.push(&String::from_utf8_lossy(&buf[..n])) {
                let request: Value = serde_json::from_str(&object).unwrap();
                ids.push(request["id"].as_i64().unwrap());
            }
        }
        // The first reply arrives after its call gave up.
        for (id, value) in [(ids[0], "late"), (ids[1], "fresh")] {
            let body = json!({"jsonrpc":"2.0","id":id,"result":value});
            socket
                .write_all(serde_json::to_vec(&body).unwrap().as_slice())
                .await
                .unwrap();
        }
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let first = client
        .call("JSONRPC.Ping", json!({}), Duration::from_millis(100))
        .await;
    assert!(matches!(first, Err(RpcFail::Timeout(_))));
    assert!(client.connected().await);
    let second = client
        .call("JSONRPC.Ping", json!({}), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(second, "fresh");
    client.close().await;
}

#[tokio::test]
async fn waiting_behind_another_call_counts_toward_the_limit() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // The fixture reads but never answers, so the first call holds the lock.
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        while socket.read(&mut buf).await.is_ok_and(|n| n > 0) {}
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    let slow = client.clone();
    let held = tokio::spawn(async move {
        slow.call("Player.Open", json!({}), Duration::from_secs(5))
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = std::time::Instant::now();
    let quick = client
        .call("JSONRPC.Ping", json!({}), Duration::from_millis(150))
        .await;
    assert!(matches!(quick, Err(RpcFail::Timeout(_))));
    assert!(started.elapsed() < Duration::from_secs(2));
    held.abort();
    client.close().await;
}

#[tokio::test]
async fn a_send_kodi_never_reads_times_out_and_frees_the_lock() {
    // A small receive window, so the sender's buffers fill quickly.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(1).unwrap();
    let port = listener.local_addr().unwrap().port();
    // Accepts and holds the socket but never reads, so the send buffers fill.
    let held = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(socket);
    });
    let client = KodiClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    // Windows takes one whole send into its buffer however big, so the
    // stall shows on a later one.
    let blob = "x".repeat(4 * 1024 * 1024);
    for _ in 0..8 {
        let started = std::time::Instant::now();
        let sent = client
            .call(
                "Files.SetFileDetails",
                json!({ "blob": blob }),
                Duration::from_millis(500),
            )
            .await;
        assert!(matches!(sent, Err(RpcFail::Timeout(_))), "{sent:?}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        if !client.connected().await {
            break;
        }
    }
    // The half-sent stream is dropped, and the lock is free for the next call.
    assert!(!client.connected().await);
    let next = timeout(
        Duration::from_secs(1),
        client.call("JSONRPC.Ping", json!({}), Duration::from_secs(1)),
    )
    .await
    .expect("the write lock was released");
    assert!(matches!(next, Err(RpcFail::NotConnected)));
    held.abort();
}
