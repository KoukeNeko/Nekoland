//! Lyrics from the services besides LRCLIB, asked for together, each on a thread of its own.
//! They are found by title and artist; a take whose title is in another language is the same
//! song when it is as long as this recording, as a translated title is.
//!
//! None of them is asked to do what it has not said it does: they are the services' own
//! public pages and interfaces, and what a service refuses, or does not have, leaves
//! nothing, not an error.

use std::io::Read;

use base64::Engine;

use crate::lyrics::{
    clean_artist, clean_title, get, get_from, get_headed, get_text, parse_lrc, post_form, post_json,
    song_title, Candidate, Line, MATCH,
};

const NETEASE: &str = "https://music.163.com/api";
const QQ: &str = "https://c.y.qq.com";
const BETTER_LYRICS: &str = "https://lyrics-api.boidu.dev/getLyrics";
const BINI_LYRICS: &str = "https://lrc.red";
const KUGOU_SONGS: &str = "https://mobileservice.kugou.com/api/v3/search/song";
const KUGOU_LYRICS: &str = "https://lyrics.kugou.com";
const YOUTUBE_MUSIC: &str = "https://music.youtube.com/youtubei/v1";
const UNISON: &str = "https://unison.betterlyrics.org/lyrics";
/// The most takes of a song read from each service, each being a request or more.
const TAKES: usize = 3;

/// Every take the services have, in a fixed order, which what the user chose among them is
/// kept by. KuGou is asked last, for it finds a song only by the name it has, which is often
/// the song's own language's, so what the others call it is asked as well.
pub fn all(title: &str, artist: &str, duration: f64) -> Vec<Candidate> {
    let asked = Want::new(title, artist, duration);
    // The video's title may be in the song's own language, which the others find it by.
    let video = youtube_video(&asked);
    let want = &match &video {
        Some(video) => asked.with_aliases(&video.2),
        None => asked,
    };
    let mut first: Vec<Vec<Candidate>> = std::thread::scope(|scope| {
        let sources: [fn(&Want) -> Vec<Candidate>; 4] = [netease, qq, better_lyrics, bini_lyrics];
        let running: Vec<_> = sources
            .into_iter()
            .map(|source| scope.spawn(move || source(want)))
            .collect();
        running
            .into_iter()
            .map(|run| run.join().unwrap_or_default())
            .collect()
    });
    let mut names: Vec<String> = Vec::new();
    for found in first.iter().flatten() {
        let known = |name: &str| {
            [&want.title, &want.cleaned]
                .into_iter()
                .chain(names.iter())
                .any(|known| known.to_lowercase() == name.to_lowercase())
        };
        if !found.title.is_empty() && !known(&found.title) {
            names.push(found.title.clone());
        }
    }
    let names = &names;
    let video = &video;
    let (kugou, unison, youtube) = std::thread::scope(|scope| {
        let kugou = scope.spawn(move || legato(want, names));
        let unison = scope.spawn(move || video.as_ref().map(|video| unison(video)).unwrap_or_default());
        let youtube = scope.spawn(move || video.as_ref().map(|video| youtube(video)).unwrap_or_default());
        (
            kugou.join().unwrap_or_default(),
            unison.join().unwrap_or_default(),
            youtube.join().unwrap_or_default(),
        )
    });
    // Netease, QQ, Better Lyrics, BiniLyrics, then Legato, then Unison and YouTube.
    let mut all: Vec<Candidate> = first.drain(..).flatten().collect();
    all.extend(kugou);
    all.extend(unison);
    all.extend(youtube);
    all
}

/// The song asked for.
#[derive(Clone)]
struct Want {
    /// As it is, and without "(Live)" and the like.
    title: String,
    cleaned: String,
    /// Other names the song has, as the video of it is titled in another language.
    aliases: Vec<String>,
    artist: String,
    duration: f64,
}

impl Want {
    fn new(title: &str, artist: &str, duration: f64) -> Self {
        Self {
            title: title.to_string(),
            cleaned: song_title(title, artist),
            aliases: Vec::new(),
            artist: clean_artist(artist),
            duration,
        }
    }

