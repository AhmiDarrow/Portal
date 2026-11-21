use super::*;

const HASH: &str = "3f79bb7b435b05321651daefd374cdc681dc06faa65e374e38337b88ca046dea";

fn v(text: &str) -> Version {
    Version::parse(text).expect(text)
}

#[test]
fn versions_compare_numerically() {
    assert!(v("0.10.0") > v("0.9.9"));
    assert!(v("1.0.0") > v("0.99.99"));
    assert!(v("0.1.10") > v("0.1.9"));
    assert_eq!(v("1.2"), v("1.2.0"));
    assert_eq!(v("2"), v("2.0.0"));
    assert!(Version::current() >= v("0.1.0"));
}

#[test]
fn tags_parse_with_or_without_v() {
    assert_eq!(v("v1.2.3"), Version([1, 2, 3]));
    assert_eq!(v("1.2.3"), Version([1, 2, 3]));
    assert_eq!(v(" V0.2.0 "), Version([0, 2, 0]));
    assert_eq!(v("1.2.3+build.5"), Version([1, 2, 3]));
    assert_eq!(v("1.2.3").to_string(), "1.2.3");
    for bad in [
        "",
        "v",
        "1.2.3-rc.1",
        "1..2",
        "1.2.3.4",
        "is-6_4_3",
        "latest",
        "1.x",
        "-1.0",
    ] {
        assert_eq!(Version::parse(bad), None, "{bad}");
    }
}

fn sample(assets: &str) -> Vec<u8> {
    format!(
        r#"{{"url":"https://api.github.com/repos/AhmiDarrow/Portal/releases/1",
            "html_url":"https://github.com/AhmiDarrow/Portal/releases/tag/v0.2.0",
            "tag_name":"v0.2.0","name":"Portal 0.2.0","draft":false,"prerelease":false,
            "assets":[{assets}]}}"#
    )
    .into_bytes()
}

fn asset(name: &str, digest: Option<&str>) -> String {
    let digest = digest.map_or("null".to_string(), |d| format!("\"{d}\""));
    format!(
        r#"{{"name":"{name}","size":1234,"digest":{digest},"content_type":"application/octet-stream",
            "url":"https://api.github.com/repos/AhmiDarrow/Portal/releases/assets/{}",
            "browser_download_url":"https://github.com/AhmiDarrow/Portal/releases/download/v0.2.0/{name}"}}"#,
        name.len()
    )
}

#[test]
fn picks_the_setup_asset_and_its_digest() {
    let body = sample(
        &[
            asset(
                "Portal-0.2.0.zip",
                Some(&format!("sha256:{}", "0".repeat(64))),
            ),
            asset("Portal-Setup-0.2.0.exe", Some(&format!("sha256:{HASH}"))),
            asset("Portal-Setup-0.2.0.exe.sha256", None),
        ]
        .join(","),
    );
    let release = parse_release(&body).unwrap().unwrap();
    assert_eq!(release.version, Version([0, 2, 0]));
    assert_eq!(
        release.page.as_deref(),
        Some("https://github.com/AhmiDarrow/Portal/releases/tag/v0.2.0")
    );
    let setup = release.setup.unwrap();
    assert_eq!(setup.name, "Portal-Setup-0.2.0.exe");
    assert_eq!(setup.size, 1234);
    assert_eq!(
            setup.link,
            AssetLink {
                browser: "https://github.com/AhmiDarrow/Portal/releases/download/v0.2.0/Portal-Setup-0.2.0.exe".into(),
            }
        );
    assert_eq!(setup.checksum, Checksum::Known(parse_hex32(HASH).unwrap()));
}

#[test]
fn falls_back_to_the_sha256_asset() {
    let body = sample(
        &[
            asset("Portal-Setup-0.2.0.exe", None),
            asset("Portal-Setup-0.2.0.exe.sha256", None),
        ]
        .join(","),
    );
    let setup = parse_release(&body).unwrap().unwrap().setup.unwrap();
    assert_eq!(
            setup.checksum,
            Checksum::File(AssetLink {
                browser: "https://github.com/AhmiDarrow/Portal/releases/download/v0.2.0/Portal-Setup-0.2.0.exe.sha256"
                    .into(),
            })
        );
}

#[test]
fn missing_checksum_and_missing_asset() {
    let body = sample(&asset("Portal-Setup-0.2.0.exe", Some("md5:abc")));
    let setup = parse_release(&body).unwrap().unwrap().setup.unwrap();
    assert_eq!(setup.checksum, Checksum::Missing);

    let body = sample(&asset("Portal-Setup-0.1.0.exe", None));
    let release = parse_release(&body).unwrap().unwrap();
    assert!(release.setup.is_none());
    let shared = Shared {
        state: UpdateState::Idle,
        release: Some(release),
        progress: None,
        error: None,
        ready: None,
        checked: None,
    };
    let info = view_of(&shared);
    assert_eq!(info.latest.as_deref(), Some("0.2.0"));
    assert!(!info.available);
}

