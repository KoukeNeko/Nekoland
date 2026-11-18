//! What hint mode labels on a screen, as Nekominal's: the URLs, file paths and git hashes, each with a short label the
//! keyboard picks it by. Pure: it reads the rows of the screen as text.

use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKind {
    Url,
    /// A path as a compiler or a tool prints it, with its `:line` or `:line:column` when it has one.
    Path,
    Hash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    /// Where its first character is on the screen.
    pub row: usize,
    pub column: usize,
    pub text: String,
    pub kind: HintKind,
    pub label: String,
}

/// A row of the screen: its text, and whether it goes on in the row under it.
pub struct ScreenRow {
    pub text: String,
    pub wraps: bool,
}

/// The letters labels are made of, those under the fingers first.
const ALPHABET: &str = "fjdkslaghrueiwoqptyvbcnxmz";
const URL_SCHEMES: [&str; 4] = ["http://", "https://", "ftp://", "file://"];
const HASH_LENGTH: std::ops::RangeInclusive<usize> = 7..=40;
const TRAILING_PUNCTUATION: [char; 6] = ['.', ',', ';', ':', '!', '?'];
const BREAKS: &str = "\"'`<>()[]{}|";

/// A character of the screen and where it is.
type Placed = (char, usize, usize);

/// The hints on the screen, the one nearest the bottom (the latest output) with the first label. One text found twice
/// has one label; past 26 texts every label is two letters, so that no label is the start of another.
pub fn find(rows: &[ScreenRow]) -> Vec<Hint> {
    let mut found: Vec<Hint> = logical_lines(rows)
        .iter()
        .flat_map(|line| tokens(line))
        .filter_map(|token| {
            let text: String = token.iter().map(|(letter, _, _)| *letter).collect();
            let (kind, length) = classify(&text)?;
            let (_, row, column) = token[0];
            Some(Hint { row, column, text: text.chars().take(length).collect(), kind, label: String::new() })
        })
        .collect();
    found.sort_by(|above, below| below.row.cmp(&above.row).then(above.column.cmp(&below.column)));
    let mut texts: Vec<String> = Vec::new();
    for hint in &found {
        if !texts.contains(&hint.text) {
            texts.push(hint.text.clone());
        }
    }
    let labels = labels(texts.len());
    for hint in &mut found {
        let at = texts.iter().position(|text| *text == hint.text).unwrap_or_default();
        hint.label = labels[at].clone();
    }
    found
}

fn labels(count: usize) -> Vec<String> {
    let letters: Vec<char> = ALPHABET.chars().collect();
    if count <= letters.len() {
        return letters.iter().take(count).map(char::to_string).collect();
    }
    letters
        .iter()
        .flat_map(|first| letters.iter().map(move |second| format!("{first}{second}")))
        .take(count)
        .collect()
}

