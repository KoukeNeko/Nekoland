//! Lyrics for what plays: fetched from LRCLIB, chosen by the recording's length, and
//! mapped onto playback time, by the user's taps when the only lyrics are another take's.
//!
//! A live take and the album take share a title but not a length, an intro or a pace, so
//! lyrics are chosen by length, never by name, and other takes' lyrics are not played on a guess.

use std::process::Command;

use serde::{Deserialize, Serialize};

const LRCLIB: &str = "https://lrclib.net/api";
/// How many sources a song's lyrics were asked of; a song kept from fewer is asked again.
pub const SOURCES: u8 = 5;
/// Seconds: lyrics this close to the track's length are of the same recording.
pub const MATCH: f64 = 2.0;
/// Playback seconds per lyric second a live take can plausibly run at.
const PACE: (f64, f64) = (0.8, 1.25);
/// A tap this soon after an earlier one corrects it rather than adding to it.
const CORRECTION: f64 = 15.0;

/// One timed line. An empty `text` marks where the singing stops, for an interlude.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub time: f64,
    pub text: String,
    /// Each character's start, when the source times words (enhanced LRC).
    pub words: Option<Vec<(f64, String)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Live,
    Album,
}

/// One set of lyrics found for a song.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    #[serde(default = "lrclib_name")]
    pub source: String,
    /// What the source calls the song, which another source may know it by.
    #[serde(default)]
    pub title: String,
    pub album: String,
    /// The length of the recording these lyrics were timed to.
    pub duration: f64,
    pub synced: Vec<Line>,
    pub plain: Vec<String>,
}

fn lrclib_name() -> String {
    "LRCLIB".into()
}

impl Candidate {
    /// Whether the lines are timed word by word.
    pub fn has_words(&self) -> bool {
        self.synced.iter().any(|line| line.words.is_some())
    }

    pub fn kind(&self) -> Kind {
        kind_of(&self.album)
    }

    pub fn matches(&self, duration: f64) -> bool {
        (self.duration - duration).abs() <= MATCH
    }
}

/// Whether a name says it is a live recording.
pub fn says_live(name: &str) -> bool {
    let lower = name.to_lowercase();
    let word = |w: &str| {
        lower
            .split(|c: char| !c.is_alphanumeric())
            .any(|token| token == w)
    };
    ["live", "concert", "unplugged", "tour"].into_iter().any(word)
        || ["現場", "演唱會", "巡迴"].into_iter().any(|w| lower.contains(w))
}

fn kind_of(album: &str) -> Kind {
    if says_live(album) {
        Kind::Live
    } else {
        Kind::Album
    }
}

/// What this recording is, and what says so: the album of lyrics matching its length,
/// else whether MusicBrainz files its album as live, else its own album or title. None
/// when nothing tells; it is not guessed.
pub fn recording_kind(
    title: &str,
    album: &str,
    duration: f64,
    found: &[Candidate],
    release_live: Option<bool>,
) -> Option<(Kind, &'static str)> {
    if let Some(matching) = found.iter().find(|c| c.matches(duration)) {
        return Some((matching.kind(), "時長相符的歌詞"));
    }
    if let Some(live) = release_live {
        return Some((
            if live { Kind::Live } else { Kind::Album },
            "MusicBrainz 專輯類型",
        ));
    }
    (says_live(album) || says_live(title)).then_some((Kind::Live, "曲目名稱"))
}

/// Whether MusicBrainz files an artist's album as live; None when it has no such album.
pub fn release_is_live(artist: &str, album: &str) -> Result<Option<bool>, Missing> {
    let quote = |text: &str| format!("\"{}\"", text.replace(['"', '\\'], " "));
    let query = format!("releasegroup:{} AND artist:{}", quote(album), quote(artist));
    let found = get(
        "https://musicbrainz.org/ws/2/release-group/",
        &[("query", query), ("fmt", "json".into()), ("limit", "1".into())],
    )?;
    let group = found
        .as_ref()
        .and_then(|found| found.get("release-groups")?.as_array()?.first().cloned());
    Ok(group
        .filter(|group| group.get("score").and_then(|score| score.as_i64()).unwrap_or(0) >= 90)
        .map(|group| {
            (group.get("secondary-types").and_then(|types| types.as_array()))
                .is_some_and(|types| types.iter().any(|kind| kind.as_str() == Some("Live")))
        }))
}

