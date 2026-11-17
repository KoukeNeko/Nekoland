//! Renaming several items at once, as the Finder does: text found in their names replaced,
//! text added before or after them, or one name with a number for each.

use std::{cell::RefCell, collections::HashSet, rc::Rc};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::browse::{EDGE, GAP};

/// The rules, in the order the toggle group shows them.
const REPLACE: u32 = 0;
const ADD: u32 = 1;
const FORMAT: u32 = 2;

#[derive(Clone, Debug, PartialEq)]
pub enum Format {
    /// 東京行 1, 東京行 2
    Index,
    /// 東京行 00001
    Counter,
    /// 東京行 2026-11-17 1
    Date,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    Replace {
        find: String,
        with: String,
    },
    Add {
        text: String,
        after: bool,
    },
    Format {
        base: String,
        format: Format,
        start: u32,
    },
}

/// A name split at its extension, which renaming leaves alone.
fn split(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    }
}

/// What each of `names` becomes; `today` is the date a dated format uses.
pub fn apply(rule: &Rule, names: &[String], today: &str) -> Vec<String> {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let (stem, extension) = split(name);
            let stem = match rule {
                Rule::Replace { find, with } if !find.is_empty() => stem.replace(find.as_str(), with),
                Rule::Replace { .. } => stem.to_owned(),
                Rule::Add { text, after: true } => format!("{stem}{text}"),
                Rule::Add { text, after: false } => format!("{text}{stem}"),
                Rule::Format { base, format, start } => {
                    let number = *start as usize + index;
                    match format {
                        Format::Index => format!("{base} {number}"),
                        Format::Counter => format!("{base} {number:05}"),
                        Format::Date => format!("{base} {today} {number}"),
                    }
                }
            };
            format!("{stem}{extension}")
        })
        .collect()
}

/// Which new names cannot be used: empty, with a slash, given twice, or taken by another
/// file in the folder (`others`, the names not being renamed).
pub fn refused(new_names: &[String], others: &HashSet<String>) -> Vec<bool> {
    new_names
        .iter()
        .map(|name| {
            name.trim().is_empty()
                || name.contains('/')
                || others.contains(name)
                || new_names.iter().filter(|n| *n == name).count() > 1
        })
        .collect()
}

/// Renames `files` to `new_names`. When one fails, those renamed before it get their old
/// names back, so that none is left under the hidden name it went through.
pub fn rename_all(files: &[gio::File], new_names: &[String]) -> Result<(), glib::Error> {
    let mut current: Vec<gio::File> = files.to_vec();
    let renamed = rename_to(&mut current, new_names);
    if renamed.is_err() {
        let old_names: Vec<String> = files.iter().map(base_name).collect();
        if let Err(err) = rename_to(&mut current, &old_names) {
            eprintln!("could not give the old names back: {err}");
        }
    }
    renamed
}

/// Gives each of `current` its name, and keeps where it is now. Through a name of its own
/// first when one item takes another's old name, so that two can swap.
fn rename_to(current: &mut [gio::File], new_names: &[String]) -> Result<(), glib::Error> {
    let old: HashSet<String> = current.iter().map(base_name).collect();
    if new_names.iter().any(|n| old.contains(n)) {
        for (index, file) in current.iter_mut().enumerate() {
            rename_one(file, &format!(".nekoland-rename-{}-{index}", std::process::id()))?;
        }
    }
    for (file, name) in current.iter_mut().zip(new_names) {
        rename_one(file, name)?;
    }
    Ok(())
}

/// Renames `file` and keeps where it is now; one that has the name already is left, as GIO
/// would refuse to rename it to itself.
fn rename_one(file: &mut gio::File, name: &str) -> Result<(), glib::Error> {
    if base_name(file) != name {
        *file = file.set_display_name(name, gio::Cancellable::NONE)?;
    }
    Ok(())
}