    /// Whether a take, named `name` by `by` and `seconds` long, is this song: by this artist
    /// and either named alike or as long, or, the artist's name being in another script, both.
    fn is(&self, name: &str, by: &[String], seconds: f64) -> bool {
        // Named alike is the same name, not one that holds the other: "雨晴るる" is not "晴る".
        let name = alike(name);
        let named = !name.is_empty() && self.all_titles().any(|title| alike(title) == name);
        let gap = (seconds - self.duration).abs();
        let long = self.duration > 0.0 && gap <= MATCH;
        let artist = self.artist.to_lowercase();
        let by_artist = artist.is_empty() || by.iter().any(|by| same_artist(&artist, &by.to_lowercase()));
        // An artist whose name is in another script than the one asked for may be the same, as
        // "スイ" is of "suis", but one in the same script that is not alike is another's.
        let other_script = by.iter().any(|by| non_latin(by) != non_latin(&artist));
        // By an artist and as long to the second, a name in another language is the song.
        let exact = self.duration > 0.0 && gap <= 1.0;
        (named && (by_artist || (long && other_script))) || (by_artist && exact)
    }

    /// The same song also known by the names in `name`, a title that may hold several, as
    /// "逃亡 - Escape" does.
    fn with_aliases(&self, name: &str) -> Self {
        let mut aliases: Vec<String> = Vec::new();
        let pieces = name
            .split([' ', '(', ')', '（', '）', '/', '-', '–'])
            .collect::<Vec<_>>();
        // Words and the groups of them between the separators, as a title is both.
        let mut candidates: Vec<String> = vec![name.to_string()];
        let marked = [" - ", " – ", " / ", "(", ")", "（", "）"]
            .iter()
            .fold(name.to_string(), |marked, separator| {
                marked.replace(separator, "\u{1}")
            });
        for part in marked.split('\u{1}') {
            candidates.push(part.trim().to_string());
        }
        candidates.extend(
            pieces
                .iter()
                .filter(|word| !word.is_empty())
                .map(|word| word.to_string()),
        );
        for candidate in candidates {
            let lower = candidate.to_lowercase();
            let known = [&self.title, &self.cleaned]
                .into_iter()
                .chain(aliases.iter())
                .any(|known| known.to_lowercase() == lower);
            if candidate.chars().count() >= 2 && !known {
                aliases.push(candidate);
            }
        }
        Self {
            aliases,
            ..self.clone()
        }
    }

    /// Every title the song is asked for by.
    fn all_titles(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.cleaned).chain(self.aliases.iter())
    }

    /// The same song under another title.
    fn clone_for(&self, title: &str) -> Self {
        Self {
            title: title.to_string(),
            cleaned: song_title(title, &self.artist),
            aliases: Vec::new(),
            artist: self.artist.clone(),
            duration: self.duration,
        }
    }

    /// What to search for in a service that finds by words: the title and the artist, and,
    /// where that finds nothing for the artist's name being longer or in other words than the
    /// service's, the title and the artist's first word.
    fn searches(&self) -> Vec<String> {
        let first = self.artist.split_whitespace().next().unwrap_or_default();
        let mut searches: Vec<String> = Vec::new();
        for title in self.all_titles() {
            for artist in [self.artist.as_str(), first] {
                let search = format!("{title} {artist}").trim().to_string();
                if !searches.contains(&search) {
                    searches.push(search);
                }
            }
        }
        searches
    }

    /// The titles to ask for, the one as it is and the one without version words.
    fn titles(&self) -> Vec<String> {
        let mut titles = vec![self.title.clone()];
        if self.cleaned != self.title && !self.cleaned.is_empty() {
            titles.push(self.cleaned.clone());
        }
        titles
    }
}

/// A song's name as it is compared: without version words, brackets and punctuation, in
/// lower case.
fn alike(name: &str) -> String {
    clean_title(name)
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn non_latin(name: &str) -> bool {
    name.chars().any(|c| c.is_alphabetic() && !c.is_ascii())
}

/// Whether two names are an artist's: one holds the other, or they start with the same word,
/// as "suis from ヨルシカ" and "suis from Yorushika" do.
fn same_artist(a: &str, b: &str) -> bool {
    let first = |name: &str| name.split_whitespace().next().unwrap_or_default().to_string();
    a.contains(b)
        || b.contains(a)
        || (first(a).chars().count() >= 3 && first(a) != "the" && first(a) == first(b))
}

fn text_of(item: &serde_json::Value, key: &str) -> String {
    item.get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Names that come as a list of objects, as `key` of each.
fn names(item: &serde_json::Value, list: &str, key: &str) -> Vec<String> {
    item.get(list)
        .and_then(|list| list.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| Some(entry.get(key)?.as_str()?.to_string()))
        .collect()
}

/// NetEase: the takes of this song, with each word's time where it has them.
fn netease(want: &Want) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    for search in want.searches() {
        if !found.is_empty() {
            break;
        }
        let songs = get(
            &format!("{NETEASE}/search/get"),
            &[("s", search), ("type", "1".into()), ("limit", "10".into())],
        )
        .ok()
        .flatten()
        .and_then(|found| found.pointer("/result/songs")?.as_array().cloned())
        .unwrap_or_default();
        for song in &songs {
            if found.len() >= TAKES {
                break;
            }
            let (Some(id), Some(ms)) = (
                song.get("id").and_then(|id| id.as_i64()),
                song.get("duration").and_then(|d| d.as_f64()),
            ) else {
                continue;
            };
            let seconds = ms / 1000.0;
            if !want.is(&text_of(song, "name"), &names(song, "artists", "name"), seconds)
                || found.iter().any(|c| (c.duration - seconds).abs() < 1.0)
            {
                continue;
            }
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
                    title: text_of(song, "name"),
                    album: song
                        .pointer("/album/name")
                        .and_then(|a| a.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    duration: seconds,
                    synced,
                    plain: vec![],
                });
            }
        }
    }
    found
}

