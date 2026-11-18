//! The Profiles page of the settings, as Nekominal's: the profiles under the heading of the default one, each with a
//! radio that makes it the default and a button that opens its editor in place of the list; the editor has a line for
//! each field of the profile. A profile found on the system becomes the user's own the first time it is changed, and
//! taking the user's out puts the found one back.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::{gio, glib};

use super::{
    lang::tr,
    config::{self, Config, Profile},
    profiles,
    settings::{self, change},
};

const SSH: &str = "ssh";
const NEW_NAME: &str = "設定檔";
const LABEL_WIDTH: i32 = 180;
const ROW_MARGIN: i32 = 12;
const ROW_PAD: i32 = 8;
const SAME_AS_WINDOW: &str = "與視窗相同";
const NEEDS_VALUE: &str = "不能空白";
const TAKEN_NAME: &str = "已經有這個設定檔";
const BAD_VARIABLE: &str = "名稱不能有空格或 =";
const TAKEN_VARIABLE: &str = "已經有這個變數";

type Rerender = Rc<dyn Fn()>;
type Editing = Rc<RefCell<Option<String>>>;

/// The page: the editor of the profile being edited, else the list.
pub fn widgets(editing: &Editing, rerender: &Rerender) -> Vec<gtk::Widget> {
    let all = profiles::all();
    let wanted = editing.borrow().clone();
    match wanted.and_then(|name| all.iter().find(|profile| profile.name == name).cloned()) {
        Some(profile) => editor(&profile, editing, rerender),
        None => {
            editing.take();
            list(&all, editing, rerender)
        }
    }
}

fn icon(profile: &Profile) -> &'static str {
    if profile.program == SSH {
        "network-server-symbolic"
    } else {
        "utilities-terminal-symbolic"
    }
}

fn command_line(profile: &Profile) -> String {
    format!("{} {}", profile.program, profile.args).trim().to_string()
}

fn open_editor(editing: &Editing, rerender: &Rerender, name: &str) {
    editing.replace(Some(name.to_string()));
    rerender();
}

fn list(all: &[Profile], editing: &Editing, rerender: &Rerender) -> Vec<gtk::Widget> {
    let default = profiles::named_in(all, None).name;
    let plates = settings::plates();
    let mut group: Option<gtk::CheckButton> = None;
    for profile in all {
        let radio = gtk::CheckButton::builder()
            .active(profile.name == default)
            .valign(gtk::Align::Center)
            .build();
        match &group {
            Some(first) => radio.set_group(Some(first)),
            None => group = Some(radio.clone()),
        }
        let name = profile.name.clone();
        radio.connect_toggled(move |radio| {
            if radio.is_active() {
                let name = name.clone();
                change(move |config| config.default_profile = name);
            }
        });
        let row = adw::ActionRow::builder()
            .title(&profile.name)
            .subtitle(command_line(profile))
            .use_markup(false)
            .activatable_widget(&radio)
            .build();
        // libadwaita puts each prefix before the last: the logo, then the radio in front of it.
        row.add_prefix(&gtk::Image::from_icon_name(icon(profile)));
        row.add_prefix(&radio);
        let edit = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text(tr("編輯"))
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build();
        let name = profile.name.clone();
        edit.connect_clicked(glib::clone!(
            #[strong]
            editing,
            #[strong]
            rerender,
            move |_| open_editor(&editing, &rerender, &name)
        ));
        row.add_suffix(&edit);
        plates.append(&row);
    }
    let add = icon_button("list-add-symbolic", tr("新增設定檔"));
    add.connect_clicked(glib::clone!(
        #[strong]
        editing,
        #[strong]
        rerender,
        move |_| {
            let name = add_profile();
            open_editor(&editing, &rerender, &name);
        }
    ));
    let adding = settings::plates();
    adding.append(&line_row(&[add.upcast()]));
    vec![settings::heading(tr("預設設定檔")).upcast(), plates.upcast(), adding.upcast()]
}

