use super::*;
use serde_json::json;

#[test]
fn wikidata_claims_give_the_imdb_id() {
    let claims = json!({"claims": {"P345": [
        {"mainsnak": {"datavalue": {"value": "not an id"}}},
        {"mainsnak": {"datavalue": {"value": "tt0111161", "type": "string"}}}
    ]}});
    assert_eq!(imdb_claim(&claims).as_deref(), Some("tt0111161"));
    assert_eq!(imdb_claim(&json!({"claims": {}})), None);
    assert_eq!(imdb_claim(&json!({"error": {}})), None);
}

#[test]
fn wikidata_answers_give_the_article() {
    let found = json!({"query": {"searchinfo": {"totalhits": 1}, "search": [{"ns": 0, "title": "Q172241"}]}});
    assert_eq!(wikidata_item(&found).as_deref(), Some("Q172241"));
    assert_eq!(wikidata_item(&json!({"query": {"search": []}})), None);
    assert_eq!(
        wikidata_item(&json!({"query": {"search": [{"title": "Property:P345"}]}})),
        None
    );

    let entity = json!({"entities": {"Q172241": {"type": "item", "id": "Q172241",
            "sitelinks": {"enwiki": {"site": "enwiki", "title": "The Shawshank Redemption", "badges": []}}}}});
    assert_eq!(
        enwiki_title(&entity, "Q172241").as_deref(),
        Some("The Shawshank Redemption")
    );
    assert_eq!(
        enwiki_title(&json!({"entities": {"Q1": {"sitelinks": {}}}}), "Q1"),
        None
    );
}

fn summary_fixture() -> Value {
    json!({
        "type": "standard",
        "title": "Heat (1995 film)",
        "description": "1995 film by Michael Mann",
        "extract": "Heat is a 1995 American crime film written and directed by Michael Mann.",
        "thumbnail": {"source": "https://upload.wikimedia.org/thumb.jpg"},
        "originalimage": {"source": "https://upload.wikimedia.org/original.jpg"},
        "content_urls": {"desktop": {"page": "https://en.wikipedia.org/wiki/Heat_(1995_film)"}}
    })
}

#[test]
fn a_summary_parses() {
    let info = parse_summary(&summary_fixture()).unwrap();
    assert!(info.plot.as_deref().unwrap().starts_with("Heat is a 1995"));
    assert_eq!(
        info.poster.as_deref(),
        Some("https://upload.wikimedia.org/original.jpg")
    );
    assert_eq!(info.sources, vec![SOURCE_WIKIPEDIA]);
    assert_eq!(
        info.links[0].url,
        "https://en.wikipedia.org/wiki/Heat_(1995_film)"
    );
    assert_eq!(info.title, None);
    assert_eq!(
        parse_summary(&json!({"type": "disambiguation", "extract": "Heat may refer to"})),
        None
    );
}

#[test]
fn title_searches_are_conservative() {
    let results = json!({"query": {"search": [
        {"title": "Heat (1995 film)"},
        {"title": "Heat Wave (film)"},
        {"title": "Heat"},
        {"title": "Michael Mann"}
    ]}});
    assert_eq!(
        title_candidates(&results, "Heat"),
        vec!["Heat (1995 film)", "Heat"]
    );

    let page = summary_fixture();
    assert!(film_matches(&page, "Heat", Some(1995)));
    assert!(film_matches(&page, "Heat", None));
    assert!(!film_matches(&page, "Heat", Some(1986)));
    assert!(!film_matches(&page, "Cold", Some(1995)));
    let physics =
        json!({"type": "standard", "title": "Heat", "description": "Type of energy transfer"});
    assert!(!film_matches(&physics, "Heat", None));
    let disambiguation =
        json!({"type": "disambiguation", "title": "Heat (film)", "description": "film"});
    assert!(!film_matches(&disambiguation, "Heat", None));
}
