//! Episode titles and clock text as the library and the player show them (web/src/titles.ts,
//! player-time.ts).

/// `S01E05`, or "" when the season or episode is unknown (season 0 is specials: real).
pub fn episode_code(season: Option<u32>, episode: Option<u32>) -> String {
    match (season, episode) {
        (Some(s), Some(e)) if e > 0 => format!("S{s:02}E{e:02}"),
        _ => String::new(),
    }
}

/// `name` without a leading show name, then without a leading `S01E05`, `(S01E05):`, `1x05.`
/// or `Season N Episode M` (titles.ts `cleanEpisodeTitle`).
pub fn clean_episode_title(name: &str, season: Option<u32>, episode: Option<u32>, show: Option<&str>) -> String {
    let mut text = name.trim().to_owned();
    if text.is_empty() {
        return text;
    }
    if let Some(series) = show.map(str::trim).filter(|s| !s.is_empty()) {
        if text.len() > series.len()
            && text.is_char_boundary(series.len())
            && text[..series.len()].to_lowercase() == series.to_lowercase()
        {
            text = strip_separators(&text[series.len()..], "\u{20}:.-–—·|").to_owned();
        }
    }
    for pattern in [paren_code as fn(&mut Cur) -> bool, plain_code, cross_code, season_episode] {
        if let Some(rest) = matched(&text, pattern) {
            text = rest;
        }
    }
    let code = episode_code(season, episode);
    if !code.is_empty() {
        let c2 = code.clone();
        if let Some(rest) = matched(&text, move |c| {
            c.lit("(") && {
                c.ws();
                c.lit_ci(&c2)
            } && {
                c.ws();
                c.lit(")")
            } && {
                c.ws();
                c.one_of(":.-–—");
                c.ws();
                true
            }
        }) {
            text = rest;
        }
        if text.to_uppercase().starts_with(&code) {
            text = strip_separators(&text[code.len()..], "\u{20}:.-–—·|").to_owned();
        }
    }
    text.trim().to_owned()
}

/// `S01E05 · Name`, or whichever half is known (main.ts `episodeLine`).
pub fn episode_line(season: Option<u32>, episode: Option<u32>, name: Option<&str>) -> String {
    let code = episode_code(season, episode);
    let title = clean_episode_title(name.unwrap_or(""), season, episode, None);
    match (code.is_empty(), title.is_empty()) {
        (false, false) => format!("{code} · {title}"),
        (false, true) => code,
        _ => title,
    }
}

/// Clock text: `4:05`, hours only after the first hour (`1:02:03`).
pub fn format_clock(seconds: f64) -> String {
    let whole = if seconds.is_finite() && seconds > 0.0 { seconds.floor() as u64 } else { 0 };
    let (h, m, s) = (whole / 3600, (whole % 3600) / 60, whole % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// A resume point worth offering: past the first minute.
pub fn resume_of(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v > 60.0)
}

/// Leading whitespace and any of `seps`, as `^[\s<seps>]+` removes them.
fn strip_separators<'a>(s: &'a str, seps: &str) -> &'a str {
    s.trim_start_matches(|c: char| c.is_whitespace() || seps.contains(c))
}

fn matched(text: &str, pattern: impl Fn(&mut Cur) -> bool) -> Option<String> {
    let mut c = Cur { s: text, at: 0 };
    pattern(&mut c).then(|| text[c.at..].to_owned())
}

/// `^\(\s*S?\d{1,2}\s*[Ex]\s*\d{1,3}\s*\)\s*[:.\-–—·|]?\s*` (case-insensitive).
fn paren_code(c: &mut Cur) -> bool {
    if !c.lit("(") {
        return false;
    }
    c.ws();
    c.lit_ci("s");
    let ok = c.digits(1, 2) && {
        c.ws();
        c.one_of("eExX")
    } && {
        c.ws();
        c.digits(1, 3)
    } && {
        c.ws();
        c.lit(")")
    };
    ok && c.tail(":.-–—·|")
}

