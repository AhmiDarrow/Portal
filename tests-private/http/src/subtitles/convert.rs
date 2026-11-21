use super::*;
use std::io::Write;

fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap();
    }
    buffer.into_inner()
}

const SRT: &str = "1\r\n00:00:01,000 --> 00:00:02,500\r\n<font color=\"#ffff00\">Hello</font> <i>there</i>\r\n{\\an8}Second & line\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000 X1:10 X2:20\r\n-- Daddy!\r\n\r\n3\r\n00:00:05,000 --> 00:00:06,000\r\n<b></b>\r\n\r\n";

#[test]
fn srt_becomes_clean_webvtt() {
    let vtt = to_vtt(SRT.as_bytes(), None, Some("en")).unwrap();
    assert_eq!(
            vtt,
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.500 line:0\nHello <i>there</i>\nSecond &amp; line\n\n00:00:03.000 --> 00:00:04.000\n-- Daddy!\n\n"
        );
}

#[test]
fn webvtt_input_loses_ids_notes_and_settings() {
    let text = "WEBVTT - title\n\nSTYLE\n::cue { color: red }\n\nNOTE a note\n\nintro\n00:01.000 --> 00:02.000 align:start line:0\n<c.yellow>Hi</c> <v Bob>there</v>\n\n00:00:00.500 --> 00:00:00.900\nFirst\n";
    let cues = timed_cues(text);
    assert_eq!(cues.len(), 2);
    assert_eq!(cues[0].text, "First");
    assert_eq!(cues[1].start, 1.0);
    assert_eq!(cues[1].text, "Hi there");
}

#[test]
fn ass_becomes_plain_cues() {
    let ass = "[Script Info]\nTitle: x\n\n[V4+ Styles]\nFormat: Name, Fontname\nStyle: Default,Arial\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:05.10,0:00:07.00,Default,,0,0,0,,{\\i1\\fad(10,20)}Hello,\\Nworld{\\i0}\\hagain\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,First < last\nDialogue: 0,0:00:03.00,0:00:04.00,Default,,0,0,0,,{\\p1}m 0 0 l 100 0 100 100{\\p0}\nComment: 0,0:00:03.00,0:00:04.00,Default,,0,0,0,,not shown\n";
    let vtt = to_vtt(ass.as_bytes(), None, None).unwrap();
    assert_eq!(
            vtt,
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nFirst &lt; last\n\n00:00:05.100 --> 00:00:07.000\n<i>Hello,</i>\n<i>world</i> again\n\n"
        );
}

const SCRIPT: &str = "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, Bold, Italic, Alignment\nStyle: Default,Arial,20,0,0,2\nStyle: Italics,Arial,20,0,-1,2\nStyle: Top,Arial,20,0,0,8\nStyle: Signs,Arial,20,0,0,5\nStyle: OP Romaji,Arial,20,0,0,8\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

#[test]
fn ass_signs_and_repeats_fall_away() {
    let events = [
        // The same line on two layers shows once.
        "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\bord3\\blur2}Hi there",
        "Dialogue: 1,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\bord0}Hi there",
        "Dialogue: 0,0:00:01.00,0:00:02.00,Signs,,0,0,0,,CAFE",
        "Dialogue: 0,0:00:01.00,0:00:02.00,OP Romaji,,0,0,0,,La la la",
        "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\pos(320,40)}STATION",
        "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\move(0,0,9,9)}SCROLL",
        "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\p1}m 0 0 l 9 9{\\p0}Box",
        "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\fad(1,1)}",
        "Dialogue: 0,0:00:03.00,0:00:04.00,Italics,,0,0,0,,Thinking{\\i0} aloud",
        "Dialogue: 0,0:00:03.00,0:00:04.00,Top,,0,0,0,,Up here",
    ];
    let ass = format!("{SCRIPT}{}\n", events.join("\n"));
    let vtt = to_vtt(ass.as_bytes(), None, None).unwrap();
    assert_eq!(
            vtt,
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHi there\n\n00:00:03.000 --> 00:00:04.000\n<i>Thinking</i> aloud\n\n00:00:03.000 --> 00:00:04.000 line:0\nUp here\n\n"
        );
    assert!(!is_sign_style("Main") && !is_sign_style("Top") && !is_sign_style("Editor"));
    assert!(is_sign_style("ED-Eng") && is_sign_style("Karaoke") && is_sign_style("Song1"));
}

#[test]
fn a_script_that_places_every_line_keeps_them() {
    let ass = format!(
            "{SCRIPT}Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{{\\pos(320,440)}}One\nDialogue: 0,0:00:03.00,0:00:04.00,Default,,0,0,0,,{{\\pos(320,440)}}Two\n"
        );
    let cues = ass_cues(&ass);
    assert_eq!(cues.len(), 2);
}