/// QQ Music: the takes of this song, timed by line.
fn qq(want: &Want) -> Vec<Candidate> {
    let referer = Some("https://y.qq.com/");
    let mut found: Vec<Candidate> = Vec::new();
    for search in want.searches() {
        if !found.is_empty() {
            break;
        }
        let songs = get_from(
            &format!("{QQ}/soso/fcgi-bin/client_search_cp"),
            &[("w", search), ("format", "json".into()), ("n", "10".into())],
            referer,
        )
        .ok()
        .flatten()
        .and_then(|found| found.pointer("/data/song/list")?.as_array().cloned())
        .unwrap_or_default();
        for song in &songs {
            if found.len() >= TAKES {
                break;
            }
            let (Some(mid), Some(seconds)) = (
                song.get("songmid").and_then(|m| m.as_str()),
                song.get("interval").and_then(|i| i.as_f64()),
            ) else {
                continue;
            };
            if !want.is(
                &text_of(song, "songname"),
                &names(song, "singer", "name"),
                seconds,
            ) || found.iter().any(|c| (c.duration - seconds).abs() < 1.0)
            {
                continue;
            }
            let lyric = get_from(
                &format!("{QQ}/lyric/fcgi-bin/fcg_query_lyric_new.fcg"),
                &[
                    ("songmid", mid.to_string()),
                    ("format", "json".into()),
                    ("nobase64", "1".into()),
                ],
                referer,
            );
            // Timed to the word where QQ Music has them, which is Portato, else by line.
            let (synced, source) = match qrc_lyrics(song.get("songid").and_then(|id| id.as_i64())) {
                Some(words) => (words, "Better Lyrics Portato"),
                None => (
                    lyric
                        .ok()
                        .flatten()
                        .and_then(|lyric| Some(parse_lrc(lyric.get("lyric")?.as_str()?)))
                        .unwrap_or_default(),
                    "QQ 音樂",
                ),
            };
            if !synced.is_empty() {
                found.push(Candidate {
                    source: source.into(),
                    title: text_of(song, "songname"),
                    album: text_of(song, "albumname"),
                    duration: seconds,
                    synced,
                    plain: vec![],
                });
            }
        }
    }
    found
}

/// A song's QRC from QQ Music, timed to the word.
fn qrc_lyrics(id: Option<i64>) -> Option<Vec<Line>> {
    let answer = post_form(
        &format!("{QQ}/qqmusic/fcgi-bin/lyric_download.fcg"),
        &format!("version=15&miniversion=82&lrctype=4&musicid={}", id?),
        "https://y.qq.com/",
    )
    .ok()??;
    let hex = answer.split("<![CDATA[").nth(1)?.split("]]>").next()?;
    let lines = crate::qrc::parse(&crate::qrc::decode(hex)?);
    (!lines.is_empty()).then_some(lines)
}

/// Better Lyrics: what it has kept of this song, a take of Apple Music's, timed to the word
/// or the line. It answers only for songs it has already, which is all it says it will.
fn better_lyrics(want: &Want) -> Vec<Candidate> {
    for title in want.titles() {
        let lyric = get(
            BETTER_LYRICS,
            &[
                ("s", title),
                ("a", want.artist.clone()),
                ("d", format!("{}", want.duration.round() as i64)),
            ],
        );
        let Some(ttml) = lyric
            .ok()
            .flatten()
            .and_then(|found| Some(found.get("ttml")?.as_str()?.to_string()))
        else {
            continue;
        };
        let (synced, length) = parse_ttml(&ttml);
        if !synced.is_empty() {
            return vec![Candidate {
                source: "Better Lyrics".into(),
                title: String::new(),
                album: String::new(),
                duration: length.unwrap_or(want.duration),
                synced,
                plain: vec![],
            }];
        }
    }
    Vec::new()
}