/// The lyrics closest in length, and whether they are this recording's.
pub fn choose(duration: f64, found: &[Candidate]) -> Option<(usize, bool)> {
    // The recording's own timed lyrics first, those timed word by word among them, then the
    // nearest in length.
    let rank = |c: &Candidate| (!(c.matches(duration) && !c.synced.is_empty()), !c.has_words());
    let (index, best) = found
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.synced.is_empty() || !c.plain.is_empty())
        .min_by(|a, b| {
            let (ra, rb) = (rank(a.1), rank(b.1));
            // Words only tell takes apart that are all the recording's own.
            let words = if ra.0 || rb.0 {
                std::cmp::Ordering::Equal
            } else {
                ra.1.cmp(&rb.1)
            };
            // Lyrics with no times are for when there are none with, however near in length.
            let plain = a.1.synced.is_empty().cmp(&b.1.synced.is_empty());
            ra.0.cmp(&rb.0).then(words).then(plain).then_with(|| {
                (a.1.duration - duration)
                    .abs()
                    .total_cmp(&(b.1.duration - duration).abs())
            })
        })?;
    Some((index, best.matches(duration) && !best.synced.is_empty()))
}

/// What the lyrics show one after another: their lines, and the interludes, the stretches
/// longer than `INTERLUDE` without singing, the intro among them.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub start: f64,
    /// The line shown, or None for an interlude, which lasts until `end`.
    pub line: Option<usize>,
    pub end: f64,
}

/// Seconds without singing that are an interlude, shown as such rather than as the line before.
pub const INTERLUDE: f64 = 12.0;

/// The rows of `lines`, timed to a recording `length` long.
pub fn rows(lines: &[Line], length: f64) -> Vec<Row> {
    let mut rows = Vec::new();
    let first = lines
        .iter()
        .find(|line| !line.text.is_empty())
        .map_or(length, |line| line.time);
    if first > INTERLUDE {
        rows.push(Row {
            start: 0.0,
            line: None,
            end: first,
        });
    }
    for (index, line) in lines.iter().enumerate() {
        let next = lines.get(index + 1).map_or(length, |next| next.time);
        if !line.text.is_empty() {
            rows.push(Row {
                start: line.time,
                line: Some(index),
                end: next,
            });
        } else if next - line.time > INTERLUDE && line.time >= first {
            rows.push(Row {
                start: line.time,
                line: None,
                end: next,
            });
        }
    }
    rows
}

/// The row being sung at lyric time `at`; None before the first.
pub fn row_at(rows: &[Row], at: f64) -> Option<usize> {
    rows.iter().rposition(|row| row.start <= at)
}