#[test]
fn ass_overrides_become_tags_and_placement() {
    let style = AssStyle::default();
    let line = ass_text("{\\an8}{\\b1}Bold {\\i1}both{\\b0} it & <x>", style).unwrap();
    assert_eq!(
        line.text,
        "<b>Bold </b><b><i>both</i></b><i> it &amp; &lt;x&gt;</i>"
    );
    assert!(line.top);
    // An italic left open closes at each line's end.
    let line = ass_text("{\\i1}One\\NTwo", style).unwrap();
    assert_eq!(line.text, "<i>One</i>\n<i>Two</i>");
    assert!(!line.top);
    // SSA's own top codes, and the first alignment wins.
    assert!(ass_text("{\\a6}Up{\\an2}", style).unwrap().top);
    let middle = ass_text("{\\an5\\iclip(0,0,1,1)\\blur2}Mid", style).unwrap();
    assert!(!middle.top);
}

#[test]
fn srt_top_lines_stay_at_the_top_through_a_shift() {
    let srt = "1\n00:00:01,000 --> 00:00:02,500\n{\\an8}Up here\n\n2\n00:00:03,000 --> 00:00:04,000\n{\\an2}Down here\n\n3\n00:00:05,000 --> 00:00:06,000\n{\\an9}Corner\n";
    let vtt = to_vtt(srt.as_bytes(), None, None).unwrap();
    assert_eq!(
            vtt,
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.500 line:0\nUp here\n\n00:00:03.000 --> 00:00:04.000\nDown here\n\n00:00:05.000 --> 00:00:06.000 line:0\nCorner\n\n"
        );
    // `?offset=` moves cue times and keeps their settings.
    let shifted = portal_engine::shift_vtt(&vtt, -0.5);
    assert!(
        shifted.contains("00:00:00.500 --> 00:00:02.000 line:0\nUp here"),
        "{shifted}"
    );
    assert!(shifted.contains("00:00:02.500 --> 00:00:03.500\nDown here"));
}

#[test]
fn charsets_are_decoded() {
    let body = "1\n00:00:01,000 --> 00:00:02,000\nCafé déjà vu\n";
    let latin: Vec<u8> = encoding_rs::WINDOWS_1252.encode(body).0.into_owned();
    assert!(std::str::from_utf8(&latin).is_err());
    assert!(decode(&latin, Some("fr")).contains("Café déjà vu"));

    let cyrillic = "Привет, мир";
    let cp1251 = encoding_rs::WINDOWS_1251.encode(cyrillic).0.into_owned();
    assert_eq!(decode(&cp1251, Some("ru")), cyrillic);

    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend_from_slice(body.as_bytes());
    assert!(decode(&bom, None).starts_with('1'));

    let mut utf16 = vec![0xFF, 0xFE];
    for unit in body.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    assert_eq!(decode(&utf16, None), body);
    // Without its mark, too.
    assert_eq!(decode(&utf16[2..], None), body);
}

#[test]
fn zips_give_up_the_right_member() {
    let other = b"1\n00:00:09,000 --> 00:00:10,000\nWrong\n";
    let zip = zip_of(&[
        ("Show.S01E01.srt", other),
        ("Show.S01E02.720p.srt", SRT.as_bytes()),
        ("readme.txt", b"hi"),
    ]);
    let vtt = to_vtt(&zip, Some((1, 2)), None).unwrap();
    assert!(vtt.contains("Hello"));
    assert_eq!(to_vtt(&zip, Some((1, 5)), None), Err(Unusable::Archive));

    let movie = zip_of(&[
        (
            "Movie.ass",
            b"[Events]\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,Hi\n",
        ),
        ("Movie.srt", SRT.as_bytes()),
    ]);
    assert!(to_vtt(&movie, None, None).unwrap().contains("Hello"));
    assert_eq!(
        to_vtt(&zip_of(&[("a.txt", b"x")]), None, None),
        Err(Unusable::Archive)
    );
    assert_eq!(
        to_vtt(b"PK\x03\x04garbage", None, None),
        Err(Unusable::Archive)
    );
    assert_eq!(
        to_vtt(b"<html>nope</html>", None, None),
        Err(Unusable::Empty)
    );
}

#[test]
fn member_choice_prefers_srt_then_size() {
    let names = vec![
        ("b.ass".to_string(), 900),
        ("a.srt".to_string(), 10),
        ("c.srt".to_string(), 20),
    ];
    assert_eq!(choose_member(&names, None).as_deref(), Some("c.srt"));
    assert_eq!(episode_in_name("Show.1x02.HDTV.srt"), Some((1, 2)));
    assert_eq!(
        episode_in_name("dir/Show - S10E105 - x.srt"),
        Some((10, 105))
    );
    assert_eq!(episode_in_name("Movie.1920x1080.srt"), None);
    assert_eq!(episode_in_name("Uses.srt"), None);
}

