//! What plays, in the Dock: the player read on a thread, where it is in what it plays run on
//! from a clock between readings, its lyrics, and the card they are read in.

use std::{
    collections::{HashMap, VecDeque},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use cairo::{Context, ImageSurface};
use smithay::utils::{Logical, Point, Rectangle, Size};

use serde::{Deserialize, Serialize};

use crate::{
    lyrics::{self, Alignment, Candidate, Kind, Missing, Row},
    stacks,
    system::{self, Player},
    text::{self, Picture},
};

type Rgb = (f64, f64, f64);

/// The Dock's media tile, as wide at the Dock's standard art: the cover, two lines of text,
/// and the three buttons, the last one's padding sticking into the Dock's.
pub const TILE: i32 = 296;
const TEXT_X: f64 = 52.0;
const TEXT_WIDTH: f64 = 156.0;
const BUTTONS: [(Part, f64, f64); 3] = [
    (Part::Previous, 214.0, 28.0),
    (Part::PlayPause, 242.0, 32.0),
    (Part::Next, 274.0, 28.0),
];
const READ_EVERY: Duration = Duration::from_secs(1);
/// A position read this far from the clock's is a jump, a seek; nearer, the clock is kept,
/// as the readings come late by however long the bus took.
const JUMP: f64 = 1.5;
/// After seeking from here, readings from before the player got there are ignored this long.
const SEEK_SETTLE: Duration = Duration::from_millis(1500);
/// Lyrics that could not be asked for are asked for again after this.
const RETRY: Duration = Duration::from_secs(60);
/// The Dock's line of lyrics: how far one moves out and in, and when the next comes.
const LINE_RISE: f64 = 21.0;
const LINE_OUT: f64 = 0.2;
const LINE_IN_AFTER: f64 = 0.12;
const LINE_FONT: f64 = 16.0;
const LINE_FONT_MIN: f64 = 13.0;
const PAUSED_COVER: f64 = 0.88;
const PRESSED: f64 = 0.84;

/// Springs as stiffness and damping, for `motion::swing`'s rate and damping ratio.
fn spring(stiffness: f64, damping: f64) -> (f64, f64) {
    (stiffness.sqrt(), damping / (2.0 * stiffness.sqrt()))
}

/// How far a spring from 0 has come to 1 `t` seconds after it was let go.
fn spring_at(t: f64, (rate, ratio): (f64, f64)) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if ratio >= 1.0 {
        return 1.0 - (1.0 + rate * t) * (-rate * t).exp();
    }
    let damped = rate * (1.0 - ratio * ratio).sqrt();
    1.0 - (-ratio * rate * t).exp() * ((damped * t).cos() + ratio * rate / damped * (damped * t).sin())
}

fn ease_in(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * t
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    /// The cover or the text, which open the lyrics.
    Cover,
    Previous,
    PlayPause,
    Next,
}

/// What is at `x` across the tile, drawn with art `art` big.
pub fn part_at(x: f64, art: f64) -> Option<Part> {
    let x = x * 40.0 / art;
    if (0.0..TEXT_X + TEXT_WIDTH).contains(&x) {
        return Some(Part::Cover);
    }
    BUTTONS
        .iter()
        .find(|(_, at, width)| x >= *at && x < at + width)
        .map(|(part, ..)| *part)
}

/// What the Dock's tile shows, all of it compared to see whether to draw it again.
#[derive(Clone, PartialEq, Debug)]
pub struct Face {
    upper: String,
    /// The line under it, in its size in quarter points, or the dots of an interlude, lit up
    /// to how many.
    lower: (String, u16, Option<u8>),
    /// The line before it on its way out.
    old: Option<(String, u16, Option<u8>)>,
    /// Where each is, in quarter points under its place, and how solid in hundredths.
    moving: (i16, u8, i16, u8),
    playing: bool,
    /// The cover's size, in thousandths, and whether its picture is read yet.
    cover: (u16, bool),
    /// The three bars on the cover's corner, how high in twentieths: they move while it
    /// plays and fold away as it pauses.
    bars: [u8; 3],
    /// On a Dock at a side only the cover shows, the rest being in the card.
    pub(crate) side: bool,
    /// A new song's cover coming in: how far aside in quarter points, how solid in
    /// hundredths and how big in thousandths.
    arrival: (i16, u8, u16),
    /// A line too long for the Dock even at its smallest, slid along: by how many quarter
    /// points, and whether it fades at its left and right.
    slide: Option<(u16, bool, bool)>,
    pressed: Option<Part>,
}

/// The Dock's line of lyrics changing: what it shows, what it showed, and since when.
struct Swap {
    id: String,
    shown: (String, f64, Option<u8>),
    old: Option<(String, f64, Option<u8>)>,
    since: Instant,
    /// 1 when the line comes from below, -1 when back from above, as after seeking back.
    way: f64,
    /// The first line of a song just come, which fades in rather than taking over.
    fresh: bool,
    /// How much wider than the Dock's room the line is at its size.
    over: f64,
}

/// One set of lyrics, chosen for what plays, and how it is mapped onto it.
pub struct Lyrics<'a> {
    pub found: &'a [Candidate],
    pub chosen: usize,
    /// Whether they are the recording's own, matching its length.
    pub matched: bool,
    pub alignment: Option<&'a Alignment>,
    pub length: f64,
    /// Shown as plain text, as the user chose.
    pub plain: bool,
    /// Whether MusicBrainz files the album as live, when it was asked.
    pub release_live: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Timed to this recording.
    Synced,
    /// Another take's, mapped onto this one by the user's taps.
    Aligned,
    /// Another take's, not mapped: shown still, to be tapped.
    Unaligned,
    /// Without times.
    Plain,
}

impl Lyrics<'_> {
    pub fn candidate(&self) -> &Candidate {
        &self.found[self.chosen]
    }

    pub fn mode(&self) -> Mode {
        let candidate = self.candidate();
        if candidate.synced.is_empty() || self.plain {
            Mode::Plain
        } else if self.matched {
            Mode::Synced
        } else if self
            .alignment
            .is_some_and(|alignment| !alignment.anchors.is_empty())
        {
            Mode::Aligned
        } else {
            Mode::Unaligned
        }
    }

    /// The lyrics' time at playback time `at`, when they follow playback.
    pub fn lyric_time(&self, at: f64) -> Option<f64> {
        let alignment = self.alignment.cloned().unwrap_or_default();
        match self.mode() {
            Mode::Synced => Some(at - alignment.offset),
            Mode::Aligned => Some(alignment.lyric_at(at, self.candidate().duration, self.length)),
            Mode::Unaligned | Mode::Plain => None,
        }
    }

    pub fn playback_time(&self, lyric: f64) -> f64 {
        let alignment = self.alignment.cloned().unwrap_or_default();
        match self.mode() {
            Mode::Aligned => alignment.playback_at(lyric, self.candidate().duration, self.length),
            _ => lyric + alignment.offset,
        }
    }

    /// Where in the lyrics an unaligned take may be at `at`: the extra length may be an
    /// intro or a slower pace, so halfway between what each would make it.
    pub fn guess(&self, at: f64) -> f64 {
        let lyrics = self.candidate().duration;
        let intro = at - (self.length - lyrics).max(0.0);
        let paced = if self.length > 0.0 {
            at * lyrics / self.length
        } else {
            at
        };
        (intro + paced) / 2.0
    }

    pub fn rows(&self) -> Vec<Row> {
        let candidate = self.candidate();
        if candidate.synced.is_empty() {
            return (0..candidate.plain.len())
                .map(|index| Row {
                    start: 0.0,
                    line: Some(index),
                    end: 0.0,
                })
                .collect();
        }
        if matches!(self.mode(), Mode::Unaligned | Mode::Plain) {
            // Shown still, without the interludes their timing would make.
            return (candidate.synced.iter().enumerate())
                .filter(|(_, line)| !line.text.is_empty())
                .map(|(index, line)| Row {
                    start: line.time,
                    line: Some(index),
                    end: line.time,
                })
                .collect();
        }
        lyrics::rows(&candidate.synced, candidate.duration)
    }

    pub fn text(&self, row: &Row) -> Option<&str> {
        let candidate = self.candidate();
        let index = row.line?;
        Some(if candidate.synced.is_empty() {
            &candidate.plain[index]
        } else {
            &candidate.synced[index].text
        })
    }

    /// How the version is named: by what its album says it is.
    pub fn version(&self) -> &'static str {
        version(self.candidate())
    }

    /// What this recording is judged to be, and on what.
    pub fn kind(&self, player: &Player) -> Option<(Kind, &'static str)> {
        lyrics::recording_kind(
            &player.title,
            &player.album,
            self.length,
            self.found,
            self.release_live,
        )
    }
}

fn version(candidate: &Candidate) -> &'static str {
    match candidate.kind() {
        Kind::Live => "LIVE 版",
        Kind::Album => "專輯版",
    }
}

/// Which lyrics the user chose for a song, over those chosen by length.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Choice {
    Candidate(usize),
    Plain,
}

/// What is kept of a song between sessions: its lyrics, and how the user set them.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Song {
    /// None where no source has any.
    found: Option<Vec<Candidate>>,
    /// How many sources were asked, as `lyrics::SOURCES` counted them then.
    #[serde(default)]
    sources: u8,
    #[serde(default)]
    release_live: Option<bool>,
    /// When they were asked for, in seconds since 1970.
    asked: u64,
    #[serde(default)]
    alignment: Alignment,
    #[serde(default)]
    choice: Option<Choice>,
}

/// Songs whose lyrics are kept; the ones asked for longest ago go first.
const KEPT_SONGS: usize = 300;
/// Lyrics not found are asked for again after this many seconds, as they may be added.
const NOT_FOUND_FOR: u64 = 24 * 3600;

fn seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn cache_path() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join(".cache")))?;
    Some(dir.join("nekoland").join("lyrics.json"))
}

fn read_cache() -> HashMap<String, Song> {
    let Some(path) = cache_path() else {
        return HashMap::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!("{}: {err}", path.display());
            HashMap::new()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(err) => {
            tracing::warn!("{}: {err}", path.display());
            HashMap::new()
        }
    }
}

/// What a song is known by: its length too, as a live take shares its album take's name.
pub fn song_key(player: &Player) -> String {
    format!(
        "{}\n{}\n{}",
        player.title,
        player.artist,
        player.length.map_or(0, |length| length.round() as i64)
    )
}

pub struct NowPlaying {
    found: Receiver<(Option<Player>, Option<f64>)>,
    pub player: Option<Player>,
    /// Where playback was, and when: it runs on from there while playing.
    clock: Option<(f64, Instant)>,
    seeked: Option<Instant>,
    /// Counts jumps in playback, as a seek, after which unaligned lyrics are scrolled to
    /// where they may be.
    pub jumps: u32,
    /// The lyrics found for each song, and how the user set them, kept on disk.
    songs: HashMap<String, Song>,
    asking: Option<(String, Receiver<Result<(Vec<Candidate>, Option<bool>), Missing>>)>,
    unreachable: Option<(String, Instant)>,
    cover: Option<(String, Option<(ImageSurface, [Rgb; 3])>)>,
    cover_read: Option<(String, stacks::Reading)>,
    swap: Option<Swap>,
    last_row: Option<usize>,
    cover_scale: (f64, f64),
    /// Counts the changes to the lyrics kept, which the card is told of when they come.
    pub revision: u64,
    /// When another song came, and from which way: -1 after going back to the previous.
    arrived: Option<(Instant, f64)>,
    went_back: bool,
    pub press: Option<(Part, Instant)>,
    since: Instant,
    /// The pieces a line too long for the Dock is shown in, each in its font size.
    pieces: Option<(String, Vec<(String, f64, f64)>)>,
}