/// BiniLyrics (lrc.red): searched by title, and by artist where the title finds none, as it
/// has no search for both together.
fn bini_lyrics(want: &Want) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    let queries: Vec<&str> = want
        .all_titles()
        .map(String::as_str)
        .chain([want.artist.as_str()])
        .collect();
    for query in queries {
        if query.is_empty() || !found.is_empty() {
            continue;
        }
        let hits = get(&format!("{BINI_LYRICS}/search.json"), &[("q", query.to_string())])
            .ok()
            .flatten()
            .and_then(|found| found.get("hits")?.as_array().cloned())
            .unwrap_or_default();
        for hit in &hits {
            if found.len() >= TAKES {
                break;
            }
            let (Some(isrc), Some(seconds)) = (
                hit.get("isrc").and_then(|i| i.as_str()),
                hit.get("duration").and_then(|d| d.as_f64()),
            ) else {
                continue;
            };
            if !want.is(&text_of(hit, "title"), &[text_of(hit, "artist")], seconds)
                || found.iter().any(|c| (c.duration - seconds).abs() < 1.0)
            {
                continue;
            }
            let ttml = get_text(&format!("{BINI_LYRICS}/s/{isrc}.ttml"), &[], None);
            let Some((synced, length)) = ttml.ok().flatten().map(|ttml| parse_ttml(&ttml)) else {
                continue;
            };
            if !synced.is_empty() {
                found.push(Candidate {
                    source: "BiniLyrics".into(),
                    title: text_of(hit, "title"),
                    album: text_of(hit, "album"),
                    duration: length.unwrap_or(seconds),
                    synced,
                    plain: vec![],
                });
            }
        }
    }
    found
}

/// Legato, which is KuGou: its lyrics timed to the word where it has them, else by line. Asked
/// by the title as given and by the other names `names` the song has.
fn legato(want: &Want, names: &[String]) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let asked: Vec<Want> = std::iter::once(want.clone_for(&want.cleaned))
        .chain(names.iter().map(|name| want.clone_for(name)))
        .collect();
    for want in &asked {
        let keywords = std::iter::once(want.cleaned.clone()).chain(want.searches());
        for keyword in keywords {
            let songs = get(
                KUGOU_SONGS,
                &[
                    ("keyword", keyword),
                    ("pagesize", "10".into()),
                    ("format", "json".into()),
                    ("showtype", "1".into()),
                ],
            )
            .ok()
            .flatten()
            .and_then(|found| found.pointer("/data/info")?.as_array().cloned())
            .unwrap_or_default();
            for song in &songs {
                if found.len() >= TAKES {
                    break;
                }
                let (hash, seconds) = (
                    text_of(song, "hash"),
                    song.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0),
                );
                // KuGou names the artists in one string, which may be a few.
                let by: Vec<String> = text_of(song, "singername")
                    .split('、')
                    .map(String::from)
                    .collect();
                if hash.is_empty()
                    || seen.contains(&hash)
                    || !want.is(&text_of(song, "songname"), &by, seconds)
                {
                    continue;
                }
                seen.push(hash.clone());
                let Some(synced) = kugou_lyrics(&hash, seconds) else {
                    continue;
                };
                found.push(Candidate {
                    source: "Better Lyrics Legato".into(),
                    title: text_of(song, "songname"),
                    album: text_of(song, "album_name"),
                    duration: seconds,
                    synced,
                    plain: vec![],
                });
            }
            if !found.is_empty() {
                return found;
            }
        }
    }
    found
}

/// The lyrics KuGou has for a recording: in its own word-timed form, else as plain LRC.
fn kugou_lyrics(hash: &str, seconds: f64) -> Option<Vec<Line>> {
    let offered = get(
        &format!("{KUGOU_LYRICS}/search"),
        &[
            ("ver", "1".into()),
            ("man", "yes".into()),
            ("client", "pc".into()),
            ("hash", hash.to_string()),
            ("duration", format!("{}", (seconds * 1000.0) as i64)),
        ],
    )
    .ok()??;
    let offer = offered.get("candidates")?.as_array()?.first()?.clone();
    let download = |format: &str| -> Option<Vec<u8>> {
        let answer = get(
            &format!("{KUGOU_LYRICS}/download"),
            &[
                ("ver", "1".into()),
                ("client", "pc".into()),
                ("id", text_of(&offer, "id")),
                ("accesskey", text_of(&offer, "accesskey")),
                ("fmt", format.into()),
                ("charset", "utf8".into()),
            ],
        )
        .ok()??;
        base64::engine::general_purpose::STANDARD
            .decode(answer.get("content")?.as_str()?)
            .ok()
    };
    if let Some(lines) = download("krc")
        .and_then(|bytes| unlock_krc(&bytes))
        .map(|text| parse_krc(&text))
        .filter(|lines| !lines.is_empty())
    {
        return Some(lines);
    }
    let lines = parse_lrc(&String::from_utf8_lossy(&download("lrc")?));
    (!lines.is_empty()).then_some(lines)
}