/// A new profile of the default one's program, named the first `設定檔 N` not taken; its name.
fn add_profile() -> String {
    let all = profiles::all();
    let name = (1..)
        .map(|number| format!("{} {number}", tr(NEW_NAME)))
        .find(|name| all.iter().all(|profile| profile.name != *name))
        .unwrap_or_else(|| tr(NEW_NAME).to_string());
    let program = profiles::named_in(&all, None).program;
    let profile = Profile {
        name: name.clone(),
        program,
        ..Profile::default()
    };
    change(move |config| config.profiles.push(profile));
    name
}

pub fn icon_button(icon: &str, label: &str) -> gtk::Button {
    gtk::Button::builder()
        .child(&adw::ButtonContent::builder().icon_name(icon).label(label).build())
        .valign(gtk::Align::Center)
        .build()
}

/// A plate of widgets in a line.
pub fn line_row(widgets: &[gtk::Widget]) -> gtk::ListBoxRow {
    let line = gtk::Box::builder()
        .spacing(ROW_MARGIN)
        .margin_start(ROW_MARGIN)
        .margin_end(ROW_MARGIN)
        .margin_top(ROW_PAD)
        .margin_bottom(ROW_PAD)
        .build();
    for widget in widgets {
        line.append(widget);
    }
    gtk::ListBoxRow::builder().child(&line).activatable(false).build()
}

/// A line of the editor: its name at the left, then what edits it.
fn form_row(label: &str, widgets: &[gtk::Widget]) -> gtk::ListBoxRow {
    let name = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .width_request(LABEL_WIDTH)
        .build();
    let mut line = vec![name.upcast()];
    line.extend_from_slice(widgets);
    line_row(&line)
}

/// Changes the profile named `name` in the settings, a found one becoming the user's own first.
fn edit_config(config: &mut Config, name: &str, found: Profile, edit: impl FnOnce(&mut Profile)) {
    let index = match config.profiles.iter().position(|profile| profile.name == name) {
        Some(index) => index,
        None => {
            config.profiles.push(found);
            config.profiles.len() - 1
        }
    };
    edit(&mut config.profiles[index]);
}

fn edit(name: &str, edit: impl FnOnce(&mut Profile)) {
    let found = profiles::named(Some(name));
    let name = name.to_string();
    change(move |config| edit_config(config, &name, found, edit));
}

fn editor(profile: &Profile, editing: &Editing, rerender: &Rerender) -> Vec<gtk::Widget> {
    let header = settings::plates();
    header.append(&header_row(profile, editing, rerender));
    let fields = settings::plates();
    fields.append(&name_row(profile, editing, rerender));
    fields.append(&program_row(profile));
    fields.append(&args_row(profile));
    fields.append(&cwd_row(profile));
    fields.append(&scheme_row(profile, rerender));
    fields.append(&font_row(profile, rerender));
    let variables = settings::plates();
    env_rows(profile, &variables, rerender);
    vec![header.upcast(), fields.upcast(), variables.upcast()]
}

/// Back to the list, the name, and the deletion of the user's profile, or for a found one going back to it as found.
fn header_row(profile: &Profile, editing: &Editing, rerender: &Rerender) -> gtk::ListBoxRow {
    let back = icon_button("go-previous-symbolic", tr("所有設定檔"));
    back.add_css_class("flat");
    back.connect_clicked(glib::clone!(
        #[strong]
        editing,
        #[strong]
        rerender,
        move |_| {
            editing.take();
            rerender();
        }
    ));
    let name = gtk::Label::builder()
        .label(&profile.name)
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["heading"])
        .build();
    let mut line: Vec<gtk::Widget> = vec![back.upcast(), name.upcast()];
    let own = config::current().profiles.iter().any(|own| own.name == profile.name);
    if own {
        let found = profiles::is_found(&profile.name);
        let label = tr(if found { "還原為偵測到的設定" } else { "刪除設定檔" });
        let remove = gtk::Button::builder().label(label).valign(gtk::Align::Center).build();
        if !found {
            remove.add_css_class("destructive-action");
        }
        let name = profile.name.clone();
        remove.connect_clicked(glib::clone!(
            #[strong]
            editing,
            #[strong]
            rerender,
            move |_| {
                let name = name.clone();
                change(|config| {
                    config.profiles.retain(|own| own.name != name);
                    if !found && config.default_profile == name {
                        config.default_profile.clear();
                    }
                });
                if !found {
                    editing.take();
                }
                rerender();
            }
        ));
        line.push(remove.upcast());
    }
    line_row(&line)
}

