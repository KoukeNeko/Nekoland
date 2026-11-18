//! VTE's HTML of a range of cells as Pango markup, so that what is drawn again over the screen keeps the colours and
//! weight VTE gave the cells.

/// VTE's HTML of a row and the Pango markup of each piece; the text in it is escaped the same in both.
const HTML_TO_MARKUP: [(&str, &str); 7] = [
    ("<pre>", ""),
    ("</pre>", ""),
    ("<br>", ""),
    ("<font color=", "<span foreground="),
    ("</font>", "</span>"),
    ("<span style=\"background-color:", "<span background=\""),
    ("strike>", "s>"),
];

/// The markup of VTE's HTML; VTE escapes the text in it as Pango reads it.
pub fn html_to_markup(html: &str) -> String {
    HTML_TO_MARKUP.iter().fold(html.trim_end().to_owned(), |markup, (from, to)| markup.replace(from, to))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vte_html_reads_as_markup() {
        let html = "<pre><b><font color=\"#73D216\">me&amp;you</font></b>:<span style=\"background-color:#000000\">~</span>$<br></pre>";
        let markup = html_to_markup(html);
        assert_eq!(
            markup,
            "<b><span foreground=\"#73D216\">me&amp;you</span></b>:<span background=\"#000000\">~</span>$"
        );
        assert!(gtk::pango::parse_markup(&markup, '\0').is_ok());
    }
}
