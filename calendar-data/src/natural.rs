//! An event typed as a sentence: its title with the day, the time, how long it lasts, whether
//! it takes the whole day and which calendar it is in said among the words, in Chinese or in
//! English. What is understood is taken out of the title and shown as the fields it sets.

use std::ops::Range;

use crate::date::Date;

const MINUTES_IN_HOUR: u32 = 60;
const MINUTES_IN_DAY: u32 = 24 * 60;
const DAYS_IN_WEEK: i64 = 7;
const NOON: u32 = 12;
const MORNING_HOUR: u32 = 9;
const AFTERNOON_HOUR: u32 = 15;
const EVENING_HOUR: u32 = 19;
const DEFAULT_MINUTES: u32 = 0;

const MONTH_NAMES: [&str; 12] = [
    "january", "february", "march", "april", "may", "june", "july", "august", "september", "october",
    "november", "december",
];
const ENGLISH_WEEKDAYS: [&str; 7] = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];
const CHINESE_WEEKDAYS: [char; 7] = ['日', '一', '二', '三', '四', '五', '六'];
const WEEK_WORDS: [&str; 3] = ["星期", "禮拜", "週"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// What is left of the sentence once what was understood is taken out.
    pub title: String,
    pub date: Option<Date>,
    pub start: Option<(u32, u32)>,
    pub end: Option<(u32, u32)>,
    pub all_day: bool,
    pub calendar: Option<usize>,
    /// Where in the sentence what was understood is, in bytes, to show it marked.
    pub marks: Vec<Range<usize>>,
}

#[derive(Clone, Copy, Debug)]
enum Token {
    Day(Date),
    Time((u32, u32)),
    Range((u32, u32), (u32, u32)),
    /// How long it lasts, in minutes.
    Length(u32),
    /// It begins this many minutes from now.
    FromNow(u32),
    AllDay,
}

fn is_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric()
}

/// Whether `word` is at `at` of `chars`, as a whole word where it is made of letters.
fn word_at(chars: &[char], at: usize, word: &str) -> Option<usize> {
    let length = word.chars().count();
    let found: String = chars.get(at..at + length)?.iter().collect();
    if found != word {
        return None;
    }
    let letters = word.chars().next().is_some_and(is_word_char);
    let before_is_word = at > 0 && is_word_char(chars[at - 1]);
    let after_is_word = chars.get(at + length).is_some_and(|next| is_word_char(*next));
    (!(letters && (before_is_word || after_is_word && word.chars().last().is_some_and(is_word_char)))).then_some(length)
}

fn first_word(chars: &[char], at: usize, words: &[&str]) -> Option<usize> {
    words.iter().find_map(|word| word_at(chars, at, word))
}

/// A number written in digits, or in Chinese up to ninety-nine.
fn number_at(chars: &[char], at: usize) -> Option<(u32, usize)> {
    let digits = chars[at..].iter().take_while(|character| character.is_ascii_digit()).count();
    if digits > 0 {
        let text: String = chars[at..at + digits].iter().collect();
        return text.parse().ok().map(|value| (value, digits));
    }
    let digit = |character: char| "零一二三四五六七八九".chars().position(|found| found == character).map(|at| at as u32);
    let first = *chars.get(at)?;
    let second = chars.get(at + 1).copied();
    let third = chars.get(at + 2).copied();
    match (first, second, third) {
        ('兩', ..) => Some((2, 1)),
        ('十', Some(unit), _) if digit(unit).is_some_and(|unit| unit > 0) => Some((10 + digit(second?)?, 2)),
        ('十', ..) => Some((10, 1)),
        (tens, Some('十'), Some(unit)) if digit(unit).is_some_and(|unit| unit > 0) && digit(tens).is_some() => {
            Some((digit(tens)? * 10 + digit(unit)?, 3))
        }
        (tens, Some('十'), _) if digit(tens).is_some_and(|tens| tens > 0) => Some((digit(tens)? * 10, 2)),
        (single, ..) => digit(single).filter(|&value| value > 0).map(|value| (value, 1)),
    }
}