impl NowPlaying {
    pub fn start() -> Option<Self> {
        let (send, found) = mpsc::channel();
        std::thread::Builder::new()
            .name("nekoland-media".into())
            .spawn(move || loop {
                let player = system::read_player();
                let position = player
                    .as_ref()
                    .and_then(|player| system::read_position(&player.bus));
                if send.send((player, position)).is_err() {
                    break;
                }
                text::redraw_soon();
                std::thread::sleep(READ_EVERY);
            })
            .inspect_err(|err| tracing::warn!("media: {err}"))
            .ok()?;
        Some(Self {
            found,
            player: None,
            clock: None,
            seeked: None,
            jumps: 0,
            songs: read_cache(),
            asking: None,
            unreachable: None,
            cover: None,
            cover_read: None,
            swap: None,
            last_row: None,
            cover_scale: (1.0, 0.0),
            arrived: None,
            went_back: false,
            revision: 0,
            press: None,
            since: Instant::now(),
            pieces: None,
        })
    }

    /// Where playback is now, in seconds.
    pub fn position(&self, now: Instant) -> Option<f64> {
        let (at, when) = self.clock?;
        let playing = self.player.as_ref().is_some_and(|player| player.playing);
        let at = if playing {
            at + now.duration_since(when).as_secs_f64()
        } else {
            at
        };
        let length = self.player.as_ref().and_then(|player| player.length);
        Some(length.map_or(at, |length| at.min(length)).max(0.0))
    }

    pub fn refresh(&mut self, now: Instant) {
        if let Some((player, reported)) = self.found.try_iter().last() {
            let estimate = self.position(now);
            match (&self.player, &player) {
                (Some(was), Some(is)) if song_key(was) == song_key(is) => {
                    let settling = self.seeked.is_some_and(|at| now.duration_since(at) < SEEK_SETTLE);
                    if let (Some(reported), false) = (reported, settling) {
                        let off = estimate.map_or(f64::INFINITY, |estimate| (reported - estimate).abs());
                        if off > JUMP {
                            self.jumps += 1;
                        }
                        if off > JUMP || was.playing != is.playing {
                            self.clock = Some((reported, now));
                        }
                    }
                }
                _ => {
                    self.clock = reported.map(|reported| (reported, now));
                    self.jumps += 1;
                    self.swap = None;
                    self.last_row = None;
                    if self.player.is_some() && player.is_some() {
                        self.arrived = Some((now, if self.went_back { -1.0 } else { 1.0 }));
                    }
                    self.went_back = false;
                }
            }
            self.player = player;
        }
        self.ask_for_lyrics(now);
    }

    fn ask_for_lyrics(&mut self, now: Instant) {
        if let Some((asked, answer)) = &self.asking {
            // Asked again for a source it was not asked of, the user's choices are kept: the new
            // takes come after the ones they point at.
            let kept = self.songs.get(asked).cloned().unwrap_or_default();
            let song = |found, release_live| Song {
                found,
                release_live,
                sources: lyrics::SOURCES,
                asked: seconds_now(),
                alignment: kept.alignment.clone(),
                choice: kept.choice.clone(),
            };
            match answer.try_recv() {
                Ok(Ok((found, live))) => {
                    self.songs.insert(asked.clone(), song(Some(found), live));
                    self.revision += 1;
                    self.save();
                }
                Ok(Err(Missing::NotFound)) => {
                    // Asked again for a new source, what was kept stays.
                    let (found, live) = (kept.found.clone(), kept.release_live);
                    self.songs.insert(asked.clone(), song(found, live));
                    self.revision += 1;
                    self.save();
                }
                Ok(Err(Missing::Unreachable)) | Err(mpsc::TryRecvError::Disconnected) => {
                    self.unreachable = Some((asked.clone(), now));
                }
                Err(mpsc::TryRecvError::Empty) => return,
            }
            self.asking = None;
        }
        let Some(player) = &self.player else {
            return;
        };
        let key = song_key(player);
        let failed = self
            .unreachable
            .as_ref()
            .is_some_and(|(failed, at)| *failed == key && now.duration_since(*at) < RETRY);
        let known = self.songs.get(&key).is_some_and(|song| {
            song.sources >= lyrics::SOURCES
                && (song.found.is_some() || seconds_now().saturating_sub(song.asked) < NOT_FOUND_FOR)
        });
        if known || failed {
            return;
        }
        let (send, answer) = mpsc::channel();
        let (title, artist, album) = (player.title.clone(), player.artist.clone(), player.album.clone());
        let length = player.length.unwrap_or(0.0);
        let started = std::thread::Builder::new()
            .name("nekoland-lyrics".into())
            .spawn(move || {
                let found = lyrics::fetch(&title, &artist, &album, length).map(|found| {
                    // Without this recording's own lyrics, its album's type tells whether it is live.
                    let matched = found.iter().any(|candidate| candidate.matches(length));
                    let live = (!matched && !album.is_empty())
                        .then(|| lyrics::release_is_live(&artist, &album).ok().flatten())
                        .flatten();
                    (found, live)
                });
                drop(send.send(found));
                text::redraw_soon();
            });
        match started {
            Ok(_) => self.asking = Some((key, answer)),
            Err(err) => tracing::warn!("lyrics: {err}"),
        }
    }

