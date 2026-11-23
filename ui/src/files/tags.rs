//! Tags: names kept on each file in its `user.xdg.tags` attribute, a comma between two, as
//! the hover panel reads them. The seven the sidebar lists each have a colour.

use gtk::prelude::*;
use gtk::{gio, glib};

/// GIO's name for the `user.xdg.tags` extended attribute.
pub const ATTRIBUTE: &str = "xattr::xdg.tags";

/// The tags the sidebar offers, with the colour each is drawn in (a CSS class of files.rs).
pub const TAGS: [(&str, &str); 7] = [
    ("重要", "red"),
    ("工作", "orange"),
    ("黃色", "yellow"),
    ("個人", "green"),
    ("旅行", "blue"),
    ("紫色", "purple"),
    ("灰色", "gray"),
];

pub fn parse(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_owned());
        }
    }
    tags
}

pub fn of(info: &gio::FileInfo) -> Vec<String> {
    info.attribute_string(ATTRIBUTE)
        .map(|text| parse(&unescape(&text)))
        .unwrap_or_default()
}

/// GIO gives an attribute's bytes outside printable ASCII, and `\`, as `\xNN`; tags are
/// UTF-8.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'x'))
            .then(|| text.get(i + 2..i + 4))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A tag's dot, in each of the tags' colours.
pub const CSS: &str = "
.nf-dot { min-width: 10px; min-height: 10px; border-radius: 5px; box-shadow: 0 0 0 1.5px var(--view-bg-color); }
.nf-dots > .nf-dot + .nf-dot { margin-left: -3px; }
.red { background-color: #d4493f; }
.orange { background-color: #dc8436; }
.yellow { background-color: #cfa92e; }
.green { background-color: #4f9a61; }
.blue { background-color: #4a7fc0; }
.purple { background-color: #8e63b5; }
.gray { background-color: #8e8c90; }
";

/// The colour a tag is drawn in; one the sidebar does not list is gray.
pub fn color(tag: &str) -> &'static str {
    TAGS.iter()
        .find(|(name, _)| *name == tag)
        .map_or("gray", |(_, color)| color)
}

/// `tags` with `tag` added or taken away.
pub fn toggled(tags: &[String], tag: &str, on: bool) -> Vec<String> {
    let mut tags: Vec<String> = tags.iter().filter(|t| *t != tag).cloned().collect();
    if on {
        tags.push(tag.to_owned());
    }
    tags
}

pub fn write(file: &gio::File, tags: &[String]) -> Result<(), glib::Error> {
    file.set_attribute_string(
        ATTRIBUTE,
        &tags.join(","),
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    )
}

/// A row of dots, one a tag, each in its colour, overlapping a little as the Finder's do.
pub fn dots(tags: &[String]) -> gtk::Box {
    let row = gtk::Box::builder()
        .css_classes(["nf-dots"])
        .valign(gtk::Align::Center)
        .build();
    fill_dots(&row, tags);
    row
}

pub fn fill_dots(row: &gtk::Box, tags: &[String]) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }
    for tag in tags {
        row.append(&dot(tag));
    }
    row.set_visible(!tags.is_empty());
}

pub fn dot(tag: &str) -> gtk::Widget {
    gtk::Box::builder()
        .css_classes(["nf-dot", color(tag)])
        .valign(gtk::Align::Center)
        .tooltip_text(tag)
        .build()
        .upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_read_once_each_without_spaces() {
        assert_eq!(parse("重要, 工作,,重要"), ["重要", "工作"]);
        assert!(parse("").is_empty());
    }

    #[test]
    fn a_tag_is_added_last_and_taken_away_anywhere() {
        let tags = parse("重要,旅行");
        assert_eq!(toggled(&tags, "工作", true), ["重要", "旅行", "工作"]);
        assert_eq!(toggled(&tags, "重要", false), ["旅行"]);
        assert_eq!(toggled(&tags, "旅行", true), ["重要", "旅行"]);
    }

    #[test]
    fn tags_are_read_as_gio_escapes_them() {
        assert_eq!(unescape("\\xe9\\x87\\x8d\\xe8\\xa6\\x81,A\\x5cB"), "重要,A\\B");
        assert_eq!(unescape("旅行,工作"), "旅行,工作");
    }

    #[test]
    fn an_unknown_tag_is_gray() {
        assert_eq!(color("旅行"), "blue");
        assert_eq!(color("專案 A"), "gray");
    }
}
