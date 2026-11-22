use super::*;

fn scratch() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("portal-cast-{nanos}-{n}"))
}

#[test]
fn cast_json_defaults_to_lan() {
    assert!(settings_from_text(None).lan);
    // A damaged file keeps Portal on this PC.
    assert!(!settings_from_text(Some("")).lan);
    assert!(settings_from_text(Some("{}")).lan);
    assert!(!settings_from_text(Some("null")).lan);
    assert!(!settings_from_text(Some("{\"lan\":tr")).lan);
    assert!(settings_from_text(Some(r#"{"lan":true}"#)).lan);
    assert!(!settings_from_text(Some(r#"{"lan":false}"#)).lan);
    assert_eq!(listen_addr(true), "0.0.0.0:8490");
    assert_eq!(listen_addr(false), "127.0.0.1:8490");
    assert!(!restart_needed(true, true));
    assert!(!restart_needed(false, false));
    assert!(restart_needed(true, false));
    assert!(restart_needed(false, true));
    assert_eq!(
        cast_url(true, Some("192.0.2.3")).as_deref(),
        Some("http://192.0.2.3:8490")
    );
    assert_eq!(cast_url(false, Some("192.0.2.3")), None);
    assert_eq!(cast_url(true, None), None);

    let dir = scratch();
    let hub = CastHub::open(&dir);
    assert!(hub.settings.lan);
    assert!(hub.bound_lan);
    assert_eq!(hub.bind_addr(), "0.0.0.0:8490");
    let raw = std::fs::read_to_string(dir.join("cast.json")).unwrap();
    assert!(settings_from_text(Some(&raw)).lan);

    std::fs::write(dir.join("cast.json"), "{\"lan\":false}\n").unwrap();
    let off = CastHub::open(&dir);
    assert!(!off.settings.lan);
    assert_eq!(off.bind_addr(), "127.0.0.1:8490");

    // Turning TVs on hands out no link until Portal listens on the network.
    let mut local = CastHub::open(&dir);
    let pending = local.put_lan(true).unwrap();
    assert_eq!(pending.restart_needed, Some(true));
    assert!(pending.url.is_none());
    std::fs::write(
        dir.join("cast.json"),
        "{\"lan\":false}
",
    )
    .unwrap();

    // A damaged file keeps Portal on this PC.
    std::fs::write(dir.join("cast.json"), "not-json").unwrap();
    assert!(!CastHub::open(&dir).settings.lan);

    let mut hub = CastHub::open(&scratch());
    let changed = hub.put_lan(false).unwrap();
    assert_eq!(changed.restart_needed, Some(true));
    assert_eq!(changed.bind, "127.0.0.1:8490");
    // Until the restart Portal still listens on the network, so TV links stay as they are.
    assert_eq!(changed.url, hub.suggested_url());
    assert!(changed.session.is_none());
    let still = hub.put_lan(false).unwrap();
    assert_eq!(still.restart_needed, Some(true));
    let back = hub.put_lan(true).unwrap();
    assert_eq!(back.restart_needed, Some(false));
    assert_eq!(back.bind, "0.0.0.0:8490");
    assert!(settings_from_text(Some(&std::fs::read_to_string(&hub.path).unwrap())).lan);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(hub.path.parent().unwrap());
}

#[test]
fn receivers_expire_after_45s() {
    let start = Instant::now();
    let mut rows = vec![
        ReceiverStamp {
            id: "old".into(),
            name: "Old".into(),
            seen: start,
            seen_ms: 1,
        },
        ReceiverStamp {
            id: "fresh".into(),
            name: "Fresh".into(),
            seen: start + Duration::from_secs(20),
            seen_ms: 2,
        },
    ];
    expire_receivers(&mut rows, start + Duration::from_secs(44), RECEIVER_TTL);
    assert_eq!(rows.len(), 2);
    expire_receivers(&mut rows, start + Duration::from_secs(45), RECEIVER_TTL);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "fresh");

    touch_receiver(
        &mut rows,
        "fresh".into(),
        "Den".into(),
        start + Duration::from_secs(70),
        3,
        RECEIVER_TTL,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Den");
    assert_eq!(rows[0].seen_ms, 3);
    touch_receiver(
        &mut rows,
        "other".into(),
        "Kitchen".into(),
        start + Duration::from_secs(70),
        4,
        RECEIVER_TTL,
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].id, "other");
}

#[test]
fn session_set_requires_a_live_receiver_and_clear_empties_it() {
    let receivers = vec![ReceiverStamp {
        id: "tv".into(),
        name: "Living room".into(),
        seen: Instant::now(),
        seen_ms: 1,
    }];
    assert_eq!(
        assign_session(&receivers, "missing", "job", None, None),
        Err(SessionSetError::UnknownReceiver)
    );
    assert_eq!(
        assign_session(&receivers, "tv", "  ", None, None),
        Err(SessionSetError::MissingField)
    );
    let session = assign_session(
        &receivers,
        " tv ",
        " job-1 ",
        Some("  Pilot  ".into()),
        None,
    )
    .unwrap();
    assert_eq!(session.receiver_id, "tv");
    assert_eq!(session.job_id, "job-1");
    assert_eq!(session.title.as_deref(), Some("Pilot"));
    assert!(session.replay.is_none());
    let view = session_view(&session);
    let mut slot = Some(session);
    clear_session(&mut slot);
    assert!(slot.is_none());
    let empty = SessionView::default();
    let value = serde_json::to_value(&empty).unwrap();
    assert!(value["receiver_id"].is_null());
    assert!(value["job_id"].is_null());
    assert!(value["title"].is_null());
    let listed = serde_json::to_value(view).unwrap();
    assert_eq!(listed["receiver_id"], "tv");
    assert_eq!(listed["job_id"], "job-1");
    assert_eq!(listed["title"], "Pilot");
    assert!(listed.get("place").is_none());
    assert!(listed.get("command").is_none());

    let status = serde_json::to_value(CastBody {
        lan: true,
        bind: listen_addr(true),
        url: None,
        receivers: vec![],
        session: None,
        restart_needed: None,
    })
    .unwrap();
    assert_eq!(status["bind"], "0.0.0.0:8490");
    assert!(status["url"].is_null());
    assert!(status["session"].is_null());
    assert!(status.get("restart_needed").is_none());
}

#[test]
fn a_special_keeps_season_zero_on_a_cast() {
    let replay = cast_replay(
        &ReplayFields {
            kind: Some("episode".into()),
            id: Some("plugin://plugin.video.example/?show=1".into()),
            file: Some("plugin://plugin.video.example/?ep=1".into()),
            season: Some(0),
            episode: Some(1),
            ..ReplayFields::default()
        },
        Some("Pilot"),
    )
    .unwrap();
    assert_eq!(replay.season, Some(0));
    assert_eq!(replay.episode, Some(1));
    let dropped = cast_replay(
        &ReplayFields {
            kind: Some("episode".into()),
            id: Some("show".into()),
            file: Some("file".into()),
            season: Some(1),
            episode: Some(0),
            ..ReplayFields::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(dropped.season, Some(1));
    assert_eq!(dropped.episode, None);
}

#[test]
fn browser_place_and_command_follow_the_same_job() {
    let dir = scratch();
    std::fs::create_dir_all(&dir).unwrap();
    let mut hub = CastHub::open(&dir);
    let now = Instant::now();
    hub.presence("tv", "Den", now).unwrap();
    hub.put_session("tv", "job", Some("Pilot".into()), None, now)
        .unwrap();
    hub.put_place(12.5, 100.0, false).unwrap();
    let view = hub.session_view();
    let place = view.place.expect("place");
    assert_eq!(place.position, 12.5);
    assert!(!place.paused);
    hub.put_command("seek", Some(40.0)).unwrap();
    hub.put_session("tv", "job", Some("Pilot".into()), None, now)
        .unwrap();
    let kept = hub.session_view();
    assert_eq!(kept.place.as_ref().map(|item| item.position), Some(12.5));
    assert_eq!(kept.command.as_ref().map(|item| item.seq), Some(1));
    assert_eq!(
        kept.command.as_ref().map(|item| item.action.as_str()),
        Some("seek")
    );
    hub.put_session("tv", "job-2", None, None, now).unwrap();
    let fresh = hub.session_view();
    assert!(fresh.place.is_none());
    assert!(fresh.command.is_none());
    assert!(hub.put_command("jump", None).is_err());
    assert!(hub.put_place(f64::NAN, 1.0, false).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cast_ends_once_its_tv_goes_quiet_and_keeps_the_last_place() {
    let dir = scratch();
    std::fs::create_dir_all(&dir).unwrap();
    let mut hub = CastHub::open(&dir);
    let start = Instant::now();
    hub.presence("tv", "Den", start).unwrap();
    hub.put_session("tv", "job", Some("Pilot".into()), None, start)
        .unwrap();
    hub.put_place(612.0, 1800.0, false).unwrap();

    // Still saying it is there, the TV keeps the cast.
    hub.presence("tv", "Den", start + Duration::from_secs(15))
        .unwrap();
    hub.expire_session(start + Duration::from_secs(34));
    assert_eq!(hub.session_view().job_id.as_deref(), Some("job"));
    assert!(hub.session_view().lost.is_none());

    // Quiet for longer than four pulses, it does not.
    hub.expire_session(start + Duration::from_secs(35));
    let view = hub.session_view();
    assert!(view.job_id.is_none());
    let lost = view.lost.expect("lost");
    assert_eq!(lost.receiver_id, "tv");
    assert_eq!(lost.job_id, "job");
    assert_eq!(lost.place.as_ref().map(|place| place.position), Some(612.0));
    let value = serde_json::to_value(hub.session_view()).unwrap();
    assert!(value["job_id"].is_null());
    assert_eq!(value["lost"]["job_id"], "job");
    assert_eq!(value["lost"]["place"]["position"], 612.0);
    assert!(hub.put_place(620.0, 1800.0, false).is_err());

    // A new cast, or Stop, forgets the lost one.
    let later = Instant::now();
    hub.presence("tv", "Den", later).unwrap();
    hub.put_session("tv", "job", None, None, later).unwrap();
    assert!(hub.session_view().lost.is_none());
    assert!(hub.session_view().place.is_none());
    hub.delete_session();
    assert!(hub.session_view().lost.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_session_view_shows_a_lost_cast_only_without_a_live_one() {
    let receivers = vec![ReceiverStamp {
        id: "tv".into(),
        name: "Den".into(),
        seen: Instant::now(),
        seen_ms: 1,
    }];
    let session = assign_session(&receivers, "tv", "job", None, None).unwrap();
    let live = serde_json::to_value(session_view(&session)).unwrap();
    assert!(live.get("lost").is_none());
    let empty = serde_json::to_value(SessionView::default()).unwrap();
    assert!(empty.get("lost").is_none());
}

#[test]
fn cast_media_url_requires_a_lan_portal_link() {
    // This PC's own network address; connecting a UDP socket sends nothing.
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let own = probe
        .connect("192.0.2.1:9")
        .ok()
        .and_then(|_| probe.local_addr().ok())
        .map(|addr| addr.ip().to_string());
    if let Some(own) = own.filter(|ip| ip != "0.0.0.0") {
        assert!(cast_media_url(&format!("http://{own}:8490/media/abc/index.m3u8")).is_ok());
        assert!(cast_media_url(&format!("https://{own}/media/abc/seg00000.ts")).is_ok());
    }
    // Another machine's address is not Portal's stream.
    assert!(cast_media_url("http://192.0.2.46:8490/media/abc/index.m3u8").is_err());
    assert!(cast_media_url("http://127.0.0.1:8490/media/abc/index.m3u8").is_err());
    assert!(cast_media_url("http://localhost:8490/media/abc/index.m3u8").is_err());
    assert!(cast_media_url("http://192.0.2.46:8490/api/cast").is_err());
    assert!(cast_media_url("http://user:pass@192.0.2.46/media/a/index.m3u8").is_err());
    assert!(cast_media_url("/media/abc/index.m3u8").is_err());
    assert_eq!(
        cast_thumb_url("http://192.0.2.46:8490/api/image?path=1").as_deref(),
        Some("http://192.0.2.46:8490/api/image?path=1")
    );
    assert!(cast_thumb_url("http://127.0.0.1/poster.jpg").is_none());
    assert!(clean_id("not a device").is_err());
    assert!(clean_id("6b86b273-ff34-4b1a-9b0a-0c0c0c0c0c0c").is_ok());
}

#[test]
fn cast_agent_args_and_device_list() {
    let endpoint = DeviceEndpoint {
        host: "192.0.2.9".into(),
        port: 8009,
        name: "Main  Room".into(),
        model: "Example TV".into(),
    };
    let args = play_args(
        "6b86b273-ff34-4b1a-9b0a-0c0c0c0c0c0c",
        Some(&endpoint),
        "http://192.0.2.46:8490/media/abc/index.m3u8",
        Some("Pilot"),
        None,
        61.25,
        Some(("http://192.0.2.46:8490/media/abc/sub00002.vtt", "en")),
    );
    assert!(args.windows(2).any(|pair| pair
        == [
            "--subtitles",
            "http://192.0.2.46:8490/media/abc/sub00002.vtt"
        ]));
    assert!(args.iter().any(|arg| arg == "--subtitles-lang=en"));
    assert_eq!(args[0], "play");
    assert!(args.windows(2).any(|pair| pair == ["--seconds", "61.250"]));
    assert!(args.windows(2).any(|pair| pair == ["--host", "192.0.2.9"]));
    assert!(args.windows(2).any(|pair| pair == ["--port", "8009"]));
    assert!(args.iter().any(|arg| arg == "--model=Example TV"));
    assert!(args.iter().any(|arg| arg == "--name=Main  Room"));
    assert!(args.iter().any(|arg| arg == "--title=Pilot"));
    // A separate value starting with a hyphen would read as an option.
    let dashed = play_args(
        "tv",
        None,
        "http://192.0.2.46:8490/media/abc/index.m3u8",
        Some("-Pilot"),
        None,
        0.0,
        None,
    );
    assert!(dashed.iter().any(|arg| arg == "--title=-Pilot"));
    assert!(!dashed.iter().any(|arg| arg == "-Pilot"));
    assert!(args.iter().any(|arg| arg.ends_with("index.m3u8")));
    let seek = control_args(
        "seek",
        "6b86b273-ff34-4b1a-9b0a-0c0c0c0c0c0c",
        None,
        Some(12.5),
    );
    assert_eq!(seek.first().map(String::as_str), Some("seek"));
    assert!(seek.windows(2).any(|pair| pair == ["--seconds", "12.500"]));

    let raw = serde_json::json!({
        "ok": true,
        "items": [
            {"id": "6b86b273-ff34-4b1a-9b0a-0c0c0c0c0c0c", "name": "Main  Room", "host": "192.0.2.9", "model": "Example TV", "port": 8009},
            {"id": "../etc", "name": "nope", "host": "192.0.2.8", "model": ""}
        ]
    });
    let rows = devices_from_value(&raw);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Main  Room");
    assert_eq!(rows[0].host, "192.0.2.9");
    assert_eq!(rows[0].model, "Example TV");
    let listed = DeviceList {
        items: vec![DeviceItem {
            id: rows[0].id.clone(),
            name: rows[0].name.clone(),
            host: rows[0].host.clone(),
            model: rows[0].model.clone(),
        }],
    };
    let value = serde_json::to_value(&listed).unwrap();
    assert!(value["items"][0].get("port").is_none());
    assert_eq!(value["items"][0]["model"], "Example TV");

    let parsed = parse_agent_stdout(
            "noise\n{\"ok\":false,\"error\":\"That Cast device is not on the network.\",\"code\":\"not_found\"}\n",
        );
    assert!(parsed.is_some());
    let err = agent_output(
            false,
            "{\"ok\":false,\"error\":\"That Cast device is not on the network.\",\"code\":\"not_found\"}\n",
            "",
        );
    match err {
        Err(AgentFail::NotFound(message)) => {
            assert_eq!(message, "That Cast device is not on the network.");
        }
        other => panic!("expected not found, got a different result: {other:?}"),
    }
    let _ = parsed;
}

fn device_view(device_id: &str) -> DeviceView {
    DeviceView {
        active: true,
        device_id: Some(device_id.into()),
        device_name: Some("Living room".into()),
        job_id: Some("job-1".into()),
        title: Some("Episode 1".into()),
        item: None,
        state: Some(DeviceState::Starting),
        position: Some(0.0),
        duration: Some(1200.0),
        origin: 0.0,
        note: None,
        updated_ms: 1,
        subtitle: None,
        subtitle_job: None,
        subtitle_lang: None,
        subtitle_pending: false,
        volume: None,
        muted: None,
    }
}

#[test]
fn a_cast_subtitle_gets_an_address_timed_to_the_stream() {
    let mut job = portal_api::PlayJob {
        id: "job-1".into(),
        state: portal_api::JobState::Ready,
        error: None,
        session: Some("abc".into()),
        playlist: Some("/media/abc/index.m3u8".into()),
        subtitles: vec![portal_api::SubtitleTrack {
            id: 3,
            label: "English".into(),
            lang: Some("en".into()),
            forced: false,
            url: "/media/abc/sub00003.vtt".into(),
        }],
        subtitles_pending: false,
        next: None,
        browse_path: None,
        title: None,
        position: Some(600.0),
        duration: Some(2700.0),
        release: None,
        audio: Vec::new(),
        audio_index: None,
        prompt: None,
        prompt_known: false,
    };
    let base = "http://192.0.2.10:8490";
    let embedded = CastSubtitle::Embedded {
        id: 3,
        lang: Some("en".into()),
    };
    assert_eq!(
        subtitle_address_in(base, &job, &embedded, None),
        Some((
            "http://192.0.2.10:8490/media/abc/sub00003.vtt".into(),
            "en".into()
        ))
    );
    // A track id from before a seek is found again by language.
    let renumbered = CastSubtitle::Embedded {
        id: 9,
        lang: Some("en".into()),
    };
    assert!(subtitle_address_in(base, &job, &renumbered, None).is_some());
    let online = CastSubtitle::Online {
        id: "gestdown:eae9a04d-0eb3-436f-a7e2-d8b9ad4dd5a0".into(),
        lang: Some("en".into()),
    };
    assert_eq!(
            subtitle_address_in(base, &job, &online, None),
            Some((
                "http://192.0.2.10:8490/api/subtitles/file/gestdown%3Aeae9a04d-0eb3-436f-a7e2-d8b9ad4dd5a0?offset=600.000".into(),
                "en".into()
            ))
        );
    let bogus = CastSubtitle::Online {
        id: "../etc".into(),
        lang: None,
    };
    // A whole-title stream runs on the title's own clock.
    assert_eq!(
        subtitle_address_in(base, &job, &online, Some("whole1")).map(|(address, _)| address),
        subtitle_address_in(base, &job, &online, None).map(|(address, _)| address
            .split("?offset=")
            .next()
            .unwrap()
            .to_string()
            + "?offset=0.000")
    );
    assert!(subtitle_address_in(base, &job, &embedded, Some("whole1"))
        .is_some_and(|(address, _)| address.contains("/media/whole1/sub")));
    assert!(subtitle_address_in(base, &job, &bogus, None).is_none());
    job.subtitles.clear();
    assert!(subtitle_address_in(base, &job, &embedded, None).is_none());
}

#[test]
fn a_device_stop_ends_the_watch_and_later_lines_are_ignored() {
    let mut hub = CastHub::open(&scratch());
    let (first, first_cancel) = hub.begin_device(device_view("tv-1"));
    hub.apply_device_update(
        first,
        Update::Status {
            state: DeviceState::Playing,
            position: Some(61.0),
            duration: Some(1200.0),
            volume: Some(0.4),
            muted: Some(false),
        },
    );
    let view = hub.device_view();
    assert!(view.active);
    assert_eq!(view.state, Some(DeviceState::Playing));
    assert_eq!(view.position, Some(61.0));
    let listed = serde_json::to_value(&view).unwrap();
    assert_eq!(listed["state"], "playing");
    assert_eq!(listed["device_id"], "tv-1");
    assert_eq!(listed["volume"], 0.4);
    assert_eq!(listed["muted"], false);
    assert_eq!(listed["subtitle_pending"], false);
    // A line without the volume keeps the last one; a change shows at once.
    hub.apply_device_update(
        first,
        Update::Status {
            state: DeviceState::Playing,
            position: Some(65.0),
            duration: None,
            volume: None,
            muted: None,
        },
    );
    assert_eq!(hub.device_view().volume, Some(0.4));
    hub.set_device_volume("tv-1", None, Some(true));
    assert_eq!(hub.device_view().muted, Some(true));
    assert_eq!(hub.device_view().volume, Some(0.4));

    // A new cast replaces the watch; the old one's lines no longer count.
    let (second, second_cancel) = hub.begin_device(device_view("tv-1"));
    assert!(*first_cancel.borrow());
    hub.apply_device_update(
        first,
        Update::Ended {
            state: DeviceState::Failed,
            note: "old".into(),
        },
    );
    assert!(hub.device_view().active);

    // Pause shows at once; a stop for another device changes nothing.
    hub.mark_device("tv-1", DeviceState::Paused);
    assert_eq!(hub.device_view().state, Some(DeviceState::Paused));
    hub.end_device("tv-2");
    assert!(!*second_cancel.borrow());
    hub.end_device("tv-1");
    assert!(*second_cancel.borrow());
    let view = hub.device_view();
    assert!(!view.active);
    assert_eq!(view.state, Some(DeviceState::Stopped));
    // Anything the watch still sends after a stop is dropped.
    hub.apply_device_update(
        second,
        Update::Next {
            job_id: "job-2".into(),
            title: "Episode 2".into(),
            item: None,
        },
    );
    assert_eq!(hub.device_view().job_id.as_deref(), Some("job-1"));
    assert_eq!(
        serde_json::to_value(DeviceView::default()).unwrap()["active"],
        false
    );
}

#[test]
fn device_updates_follow_the_next_episode() {
    let mut view = device_view("tv-1");
    apply_update(
        &mut view,
        Update::Next {
            job_id: "job-2".into(),
            title: "Episode 2".into(),
            item: None,
        },
        5,
    );
    assert_eq!(view.job_id.as_deref(), Some("job-2"));
    assert_eq!(view.state, Some(DeviceState::Next));
    assert_eq!(view.position, None);
    assert_eq!(view.updated_ms, 5);
    let replay = CastReplay {
        kind: "episode".into(),
        id: "show-1".into(),
        file: "plugin://plugin.video.example/?ep=2".into(),
        title: "Episode 2".into(),
        season: Some(1),
        episode: Some(2),
        art: None,
        show_title: Some("Example Show".into()),
        show_id: Some("show-1".into()),
    };
    let watched = Watched {
        device: DeviceRef {
            id: "tv-1".into(),
            name: "Living room".into(),
            endpoint: None,
        },
        job_id: "job-2".into(),
        media_url: "http://192.0.2.46:8490/media/b/index.m3u8".into(),
        origin: 0.0,
        job_duration: Some(1300.0),
        item: Some(replay),
        next: None,
        quality: portal_api::Quality::Auto,
    };
    apply_update(&mut view, Update::Casting(Box::new(watched)), 6);
    assert_eq!(view.state, Some(DeviceState::Starting));
    assert_eq!(view.duration, Some(1300.0));
    let listed = serde_json::to_value(&view).unwrap();
    assert_eq!(listed["item"]["show_title"], "Example Show");
    assert_eq!(listed["item"]["episode"], 2);
    apply_update(
        &mut view,
        Update::Ended {
            state: DeviceState::Ended,
            note: "Finished.".into(),
        },
        7,
    );
    assert!(!view.active);
    assert_eq!(view.note.as_deref(), Some("Finished."));
}

#[test]
fn device_play_bodies_and_helpers() {
    let body: DevicePlayBody = serde_json::from_str(
            r#"{"url":"http://192.0.2.46:8490/media/a/index.m3u8","title":"Pilot","job_id":"job-1",
                "kind":"episode","id":"show-1","file":"plugin://plugin.video.example/?ep=1","season":1,"episode":1,
                "show_title":"Example Show","show_id":"show-1","unknown":true}"#,
        )
        .unwrap();
    assert_eq!(body.job_id.as_deref(), Some("job-1"));
    let replay = cast_replay(&body.replay, body.title.as_deref()).unwrap();
    assert_eq!(replay.title, "Pilot");
    assert_eq!(replay.episode, Some(1));
    // An older page sends only the address.
    let old: DevicePlayBody =
        serde_json::from_str(r#"{"url":"http://192.0.2.46:8490/media/a/index.m3u8"}"#).unwrap();
    assert!(old.job_id.is_none());
    assert!(cast_replay(&old.replay, None).is_none());
    // An add-on's link is the title's id in the library; it stays whole.
    let long = format!(
        "plugin://plugin.video.example/?action=play&meta={}",
        "x".repeat(4000)
    );
    let body: DevicePlayBody = serde_json::from_value(serde_json::json!({
        "url": "http://192.0.2.46:8490/media/a/index.m3u8", "title": "Film",
        "kind": "movie", "id": long, "file": long, "show_id": long,
    }))
    .unwrap();
    let replay = cast_replay(&body.replay, body.title.as_deref()).unwrap();
    assert_eq!(replay.id, long);
    assert_eq!(replay.show_id.as_deref(), Some(long.as_str()));

    assert!(same_media_path(
        "http://192.0.2.46:8490/media/a/index.m3u8?x=1",
        "http://192.0.2.46:8490/media/a/index.m3u8"
    ));
    assert!(!same_media_path(
        "http://192.0.2.46:8490/media/a/index.m3u8",
        "http://192.0.2.46:8490/media/b/index.m3u8"
    ));
    assert!(!same_media_path("", ""));
    assert_eq!(
        next_thumb(
            "http://192.0.2.46:8490",
            "image://https%3a%2f%2fart.example.com%2fp.jpg/"
        ),
        Some("https://art.example.com/p.jpg".to_string())
    );
    // A TV does not pair, so a local-only picture is left out.
    assert_eq!(
        next_thumb(
            "http://192.0.2.46:8490",
            "image://C%3a%5cKodi%5caddons%5cplugin.video.example%5cicon.png/"
        ),
        None
    );
    assert_eq!(
        next_thumb("http://192.0.2.46:8490", "http://127.0.0.1/p.jpg"),
        None
    );
    let args = watch_args(&DeviceRef {
        id: "6b86b273-ff34-4b1a-9b0a-0c0c0c0c0c0c".into(),
        name: "Living room".into(),
        endpoint: Some(DeviceEndpoint {
            host: "192.0.2.9".into(),
            port: 8009,
            name: "Living room".into(),
            model: String::new(),
        }),
    });
    assert_eq!(args[0], "watch");
    assert!(args.windows(2).any(|pair| pair == ["--host", "192.0.2.9"]));
    assert!(args.windows(2).any(|pair| pair == ["--interval", "4"]));
}

#[test]
fn cast_agent_script_is_in_the_repo() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/cast/cast_agent.py");
    assert!(path.is_file(), "{}", path.display());
}

#[test]
fn a_scan_that_misses_a_tv_keeps_its_address() {
    let row = |id: &str, host: &str| DeviceRecord {
        id: id.into(),
        name: id.into(),
        host: host.into(),
        model: String::new(),
        port: 8009,
    };
    let mut hub = CastHub::open(&scratch());
    hub.remember_devices(vec![row("den", "192.0.2.9"), row("lounge", "192.0.2.10")]);
    // The next scan misses the den TV and finds the lounge one at a new address.
    hub.remember_devices(vec![row("lounge", "192.0.2.11")]);
    assert_eq!(
        hub.device_endpoint("den").map(|end| end.host),
        Some("192.0.2.9".into())
    );
    assert_eq!(
        hub.device_endpoint("lounge").map(|end| end.host),
        Some("192.0.2.11".into())
    );
    assert_eq!(hub.devices.len(), 2);
    hub.remember_devices(
        (0..DEVICE_LIMIT + 5)
            .map(|n| row(&format!("tv{n}"), "192.0.2.20"))
            .collect(),
    );
    hub.remember_devices(Vec::new());
    assert!(hub.devices.len() <= DEVICE_LIMIT + 5);
    hub.remember_devices(vec![row("new", "192.0.2.30")]);
    assert_eq!(hub.devices.len(), DEVICE_LIMIT);
}

#[test]
fn an_hls_address_names_its_session() {
    let raw = "http://192.0.2.46:8490/media/abc/index.m3u8?x=1";
    assert!(url_targets_m3u8(raw));
    assert!(url_targets_m3u8(
        "https://192.0.2.3/media/sess/INDEX.M3U8#t=1"
    ));
    assert!(!url_targets_m3u8(
        "http://192.0.2.46:8490/media/abc/seg00000.ts"
    ));
    assert_eq!(media_url_parts(raw).unwrap().session, "abc");
    let https = media_url_parts("https://192.0.2.3/media/sess/index.m3u8").unwrap();
    assert_eq!(https.session, "sess");
    let v6 = media_url_parts("http://[2001:db8::1]:8490/media/abc/index.m3u8").unwrap();
    assert_eq!(v6.session, "abc");
    assert!(media_url_parts("http://192.0.2.46:8490/media/abc/index.m3u8/extra").is_none());
    assert!(media_url_parts("http:///media/abc/index.m3u8").is_none());
}

#[test]
fn a_device_seek_stays_inside_the_transcoded_part() {
    let growing = "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXTINF:4.004,\nseg00000.ts\n#EXTINF:3.96,\nseg00001.ts\n#EXTINF:4.0,\nseg00002.ts\n";
    assert!(playlist_reaches(growing, 0.0));
    assert!(playlist_reaches(growing, 3.9));
    assert!(!playlist_reaches(growing, 5.0));
    assert!(!playlist_reaches(growing, 600.0));
    let done = format!("{growing}#EXT-X-ENDLIST\n");
    assert!(playlist_reaches(&done, 600.0));
    assert_eq!(
        playlist_session("/media/abc123/index.m3u8").as_deref(),
        Some("abc123")
    );
    assert_eq!(
        playlist_session("/media/abc123/index.m3u8?x=1").as_deref(),
        Some("abc123")
    );
    assert_eq!(playlist_session("/media/abc123/seg00001.ts"), None);
    assert_eq!(playlist_session("/other/abc123/index.m3u8"), None);
}

#[tokio::test]
async fn prepare_cast_media_rewrites_a_finished_playlist_to_its_mp4() {
    let dir = scratch();
    let root = dir.join("hls");
    let session = root.join("abc");
    std::fs::create_dir_all(&session).unwrap();
    let playlist = session.join("index.m3u8");
    std::fs::write(
        &playlist,
        b"#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXTINF:2.0,\nseg00000.ts\n#EXT-X-ENDLIST\n",
    )
    .unwrap();
    let mp4 = session.join("cast.mp4");
    std::fs::write(&mp4, vec![7u8; 1200]).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&playlist)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&mp4)
        .unwrap()
        .set_modified(std::time::SystemTime::now())
        .unwrap();
    let player = portal_engine::Player::new("ffmpeg", "ffprobe", &root);
    let url = "http://192.0.2.46:8490/media/abc/index.m3u8?x=1";
    let cast = prepare_cast_media(&player, url).await.unwrap();
    assert!(cast.contains("/media/abc/cast.mp4?v="), "{cast}");
    assert!(!cast.contains("m3u8"));
    let same = prepare_cast_media(&player, "http://192.0.2.46:8490/media/abc/seg00000.ts")
        .await
        .unwrap();
    assert!(same.ends_with("seg00000.ts"));
    let missing =
        prepare_cast_media(&player, "http://192.0.2.46:8490/media/missing01/index.m3u8").await;
    assert!(missing.is_err());
    std::fs::write(&playlist, b"#EXTM3U\n").unwrap();
    assert!(prepare_cast_media(&player, url).await.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn prepare_cast_media_reuses_a_growing_playlist_span() {
    let dir = scratch();
    let root = dir.join("hls");
    let session = root.join("abc");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
            session.join("index.m3u8"),
            b"#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXTINF:4.0,\nseg00000.ts\n#EXTINF:4.0,\nseg00001.ts\n",
        )
        .unwrap();
    std::fs::write(session.join("cast.mp4"), vec![1u8; 1500]).unwrap();
    std::fs::write(session.join("cast.span"), b"8.000\n").unwrap();
    let player = portal_engine::Player::new("ffmpeg", "ffprobe", &root);
    let cast = prepare_cast_media(&player, "http://192.0.2.46:8490/media/abc/index.m3u8")
        .await
        .unwrap();
    assert!(cast.contains("/media/abc/cast.mp4?v=8.000"), "{cast}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cast_remux_args_reencode_audio_into_an_mp4() {
    let playlist = PathBuf::from("index.m3u8");
    let dest = PathBuf::from("cast.mp4.part");
    let copy = remux_copy_args(&playlist, &dest);
    assert!(copy.windows(2).any(|pair| pair == ["-c:v", "copy"]));
    assert!(copy.windows(2).any(|pair| pair == ["-c:a", "aac"]));
    assert!(copy.windows(2).any(|pair| pair == ["-b:a", "192k"]));
    assert!(copy.windows(2).any(|pair| pair == ["-ar", "48000"]));
    assert!(copy.windows(2).any(|pair| pair == ["-ac", "2"]));
    assert!(copy.windows(2).any(|pair| pair == ["-f", "mp4"]));
    assert!(copy.iter().any(|arg| arg == "+faststart"));
    assert_eq!(
        copy.last().map(String::as_str),
        Some(dest.to_str().unwrap())
    );
    let encode = remux_encode_args(&playlist, &dest);
    assert!(encode.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
    assert!(encode
        .windows(2)
        .any(|pair| pair == ["-preset", "veryfast"]));
    assert!(encode.windows(2).any(|pair| pair == ["-crf", "21"]));
    assert!(encode
        .windows(2)
        .any(|pair| pair == ["-pix_fmt", "yuv420p"]));
    assert!(encode.windows(2).any(|pair| pair == ["-c:a", "aac"]));
    assert!(encode.windows(2).any(|pair| pair == ["-ar", "48000"]));
    assert!(encode.windows(2).any(|pair| pair == ["-b:a", "192k"]));
    assert!(encode.windows(2).any(|pair| pair == ["-f", "mp4"]));
    assert!(encode.iter().any(|arg| arg == "+faststart"));
    assert_eq!(
        encode.last().map(String::as_str),
        Some(dest.to_str().unwrap())
    );
}

#[test]
fn a_growing_playlist_snapshot_closes_and_a_short_file_continues() {
    let event =
        "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXTINF:2.0,\nseg00000.ts\n";
    let vod = vod_snapshot(event);
    assert!(vod.contains("#EXT-X-PLAYLIST-TYPE:VOD\n"));
    assert!(vod.trim_end().ends_with("#EXT-X-ENDLIST"));
    assert!(!vod.contains("EVENT"));
    assert_eq!(playlist_coverage(&vod), 2.0);
    assert!(playlist_ended(&vod));
    assert_eq!(extend_choice(100.0, 40.0, false), ExtendChoice::Extend);
    assert_eq!(extend_choice(50.0, 40.0, false), ExtendChoice::Wait);
    assert_eq!(extend_choice(100.0, 90.0, true), ExtendChoice::Finish);
    assert_eq!(extend_choice(200.0, 40.0, true), ExtendChoice::Extend);
    assert_eq!(extend_choice(10.0, 0.2, false), ExtendChoice::Finish);
    let dir = scratch();
    std::fs::create_dir_all(&dir).unwrap();
    let mp4 = dir.join("cast.mp4");
    let span = dir.join("cast.span");
    let playlist = dir.join("index.m3u8");
    std::fs::write(&mp4, vec![1u8; 1500]).unwrap();
    std::fs::write(&span, b"8.000\n").unwrap();
    std::fs::write(&playlist, b"#EXTM3U\n").unwrap();
    assert!(cast_mp4_fresh(&mp4, &span, &playlist, 9.0, false));
    assert!(!cast_mp4_fresh(&mp4, &span, &playlist, 40.0, false));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cast_file_address_carries_its_span_and_bounds_the_start() {
    let url = "http://192.0.2.46:8490/media/abc/cast.mp4?v=120.500";
    assert_eq!(url_span(url), Some(120.5));
    assert_eq!(url_span("http://192.0.2.46:8490/media/abc/cast.mp4"), None);
    assert_eq!(
        url_span("http://192.0.2.46:8490/media/abc/cast.mp4?v=nan"),
        None
    );
    assert_eq!(
        url_span("http://192.0.2.46:8490/media/abc/cast.mp4?x=1&v=9#t"),
        Some(9.0)
    );
    assert_eq!(start_within(url, 60.0), 60.0);
    assert_eq!(start_within(url, 500.0), 120.0);
    assert_eq!(start_within(url, -4.0), 0.0);
    assert_eq!(start_within(url, f64::NAN), 0.0);
    assert_eq!(
        start_within("http://192.0.2.46:8490/media/abc/index.m3u8", 500.0),
        500.0
    );
    let longer = versioned_media(url, 300.0);
    assert_eq!(url_span(&longer), Some(300.0));
    assert!(same_media_path(url, &longer));
}

#[test]
fn a_seek_past_the_cast_file_is_not_sent_to_the_tv() {
    let dir = scratch();
    let root = dir.join("hls");
    let player = portal_engine::Player::new("ffmpeg", "ffprobe", &root);
    // No job, so no cast file: nothing holds the seek back.
    assert!(cast_file_reaches(&player, "missing", 900.0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_scan_answers_the_tabs_that_waited_on_it() {
    let failed: Discovery = Err(AgentFail::Timeout);
    assert!(discovery_reusable(&failed, Duration::from_secs(1)));
    assert!(!discovery_reusable(&failed, DISCOVERY_FAIL_REUSE));
    let found: Discovery = Ok(serde_json::json!({"devices": []}));
    assert!(discovery_reusable(&found, Duration::from_secs(9)));
    assert!(!discovery_reusable(&found, DISCOVERY_REUSE));
}

#[test]
fn a_receiver_gets_a_short_language_tag() {
    assert_eq!(receiver_lang(Some("eng")), "en");
    assert_eq!(receiver_lang(Some("ger")), "de");
    assert_eq!(receiver_lang(Some("pt-BR")), "pt-BR");
    assert_eq!(receiver_lang(Some("pob")), "pt-BR");
    assert_eq!(receiver_lang(Some("GA")), "ga");
    assert_eq!(receiver_lang(Some("tlh")), "und");
    assert_eq!(receiver_lang(Some("und")), "und");
    assert_eq!(receiver_lang(Some(" ")), "und");
    assert_eq!(receiver_lang(None), "und");
}

#[test]
fn an_untagged_pick_keeps_a_language_for_the_next_episode() {
    let track = |id, lang: Option<&str>| portal_api::SubtitleTrack {
        id,
        label: "Track".into(),
        lang: lang.map(str::to_string),
        forced: false,
        url: format!("/media/abc/sub{id:05}.vtt"),
    };
    let tracks = [track(1, Some("fre")), track(2, None), track(3, Some("und"))];
    let embedded = |id, lang: Option<&str>| CastSubtitle::Embedded {
        id,
        lang: lang.map(str::to_string),
    };
    let preferred = || Some("es".to_string());
    assert_eq!(
        pick_lang(&embedded(1, Some("en")), &tracks, preferred()).as_deref(),
        Some("en")
    );
    // The dashboard sent no language: the track's tag, else the settings' first.
    assert_eq!(
        pick_lang(&embedded(1, None), &tracks, preferred()).as_deref(),
        Some("fre")
    );
    assert_eq!(
        pick_lang(&embedded(2, None), &tracks, preferred()).as_deref(),
        Some("es")
    );
    assert_eq!(
        pick_lang(&embedded(3, Some("und")), &tracks, preferred()).as_deref(),
        Some("es")
    );
    let online = CastSubtitle::Online {
        id: "gestdown:eae9a04d-0eb3-436f-a7e2-d8b9ad4dd5a0".into(),
        lang: None,
    };
    assert_eq!(pick_lang(&online, &tracks, None), None);
}

#[test]
fn the_wanted_subtitle_language_lasts_until_subtitles_are_turned_off() {
    let mut hub = CastHub::open(&scratch());
    hub.begin_device(device_view("tv-1"));
    let english = CastSubtitle::Embedded {
        id: 3,
        lang: Some("en".into()),
    };
    hub.set_device_subtitle("tv-1", Some(english.clone()), Some("job-1".into()), None);
    assert_eq!(hub.device_view().subtitle_lang.as_deref(), Some("en"));
    // An untagged pick keeps the language wanted before.
    let untagged = CastSubtitle::Embedded { id: 4, lang: None };
    hub.set_device_subtitle("tv-1", Some(untagged.clone()), Some("job-1".into()), None);
    assert_eq!(hub.device_view().subtitle_lang.as_deref(), Some("en"));
    hub.set_device_subtitle(
        "tv-1",
        Some(untagged),
        Some("job-1".into()),
        Some("fr".into()),
    );
    assert_eq!(hub.device_view().subtitle_lang.as_deref(), Some("fr"));

    // While one is on its way the page is told so; the language stays.
    hub.pend_device_subtitle("tv-1", "job-1", None);
    let view = hub.device_view();
    assert!(view.subtitle_pending);
    assert_eq!(view.subtitle, None);
    assert_eq!(view.subtitle_lang.as_deref(), Some("fr"));
    assert_eq!(
        serde_json::to_value(&view).unwrap()["subtitle_pending"],
        true
    );
    // An earlier pick giving up says nothing for a later one.
    let first = hub.ask_subtitle();
    hub.pend_device_subtitle("tv-1", "job-1", None);
    let second = hub.ask_subtitle();
    hub.pend_device_subtitle("tv-1", "job-1", None);
    assert_eq!(hub.latest_subtitle_ask(), second);
    hub.settle_device_subtitle("tv-1", "job-1", first);
    assert!(hub.device_view().subtitle_pending);
    hub.settle_device_subtitle("tv-1", "job-2", second);
    assert!(hub.device_view().subtitle_pending);
    hub.settle_device_subtitle("tv-1", "job-1", second);
    assert!(!hub.device_view().subtitle_pending);
    assert_eq!(hub.device_view().subtitle_lang.as_deref(), Some("fr"));

    // A subtitle shown ends the wait; a carried state comes back whole.
    hub.pend_device_subtitle("tv-1", "job-1", None);
    hub.set_device_subtitle("tv-1", Some(english.clone()), Some("job-1".into()), None);
    assert!(!hub.device_view().subtitle_pending);
    let carried = hub.device_subtitle_state();
    hub.begin_device(device_view("tv-1"));
    hub.restore_device_subtitle("tv-1", carried, "job-1");
    assert_eq!(hub.device_view().subtitle, Some(english));
    assert_eq!(hub.device_view().subtitle_lang.as_deref(), Some("en"));

    // Only turning them off forgets the language.
    hub.set_device_subtitle("tv-1", None, Some("job-1".into()), Some("de".into()));
    let view = hub.device_view();
    assert_eq!(view.subtitle, None);
    assert_eq!(view.subtitle_lang, None);
}

#[test]
fn a_tv_volume_is_checked_and_sent_to_the_helper() {
    assert_eq!(volume_level(None), Some(None));
    assert_eq!(volume_level(Some(0.25)), Some(Some(0.25)));
    assert_eq!(volume_level(Some(1.5)), Some(Some(1.0)));
    assert_eq!(volume_level(Some(-0.2)), Some(Some(0.0)));
    assert_eq!(volume_level(Some(f64::NAN)), None);
    assert_eq!(volume_level(Some(f64::INFINITY)), None);
    let body: DeviceVolumeBody = serde_json::from_str(r#"{"level":0.5}"#).unwrap();
    assert_eq!((body.level, body.muted), (Some(0.5), None));
    let body: DeviceVolumeBody = serde_json::from_str(r#"{"muted":true}"#).unwrap();
    assert_eq!((body.level, body.muted), (None, Some(true)));
    let endpoint = DeviceEndpoint {
        host: "192.0.2.46".into(),
        port: 8009,
        name: "Living room".into(),
        model: String::new(),
    };
    assert_eq!(
        volume_args("tv-1", Some(&endpoint), Some(0.5), Some(false)),
        vec![
            "volume",
            "--id",
            "tv-1",
            "--host",
            "192.0.2.46",
            "--port",
            "8009",
            "--name=Living room",
            "--level=0.500",
            "--muted=false",
            "--timeout",
            "20",
        ]
    );
    assert_eq!(
        volume_args("tv-1", None, None, Some(true)),
        vec!["volume", "--id", "tv-1", "--muted=true", "--timeout", "20"]
    );
}

#[test]
fn the_next_cast_file_is_made_shortly_before_the_end() {
    assert_eq!(stage_lead(52.0), 30.0);
    assert_eq!(stage_lead(1000.0), 150.0);
    assert_eq!(stage_lead(7200.0), 240.0);
    // Far from the end, nothing is made yet.
    assert!(!stage_due(300.0, 100.0, 900.0, false, None));
    // Near the end with a real tail waiting.
    assert!(stage_due(300.0, 275.0, 900.0, false, None));
    // The stream holds too little past this file.
    assert!(!stage_due(300.0, 275.0, 310.0, false, None));
    // The title ends here.
    assert!(!stage_due(300.0, 290.0, 305.0, true, None));
    // One is already made for this file.
    assert!(!stage_due(300.0, 290.0, 900.0, false, Some(700.0)));
    // One left from the file before is not.
    assert!(stage_due(300.0, 290.0, 900.0, false, Some(300.0)));
}

#[tokio::test]
async fn a_file_made_ahead_becomes_the_cast_file() {
    let dir = scratch();
    std::fs::create_dir_all(&dir).unwrap();
    let playlist = dir.join("index.m3u8");
    let mut text = String::from("#EXTM3U\n");
    for n in 0..30 {
        text.push_str(&format!("#EXTINF:10.0,\nseg{n}.ts\n"));
    }
    std::fs::write(&playlist, text).unwrap();
    std::fs::write(dir.join("cast.mp4"), vec![1u8; 2000]).unwrap();
    std::fs::write(dir.join("cast.span"), "50.000\n").unwrap();
    // Nothing made ahead.
    assert_eq!(take_staged(&playlist, 50.0).await, None);
    std::fs::write(dir.join(STAGED_FILE), vec![2u8; 3000]).unwrap();
    std::fs::write(dir.join(STAGED_SPAN), "200.000\n").unwrap();
    assert_eq!(take_staged(&playlist, 50.0).await, Some(200.0));
    assert_eq!(std::fs::read(dir.join("cast.mp4")).unwrap().len(), 3000);
    assert_eq!(read_span(&dir.join("cast.span")), Some(200.0));
    assert!(!dir.join(STAGED_FILE).exists() && !dir.join(STAGED_SPAN).exists());
    // One that adds nothing past what played is thrown away.
    std::fs::write(dir.join(STAGED_FILE), vec![2u8; 3000]).unwrap();
    std::fs::write(dir.join(STAGED_SPAN), "203.000\n").unwrap();
    assert_eq!(take_staged(&playlist, 200.0).await, None);
    assert!(!dir.join(STAGED_FILE).exists());
    // So is one longer than the stream, left from a stream since replaced.
    std::fs::write(dir.join(STAGED_FILE), vec![2u8; 3000]).unwrap();
    std::fs::write(dir.join(STAGED_SPAN), "900.000\n").unwrap();
    assert_eq!(take_staged(&playlist, 200.0).await, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_whole_title_playlist_is_told_from_other_media() {
    assert!(is_cast_playlist(
        "http://192.0.2.46:8490/media/abc/cast.m3u8"
    ));
    assert!(is_cast_playlist(
        "http://192.0.2.46:8490/media/abc/cast.m3u8?x=1"
    ));
    assert!(!is_cast_playlist(
        "http://192.0.2.46:8490/media/abc/index.m3u8"
    ));
    assert!(!is_cast_playlist(
        "http://192.0.2.46:8490/media/abc/cast.mp4?v=52.000"
    ));
    assert!(!is_cast_file("http://192.0.2.46:8490/media/abc/cast.m3u8"));
}

#[tokio::test]
async fn a_job_with_no_whole_title_stream_is_cast_as_a_file() {
    let root = scratch();
    let player = portal_engine::Player::new("ffmpeg", "ffprobe", &root);
    // Not a Portal playlist: handed on as it is, at the start asked for.
    let url = "http://192.0.2.46:8490/media/abc/seg00000.ts";
    let (same, start) = cast_target(&player, url, None, 12.0).await.unwrap();
    assert_eq!((same.as_str(), start), (url, 12.0));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn seek_answer_wait_has_a_default_and_a_setter() {
    assert_eq!(SEEK_ANSWER_WAIT, Duration::from_secs(20));
    set_seek_answer_wait(Duration::from_secs(90));
    assert_eq!(seek_answer_wait(), Duration::from_secs(90));
    // No cap: a long wait is kept as given.
    set_seek_answer_wait(Duration::from_secs(3600));
    assert_eq!(seek_answer_wait(), Duration::from_secs(3600));
    set_seek_answer_wait(SEEK_ANSWER_WAIT);
}