/// Parses LRC: several stamps on one line, `[offset:]`, and `<mm:ss.xx>` word stamps.
pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut offset = 0.0;
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        if let Some(value) = rest.strip_prefix("[offset:").and_then(|r| r.strip_suffix(']')) {
            // A positive offset shows the lyrics sooner.
            offset = value.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
            continue;
        }
        let mut times = Vec::new();
        while let Some(inner) = rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            match stamp(inner.0) {
                Some(time) => times.push(time),
                None => break,
            }
            rest = inner.1;
        }
        if times.is_empty() {
            continue;
        }
        let (text, words) = words_of(rest);
        for time in times {
            let shift = |t: f64| (t - offset).max(0.0);
            lines.push(Line {
                time: shift(time),
                text: text.clone(),
                words: words
                    .as_ref()
                    .map(|w| w.iter().map(|(t, s)| (shift(*t), s.clone())).collect()),
            });
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx`, in seconds.
fn stamp(text: &str) -> Option<f64> {
    let (minutes, seconds) = text.split_once(':')?;
    Some(minutes.trim().parse::<u32>().ok()? as f64 * 60.0 + seconds.trim().parse::<f64>().ok()?)
}

/// The words of a line, with their times when the line is word-timed.
fn words_of(rest: &str) -> (String, Option<Vec<(f64, String)>>) {
    if !rest.contains('<') {
        return (rest.trim().to_string(), None);
    }
    let mut words = Vec::new();
    let mut text = String::new();
    let mut at = None;
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let tag: String = chars.by_ref().take_while(|&c| c != '>').collect();
            at = stamp(&tag).or(at);
            continue;
        }
        text.push(c);
        if let Some(time) = at {
            words.push((time, c.to_string()));
        }
    }
    let text = text.trim().to_string();
    (text, (!words.is_empty()).then_some(words))
}

/// How lyric time maps onto playback time: by an offset, or through the lines the user
/// tapped while hearing them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Alignment {
    pub offset: f64,
    /// (lyric time, playback time) of each tap, in order.
    pub anchors: Vec<(f64, f64)>,
}

impl Alignment {
    /// The lyric time at playback time `at`.
    pub fn lyric_at(&self, at: f64, lyrics: f64, track: f64) -> f64 {
        self.map(at, 1, 0, lyrics, track)
    }

    /// The playback time at lyric time `at`.
    pub fn playback_at(&self, at: f64, lyrics: f64, track: f64) -> f64 {
        self.map(at, 0, 1, lyrics, track)
    }

    /// Through one tap the pace comes from what is left of each, lyrics and track, to their
    /// ends; between taps it is piecewise linear, so a solo or a speech affects only its stretch.
    fn map(&self, x: f64, from: usize, to: usize, lyrics: f64, track: f64) -> f64 {
        let pick = |a: (f64, f64), i: usize| if i == 0 { a.0 } else { a.1 };
        let Some(&last) = self.anchors.last() else {
            return if from == 0 {
                x + self.offset
            } else {
                x - self.offset
            };
        };
        if self.anchors.len() == 1 || x >= pick(last, from) {
            let pace = ((track - last.1) / (lyrics - last.0)).clamp(PACE.0, PACE.1);
            let rate = if from == 0 { pace } else { 1.0 / pace };
            return pick(last, to) + (x - pick(last, from)) * rate;
        }
        let i = self
            .anchors
            .windows(2)
            .position(|w| x <= pick(w[1], from))
            .unwrap_or(0);
        let (a, b) = (self.anchors[i], self.anchors[i + 1]);
        pick(a, to) + (x - pick(a, from)) * (pick(b, to) - pick(a, to)) / (pick(b, from) - pick(a, from))
    }

    /// The user hears lyric time `lyric` at playback time `now`. Taps for later lines, and
    /// ones too close to this or implying an implausible pace, are corrections and go.
    pub fn tap(&mut self, lyric: f64, now: f64) {
        self.anchors.retain(|&(l, p)| {
            let pace = (now - p) / (lyric - l);
            l < lyric && now - p >= CORRECTION && (PACE.0..=PACE.1).contains(&pace)
        });
        self.anchors.push((lyric, now));
    }
}

/// Why no lyrics came: none exist, or they could not be asked for. Only the first is kept.
#[derive(Debug, PartialEq)]
pub enum Missing {
    NotFound,
    Unreachable,
}

/// Every set of lyrics the services have for this song: LRCLIB's first, the one for this length
/// at the head of them, then the others' (`lyric_sources`), which are often timed word by word.
/// The others being down does not stop LRCLIB's, and LRCLIB being down is told, as the rest do
/// not say whether there are none.
pub fn fetch(title: &str, artist: &str, album: &str, duration: f64) -> Result<Vec<Candidate>, Missing> {
    let (from_lrclib, others) = std::thread::scope(|scope| {
        let others = scope.spawn(|| crate::lyric_sources::all(title, artist, duration));
        let from_lrclib = fetch_lrclib(title, artist, album, duration);
        (from_lrclib, others.join().unwrap_or_default())
    });
    let mut found = match from_lrclib {
        Ok(found) => found,
        Err(Missing::NotFound) if !others.is_empty() => Vec::new(),
        Err(missing) => return Err(missing),
    };
    found.extend(others);
    // LRCLIB lists some of its takes more than once.
    let mut kept: Vec<(String, i64, usize, usize)> = Vec::new();
    found.retain(|c| {
        let key = (
            c.source.clone(),
            c.duration.round() as i64,
            c.synced.len(),
            c.plain.len(),
        );
        !kept.contains(&key) && {
            kept.push(key);
            true
        }
    });
    Ok(found)
}

fn fetch_lrclib(title: &str, artist: &str, album: &str, duration: f64) -> Result<Vec<Candidate>, Missing> {
    let title = song_title(title, artist);
    let mut exact = vec![
        ("track_name", title.clone()),
        ("artist_name", artist.to_string()),
        ("duration", format!("{}", duration.round() as i64)),
    ];
    if !album.is_empty() {
        exact.push(("album_name", album.to_string()));
    }
    let mut found: Vec<Candidate> = get(&format!("{LRCLIB}/get"), &exact)?
        .as_ref()
        .and_then(candidate)
        .into_iter()
        .collect();
    let search = get(
        &format!("{LRCLIB}/search"),
        &[("track_name", title), ("artist_name", artist.to_string())],
    )?;
    for item in search.as_ref().and_then(|v| v.as_array()).into_iter().flatten() {
        if let Some(c) = candidate(item).filter(|c| {
            !found
                .iter()
                .any(|f| f.duration == c.duration && f.album == c.album)
        }) {
            found.push(c);
        }
    }
    if found.is_empty() {
        Err(Missing::NotFound)
    } else {
        Ok(found)
    }
}

/// "(Live)", "- Live at …" and the like do not name the song.
/// The song a player's title names, which a video's is "Artist - Song", "Artist - LIVE「Song」"
/// and the like: what is in quotes, else what is not the artist's, without version words.
pub(crate) fn song_title(title: &str, artist: &str) -> String {
    for (open, close) in [('「', '」'), ('『', '』'), ('“', '”')] {
        let quoted = title
            .split_once(open)
            .and_then(|(_, rest)| rest.split_once(close))
            .map(|(inside, _)| inside.trim());
        if let Some(inside) = quoted.filter(|inside| !inside.is_empty()) {
            return clean_title(inside);
        }
    }
    let artist = clean_artist(artist).to_lowercase();
    let parts: Vec<&str> = title.split(" - ").collect();
    let kept: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|part| {
            let part = part.trim().to_lowercase();
            artist.is_empty() || part.is_empty() || !(artist.contains(&part) || part.contains(&artist))
        })
        .collect();
    match kept.as_slice() {
        [only] if parts.len() > 1 => clean_title(only),
        _ => clean_title(title),
    }
}

/// An artist as a channel names it ("ヨルシカ / n-buna Official", "Yorushika - Topic"), as
/// the artist's name.
pub(crate) fn clean_artist(artist: &str) -> String {
    let name = artist.split(" / ").next().unwrap_or(artist);
    let name = name.trim().trim_end_matches(" - Topic");
    let name = name.trim_end_matches("VEVO").trim_end_matches(" Official").trim();
    name.to_string()
}

pub(crate) fn clean_title(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' | '（' => depth += 1,
            ')' | ']' | '）' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split(" - ").next().unwrap_or_default().trim().to_string()
}

/// GETs JSON; Ok(None) when the service says there is nothing.
pub(crate) fn get(url: &str, query: &[(&str, String)]) -> Result<Option<serde_json::Value>, Missing> {
    get_from(url, query, None)
}

/// As `get`, for a service that asks where the request is from.
pub(crate) fn get_from(
    url: &str,
    query: &[(&str, String)],
    referer: Option<&str>,
) -> Result<Option<serde_json::Value>, Missing> {
    Ok(get_text(url, query, referer)?.and_then(|text| serde_json::from_str(&text).ok()))
}

/// As `get`, with a header the service asks for.
pub(crate) fn get_headed(
    url: &str,
    query: &[(&str, String)],
    header: &str,
) -> Result<Option<serde_json::Value>, Missing> {
    let mut args = vec![
        "-G".to_string(),
        url.to_string(),
        "-H".to_string(),
        header.to_string(),
    ];
    for (key, value) in query {
        args.push("--data-urlencode".into());
        args.push(format!("{key}={value}"));
    }
    Ok(curl(args)?.and_then(|text| serde_json::from_str(&text).ok()))
}

/// What a GET answers, or Ok(None) when the service says there is nothing.
pub(crate) fn get_text(
    url: &str,
    query: &[(&str, String)],
    referer: Option<&str>,
) -> Result<Option<String>, Missing> {
    let mut args = vec!["-G".to_string(), url.to_string()];
    if let Some(referer) = referer {
        args.extend(["-e".to_string(), referer.to_string()]);
    }
    for (key, value) in query {
        args.push("--data-urlencode".into());
        args.push(format!("{key}={value}"));
    }
    curl(args)
}

/// POSTs a form, and what it answers.
pub(crate) fn post_form(url: &str, form: &str, referer: &str) -> Result<Option<String>, Missing> {
    let args = ["-e", referer, "-X", "POST", "-d", form, url]
        .map(String::from)
        .to_vec();
    curl(args)
}

/// POSTs JSON, and reads JSON back.
pub(crate) fn post_json(url: &str, body: &str) -> Result<Option<serde_json::Value>, Missing> {
    let args = [
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
        "-d",
        body,
        url,
    ]
    .map(String::from)
    .to_vec();
    Ok(curl(args)?.and_then(|text| serde_json::from_str(&text).ok()))
}

fn curl(mut args: Vec<String>) -> Result<Option<String>, Missing> {
    let agent = concat!("Nekoland/", env!("CARGO_PKG_VERSION"));
    let mut all = vec![
        "-s".to_string(),
        "-A".into(),
        agent.into(),
        "--max-time".into(),
        "6".into(),
        "-w".into(),
        "\n%{http_code}".into(),
    ];
    all.append(&mut args);
    let output = Command::new("curl")
        .args(&all)
        .output()
        .map_err(|_| Missing::Unreachable)?;
    let body = String::from_utf8_lossy(&output.stdout);
    let (text, code) = body.rsplit_once('\n').ok_or(Missing::Unreachable)?;
    match code.trim() {
        "200" => Ok(Some(text.to_string())),
        "404" => Ok(None),
        _ => Err(Missing::Unreachable),
    }
}

fn candidate(item: &serde_json::Value) -> Option<Candidate> {
    let text = |key: &str| item.get(key).and_then(|v| v.as_str()).unwrap_or_default();
    let c = Candidate {
        source: lrclib_name(),
        title: String::new(),
        album: text("albumName").to_string(),
        duration: item.get("duration")?.as_f64()?,
        synced: parse_lrc(text("syncedLyrics")),
        plain: text("plainLyrics")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
    };
    (!c.synced.is_empty() || !c.plain.is_empty()).then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_break_a_tie_between_takes_of_one_recording() {
        let mut plain = cand("A", 200.0);
        let mut timed = cand("B", 201.0);
        timed.synced[0].words = Some(vec![(0.0, "a".into())]);
        assert_eq!(choose(200.0, &[plain.clone(), timed.clone()]), Some((1, true)));
        // Not the recording's, they do not count for more than being nearer.
        plain.duration = 100.0;
        timed.duration = 150.0;
        assert_eq!(choose(100.0, &[timed, plain]).map(|c| c.0), Some(1));
    }

    fn cand(album: &str, duration: f64) -> Candidate {
        Candidate {
            source: "LRCLIB".into(),
            title: String::new(),
            album: album.into(),
            duration,
            synced: parse_lrc("[00:12.00]a"),
            plain: vec![],
        }
    }

    #[test]
    fn parses_repeats_offsets_words_and_gaps() {
        let lines = parse_lrc(
            "[offset:+500]\n[00:12.00][01:30.50]副歌\n[00:20.00]\n[00:21.00]<00:21.00>夜<00:21.50>行",
        );
        let times: Vec<f64> = lines.iter().map(|l| l.time).collect();
        assert_eq!(times, vec![11.5, 19.5, 20.5, 90.0]);
        assert_eq!(lines[1].text, "");
        assert_eq!(lines[2].text, "夜行");
        assert_eq!(lines[2].words.as_ref().unwrap()[1], (21.0, "行".to_string()));
    }

    #[test]
    fn chooses_by_length_not_name() {
        let found = [cand("月台與海", 252.0), cand("夜行 Live at Legacy", 303.0)];
        assert_eq!(choose(303.4, &found), Some((1, true)));
        assert_eq!(choose(252.0, &found), Some((0, true)));
        // Only the album take's lyrics for a live take: shown, not synced.
        assert_eq!(choose(303.0, &found[..1]), Some((0, false)));
    }

    #[test]
    fn infers_live_without_guessing() {
        let found = [cand("夜行 Live at Legacy", 303.0)];
        assert_eq!(
            recording_kind("夜行列車", "午夜場", 303.0, &found, Some(false)),
            Some((Kind::Live, "時長相符的歌詞"))
        );
        assert_eq!(recording_kind("夜行列車", "午夜場", 252.0, &found, None), None);
        let filed = recording_kind("夜行列車", "午夜場", 252.0, &found, Some(true));
        assert_eq!(filed, Some((Kind::Live, "MusicBrainz 專輯類型")));
        assert!(!says_live("Detour"));
        assert!(says_live("2019 巡迴演唱會"));
    }

    #[test]
    fn one_tap_follows_a_slower_live_take() {
        // The live take sings album time L at 1.08·L + 31 and is 303 s long against 252.
        let mut align = Alignment::default();
        align.tap(37.0, 1.08 * 37.0 + 31.0);
        for lyric in [52.0, 98.0, 124.0] {
            let heard = align.playback_at(lyric, 252.0, 303.0);
            assert!((heard - (1.08 * lyric + 31.0)).abs() < 1.0, "{lyric}: {heard}");
            assert!((align.lyric_at(heard, 252.0, 303.0) - lyric).abs() < 1e-9);
        }
    }

    #[test]
    fn a_hurried_second_tap_corrects_the_first() {
        let mut align = Alignment::default();
        align.tap(37.0, 72.0);
        align.tap(52.0, 74.0);
        assert_eq!(align.anchors, vec![(52.0, 74.0)]);
    }

    #[test]
    fn long_silences_are_interludes_and_short_ones_keep_the_line() {
        let lines = parse_lrc("[00:20.00]a\n[00:24.00]\n[00:26.00]b\n[00:30.00]\n[00:50.00]c");
        let rows = rows(&lines, 60.0);
        let shown: Vec<(f64, Option<usize>)> = rows.iter().map(|row| (row.start, row.line)).collect();
        assert_eq!(
            shown,
            vec![
                (0.0, None),
                (20.0, Some(0)),
                (26.0, Some(2)),
                (30.0, None),
                (50.0, Some(4))
            ]
        );
        assert_eq!(row_at(&rows, 25.0), Some(1));
        assert_eq!(row_at(&rows, 40.0), Some(3));
    }

    #[test]
    fn cleans_version_words_from_titles() {
        assert_eq!(clean_title("夜行列車 (Live)"), "夜行列車");
        assert_eq!(clean_title("Song - Live at Budokan"), "Song");
        // A video is named by its artist and its song, the song in quotes where it is live.
        assert_eq!(
            song_title("ヨルシカ - LIVE「晴る」", "ヨルシカ / n-buna Official"),
            "晴る"
        );
        assert_eq!(
            song_title("Yorushika - Escape (Live)", "Yorushika - Topic"),
            "Escape"
        );
        assert_eq!(song_title("Song - Live at Budokan", "Someone"), "Song");
        assert_eq!(clean_artist("ヨルシカ / n-buna Official"), "ヨルシカ");
    }
}