fn name_row(profile: &Profile, editing: &Editing, rerender: &Rerender) -> gtk::ListBoxRow {
    let old = profile.name.clone();
    let field = settings::field(&profile.name, glib::clone!(
        #[strong]
        editing,
        #[strong]
        rerender,
        move |field, text| {
            let new = text.trim().to_string();
            if new == old {
                settings::mark_refused(field, None);
                return;
            }
            if new.is_empty() {
                settings::mark_refused(field, Some(NEEDS_VALUE));
                return;
            }
            if profiles::all().iter().any(|profile| profile.name == new) {
                settings::mark_refused(field, Some(TAKEN_NAME));
                return;
            }
            let found = profiles::named(Some(&old));
            let (old, renamed) = (old.clone(), new.clone());
            change(move |config| {
                edit_config(config, &old, found, |profile| profile.name = renamed.clone());
                if config.default_profile == old {
                    config.default_profile = renamed;
                }
            });
            editing.replace(Some(new));
            rerender();
        }
    ));
    field.set_hexpand(true);
    form_row(tr("名稱"), &[field.upcast()])
}

/// A field of the profile that cannot be empty, or can, as `required` says.
fn text_field(name: &str, text: &str, required: bool, set: fn(&mut Profile, String)) -> gtk::Entry {
    let name = name.to_string();
    let field = settings::field(text, move |field, text| {
        let text = text.trim().to_string();
        if required && text.is_empty() {
            settings::mark_refused(field, Some(NEEDS_VALUE));
            return;
        }
        settings::mark_refused(field, None);
        edit(&name, |profile| set(profile, text));
    });
    field.set_hexpand(true);
    field
}

/// A button that asks for a file, or a folder, and puts its path in `field`, taking it.
fn browse_button(field: &gtk::Entry, folder: bool) -> gtk::Button {
    let button = gtk::Button::builder().label(tr("瀏覽…")).valign(gtk::Align::Center).build();
    button.connect_clicked(glib::clone!(
        #[weak]
        field,
        move |button| {
            let dialog = gtk::FileDialog::new();
            let window = button.root().and_downcast::<gtk::Window>();
            let put = move |result: Result<gio::File, glib::Error>| match result {
                Ok(file) => {
                    if let Some(path) = file.path() {
                        field.set_text(&path.display().to_string());
                        field.emit_activate();
                    }
                }
                Err(error) if error.matches(gtk::DialogError::Dismissed) => {}
                Err(error) => eprintln!("nekoland-terminal: could not choose a path: {error}"),
            };
            if folder {
                dialog.select_folder(window.as_ref(), gio::Cancellable::NONE, put);
            } else {
                dialog.open(window.as_ref(), gio::Cancellable::NONE, put);
            }
        }
    ));
    button
}

fn program_row(profile: &Profile) -> gtk::ListBoxRow {
    let field = text_field(&profile.name, &profile.program, true, |profile, program| profile.program = program);
    let browse = browse_button(&field, false);
    form_row(tr("程式"), &[field.upcast(), browse.upcast()])
}

fn args_row(profile: &Profile) -> gtk::ListBoxRow {
    let field = text_field(&profile.name, &profile.args, false, |profile, args| profile.args = args);
    form_row(tr("引數"), &[field.upcast()])
}