/// `^S\d{1,2}E\d{1,3}\s*[:.\-–—·|]?\s*` (case-insensitive).
fn plain_code(c: &mut Cur) -> bool {
    c.lit_ci("s") && c.digits(1, 2) && c.lit_ci("e") && c.digits(1, 3) && c.tail(":.-–—·|")
}

/// `^\d{1,2}\s*[x×]\s*\d{1,3}\s*[.:,\-–—·|]?\s*` (case-insensitive).
fn cross_code(c: &mut Cur) -> bool {
    c.digits(1, 2)
        && {
            c.ws();
            c.one_of("xX×")
        }
        && {
            c.ws();
            c.digits(1, 3)
        }
        && c.tail(".:,-–—·|")
}

/// `^Season\s+\d+\s+Episode\s+\d+\s*[:.\-–—]?\s*` (case-insensitive).
fn season_episode(c: &mut Cur) -> bool {
    c.lit_ci("season")
        && c.ws() > 0
        && c.digits(1, usize::MAX)
        && c.ws() > 0
        && c.lit_ci("episode")
        && c.ws() > 0
        && c.digits(1, usize::MAX)
        && c.tail(":.-–—")
}

/// A cursor for the anchored patterns above.
struct Cur<'a> {
    s: &'a str,
    at: usize,
}

impl Cur<'_> {
    fn rest(&self) -> &str {
        &self.s[self.at..]
    }

    fn ws(&mut self) -> usize {
        let n = self.rest().len() - self.rest().trim_start().len();
        self.at += n;
        n
    }

    fn lit(&mut self, l: &str) -> bool {
        let ok = self.rest().starts_with(l);
        if ok {
            self.at += l.len();
        }
        ok
    }

    fn lit_ci(&mut self, l: &str) -> bool {
        let r = self.rest();
        let ok = r.len() >= l.len() && r.is_char_boundary(l.len()) && r[..l.len()].eq_ignore_ascii_case(l);
        if ok {
            self.at += l.len();
        }
        ok
    }

    fn one_of(&mut self, set: &str) -> bool {
        match self.rest().chars().next() {
            Some(ch) if set.contains(ch) => {
                self.at += ch.len_utf8();
                true
            }
            _ => false,
        }
    }

    fn digits(&mut self, min: usize, max: usize) -> bool {
        let n = self.rest().bytes().take_while(u8::is_ascii_digit).count().min(max);
        if n < min {
            return false;
        }
        self.at += n;
        true
    }

    /// `\s*[<set>]?\s*`.
    fn tail(&mut self, set: &str) -> bool {
        self.ws();
        self.one_of(set);
        self.ws();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_like_titles_ts() {
        let c = |n: &str| clean_episode_title(n, Some(1), Some(5), None);
        assert_eq!(c("S01E05 - Pilot"), "Pilot");
        assert_eq!(c("(s1x05): Pilot"), "Pilot");
        assert_eq!(c("1x05. Pilot"), "Pilot");
        assert_eq!(c("Season 1 Episode 5: Pilot"), "Pilot");
        assert_eq!(c("(S01E05) Pilot"), "Pilot");
        assert_eq!(c("Pilot"), "Pilot");
        assert_eq!(c("S01E05"), "");
        assert_eq!(clean_episode_title("Show: S01E05 Pilot", Some(1), Some(5), Some("show")), "Pilot");
        assert_eq!(clean_episode_title("123x4 x", None, None, None), "123x4 x");
        assert_eq!(episode_line(Some(0), Some(3), Some("Special")), "S00E03 · Special");
        assert_eq!(episode_line(Some(2), None, Some("Name")), "Name");
        assert_eq!(episode_line(Some(2), Some(1), None), "S02E01");
    }

    #[test]
    fn clocks_like_player_time_ts() {
        assert_eq!(format_clock(65.9), "1:05");
        assert_eq!(format_clock(3723.0), "1:02:03");
        assert_eq!(format_clock(f64::NAN), "0:00");
        assert_eq!(resume_of(Some(60.0)), None);
        assert_eq!(resume_of(Some(61.0)), Some(61.0));
    }
}