/// What KuGou's own lyrics format holds: after four bytes that name it, the text is
/// compressed, then masked by a key it repeats.
const KRC_KEY: [u8; 16] = [
    64, 71, 97, 119, 94, 50, 116, 71, 81, 54, 49, 45, 206, 210, 110, 105,
];

fn unlock_krc(bytes: &[u8]) -> Option<String> {
    let masked: Vec<u8> = bytes
        .get(4..)?
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ KRC_KEY[index % KRC_KEY.len()])
        .collect();
    let mut text = String::new();
    flate2::read::ZlibDecoder::new(masked.as_slice())
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// KuGou's word-timed lines: `[start,length]<offset,length,0>word…`, in milliseconds, the
/// offsets from the line's start.
fn parse_krc(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let Some((head, body)) = raw.strip_prefix('[').and_then(|rest| rest.split_once(']')) else {
            continue;
        };
        let Some(start) = head.split(',').next().and_then(|n| n.parse::<f64>().ok()) else {
            continue;
        };
        if !head.contains(',') {
            continue;
        }
        let mut words: Vec<(f64, String)> = Vec::new();
        let mut rest = body;
        while let Some(open) = rest.find('<') {
            let Some(close) = rest[open..].find('>') else {
                break;
            };
            let stamp: Vec<&str> = rest[open + 1..open + close].split(',').collect();
            let after = &rest[open + close + 1..];
            let end = after.find('<').unwrap_or(after.len());
            if let (Some(at), Some(length)) = (
                stamp.first().and_then(|n| n.parse::<f64>().ok()),
                stamp.get(1).and_then(|n| n.parse::<f64>().ok()),
            ) {
                spread(&mut words, Some((start + at, length, after[..end].to_string())));
            }
            rest = &after[end..];
        }
        push_line(&mut lines, start / 1000.0, words);
    }
    without_credits(lines)
}

/// A song's video on YouTube Music: its id, length and title there.
type Video = (String, f64, String);

const YOUTUBE_CONTEXT: &str =
    r#""context":{"client":{"clientName":"WEB_REMIX","clientVersion":"1.20250101.01.00","hl":"en"}}"#;

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// The video YouTube Music lists for the title and artist that is of this song, which is of
/// the recording it shows, so that its length is this one's.
fn youtube_video(want: &Want) -> Option<Video> {
    let found = post_json(
        &format!("{YOUTUBE_MUSIC}/search?prettyPrint=false"),
        &format!(
            r#"{{{YOUTUBE_CONTEXT},"query":{},"params":"EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D"}}"#,
            json_string(&format!("{} {}", want.cleaned, want.artist))
        ),
    );
    found.ok().flatten().and_then(|found| listed_video(&found, want))
}

/// Unison's, a library its users made and vote on, found by the video: any of the three kinds
/// of lyrics it keeps.
fn unison(video: &Video) -> Vec<Candidate> {
    let answer = get_headed(
        UNISON,
        &[("v", video.0.clone())],
        &format!("x-key-id: nekoland-{}", env!("CARGO_PKG_VERSION")),
    );
    let Some(data) = answer
        .ok()
        .flatten()
        .and_then(|answer| answer.get("data").cloned())
    else {
        return Vec::new();
    };
    let lyrics = text_of(&data, "lyrics");
    let (synced, plain) = match text_of(&data, "format").as_str() {
        "ttml" => (parse_ttml(&lyrics).0, Vec::new()),
        "lrc" => (parse_lrc(&lyrics), Vec::new()),
        _ => (Vec::new(), plain_lines(&lyrics)),
    };
    if synced.is_empty() && plain.is_empty() {
        return Vec::new();
    }
    vec![Candidate {
        source: "Unison".into(),
        title: video.2.clone(),
        album: String::new(),
        duration: video.1,
        synced,
        plain,
    }]
}

fn plain_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect()
}