    /// The lyrics of what plays, once they are found.
    pub fn lyrics(&self) -> Option<Lyrics<'_>> {
        let player = self.player.as_ref()?;
        let song = self.songs.get(&song_key(player))?;
        let found = song.found.as_deref()?;
        let length = player.length.unwrap_or(0.0);
        let (mut chosen, _) = lyrics::choose(length, found)?;
        if let Some(Choice::Candidate(index)) = song.choice.filter(|choice| match choice {
            Choice::Candidate(index) => *index < found.len(),
            Choice::Plain => true,
        }) {
            chosen = index;
        }
        let candidate = &found[chosen];
        Some(Lyrics {
            found,
            chosen,
            matched: candidate.matches(length) && !candidate.synced.is_empty(),
            alignment: Some(&song.alignment),
            length,
            plain: song.choice == Some(Choice::Plain),
            release_live: song.release_live,
        })
    }

    /// Changes how the playing song's lyrics are set, and keeps it.
    pub fn set(&mut self, change: impl FnOnce(&mut Alignment, &mut Option<Choice>)) {
        let Some(key) = self.player.as_ref().map(song_key) else {
            return;
        };
        if let Some(song) = self.songs.get_mut(&key) {
            change(&mut song.alignment, &mut song.choice);
            self.revision += 1;
            self.save();
        }
    }

    /// Writes the songs kept to disk, on a thread.
    fn save(&mut self) {
        if self.songs.len() > KEPT_SONGS {
            let mut asked: Vec<u64> = self.songs.values().map(|song| song.asked).collect();
            asked.sort_unstable_by(|a, b| b.cmp(a));
            let oldest = asked[KEPT_SONGS - 1];
            self.songs.retain(|_, song| song.asked >= oldest);
        }
        let (Some(path), Ok(text)) = (cache_path(), serde_json::to_string(&self.songs)) else {
            return;
        };
        std::thread::spawn(move || {
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    let partial = path.with_extension("json.partial");
                    std::fs::write(&partial, text)?;
                    std::fs::rename(&partial, &path)
                });
            if let Err(err) = written {
                tracing::warn!("{}: {err}", path.display());
            }
        });
    }

    /// The user played from `seconds` in.
    pub fn seek(&mut self, seconds: f64) {
        let Some(player) = &self.player else {
            return;
        };
        let Some(track) = player.trackid.clone() else {
            return tracing::warn!("{}: no track to seek in", player.bus);
        };
        let seconds = player
            .length
            .map_or(seconds, |length| seconds.min(length))
            .max(0.0);
        let now = Instant::now();
        self.clock = Some((seconds, now));
        self.seeked = Some(now);
        self.jumps += 1;
        let bus = player.bus.clone();
        std::thread::spawn(move || system::player_seek(&bus, &track, seconds));
    }

    /// Asks the player to `part`'s action, showing it done at once.
    pub fn command(&mut self, part: Part) {
        match part {
            Part::Previous => self.went_back = true,
            Part::Next => self.went_back = false,
            _ => {}
        }
        let Some(player) = self.player.as_mut() else {
            return;
        };
        let method = match part {
            Part::Previous => "Previous",
            Part::Next => "Next",
            Part::PlayPause => {
                if let (Some(at), true) = (self.clock, player.playing) {
                    // Paused here, the clock stops where playback is.
                    let elapsed = at.1.elapsed().as_secs_f64();
                    self.clock = Some((at.0 + elapsed, Instant::now()));
                } else if let Some(at) = self.clock {
                    self.clock = Some((at.0, Instant::now()));
                }
                player.playing = !player.playing;
                "PlayPause"
            }
            Part::Cover => return,
        };
        let bus = player.bus.clone();
        std::thread::spawn(move || system::player_command(&bus, method));
    }

    /// The cover, read at `pixels` across, with the three colours the card is tinted with.
    pub fn cover(&mut self, pixels: i32) -> Option<&(ImageSurface, [Rgb; 3])> {
        let path = self.player.as_ref()?.art.clone()?;
        if self.cover.as_ref().map(|(read, _)| read) != Some(&path) {
            let reading = match self.cover_read.take() {
                Some((asked, reading)) if asked == path => reading,
                _ => stacks::read_on_a_thread(path.clone().into(), Size::from((pixels, pixels))),
            };
            let Some(read) = reading.lock().ok().and_then(|mut read| read.take()) else {
                text::picture_waited();
                self.cover_read = Some((path, reading));
                return None;
            };
            let scale = f64::from(text::picture_scale());
            let found = read.and_then(|pixels| {
                let colors = cover_colors(&pixels);
                let surface = stacks::pixels_surface(pixels)?;
                surface.set_device_scale(scale, scale);
                Some((surface, colors))
            });
            self.cover = Some((path, found));
        }
        self.cover.as_ref()?.1.as_ref()
    }

    /// What the tile shows now, its line moving on as the song does.
    pub fn face(&mut self, now: Instant, animations: bool, dt: f64) -> Option<Face> {
        let player = self.player.clone()?;
        let position = self.position(now).unwrap_or(0.0);
        let following = self.lyrics().and_then(|lyrics| {
            let at = lyrics.lyric_time(position)?;
            let rows = lyrics.rows();
            let index = lyrics::row_at(&rows, at);
            let shown = index.map(|index| {
                let row = &rows[index];
                (index, row.clone(), lyrics.text(row).map(String::from), at)
            });
            Some(shown)
        });
        let by = |a: &str, b: &str| match (a.is_empty(), b.is_empty()) {
            (false, false) => format!("{a} · {b}"),
            (false, true) => a.to_string(),
            _ => b.to_string(),
        };
        let (upper, row, id, lower, over) = match following {
            Some(shown) => {
                let upper = by(&player.title, &player.artist);
                match shown {
                    Some((index, row, Some(line), at)) => {
                        let pieces = self.pieces_of(&line);
                        // Each piece of a long line for its share of the line's time.
                        let span = (row.end - row.start).clamp(0.1, 8.0);
                        let total: usize = pieces.iter().map(|(piece, ..)| piece.chars().count()).sum();
                        let mut piece = 0;
                        let mut through = pieces[0].0.chars().count();
                        while piece + 1 < pieces.len()
                            && (at - row.start) / span > through as f64 / total as f64
                        {
                            piece += 1;
                            through += pieces[piece].0.chars().count();
                        }
                        let (text, size, over) = pieces[piece].clone();
                        (
                            upper,
                            Some(index),
                            format!("{index}:{piece}"),
                            (text, size, None),
                            over,
                        )
                    }
                    Some((index, row, None, at)) => {
                        let lit = (((at - row.start) / (row.end - row.start)) * 3.0)
                            .floor()
                            .clamp(0.0, 3.0) as u8;
                        (
                            upper,
                            Some(index),
                            format!("{index}"),
                            (String::new(), LINE_FONT, Some(lit)),
                            0.0,
                        )
                    }
                    None => (
                        upper,
                        None,
                        "title".into(),
                        (player.title.clone(), LINE_FONT, None),
                        0.0,
                    ),
                }
            }
            None => (
                by(&player.artist, &player.album),
                None,
                "title".into(),
                (player.title.clone(), LINE_FONT, None),
                0.0,
            ),
        };
        match &mut self.swap {
            Some(swap) if swap.id == id => swap.shown = lower.clone(),
            swap => {
                let fresh = self
                    .arrived
                    .is_some_and(|(at, _)| now.duration_since(at).as_secs_f64() < 0.5);
                let old = swap
                    .take()
                    .filter(|_| animations && !fresh)
                    .map(|swap| swap.shown);
                // Back to an earlier line, as after seeking back, it comes down from above.
                let way = match (row, self.last_row) {
                    (Some(row), Some(last)) if row < last => -1.0,
                    _ => 1.0,
                };
                *swap = Some(Swap {
                    id,
                    shown: lower,
                    old,
                    since: now,
                    way,
                    fresh,
                    over,
                });
            }
        }
        if row.is_some() {
            self.last_row = row;
        }
        let swap = self.swap.as_mut()?;
        let t = now.duration_since(swap.since).as_secs_f64();
        if t > 1.0 {
            swap.old = None;
        }
        let rise = LINE_RISE * swap.way;
        let (new_y, new_alpha) = if swap.old.is_none() && t > 1.0 || !animations {
            (0.0, 1.0)
        } else if swap.fresh {
            // A new song's first line: in from 30 % and 10 points lower.
            (
                10.0 * (1.0 - spring_at(t, spring(260.0, 24.0))),
                (0.3 + 0.7 * t / 0.2).min(1.0),
            )
        } else {
            let t = t - LINE_IN_AFTER;
            (
                rise * (1.0 - spring_at(t, spring(380.0, 20.0))),
                (t / 0.12).clamp(0.0, 1.0),
            )
        };
        let out = t / LINE_OUT;
        let (old_y, old_alpha) = (-rise * ease_in(out), (1.0 - out).clamp(0.0, 1.0));
        let cover_to = if player.playing { 1.0 } else { PAUSED_COVER };
        let (rate, ratio) = spring(260.0, 24.0);
        crate::motion::swing(
            &mut self.cover_scale.0,
            &mut self.cover_scale.1,
            cover_to,
            rate,
            ratio,
            dt,
        );
        if !animations || (self.cover_scale.0 - cover_to).abs() < 0.001 && self.cover_scale.1.abs() < 0.01 {
            self.cover_scale = (cover_to, 0.0);
        }
        let quarter = |size: f64| (size * 4.0).round() as u16;
        let pressed = self.press.map(|(part, _)| part);
        let read = self
            .cover
            .as_ref()
            .is_some_and(|(path, read)| Some(path) == player.art.as_ref() && read.is_some());
        // A new song's cover comes in from the way it was gone to, growing from 0.82.
        let arrival = match self.arrived {
            Some((at, way)) if animations && now.duration_since(at).as_secs_f64() < 1.2 => {
                let t = now.duration_since(at).as_secs_f64();
                let p = spring_at(t, spring(260.0, 24.0));
                (
                    (way * 12.0 * (1.0 - p) * 4.0).round() as i16,
                    ((0.35 + 0.65 * t / 0.18).min(1.0) * 100.0).round() as u8,
                    ((0.82 + 0.18 * p) * 1000.0).round() as u16,
                )
            }
            _ => {
                self.arrived = None;
                (0, 100, 1000)
            }
        };
        let swap = self.swap.as_ref()?;
        // Held at its start, slid to its end at 45 points a second, held, and back.
        let slide = (animations && swap.over > 0.0).then(|| {
            let t = (now.duration_since(swap.since).as_secs_f64() - LINE_IN_AFTER - 0.3).max(0.0);
            let travel = swap.over / 45.0;
            let at = t % (2.2 + travel);
            let off = if at < 1.1 {
                0.0
            } else {
                ((at - 1.1) * 45.0).min(swap.over)
            };
            ((off * 4.0).round() as u16, off > 0.0, off < swap.over)
        });
        Some(Face {
            upper,
            lower: (swap.shown.0.clone(), quarter(swap.shown.1), swap.shown.2),
            old: swap
                .old
                .as_ref()
                .filter(|_| old_alpha > 0.0)
                .map(|(text, size, dots)| (text.clone(), quarter(*size), *dots)),
            moving: (
                (new_y * 4.0).round() as i16,
                (new_alpha * 100.0).round() as u8,
                (old_y * 4.0).round() as i16,
                (old_alpha * 100.0).round() as u8,
            ),
            playing: player.playing,
            cover: ((self.cover_scale.0 * 1000.0).round() as u16, read),
            side: false,
            arrival,
            slide,
            bars: {
                // Folded with the cover as it shrinks for a pause.
                let open = ((self.cover_scale.0 - PAUSED_COVER) / (1.0 - PAUSED_COVER)).clamp(0.0, 1.0);
                let t = if animations {
                    now.duration_since(self.since).as_secs_f64()
                } else {
                    0.0
                };
                [(0, 1.7), (1, 2.3), (2, 1.3)].map(|(i, rate): (i32, f64)| {
                    let wave = 0.55 + 0.45 * (t * rate * std::f64::consts::TAU + f64::from(i) * 1.9).sin();
                    (open * wave * 20.0).round() as u8
                })
            },
            pressed,
        })
    }

    /// A line in the pieces that fit the Dock: broken after punctuation or at spaces, the
    /// neighbours joined while they still fit, a piece that still does not fit made smaller.
    fn pieces_of(&mut self, line: &str) -> Vec<(String, f64, f64)> {
        if let Some((was, pieces)) = &self.pieces {
            if was == line {
                return pieces.clone();
            }
        }
        let pieces = text::canvas(1, 1)
            .ok()
            .and_then(|surface| Context::new(&surface).ok())
            .map(|cr| {
                let width = |text: &str, size: f64| {
                    let layout = text::new_layout(&cr);
                    layout.set_font_description(Some(&face(pango::Weight::Semibold, size)));
                    layout.set_text(text);
                    f64::from(layout.pixel_size().0)
                };
                split_to_fit(line, TEXT_WIDTH, |text| width(text, LINE_FONT))
                    .into_iter()
                    .map(|piece| {
                        let mut size = LINE_FONT;
                        while size > LINE_FONT_MIN && width(&piece, size) > TEXT_WIDTH {
                            size -= 0.5;
                        }
                        let over = (width(&piece, size) - TEXT_WIDTH).max(0.0);
                        (piece, size, over)
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|pieces| !pieces.is_empty())
            .unwrap_or_else(|| vec![(line.to_string(), LINE_FONT, 0.0)]);
        self.pieces = Some((line.to_string(), pieces.clone()));
        pieces
    }
}

/// `line` broken after punctuation and at spaces into pieces, neighbours joined while they
/// fit in `room` as `width` measures them.
fn split_to_fit(line: &str, room: f64, width: impl Fn(&str) -> f64) -> Vec<String> {
    if width(line) <= room {
        return vec![line.to_string()];
    }
    let mut parts: Vec<String> = Vec::new();
    let mut part = String::new();
    for c in line.chars() {
        if c.is_whitespace() {
            if !part.is_empty() {
                parts.push(std::mem::take(&mut part));
            }
            continue;
        }
        part.push(c);
        if "，、。,；！？!?".contains(c) {
            parts.push(std::mem::take(&mut part));
        }
    }
    if !part.is_empty() {
        parts.push(part);
    }
    let mut pieces: Vec<String> = Vec::new();
    for part in parts {
        // Words of a Latin script were apart, Chinese characters were not.
        let joined = |last: &str| {
            let space = if last.ends_with(|c: char| c.is_ascii_alphanumeric()) {
                " "
            } else {
                ""
            };
            format!("{last}{space}{part}")
        };
        match pieces.last_mut() {
            Some(last) if width(&joined(last)) <= room => *last = joined(last),
            _ => pieces.push(part),
        }
    }
    // A piece's trailing comma is left out: the next piece shows the line goes on.
    pieces
        .into_iter()
        .map(|piece| piece.trim_end_matches(['，', '、', ',']).to_string())
        .collect()
}

/// The three colours of a cover: its top left, middle and bottom right, each averaged.
fn cover_colors((bytes, alpha, width, height, stride): &stacks::Pixels) -> [Rgb; 3] {
    let channels = if *alpha { 4 } else { 3 };
    let average = |x0: i32, y0: i32| {
        let (mut sum, mut count) = ((0.0, 0.0, 0.0), 0.0);
        for y in (y0..y0 + height / 2).step_by(4) {
            for x in (x0..x0 + width / 2).step_by(4) {
                let at = (y * stride + x * channels) as usize;
                if let Some(pixel) = bytes.get(at..at + 3) {
                    sum.0 += f64::from(pixel[0]);
                    sum.1 += f64::from(pixel[1]);
                    sum.2 += f64::from(pixel[2]);
                    count += 1.0;
                }
            }
        }
        let count = f64::max(count, 1.0) * 255.0;
        (sum.0 / count, sum.1 / count, sum.2 / count)
    };
    [
        average(0, 0),
        average(width / 4, height / 4),
        average(width / 2, height / 2),
    ]
}

fn face(weight: pango::Weight, size: f64) -> pango::FontDescription {
    let mut font = text::font("");
    font.set_weight(weight);
    font.set_absolute_size(size * f64::from(pango::SCALE));
    font
}

fn layout_line(cr: &Context, line: &str, font: &pango::FontDescription) -> pango::Layout {
    let layout = text::new_layout(cr);
    layout.set_font_description(Some(font));
    layout.set_text(line);
    layout
}

/// A line cut short with an ellipsis where it would pass `width`, its top at `y`.
fn show_cut(cr: &Context, line: &str, font: &pango::FontDescription, x: f64, y: f64, width: f64) -> f64 {
    let layout = layout_line(cr, line, font);
    layout.set_width((width.max(0.0) * f64::from(pango::SCALE)) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, &layout);
    f64::from(layout.pixel_size().0)
}

/// Draws the Dock's tile at `(x, y)`, its art `art` big.
pub fn draw_tile(
    cr: &Context,
    (x, y, art): (f64, f64, f64),
    tile: &Face,
    cover: Option<&ImageSurface>,
    dark: bool,
) {
    let unit = art / 40.0;
    let ink = crate::dock::ink(dark);
    let ink_3 = crate::menubar::colors(dark).ink_3;
    let middle = y + art / 2.0;

    // The cover, smaller while paused or pressed.
    let pressed = |part| if tile.pressed == Some(part) { PRESSED } else { 1.0 };
    let scale = f64::from(tile.cover.0) / 1000.0 * f64::from(tile.arrival.2) / 1000.0 * pressed(Part::Cover);
    let size = art * scale;
    let shift = f64::from(tile.arrival.0) / 4.0 * unit;
    let solid = f64::from(tile.arrival.1) / 100.0;
    let (cx, cy) = (x + (art - size) / 2.0 + shift, y + (art - size) / 2.0);
    let _ = cr.save();
    text::rounded_rectangle(cr, cx, cy, size, size, 6.0 * unit * scale);
    match cover {
        Some(cover) => {
            cr.clip();
            let (width, height) = text::points(cover);
            cr.translate(cx, cy);
            cr.scale(size / width, size / height);
            let _ = cr.set_source_surface(cover, 0.0, 0.0);
            let _ = cr.paint_with_alpha(solid);
        }
        None => {
            cr.set_source_rgba(ink.0, ink.1, ink.2, 0.08 * solid);
            let _ = cr.fill();
            if let Some(icon) = text::named_icon("audio-x-generic-symbolic", "", 16, ink_3) {
                let _ = cr.set_source_surface(&icon, x + (art - 16.0) / 2.0, middle - 8.0);
                let _ = cr.paint();
            }
        }
    }
    let _ = cr.restore();
    if tile.bars.iter().any(|&bar| bar > 0) {
        // Three bars on a dark patch in the cover's corner.
        let (bar, gap, tall) = (2.0 * unit, 1.5 * unit, 9.0 * unit);
        let right = cx + size - 4.0 * unit;
        let bottom = cy + size - 4.0 * unit;
        let width = 3.0 * bar + 2.0 * gap;
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
        text::rounded_rectangle(
            cr,
            right - width - 2.0 * unit,
            bottom - tall - 2.0 * unit,
            width + 4.0 * unit,
            tall + 4.0 * unit,
            3.0 * unit,
        );
        let _ = cr.fill();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        for (index, level) in tile.bars.iter().enumerate() {
            let height = (tall * f64::from(*level) / 20.0).max(bar);
            let left = right - width + index as f64 * (bar + gap);
            text::rounded_rectangle(cr, left, bottom - height, bar, height, bar / 2.0);
            let _ = cr.fill();
        }
    }

    if tile.side {
        return;
    }

    // Two lines: what plays, and the line sung now coming up from under it.
    let left = x + TEXT_X * unit;
    let room = TEXT_WIDTH * unit;
    cr.set_source_rgb(ink_3.0, ink_3.1, ink_3.2);
    show_cut(
        cr,
        &tile.upper,
        &face(pango::Weight::Normal, 11.0 * unit),
        left,
        middle - 17.0 * unit,
        room,
    );
    let row = (middle - 3.0 * unit, LINE_RISE * unit);
    let _ = cr.save();
    cr.rectangle(left - 2.0, row.0, room + 4.0, row.1);
    cr.clip();
    cr.push_group();
    let shown = |(line, size, dots): &(String, u16, Option<u8>),
                 dy: i16,
                 alpha: u8,
                 slide: Option<(u16, bool, bool)>| {
        let alpha = f64::from(alpha) / 100.0;
        let top = row.0 + f64::from(dy) / 4.0 * unit;
        if let Some(lit) = dots {
            for dot in 0..3u8 {
                let on = if dot < *lit { 1.0 } else { 0.35 };
                cr.set_source_rgba(ink.0, ink.1, ink.2, alpha * on);
                cr.arc(
                    left + (4.0 + f64::from(dot) * 11.0) * unit,
                    top + row.1 / 2.0,
                    2.5 * unit,
                    0.0,
                    std::f64::consts::TAU,
                );
                let _ = cr.fill();
            }
            return;
        }
        cr.set_source_rgba(ink.0, ink.1, ink.2, alpha);
        let font = face(pango::Weight::Semibold, f64::from(*size) / 4.0 * unit);
        let layout = layout_line(cr, line, &font);
        let height = f64::from(layout.pixel_size().1);
        let Some((off, left_fade, right_fade)) = slide else {
            show_cut(cr, line, &font, left, top + (row.1 - height) / 2.0, room);
            return;
        };
        // Slid along whole, fading where more of it is out of sight.
        cr.push_group();
        cr.move_to(left - f64::from(off) / 4.0 * unit, top + (row.1 - height) / 2.0);
        pangocairo::functions::show_layout(cr, &layout);
        if let Ok(text) = cr.pop_group() {
            let edge = 24.0 * unit / room;
            let fade = cairo::LinearGradient::new(left, 0.0, left + room, 0.0);
            fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, if left_fade { 0.0 } else { 1.0 });
            fade.add_color_stop_rgba(edge, 0.0, 0.0, 0.0, 1.0);
            fade.add_color_stop_rgba(1.0 - edge, 0.0, 0.0, 0.0, 1.0);
            fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, if right_fade { 0.0 } else { 1.0 });
            let _ = cr.set_source(&text);
            let _ = cr.mask(&fade);
        }
    };
    if let Some(old) = &tile.old {
        shown(old, tile.moving.2, tile.moving.3, None);
    }
    shown(&tile.lower, tile.moving.0, tile.moving.1, tile.slide);
    // Lines moving in or out fade at the row's edges rather than being cut there.
    if let Ok(lines) = cr.pop_group() {
        let edge = 4.0 * unit / row.1;
        let fade = cairo::LinearGradient::new(0.0, row.0, 0.0, row.0 + row.1);
        fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.0);
        fade.add_color_stop_rgba(edge, 0.0, 0.0, 0.0, 1.0);
        fade.add_color_stop_rgba(1.0 - edge, 0.0, 0.0, 0.0, 1.0);
        fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
        let _ = cr.set_source(&lines);
        let _ = cr.mask(&fade);
    }
    let _ = cr.restore();

    // The buttons, the one to play or pause on a faint round ground.
    for (part, at, width) in BUTTONS {
        let center = x + (at + width / 2.0) * unit;
        let scale = pressed(part);
        let (name, icon) = match part {
            Part::Previous => ("media-skip-backward-symbolic", 17.0),
            Part::Next => ("media-skip-forward-symbolic", 17.0),
            _ if tile.playing => ("media-playback-pause-symbolic", 18.0),
            _ => ("media-playback-start-symbolic", 18.0),
        };
        let _ = cr.save();
        cr.translate(center, middle);
        cr.scale(scale, scale);
        if part == Part::PlayPause {
            cr.set_source_rgba(ink.0, ink.1, ink.2, 0.08);
            cr.arc(0.0, 0.0, 16.0 * unit, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
        let pixels = (icon * unit).round() as i32;
        if let Some(icon) = text::named_icon(name, "", pixels, ink) {
            let half = f64::from(pixels) / 2.0;
            let _ = cr.set_source_surface(&icon, -half, -half);
            let _ = cr.paint();
        }
        let _ = cr.restore();
    }
}

/// The card's measures.
const CARD_WIDTH: i32 = 380;
const PAD: f64 = 20.0;
const HEADER: f64 = 48.0;
const HINT: f64 = 20.0;
const LYRICS_HEIGHT: f64 = 330.0;
/// Where the line sung is held in the lyrics, from their top.
const HELD_AT: f64 = 0.3;
const UNALIGNED_AT: f64 = 0.4;
const LINE_GAP: f64 = 14.0;
const FADE_EDGE: f64 = 36.0;
const BAR: f64 = 4.0;
/// A wheel turn moves the lyrics this far, and they go back to the line sung after this.
const WHEEL_STEP: f64 = 48.0;
const FREE_FOR: Duration = Duration::from_millis(2500);
/// The lines after the one moving off follow it this much later each, as far as `LAG_LINES`.
const LAG: f64 = 0.06;
const LAG_LINES: usize = 6;
/// The next line lights up this long before it is sung.
const HANDOFF: f64 = 0.4;
const CLOSE_TIME: f64 = 0.24;
/// How often the cover's glows are drawn again as they drift: this many times for each
/// radian of their way round, which they go at about half of each second.
const DRIFT_STEPS: f64 = 16.0;
/// The glows are drawn this many times smaller than the card and scaled up.
const GLOW_SHRINK: f64 = 3.0;
const REDUCED_TIME: f64 = 0.15;
pub const CARD_RADIUS: i32 = 14;

/// Where what is on the card is, from its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Places {
    height: f64,
    hint: Option<f64>,
    lyrics: Option<f64>,
    source: Option<f64>,
    bar: f64,
}