#[test]
fn timestamps_parse_in_many_forms() {
    let near = |text: &str, want: f64| (timestamp(text).unwrap() - want).abs() < 1e-9;
    assert!(near("01:02:03,456", 3723.456));
    assert!(near("02:03.4", 123.4));
    assert!(near("0:00:05.10", 5.1));
    assert_eq!(timestamp("5"), None);
    assert_eq!(timestamp("aa:bb:cc"), None);
}

#[test]
fn packs_are_matched_by_folder_or_episode_word() {
    let names = |list: &[&str]| -> Vec<(String, u64)> {
        list.iter().map(|name| (name.to_string(), 100)).collect()
    };
    let pack = names(&[
        "Show.S01E01/eng.srt",
        "Show.S01E02/eng.srt",
        "Show.S01E03/eng.srt",
    ]);
    assert_eq!(
        choose_member(&pack, Some((1, 2))).as_deref(),
        Some("Show.S01E02/eng.srt")
    );
    let words = names(&[
        "Show/Episode 01.srt",
        "Show/Episode 02.srt",
        "Show/Ep.03.srt",
    ]);
    assert_eq!(
        choose_member(&words, Some((1, 2))).as_deref(),
        Some("Show/Episode 02.srt")
    );
    assert_eq!(
        choose_member(&words, Some((1, 3))).as_deref(),
        Some("Show/Ep.03.srt")
    );
    assert_eq!(
        choose_member(&names(&["Show E02.srt"]), Some((2, 2))).as_deref(),
        Some("Show E02.srt")
    );
    // A lone file named for another episode is not this one.
    assert_eq!(
        choose_member(&names(&["Show.S01E01.srt"]), Some((1, 2))),
        None
    );
    // Of several files named for no episode, Portal does not guess.
    assert_eq!(
        choose_member(&names(&["a.srt", "b.srt"]), Some((1, 2))),
        None
    );
    assert_eq!(
        choose_member(&names(&["English.srt"]), Some((1, 2))).as_deref(),
        Some("English.srt")
    );
    assert_eq!(
        choose_member(&names(&["a.srt", "b.srt"]), None).as_deref(),
        Some("a.srt")
    );
    assert_eq!(episode_number("Movie.English.srt"), None);
    assert_eq!(episode_number("x264-e-sub.srt"), None);
}

#[test]
fn a_stray_bad_byte_keeps_utf8() {
    let mut body = "1\n00:00:01,000 --> 00:00:02,000\n".as_bytes().to_vec();
    for _ in 0..40 {
        body.extend_from_slice("Café déjà vu, señor.\n".as_bytes());
    }
    body.push(0xFF);
    let text = decode(&body, Some("fr"));
    assert!(text.contains("Café déjà vu, señor."));
    // A legacy file stays legacy.
    let latin = encoding_rs::WINDOWS_1252
        .encode("Café déjà vu")
        .0
        .into_owned();
    assert_eq!(decode(&latin, Some("fr")), "Café déjà vu");
}

#[test]
fn entities_are_read() {
    let srt = "1\n00:00:01,000 --> 00:00:02,000\n&quot;Hi&quot; &#39;there&apos; &#x2019; &amp; &lt;3 &bogus; Tom & Jerry; ok\n";
    let vtt = to_vtt(srt.as_bytes(), None, None).unwrap();
    assert!(
        vtt.contains("\"Hi\" 'there' \u{2019} &amp; &lt;3 &amp;bogus; Tom &amp; Jerry; ok"),
        "{vtt}"
    );
}

#[test]
fn microdvd_becomes_webvtt() {
    let sub = "{1}{1}25\r\n{25}{50}Hello|{y:i}/world\r\n{75}{}Last\r\n";
    let vtt = to_vtt(sub.as_bytes(), None, None).unwrap();
    assert_eq!(
            vtt,
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHello\nworld\n\n00:00:03.000 --> 00:00:05.000\nLast\n\n"
        );
    let plain = to_vtt(b"{240}{480}At 23.976", None, None).unwrap();
    // A first cue that only holds a number is a frame rate only at frame 0 or 1.
    let numbered = to_vtt(b"{240}{480}42\n{500}{600}Next", None, None).unwrap();
    assert!(
        numbered.contains("42") && numbered.contains("Next"),
        "{numbered}"
    );
    assert!(plain.contains("00:00:10.010 --> 00:00:20.020"), "{plain}");
    let zip = zip_of(&[("Movie.sub", sub.as_bytes())]);
    assert!(to_vtt(&zip, None, None).unwrap().contains("Hello"));
    // A VobSub picture file is not text.
    let vobsub = zip_of(&[
        ("Movie.idx", b"# VobSub index"),
        ("Movie.sub", b"\x00\x00\x01\xba"),
    ]);
    assert_eq!(to_vtt(&vobsub, None, None), Err(Unusable::Archive));
}