/// YouTube Music's lyrics of a video, as plain lines.
fn youtube(video: &Video) -> Vec<Candidate> {
    let next = post_json(
        &format!("{YOUTUBE_MUSIC}/next?prettyPrint=false"),
        &format!(r#"{{{YOUTUBE_CONTEXT},"videoId":{}}}"#, json_string(&video.0)),
    );
    let Some(browse) = next.ok().flatten().and_then(|next| lyrics_page(&next)) else {
        return Vec::new();
    };
    let page = post_json(
        &format!("{YOUTUBE_MUSIC}/browse?prettyPrint=false"),
        &format!(r#"{{{YOUTUBE_CONTEXT},"browseId":{}}}"#, json_string(&browse)),
    );
    let plain = plain_lines(
        &page
            .ok()
            .flatten()
            .and_then(|page| shelf_text(&page))
            .unwrap_or_default(),
    );
    if plain.is_empty() {
        return Vec::new();
    }
    vec![Candidate {
        source: "YouTube".into(),
        title: video.2.clone(),
        album: String::new(),
        duration: video.1,
        synced: vec![],
        plain,
    }]
}

/// The video id and length of the song in a search's results that is this one.
fn listed_video(found: &serde_json::Value, want: &Want) -> Option<(String, f64, String)> {
    let mut items = Vec::new();
    collect(found, "musicResponsiveListItemRenderer", &mut items);
    items.into_iter().find_map(|item| {
        let id = item.pointer("/playlistItemData/videoId")?.as_str()?.to_string();
        let columns = item.get("flexColumns")?.as_array()?;
        let runs: Vec<String> = columns
            .iter()
            .flat_map(|column| {
                column
                    .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")
                    .and_then(|runs| runs.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|run| Some(run.get("text")?.as_str()?.to_string()))
            })
            .collect();
        let name = runs.first()?.clone();
        // "3:21" among the runs is the length.
        let seconds = runs.iter().find_map(|run| {
            let (minutes, secs) = run.split_once(':')?;
            Some(minutes.parse::<f64>().ok()? * 60.0 + secs.parse::<f64>().ok()?)
        })?;
        // A title may name the song twice, in two languages, as "逃亡 - Escape" does.
        let named = want.is(&name, &runs[1..], seconds)
            || name.split(" - ").any(|part| want.is(part, &runs[1..], seconds));
        named.then_some((id, seconds, name))
    })
}

/// The id of the page of lyrics in a video's tabs.
fn lyrics_page(next: &serde_json::Value) -> Option<String> {
    let mut tabs = Vec::new();
    collect(next, "tabRenderer", &mut tabs);
    tabs.into_iter().find_map(|tab| {
        let id = tab.pointer("/endpoint/browseEndpoint/browseId")?.as_str()?;
        id.starts_with("MPLYt").then(|| id.to_string())
    })
}

/// The lyrics text of a page, which is in one description shelf.
fn shelf_text(page: &serde_json::Value) -> Option<String> {
    let mut shelves = Vec::new();
    collect(page, "musicDescriptionShelfRenderer", &mut shelves);
    let runs = shelves.first()?.pointer("/description/runs")?.as_array()?;
    Some(runs.iter().filter_map(|run| run.get("text")?.as_str()).collect())
}

/// Everything under `key`, wherever in `value` it is.
fn collect(value: &serde_json::Value, key: &str, out: &mut Vec<serde_json::Value>) {
    match value {
        serde_json::Value::Object(map) => {
            for (name, inner) in map {
                if name == key {
                    out.push(inner.clone());
                }
                collect(inner, key, out);
            }
        }
        serde_json::Value::Array(list) => list.iter().for_each(|inner| collect(inner, key, out)),
        _ => {}
    }
}

/// The lines of a TTML, with each word's time when more than one is timed in a line, and the
/// length it says the recording is.
pub fn parse_ttml(text: &str) -> (Vec<Line>, Option<f64>) {
    // What the line being read has so far: where it begins, each character with its time,
    // how many words were timed, and where the last one ended.
    struct Reading {
        begin: f64,
        chars: Vec<(f64, String)>,
        timed: usize,
        last: f64,
    }
    let length = text
        .find("<body")
        .and_then(|at| attribute(&text[at..text[at..].find('>')? + at], "dur"))
        .and_then(|dur| clock(&dur));
    let mut lines = Vec::new();
    let mut reading: Option<Reading> = None;
    // The spans open, each with its times when it has them.
    let mut spans: Vec<Option<(f64, f64)>> = Vec::new();
    let mut at = 0;
    while at < text.len() {
        if text[at..].starts_with('<') {
            let Some(close) = text[at..].find('>') else {
                break;
            };
            let tag = &text[at + 1..at + close];
            at += close + 1;
            let (closing, inner) = match tag.strip_prefix('/') {
                Some(inner) => (true, inner),
                None => (false, tag),
            };
            let name = inner
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or_default();
            let empty = tag.ends_with('/');
            match (closing, name) {
                (false, "p") if !empty => {
                    let begin = attribute(tag, "begin").and_then(|t| clock(&t)).unwrap_or(0.0);
                    reading = Some(Reading {
                        begin,
                        chars: Vec::new(),
                        timed: 0,
                        last: begin,
                    });
                    spans.clear();
                }
                (true, "p") => {
                    if let Some(done) = reading.take() {
                        let mut words = done.chars;
                        push_line(&mut lines, done.begin, std::mem::take(&mut words)).then(|| {
                            if done.timed < 2 {
                                if let Some(line) = lines.last_mut() {
                                    line.words = None;
                                }
                            }
                        });
                    }
                }
                (false, "span") if !empty => {
                    let times = attribute(tag, "begin")
                        .and_then(|t| clock(&t))
                        .zip(attribute(tag, "end").and_then(|t| clock(&t)));
                    spans.push(times);
                }
                (true, "span") => {
                    spans.pop();
                }
                _ => {}
            }
        } else {
            let end = text[at..].find('<').map_or(text.len(), |n| at + n);
            let node = decode(&text[at..end]);
            at = end;
            let Some(line) = reading.as_mut() else {
                continue;
            };
            match spans.iter().rev().find_map(|span| *span) {
                Some((begin, end)) => {
                    let count = node.chars().count().max(1) as f64;
                    for (index, c) in node.chars().enumerate() {
                        let c = if c.is_whitespace() { ' ' } else { c };
                        line.chars
                            .push((begin + (end - begin) * index as f64 / count, c.to_string()));
                    }
                    line.timed += 1;
                    line.last = end;
                }
                None => {
                    // Between words, a space; a word outside any span is the line's own.
                    for c in node.chars() {
                        if c.is_whitespace() {
                            if line.chars.last().is_some_and(|(_, last)| last != " ") {
                                line.chars.push((line.last, " ".into()));
                            }
                        } else {
                            line.chars.push((line.last, c.to_string()));
                        }
                    }
                }
            }
        }
    }
    (lines, length)
}

/// The value of attribute `name` in a tag.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(found) = tag[from..].find(&format!("{name}=")) {
        let at = from + found;
        let boundary = tag[..at].chars().last().is_none_or(|c| c.is_whitespace());
        let rest = &tag[at + name.len() + 1..];
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'');
        if let (true, Some(quote)) = (boundary, quote) {
            let value = &rest[1..];
            return value.find(quote).map(|end| value[..end].to_string());
        }
        from = at + name.len() + 1;
    }
    None
}