fn places(mode: Option<Mode>) -> Places {
    let mut y = PAD + HEADER + 12.0;
    let hint = matches!(mode, Some(Mode::Unaligned | Mode::Aligned)).then(|| {
        let at = y;
        y += HINT + 4.0;
        at
    });
    let lyrics = mode.map(|_| {
        let at = y;
        y += LYRICS_HEIGHT + 10.0;
        at
    });
    let source = mode.map(|mode| {
        let at = y;
        y += if matches!(mode, Mode::Unaligned | Mode::Aligned) {
            36.0
        } else {
            20.0
        };
        at
    });
    let bar = y + 6.0;
    Places {
        height: bar + 34.0 + PAD - 6.0,
        hint,
        lyrics,
        source,
        bar,
    }
}

/// What a press on the card is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CardPart {
    Row(usize),
    Bar(f64),
    Realign,
    Cancel,
    /// The line naming where the lyrics are from, which opens their menu.
    Source,
    Menu(MenuAction),
    /// The card away from the open menu, which only closes it.
    Away,
}

/// What an entry of the lyrics' menu does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuAction {
    Choose(usize),
    Plain,
    Align,
    ResetAlignment,
    Earlier,
    Later,
}

/// One entry of the lyrics' menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub label: String,
    pub detail: String,
    pub checked: bool,
    pub enabled: bool,
    pub action: MenuAction,
    /// Whether a rule goes over it, starting another group.
    pub apart: bool,
}

/// The lyrics' menu: what this recording is judged to be and on what, then every version
/// found, plain text, aligning by hand and, for lyrics of this recording, moving them in time.
pub fn menu(lyrics: &Lyrics, player: &Player) -> (Option<String>, Vec<Entry>) {
    let why = lyrics.kind(player).map(|(kind, by)| {
        let kind = match kind {
            Kind::Live => " LIVE 版",
            Kind::Album => "專輯版",
        };
        format!("這首是{kind}（依{by}）")
    });
    let mode = lyrics.mode();
    let entry = |label: String, detail: String, checked, enabled, action, apart| Entry {
        label,
        detail,
        checked,
        enabled,
        action,
        apart,
    };
    let mut entries: Vec<Entry> = (lyrics.found.iter().enumerate())
        .map(|(index, candidate)| {
            let off = candidate.duration - lyrics.length;
            let fit = if candidate.matches(lyrics.length) {
                "相符".to_string()
            } else {
                format!("差 {}", clock(off.abs()))
            };
            let text = if candidate.synced.is_empty() {
                " · 純文字"
            } else {
                ""
            };
            let words = if candidate.has_words() { " · 逐字" } else { "" };
            entry(
                format!("{} · {}{words}{text}", version(candidate), candidate.source),
                format!("{} · {fit}", clock(candidate.duration)),
                index == lyrics.chosen && !lyrics.plain && mode != Mode::Aligned,
                true,
                MenuAction::Choose(index),
                false,
            )
        })
        .collect();
    entries.push(entry(
        "純文字".into(),
        String::new(),
        lyrics.plain,
        true,
        MenuAction::Plain,
        false,
    ));
    let timed = !lyrics.candidate().synced.is_empty();
    let anchored = lyrics
        .alignment
        .is_some_and(|alignment| !alignment.anchors.is_empty());
    entries.push(entry(
        "手動對齊".into(),
        String::new(),
        mode == Mode::Aligned,
        timed && !lyrics.matched,
        MenuAction::Align,
        true,
    ));
    entries.push(entry(
        "重設對齊".into(),
        String::new(),
        false,
        anchored,
        MenuAction::ResetAlignment,
        false,
    ));
    if mode == Mode::Synced {
        entries.push(entry(
            "歌詞提早 0.5 秒".into(),
            String::new(),
            false,
            true,
            MenuAction::Earlier,
            true,
        ));
        entries.push(entry(
            "歌詞延後 0.5 秒".into(),
            String::new(),
            false,
            true,
            MenuAction::Later,
            false,
        ));
    }
    (why, entries)
}