fn cwd_row(profile: &Profile) -> gtk::ListBoxRow {
    let field = text_field(&profile.name, &profile.cwd, false, |profile, cwd| profile.cwd = cwd);
    field.set_placeholder_text(Some(tr(SAME_AS_WINDOW)));
    let browse = browse_button(&field, true);
    form_row(tr("起始資料夾"), &[field.upcast(), browse.upcast()])
}

/// The window's scheme beside the name, then the schemes as tiles, as on the Appearance page.
fn scheme_row(profile: &Profile, rerender: &Rerender) -> gtk::ListBoxRow {
    let same = gtk::ToggleButton::builder()
        .label(tr(SAME_AS_WINDOW))
        .active(profile.color_scheme.is_none())
        .valign(gtk::Align::Center)
        .build();
    let name = profile.name.clone();
    same.connect_clicked(glib::clone!(
        #[strong]
        rerender,
        move |_| {
            edit(&name, |profile| profile.color_scheme = None);
            rerender();
        }
    ));
    let name = profile.name.clone();
    let tiles = settings::scheme_tiles(profile.color_scheme.as_deref(), glib::clone!(
        #[strong]
        rerender,
        move |scheme| {
            let scheme = scheme.to_string();
            edit(&name, |profile| profile.color_scheme = Some(scheme));
            rerender();
        }
    ));
    let title = gtk::Label::builder().label(tr("色彩配置")).xalign(0.0).hexpand(true).build();
    let line = gtk::Box::builder().spacing(ROW_MARGIN).build();
    line.append(&title);
    line.append(&same);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(ROW_MARGIN)
        .margin_start(ROW_MARGIN)
        .margin_end(ROW_MARGIN)
        .margin_top(ROW_PAD)
        .margin_bottom(ROW_MARGIN)
        .build();
    content.append(&line);
    content.append(&tiles);
    gtk::ListBoxRow::builder().child(&content).activatable(false).build()
}

/// The size steps as the window's does; while the profile has none, the window's shows, and a reset follows one given.
fn font_row(profile: &Profile, rerender: &Rerender) -> gtk::ListBoxRow {
    let window_size = config::current().font_size;
    let (min, max) = config::FONT_SIZE_RANGE;
    let spin = gtk::SpinButton::with_range(min, max, 1.0);
    spin.set_value(profile.font_size.unwrap_or(window_size));
    spin.set_valign(gtk::Align::Center);
    let name = profile.name.clone();
    let own = profile.font_size.is_some();
    spin.connect_value_changed(glib::clone!(
        #[strong]
        rerender,
        move |spin| {
            let size = spin.value();
            edit(&name, |profile| profile.font_size = Some(size));
            if !own {
                rerender();
            }
        }
    ));
    let spacer = gtk::Box::builder().hexpand(true).build();
    let mut line: Vec<gtk::Widget> = vec![spacer.upcast()];
    if own {
        line.push(spin.upcast());
        let reset = gtk::Button::builder()
            .icon_name("edit-undo-symbolic")
            .tooltip_text(tr(SAME_AS_WINDOW))
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .build();
        let name = profile.name.clone();
        reset.connect_clicked(glib::clone!(
            #[strong]
            rerender,
            move |_| {
                edit(&name, |profile| profile.font_size = None);
                rerender();
            }
        ));
        line.push(reset.upcast());
    } else {
        line.push(gtk::Label::builder().label(tr(SAME_AS_WINDOW)).css_classes(["dim-label"]).build().upcast());
        line.push(spin.upcast());
    }
    form_row(tr("字型大小"), &line)
}

/// Why a variable cannot be called `key`, when it cannot.
fn variable_problem(key: &str, others: &[String]) -> Option<&'static str> {
    if key.is_empty() {
        Some(NEEDS_VALUE)
    } else if key.contains([' ', '=']) {
        Some(BAD_VARIABLE)
    } else if others.iter().any(|other| other == key) {
        Some(TAKEN_VARIABLE)
    } else {
        None
    }
}

