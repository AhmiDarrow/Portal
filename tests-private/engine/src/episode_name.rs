use super::*;

const DAY_NINETEEN: &str =
    "Example Show (2001) - S04E19 - Day 4 1.00 A.M. - 2.00 A.M. (1080p BluRay x265 Group).mkv";

#[test]
fn a_clock_in_the_name_does_not_change_the_episode() {
    assert_eq!(stated_episode(DAY_NINETEEN), Some((4, 19)));
    let encoded = format!(
        "https://example.test/d/abc/{}?x=1",
        DAY_NINETEEN.replace(' ', "%20")
    );
    assert_eq!(named_episode(&encoded), Some((4, 19)));
    assert!(episode_conflict(&encoded, 4, 1));
    assert!(!episode_conflict(&encoded, 4, 19));
    assert_eq!(
        stated_episode("Example Show (2001) - S04E01 - Day 4 7.00 A.M. - 8.00 A.M..mkv"),
        Some((4, 1))
    );
    assert!(stated_episode("Day 4 1.00 A.M. - 2.00 A.M..mkv").is_none());
    assert!(!episode_conflict("day.4.1.00.mkv", 4, 1));
    assert!(stated_episode("Show.S04E01E02.mkv").is_none());
    assert!(!episode_conflict("Show.S04E01E02.mkv", 4, 1));
    assert_eq!(named_episode("Show+Name.S04E19.mkv"), Some((4, 19)));
    assert_eq!(named_episode("Show.3x16.mkv"), Some((3, 16)));
    assert_eq!(
        decoded_media_path("dir/100%25+S04E01.mkv"),
        "dir/100%+S04E01.mkv"
    );
}
