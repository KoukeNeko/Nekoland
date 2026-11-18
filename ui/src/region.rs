//! Settings' 語言與地區 page: the system's language and the formats of dates, numbers and money,
//! systemd's `localectl`, with how each looks; and adding a language the system has not yet
//! been set up for, through `locale-gen`.

use std::{process::Command, rc::Rc};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{alert, as_root, group, output};

/// The categories 地區格式 sets, all but the language of messages.
const FORMATS: [&str; 5] = ["LC_TIME", "LC_NUMERIC", "LC_MONETARY", "LC_MEASUREMENT", "LC_PAPER"];

/// What a locale is called, in its own language where it has one Nekoland's people read.
fn locale_name(locale: &str) -> String {
    let code = locale.split(['.', '@']).next().unwrap_or(locale);
    let name = match code {
        "C" | "POSIX" => "未指定（C）",
        "zh_TW" => "中文（台灣）",
        "zh_HK" => "中文（香港）",
        "zh_CN" => "中文（中國）",
        "zh_SG" => "中文（新加坡）",
        "en_US" => "English (United States)",
        "en_GB" => "English (United Kingdom)",
        "en_AU" => "English (Australia)",
        "en_CA" => "English (Canada)",
        "ja_JP" => "日本語（日本）",
        "ko_KR" => "한국어（대한민국）",
        "fr_FR" => "Français (France)",
        "de_DE" => "Deutsch (Deutschland)",
        "es_ES" => "Español (España)",
        "it_IT" => "Italiano (Italia)",
        "pt_BR" => "Português (Brasil)",
        "pt_PT" => "Português (Portugal)",
        "ru_RU" => "Русский (Россия)",
        "th_TH" => "ไทย (ประเทศไทย)",
        "vi_VN" => "Tiếng Việt (Việt Nam)",
        "id_ID" => "Bahasa Indonesia (Indonesia)",
        "nl_NL" => "Nederlands (Nederland)",
        _ => return code.to_string(),
    };
    name.to_string()
}