#[test]
fn unusable_releases_are_not_updates() {
    let other = br#"{"tag_name":"is-6_4_3","html_url":"https://github.com/jrsoftware/issrc/releases/tag/is-6_4_3",
            "assets":[{"name":"innosetup-6.4.3.exe","browser_download_url":"https://github.com/x/y/z.exe"}]}"#;
    assert_eq!(parse_release(other).unwrap(), None);
    let pre = br#"{"tag_name":"v9.0.0","prerelease":true,"assets":[]}"#;
    assert_eq!(parse_release(pre).unwrap(), None);
    assert!(parse_release(b"not json").is_err());
}

#[test]
fn insecure_asset_urls_are_ignored() {
    let body = sample(
        r#"{"name":"Portal-Setup-0.2.0.exe","browser_download_url":"http://example.com/Portal-Setup-0.2.0.exe"}"#,
    );
    assert!(parse_release(&body).unwrap().unwrap().setup.is_none());
}

#[test]
fn sha256_files_parse() {
    let hash = parse_hex32(HASH).unwrap();
    let name = "Portal-Setup-0.2.0.exe";
    assert_eq!(
        parse_sum_file(&format!("{HASH}  {name}\n"), name),
        Some(hash)
    );
    assert_eq!(
        parse_sum_file(&format!("{HASH} *{name}\r\n"), name),
        Some(hash)
    );
    assert_eq!(
        parse_sum_file(&format!("\n{}\n", HASH.to_uppercase()), name),
        Some(hash)
    );
    assert_eq!(parse_sum_file(&format!("{HASH}  Other.exe"), name), None);
    assert_eq!(parse_sum_file("abc  Portal-Setup-0.2.0.exe", name), None);
    assert_eq!(parse_sum_file("", name), None);
    assert_eq!(parse_digest(&format!("SHA256:{HASH}")), Some(hash));
    assert_eq!(parse_digest(HASH), None);
}

#[test]
fn checksum_mismatch_is_refused() {
    let body = b"portal installer bytes";
    let found: [u8; 32] = Sha256::digest(body).into();
    assert!(verify(&found, &found).is_ok());
    let mut wrong = found;
    wrong[0] ^= 1;
    let refusal = verify(&found, &wrong).unwrap_err();
    assert_eq!(refusal.1, BAD_CHECKSUM);
    assert_eq!(hex(&found).len(), 64);
    assert_eq!(parse_hex32(&hex(&found)), Some(found));
}

#[tokio::test]
async fn a_file_on_disk_is_hashed() {
    let dir = std::env::temp_dir().join(format!("portal-update-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Portal-Setup-0.0.1.exe");
    std::fs::write(&path, b"hello").unwrap();
    let expected: [u8; 32] = Sha256::digest(b"hello").into();
    assert_eq!(hash_file(path.clone()).await, Some(expected));
    clear_stale(&dir);
    assert!(!path.exists(), "an installer for an older version is stale");
    assert_eq!(hash_file(path).await, None);
    let _ = std::fs::remove_dir_all(&dir);
}

async fn read_head(stream: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).await.unwrap() == 1 {
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).to_ascii_lowercase()
}

/// Serves `routes` over loopback HTTP, one request per connection.
async fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let head = read_head(&mut stream).await;
            let path = head
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            let reply = match routes.iter().find(|(route, _)| *route == path) {
                Some((_, body)) => {
                    let mut out = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    out.extend_from_slice(body);
                    out
                }
                None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            };
            let _ = stream.write_all(&reply).await;
        }
    });
    port
}

