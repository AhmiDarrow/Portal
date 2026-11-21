use super::*;
use serde_json::json;

fn shows() -> Value {
    json!({"shows": [
        {"id": "11111111-1111-4111-8111-111111111111", "name": "Network Some Show", "tvDbId": 101, "tmdbId": 201},
        {"id": "22222222-2222-4222-8222-222222222222", "name": "Some Show", "tvDbId": 102, "tmdbId": 202},
        {"id": "33333333-3333-4333-8333-333333333333", "name": "Some Show: The Animation", "tvDbId": 103, "tmdbId": 203}
    ]})
}

const SOME_SHOW: &str = "22222222-2222-4222-8222-222222222222";

#[test]
fn shows_are_picked_by_id_then_exact_name() {
    let by_name = ShowHints {
        name: "Some Show".into(),
        ..ShowHints::default()
    };
    assert_eq!(pick_show(&shows(), &by_name).as_deref(), Some(SOME_SHOW));
    let by_tvdb = ShowHints {
        name: "Something else".into(),
        tvdb: vec![102],
        tmdb: vec![],
    };
    assert_eq!(pick_show(&shows(), &by_tvdb).as_deref(), Some(SOME_SHOW));
    let by_tmdb = ShowHints {
        name: String::new(),
        tvdb: vec![5],
        tmdb: vec![201],
    };
    assert_eq!(
        pick_show(&shows(), &by_tmdb).as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    // An episode's own id matches nothing, so the name decides.
    let episode_ids = ShowHints {
        name: "some show".into(),
        tvdb: vec![999],
        tmdb: vec![],
    };
    assert_eq!(
        pick_show(&shows(), &episode_ids).as_deref(),
        Some(SOME_SHOW)
    );
    let partial = ShowHints {
        name: "Some Sh".into(),
        ..ShowHints::default()
    };
    assert_eq!(pick_show(&shows(), &partial), None);
    let twins =
        json!({"shows": [{"id": "a", "name": "Twin Show"}, {"id": "b", "name": "Twin Show"}]});
    let twin = ShowHints {
        name: "Twin Show".into(),
        ..ShowHints::default()
    };
    assert_eq!(pick_show(&twins, &twin), None);
    assert_eq!(pick_show(&Value::Null, &twin), None);
}

#[test]
fn only_finished_subtitles_are_listed() {
    let answer = json!({"matchingSubtitles": [
            {"subtitleId": "44444444-4444-4444-8444-444444444444", "version": "DVDRip.GRPA", "completed": true,
             "hearingImpaired": false, "downloadCount": 20, "language": "English", "qualities": []},
            {"subtitleId": "55555555-5555-4555-8555-555555555555", "version": "720p.BluRay.GRPB", "completed": true,
             "hearingImpaired": true, "downloadCount": 30, "qualities": ["720p", "1080p"]},
            {"subtitleId": "00000000-0000-0000-0000-000000000000", "version": "WIP", "completed": false},
            {"subtitleId": "../../etc", "version": "bad", "completed": true}
        ], "episode": {"season": 1, "number": 1}});
    let found = parse_episode(&answer, "en", (1, 1));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id, "gestdown:44444444-4444-4444-8444-444444444444");
    assert_eq!(found[0].release, "DVDRip.GRPA");
    assert_eq!(found[0].downloads, Some(20));
    assert_eq!(found[1].release, "720p.BluRay.GRPB (720p, 1080p)");
    assert!(found[1].hearing_impaired);
    assert_eq!(found[1].episode, Some((1, 1)));
    assert!(parse_episode(&json!({}), "en", (1, 1)).is_empty());
}