/// A TTML time in seconds: "12.5", "1:02.5", "1:02:03.5", with or without a trailing "s".
fn clock(time: &str) -> Option<f64> {
    time.trim_end_matches('s')
        .split(':')
        .try_fold(0.0, |total, part| Some(total * 60.0 + part.parse::<f64>().ok()?))
}

fn decode(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let Some(end) = rest[at..].find(';') else {
            rest = &rest[at..];
            break;
        };
        let entity = &rest[at + 1..at + end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(&rest[at..=at + end]),
        }
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    out
}

/// A line from its characters and when it starts, unless nothing is sung in it. Returns
/// whether it was kept.
fn push_line(lines: &mut Vec<Line>, time: f64, mut words: Vec<(f64, String)>) -> bool {
    // The text is what is sung, so spaces at its ends are not part of it, nor of the timing.
    let skip = words.iter().take_while(|(_, c)| c.trim().is_empty()).count();
    words.drain(..skip);
    while words.last().is_some_and(|(_, c)| c.trim().is_empty()) {
        words.pop();
    }
    let text: String = words.iter().map(|(_, c)| c.as_str()).collect();
    if text.is_empty() {
        return false;
    }
    lines.push(Line {
        time,
        text,
        words: Some(words),
    });
    true
}

/// The credits put in the first lines ("作词 : …") are not sung.
fn without_credits(mut lines: Vec<Line>) -> Vec<Line> {
    let credits = lines
        .iter()
        .take_while(|line| line.text.contains(" : ") || line.text.contains('：'))
        .count();
    lines.drain(..credits);
    lines
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
    without_credits(lines)
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
    fn reads_ttml_by_word_and_by_line() {
        let ttml = r#"<tt><body dur="3:53.713"><div><p begin="9.731" end="12.1"><span begin="9.731" end="9.927">The</span> <span begin="9.927" end="10.284">club &amp; bar</span></p><p begin="1:02.5" end="1:05"><span begin="1:02.5" end="1:05">One timed span</span></p><p begin="70" end="72">Plain line</p></div></body></tt>"#;
        let (lines, length) = parse_ttml(ttml);
        assert_eq!(length, Some(233.713));
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "The club & bar");
        let words = lines[0].words.as_ref().unwrap();
        assert_eq!(words.len(), 14);
        assert!((words[5].0 - (9.927 + 0.357 / 10.0)).abs() < 1e-9);
        assert!((lines[1].time - 62.5).abs() < 1e-9);
        assert!(lines[1].words.is_none());
        assert_eq!((lines[2].text.as_str(), lines[2].time), ("Plain line", 70.0));
    }

    #[test]
    fn reads_kugou_lines_after_unlocking_them() {
        use std::io::Write;
        let krc = "[id:$00000000]\n[ar:someone]\n[0,1000]<0,500,0>作词 : <500,500,0>x\n[12000,3000]<0,800,0>He<800,400,0>llo<1200,600,0>世界";
        let mut packed = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        packed.write_all(krc.as_bytes()).unwrap();
        let mut bytes = b"krc1".to_vec();
        bytes.extend(
            packed
                .finish()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, byte)| byte ^ KRC_KEY[i % KRC_KEY.len()]),
        );
        let lines = parse_krc(&unlock_krc(&bytes).unwrap());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello世界");
        assert!((lines[0].time - 12.0).abs() < 1e-9);
        let words = lines[0].words.as_ref().unwrap();
        assert!((words[2].0 - 12.8).abs() < 1e-9);
    }

    /// Asks the services, which a test of the code does not: run it to see what each says.
    #[test]
    #[ignore = "asks the services"]
    fn live_sources() {
        for (title, artist, seconds) in [
            ("Around The Stars", "suis from Yorushika", 201.0),
            ("Finale.", "eill", 242.0),
            ("Shape of You", "Ed Sheeran", 233.7),
            ("Escape", "Yorushika", 287.0),
            ("ヨルシカ - LIVE「晴る」", "ヨルシカ / n-buna Official", 278.8),
            // Nobody has this, whatever it is as long as.
            ("夜行列車", "月台樂團", 252.0),
        ] {
            let want = Want::new(title, artist, seconds);
            let sources: [(&str, fn(&Want) -> Vec<Candidate>); 6] = [
                ("netease", netease),
                ("qq", qq),
                ("better", better_lyrics),
                ("bini", bini_lyrics),
                ("legato", |want| {
                    legato(want, &["フィナーレ。".to_string(), "星めぐる詩".to_string()])
                }),
                ("youtube", |want| {
                    youtube_video(want)
                        .map(|video| youtube(&video))
                        .unwrap_or_default()
                }),
            ];
            for (name, source) in sources {
                let found = source(&want);
                let shown: Vec<String> = found
                    .iter()
                    .map(|c| {
                        format!(
                            "{:.0}s [{}] {} lines{} plain {}",
                            c.duration,
                            c.title,
                            c.synced.len(),
                            if c.synced.iter().any(|l| l.words.is_some()) {
                                " word-timed"
                            } else {
                                ""
                            },
                            c.plain.len()
                        )
                    })
                    .collect();
                println!("{title:>18} | {name:<8} {shown:?}");
            }
            let together = all(title, artist, seconds);
            let sources: Vec<&str> = together.iter().map(|c| c.source.as_str()).collect();
            println!("{title:>18} | all      {sources:?}");
        }
    }

    #[test]
    fn a_title_in_another_language_is_the_song_when_as_long() {
        let want = Want::new("Around The Stars", "suis from Yorushika", 201.0);
        let by = ["suis from ヨルシカ".to_string()];
        assert!(want.is("星めぐる詩", &by, 201.8));
        assert!(!want.is("星めぐる詩", &by, 230.0));
        // A song whose name holds the one asked for is another.
        let live = Want::new("ヨルシカ - LIVE「晴る」", "ヨルシカ / n-buna Official", 278.8);
        assert_eq!(live.cleaned, "晴る");
        assert!(!live.is("雨晴るる", &["ヨルシカ".to_string()], 277.0));
        assert!(live.is("晴る", &["ヨルシカ".to_string()], 259.0));
        // Another artist's song of the same name is not it.
        assert!(!want.is("Around the Stars", &["Brank".to_string()], 266.0));
        // The artist in another script is the song only if its name and length also fit.
        assert!(want.is("Around The Stars (Live)", &["スイ".to_string()], 201.0));
        // Another song of the same name by an artist in the same script is not it.
        let want = Want::new("夜行列車", "月台樂團", 252.0);
        assert!(!want.is("夜行列車", &["我々楽".to_string()], 254.0));
    }
}