const MENU_ROW: f64 = 30.0;
const MENU_WHY: f64 = 28.0;
const MENU_PAD: f64 = 6.0;

/// The lyrics card, grown out of the tile's cover.
pub struct Card {
    song: String,
    opened: Instant,
    pub closing: Option<Instant>,
    open: (f64, f64),
    pub rect: Rectangle<i32, Logical>,
    /// Where the tile's cover is, which the card grows out of and goes back into.
    pub from: Point<i32, Logical>,
    pub aligning: bool,
    pub hover_bar: bool,
    pub scrub: Option<f64>,
    pub picture: Option<Picture>,
    /// The entry of the open menu the keyboard is on, and where its entries were drawn.
    pub menu: Option<usize>,
    /// The line the keyboard is on, and the line sung now or, unaligned, the one guessed.
    pub focus: Option<usize>,
    pub active: Option<usize>,
    /// Where the drawing, on its thread, put things, as of its last picture.
    menu_drawn: Option<((f64, f64, f64, f64), Vec<(f64, Entry)>)>,
    tops: Vec<f64>,
    heights: Vec<f64>,
    scrolled: f64,
    jobs: mpsc::Sender<Job>,
    results: Receiver<Output>,
    /// Whether a picture is being drawn, which is waited for before another is asked.
    asked: bool,
    /// What the drawing has been told of the cover and the lyrics, which are told only when
    /// they change.
    sent_cover: Option<String>,
    sent_lyrics: Option<(String, u64)>,
    /// What the user did since, for the drawing to apply.
    events: Vec<Event>,
}

impl Card {
    pub fn new(song: String, rect: Rectangle<i32, Logical>, from: Point<i32, Logical>) -> Self {
        let opened = Instant::now();
        let (jobs, asked) = mpsc::channel::<Job>();
        let (sent, results) = mpsc::channel();
        let named = song.clone();
        // It ends with the card, whose senders it is waiting on. What it draws with stays on it.
        let started = std::thread::Builder::new()
            .name("nekoland-card".into())
            .spawn(move || {
                let mut painter = Painter::new(named, opened);
                while let Ok(job) = asked.recv() {
                    if sent.send(painter.paint(job)).is_err() {
                        break;
                    }
                    text::redraw_soon();
                }
            });
        if let Err(err) = started {
            tracing::warn!("card: {err}");
        }
        Self {
            song,
            opened,
            closing: None,
            open: (0.0, 0.0),
            rect,
            from,
            aligning: false,
            hover_bar: false,
            scrub: None,
            picture: None,
            menu: None,
            focus: None,
            active: None,
            menu_drawn: None,
            tops: Vec::new(),
            heights: Vec::new(),
            scrolled: 0.0,
            jobs,
            results,
            asked: false,
            sent_cover: None,
            sent_lyrics: None,
            events: Vec::new(),
        }
    }

    pub fn size(mode: Option<Mode>) -> Size<i32, Logical> {
        Size::from((CARD_WIDTH, places(mode).height.round() as i32))
    }

    /// How far open the card is, 0 to 1, and whether it is gone.
    pub fn openness(&self, animations: bool) -> (f64, bool) {
        if let Some(closed) = self.closing {
            let time = if animations { CLOSE_TIME } else { REDUCED_TIME };
            let t = closed.elapsed().as_secs_f64() / time;
            return (1.0 - ease_in(t), t >= 1.0);
        }
        if !animations {
            return (
                (self.opened.elapsed().as_secs_f64() / REDUCED_TIME).min(1.0),
                false,
            );
        }
        (self.open.0, false)
    }

    /// Where it is drawn now, how large and how solid: while opening and closing it grows out
    /// of the cover and goes back into it.
    pub fn shown(&self, animations: bool) -> (Rectangle<i32, Logical>, f64, f32) {
        let (open, _) = self.openness(animations);
        if !animations {
            return (self.rect, 1.0, open as f32);
        }
        let center = self.rect.loc.to_f64() + self.rect.size.to_f64().downscale(2.0).to_point();
        let way = (self.from.to_f64() - center).upscale(1.0 - open);
        let rect = Rectangle::new(self.rect.loc + way.to_i32_round(), self.rect.size);
        (rect, 0.12 + 0.88 * open, (open * 3.0).min(1.0) as f32)
    }

    /// The entries of the open menu, as last drawn.
    pub fn menu_entries(&self) -> Vec<Entry> {
        self.menu_drawn
            .as_ref()
            .map(|(_, placed)| placed.iter().map(|(_, entry)| entry.clone()).collect())
            .unwrap_or_default()
    }

    /// Where the rows are, from the top of the lyrics, as last measured.
    pub fn row_count(&self) -> usize {
        self.tops.len()
    }

    /// Scrolls so that row `row` shows, as the keyboard moves onto it.
    pub fn show_row(&mut self, row: usize) {
        self.events.push(Event::ShowRow(row));
    }

    pub fn song(&self) -> &str {
        &self.song
    }

    /// How far along the bar a position is, 0 to 1.
    pub fn through_at(&self, pos: Point<f64, Logical>) -> f64 {
        let x = pos.x - f64::from(self.rect.loc.x) - PAD;
        (x / (f64::from(CARD_WIDTH) - 2.0 * PAD)).clamp(0.0, 1.0)
    }

    pub fn wheel(&mut self, amount: f64) {
        self.events.push(Event::Wheel(amount));
    }

    /// The row a point on the output is on, or the control.
    pub fn part_at(&self, pos: Point<f64, Logical>, playing: &NowPlaying) -> Option<CardPart> {
        let local = pos - self.rect.loc.to_f64();
        if let (Some(_), Some(((x, y, w, h), entries))) = (self.menu, &self.menu_drawn) {
            if !(*x..x + w).contains(&local.x) || !(*y..y + h).contains(&local.y) {
                return Some(CardPart::Away);
            }
            return entries
                .iter()
                .find(|(top, entry)| entry.enabled && (*top..top + MENU_ROW).contains(&local.y))
                .map(|(_, entry)| CardPart::Menu(entry.action));
        }
        let lyrics = playing.lyrics();
        let mode = lyrics.as_ref().map(Lyrics::mode);
        let places = places(mode);
        let width = f64::from(CARD_WIDTH);
        if (places.bar - 8.0..places.bar + BAR + 8.0).contains(&local.y)
            && (PAD..width - PAD).contains(&local.x)
        {
            return Some(CardPart::Bar(
                ((local.x - PAD) / (width - 2.0 * PAD)).clamp(0.0, 1.0),
            ));
        }
        if let Some(hint) = places.hint {
            if self.aligning && (hint..hint + HINT).contains(&local.y) && local.x > width - PAD - 60.0 {
                return Some(CardPart::Cancel);
            }
        }
        if let Some(source) = places
            .source
            .filter(|source| (*source..source + 20.0).contains(&local.y))
        {
            let _ = source;
            if mode == Some(Mode::Aligned) && !self.aligning && local.x > width - PAD - 80.0 {
                return Some(CardPart::Realign);
            }
            if local.x < width - PAD - 84.0 {
                return Some(CardPart::Source);
            }
        }
        let top = places.lyrics?;
        if !(top..top + LYRICS_HEIGHT).contains(&local.y) {
            return None;
        }
        let (tops, heights) = (&self.tops, &self.heights);
        let y = local.y - top + self.scrolled;
        (0..tops.len())
            .find(|&row| (tops[row] - LINE_GAP / 2.0..tops[row] + heights[row] + LINE_GAP / 2.0).contains(&y))
            .map(CardPart::Row)
    }

    /// Moves the card on by `dt` and has it drawn again where anything it shows changed.
    pub fn refresh(&mut self, playing: &mut NowPlaying, now: Instant, dt: f64, dark: bool, animations: bool) {
        let (rate, ratio) = spring(260.0, 24.0);
        if self.closing.is_none() {
            crate::motion::swing(&mut self.open.0, &mut self.open.1, 1.0, rate, ratio, dt);
            if (self.open.0 - 1.0).abs() < 0.001 && self.open.1.abs() < 0.01 {
                self.open = (1.0, 0.0);
            }
        }
        // What was drawn since, which is as of the frame it was asked for.
        while let Ok(output) = self.results.try_recv() {
            self.asked = false;
            if output.picture.is_some() {
                self.picture = output.picture;
            }
            self.tops = output.tops;
            self.heights = output.heights;
            self.scrolled = output.scrolled;
            self.menu_drawn = output.menu_drawn;
            self.active = output.active;
            self.focus = self.focus.filter(|&focus| focus < self.tops.len());
        }
        if self.asked {
            return;
        }
        let Some(player) = playing.player.clone() else {
            return;
        };
        let position = self
            .scrub
            .and_then(|scrub| Some(scrub * player.length?))
            .or(playing.position(now))
            .unwrap_or(0.0);
        let cover = playing.cover(64 * text::picture_scale()).cloned();
        let cover_id = cover.as_ref().and_then(|_| player.art.clone());
        let cover = (cover_id != self.sent_cover)
            .then(|| cover.and_then(|(surface, colors)| CoverData::of(&surface, colors)));
        self.sent_cover = if cover.as_ref().is_some_and(Option::is_none) {
            None
        } else {
            cover_id
        };
        let lyrics_key = (song_key(&player), playing.revision);
        let lyrics = (self.sent_lyrics.as_ref() != Some(&lyrics_key))
            .then(|| playing.lyrics().as_ref().map(LyricsData::of));
        if lyrics.is_some() {
            self.sent_lyrics = Some(lyrics_key);
        }
        let job = Job {
            now,
            dt,
            dark,
            animations,
            height: self.rect.size.h,
            jumps: playing.jumps,
            player,
            position,
            aligning: self.aligning,
            scrub: self.scrub,
            hover_bar: self.hover_bar,
            focus: self.focus,
            menu: self.menu,
            events: std::mem::take(&mut self.events),
            cover,
            lyrics,
        };
        if self.jobs.send(job).is_ok() {
            self.asked = true;
        }
    }
}

/// What the card shows is drawn on a thread of its own: its text is set out and painted over
/// a surface the size of the card, which takes longer than a frame of the compositor's thread
/// can spare. The thread keeps what the drawing moves with, and the card asks it for a picture
/// and what the card needs to know of it.
struct Job {
    now: Instant,
    dt: f64,
    dark: bool,
    animations: bool,
    height: i32,
    player: Player,
    position: f64,
    jumps: u32,
    aligning: bool,
    scrub: Option<f64>,
    hover_bar: bool,
    focus: Option<usize>,
    menu: Option<usize>,
    events: Vec<Event>,
    /// Some when the cover is another, with None for no cover.
    cover: Option<Option<CoverData>>,
    /// Some when the lyrics are others, with None for none.
    lyrics: Option<Option<LyricsData>>,
}

/// What the user did to the lyrics, which the drawing's own measures decide the effect of.
enum Event {
    Wheel(f64),
    ShowRow(usize),
}

/// A cover's pixels, which a surface cannot carry to another thread.
struct CoverData {
    pixels: Vec<u8>,
    size: (i32, i32),
    stride: i32,
    scale: (f64, f64),
    colors: [Rgb; 3],
}

impl CoverData {
    fn of(surface: &ImageSurface, colors: [Rgb; 3]) -> Option<Self> {
        // Its pixels can be had of a surface only when nothing else holds it, which the
        // cover's is, so they are drawn onto one of their own.
        let size = (surface.width(), surface.height());
        let scale = surface.device_scale();
        let mut copy = ImageSurface::create(cairo::Format::ARgb32, size.0, size.1).ok()?;
        copy.set_device_scale(scale.0, scale.1);
        let cr = Context::new(&copy).ok()?;
        cr.set_source_surface(surface, 0.0, 0.0).ok()?;
        cr.paint().ok()?;
        drop(cr);
        copy.flush();
        let stride = copy.stride();
        let pixels = copy.data().ok()?.to_vec();
        Some(Self {
            pixels,
            size,
            stride,
            scale,
            colors,
        })
    }

