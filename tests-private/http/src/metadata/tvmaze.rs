use super::*;
use serde_json::json;

fn show() -> Value {
    json!({
        "id": 169,
        "url": "https://www.tvmaze.com/shows/169/breaking-bad",
        "name": "Breaking Bad",
        "type": "Scripted",
        "genres": ["Drama", "Crime", "Thriller"],
        "runtime": 60,
        "averageRuntime": 60,
        "premiered": "2008-01-20",
        "rating": {"average": 9.2},
        "network": {"id": 20, "name": "AMC", "country": {"name": "United States", "code": "US"}},
        "webChannel": null,
        "externals": {"tvrage": 18164, "thetvdb": 81189, "imdb": "tt0903747"},
        "image": {"medium": "https://static.tvmaze.com/m.jpg", "original": "https://static.tvmaze.com/o.jpg"},
        "summary": "<p><b>Breaking Bad</b> follows protagonist Walter White &amp; his partner.</p>",
        "_embedded": {"cast": [
            {"person": {"name": "Bryan Cranston", "image": {"medium": "https://static.tvmaze.com/bc.jpg"}},
             "character": {"name": "Walter White"}},
            {"person": {"name": "Aaron Paul", "image": null}, "character": {"name": "Jesse Pinkman"}},
            {"person": {"name": ""}, "character": {"name": "Nobody"}}
        ]}
    })
}

#[test]
fn a_show_parses() {
    let info = parse_show(&show());
    assert_eq!(info.kind, Some(MediaKind::Tv));
    assert_eq!(info.title.as_deref(), Some("Breaking Bad"));
    assert_eq!(info.year, Some(2008));
    assert_eq!(
        info.plot.as_deref(),
        Some("Breaking Bad follows protagonist Walter White & his partner.")
    );
    assert_eq!(info.genres, vec!["Drama", "Crime", "Thriller"]);
    assert_eq!(info.runtime_minutes, Some(60));
    assert_eq!(info.rating, Some(9.2));
    assert_eq!(info.studios, vec!["AMC"]);
    assert_eq!(info.countries, vec!["United States"]);
    assert_eq!(info.cast.len(), 2);
    assert_eq!(
        info.cast[0].thumb.as_deref(),
        Some("https://static.tvmaze.com/bc.jpg")
    );
    assert_eq!(info.cast[1].role.as_deref(), Some("Jesse Pinkman"));
    assert_eq!(info.imdb_id.as_deref(), Some("tt0903747"));
    assert_eq!(info.tvdb_id.as_deref(), Some("81189"));
    assert_eq!(
        info.poster.as_deref(),
        Some("https://static.tvmaze.com/o.jpg")
    );
    assert_eq!(info.sources, vec![SOURCE_TVMAZE]);
    assert_eq!(
        info.links[0].url,
        "https://www.tvmaze.com/shows/169/breaking-bad"
    );
}

#[test]
fn an_episode_parses_with_its_show() {
    let episode = json!({
        "id": 12192,
        "url": "https://www.tvmaze.com/episodes/12192/breaking-bad-1x01-pilot",
        "name": "Pilot",
        "season": 1,
        "number": 1,
        "airdate": "2008-01-20",
        "runtime": 60,
        "rating": {"average": 8.4},
        "image": {"original": "https://static.tvmaze.com/ep.jpg"},
        "summary": "<p>A chemistry teacher...</p>"
    });
    let info = parse_episode(&show(), &episode);
    assert_eq!(info.kind, Some(MediaKind::Episode));
    assert_eq!(info.title.as_deref(), Some("Pilot"));
    assert_eq!(info.show_title.as_deref(), Some("Breaking Bad"));
    assert_eq!((info.season, info.episode), (Some(1), Some(1)));
    assert_eq!(info.plot.as_deref(), Some("A chemistry teacher..."));
    assert_eq!(info.rating, Some(8.4));
    assert_eq!(
        info.fanart.as_deref(),
        Some("https://static.tvmaze.com/ep.jpg")
    );
    assert_eq!(
        info.poster.as_deref(),
        Some("https://static.tvmaze.com/o.jpg")
    );
    // The show's ids are not the episode's.
    assert_eq!(info.imdb_id, None);
    assert_eq!(info.tvdb_id, None);
    assert_eq!(info.links.len(), 2);
    assert_eq!(info.links[1].label, "TVmaze show");

    let facts = show_facts_for_episode(parse_show(&show()));
    assert_eq!(facts.plot, None);
    assert_eq!(facts.show_title.as_deref(), Some("Breaking Bad"));
}

#[test]
fn a_search_hit_must_match_name_and_year() {
    assert_eq!(
        accept_search(&show(), "breaking bad", Some(2008)),
        Some(169)
    );
    assert_eq!(accept_search(&show(), "Breaking Bad", None), Some(169));
    assert_eq!(accept_search(&show(), "Breaking Bad", Some(2019)), None);
    assert_eq!(accept_search(&show(), "Breaking", None), None);
    assert_eq!(accept_search(&json!({"id": 1}), "x", None), None);
}
