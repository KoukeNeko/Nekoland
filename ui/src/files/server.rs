//! 連接伺服器: an address mounted by gvfs (smb, sftp, ftp, dav and the rest), opened in
//! the tab once it is, and kept among the servers used before.

use std::{path::PathBuf, rc::Rc};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{browse::EDGE, pane::Where, Window};

const RECENT: usize = 8;

fn recent_path() -> PathBuf {
    glib::user_config_dir().join("nekoland/servers")
}

fn recent() -> Vec<String> {
    std::fs::read_to_string(recent_path())
        .map(|text| {
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `servers` with `used` first, each once, as many as are kept.
fn remembered(servers: &[String], used: &str) -> Vec<String> {
    std::iter::once(used.to_owned())
        .chain(servers.iter().filter(|s| *s != used).cloned())
        .take(RECENT)
        .collect()
}

fn remember(used: &str) {
    let path = recent_path();
    let text: String = remembered(&recent(), used)
        .iter()
        .map(|s| format!("{s}\n"))
        .collect();
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, text));
    if let Err(err) = written {
        eprintln!("nekoland-files: cannot save {}: {err}", path.display());
    }
}

/// An address as typed, with smb:// when no scheme was.
fn address(typed: &str) -> String {
    let typed = typed.trim();
    if typed.contains("://") {
        typed.to_owned()
    } else {
        format!("smb://{}", typed.trim_start_matches('/'))
    }
}

pub fn ask(win: &Rc<Window>) {
    let entry = gtk::Entry::builder()
        .placeholder_text("smb://nas.local/共享")
        .hexpand(true)
        .activates_default(true)
        .build();
    let servers = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let used = recent();
    for server in &used {
        let row = adw::ActionRow::builder().title(server).activatable(true).build();
        row.add_prefix(&gtk::Image::from_icon_name("network-server-symbolic"));
        servers.append(&row);
    }
    servers.set_visible(!used.is_empty());
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(EDGE)
        .width_request(crate::alert::FIELD_WIDTH)
        .build();
    content.append(&entry);
    if !used.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label("最近使用的伺服器")
                .xalign(0.0)
                .css_classes(["dim-label", "nf-small"])
                .build(),
        );
    }
    content.append(&servers);
    let alert = adw::AlertDialog::new(Some("連接伺服器"), None);
    alert.set_extra_child(Some(&content));
    alert.add_responses(&[("browse", "瀏覽網路"), ("cancel", "取消"), ("connect", "連線")]);
    alert.set_response_appearance("connect", adw::ResponseAppearance::Suggested);
    alert.set_response_enabled("connect", false);
    alert.set_default_response(Some("connect"));
    alert.set_close_response("cancel");

    entry.connect_changed(glib::clone!(
        #[weak]
        alert,
        move |entry| alert.set_response_enabled("connect", !entry.text().trim().is_empty())
    ));
    let weak = Rc::downgrade(win);
    servers.connect_row_activated(glib::clone!(
        #[weak]
        alert,
        #[strong]
        weak,
        move |_, row| {
            if let Some(server) = used.get(row.index() as usize) {
                alert.close();
                if let Some(win) = weak.upgrade() {
                    mount_and_open(&win, address(server));
                }
            }
        }
    ));
    let typed = entry.clone();
    alert.choose(&win.window, gio::Cancellable::NONE, move |response| {
        let Some(win) = weak.upgrade() else { return };
        match response.as_str() {
            "browse" => {
                if let Some(pane) = win.pane() {
                    pane.go(Where::Folder(gio::File::for_uri("network:///")), true);
                }
            }
            "connect" => mount_and_open(&win, address(&typed.text())),
            _ => {}
        }
    });
    entry.grab_focus();
}

/// Mounts the server's share, asking for a password in GTK's own way, and opens it.
pub fn mount_and_open(win: &Rc<Window>, uri: String) {
    let file = gio::File::for_uri(&uri);
    let operation = gtk::MountOperation::new(Some(&win.window));
    let weak = Rc::downgrade(win);
    let opened = file.clone();
    file.mount_enclosing_volume(
        gio::MountMountFlags::NONE,
        Some(&operation),
        gio::Cancellable::NONE,
        move |mounted| {
            let Some(win) = weak.upgrade() else { return };
            match mounted {
                Ok(()) => {}
                Err(err) if err.matches(gio::IOErrorEnum::AlreadyMounted) => {}
                Err(err) if err.matches(gio::IOErrorEnum::FailedHandled) => return,
                Err(err) => {
                    win.alert(&format!("無法連接「{uri}」"), Some(err.message()));
                    return;
                }
            }
            remember(&uri);
            win.places_changed();
            if let Some(pane) = win.pane() {
                pane.go(Where::Folder(opened), true);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_without_a_scheme_is_smb() {
        assert_eq!(address(" nas.local/共享 "), "smb://nas.local/共享");
        assert_eq!(address("sftp://neko@host/"), "sftp://neko@host/");
    }

    #[test]
    fn a_used_server_comes_first_once() {
        let servers = vec!["smb://a".to_string(), "smb://b".to_string()];
        assert_eq!(remembered(&servers, "smb://b"), ["smb://b", "smb://a"]);
    }
}