    fn surface(self) -> Option<(ImageSurface, [Rgb; 3])> {
        let surface = ImageSurface::create_for_data(
            self.pixels,
            cairo::Format::ARgb32,
            self.size.0,
            self.size.1,
            self.stride,
        )
        .ok()?;
        surface.set_device_scale(self.scale.0, self.scale.1);
        Some((surface, self.colors))
    }
}

/// `Lyrics`, owned.
struct LyricsData {
    found: Vec<Candidate>,
    chosen: usize,
    matched: bool,
    alignment: Option<Alignment>,
    length: f64,
    plain: bool,
    release_live: Option<bool>,
}

impl LyricsData {
    fn of(lyrics: &Lyrics) -> Self {
        Self {
            found: lyrics.found.to_vec(),
            chosen: lyrics.chosen,
            matched: lyrics.matched,
            alignment: lyrics.alignment.cloned(),
            length: lyrics.length,
            plain: lyrics.plain,
            release_live: lyrics.release_live,
        }
    }

    fn view(&self) -> Lyrics<'_> {
        Lyrics {
            found: &self.found,
            chosen: self.chosen,
            matched: self.matched,
            alignment: self.alignment.as_ref(),
            length: self.length,
            plain: self.plain,
            release_live: self.release_live,
        }
    }
}

/// What the drawing tells the card: the picture when it changed, and where things are.
struct Output {
    picture: Option<Picture>,
    tops: Vec<f64>,
    heights: Vec<f64>,
    /// How far the lyrics are scrolled, by the line sung and by the user together.
    scrolled: f64,
    menu_drawn: Option<((f64, f64, f64, f64), Vec<(f64, Entry)>)>,
    active: Option<usize>,
}

/// The drawing's side of the card, on its thread.
struct Painter {
    song: String,
    opened: Instant,
    scroll: f64,
    speed: f64,
    trail: VecDeque<(Instant, f64)>,
    free: f64,
    free_at: Option<Instant>,
    emphasis: Vec<f64>,
    metrics: Option<(String, Vec<f64>, Vec<f64>)>,
    /// Each row's text laid out, kept with the measures: setting a line out costs more than
    /// painting it.
    layouts: Vec<Option<pango::Layout>>,
    jumps: u32,
    drawn: Vec<i64>,
    menu_drawn: Option<((f64, f64, f64, f64), Vec<(f64, Entry)>)>,
    active: Option<usize>,
    cover: Option<(ImageSurface, [Rgb; 3])>,
    lyrics: Option<LyricsData>,
}

impl Painter {
    fn new(song: String, opened: Instant) -> Self {
        Self {
            song,
            opened,
            scroll: f64::NAN,
            speed: 0.0,
            trail: VecDeque::new(),
            free: 0.0,
            free_at: None,
            emphasis: Vec::new(),
            metrics: None,
            layouts: Vec::new(),
            jumps: 0,
            drawn: Vec::new(),
            menu_drawn: None,
            active: None,
            cover: None,
            lyrics: None,
        }
    }

    fn output(&self, picture: Option<Picture>) -> Output {
        let (tops, heights) = self
            .metrics
            .as_ref()
            .map(|(_, tops, heights)| (tops.clone(), heights.clone()))
            .unwrap_or_default();
        Output {
            picture,
            tops,
            heights,
            scrolled: self.scroll + self.free,
            menu_drawn: self.menu_drawn.clone(),
            active: self.active,
        }
    }

    /// Draws what `job` asks, as the card is drawn again where anything it shows changed.
    fn paint(&mut self, mut job: Job) -> Output {
        if let Some(cover) = job.cover.take() {
            self.cover = cover.and_then(CoverData::surface);
        }
        if let Some(lyrics) = job.lyrics.take() {
            self.lyrics = lyrics;
        }
        for event in &job.events {
            match *event {
                Event::Wheel(amount) => {
                    self.free += amount.signum() * WHEEL_STEP;
                    self.free_at = Some(job.now);
                }
                Event::ShowRow(row) => {
                    let Some((_, tops, heights)) = &self.metrics else {
                        continue;
                    };
                    let (Some(top), Some(height)) = (tops.get(row), heights.get(row)) else {
                        continue;
                    };
                    let shown = self.scroll + self.free;
                    if *top < shown + FADE_EDGE || top + height > shown + LYRICS_HEIGHT - FADE_EDGE {
                        self.free = top - LYRICS_HEIGHT * UNALIGNED_AT - self.scroll;
                    }
                    self.free_at = Some(job.now);
                }
            }
        }
        // They are lent to the drawing, which also changes the rest of what is kept.
        let lyrics = self.lyrics.take();
        let cover = self.cover.take();
        let output = self.draw(&job, lyrics.as_ref().map(LyricsData::view), cover.as_ref());
        self.lyrics = lyrics;
        self.cover = cover;
        output
    }

    fn draw(
        &mut self,
        job: &Job,
        lyrics: Option<Lyrics<'_>>,
        cover: Option<&(ImageSurface, [Rgb; 3])>,
    ) -> Output {
        let (now, dt, dark, animations) = (job.now, job.dt, job.dark, job.animations);
        let (player, position, jumps) = (&job.player, job.position, job.jumps);
        let waits = text::icon_waits();
        let Ok(surface) = text::canvas(CARD_WIDTH, job.height) else {
            return self.output(None);
        };
        let mode = lyrics.as_ref().map(Lyrics::mode);
        let places = places(mode);
        let width = f64::from(CARD_WIDTH);
        let room = width - 2.0 * PAD;
        let Ok(cr) = Context::new(&surface) else {
            return self.output(None);
        };
        // The glass under it is rounded; the cover's colours over it are too.
        text::rounded_rectangle(&cr, 0.0, 0.0, width, places.height, f64::from(CARD_RADIUS));
        cr.clip();
        let ink = crate::dock::ink(dark);
        let ink_3 = crate::menubar::colors(dark).ink_3;
        let mut key: Vec<i64> = vec![
            dark as i64,
            player.playing as i64,
            mode.map_or(-1, |mode| mode as i64),
        ];

        // The cover's colours on the glass, two of them drifting about inside the card.
        let colors = cover.as_ref().map(|(_, colors)| *colors);
        let drift = now.duration_since(self.opened).as_secs_f64() / 11.0 * std::f64::consts::TAU;
        if let Some([base, one, two]) = colors {
            key.push((drift * DRIFT_STEPS) as i64);
            let strength = if dark { 0.6 } else { 0.3 };
            cr.set_source_rgba(base.0, base.1, base.2, strength);
            let _ = cr.paint();
            // Soft as they are, the two glows are drawn small and scaled up: over the whole
            // card they cost most of what drawing it did.
            let height = places.height;
            let small = (
                (width / GLOW_SHRINK).ceil() as i32,
                (height / GLOW_SHRINK).ceil() as i32,
            );
            if let Ok(glow) = cairo::ImageSurface::create(cairo::Format::ARgb32, small.0, small.1) {
                if let Ok(small_cr) = Context::new(&glow) {
                    small_cr.scale(1.0 / GLOW_SHRINK, 1.0 / GLOW_SHRINK);
                    for (color, phase) in [(one, 0.0), (two, 2.1)] {
                        let x = width * (0.5 + 0.28 * (drift + phase).sin());
                        let y = height * (0.45 + 0.3 * (drift * 0.8 + phase).cos());
                        let blob = cairo::RadialGradient::new(x, y, 0.0, x, y, width * 1.1);
                        blob.add_color_stop_rgba(0.0, color.0, color.1, color.2, strength);
                        blob.add_color_stop_rgba(1.0, color.0, color.1, color.2, 0.0);
                        let _ = small_cr.set_source(&blob);
                        let _ = small_cr.paint();
                    }
                }
                let _ = cr.save();
                cr.scale(GLOW_SHRINK, GLOW_SHRINK);
                let _ = cr.set_source_surface(&glow, 0.0, 0.0);
                cr.source().set_filter(cairo::Filter::Bilinear);
                let _ = cr.paint();
                let _ = cr.restore();
            }
            let veil = if dark {
                (0.0, 0.0, 0.0, 0.25)
            } else {
                (1.0, 1.0, 1.0, 0.35)
            };
            cr.set_source_rgba(veil.0, veil.1, veil.2, veil.3);
            let _ = cr.paint();
        }
        // What it shows comes in after the card, a part at a time.
        let entered = now.duration_since(self.opened).as_secs_f64();
        if animations && entered < 0.5 {
            key.push((entered * 60.0) as i64);
        }
        let entering = |part: f64| {
            if !animations {
                return (0.0, 1.0);
            }
            let t = ((entered - 0.06 - part * 0.04) / 0.22).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - t).powi(3);
            (8.0 * (1.0 - eased), eased)
        };
        let begin = |part: f64| {
            cr.push_group();
            cr.translate(0.0, entering(part).0);
        };
        let end = |part: f64| {
            if let Ok(drawn) = cr.pop_group() {
                let _ = cr.set_source(&drawn);
                let _ = cr.paint_with_alpha(entering(part).1);
            }
        };

