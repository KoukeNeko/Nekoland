//! Lyrics for what plays: fetched from LRCLIB, chosen by the recording's length, and
//! mapped onto playback time, by the user's taps when the only lyrics are another take's.
//!
//! A live take and the album take share a title but not a length, an intro or a pace, so
//! lyrics are chosen by length, never by name, and other takes' lyrics are not played on a guess.

use std::process::Command;

use serde::{Deserialize, Serialize};

const LRCLIB: &str = "https://lrclib.net/api";
const NETEASE: &str = "https://music.163.com/api";
/// How many sources a song's lyrics were asked of; a song kept from fewer is asked again.
pub const SOURCES: u8 = 2;
/// The most takes of a song read from NetEase, each being a request.
const NETEASE_TAKES: usize = 3;
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
            ra.0.cmp(&rb.0).then(words).then_with(|| {
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

/// Every set of lyrics LRCLIB and NetEase have for this song: LRCLIB's first, the one for this
/// length at the head of them, then NetEase's, which are often timed word by word. NetEase
/// being down does not stop the rest.
pub fn fetch(title: &str, artist: &str, album: &str, duration: f64) -> Result<Vec<Candidate>, Missing> {
    let from_lrclib = fetch_lrclib(title, artist, album, duration);
    let from_netease = fetch_netease(title, artist);
    match from_lrclib {
        Ok(mut found) => {
            found.extend(from_netease);
            Ok(found)
        }
        Err(Missing::NotFound) if !from_netease.is_empty() => Ok(from_netease),
        Err(missing) => Err(missing),
    }
}

fn fetch_lrclib(title: &str, artist: &str, album: &str, duration: f64) -> Result<Vec<Candidate>, Missing> {
    let title = clean_title(title);
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
fn clean_title(title: &str) -> String {
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
fn get(url: &str, query: &[(&str, String)]) -> Result<Option<serde_json::Value>, Missing> {
    let agent = concat!("Nekoland/", env!("CARGO_PKG_VERSION"));
    let mut args = vec![
        "-s".to_string(),
        "-A".into(),
        agent.into(),
        "--max-time".into(),
        "6".into(),
        "-G".into(),
        "-w".into(),
        "\n%{http_code}".into(),
        url.to_string(),
    ];
    for (key, value) in query {
        args.push("--data-urlencode".into());
        args.push(format!("{key}={value}"));
    }
    let output = Command::new("curl")
        .args(&args)
        .output()
        .map_err(|_| Missing::Unreachable)?;
    let body = String::from_utf8_lossy(&output.stdout);
    let (json, code) = body.rsplit_once('\n').ok_or(Missing::Unreachable)?;
    match code.trim() {
        "200" => Ok(serde_json::from_str(json).ok()),
        "404" => Ok(None),
        _ => Err(Missing::Unreachable),
    }
}

/// The takes of this song NetEase has lyrics for, with each word's time where it has them.
fn fetch_netease(title: &str, artist: &str) -> Vec<Candidate> {
    let title = clean_title(title);
    let search = get(
        &format!("{NETEASE}/search/get"),
        &[
            ("s", format!("{title} {artist}")),
            ("type", "1".into()),
            ("limit", "10".into()),
        ],
    );
    let songs = search
        .ok()
        .flatten()
        .and_then(|found| found.pointer("/result/songs")?.as_array().cloned())
        .unwrap_or_default();
    let lower = |text: &str| text.to_lowercase();
    let (want_title, want_artist) = (lower(&title), lower(artist));
    let mut found: Vec<Candidate> = Vec::new();
    for song in &songs {
        if found.len() >= NETEASE_TAKES {
            break;
        }
        let name = lower(song.get("name").and_then(|n| n.as_str()).unwrap_or_default());
        let by: Vec<String> = song
            .get("artists")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
            .filter_map(|a| Some(lower(a.get("name")?.as_str()?)))
            .collect();
        // The same song by the same artist, not another that shares a word of either.
        let same_title = name.contains(&want_title) || want_title.contains(&name);
        let same_artist = want_artist.is_empty()
            || by
                .iter()
                .any(|by| by.contains(&want_artist) || want_artist.contains(by.as_str()));
        let (Some(id), Some(ms)) = (
            song.get("id").and_then(|id| id.as_i64()),
            song.get("duration").and_then(|d| d.as_f64()),
        ) else {
            continue;
        };
        if !same_title || !same_artist || found.iter().any(|c| (c.duration - ms / 1000.0).abs() < 1.0) {
            continue;
        }
        let album = song
            .pointer("/album/name")
            .and_then(|a| a.as_str())
            .unwrap_or_default();
        let lyric = get(
            &format!("{NETEASE}/song/lyric"),
            &[("id", id.to_string()), ("lv", "1".into()), ("yv", "1".into())],
        );
        let Some(lyric) = lyric.ok().flatten() else {
            continue;
        };
        let text = |pointer: &str| {
            lyric
                .pointer(pointer)
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let mut synced = parse_yrc(&text("/yrc/lyric"));
        if synced.is_empty() {
            synced = parse_lrc(&text("/lrc/lyric"));
        }
        if !synced.is_empty() {
            found.push(Candidate {
                source: "網易雲音樂".into(),
                album: album.to_string(),
                duration: ms / 1000.0,
                synced,
                plain: vec![],
            });
        }
    }
    found
}

/// A word's characters, each starting its share of the word's length after the word's start.
fn spread(words: &mut Vec<(f64, String)>, word: Option<(f64, f64, String)>) {
    let Some((at, length, said)) = word else {
        return;
    };
    let count = said.chars().count().max(1) as f64;
    for (index, c) in said.chars().enumerate() {
        words.push(((at + length * index as f64 / count) / 1000.0, c.to_string()));
    }
}

/// NetEase's word-timed lyrics: `[start,length](start,length,0)word…` a line, in milliseconds,
/// each word's time spread over its characters, as the rest of the lyrics keep them.
pub fn parse_yrc(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let Some(rest) = raw.strip_prefix('[') else {
            continue;
        };
        let Some((head, body)) = rest.split_once(']') else {
            continue;
        };
        let Some(start) = head.split(',').next().and_then(|n| n.parse::<f64>().ok()) else {
            continue;
        };
        let mut words: Vec<(f64, String)> = Vec::new();
        // The word being read: its start, its length and what it says so far.
        let mut word: Option<(f64, f64, String)> = None;
        let chars: Vec<char> = body.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '(' {
                if let Some(close) = chars[i..].iter().position(|&c| c == ')') {
                    let inner: String = chars[i + 1..i + close].iter().collect();
                    let stamp: Vec<&str> = inner.split(',').collect();
                    if stamp.len() == 3 && stamp.iter().all(|n| n.parse::<u32>().is_ok()) {
                        spread(&mut words, word.take());
                        word = Some((
                            stamp[0].parse().unwrap_or(0.0),
                            stamp[1].parse().unwrap_or(0.0),
                            String::new(),
                        ));
                        i += close + 1;
                        continue;
                    }
                }
            }
            if let Some((_, _, said)) = word.as_mut() {
                said.push(chars[i]);
            }
            i += 1;
        }
        spread(&mut words, word);
        // The text is what is sung, so leading spaces are not part of it, nor of the timing.
        let skip = words.iter().take_while(|(_, c)| c.trim().is_empty()).count();
        words.drain(..skip);
        while words.last().is_some_and(|(_, c)| c.trim().is_empty()) {
            words.pop();
        }
        let text: String = words.iter().map(|(_, c)| c.as_str()).collect();
        if !text.is_empty() {
            lines.push(Line {
                time: start / 1000.0,
                text,
                words: Some(words),
            });
        }
    }
    // The credits NetEase puts in the first lines ("作词 : …") are not sung.
    let credits = lines
        .iter()
        .take_while(|line| line.text.contains(" : ") || line.text.contains('：'))
        .count();
    lines.drain(..credits);
    lines
}

fn candidate(item: &serde_json::Value) -> Option<Candidate> {
    let text = |key: &str| item.get(key).and_then(|v| v.as_str()).unwrap_or_default();
    let c = Candidate {
        source: lrclib_name(),
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
    fn reads_word_timed_lyrics() {
        let yrc = "[0,1000](0,1000,0) 作词 : someone\n[12040,3000](12040,800,0)Hel(12840,400,0)lo (13240,600,0)世界";
        let lines = parse_yrc(yrc);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello 世界");
        assert!((lines[0].time - 12.04).abs() < 1e-9);
        let words = lines[0].words.as_ref().unwrap();
        assert_eq!(words.len(), 8);
        // "Hel" takes 0.8 s over three characters, the next word starting where it ends.
        assert!((words[1].0 - (12.04 + 0.8 / 3.0)).abs() < 1e-9);
        assert!((words[3].0 - 12.84).abs() < 1e-9);
    }

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
    }
}