/// `localectl status`'s "System Locale" settings, as NAME=value pairs.
fn system_locale(status: &str) -> Vec<(String, String)> {
    let mut lines = status.lines().skip_while(|line| !line.contains("System Locale:"));
    let first = lines.next().and_then(|line| line.split_once("System Locale:")).map(|(_, rest)| rest);
    // Further settings follow on lines of their own, indented, until the next "Name: value".
    first
        .into_iter()
        .chain(lines.take_while(|line| !line.contains(": ")))
        .filter_map(|setting| setting.trim().split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// The locales the system can be set to, and those it could be set up for, from glibc's list.
fn supported(listed: &str) -> Vec<String> {
    listed
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(_, charset)| *charset == "UTF-8")
        .map(|(locale, _)| locale.to_string())
        .collect()
}

/// A date, a time, a number and the currency's symbol, as `locale` writes them.
fn samples_in(locale: &str) -> [String; 4] {
    [
        in_locale(locale, "date", &["+%x"]),
        in_locale(locale, "date", &["+%X"]),
        number_in(&in_locale(locale, "locale", &["decimal_point", "thousands_sep"])),
        in_locale(locale, "locale", &["currency_symbol"]),
    ]
}

/// 1234567.89 with the separators `locale decimal_point thousands_sep` prints, a line each.
fn number_in(separators: &str) -> String {
    let mut lines = separators.lines();
    let point = lines.next().filter(|point| !point.is_empty()).unwrap_or(".");
    let thousands = lines.next().unwrap_or_default();
    format!("1{thousands}234{thousands}567{point}89")
}

/// What `program` prints under `locale`, for the samples.
fn in_locale(locale: &str, program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .env("LC_ALL", locale)
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn region_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    fill(&page);
    page
}

fn fill(page: &adw::PreferencesPage) {
    let settings = system_locale(&output("localectl", &["status"]).unwrap_or_default());
    let get = |name: &str| settings.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    let language = get("LANG").unwrap_or_else(|| "C.UTF-8".into());
    let formats = get("LC_TIME").unwrap_or_else(|| language.clone());
    let mut locales: Vec<String> =
        output("localectl", &["list-locales"]).unwrap_or_default().lines().map(str::to_string).collect();
    for current in [&language, &formats] {
        if !locales.contains(current) {
            locales.push(current.clone());
        }
    }
    let names: Vec<String> = locales.iter().map(|locale| locale_name(locale)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let locales = Rc::new(locales);
    let position = |wanted: &str| locales.iter().position(|locale| locale == wanted).unwrap_or(0) as u32;

    let rows = group("");
    let language_row = adw::ComboRow::builder()
        .title("語言")
        .model(&gtk::StringList::new(&names))
        .selected(position(&language))
        .build();
    let formats_row = adw::ComboRow::builder()
        .title("地區格式")
        .model(&gtk::StringList::new(&names))
        .selected(position(&formats))
        .build();
    let add = adw::ActionRow::builder().title("加入語言⋯").activatable(true).build();
    add.connect_activated(glib::clone!(
        #[weak]
        page,
        move |_| add_language(&page)
    ));
    rows.add(&language_row);
    rows.add(&formats_row);
    rows.add(&add);

    let samples = group("範例");
    let shown: Vec<gtk::Label> = ["日期", "時間", "數字", "貨幣符號"]
        .into_iter()
        .map(|title| {
            let row = adw::ActionRow::builder().title(title).build();
            let label = gtk::Label::builder().css_classes(["dim-label"]).selectable(true).build();
            row.add_suffix(&label);
            samples.add(&row);
            label
        })
        .collect();
    let show_samples = move |locale: &str| {
        for (label, text) in shown.iter().zip(samples_in(locale)) {
            label.set_label(&text);
        }
    };
    show_samples(&formats);

    // Both are set at once, as localectl replaces what it is given.
    let apply = {
        let (locales, language_row, formats_row) = (locales.clone(), language_row.clone(), formats_row.clone());
        Rc::new(move |changed: &adw::ComboRow| {
            let language = &locales[language_row.selected() as usize];
            let formats = &locales[formats_row.selected() as usize];
            let mut args = vec!["localectl".to_string(), "set-locale".into(), format!("LANG={language}")];
            if formats != language {
                args.extend(FORMATS.iter().map(|category| format!("{category}={formats}")));
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            changed.set_sensitive(false);
            let changed = changed.clone();
            as_root("變更語言與地區", &args, None, move |worked| {
                changed.set_sensitive(true);
                if worked {
                    changed.set_subtitle("下次登入時生效");
                } else {
                    alert::notice(&changed, "無法變更語言與地區", Some("請確認密碼正確後再試一次。"));
                }
            });
        })
    };
    language_row.connect_selected_notify(glib::clone!(
        #[strong]
        apply,
        move |row| apply(row)
    ));
    let samples_for = locales.clone();
    formats_row.connect_selected_notify(move |row| {
        show_samples(&samples_for[row.selected() as usize]);
        apply(row);
    });

    page.add(&rows);
    page.add(&samples);
}

/// Sets the system up for one more language, chosen from those glibc has.
fn add_language(page: &adw::PreferencesPage) {
    let installed: Vec<String> =
        output("localectl", &["list-locales"]).unwrap_or_default().lines().map(str::to_string).collect();
    let choices: Vec<String> = supported(&std::fs::read_to_string("/usr/share/i18n/SUPPORTED").unwrap_or_default())
        .into_iter()
        .filter(|locale| !installed.contains(locale))
        .collect();
    let labels: Vec<String> = choices.iter().map(|locale| format!("{} — {locale}", locale_name(locale))).collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let list = gtk::DropDown::builder()
        .model(&gtk::StringList::new(&labels))
        .enable_search(true)
        .width_request(alert::FIELD_WIDTH)
        .build();
    list.set_expression(Some(gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        gtk::Expression::NONE,
        "string",
    )));
    if let Some(index) = choices.iter().position(|locale| locale == "zh_TW.UTF-8") {
        list.set_selected(index as u32);
    }
    let alert = adw::AlertDialog::new(Some("加入語言"), None);
    alert.set_extra_child(Some(&list));
    alert.add_responses(&[("cancel", "取消"), ("do", "加入")]);
    alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("do"));
    alert.set_close_response("cancel");
    let page = page.clone();
    alert.choose(&page.clone(), gio::Cancellable::NONE, move |response| {
        let Some(locale) = choices.get(list.selected() as usize).filter(|_| response == "do") else {
            return;
        };
        let page = page.clone();
        as_root("加入語言", &["locale-gen", locale], None, move |worked| {
            if !worked {
                return alert::notice(&page, "無法加入語言", Some("請確認密碼正確後再試一次。"));
            }
            // Rebuilt, so the new language is among the choices.
            refill(&page);
        });
    });
}

fn refill(page: &adw::PreferencesPage) {
    let mut groups = Vec::new();
    let mut child = page.first_child();
    while let Some(widget) = child {
        collect_groups(&widget, &mut groups);
        child = widget.next_sibling();
    }
    for old in groups {
        page.remove(&old);
    }
    fill(page);
}

fn collect_groups(widget: &gtk::Widget, groups: &mut Vec<adw::PreferencesGroup>) {
    if let Some(found) = widget.downcast_ref::<adw::PreferencesGroup>() {
        return groups.push(found.clone());
    }
    let mut child = widget.first_child();
    while let Some(inner) = child {
        collect_groups(&inner, groups);
        child = inner.next_sibling();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_locale_is_read() {
        let status = "System Locale: LANG=zh_TW.UTF-8\n                  LC_TIME=en_GB.UTF-8\n    VC Keymap: (unset)\n";
        assert_eq!(
            system_locale(status),
            [("LANG".into(), "zh_TW.UTF-8".into()), ("LC_TIME".into(), "en_GB.UTF-8".into())]
        );
    }

    #[test]
    fn only_utf8_locales_are_offered() {
        let listed = "en_US.UTF-8 UTF-8\nen_US ISO-8859-1\nzh_TW.UTF-8 UTF-8\nzh_TW BIG5\n";
        assert_eq!(supported(listed), ["en_US.UTF-8", "zh_TW.UTF-8"]);
        assert_eq!(locale_name("zh_TW.UTF-8"), "中文（台灣）");
        assert_eq!(locale_name("sw_KE.UTF-8"), "sw_KE");
        assert_eq!(number_in(".\n,\n"), "1,234,567.89");
        assert_eq!(number_in(",\n.\n"), "1.234.567,89");
    }
}