fn base_name(file: &gio::File) -> String {
    file.basename()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The sheet for renaming several items: how, and each name before and after.
pub fn ask(
    parent: &impl IsA<gtk::Widget>,
    files: Vec<gio::File>,
    done: impl Fn(Result<(), glib::Error>) + 'static,
) {
    let names: Vec<String> = files
        .iter()
        .filter_map(|f| f.basename().map(|b| b.to_string_lossy().into_owned()))
        .collect();
    let others: HashSet<String> = files
        .first()
        .and_then(|f| f.parent())
        .and_then(|folder| {
            folder
                .enumerate_children(
                    "standard::name",
                    gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                    gio::Cancellable::NONE,
                )
                .ok()
        })
        .map(|children| {
            children
                .filter_map(Result::ok)
                .map(|info| info.name().to_string_lossy().into_owned())
                .filter(|name| !names.contains(name))
                .collect()
        })
        .unwrap_or_default();
    let today = glib::DateTime::now_local()
        .ok()
        .and_then(|now| now.format("%Y-%m-%d").ok())
        .map(String::from)
        .unwrap_or_default();

    // Which rule, as the views are chosen: libadwaita's toggle group, made from its name as
    // files.rs makes the views', its `active` the index in REPLACE, ADD and FORMAT.
    let toggles: String = ["取代文字", "加入文字", "格式"]
        .iter()
        .map(|label| {
            format!(
                "<child><object class=\"AdwToggle\"><property name=\"label\">{label}</property></object></child>"
            )
        })
        .collect();
    let modes: gtk::Widget = gtk::Builder::from_string(&format!(
        "<interface><object class=\"AdwToggleGroup\" id=\"group\">{toggles}</object></interface>"
    ))
    .object("group")
    .expect("the toggle group just described");
    modes.set_halign(gtk::Align::Start);
    let mode = {
        let modes = modes.clone();
        move || modes.property::<u32>("active")
    };

    let field = |placeholder: &str| {
        gtk::Entry::builder()
            .placeholder_text(placeholder)
            .hexpand(true)
            .build()
    };
    // One width for every rule's labels, so the fields stay put when the rule changes.
    let labels = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
    let label = |text: &str| {
        let label = gtk::Label::builder()
            .label(text)
            .xalign(1.0)
            .css_classes(["dim-label"])
            .build();
        labels.add_widget(&label);
        label
    };
    let find = field("");
    let with = field("");
    let text = field("");
    let position = gtk::DropDown::from_strings(&["名稱之後", "名稱之前"]);
    let style = gtk::DropDown::from_strings(&["名稱加索引", "名稱加計數器", "名稱加日期"]);
    let base = field("");
    base.set_text(split(names.first().map(String::as_str).unwrap_or("")).0);
    let start = gtk::SpinButton::with_range(0.0, 99999.0, 1.0);
    start.set_value(1.0);

    let grid = |rows: Vec<(&str, gtk::Widget)>| {
        let grid = gtk::Grid::builder().column_spacing(GAP).row_spacing(GAP).build();
        for (row, (title, widget)) in rows.into_iter().enumerate() {
            grid.attach(&label(title), 0, row as i32, 1, 1);
            grid.attach(&widget, 1, row as i32, 1, 1);
        }
        grid
    };
    let fields = gtk::Stack::builder().vhomogeneous(false).build();
    fields.add_named(
        &grid(vec![
            ("尋找", find.clone().upcast()),
            ("取代為", with.clone().upcast()),
        ]),
        Some("replace"),
    );
    fields.add_named(
        &grid(vec![
            ("文字", text.clone().upcast()),
            ("位置", position.clone().upcast()),
        ]),
        Some("add"),
    );
    fields.add_named(
        &grid(vec![
            ("格式", style.clone().upcast()),
            ("自訂格式", base.clone().upcast()),
            ("起始編號", start.clone().upcast()),
        ]),
        Some("format"),
    );

    let before_after = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["nf-before-after"])
        .build();
    // An alert is as wide as what it holds.
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(EDGE)
        .width_request(472)
        .build();
    content.append(&modes);
    content.append(&fields);
    content.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(196)
            .propagate_natural_height(true)
            .child(&before_after)
            .build(),
    );
    let alert = adw::AlertDialog::new(Some(&format!("重新命名 {} 個項目", files.len())), None);
    alert.set_extra_child(Some(&content));
    alert.add_responses(&[("cancel", "取消"), ("rename", "重新命名")]);
    alert.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("rename"));
    alert.set_close_response("cancel");

    let rule = {
        let (mode, find, with, text, position, style, base, start) = (
            mode.clone(),
            find.clone(),
            with.clone(),
            text.clone(),
            position.clone(),
            style.clone(),
            base.clone(),
            start.clone(),
        );
        move || {
            let chosen = mode();
            if chosen == REPLACE {
                Rule::Replace {
                    find: find.text().into(),
                    with: with.text().into(),
                }
            } else if chosen == ADD {
                Rule::Add {
                    text: text.text().into(),
                    after: position.selected() == 0,
                }
            } else {
                Rule::Format {
                    base: base.text().into(),
                    format: match style.selected() {
                        0 => Format::Index,
                        1 => Format::Counter,
                        _ => Format::Date,
                    },
                    start: start.value() as u32,
                }
            }
        }
    };
    let new_names = Rc::new(RefCell::new(Vec::<String>::new()));
    let update = {
        let (names, others, today, fields, before_after, alert, new_names, mode) = (
            names.clone(),
            others.clone(),
            today.clone(),
            fields.clone(),
            before_after.clone(),
            alert.clone(),
            new_names.clone(),
            mode.clone(),
        );
        Rc::new(move || {
            fields.set_visible_child_name(match mode() {
                ADD => "add",
                FORMAT => "format",
                _ => "replace",
            });
            let renamed = apply(&rule(), &names, &today);
            let refused = refused(&renamed, &others);
            while let Some(row) = before_after.first_child() {
                before_after.remove(&row);
            }
            for ((old, new), bad) in names.iter().zip(&renamed).zip(&refused) {
                let line = gtk::Box::builder().spacing(GAP).build();
                line.append(
                    &gtk::Label::builder()
                        .label(old)
                        .xalign(0.0)
                        .hexpand(true)
                        .ellipsize(gtk::pango::EllipsizeMode::Middle)
                        .css_classes(["dim-label"])
                        .build(),
                );
                line.append(
                    &gtk::Image::builder()
                        .icon_name("go-next-symbolic")
                        .pixel_size(12)
                        .css_classes(["dim-label"])
                        .build(),
                );
                let after = gtk::Label::builder()
                    .label(new)
                    .xalign(0.0)
                    .hexpand(true)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build();
                if *bad {
                    after.add_css_class("error");
                }
                line.append(&after);
                before_after.append(&line);
            }
            alert.set_response_enabled("rename", !refused.contains(&true) && renamed != names);
            *new_names.borrow_mut() = renamed;
        })
    };
    {
        let update = update.clone();
        modes.connect_notify_local(Some("active"), move |_, _| update());
    }
    for entry in [&find, &with, &text, &base] {
        let update = update.clone();
        entry.connect_changed(move |_| update());
    }
    for pick in [&position, &style] {
        let update = update.clone();
        pick.connect_selected_notify(move |_| update());
    }
    {
        let update = update.clone();
        start.connect_value_changed(move |_| update());
    }
    update();

    alert.choose(parent, gio::Cancellable::NONE, move |response| {
        if response == "rename" {
            done(rename_all(&files, &new_names.borrow()));
        }
    });
    find.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn text_is_replaced_in_the_name_but_not_its_extension() {
        let rule = Rule::Replace {
            find: "IMG".into(),
            with: "東京".into(),
        };
        assert_eq!(
            apply(&rule, &names(&["IMG_01.png", "IMG.IMG", "a"]), ""),
            ["東京_01.png", "東京.IMG", "a"]
        );
    }

    #[test]
    fn text_is_added_before_or_after_the_name() {
        let after = Rule::Add {
            text: " 最終".into(),
            after: true,
        };
        let before = Rule::Add {
            text: "2026 ".into(),
            after: false,
        };
        assert_eq!(apply(&after, &names(&["報告.pdf"]), ""), ["報告 最終.pdf"]);
        assert_eq!(apply(&before, &names(&[".bashrc"]), ""), ["2026 .bashrc"]);
    }

    #[test]
    fn a_format_numbers_each_name_from_the_start() {
        let rule = |format| Rule::Format {
            base: "東京行".into(),
            format,
            start: 1,
        };
        let list = names(&["a.png", "b.jpg"]);
        assert_eq!(
            apply(&rule(Format::Index), &list, ""),
            ["東京行 1.png", "東京行 2.jpg"]
        );
        assert_eq!(
            apply(&rule(Format::Counter), &list, ""),
            ["東京行 00001.png", "東京行 00002.jpg"]
        );
        assert_eq!(
            apply(&rule(Format::Date), &list, "2026-11-17"),
            ["東京行 2026-11-17 1.png", "東京行 2026-11-17 2.jpg"]
        );
    }

    #[test]
    fn names_given_twice_or_taken_are_refused() {
        let others: HashSet<String> = ["c.png".to_string()].into();
        assert_eq!(
            refused(&names(&["a.png", "a.png", "c.png", "", "x/y", "d"]), &others),
            [true, true, true, true, true, false]
        );
    }

    #[test]
    fn two_items_can_swap_names() {
        let dir = std::env::temp_dir().join(format!("nekoland-rename-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "A").unwrap();
        std::fs::write(dir.join("b"), "B").unwrap();
        let files = [
            gio::File::for_path(dir.join("a")),
            gio::File::for_path(dir.join("b")),
        ];
        rename_all(&files, &names(&["b", "a"])).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a")).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(dir.join("b")).unwrap(), "A");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_rename_gives_the_old_names_back() {
        let dir = std::env::temp_dir().join(format!("nekoland-rename-undo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "A").unwrap();
        std::fs::write(dir.join("b"), "B").unwrap();
        let files = [
            gio::File::for_path(dir.join("a")),
            gio::File::for_path(dir.join("b")),
        ];
        // "b" goes through a hidden name; the second cannot take a name with a slash.
        assert!(rename_all(&files, &names(&["b", "c/d"])).is_err());
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["a", "b"]);
        assert_eq!(std::fs::read_to_string(dir.join("a")).unwrap(), "A");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