/// The variables: a line to add one, and a line a variable, its name, its value and the button that deletes it.
fn env_rows(profile: &Profile, list: &gtk::ListBox, rerender: &Rerender) {
    let add = icon_button("list-add-symbolic", tr("新增變數"));
    let spacer = gtk::Box::builder().hexpand(true).build();
    list.append(&form_row(tr("環境變數"), &[spacer.upcast(), add.clone().upcast()]));
    for (key, value) in &profile.env {
        list.append(&variable_row(&profile.name, Some((key, value)), rerender));
    }
    let name = profile.name.clone();
    add.connect_clicked(glib::clone!(
        #[weak]
        list,
        #[strong]
        rerender,
        move |_| {
            let row = variable_row(&name, None, &rerender);
            list.append(&row);
            row.child_focus(gtk::DirectionType::TabForward);
        }
    ));
}

/// A variable, or with None one being added, which is kept once it has a name.
fn variable_row(profile: &str, variable: Option<(&String, &String)>, rerender: &Rerender) -> gtk::ListBoxRow {
    let old_key = variable.map(|(key, _)| key.clone());
    let value = variable.map(|(_, value)| value.clone()).unwrap_or_default();
    let name = profile.to_string();
    let key_field = settings::field(old_key.as_deref().unwrap_or_default(), glib::clone!(
        #[strong]
        rerender,
        #[strong]
        old_key,
        #[strong]
        value,
        move |field, text| {
            let key = text.trim().to_string();
            if old_key.as_deref() == Some(key.as_str()) {
                return;
            }
            let others: Vec<String> = profiles::named(Some(&name)).env.into_keys().collect();
            if let Some(problem) = variable_problem(&key, &others) {
                settings::mark_refused(field, Some(problem));
                return;
            }
            let (old_key, value) = (old_key.clone(), value.clone());
            edit(&name, |profile| {
                if let Some(old_key) = &old_key {
                    profile.env.remove(old_key);
                }
                profile.env.insert(key, value);
            });
            rerender();
        }
    ));
    let name = profile.to_string();
    let value_field = settings::field(&value, glib::clone!(
        #[strong]
        old_key,
        move |_, text| {
            if let Some(key) = old_key.clone() {
                edit(&name, |profile| {
                    profile.env.insert(key, text);
                });
            }
        }
    ));
    value_field.set_placeholder_text(Some(tr("值")));
    value_field.set_hexpand(true);
    value_field.set_sensitive(old_key.is_some());
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text(tr("刪除"))
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();
    let name = profile.to_string();
    delete.connect_clicked(glib::clone!(
        #[strong]
        rerender,
        move |_| {
            if let Some(key) = old_key.clone() {
                edit(&name, |profile| {
                    profile.env.remove(&key);
                });
            }
            rerender();
        }
    ));
    line_row(&[key_field.upcast(), value_field.upcast(), delete.upcast()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_variable_needs_a_name_without_spaces_that_no_other_has() {
        let others = vec!["PATH".to_string()];
        assert_eq!(variable_problem("", &others), Some(NEEDS_VALUE));
        assert_eq!(variable_problem("A B", &others), Some(BAD_VARIABLE));
        assert_eq!(variable_problem("A=B", &others), Some(BAD_VARIABLE));
        assert_eq!(variable_problem("PATH", &others), Some(TAKEN_VARIABLE));
        assert_eq!(variable_problem("EDITOR", &others), None);
    }

    #[test]
    fn a_found_profile_becomes_the_users_when_changed() {
        let mut config = Config::default();
        let found = Profile { name: "bash".to_string(), program: "/bin/bash".to_string(), ..Profile::default() };
        edit_config(&mut config, "bash", found, |profile| profile.args = "-l".to_string());
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].args, "-l");
    }
}
