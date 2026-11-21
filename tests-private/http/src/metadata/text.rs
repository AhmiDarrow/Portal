use super::*;

#[test]
fn titles_normalize() {
    assert_eq!(
        norm_title("  The Shawshank  Redemption! "),
        "the shawshank redemption"
    );
    assert_eq!(
        norm_title("Marvel's Agents of S.H.I.E.L.D."),
        "marvels agents of shield"
    );
    assert_eq!(norm_title("Law & Order: SVU"), "law and order svu");
    assert_eq!(norm_title("Amélie"), "amélie");
}

#[test]
fn html_becomes_plain_text() {
    assert_eq!(
            strip_html("<p><b>Breaking Bad</b> follows Walter &amp; Jesse.</p><p>Second&nbsp;line &#39;x&#39; &#x2014; y</p>"),
            "Breaking Bad follows Walter & Jesse.\nSecond line 'x' — y"
        );
    assert_eq!(
        strip_html("A <i>tag</i> & a lone amp"),
        "A tag & a lone amp"
    );
    assert_eq!(strip_html(""), "");
}

#[test]
fn clip_marks_a_cut() {
    assert_eq!(clip("abc", 3), "abc");
    assert_eq!(clip("abcd", 3), "abc…");
}

#[test]
fn years_within_one_are_close() {
    assert!(year_close(Some(1994), Some(1995)));
    assert!(!year_close(Some(1994), Some(1996)));
    assert!(year_close(None, Some(1994)));
    assert!(year_close(Some(1994), None));
}