fn minutes_of(hour: u32, minute: u32) -> Option<(u32, u32)> {
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// The hour a day part gives to one said in twelve.
fn in_day_part(part: &str, hour: u32) -> u32 {
    match part {
        "下午" | "晚上" | "傍晚" | "pm" if hour < NOON => hour + NOON,
        "中午" if hour < 6 => hour + NOON,
        _ => hour,
    }
}

const DAY_PARTS: [(&str, u32); 8] = [
    ("凌晨", 3),
    ("早上", MORNING_HOUR),
    ("上午", MORNING_HOUR),
    ("中午", NOON),
    ("下午", AFTERNOON_HOUR),
    ("傍晚", EVENING_HOUR),
    ("晚上", EVENING_HOUR),
    ("半夜", 0),
];

/// A clock time at `at`: 「下午3點」「3:30」「2pm」「at noon」, with how many characters it is.
fn time_at(chars: &[char], at: usize) -> Option<((u32, u32), usize)> {
    let mut cursor = at;
    let mut part: Option<&str> = None;
    for (name, _) in DAY_PARTS {
        if word_at(chars, cursor, name).is_some() {
            part = Some(name);
            cursor += name.chars().count();
            break;
        }
    }
    if let Some(skip) = first_word(chars, cursor, &["at "]) {
        cursor += skip;
    }
    if first_word(chars, cursor, &["noon"]).is_some() {
        return Some(((NOON, 0), cursor + 4 - at));
    }
    if first_word(chars, cursor, &["midnight"]).is_some() {
        return Some(((0, 0), cursor + 8 - at));
    }
    let Some((hour, digits)) = number_at(chars, cursor) else {
        // A day part alone is a time of its own.
        let (_, hour) = DAY_PARTS.iter().find(|(name, _)| Some(*name) == part)?;
        return Some(((*hour, 0), cursor - at));
    };
    let mut end = cursor + digits;
    let mut minute = DEFAULT_MINUTES;
    let mut marked = false;
    match chars.get(end) {
        Some(':' | '：') => {
            let (found, length) = number_at(chars, end + 1).filter(|(_, length)| *length == 2)?;
            minute = found;
            end += 1 + length;
            marked = true;
        }
        Some('點' | '时' | '時' | '点') => {
            end += 1;
            marked = true;
            if chars.get(end) == Some(&'半') {
                minute = 30;
                end += 1;
            } else if let Some((found, length)) = number_at(chars, end) {
                if chars.get(end + length) == Some(&'分') {
                    minute = found;
                    end += length + 1;
                }
            }
        }
        _ => {}
    }
    let mut hour = hour;
    for suffix in ["am", "pm"] {
        // Joined to the number, so only what follows it has to end the word.
        let joined = chars.get(end..end + 2).is_some_and(|found| found.iter().collect::<String>() == suffix)
            && !chars.get(end + 2).is_some_and(|next| is_word_char(*next));
        if joined {
            if suffix == "pm" && hour < NOON {
                hour += NOON;
            }
            if suffix == "am" && hour == NOON {
                hour = 0;
            }
            end += 2;
            marked = true;
        }
    }
    // A bare number is a time only where a word before it says so.
    let said = marked || part.is_some() || chars[at..cursor].iter().collect::<String>().trim() == "at";
    if !said {
        return None;
    }
    if let Some(name) = part {
        hour = in_day_part(name, hour);
    }
    minutes_of(hour, minute).map(|time| (time, end - at))
}

fn range_at(chars: &[char], at: usize) -> Option<(Token, usize)> {
    let mut cursor = at;
    if let Some(skip) = first_word(chars, cursor, &["from "]) {
        cursor += skip;
    }
    let (start, length) = bare_time_at(chars, cursor)?;
    cursor += length;
    let joined = ["到", "至", "~", "-", "–", " to ", " until ", "-"].iter().find_map(|word| {
        let word_length = word.chars().count();
        let found: String = chars.get(cursor..cursor + word_length)?.iter().collect();
        (found == *word).then_some(word_length)
    })?;
    cursor += joined;
    let (end, length) = bare_time_at(chars, cursor)?;
    Some((Token::Range(start, after(start, end)), cursor + length - at))
}

/// The end of a range, which is the afternoon where saying so was left out: 3pm to 4 ends at 4pm.
fn after(start: (u32, u32), end: (u32, u32)) -> (u32, u32) {
    if end <= start && end.0 < NOON {
        (end.0 + NOON, end.1)
    } else {
        end
    }
}

/// A time in a range, where a bare number is an hour.
fn bare_time_at(chars: &[char], at: usize) -> Option<((u32, u32), usize)> {
    if let Some(found) = time_at(chars, at) {
        return Some(found);
    }
    let (hour, digits) = number_at(chars, at)?;
    let mut length = digits;
    if matches!(chars.get(at + length), Some('點' | '点' | '時')) {
        length += 1;
    }
    minutes_of(hour, 0).map(|time| (time, length))
}

fn length_at(chars: &[char], at: usize) -> Option<(Token, usize)> {
    let mut cursor = at;
    let english = first_word(chars, cursor, &["for "]);
    if let Some(skip) = english {
        cursor += skip;
    }
    if chars.get(cursor) == Some(&'半') && chars.get(cursor + 1) == Some(&'小') {
        return Some((Token::Length(30), cursor + 3 - at));
    }
    let (amount, digits) = number_at(chars, cursor)?;
    cursor += digits;
    if chars.get(cursor) == Some(&'個') {
        cursor += 1;
    }
    let units: [(&str, u32); 9] = [
        ("小時", MINUTES_IN_HOUR),
        ("小时", MINUTES_IN_HOUR),
        ("分鐘", 1),
        ("分钟", 1),
        ("hours", MINUTES_IN_HOUR),
        ("hour", MINUTES_IN_HOUR),
        ("hrs", MINUTES_IN_HOUR),
        ("minutes", 1),
        ("mins", 1),
    ];
    // Spoken first, with no "for": 「2 小時」 is a length only when what follows it says so.
    let spaced = if chars.get(cursor) == Some(&' ') && english.is_some() { 1 } else { 0 };
    for (unit, factor) in units {
        if let Some(unit_length) = word_at(chars, cursor + spaced, unit) {
            let after = cursor + spaced + unit_length;
            // 「2 小時後」 and 「in 2 hours」 say when it begins, not how long it is.
            if chars.get(after) == Some(&'後') {
                return Some((Token::FromNow(amount * factor), after + 1 - at));
            }
            if english.is_none() && unit.is_ascii() {
                return None;
            }
            return Some((Token::Length((amount * factor).min(MINUTES_IN_DAY)), after - at));
        }
    }
    None
}

fn from_now_at(chars: &[char], at: usize) -> Option<(Token, usize)> {
    let skip = first_word(chars, at, &["in "])?;
    let (amount, digits) = number_at(chars, at + skip)?;
    let mut cursor = at + skip + digits;
    if chars.get(cursor) == Some(&' ') {
        cursor += 1;
    }
    for (unit, factor) in [("hours", MINUTES_IN_HOUR), ("hour", MINUTES_IN_HOUR), ("minutes", 1), ("mins", 1)] {
        if let Some(length) = word_at(chars, cursor, unit) {
            return Some((Token::FromNow(amount * factor), cursor + length - at));
        }
    }
    None
}

fn weekday_at(chars: &[char], at: usize, today: Date) -> Option<(Token, usize)> {
    let mut cursor = at;
    let mut next = false;
    for word in ["下個", "下"] {
        if word_at(chars, cursor, word).is_some() {
            next = true;
            cursor += word.chars().count();
            break;
        }
    }
    for word in ["next ", "on ", "at "] {
        if let Some(length) = word_at(chars, cursor, word) {
            next |= word == "next ";
            cursor += length;
            break;
        }
    }
    let target = if let Some(skip) = WEEK_WORDS.iter().find_map(|word| word_at(chars, cursor, word)) {
        let name = *chars.get(cursor + skip)?;
        let weekday = CHINESE_WEEKDAYS.iter().position(|found| *found == name || (name == '天' && *found == '日'))?;
        (weekday, skip + 1)
    } else {
        let (weekday, name) = ENGLISH_WEEKDAYS.iter().enumerate().find(|(_, name)| word_at(chars, cursor, name).is_some())?;
        (weekday, name.chars().count())
    };
    let (weekday, length) = target;
    let from_monday = |day: usize| (day + 6) % 7;
    let day = if next {
        // The week after this one, which begins on the coming Monday.
        let monday_after = DAYS_IN_WEEK - from_monday(today.weekday()) as i64;
        today.plus_days(monday_after + from_monday(weekday) as i64)
    } else {
        let ahead = (weekday + 7 - today.weekday()) % 7;
        today.plus_days(ahead as i64)
    };
    Some((Token::Day(day), cursor + length - at))
}

fn day_at(chars: &[char], at: usize, today: Date) -> Option<(Token, usize)> {
    let words: [(&str, i64); 9] = [
        ("大後天", 3),
        ("後天", 2),
        ("明天", 1),
        ("明日", 1),
        ("tomorrow", 1),
        ("今天", 0),
        ("today", 0),
        ("昨天", -1),
        ("yesterday", -1),
    ];
    if let Some((word, offset)) = words.iter().find(|(word, _)| word_at(chars, at, word).is_some()) {
        return Some((Token::Day(today.plus_days(*offset)), word.chars().count()));
    }
    if let Some(found) = weekday_at(chars, at, today) {
        return Some(found);
    }
    relative_day_at(chars, at, today).or_else(|| calendar_day_at(chars, at, today))
}

/// 「3天後」「in 3 days」「in a week」「兩週後」.
fn relative_day_at(chars: &[char], at: usize, today: Date) -> Option<(Token, usize)> {
    if let Some(skip) = first_word(chars, at, &["in a week"]) {
        return Some((Token::Day(today.plus_days(DAYS_IN_WEEK)), skip));
    }
    let english = first_word(chars, at, &["in "]);
    let cursor = at + english.unwrap_or(0);
    let (amount, digits) = number_at(chars, cursor)?;
    let mut end = cursor + digits;
    if chars.get(end) == Some(&' ') {
        end += 1;
    }
    let units: [(&str, i64); 6] = [("天", 1), ("日", 1), ("週", 7), ("周", 7), ("days", 7 / 7), ("weeks", 7)];
    let (unit, factor) = units.iter().find(|(unit, _)| word_at(chars, end, unit).is_some())?;
    end += unit.chars().count();
    if english.is_none() {
        // 「3天後」 only; 「3天」 alone is how long something is.
        if chars.get(end) != Some(&'後') {
            return None;
        }
        end += 1;
    }
    Some((Token::Day(today.plus_days(i64::from(amount) * factor)), end - at))
}

/// 「8月12日」「8/12」「august 12」, the next one that is not past.
fn calendar_day_at(chars: &[char], at: usize, today: Date) -> Option<(Token, usize)> {
    let named = MONTH_NAMES.iter().enumerate().find_map(|(index, name)| {
        let full = word_at(chars, at, name).map(|length| (index as u32 + 1, length));
        full.or_else(|| word_at(chars, at, &name[..3]).map(|length| (index as u32 + 1, length)))
    });
    let (month, day, length) = if let Some((month, length)) = named {
        let mut cursor = at + length;
        if chars.get(cursor) == Some(&' ') {
            cursor += 1;
        }
        let (day, digits) = number_at(chars, cursor).filter(|(day, _)| *day <= 31)?;
        (month, day, cursor + digits - at)
    } else {
        let (month, digits) = number_at(chars, at)?;
        let separator = *chars.get(at + digits)?;
        let after = at + digits + 1;
        let (day, day_digits) = number_at(chars, after)?;
        match separator {
            '/' => (month, day, after + day_digits - at),
            '月' => {
                let closing = matches!(chars.get(after + day_digits), Some('日' | '號' | '号'));
                (month, day, after + day_digits + usize::from(closing) - at)
            }
            _ => return None,
        }
    };
    let this_year = Date::new(today.year, month, day)?;
    let found = if this_year < today { Date::new(today.year + 1, month, day)? } else { this_year };
    Some((Token::Day(found), length))
}

fn all_day_at(chars: &[char], at: usize) -> Option<(Token, usize)> {
    first_word(chars, at, &["整天", "全天", "整日", "all day", "full day"]).map(|length| (Token::AllDay, length))
}

fn token_at(chars: &[char], at: usize, today: Date) -> Option<(Token, usize)> {
    all_day_at(chars, at)
        .or_else(|| range_at(chars, at))
        .or_else(|| from_now_at(chars, at))
        .or_else(|| day_at(chars, at, today))
        .or_else(|| time_at(chars, at).map(|(time, length)| (Token::Time(time), length)))
        .or_else(|| length_at(chars, at))
}

/// The calendar a 「/word」 names: the first whose name has the word's letters in order.
fn calendar_named(names: &[String], query: &str) -> Option<usize> {
    let query = query.to_lowercase();
    let contains = |name: &String| name.to_lowercase().contains(&query);
    let in_order = |name: &String| {
        let mut letters = query.chars();
        let mut wanted = letters.next();
        for character in name.to_lowercase().chars() {
            if Some(character) == wanted {
                wanted = letters.next();
            }
        }
        wanted.is_none()
    };
    names.iter().position(contains).or_else(|| names.iter().position(in_order))
}

pub fn parse(sentence: &str, today: Date, calendars: &[String], now: (u32, u32)) -> Parsed {
    let lowered: Vec<char> = sentence.to_lowercase().chars().collect();
    let original: Vec<char> = sentence.chars().collect();
    // Lowercasing may change how many characters there are; then the original is read as it is.
    let chars: &[char] = if lowered.len() == original.len() { &lowered } else { &original };
    let mut parsed = Parsed::default();
    let mut kept = String::new();
    let mut marks: Vec<Range<usize>> = Vec::new();
    let mut length_minutes: Option<u32> = None;
    let mut from_now: Option<u32> = None;
    let mut at = 0;
    while at < chars.len() {
        // The first word is always the title, whatever it says.
        let found = if at == 0 { None } else { token_at(chars, at, today) };
        let called = (chars[at] == '/' && at > 0).then(|| {
            let name: String = chars[at + 1..].iter().take_while(|character| !character.is_whitespace()).collect();
            (calendar_named(calendars, &name), name.chars().count() + 1)
        });
        match (found, called) {
            (_, Some((Some(calendar), length))) => {
                parsed.calendar = Some(calendar);
                marks.push(span(&original, at, at + length));
                at += length;
            }
            (Some((token, length)), _) => {
                match token {
                    Token::Day(day) => parsed.date = Some(day),
                    Token::Time(time) => parsed.start = Some(time),
                    Token::Range(start, end) => (parsed.start, parsed.end) = (Some(start), Some(end)),
                    Token::Length(minutes) => length_minutes = Some(minutes),
                    Token::FromNow(minutes) => from_now = Some(minutes),
                    Token::AllDay => parsed.all_day = true,
                }
                marks.push(span(&original, at, at + length));
                at += length;
            }
            _ => {
                kept.push(original[at]);
                at += 1;
            }
        }
    }
    if parsed.start.is_none() {
        if let Some(minutes) = from_now {
            let total = now.0 * MINUTES_IN_HOUR + now.1 + minutes;
            parsed.date.get_or_insert(today);
            if total >= MINUTES_IN_DAY {
                parsed.date = parsed.date.map(|day| day.plus_days(i64::from(total / MINUTES_IN_DAY)));
            }
            let total = total % MINUTES_IN_DAY;
            parsed.start = Some((total / MINUTES_IN_HOUR, total % MINUTES_IN_HOUR));
        }
    }
    if parsed.all_day {
        parsed.start = None;
        parsed.end = None;
    } else if let (Some((hour, minute)), Some(minutes), None) = (parsed.start, length_minutes, parsed.end) {
        let total = (hour * MINUTES_IN_HOUR + minute + minutes) % MINUTES_IN_DAY;
        parsed.end = Some((total / MINUTES_IN_HOUR, total % MINUTES_IN_HOUR));
    }
    parsed.title = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    parsed.marks = marks;
    parsed
}

/// The bytes of the original sentence for the characters from `from` to `to`.
fn span(original: &[char], from: usize, to: usize) -> Range<usize> {
    let bytes = |count: usize| original[..count.min(original.len())].iter().map(|character| character.len_utf8()).sum::<usize>();
    bytes(from)..bytes(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A Wednesday.
    const TODAY: Date = Date { year: 2026, month: 11, day: 18 };

    fn names() -> Vec<String> {
        ["個人", "Work", "Sport", "節日"].map(String::from).to_vec()
    }

    fn parse_it(sentence: &str) -> Parsed {
        parse(sentence, TODAY, &names(), (10, 0))
    }

    #[test]
    fn plain_words_are_a_title() {
        let parsed = parse_it("產品週會");
        assert_eq!(parsed.title, "產品週會");
        assert_eq!((parsed.date, parsed.start, parsed.all_day), (None, None, false));
    }

    #[test]
    fn english_dates_and_times_are_understood() {
        let parsed = parse_it("Pickleball with Tom next Friday from 10 to 12 /sport");
        assert_eq!(parsed.title, "Pickleball with Tom");
        assert_eq!(parsed.date, Date::new(2026, 11, 27));
        assert_eq!((parsed.start, parsed.end), (Some((10, 0)), Some((12, 0))));
        assert_eq!(parsed.calendar, Some(2));
    }

    #[test]
    fn the_end_of_a_range_follows_the_start_into_the_afternoon() {
        let parsed = parse_it("後天 下午3點-4點");
        assert_eq!((parsed.start, parsed.end), (Some((15, 0)), Some((16, 0))));
        let parsed = parse_it("meeting 9-5");
        assert_eq!((parsed.start, parsed.end), (Some((9, 0)), Some((17, 0))));
        // A range that already runs forward is left as it is.
        let parsed = parse_it("meeting 10:00-11:30");
        assert_eq!(parsed.end, Some((11, 30)));
    }

    #[test]
    fn chinese_dates_and_times_are_understood() {
        let parsed = parse_it("牙醫回診 明天下午3點半");
        assert_eq!(parsed.title, "牙醫回診");
        assert_eq!(parsed.date, Date::new(2026, 11, 19));
        assert_eq!(parsed.start, Some((15, 30)));
    }

    #[test]
    fn lengths_end_what_begins() {
        let parsed = parse_it("Dentist today at 2pm for 90 minutes");
        assert_eq!((parsed.title.as_str(), parsed.start, parsed.end), ("Dentist", Some((14, 0)), Some((15, 30))));
        let parsed = parse_it("開會 後天 早上9點 2小時");
        assert_eq!((parsed.date, parsed.start, parsed.end), (Date::new(2026, 11, 20), Some((9, 0)), Some((11, 0))));
    }

    #[test]
    fn a_day_in_the_week_is_the_next_one() {
        assert_eq!(parse_it("Lunch on Friday").date, Date::new(2026, 11, 20));
        assert_eq!(parse_it("Lunch on Wednesday").date, Date::new(2026, 11, 18));
        assert_eq!(parse_it("吃飯 週五").date, Date::new(2026, 11, 20));
        assert_eq!(parse_it("吃飯 下週五").date, Date::new(2026, 11, 27));
        assert_eq!(parse_it("Lunch next Monday").date, Date::new(2026, 11, 23));
    }

    #[test]
    fn days_are_counted_from_today() {
        assert_eq!(parse_it("Trip in 3 days").date, Date::new(2026, 11, 21));
        assert_eq!(parse_it("Trip in a week").date, Date::new(2026, 11, 25));
        assert_eq!(parse_it("出差 3天後").date, Date::new(2026, 11, 21));
    }

    #[test]
    fn a_named_day_is_the_next_one() {
        assert_eq!(parse_it("Party August 12").date, Date::new(2027, 8, 12));
        assert_eq!(parse_it("聚會 12月25日").date, Date::new(2026, 12, 25));
        assert_eq!(parse_it("聚會 12/25").date, Date::new(2026, 12, 25));
    }

    #[test]
    fn a_whole_day_has_no_time() {
        let parsed = parse_it("Birthday tomorrow all day");
        assert_eq!((parsed.all_day, parsed.start, parsed.date), (true, None, Date::new(2026, 11, 19)));
        assert!(parse_it("生日 整天").all_day);
    }

    #[test]
    fn a_wait_sets_when_it_begins() {
        let parsed = parse_it("Call in 2 hours");
        assert_eq!((parsed.title.as_str(), parsed.start), ("Call", Some((12, 0))));
    }

    #[test]
    fn the_first_word_is_always_the_title() {
        assert_eq!(parse_it("Tomorrow is a holiday").title, "Tomorrow is a holiday");
        assert_eq!(parse_it("Tomorrow is a holiday tomorrow").date, Date::new(2026, 11, 19));
    }

    #[test]
    fn what_is_understood_is_marked() {
        let sentence = "Lunch tomorrow at noon /work";
        let parsed = parse_it(sentence);
        let marked: Vec<&str> = parsed.marks.iter().map(|mark| &sentence[mark.clone()]).collect();
        assert_eq!(marked, ["tomorrow", "at noon", "/work"]);
        assert_eq!((parsed.start, parsed.calendar), (Some((12, 0)), Some(1)));
    }

    #[test]
    fn a_calendar_is_named_by_part_of_its_name() {
        assert_eq!(calendar_named(&names(), "wo"), Some(1));
        assert_eq!(calendar_named(&names(), "spt"), Some(2));
        assert_eq!(calendar_named(&names(), "個"), Some(0));
        assert_eq!(calendar_named(&names(), "zzz"), None);
    }
}