        // What plays, and whether it is a live take.
        begin(0.0);
        if let Some((surface, _)) = &cover {
            let _ = cr.save();
            let scale = if player.playing { 1.0 } else { 0.8 };
            let size = HEADER * scale;
            let at = PAD + (HEADER - size) / 2.0;
            text::rounded_rectangle(&cr, at, at, size, size, 8.0 * scale);
            cr.clip();
            let (w, h) = text::points(surface);
            cr.translate(at, at);
            cr.scale(size / w, size / h);
            let _ = cr.set_source_surface(surface, 0.0, 0.0);
            let _ = cr.paint();
            let _ = cr.restore();
        }
        let left = PAD + HEADER + 12.0;
        let live = match &lyrics {
            Some(lyrics) => lyrics.kind(&player),
            None => (lyrics::says_live(&player.title) || lyrics::says_live(&player.album))
                .then_some((Kind::Live, "曲目名稱")),
        };
        let is_live = matches!(live, Some((Kind::Live, _)));
        let tag = if is_live { 42.0 } else { 0.0 };
        cr.set_source_rgb(ink.0, ink.1, ink.2);
        let title_width = show_cut(
            &cr,
            &player.title,
            &face(pango::Weight::Semibold, 15.0),
            left,
            PAD + 4.0,
            width - left - PAD - tag,
        );
        if is_live {
            let x = left + title_width + 8.0;
            cr.set_source_rgba(ink.0, ink.1, ink.2, 0.7);
            cr.set_line_width(1.0);
            text::rounded_rectangle(&cr, x + 0.5, PAD + 6.5, 34.0, 16.0, 4.0);
            let _ = cr.stroke();
            show_cut(
                &cr,
                "LIVE",
                &face(pango::Weight::Bold, 10.0),
                x + 6.0,
                PAD + 7.5,
                30.0,
            );
        }
        cr.set_source_rgb(ink_3.0, ink_3.1, ink_3.2);
        let by = [player.artist.as_str(), player.album.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        show_cut(
            &cr,
            &by,
            &face(pango::Weight::Normal, 12.0),
            left,
            PAD + 26.0,
            width - left - PAD,
        );

        end(0.0);
        begin(1.0);
        if let (Some(lyrics), Some(top)) = (&lyrics, places.lyrics) {
            let mode = lyrics.mode();
            let rows = lyrics.rows();
            let (font, color) = match mode {
                Mode::Synced | Mode::Aligned => (face(pango::Weight::Bold, 26.0), ink),
                Mode::Unaligned | Mode::Plain => {
                    (face(pango::Weight::Medium, 22.0), crate::dock::ink_2(dark))
                }
            };
            // Each row's top and height, kept for the song and how it is shown.
            let measured = format!("{}\n{:?}\n{}", self.song, mode, lyrics.chosen);
            if self.metrics.as_ref().map(|(was, ..)| was) != Some(&measured) {
                let mut tops = Vec::with_capacity(rows.len());
                let mut heights = Vec::with_capacity(rows.len());
                self.layouts.clear();
                let mut y = 0.0;
                for row in &rows {
                    let height = match lyrics.text(row) {
                        Some(line) => {
                            let layout = layout_line(&cr, line, &font);
                            layout.set_width((room * f64::from(pango::SCALE)) as i32);
                            layout.set_wrap(pango::WrapMode::WordChar);
                            let height = f64::from(layout.pixel_size().1);
                            self.layouts.push(Some(layout));
                            height
                        }
                        None => {
                            self.layouts.push(None);
                            26.0
                        }
                    };
                    tops.push(y);
                    heights.push(height);
                    y += height + LINE_GAP;
                }
                self.metrics = Some((measured, tops, heights));
                self.emphasis = vec![0.26; rows.len()];
                self.scroll = f64::NAN;
            }
            let (_, tops, heights) = self.metrics.clone().unwrap_or_default();
            let at = lyrics.lyric_time(position);
            let active = at.and_then(|at| lyrics::row_at(&rows, at));
            self.active = match mode {
                Mode::Synced | Mode::Aligned => active,
                Mode::Unaligned => rows.iter().rposition(|row| row.start <= lyrics.guess(position)),
                Mode::Plain => None,
            };
            let focus = job.focus.filter(|&focus| focus < rows.len());
            key.push(focus.map_or(-1, |focus| focus as i64));
            let goal = match (mode, active) {
                (Mode::Synced | Mode::Aligned, _) => {
                    tops.get(active.unwrap_or(0)).copied().unwrap_or(0.0) - LYRICS_HEIGHT * HELD_AT
                }
                (Mode::Unaligned, _) => {
                    let guess = lyrics.guess(position);
                    let row = rows.iter().rposition(|row| row.start <= guess).unwrap_or(0);
                    tops.get(row).copied().unwrap_or(0.0) - LYRICS_HEIGHT * UNALIGNED_AT
                }
                (Mode::Plain, _) => -LINE_GAP,
            };
            let last = tops.last().copied().unwrap_or(0.0);
            let goal = goal.clamp(-LYRICS_HEIGHT * HELD_AT, last.max(0.0));
            // Unaligned lyrics stay where they are, and are scrolled to the guess only when
            // the card opens and playback jumps.
            let still = matches!(mode, Mode::Unaligned | Mode::Plain);
            if self.scroll.is_nan() || !animations {
                self.scroll = goal;
                self.speed = 0.0;
                self.jumps = jumps;
            } else if !still || self.jumps != jumps {
                if still && self.jumps != jumps {
                    self.free = 0.0;
                }
                let (rate, ratio) = spring(170.0, 26.0);
                crate::motion::swing(&mut self.scroll, &mut self.speed, goal, rate, ratio, dt);
                if (self.scroll - goal).abs() < 0.3 && self.speed.abs() < 1.0 {
                    self.scroll = goal;
                    self.speed = 0.0;
                    self.jumps = jumps;
                }
            }
            self.trail.push_back((now, self.scroll));
            while self.trail.len() > 2
                && now.duration_since(self.trail[1].0).as_secs_f64() > LAG * LAG_LINES as f64 + 0.05
            {
                self.trail.pop_front();
            }
            // Turned by the wheel, the lyrics go back to the line sung a little later.
            let free = self.free_at.is_some_and(|at| now.duration_since(at) < FREE_FOR);
            if !free && !still && self.free != 0.0 {
                self.scroll += self.free;
                self.free = 0.0;
            }
            let free_shown = self.free != 0.0;
            key.extend([
                (self.scroll * 2.0) as i64,
                (self.free * 2.0) as i64,
                job.aligning as i64,
            ]);

            let at_lag = |lag: f64| -> f64 {
                let when = now - Duration::from_secs_f64(lag);
                match self.trail.iter().position(|(t, _)| *t >= when) {
                    Some(0) | None => self.trail.front().map_or(self.scroll, |(_, s)| *s),
                    Some(i) => {
                        let ((t0, s0), (t1, s1)) = (self.trail[i - 1], self.trail[i]);
                        let span = t1.duration_since(t0).as_secs_f64().max(1e-6);
                        s0 + (s1 - s0) * (when.duration_since(t0).as_secs_f64() / span).clamp(0.0, 1.0)
                    }
                }
            };
            let follows = !still && self.speed.abs() > 1.0;
            let ahead = self.speed > 0.0;
            let _ = cr.save();
            cr.rectangle(0.0, top, width, LYRICS_HEIGHT);
            cr.clip();
            cr.push_group();
            let dt_emphasis = 1.0 - (-dt / 0.12).exp();
            for (index, row) in rows.iter().enumerate() {
                let distance = active.map_or(index + 1, |active| index.abs_diff(active));
                let lagging = match active {
                    Some(active) if follows && ahead && index > active => (index - active).min(LAG_LINES),
                    Some(active) if follows && !ahead && index < active => (active - index).min(LAG_LINES),
                    _ => 0,
                };
                let scroll = if lagging > 0 {
                    at_lag(LAG * lagging as f64)
                } else {
                    self.scroll
                } + self.free;
                let y = top + tops[index] - scroll;
                let goal = match mode {
                    Mode::Unaligned | Mode::Plain => 1.0,
                    _ if Some(index) == active => {
                        if player.playing {
                            1.0
                        } else {
                            0.8
                        }
                    }
                    _ if free_shown => 0.55,
                    _ if at.is_some_and(|at| {
                        Some(index) == active.map(|a| a + 1).or(Some(0)) && row.start - at < HANDOFF
                    }) =>
                    {
                        0.7
                    }
                    _ if distance == 1 => 0.48,
                    _ => 0.26,
                };
                let emphasis = self.emphasis.get_mut(index).map_or(goal, |e| {
                    *e += (goal - *e) * if animations { dt_emphasis } else { 1.0 };
                    *e
                });
                if y + heights[index] < top - 40.0 || y > top + LYRICS_HEIGHT + 40.0 {
                    continue;
                }
                key.push((emphasis * 100.0) as i64);
                let scale = if matches!(mode, Mode::Synced | Mode::Aligned) {
                    0.93 + 0.07 * ((emphasis - 0.26) / (0.48 - 0.26)).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                if focus == Some(index) {
                    cr.set_source_rgb(ink.0, ink.1, ink.2);
                    cr.set_line_width(2.0);
                    text::rounded_rectangle(&cr, PAD - 8.0, y - 5.0, room + 16.0, heights[index] + 10.0, 8.0);
                    let _ = cr.stroke();
                }
                let _ = cr.save();
                cr.translate(PAD, y);
                cr.scale(scale, scale);
                match lyrics.text(row) {
                    Some(line) => {
                        let layout = match self.layouts.get(index).cloned().flatten() {
                            Some(layout) => {
                                pangocairo::functions::update_layout(&cr, &layout);
                                layout
                            }
                            None => {
                                let layout = layout_line(&cr, line, &font);
                                layout.set_width((room * f64::from(pango::SCALE)) as i32);
                                layout.set_wrap(pango::WrapMode::WordChar);
                                layout
                            }
                        };
                        let words = (Some(index) == active && mode == Mode::Synced)
                            .then(|| lyrics.candidate().synced[row.line?].words.clone())
                            .flatten();
                        match (words, at) {
                            (Some(words), Some(at)) => {
                                key.push((at * 30.0) as i64);
                                draw_lit(&cr, &layout, &words, at, color, emphasis);
                            }
                            _ => {
                                // In the dark, lines from the second away are blurred a step
                                // further each, as far as three: the same line drawn around
                                // itself, as cairo has no blur. Not while turned by the wheel.
                                let steps = if dark && !still && !free_shown {
                                    distance.saturating_sub(1).min(3)
                                } else {
                                    0
                                };
                                let radius = 0.55 * steps as f64;
                                let around: &[(f64, f64)] = if steps == 0 {
                                    &[(0.0, 0.0)]
                                } else {
                                    &[(0.0, 0.0), (1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)]
                                };
                                let alpha = emphasis / if steps == 0 { 1.0 } else { 2.2 };
                                for (dx, dy) in around {
                                    cr.set_source_rgba(color.0, color.1, color.2, alpha);
                                    cr.move_to(dx * radius, dy * radius);
                                    pangocairo::functions::show_layout(&cr, &layout);
                                }
                            }
                        }
                    }
                    None => {
                        // An interlude: three dots lit as it goes, breathing, shrinking away
                        // at its end for the line after it.
                        let progress = at.map_or(0.0, |at| {
                            ((at - row.start) / (row.end - row.start)).clamp(0.0, 1.0)
                        });
                        key.push((progress * 60.0) as i64);
                        let breath = 1.0 + 0.06 * (now.duration_since(self.opened).as_secs_f64() * 2.4).sin();
                        let shrink = if progress > 0.94 {
                            1.0 - (progress - 0.94) / 0.06
                        } else {
                            1.0
                        };
                        for dot in 0..3 {
                            let lit = (progress * 3.0 - f64::from(dot)).clamp(0.0, 1.0);
                            cr.set_source_rgba(
                                color.0,
                                color.1,
                                color.2,
                                (0.3 + 0.7 * lit) * emphasis.max(0.48),
                            );
                            cr.arc(
                                6.0 + f64::from(dot) * 18.0,
                                13.0,
                                5.0 * breath * shrink,
                                0.0,
                                std::f64::consts::TAU,
                            );
                            let _ = cr.fill();
                        }
                    }
                }
                let _ = cr.restore();
            }
            let _ = cr.pop_group_to_source();
            // The lyrics fade out at the top and the bottom.
            let fade = cairo::LinearGradient::new(0.0, top, 0.0, top + LYRICS_HEIGHT);
            let edge = FADE_EDGE / LYRICS_HEIGHT;
            fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.0);
            fade.add_color_stop_rgba(edge, 0.0, 0.0, 0.0, 1.0);
            fade.add_color_stop_rgba(1.0 - edge, 0.0, 0.0, 0.0, 1.0);
            fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
            let _ = cr.mask(&fade);
            let _ = cr.restore();

            let small = face(pango::Weight::Normal, 12.0);
            let button = face(pango::Weight::Semibold, 12.0);
            if let Some(hint) = places.hint {
                if mode == Mode::Unaligned || job.aligning {
                    cr.set_source_rgb(ink.0, ink.1, ink.2);
                    show_cut(
                        &cr,
                        "點正在唱的那句",
                        &face(pango::Weight::Bold, 13.0),
                        PAD,
                        hint + 1.0,
                        room - 60.0,
                    );
                }
                if job.aligning {
                    draw_pill(&cr, "取消", &button, width - PAD, hint, ink);
                }
            }
            if let Some(source) = places.source {
                let state = match mode {
                    Mode::Synced | Mode::Plain => "",
                    Mode::Aligned => " · 手動對齊",
                    Mode::Unaligned => " · 未對齊",
                };
                let version = if mode == Mode::Plain {
                    "純文字"
                } else {
                    lyrics.version()
                };
                let offset = lyrics.alignment.map_or(0.0, |alignment| alignment.offset);
                let offset = if mode == Mode::Synced && offset != 0.0 {
                    format!(" · 偏移 {offset:+.1} 秒")
                } else {
                    String::new()
                };
                cr.set_source_rgb(ink_3.0, ink_3.1, ink_3.2);
                let words = if mode != Mode::Plain && lyrics.candidate().has_words() {
                    " · 逐字"
                } else {
                    ""
                };
                let shown = format!(
                    "歌詞來源：{} · {version}{words}{state}{offset}",
                    lyrics.candidate().source
                );
                let used = show_cut(&cr, &shown, &small, PAD, source + 2.0, room - 100.0);
                // It opens the menu of versions.
                if let Some(icon) = text::named_icon("pan-down-symbolic", "", 12, ink_3) {
                    let _ = cr.set_source_surface(&icon, PAD + used + 4.0, source + 4.0);
                    let _ = cr.paint();
                }
                if matches!(mode, Mode::Unaligned | Mode::Aligned) {
                    let longer = lyrics.length - lyrics.candidate().duration;
                    let than = if longer >= 0.0 { "長" } else { "短" };
                    cr.set_source_rgb(ink_3.0, ink_3.1, ink_3.2);
                    show_cut(
                        &cr,
                        &format!("比{}{than} {}", lyrics.version(), clock(longer.abs())),
                        &small,
                        PAD,
                        source + 19.0,
                        room,
                    );
                }
                if mode == Mode::Aligned && !job.aligning {
                    draw_pill(&cr, "重新對齊", &button, width - PAD, source, ink);
                }
            }
        }

        end(1.0);
        begin(2.0);
        // How far it has played, which can be pressed or dragged to play from elsewhere.
        let length = player.length.unwrap_or(0.0);
        let through = if length > 0.0 {
            (position / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thick = if job.hover_bar || job.scrub.is_some() {
            10.0
        } else {
            BAR
        };
        key.extend([
            (through * room * 2.0) as i64,
            thick as i64,
            (position as i64),
            job.scrub.is_some() as i64,
        ]);
        let bar_y = places.bar + BAR / 2.0 - thick / 2.0;
        cr.set_source_rgba(ink.0, ink.1, ink.2, 0.16);
        text::rounded_rectangle(&cr, PAD, bar_y, room, thick, thick / 2.0);
        let _ = cr.fill();
        cr.set_source_rgba(ink.0, ink.1, ink.2, 0.85);
        text::rounded_rectangle(&cr, PAD, bar_y, (room * through).max(thick), thick, thick / 2.0);
        let _ = cr.fill();
        if job.hover_bar || job.scrub.is_some() {
            let radius = if job.scrub.is_some() { 7.5 } else { 6.0 };
            cr.set_source_rgb(ink.0, ink.1, ink.2);
            cr.arc(
                PAD + room * through,
                places.bar + BAR / 2.0,
                radius,
                0.0,
                std::f64::consts::TAU,
            );
            let _ = cr.fill();
        }
        let times = face(pango::Weight::Normal, 11.0);
        cr.set_source_rgb(ink_3.0, ink_3.1, ink_3.2);
        show_cut(&cr, &clock(position), &times, PAD, places.bar + 12.0, 80.0);
        let total = layout_line(&cr, &clock(length), &times);
        cr.move_to(width - PAD - f64::from(total.pixel_size().0), places.bar + 12.0);
        pangocairo::functions::show_layout(&cr, &total);

        end(2.0);

        // The menu of versions, on a solid ground over the lyrics, over where they are from.
        self.menu_drawn = None;
        if let (Some(focused), Some(lyrics), Some(source)) = (job.menu, &lyrics, places.source) {
            let (why, entries) = menu(lyrics, &player);
            let rules = entries.iter().filter(|entry| entry.apart).count() as f64;
            let height = MENU_PAD * 2.0
                + why.as_ref().map_or(0.0, |_| MENU_WHY)
                + entries.len() as f64 * MENU_ROW
                + rules * 9.0;
            let (x, w) = (PAD - 6.0, room + 12.0);
            let y = (source - 6.0 - height).max(PAD);
            let colors = crate::menubar::colors(dark);
            cr.set_source_rgb(colors.sheet.0, colors.sheet.1, colors.sheet.2);
            text::rounded_rectangle(&cr, x, y, w, height, 10.0);
            let _ = cr.fill_preserve();
            let edge = colors.edge;
            cr.set_source_rgba(edge.0, edge.1, edge.2, edge.3);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
            let mut top = y + MENU_PAD;
            if let Some(why) = &why {
                cr.set_source_rgb(ink_3.0, ink_3.1, ink_3.2);
                show_cut(
                    &cr,
                    why,
                    &face(pango::Weight::Normal, 12.0),
                    x + 12.0,
                    top + 6.0,
                    w - 24.0,
                );
                top += MENU_WHY;
            }
            let label = face(pango::Weight::Normal, 13.0);
            let small = face(pango::Weight::Normal, 12.0);
            let mut placed = Vec::new();
            for (index, entry) in entries.into_iter().enumerate() {
                if entry.apart {
                    cr.set_source_rgba(ink.0, ink.1, ink.2, 0.12);
                    cr.rectangle(x + 10.0, top + 4.0, w - 20.0, 1.0);
                    let _ = cr.fill();
                    top += 9.0;
                }
                if index == focused {
                    cr.set_source_rgba(ink.0, ink.1, ink.2, 0.1);
                    text::rounded_rectangle(&cr, x + 4.0, top, w - 8.0, MENU_ROW, 6.0);
                    let _ = cr.fill();
                }
                let alpha = if entry.enabled { 1.0 } else { 0.4 };
                cr.set_source_rgba(ink.0, ink.1, ink.2, alpha);
                if entry.checked {
                    show_cut(&cr, "✓", &label, x + 10.0, top + 6.0, 16.0);
                }
                show_cut(&cr, &entry.label, &label, x + 30.0, top + 6.0, w - 150.0);
                if !entry.detail.is_empty() {
                    let detail = layout_line(&cr, &entry.detail, &small);
                    cr.set_source_rgba(ink_3.0, ink_3.1, ink_3.2, alpha);
                    cr.move_to(x + w - 12.0 - f64::from(detail.pixel_size().0), top + 7.0);
                    pangocairo::functions::show_layout(&cr, &detail);
                }
                placed.push((top, entry));
                top += MENU_ROW;
            }
            key.extend([focused as i64, placed.len() as i64, y as i64]);
            self.menu_drawn = Some(((x, y, w, height), placed));
        }
        drop(cr);
        let waited = text::icon_waits() != waits;
        let mut picture = None;
        if key != self.drawn {
            picture = text::picture(surface);
            // What was drawn while an icon was still being read is drawn again.
            self.drawn = if waited { Vec::new() } else { key };
        }
        self.output(picture)
    }
}

/// A word-timed line, lit up to where it is sung, a soft edge between lit and not.
fn draw_lit(cr: &Context, layout: &pango::Layout, words: &[(f64, String)], at: f64, color: Rgb, alpha: f64) {
    let sung = words.iter().take_while(|(time, _)| *time <= at).count();
    // How far into the character being sung, from its start to the next's.
    let into = match (sung.checked_sub(1).and_then(|i| words.get(i)), words.get(sung)) {
        (Some((from, _)), Some((to, _))) => ((at - from) / (to - from).max(0.01)).clamp(0.0, 1.0),
        (Some(_), None) => 1.0,
        _ => 0.0,
    };
    let text = layout.text();
    let byte = |chars: usize| text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i) as i32;
    let start = layout.index_to_pos(byte(sung.saturating_sub(1)));
    let end = layout.index_to_pos(byte(sung));
    let scale = f64::from(pango::SCALE);
    let x0 = f64::from(start.x()) / scale;
    let x1 = if end.y() == start.y() {
        f64::from(end.x()) / scale
    } else {
        f64::from(start.x() + start.width()) / scale
    };
    let edge_x = if sung == 0 { 0.0 } else { x0 + (x1 - x0) * into };
    let edge_y = f64::from(start.y()) / scale;
    let row_height = f64::from(start.height()) / scale;
    let (_, size) = layout.pixel_size();
    let dim = 0.48 * alpha;
    let (r, g, b) = color;
    // Rows above the one sung are lit, rows under it not; on it, the edge.
    for (y0, y1, lit) in [(0.0, edge_y, true), (edge_y + row_height, f64::from(size), false)] {
        if y1 > y0 {
            let _ = cr.save();
            cr.rectangle(-4.0, y0, 10_000.0, y1 - y0);
            cr.clip();
            cr.set_source_rgba(r, g, b, if lit { alpha } else { dim });
            cr.move_to(0.0, 0.0);
            pangocairo::functions::show_layout(cr, layout);
            let _ = cr.restore();
        }
    }
    let _ = cr.save();
    cr.rectangle(-4.0, edge_y, 10_000.0, row_height);
    cr.clip();
    let soft = 0.2 * f64::from(layout.pixel_size().0.max(1));
    let gradient = cairo::LinearGradient::new(edge_x - soft / 2.0, 0.0, edge_x + soft / 2.0, 0.0);
    gradient.add_color_stop_rgba(0.0, r, g, b, alpha);
    gradient.add_color_stop_rgba(1.0, r, g, b, dim);
    let _ = cr.set_source(&gradient);
    cr.move_to(0.0, 0.0);
    pangocairo::functions::show_layout(cr, layout);
    let _ = cr.restore();
}

/// A small button, its right edge at `right`.
fn draw_pill(cr: &Context, label: &str, font: &pango::FontDescription, right: f64, top: f64, ink: Rgb) {
    let layout = layout_line(cr, label, font);
    let (width, height) = layout.pixel_size();
    let (width, height) = (f64::from(width) + 16.0, f64::from(height) + 4.0);
    cr.set_source_rgba(ink.0, ink.1, ink.2, 0.1);
    text::rounded_rectangle(cr, right - width, top, width, height, height / 2.0);
    let _ = cr.fill();
    cr.set_source_rgb(ink.0, ink.1, ink.2);
    cr.move_to(right - width + 8.0, top + 2.0);
    pangocairo::functions::show_layout(cr, &layout);
}

/// Seconds as `m:ss`.
fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0).floor() as i64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn springs_settle_on_their_goal() {
        let open = spring(260.0, 24.0);
        assert!((spring_at(1.0, open) - 1.0).abs() < 0.005);
        // 260/24 goes a little past, not so far that a card this large is seen to sway.
        let most = (1..100)
            .map(|i| spring_at(f64::from(i) / 100.0, open))
            .fold(0.0, f64::max);
        assert!(most > 1.0 && most < 1.04, "{most}");
    }

    #[test]
    fn long_lines_break_after_punctuation_and_join_what_fits() {
        let width = |text: &str| text.chars().count() as f64;
        assert_eq!(split_to_fit("短句", 5.0, width), vec!["短句"]);
        assert_eq!(
            split_to_fit("一二三，四五，六七八九十", 7.0, width),
            vec!["一二三，四五", "六七八九十"]
        );
        assert_eq!(
            split_to_fit("one two three", 8.0, width),
            vec!["one two", "three"]
        );
    }

    #[test]
    fn the_tile_opens_the_card_on_the_cover_or_text_and_plays_on_its_buttons() {
        assert_eq!(part_at(10.0, 40.0), Some(Part::Cover));
        assert_eq!(part_at(150.0, 40.0), Some(Part::Cover));
        assert_eq!(part_at(250.0, 40.0), Some(Part::PlayPause));
        assert_eq!(part_at(500.0, 80.0), Some(Part::PlayPause));
        assert_eq!(part_at(295.0, 40.0), Some(Part::Next));
    }

    #[test]
    fn times_read_as_minutes() {
        assert_eq!(clock(51.9), "0:51");
        assert_eq!(clock(303.0), "5:03");
    }
}