#[tokio::test]
async fn downloads_are_verified_while_streaming() {
    let root = std::env::temp_dir().join(format!("portal-update-dl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let updater = Updater::new(&root);
    let body: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
    let hash: [u8; 32] = Sha256::digest(&body).into();
    let name = "Portal-Setup-9.9.9.exe";
    let port = serve(vec![
        ("/setup.exe", body.clone()),
        (
            "/setup.exe.sha256",
            format!("{}  {name}\n", hex(&hash)).into_bytes(),
        ),
        (
            "/bad.sha256",
            format!("{}  {name}\n", "0".repeat(64)).into_bytes(),
        ),
    ])
    .await;
    let link = |path: &str| AssetLink {
        browser: format!("http://127.0.0.1:{port}{path}"),
    };
    let target = updater.dir.join(name);

    // The digest GitHub publishes for the asset.
    let setup = Setup {
        name: name.into(),
        link: link("/setup.exe"),
        size: body.len() as u64,
        checksum: Checksum::Known(hash),
    };
    updater.fetch_installer(&setup, &target).await.unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), body);
    assert_eq!(updater.lock().progress, Some(1.0));

    // A file already downloaded and matching is used as is.
    let offline = Setup {
        link: link("/gone.exe"),
        ..setup.clone()
    };
    updater.fetch_installer(&offline, &target).await.unwrap();

    // A sibling .sha256 asset.
    std::fs::remove_file(&target).unwrap();
    let setup = Setup {
        checksum: Checksum::File(link("/setup.exe.sha256")),
        ..setup
    };
    updater.fetch_installer(&setup, &target).await.unwrap();
    assert!(target.is_file());

    // A mismatch leaves nothing behind.
    std::fs::remove_file(&target).unwrap();
    let setup = Setup {
        checksum: Checksum::File(link("/bad.sha256")),
        ..setup
    };
    let refusal = updater.fetch_installer(&setup, &target).await.unwrap_err();
    assert_eq!(refusal.1, BAD_CHECKSUM);
    assert!(!target.exists());
    assert!(!target.with_extension("exe.part").exists());

    // No checksum at all is refused before anything downloads.
    let setup = Setup {
        checksum: Checksum::Missing,
        ..setup
    };
    let refusal = updater.fetch_installer(&setup, &target).await.unwrap_err();
    assert_eq!(refusal.1, NO_CHECKSUM);
    assert!(!target.exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// A download that trickles in never stalls, but still ends at the limit
/// and leaves no partial file.
#[tokio::test]
async fn a_trickling_download_stops_at_the_overall_limit() {
    let root = std::env::temp_dir().join(format!("portal-update-slow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut updater = Updater::new(&root);
    updater.download_limit = Duration::from_millis(600);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let _ = read_head(&mut stream).await;
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n";
        if stream.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if stream.write_all(b"x").await.is_err() {
                return;
            }
        }
    });
    let name = "Portal-Setup-9.9.8.exe";
    let setup = Setup {
        name: name.into(),
        link: AssetLink {
            browser: format!("http://127.0.0.1:{port}/setup.exe"),
        },
        size: 100_000,
        checksum: Checksum::Known([0; 32]),
    };
    let target = updater.dir.join(name);
    let refusal = updater.fetch_installer(&setup, &target).await.unwrap_err();
    assert_eq!(refusal, (StatusCode::GATEWAY_TIMEOUT, DOWNLOAD_TOO_SLOW));
    assert!(!target.exists());
    assert!(!target.with_extension("exe.part").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// An install looks again first; when GitHub does not answer it goes on
/// with the release it knows, while a plain check reports the failure.
#[tokio::test]
async fn an_install_check_that_fails_keeps_the_known_release() {
    let root = std::env::temp_dir().join(format!("portal-update-keep-{}", std::process::id()));
    let mut updater = Updater::new(&root);
    // No client: every fetch fails, as when offline.
    updater.client = None;
    assert!(
        updater.check_locked(true).await.is_err(),
        "nothing known yet"
    );
    let known = Release {
        version: Version([99, 0, 0]),
        page: None,
        setup: None,
    };
    updater.lock().release = Some(known.clone());
    updater.lock().state = UpdateState::Failed;
    assert!(updater.check_locked(true).await.is_ok());
    assert_eq!(updater.lock().release, Some(known.clone()));
    assert_eq!(updater.lock().state, UpdateState::Idle);
    assert_eq!(updater.lock().error, None);
    assert!(updater.check_locked(false).await.is_err());
    assert_eq!(updater.lock().state, UpdateState::Failed);
    assert_eq!(updater.lock().release, Some(known));
    let _ = std::fs::remove_dir_all(&root);
}

/// Asks close together share GitHub's last answer; once it is older, or
/// something failed since, the next ask goes to GitHub.
#[tokio::test]
async fn a_check_just_answered_is_not_asked_again() {
    let root = std::env::temp_dir().join(format!("portal-update-reuse-{}", std::process::id()));
    let mut updater = Updater::new(&root);
    // No client: a check that really asks GitHub fails.
    updater.client = None;
    updater.lock().checked = Some(Instant::now());
    let info = updater.check().await;
    assert_eq!(info.state, UpdateState::Idle);
    assert_eq!(info.error, None);
    updater.lock().checked = Instant::now().checked_sub(CHECK_REUSE);
    assert_eq!(updater.check().await.state, UpdateState::Failed);
    updater.lock().checked = Some(Instant::now());
    assert!(
        !recently_checked(&updater.lock()),
        "a failure is not reused"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn repo_names_are_checked() {
    assert!(repo_ok("AhmiDarrow/Portal"));
    assert!(repo_ok("jrsoftware/issrc"));
    assert!(repo_ok("a-b/c.d_e"));
    for bad in [
        "", "Portal", "a/b/c", "../x", "a/..", "a b/c", "a/b?x=1", "/b", "a/",
    ] {
        assert!(!repo_ok(bad), "{bad}");
    }
}
