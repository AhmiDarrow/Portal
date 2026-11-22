//! The wire types moved out of `crates/http` write exactly the JSON the server wrote before:
//! the same field names, order and nulls. "Recorded" fixtures are answers of Portal 0.3.10
//! (before the move) with personal values made neutral; the others are built field for field
//! from the old server code, where a live answer would have needed a side effect.

use portal_api::*;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// `json` reads as `T` and writes back byte for byte.
fn same<T: DeserializeOwned + Serialize>(json: &str) {
    let value: T = serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
    assert_eq!(
        serde_json::to_string(&value).unwrap(),
        json,
        "{}",
        std::any::type_name::<T>()
    );
}

#[test]
fn recorded_answers_round_trip() {
    same::<Ping>(r#"{"app":"portal","pid":19852,"version":"0.3.10"}"#);
    same::<PairMe>(r#"{"paired":true,"this_pc":true}"#);
    same::<PairStatus>(r#"{"state":"expired"}"#);
    same::<AppView>(
        r#"{"tray":true,"autostart":false,"encoder":{"hardware":true,"card":"nvidia"}}"#,
    );
    same::<EncoderView>(r#"{"hardware":true,"card":"nvidia"}"#);
    same::<CastBody>(
        r#"{"lan":true,"bind":"0.0.0.0:8490","url":"http://192.0.2.10:8490","receivers":[],"session":null}"#,
    );
    same::<CastDeviceView>(
        r#"{"active":false,"device_id":null,"device_name":null,"job_id":null,"title":null,"item":null,"state":null,"position":null,"duration":null,"origin":0.0,"note":null,"updated_ms":0,"subtitle_pending":false}"#,
    );
    same::<PrefsBody>(
        r#"{"quality":"1080","subtitles":"off","subtitle_language":null,"audio_language":"en","autoplay":true,"stored":true}"#,
    );
    same::<Vec<PairedDevice>>("[]");
    same::<ErrorBody>(r#"{"detail":"That device is not paired."}"#);
}

#[test]
fn built_answers_round_trip() {
    same::<ErrorBody>(r#"{"detail":"Pair this device with the PC first.","pair":true}"#);
    same::<PairStatus>(r#"{"state":"waiting"}"#);
    same::<PairStatus>(r#"{"state":"paired"}"#);
    same::<PairStarted>(r#"{"code":"123456","ticket":"t0","expires_in":600}"#);
    same::<PairApproved>(r#"{"name":"Firefox on Android","ok":true}"#);
    same::<Vec<PairedDevice>>(
        r#"[{"id":"d1","name":"Phone","paired_at":1700000000,"last_seen":1700000100,"address":"192.0.2.20"}]"#,
    );
    same::<AppView>(r#"{"tray":false,"encoder":{"hardware":false,"card":"cpu"}}"#);
    same::<IncomingFile>(r#"{"path":"C:\\Portal\\incoming\\addon-file.zip"}"#);
    same::<ActionBody>(r#"{"ok":true}"#);
    same::<ActionBody>(r#"{"ok":true,"browse_path":"plugin://plugin.video.example/","ran":true}"#);
    same::<ActionBody>(r#"{"ok":true,"settings_addon":"plugin.video.example"}"#);
    same::<CastDeviceView>(
        r#"{"active":true,"device_id":"tv1","device_name":"Living room","job_id":"j1","title":"Example","item":{"kind":"episode","id":"show1","file":"plugin://plugin.video.example/?e=1","title":"Example","season":1,"episode":2,"art":"https://example.com/a.jpg","show_title":"Example Show","show_id":"show1"},"state":"playing","position":12.5,"duration":1800.0,"origin":0.0,"note":null,"updated_ms":1700000000000,"subtitle":{"kind":"online","id":"sub1","lang":"en"},"subtitle_pending":false,"volume":0.5,"muted":false}"#,
    );
    same::<CastDeviceList>(
        r#"{"items":[{"id":"tv1","name":"Living room","host":"192.0.2.30","model":"Chromecast"}]}"#,
    );
    same::<CastReceiverList>(
        r#"{"items":[{"id":"r1","name":"TV browser","seen_ms":1700000000000}]}"#,
    );
    same::<CastSessionView>(
        r#"{"receiver_id":null,"job_id":null,"title":null,"lost":{"receiver_id":"r1","job_id":"j1","place":{"position":30.0,"duration":60.0,"paused":true,"updated_ms":5}}}"#,
    );
    same::<CastSessionView>(
        r#"{"receiver_id":"r1","job_id":"j1","title":"Example","kind":"movie","id":"m1","file":"plugin://x","command":{"seq":3,"action":"seek","seconds":40.0}}"#,
    );
    same::<CastBody>(
        r#"{"lan":false,"bind":"127.0.0.1:8490","url":null,"receivers":[{"id":"r1","name":"TV","seen_ms":1}],"session":{"receiver_id":null,"job_id":null,"title":null},"restart_needed":true}"#,
    );
}

#[test]
fn request_bodies_read_as_the_server_read_them() {
    let put: PrefsPut =
        serde_json::from_str(r#"{"subtitle_language":null,"autoplay":false}"#).unwrap();
    assert_eq!(put.subtitle_language, Some(None));
    assert_eq!(put.audio_language, None);
    assert_eq!(
        serde_json::to_string(&put).unwrap(),
        r#"{"subtitle_language":null,"autoplay":false}"#
    );

    let rename: HistoryName = serde_json::from_str(r#"{"name":"A","id":"t1","season":2}"#).unwrap();
    assert_eq!(
        rename.title,
        HistoryTitle {
            id: Some("t1".into()),
            season: Some(2),
            episode: None
        }
    );
    assert_eq!(
        serde_json::to_string(&rename).unwrap(),
        r#"{"name":"A","id":"t1","season":2}"#
    );

    let play: CastPlayBody =
        serde_json::from_str(r#"{"url":"http://x/m.m3u8","job_id":"j1","subtitle":{"kind":"embedded","id":2},"kind":"movie","id":"m1"}"#)
            .unwrap();
    assert_eq!(play.replay.kind.as_deref(), Some("movie"));
    assert_eq!(play.subtitle.as_ref().and_then(CastSubtitle::lang), None);

    let meta = MetadataPut {
        online: None,
        tmdb_key: Some(None),
    };
    assert_eq!(
        serde_json::to_string(&meta).unwrap(),
        r#"{"tmdb_key":null}"#
    );
    assert_eq!(
        serde_json::to_string(&OnBody { on: true }).unwrap(),
        r#"{"on":true}"#
    );
    assert_eq!(
        serde_json::to_string(&SeekBody { seconds: 5.0 }).unwrap(),
        r#"{"seconds":5.0}"#
    );
    assert_eq!(
        serde_json::to_string(&PromptAnswer {
            id: "p".into(),
            action: PromptAction::Submit,
            text: Some("x".into()),
            index: None
        })
        .unwrap(),
        r#"{"id":"p","action":"submit","text":"x"}"#
    );
}