/// The characters of the screen with the row and column of each, a wrapped row joined to the one it goes on to; a
/// wide character takes two columns.
fn logical_lines(rows: &[ScreenRow]) -> Vec<Vec<Placed>> {
    let mut lines = Vec::new();
    let mut line = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let mut column = 0;
        for letter in row.text.chars() {
            line.push((letter, index, column));
            column += letter.width().unwrap_or_default();
        }
        if !row.wraps {
            lines.push(std::mem::take(&mut line));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The runs of characters between blanks, quotes and brackets.
fn tokens(line: &[Placed]) -> Vec<&[Placed]> {
    line.split(|(letter, _, _)| letter.is_whitespace() || BREAKS.contains(*letter))
        .filter(|token| !token.is_empty())
        .collect()
}

/// What a token is, and how many of its characters are it: the punctuation a sentence puts after it is not.
fn classify(token: &str) -> Option<(HintKind, usize)> {
    let trimmed = token.trim_end_matches(TRAILING_PUNCTUATION);
    let length = trimmed.chars().count();
    let lower = trimmed.to_ascii_lowercase();
    if URL_SCHEMES.iter().any(|scheme| lower.starts_with(scheme) && lower.len() > scheme.len()) {
        return Some((HintKind::Url, length));
    }
    if is_hash(trimmed) {
        return Some((HintKind::Hash, length));
    }
    looks_like_path(trimmed).then_some((HintKind::Path, length))
}

fn is_hash(text: &str) -> bool {
    HASH_LENGTH.contains(&text.len())
        && text.chars().all(|letter| letter.is_ascii_hexdigit())
        && text.chars().any(|letter| letter.is_ascii_digit())
        && text.chars().any(|letter| letter.is_ascii_alphabetic())
}

/// A path without its `:line:column`.
pub fn without_position(text: &str) -> &str {
    let mut path = text;
    for _ in 0..2 {
        match path.rsplit_once(':') {
            Some((before, after)) if !after.is_empty() && after.chars().all(|digit| digit.is_ascii_digit()) => path = before,
            _ => break,
        }
    }
    path
}

/// Whether a text names a file: from the root, home or here, or a name with an extension in a folder or with a line.
fn looks_like_path(text: &str) -> bool {
    let path = without_position(text);
    if path.contains("://") || path.chars().any(char::is_control) {
        return false;
    }
    let rooted = ["/", "~/", "./", "../"].iter().any(|start| path.starts_with(start) && path.len() > start.len());
    let name = path.rsplit('/').next().unwrap_or_default();
    let extended = name
        .rsplit_once('.')
        .is_some_and(|(stem, extension)| !stem.is_empty() && !extension.is_empty() && extension.chars().all(char::is_alphanumeric));
    let has_letters = extended && name.chars().any(char::is_alphabetic);
    rooted || (has_letters && (path.contains('/') || path.len() < text.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(lines: &[&str]) -> Vec<ScreenRow> {
        lines.iter().map(|text| ScreenRow { text: (*text).to_string(), wraps: false }).collect()
    }

    fn texts(hints: &[Hint]) -> Vec<(&str, HintKind, &str)> {
        hints.iter().map(|hint| (hint.text.as_str(), hint.kind, hint.label.as_str())).collect()
    }

    #[test]
    fn urls_paths_with_their_positions_and_hashes_are_found_the_latest_first() {
        let hints = find(&screen(&[
            "see https://example.com/a?b=1. now",
            "error at src/main.rs:12:5 and /work/x.txt",
            "commit 3f2a9c1d (HEAD)",
        ]));
        assert_eq!(
            texts(&hints),
            [
                ("3f2a9c1d", HintKind::Hash, "f"),
                ("src/main.rs:12:5", HintKind::Path, "j"),
                ("/work/x.txt", HintKind::Path, "d"),
                ("https://example.com/a?b=1", HintKind::Url, "k"),
            ]
        );
        assert_eq!((hints[0].row, hints[0].column), (2, 7));
    }

    #[test]
    fn a_hint_after_wide_characters_is_at_their_columns() {
        let hints = find(&screen(&["分組列 ./shots/a.png"]));
        assert_eq!(texts(&hints), [("./shots/a.png", HintKind::Path, "f")]);
        assert_eq!((hints[0].row, hints[0].column), (0, 7));
    }

    #[test]
    fn plain_words_numbers_and_versions_are_not_hints() {
        assert!(find(&screen(&["hello world 1234567 deadbeef 0.0.21 v1.2 ... a.b"])).is_empty());
    }

    #[test]
    fn a_url_that_wraps_onto_the_next_row_is_one_hint() {
        let rows = vec![
            ScreenRow { text: "go https://example.c".into(), wraps: true },
            ScreenRow { text: "om/a/long/path end".into(), wraps: false },
        ];
        let hints = find(&rows);
        assert_eq!(texts(&hints), [("https://example.com/a/long/path", HintKind::Url, "f")]);
        assert_eq!((hints[0].row, hints[0].column), (0, 3));
    }

    #[test]
    fn one_text_twice_has_one_label_and_past_twenty_six_every_label_is_two_letters() {
        let twice = find(&screen(&["a/b.rs", "a/b.rs"]));
        assert_eq!(twice.iter().map(|hint| hint.label.as_str()).collect::<Vec<_>>(), ["f", "f"]);
        let many: String = (0..30).map(|number| format!("d{number}/f.rs ")).collect();
        let labels: Vec<String> = find(&screen(&[&many])).into_iter().map(|hint| hint.label).collect();
        assert_eq!(labels.len(), 30);
        assert!(labels.iter().all(|label| label.chars().count() == 2));
    }
}
